# Phase 05: Consolidate leaf params

**Milestone:** M3 — Polish & Maintenance
**Status:** review
**Depends on:** phase-04 (error-message-quality, done)
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=refactor, size=m

## Goal

Resolve the five low-blast `TODO(M2): consolidate params into a struct` markers by
introducing a per-function borrow-struct (the `EditArgs<'a>` idiom already in the
tree) for each, deleting the `#[allow(clippy::too_many_arguments)]` suppression and
the TODO comment in each case. Pure refactor — no behavior change. Advances the M3
exit criteria "the 7 `TODO(M2)` markers are resolved" (this phase clears 5 of 7) and
"the `too_many_arguments` suppressions removed by M3 are gone, not re-added."

The two remaining `TODO(M2)` markers — `src/daemon/server/ask.rs` and
`src/daemon/stream.rs` — are the high-arity orchestration functions and belong to a
later phase (consolidate-loop-ctx). **Do not touch them in this phase.**

## Architecture references

Read before starting:

- None required. This is an internal refactor with no architecture impact. The
  worked example below (`EditArgs`) is the whole pattern.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## The pattern to follow (worked example — read this first)

The tree already contains the exact idiom this phase generalizes. In
`src/daemon/executor/file_ops/write.rs`, the `run_edit_file` function consolidated
its tool-call data parameters into a single borrow-struct, leaving the I/O handles
and threading-context parameters positional:

```rust
// src/daemon/executor/file_ops/write.rs:4
pub struct EditArgs<'a> {
    pub id: &'a str,
    pub path: &'a str,
    pub operation: &'a str,
    pub old_string: Option<&'a str>,
    pub new_string: Option<&'a str>,
    pub content: Option<&'a str>,
    pub dest_path: Option<&'a str>,
    pub target_pane: Option<&'a str>,
}

// src/daemon/executor/file_ops/write.rs:19
pub async fn run_edit_file<W, R>(
    args: EditArgs<'_>,
    session_id: Option<&str>,     // ← threading context stays positional
    ghost_ctx: GhostCtx<'_>,      // ← context object stays positional
    tx: &mut W,                   // ← I/O handle stays positional
    rx: &mut R,                   // ← I/O handle stays positional
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let EditArgs {
        id, path, operation, old_string, new_string, content, dest_path, target_pane,
    } = args;   // ← destructure at the top; body is otherwise unchanged
    // … rest of the function unchanged …
}
```

**Do the same shape for each of the five functions below.** The rules, exactly as
`EditArgs` applies them:

- The struct holds a borrow lifetime `<'a>` and the function's **own data
  parameters** as `pub` fields, in the original parameter order.
- **`tx`, `rx`, `session_id`, and context-object refs (`artifact_ctx`, namespaces)
  stay positional** — they are not the tool's data; `EditArgs` excludes
  `session_id`/`ghost_ctx` for exactly this reason.
- The function takes the struct as its **first** parameter (`args: TheStruct<'_>`)
  and **destructures it on the first line** (`let TheStruct { .. } = args;`). The
  rest of the body is untouched.
- Delete the `#[allow(clippy::too_many_arguments)]` attribute and the
  `// TODO(M2): consolidate params into a struct` comment above each function. After
  consolidation every function is ≤ 4 args, so the lint no longer fires — leaving
  the `allow` is a clippy `allowed_attributes`/dead-allow smell and re-adding it
  later is forbidden by the M3 exit criteria.

## Current state

Five functions carry the marker (grep-verified — `grep -rn "TODO(M2)" src/`):

| # | Function | File:line | Data params | Positional (keep out of struct) |
|---|----------|-----------|-------------|----------------------------------|
| 1 | `memory::update_memory` | `src/memory.rs:271` | key, category, body, append, tags, summary, relates_to, expires, namespace | (none) |
| 2 | `session_store::save_session` | `src/session_store.rs:175` | name, current_saved_name, description, messages, turn_count, model, artifacts, force | (none) |
| 3 | `executor::file_ops::ops::run_edit` | `src/daemon/executor/file_ops/ops.rs:11` | id, path, old_string, new_string, target_pane | session_id, tx, rx |
| 4 | `executor::knowledge::memory::update_memory` | `src/daemon/executor/knowledge/memory.rs:45` | key, category, body, append, tags, summary, relates_to, expires | session_id, namespaces |
| 5 | `executor::knowledge::create_agent` | `src/daemon/executor/knowledge/agents.rs:13` | id, name, description, prompt, model, memory_namespace, max_turns, auto_approve_read_only, auto_approve_scripts | artifact_ctx, tx, rx |

