# Phase 02: Prompt-Path Audit Test

**Milestone:** M6 — Verification & Hygiene
**Status:** in-progress
**Depends on:** phase-01 (done)
**Estimated diff:** ~400 lines
**Tags:** language=rust, kind=test, size=m

## Goal

Turn "the agent's prompt names a path that does not exist" from a discovery into
a red gate. Land a path extractor plus an explicit path inventory, and a test that
fails when any path literal in the shipped prompt or knowledge memories names a
path that is wrong or superseded.

This is the milestone's axis-2 gate. It is written **before** the fixes (phase 03)
deliberately: a path-audit test that has never failed is exactly the vacuous
coverage this milestone exists to eliminate. The failing run is the proof.

## Architecture references

Read before starting:

- `docs/dev/WORKFLOW.md` § "Coverage claims are inadmissible without mutation
  proof" and § "Confirm the property is observable before pinning it" — task 5
  is built around both. Phase 01 bounced on exactly this.
- `docs/dev/milestones/M6-verification-and-hygiene/README.md` § "Defect inventory"
  items 4–7 — what this gate is meant to catch.
- `docs/dev/STANDARDS.md` § 2.1 — no `unwrap()`/`expect()` in production paths.
  The extractor and inventory ship as **production code** (see task 1), so this
  applies to them, not just to the test module.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

**No test asserts anything about prompt content except that it parses.** The only
existing assertion is `src/config/mod.rs:147`:

```rust
assert!(def.is_ok(), "SRE_PROMPT_TOML must be valid TOML");
```

Syntax, never accuracy.

**The assets are embedded, not read from disk** (`src/config/seeds.rs`):

```rust
pub(crate) const SRE_PROMPT_TOML: &str = include_str!("../../assets/prompts/sre.toml");
const WEBHOOK_SETUP_MEMORY: &str = include_str!("../../assets/memory/knowledge/webhook-setup.md");
const RUNBOOK_FORMAT_MEMORY: &str = include_str!("../../assets/memory/knowledge/runbook-format.md");
// … 5 more knowledge memories, all include_str!
```

These consts are `pub(crate)` or private, so an **integration** test in `tests/`
cannot reach them. That is one reason this phase lands in `src/` (task 1).

**`src/config/load.rs` has 19 `pub fn … -> PathBuf` path constructors**
(`config_dir`, `etc_dir`, `var_run_dir`, `var_log_dir`, `pipe_log_dir`,
`pane_logs_dir`, `bin_dir`, `lib_dir`, `default_log_path`, `default_socket_path`,
`default_pid_path`, `events_path`, `events_dir`, `current_event_segment_path`,
`prompts_dir`, `sessions_dir`, and `scripts_dir` / `runbooks_dir` /
`schedules_path` inside the `impl Config` block at `:183`–`:195`).

## Two findings that determine the design — read before writing any code

Both were established while drafting this phase. Each invalidates an obvious
approach, and skipping them will produce a gate that passes while the defect
stands.

### Finding 1 — "resolves against `config::`" does **not** catch the motivating defect

The milestone's exit criterion says the test should fail when a path literal "does
not correspond to a path the `config` module constructs." Applied literally, that
criterion **passes** the very line that motivated this milestone.

`assets/prompts/sre.toml:320` names `var/log/events.jsonl`. And
`config::events_path()` still returns exactly that:

```rust
// src/config/load.rs:69
pub fn events_path() -> PathBuf {
    var_log_dir().join("events.jsonl")
}
```

It is alive, with 19 call sites — but read the name they bind it to:

```rust
// src/daemon/utils/event_log.rs:93
let legacy = crate::config::events_path();
```

It is a **legacy compatibility read**. Writes go to dated segments via
`current_event_segment_path()`. So the path resolves and is still wrong to put in
front of the agent.

**Consequence:** existence is not the property. The inventory must record
*status* — `Current` or `Legacy` — and the audit must reject `Legacy` paths in
agent-facing text. A pure existence check is a gate that cannot catch defect 4.

### Finding 2 — the known-path set cannot be derived from `config::` alone

Rust cannot enumerate a module's functions at runtime, and more importantly, real
runtime paths are built outside `config::`:

```rust
// src/session_store.rs:51 — no named config:: function for this
crate::config::config_dir().join("var/sessions")
```

`var/sessions/index.json` and `var/sessions/<name>/` are genuine, current paths
that appear in the knowledge memories, and no `config::` function produces them.

**Consequence:** the inventory is an explicit hand-maintained table. To stop it
rotting, the test checks it in **both** directions (task 3) — the second direction
is mechanisable and is what keeps the table honest.

## Spec

