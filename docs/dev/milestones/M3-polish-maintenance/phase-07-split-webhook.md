# Phase 07: Split `webhook.rs` into cohesive submodules

**Milestone:** M3 — Polish & Maintenance
**Status:** done
**Depends on:** none
**Estimated diff:** ~90 lines of net new plumbing (a `webhook/mod.rs`, per-file
import blocks, one visibility bump). The bulk of the change is **verbatim code
relocation** — the diff tool will show ~1200 moved lines, but no logic changes.

**Tags:** language=rust, kind=refactor, size=m

## Goal

`src/webhook.rs` is a 1210-line file mixing four concerns: alert **payload
parsing** (Alertmanager / Grafana / generic → `InternalAlert`), the **HTTP
server** (Axum router, auth, request handler), and the **alert-processing
pipeline** (dedup, masking, session injection, tmux notify, watchdog/ghost
trigger, runbook AI analysis). Split it into a `webhook/` directory module with
three cohesive submodules — `parse`, `process`, `server` — **with zero behavior
change and zero consumer edits**: every existing `crate::webhook::<name>` path
keeps resolving via glob re-exports.

This is a near-pure mechanical move. The **only** non-relocation edit is one
visibility widening (`AlertStatus::as_str`: `fn` → `pub(crate) fn`) so the method
remains callable after its type and its caller land in different submodules. No
function body changes, no signature changes, no new logic.

## Architecture references

Read before starting:

- `docs/architecture.md#1-system-layers` — confirm this stays within the webhook
  ingestion concern; the split introduces no new layer crossing.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes
   (`git status` is clean; phase-06 is already committed).

## Current state

`src/webhook.rs` (1210 lines) is a single flat file declared as `pub mod
webhook;` in `src/lib.rs:15`. Consumers reference it by fully-qualified path
`crate::webhook::<name>`. The **complete external consumer set** (grep-verified)
is just three symbols — these MUST keep resolving as `crate::webhook::<name>`:

- `crate::webhook::start` — `src/daemon/mod.rs:662`
- `crate::webhook::inject_ghost_event` — `src/daemon/scheduled.rs:13`,
  `src/daemon/stream.rs:959`/`:974`, `src/daemon/executor/knowledge/ghost.rs:17`
- `crate::webhook::evaluate_watchdog_response` — `src/daemon/scheduled.rs:343`

`parse_payload`, `process_alert`, and `parse_ghost_trigger` are also `pub` /
`pub(crate)` but are used only internally + in the file's own tests.

**The call graph (grep-verified) is what makes a 3-way split clean.** Every
internal call stays *within* one of the three groups below, with exactly two
exceptions, both already `pub`:

- `server::handle_webhook` → `parse::parse_payload` (already `pub`)
- `server::handle_webhook` → `process::process_alert` (already `pub`)

In particular, `severity_rank` is called **only** by `process_alert` (lines
461–462) — *not* by any `parse_*` function — so it groups with `process`, not
`parse`. `fingerprint_from_labels` is called only by `parse_alertmanager` /
`parse_generic`, so it groups with `parse`.

**The one private cross-boundary call:** `process_alert` calls
`alert.status.as_str()` (lines 409, 417, 430). `as_str` is a **private** method
(`src/webhook.rs:59` — `fn as_str(&self)`) on `AlertStatus`. Since `AlertStatus`
lands in `parse.rs` and `process_alert` lands in `process.rs`, this method must
be widened to `pub(crate)` (Task 4). This is the only behavior-neutral edit that
is not a verbatim move.

**Fields are all `pub`** — `InternalAlert` and `WebhookState` have no private
fields, so cross-module field access needs no change.

**Module doc comment** (lines 1–11) describes the whole ingestion flow; it moves
to `webhook/mod.rs`.

## Spec