**Naming gotcha — read before writing #1 and #4.** Functions #1 and #4 are *both*
named `update_memory`, and #4 (the tool handler) calls #1 (the persistence fn). If
you give both structs the same name you get a shadowing/clash in
`knowledge/memory.rs`. Use the two distinct names pinned below. The field types also
differ: #1 takes `category: MemoryCategory` and a single `namespace: &str`; #4 takes
`category: &str` (it converts via `MemoryCategory::from_str`) and keeps
`namespaces`/`session_id` positional.

## Spec

Do the five conversions **one function at a time**, in the order below. For each:
change the signature + destructure, update **every** call site listed for it, then
run `cargo build`. Only move to the next function once the build is green. Do **not**
edit several functions' signatures and then build once at the end — that is how a
wide multi-site change runs out of the verifier's retry budget mid-cascade (see
WORKFLOW.md § "Prefer additive change shapes"). Each function here is self-contained,
so finishing one fully before starting the next keeps the build green at every step.

### 1. `memory::update_memory` → `UpdateMemoryArgs<'a>`

In `src/memory.rs`: add `pub struct UpdateMemoryArgs<'a>` (directly above the
function) with fields `key: &'a str`, `category: MemoryCategory`,
`body: Option<&'a str>`, `append: bool`, `tags: Option<&'a [String]>`,
`summary: Option<&'a str>`, `relates_to: Option<&'a [String]>`,
`expires: Option<&'a str>`, `namespace: &'a str`. Change the signature to
`pub fn update_memory(args: UpdateMemoryArgs<'_>) -> Result<()>`, destructure on
line 1, drop the `allow` + TODO.

Call sites to update (6 — grep-verified):

- `src/daemon/executor/knowledge/memory.rs:64` — built inside function #4 (handled
  there; this is the persistence call #4 makes).
- `src/memory_tests.rs:257`, `:291`, `:334`, `:357`, `:379` — five test calls.

### 2. `session_store::save_session` → `SaveSessionArgs<'a>`

In `src/session_store.rs`: add `pub struct SaveSessionArgs<'a>` with fields
`name: &'a str`, `current_saved_name: Option<&'a str>`, `description: &'a str`,
`messages: &'a [Message]`, `turn_count: usize`, `model: &'a str`,
`artifacts: &'a [ArtifactRef]`, `force: bool`. Change the signature to
`pub fn save_session(args: SaveSessionArgs<'_>) -> Result<()>`, destructure on
line 1, drop the `allow` + TODO.

**This is the wide-blast one — 17 call sites.** Update all of them before building:

- Production (2): `src/main.rs:472`, `src/daemon/server/handlers.rs:516`.
- Tests (15) in `src/session_store_tests.rs`: lines `98`, `126`, `138`, `139`,
  `149`, `151`, `162`, `164`, `185`, `187`, `199`, `218`, `239`, `240`, `261`.

(`src/daemon/server/mod.rs:146` is `handle_save_session(...)`, a *different*
function — do **not** touch it.)

A representative test rewrite:
`save_session("aaa", None, "", &msgs, 1, "default", &[], false)` becomes
`save_session(SaveSessionArgs { name: "aaa", current_saved_name: None, description: "", messages: &msgs, turn_count: 1, model: "default", artifacts: &[], force: false })`.

### 3. `executor::file_ops::ops::run_edit` → `RunEditArgs<'a>`

In `src/daemon/executor/file_ops/ops.rs`: add `pub(super) struct RunEditArgs<'a>`
with fields `id: &'a str`, `path: &'a str`, `old_string: &'a str`,
`new_string: &'a str`, `target_pane: Option<&'a str>`. Note `old_string`/`new_string`
are `&str` here (not `Option`, unlike `EditArgs`). New signature:
`pub(super) async fn run_edit<W, R>(args: RunEditArgs<'_>, session_id: Option<&str>, tx: &mut W, rx: &mut R)` with the existing `where` clause. Destructure on line 1.

Call site (1): `src/daemon/executor/file_ops/write.rs:89` — currently
`super::ops::run_edit(id, path, old, new, target_pane, session_id, tx, rx)`. Rewrite
to `super::ops::run_edit(RunEditArgs { id, path, old_string: old, new_string: new, target_pane }, session_id, tx, rx)`.