### 1. `src/config/path_audit.rs` — extractor and inventory, as production code

New module, declared from `src/config/mod.rs`. It must be **production code, not
`#[cfg(test)]`**: phase 04 ships an operator-facing `daemoneye audit-prompts` that
reuses it, and the milestone README is explicit that 04 must not invent a second
extractor. STANDARDS § 2.1 therefore applies — no `unwrap()` / `expect()` /
`panic!()` in anything outside the test module.

Define, with whatever names read naturally:

- `PathStatus` — `Current` or `Legacy { reason: &'static str }`.
- An inventory entry pairing a **runtime-relative** path (no `~/.daemoneye/`
  prefix, no trailing slash) with its `PathStatus` and a short `source` note
  naming what constructs it (`"config::var_log_dir()"`,
  `"session_store.rs:51"`).
- `INVENTORY: &[InventoryEntry]` — the table.
- `fn extract_path_literals(text: &str) -> Vec<String>` — task 2.
- `fn audit_text(text: &str) -> Vec<Finding>` — returns one finding per bad
  literal, each carrying the literal, the reason, and enough context to act on.
  Empty vec means clean.

### 2. Extraction — the rules, with negative cases

Path literals in these assets appear inside backticks. Extract every
backtick-delimited span, then **keep a span only if**, after trimming whitespace,
it starts with one of:

```
~/.daemoneye/    etc/    var/    bin/    lib/    memory/
runbooks/        scripts/    prompts/    agents/    sessions/
```

Everything else is discarded. These are the real spans in the assets today, and
the rule must sort them correctly:

**MUST extract** (all present in the shipped assets):

```
~/.daemoneye/                              var/log/events.jsonl
~/.daemoneye/config.toml                   var/log/daemon.log
~/.daemoneye/daemon.log                    var/log/panes/<name>.log
~/.daemoneye/events.jsonl                  var/log/pipe/<id>.log
~/.daemoneye/pane_logs/                    var/log/sessions/<id>.jsonl
~/.daemoneye/pane_logs/<win_name>.log      var/run/schedules.json
~/.daemoneye/schedules.json                var/sessions/index.json
~/.daemoneye/scripts/                      var/sessions/<name>/
~/.daemoneye/sessions/ghost-<name>-<uuid>.jsonl
~/.daemoneye/agents/<name>/briefing.md     etc/config.toml
agents/<name>/mailbox/<job_id>.json        etc/prompts/sre.toml
memory/    runbooks/    scripts/    bin/    lib/    var/run/    var/log/pipe/
```

**MUST NOT extract** — every one of these is a real backticked span in
`assets/prompts/sre.toml`, and treating any of them as a path will produce a
false failure:

```
/clear            /refresh          /limits           /limits reset
/prompt <name>    /session list     /session save <name> [desc]
/session load <name>                /approvals revoke commands
//                #!/usr/bin/env python3
[Ghost Shell Completed/Failed]
```

Note the shape of the trap: the slash-command spans **begin with `/`**, and the
shebang begins with `#!`. A naive "contains a `/`" rule matches all of them. The
leading-segment allowlist above is what separates the two populations — do not
replace it with a looser heuristic.

**Normalisation** before lookup, in this order:

1. Strip a leading `~/.daemoneye/`.
2. Drop a trailing `/`.
3. Truncate at the first segment containing `<` or `>`, dropping that segment and
   everything after it. `var/log/panes/<name>.log` → `var/log/panes`;
   `var/sessions/<name>/` → `var/sessions`; `agents/<name>/mailbox/<job_id>.json`
   → `agents`.
4. A literal that normalises to the empty string (i.e. bare `~/.daemoneye/`) is
   the runtime root itself — treat it as always valid and emit no finding.

### 3. The audit, checked in both directions

`audit_text` produces a finding when a normalised literal:

- **is not in `INVENTORY`** — reason: unknown path; either the asset is wrong or
  the inventory is missing an entry; or
- **is in `INVENTORY` with `PathStatus::Legacy`** — reason: the recorded one, e.g.
  superseded by dated segments.

Then the **completeness direction**, which is what keeps the table from rotting.
A test asserts that every path the `config::` module constructs appears in
`INVENTORY`. Call each of the 19 constructors and render it runtime-relative:

```rust
// No HOME mutation needed, and therefore no crate::test_home_guard().
// Both sides derive from the same $HOME, whatever it is, so the prefix
// always strips cleanly.
let rel = config::var_log_dir()
    .strip_prefix(config::config_dir())
    .map(|p| p.to_string_lossy().into_owned())
    .unwrap_or_default();
assert!(INVENTORY.iter().any(|e| e.path == rel), "…{rel} missing from INVENTORY");
```