The strategy is the M2 **C5-split idiom** (README Notes → "Calibration carry-ins
from M2"): convert the file to a directory module, move each item plus its
co-located tests into a submodule, and re-export everything with `pub use
<submod>::*;` so all existing paths keep resolving. Cross-submodule references to
*types* and the two `pub` functions resolve through those glob re-exports plus a
`use super::*;` in the referencing submodule.

### 1. Create the directory module

Convert `src/webhook.rs` into a directory module:

- Create `src/webhook/mod.rs`.
- Create the three submodule files (`parse.rs`, `process.rs`, `server.rs`) per
  the tables below.
- Delete `src/webhook.rs` (its content is distributed; the file goes away).
- Leave `src/lib.rs` **unchanged** — `pub mod webhook;` already resolves to
  `webhook/mod.rs`.

`src/webhook/mod.rs` contains the module doc comment (moved verbatim from the top
of `webhook.rs`), the three module declarations, and the three glob re-exports —
**no helper code, no `use` statements**:

```rust
//! Webhook alert ingestion for DaemonEye.
//!
//! Listens on an HTTP port for alert payloads from Prometheus Alertmanager,
//! Grafana unified alerting, or a generic JSON format.  Received alerts are:
//!
//! 1. Deduplicated by fingerprint within a configurable window.
//! 2. Masked for sensitive data.
//! 3. Logged to `events.jsonl`.
//! 4. Injected into every active AI session history.
//! 5. Displayed via `tmux display-message` in all active chat panes.
//! 6. Optionally trigger runbook-based AI analysis (when a matching runbook exists).

mod parse;
mod process;
mod server;

pub use parse::*;
pub use process::*;
pub use server::*;
```

### 2. Distribute items into submodules per this table

Move each item **verbatim** (body, doc comments, attributes) into the named file.
This table is the authoritative partition — every top-level item in today's
`webhook.rs` appears exactly once.

| Submodule (`webhook/<file>`) | Items moved |
|---|---|
| `parse.rs` | `InternalAlert` (struct), `AlertStatus` (enum) **+ its `impl` block**, `fingerprint_from_labels`, `parse_alertmanager`, `parse_grafana_legacy`, `parse_generic`, `parse_payload` |
| `process.rs` | `severity_rank`, `now_secs`, `process_alert`, `inject_into_sessions`, `notify_chat_panes`, `inject_ghost_event`, `camel_to_kebab`, `parse_ghost_trigger`, `evaluate_watchdog_response`, `find_runbook_for_alert`, `maybe_analyze_alert` |
| `server.rs` | `WebhookState` (struct), `is_authorized`, `handle_webhook`, `start` |

Notes on specific placements:

- `AlertStatus`'s `impl` block (the `as_str` method) moves **with** the enum into
  `parse.rs`. (Its visibility is bumped in Task 4.)
- `WebhookState` lands in `server.rs` — it is documented as "Shared state passed
  to every Axum handler" and is constructed in `start`. `process.rs` references
  it through the re-export (see Task 3). Do not move it to `process.rs`.
- `severity_rank` lands in `process.rs` (only `process_alert` uses it).

### 3. Add the per-submodule imports

The current top-of-file `use` block (lines 13–28) is distributed by actual usage.
Add these module-level `use` statements at the top of each submodule. **The
compiler is the authority** — if a module needs an import not listed (or does not
need one listed), adjust to satisfy `cargo build`; do not add imports beyond what
compilation requires, and do not "tidy" fully-qualified `crate::…` paths into
`use`s.

`parse.rs` (self-contained — defines its own types, calls no sibling):

```rust
use std::collections::HashMap;

use serde_json::Value;
```

`process.rs` (references `InternalAlert` / `AlertStatus` from `parse` and
`WebhookState` from `server` via the re-exports → needs `use super::*;`):

```rust
use std::sync::Arc;

use crate::ai::{AiEvent, Message};
use crate::config::Config;
use crate::daemon::ghost::GhostManager;
use crate::daemon::session::{SessionStore, append_session_message};
use crate::daemon::utils::{UnpoisonExt, fire_notification, log_event};

use super::*;
```

`server.rs` (calls `parse_payload` / `process_alert` and references
`InternalAlert` via the re-exports → needs `use super::*;`):

```rust
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde_json::Value;

use crate::config::Config;
use crate::daemon::session::SessionStore;

use super::*;
```

`parse.rs` does **not** get `use super::*;` — it is a leaf (it references no
sibling-submodule item). Adding an unused `use super::*;` there would trip the
unused-import lint under `-D warnings`.

> Why `use super::*;` works for the cross-module references: `super` is the
> `webhook` module (`mod.rs`), whose `pub use parse::*;` / `pub use server::*;`
> re-exports make `InternalAlert`, `AlertStatus`, `WebhookState`, `parse_payload`,
> and `process_alert` members of `webhook`. A glob `use super::*;` in a child
> submodule imports those re-exported public members. This is the same mechanism
> phase-03 relied on; here two submodules actually *use* it because they reference
> each other's types.

### 4. Widen `AlertStatus::as_str` to `pub(crate)`

In `parse.rs`, change the method signature inside the `impl AlertStatus` block
from:

```rust
fn as_str(&self) -> &'static str {
```

to:

```rust
pub(crate) fn as_str(&self) -> &'static str {
```

This is required because `process_alert` (now in `process.rs`) calls
`alert.status.as_str()`; a private method would be `E0624` across the module
boundary. `pub(crate)` is the minimal visibility that keeps the call legal and
matches the codebase's prevailing idiom for crate-internal helpers. Behavior is
identical. This is the **only** signature edit in the phase.

### 5. Relocate the unit tests next to the code they cover

`webhook.rs` ends with one `#[cfg(test)] mod tests` block (lines 842–1210). Split
it: each test (and each test-only helper) moves into a co-located
`#[cfg(test)] mod tests` in the submodule that owns the function under test
(STANDARDS §2.5). Each submodule's test module starts with `use super::*;`.

Every test references **only items from its own target submodule**, so
`use super::*;` (where `super` is that submodule) is sufficient — no test needs a
cross-module path.

| Submodule | Tests + helpers moved |
|---|---|
| `parse.rs` | helper `alertmanager_payload`; helper `grafana_legacy_payload`; `alertmanager_parses_single_alert`, `alertmanager_resolved_status`, `alertmanager_multiple_alerts`, `alertmanager_fingerprint_computed_from_labels_when_absent`, `grafana_legacy_parses_firing`, `grafana_legacy_ok_maps_to_resolved`, `generic_parses_alertname_field`, `generic_parses_name_field_fallback`, `generic_unknown_fields_uses_full_body_as_description`, `generic_resolved_status`, `fingerprint_stable_regardless_of_label_order` |
| `process.rs` | `severity_rank_ordering`, `severity_rank_case_insensitive`, `camel_to_kebab_basic`, `camel_to_kebab_already_lowercase`, `camel_to_kebab_single_word`, `camel_to_kebab_consecutive_uppercase`, `ghost_trigger_yes_detected`, `ghost_trigger_no_detected`, `ghost_trigger_case_insensitive`, `ghost_trigger_absent_returns_none`, `ghost_trigger_scans_last_occurrence`, `ghost_trigger_whitespace_trimmed`, `evaluate_ghost_trigger_yes`, `evaluate_ghost_trigger_no`, `evaluate_legacy_alert_keyword`, `evaluate_no_trigger_no_alert`, `evaluate_empty_response_no_api_error`, `evaluate_api_error_empty_response`, `evaluate_api_error_with_partial_response_uses_content` |
| `server.rs` | helper `headers_with_bearer`; `auth_empty_secret_always_allows`, `auth_correct_token_allows`, `auth_missing_header_denies`, `auth_wrong_token_denies`, `auth_token_without_bearer_prefix_denies` |

Do not rename, add, or delete any test. Move them verbatim. The `server.rs` test
module will need its own imports for the auth tests (`headers_with_bearer` builds
a `HeaderMap`); add only what compiles (e.g. `use axum::http::HeaderMap;` if
`use super::*;` does not already bring it into the test scope).

### 6. Verify no consumer file needs editing

After the move, the only files changed should be under `src/webhook/` (new files)
and the deleted `src/webhook.rs`. **No consumer file should require an edit** —
confirm with `git status`. If any consumer fails to compile, a re-export in
`mod.rs` is incomplete or a visibility is too narrow; fix the `mod.rs` re-export /
the item's visibility — do **not** edit the consumer (editing consumers means a
path was dropped, which is a regression).