### 4. `executor::knowledge::memory::update_memory` → `UpdateMemoryRequest<'a>`

In `src/daemon/executor/knowledge/memory.rs`: add `pub struct UpdateMemoryRequest<'a>`
with fields `key: &'a str`, `category: &'a str`, `body: Option<&'a str>`,
`append: bool`, `tags: Option<&'a [String]>`, `summary: Option<&'a str>`,
`relates_to: Option<&'a [String]>`, `expires: Option<&'a str>`. New signature:
`pub fn update_memory(req: UpdateMemoryRequest<'_>, session_id: Option<&str>, namespaces: &[&str]) -> String`. Destructure on line 1.

Inside the body, the call to the persistence fn (#1) becomes — note it now builds the
distinct `crate::memory::UpdateMemoryArgs` struct:

```rust
let namespace = namespaces.first().copied().unwrap_or("global");
match crate::memory::update_memory(crate::memory::UpdateMemoryArgs {
    key, category: cat, body, append, tags, summary, relates_to, expires, namespace,
}) {
    // … unchanged …
}
```

Call site (1): the dispatch arm in `src/daemon/executor/mod.rs:481`
(`PendingCall::UpdateMemory { .. } => …`). Rewrite the positional
`knowledge::update_memory(key, category, body.as_deref(), *append, …, session_id, &memory_namespaces)`
call to build `UpdateMemoryRequest { key, category, body: body.as_deref(), append: *append, tags: tags.as_deref(), summary: summary.as_deref(), relates_to: relates_to.as_deref(), expires: expires.as_deref() }` and pass `(req, session_id, &memory_namespaces)`.

### 5. `executor::knowledge::create_agent` → `CreateAgentArgs<'a>`

In `src/daemon/executor/knowledge/agents.rs`: add `pub struct CreateAgentArgs<'a>`
with fields `id: &'a str`, `name: &'a str`, `description: &'a str`,
`prompt: &'a str`, `model: Option<&'a str>`, `memory_namespace: &'a str`,
`max_turns: Option<u32>`, `auto_approve_read_only: bool`,
`auto_approve_scripts: &'a [String]`. New signature:
`pub async fn create_agent<W, R>(args: CreateAgentArgs<'_>, artifact_ctx: &ArtifactCtx<'_>, tx: &mut W, rx: &mut R)` with the existing `where` clause. Destructure on line 1.

Call site (1): the dispatch arm in `src/daemon/executor/mod.rs:572`
(`PendingCall::CreateAgent { .. } => …`). Build `CreateAgentArgs { id, name, description, prompt, model: model.as_deref(), memory_namespace, max_turns: *max_turns, auto_approve_read_only: *auto_approve_read_only, auto_approve_scripts }` and pass `(args, &artifact_ctx, tx, rx)`.

## Acceptance criteria

- [ ] `grep -rn "TODO(M2)" src/` returns exactly two lines — `src/daemon/server/ask.rs`
      and `src/daemon/stream.rs` (the orchestration functions, untouched).
- [ ] `grep -rn "too_many_arguments" src/memory.rs src/session_store.rs src/daemon/executor/file_ops/ops.rs src/daemon/executor/knowledge/memory.rs src/daemon/executor/knowledge/agents.rs`
      returns nothing.
- [ ] All five structs exist with the pinned names: `UpdateMemoryArgs`,
      `SaveSessionArgs`, `RunEditArgs`, `UpdateMemoryRequest`, `CreateAgentArgs`.
- [ ] `cargo fmt --all`, `cargo build` (zero new warnings),
      `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`
      all pass.
- [ ] No behavior change: the existing `save_session` and `update_memory` test suites
      (`session_store_tests.rs`, `memory_tests.rs`) pass unmodified except for the
      mechanical call-site rewrites.

## Test plan

No new tests. The five structs are plain data carriers with no behavior
(STANDARDS §3.2 — pure plumbing does not require a test), and the functions'
behavior is unchanged. Correctness is established by the **existing** test suites
continuing to pass after the call-site rewrites:

- `src/session_store_tests.rs` — already exercises `save_session` across 15 calls
  (save, truncate, clash, force, rename, list, delete, artifacts). These passing
  after the `SaveSessionArgs` rewrite proves behavior preservation.
- `src/memory_tests.rs` — already exercises `memory::update_memory` across 5 calls.
  These passing after the `UpdateMemoryArgs` rewrite proves behavior preservation.

If any existing test needs more than a mechanical positional→struct-literal rewrite
to compile, **stop and file a blocker** — that means the refactor changed behavior,
which it must not.

## End-to-end verification

Not applicable — phase ships no runtime-loadable artifact. This is a pure internal
signature refactor; the behavior the binary exhibits is unchanged, and the existing
test suites (run under `cargo test`) are the behavior-preservation check. Restate
this line in the completion Update Log and paste the `grep -rn "TODO(M2)" src/`
output showing only the two orchestration markers remain.

## Authorizations

None. (No new dependencies. No architecture-doc change. No files from STANDARDS §5.)

## Out of scope

- Do **not** touch the two orchestration `TODO(M2)` markers in
  `src/daemon/server/ask.rs` or `src/daemon/stream.rs` — they are a later phase.
- Do **not** consolidate any function that does not currently carry a `TODO(M2)`
  marker, even if it also has many parameters.
- Do **not** add `Default`, builder methods, or `From` impls to the new structs —
  plain `pub` fields constructed at the call site, matching `EditArgs`.
- Do **not** change `handle_save_session` in `src/daemon/server/mod.rs` — it is not
  `save_session`.
- Do **not** move `session_id`/`namespaces`/`tx`/`rx`/`artifact_ctx` into the structs
  — they stay positional, exactly as `EditArgs` keeps `session_id`/`ghost_ctx` out.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-28 19:32 (started)

**Executor:** Claude Sonnet 4.5 (claude-sonnet-4-5-20250929)

Starting phase 05: consolidate 5 leaf-param `TODO(M2)` markers into borrow-structs.

### Update — 2026-06-28 19:45 (complete)

**Status:** All 5 functions converted. Build, clippy, fmt, and tests pass.

**Verification:**
- `cargo fmt --all` — passed
- `cargo build` — zero warnings
- `cargo clippy --all-targets --all-features -- -D warnings` — passed
- `cargo test` — 27 passed, 0 failed, 2 ignored

**Grep results:**
- `grep -rn "TODO(M2)" src/` → 2 lines remain: `src/daemon/stream.rs:43` and `src/daemon/server/ask.rs:20` (the two orchestration functions, untouched per spec).
- `grep -rn "too_many_arguments" src/memory.rs src/session_store.rs src/daemon/executor/file_ops/ops.rs src/daemon/executor/knowledge/memory.rs src/daemon/executor/knowledge/agents.rs` → 0 matches (exit code 1).
- All 5 structs confirmed present: `UpdateMemoryArgs`, `SaveSessionArgs`, `RunEditArgs`, `UpdateMemoryRequest`, `CreateAgentArgs`.

**Files changed:**
- `src/memory.rs` — `UpdateMemoryArgs<'a>` struct + signature change
- `src/daemon/executor/knowledge/memory.rs` — call site updated to use `UpdateMemoryArgs`
- `src/memory_tests.rs` — 5 test call sites rewritten
- `src/session_store.rs` — `SaveSessionArgs<'a>` struct + signature change
- `src/main.rs` — call site rewritten
- `src/daemon/server/handlers.rs` — call site rewritten
- `src/session_store_tests.rs` — 15 test call sites rewritten
- `tests/integration.rs` — 2 integration test call sites rewritten
- `src/daemon/executor/file_ops/ops.rs` — `RunEditArgs<'a>` struct + signature change
- `src/daemon/executor/file_ops/write.rs` — call site rewritten
- `src/daemon/executor/knowledge/memory.rs` — `UpdateMemoryRequest<'a>` struct + signature change
- `src/daemon/executor/knowledge/mod.rs` — re-exports for `UpdateMemoryRequest` and `CreateAgentArgs`
- `src/daemon/executor/mod.rs` — 2 dispatch call sites rewritten
- `src/daemon/executor/knowledge/agents.rs` — `CreateAgentArgs<'a>` struct + signature change

**End-to-end verification:** N/A — pure internal signature refactor, no runtime-loadable artifact. Existing test suites (`memory_tests.rs`, `session_store_tests.rs`, integration tests) passed after mechanical call-site rewrites, proving behavior preservation.

**Notes for review:** None.