**Do not** set `HOME` or take `crate::test_home_guard()` anywhere in this phase.
The `strip_prefix` trick makes it unnecessary, and adding the guard would
serialize these tests against the rest of the suite for no benefit.

### 4. Seed `INVENTORY` from the real tree

Populate it from `src/config/load.rs` plus the two known non-`config::`
constructors. Verify each against the source rather than copying this list on
faith — some are in the `impl Config` block, and getting one wrong makes the
completeness test fail for the wrong reason:

| runtime-relative | source | status |
|---|---|---|
| `etc`, `etc/prompts` | `config::etc_dir()`, `prompts_dir()` | Current |
| `etc/config.toml` | seeded by `config::seeds` | Current |
| `var/run`, `var/run/schedules.json` | `var_run_dir()`, `Config::schedules_path()` | Current |
| `var/log`, `var/log/daemon.log` | `var_log_dir()`, `default_log_path()` | Current |
| `var/log/events`, `var/log/panes`, `var/log/pipe`, `var/log/sessions` | `events_dir()`, `pane_logs_dir()`, `pipe_log_dir()`, `sessions_dir()` | Current |
| `var/sessions` | `session_store.rs:51` | Current |
| `bin`, `lib`, `memory`, `runbooks`, `scripts`, `agents` | `bin_dir()`, `lib_dir()`, `Config::scripts_dir()`, `Config::runbooks_dir()`, … | Current |
| `var/log/events.jsonl` | `config::events_path()` | **Legacy** — superseded by dated segments (`current_event_segment_path()`); retained only as a compatibility read at `event_log.rs:93` |

`lib` is currently an empty directory whose future is phase 11's decision. Record
it as `Current` here with a source note and leave it alone — re-litigating it is
out of scope.

### 5. Watch it fail, then quarantine — do not skip the failing run

The gate is expected to be **red** when first run: that is the point of ordering
02 before 03. Do this in order and record it in the completion Update Log:

1. Write the test with **no** quarantine list.
2. Run it. **Quote the actual failure output**, listing every literal it rejected
   and the reason for each.
3. Add a `PENDING_FIX: &[&str]` list to `path_audit.rs` — literals the audit
   skips — containing **exactly** the literals the failing run reported, no more.
   Comment it: each entry is a real defect owned by phase 03, and phase 03 empties
   this list.
4. Re-run. Green.

`PENDING_FIX` must be **non-empty** at the end of this phase. An empty one means
either the extractor found nothing (a broken extractor) or you fixed the assets —
which is phase 03's job, explicitly out of scope here.

If the first run comes back **green**, stop and file that in the Update Log as a
blocker rather than adjusting anything. A green first run means the extractor is
not extracting; at minimum `~/.daemoneye/config.toml`,
`~/.daemoneye/daemon.log`, `~/.daemoneye/events.jsonl`, `~/.daemoneye/pane_logs/`,
`~/.daemoneye/schedules.json`, `~/.daemoneye/sessions/…` and
`var/log/events.jsonl` are all wrong in the shipped assets today and must be
reported.

## Acceptance criteria

- [ ] `cargo fmt --all` reports no changes needed.
- [ ] `cargo build` succeeds.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits zero. Run
      it bare — piping through `tail` exits with `tail`'s status.
- [ ] `cargo test` green, with the lib count **above** the 947 baseline and
      integration at 27 + isolation at 3, unchanged. Quote every summary line.
- [ ] `grep -nE '\.(unwrap|expect)\(|panic!\(' src/config/path_audit.rs` returns
      nothing outside the `#[cfg(test)]` module.
- [ ] `grep -c 'test_home_guard\|set_var' src/config/path_audit.rs` prints `0`.
- [ ] `PENDING_FIX` is non-empty, and every entry in it appears in the failure
      output quoted in the Update Log. No entry is present that the failing run
      did not report.
- [ ] The Update Log quotes the red run from task 5 step 2 and the green run from
      step 4.

## Test plan

In `src/config/path_audit.rs`'s `#[cfg(test)]` module. Names are yours; these are
the behaviours:

- **Extraction accepts the real path spans.** Drive `extract_path_literals` over
  the literal strings in the MUST-extract list and assert each is returned.
- **Extraction rejects the slash-command and shebang spans.** Drive it over the
  MUST-NOT-extract list and assert none is returned. This is the discriminating
  half — a "contains `/`" implementation passes the first test and fails this one.
- **Normalisation collapses placeholder segments.** `var/log/panes/<name>.log` →
  `var/log/panes`, `var/sessions/<name>/` → `var/sessions`, bare
  `~/.daemoneye/` → runtime root, no finding.