## Acceptance criteria

- [ ] `src/webhook.rs` no longer exists; `src/webhook/mod.rs` plus the three
      submodule files (`parse.rs`, `process.rs`, `server.rs`) exist.
- [ ] `src/lib.rs` is unchanged (`git diff src/lib.rs` is empty).
- [ ] No file outside `src/webhook/` is modified, except the deletion of
      `src/webhook.rs`. Verify with `git status --short` (the only non-doc paths
      touched are `src/webhook/*` and the removed `src/webhook.rs`).
- [ ] `grep -rn "fn as_str" src/webhook/parse.rs` shows `pub(crate) fn as_str`.
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` leaves the tree clean.
- [ ] `cargo test` passes: the full pre-split test set still runs (same test
      names, now distributed across submodules). Spot-check that
      `cargo test --lib alertmanager_parses_single_alert`,
      `cargo test --lib severity_rank_ordering`,
      `cargo test --lib evaluate_api_error_with_partial_response_uses_content`,
      and `cargo test --lib auth_wrong_token_denies` each match and pass.
- [ ] The three external consumer paths still resolve unchanged:
      `cargo build` compiles `src/daemon/mod.rs`, `src/daemon/scheduled.rs`,
      `src/daemon/stream.rs`, and `src/daemon/executor/knowledge/ghost.rs`
      without edits to them.

## Test plan

No new tests. This phase **relocates** the existing `#[cfg(test)] mod tests`
content verbatim into per-submodule test modules. The assertion is that the same
named tests still compile and pass after the move:

- `parse.rs` test module — `alertmanager_*`, `grafana_legacy_*`, `generic_*`,
  `fingerprint_stable_regardless_of_label_order` (+ the `alertmanager_payload` /
  `grafana_legacy_payload` helpers).
- `process.rs` test module — `severity_rank_*`, `camel_to_kebab_*`,
  `ghost_trigger_*`, `evaluate_*`.
- `server.rs` test module — `auth_*` (+ the `headers_with_bearer` helper).

Run `cargo test 2>&1 | grep -E 'test result|alertmanager_|severity_rank_|ghost_trigger_|auth_'`
and confirm the counts match the pre-split run (no test silently dropped).

## End-to-end verification

Not applicable — phase ships no runtime-loadable artifact. It is a pure internal
module reorganization; the HTTP wire behavior, alert dedup, masking, and on-disk
event-log format are unchanged. Verification is the build + clippy + test
sequence above plus the `git status` check that no consumer was touched.

## Authorizations

None. (No new dependencies; `docs/architecture.md` is not touched; no files from
STANDARDS §5. `src/lib.rs` is *not* edited — `pub mod webhook;` resolves the
directory module unchanged.)

## Out of scope

- **No logic changes.** Do not refactor any function body, "improve" an
  algorithm, fix a latent bug, or change a signature — the **sole** exception is
  the `AlertStatus::as_str` visibility bump in Task 4. If you spot a bug, note it
  in the Update Log "Notes for review" and leave the code as-is.
- **No widening beyond `as_str`.** Do not bump any other item's visibility. The
  call graph confirms no other private item crosses a submodule boundary; if you
  hit an `E0624`/`E0603`, re-check the partition table — an item was placed in the
  wrong submodule. Fix the placement, not the visibility.