- **A `Legacy` entry is reported.** Audit a synthetic string naming
  `var/log/events.jsonl` and assert a finding carrying the recorded reason — not
  merely that the vec is non-empty.
- **Inventory completeness.** Every `config::` constructor's runtime-relative
  rendering is in `INVENTORY`, per task 3.
- **The shipped assets audit clean under `PENDING_FIX`.** Drive `audit_text` over
  `SRE_PROMPT_TOML` and each knowledge memory const.

## End-to-end verification

The gate's subject is the **shipped** asset bytes, and this phase reaches them
through the `include_str!` consts rather than re-reading the files, so the unit
tests are already against the real artifact. State that in the Update Log and
show it: quote the audit running over `SRE_PROMPT_TOML` and naming a real
line from `assets/prompts/sre.toml`.

Additionally confirm the extractor is looking at the whole corpus, not one file:
report the count of literals extracted per asset, and confirm each of the seven
`assets/memory/knowledge/*.md` files was audited.

## Authorizations

- [ ] May add a new module `src/config/path_audit.rs` and declare it from
      `src/config/mod.rs`.
- [ ] May widen the visibility of the knowledge-memory consts in
      `src/config/seeds.rs` from private to `pub(crate)` **only** if the audit
      module cannot otherwise reach them. Do not change their contents.

No new dependencies. No changes to `docs/architecture.md`.

## Out of scope

- **Do not fix a single path in any asset.** Every stale literal is phase 03's.
  This phase's deliverable is the gate plus the evidence of what it caught.
  Editing `assets/prompts/sre.toml` or `assets/memory/knowledge/*.md` is a scope
  violation.
- **Do not build the `daemoneye audit-prompts` CLI command.** That is phase 04,
  and it reuses this module. Ship the library surface; ship no subcommand.
- **Do not audit `assets/memory/session/*.md` or `assets/prompts/ghost-shell.txt`.**
  Checked while drafting: they contain **zero** runtime path literals today, so
  scoping to `sre.toml` + `knowledge/*.md` loses nothing. Widening the corpus is a
  later decision.
- **Do not resolve the `lib/` question** (defect 8). Record it `Current` and move
  on; phase 11 decides whether it lives or dies.
- **Do not delete or deprecate `config::events_path()`.** It has 19 call sites and
  a live compatibility role. This phase only *labels* it `Legacy` in the inventory.
- **Do not touch `tests/harness/` or `tests/isolation.rs`.** Phase 01's harness is
  unrelated to this gate — nothing here needs a daemon or a tmux server.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-30 16:10 (started)

**Executor:** Claude executor

**Work:** Implement `src/config/path_audit.rs` — path extractor, inventory, and audit gate.

### Update — 2026-07-30 16:15 (red run — task 5 step 2)

**First test run (no PENDING_FIX) — RED, as expected.**

Findings reported by the audit:

```
SRE_PROMPT_TOML has findings:
  var/log/events.jsonl → Legacy { reason: "superseded by dated segments (current_event_segment_path); retained only as a compatibility read at event_log.rs:93" }

knowledge memory 'webhook-setup' has findings:
  ~/.daemoneye/config.toml → Unknown
  ~/.daemoneye/events.jsonl → Unknown

knowledge memory 'ghost-shell-guide' has findings:
  ~/.daemoneye/pane_logs/ → Unknown (normalised: pane_logs)
  ~/.daemoneye/daemon.log → Unknown (normalised: daemon.log)
  ~/.daemoneye/events.jsonl → Unknown (normalised: events.jsonl)
  ~/.daemoneye/sessions/ghost-<name>-<uuid>.jsonl → Unknown (normalised: sessions)
  ~/.daemoneye/pane_logs/<win_name>.log → Unknown (normalised: pane_logs)

knowledge memory 'scheduling-guide' has findings:
  ~/.daemoneye/schedules.json → Unknown (normalised: schedules.json)
```

7 unique normalised paths flagged. All are real defects: the knowledge memories use
`~/.daemoneye/`-prefixed paths that do not match the actual runtime layout (e.g.
`~/.daemoneye/config.toml` should be `etc/config.toml`; `~/.daemoneye/pane_logs/`
should be `var/log/panes/`).

### Update — 2026-07-30 16:20 (green run — task 5 step 4)

**After adding PENDING_FIX with the 7 flagged paths — GREEN.**

```
test result: ok. 955 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.29s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 27 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s
```

Lib count: 955 (above 947 baseline, +8 new tests). Integration: 27. Isolation: 3. All unchanged.

PENDING_FIX contains exactly 7 entries, each matching a finding from the red run.