- **No import tidying beyond compilation needs.** Do not rewrite fully-qualified
  `crate::…` paths into `use` imports or vice-versa beyond the per-submodule
  blocks the spec names.
- **No consumer edits.** If a consumer needs editing, the re-export or visibility
  is wrong — fix `mod.rs` / the item, not the consumer.
- **No `src/lib.rs` edits.** It already resolves the directory module.
- **No test changes.** Move verbatim; do not rename, add, or remove tests.
- **No `WebhookState` field changes** and no splitting of `process.rs` further
  (e.g. carving out a separate `watchdog` module) — `process_alert` and
  `maybe_analyze_alert` are bidirectionally coupled and stay together.
- **Other large files** belong to later phases — leave them alone.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-28 22:48 (started)

**By:** executor

**Summary:** Split `src/webhook.rs` (1210 lines) into `src/webhook/` directory module with three submodules: `parse.rs`, `process.rs`, `server.rs`. Only non-verbatim edit: widened `AlertStatus::as_str` from `fn` to `pub(crate) fn` so `process.rs` can call it. Removed unused `use crate::config::Config` from `process.rs` (compiler caught it).

### Update — 2026-06-28 22:48 (complete)

**By:** executor

**Verification:**
- `cargo build` — succeeded with zero warnings
- `cargo clippy --all-targets --all-features -- -D warnings` — passed
- `cargo fmt --all` — tree clean
- `cargo test` — 825 lib tests passed, 0 failed; 27 integration tests passed, 0 failed
- Spot-check: `alertmanager_parses_single_alert`, `severity_rank_ordering`, `evaluate_api_error_with_partial_response_uses_content`, `auth_wrong_token_denies` — all match and pass
- `grep -rn "fn as_str" src/webhook/parse.rs` → `pub(crate) fn as_str` confirmed at line 34
- `git diff src/lib.rs` — empty (unchanged)
- `git status --short` — only `src/webhook/*` (new) and `src/webhook.rs` (deleted) touched outside docs

**Files changed:**
- `src/webhook.rs` — deleted
- `src/webhook/mod.rs` — new (module doc + 3 `mod` + 3 `pub use`)
- `src/webhook/parse.rs` — new (types, parsers, tests)
- `src/webhook/process.rs` — new (processing pipeline, tests)
- `src/webhook/server.rs` — new (HTTP handler, server, tests)

**End-to-end verification:** N/A — pure internal module reorganization.

**Notes for review:** Removed `use crate::config::Config` from `process.rs` since `Config` is accessed through `WebhookState.config` (already re-exported via `super::*`), not referenced directly. This is a compiler-observed unused import, not a spec-pinned import.

### Review verdict — 2026-06-28

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-PrismaAURA (rexyMCP)
- **Scope deviations:** none — the one compiler-observed `use crate::config::Config` drop in `process.rs` is within the spec's "the compiler is the authority" allowance for the per-submodule import blocks (Task 3).
- **Calibration:** none

**Independent re-run:** `cargo fmt --all -- --check` clean; `cargo build` zero warnings; `cargo clippy --all-targets --all-features -- -D warnings` passes; `cargo test` 825 lib + 27 integration passed, 0 failed. Webhook test set 38 functions (35 `#[test]`) — identical to pre-split count at `b040651`, none dropped. Spot-checks `alertmanager_parses_single_alert` (parse), `severity_rank_ordering` + `evaluate_api_error_with_partial_response_uses_content` (process), `auth_wrong_token_denies` (server) each match their new submodule and pass. `git diff src/lib.rs` empty; commit `d8aba17` touches only `src/webhook/*` (+ the `webhook.rs`→`process.rs` rename) and docs — no consumer edits. `pub(crate) fn as_str` confirmed at `parse.rs:34`. Two `unwrap()` in `server.rs` are both inside `#[cfg(test)] mod tests` (exempt).
