# Phase 06: error-hardening

**Milestone:** M3 — Polish & Maintenance
**Status:** review
**Depends on:** none
**Estimated diff:** ~40 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Close the three concrete error-handling gaps the M3 survey found, with **no
behavior change** beyond added diagnostics: (1) rewrite the
`memory_prompt.rs` `get_mut(...).unwrap()` as an Entry-API expression; (2) bring
the four `ai/mod.rs` circuit-breaker mutex lock sites onto the codebase's
documented `.unwrap_or_log()` poison-recovery invariant (they currently recover
silently); (3) make the five swallowed `notify_tx` sends in
`daemon/scheduled.rs` log a diagnostic on a dropped receiver instead of
discarding the `Result` silently.

This is a hardening phase. The audit of "risky production unwraps in `tmux`/`ai`
hot paths" (the M3 README row) came back **mostly clean** — see Current state.
Do not invent work beyond the three task groups below.

## Architecture references

Read before starting:

- `CLAUDE.md` → "Important Invariants" — the mutex-lock invariant: *"All mutex
  lock sites use `.unwrap_or_log()` (the `UnpoisonExt` trait from `src/util.rs`)
  to recover from poisoned locks … The trait logs an ERROR before returning the
  inner value so poison events are visible in `daemon.log`."* Task 2 brings
  `ai/mod.rs` into compliance with this.
- `docs/dev/STANDARDS.md#21-error-handling` — "Never silently swallow a `Result`
  you don't want to ignore." Task 3 applies this to the scheduler notify sends.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

The survey audited `src/tmux/` and `src/ai/` for risky production unwraps. The
findings, and the **invariant-proven unwraps that must be left ALONE**:

- `src/tmux/` is **clean**. The only two non-test unwraps are invariant-proven
  and already documented:
  - `src/tmux/ansi.rs:67` — `.expect("annotate_ansi regex is valid")` on a
    compile-time-constant `Regex`.
  - `src/tmux/ansi.rs:77` — `let m = cap.get(0).unwrap();` (capture group 0 is
    always present when `captures_iter` yields a match; carries an `INVARIANT`
    comment).
  - **Do NOT touch these — they are STANDARDS-§2.1-acceptable.** No `tmux`
    changes in this phase.
- `src/ai/mod.rs:126` — `.unwrap()` on `reqwest::Client::builder().build()`,
  carries an `INVARIANT` comment (default client config is always valid).
  **Leave it.**
- `src/ai/backends/gemini.rs:79` and `:89` — `.expect("valid regex")` on
  compile-time-constant regexes. **Leave them** (acceptable; out of scope).
- `src/ai/tools/args.rs:280` — `unreachable!("use schedule_id_event instead")` is
  a genuine can't-happen trait-method arm. **Leave it.**
- Every `.unwrap()` / `.expect()` / `panic!()` inside a `#[cfg(test)]` module
  (e.g. `ai/mod.rs:251`, the `backends/*.rs` test modules, `ai/types/wire.rs`,
  `ai/tools/dispatch.rs` test mod) is **test code — exempt by STANDARDS §2 and
  out of scope.**

The three sites this phase **does** change:

**1. `src/daemon/memory_prompt.rs:89-91`** — a redundant double-lookup:

```rust
candidate_keys.entry(info.key.clone()).or_insert(0.0);
// INVARIANT: key was just inserted via .or_insert(0.0) on the preceding line
*candidate_keys.get_mut(&info.key).unwrap() = combined;
```

**2. `src/ai/mod.rs`** — four circuit-breaker lock sites recover from poison
*silently* via `unwrap_or_else(|e| e.into_inner())` (lines 45, 54, 66, 77),
diverging from the documented `.unwrap_or_log()` invariant. The file does not
import `UnpoisonExt`. The canonical idiom is already used widely, e.g.
`src/scheduler.rs:272` — `let jobs = self.jobs.read().unwrap_or_log();`.

**3. `src/daemon/scheduled.rs`** — five `notify_tx` sends discard their `Result`
silently (lines 167, 194, 213, 251, 329), all of this shape:

```rust
if let Some(ref tx) = notify_tx {
    let _ = tx.send(Response::SystemMsg(msg));
}
```

A failed send here means the client receiver was dropped (a detached client) —
benign, but currently invisible. `job.name` is in scope throughout
`run_scheduled_job`.

## Spec

1. **Rewrite the `memory_prompt` double-lookup as an Entry expression** — in
   `src/daemon/memory_prompt.rs`, replace the three lines at 89-91 (the
   `.or_insert(0.0);` statement, the `// INVARIANT:` comment, and the
   `*candidate_keys.get_mut(&info.key).unwrap() = combined;` line) with a single
   Entry-API assignment:

   ```rust
   *candidate_keys.entry(info.key.clone()).or_insert(0.0) = combined;
   ```

   This removes the `.unwrap()` and the now-stale comment. Behavior is identical.

2. **Bring the `ai/mod.rs` circuit-breaker lock sites onto `.unwrap_or_log()`** —
   in `src/ai/mod.rs`:
   - Add `use crate::util::UnpoisonExt;` to the imports (the `use` block at the
     top of the file, lines 6-14).
   - Replace **all four** occurrences of `.unwrap_or_else(|e| e.into_inner())`
     with `.unwrap_or_log()`. Three are single-line (lines 45, 54, 77); one is
     the multi-line chain in `record_success` (lines 63-67):

     ```rust
     let prev = self
         .open_until
         .lock()
         .unwrap_or_log()
         .take();
     ```

   Leave the `#[cfg(test)]` test-module unwrap at `ai/mod.rs:251` alone (test
   code). Behavior is identical except poison events now log an ERROR.

3. **Log dropped scheduler notifications instead of swallowing them** — in
   `src/daemon/scheduled.rs`, at each of the five `notify_tx` send sites
   (lines 167, 194, 213, 251, 329), replace the swallowing `let _ = tx.send(…);`
   with a logged form. Pattern (apply to each site, keeping that site's existing
   `Response::SystemMsg(...)` argument verbatim):

   ```rust
   if let Err(e) = tx.send(Response::SystemMsg(msg)) {
       log::debug!(
           "scheduled job '{}': dropped notification (no receiver): {}",
           job.name,
           e
       );
   }
   ```

   `log::debug!` (not `warn`) — a detached client is normal operation. Keep each
   send inside its existing `if let Some(ref tx) = notify_tx { … }` guard; only
   the inner `let _ = …` line changes.

## Acceptance criteria

- [ ] `grep -n "get_mut(&info.key).unwrap()" src/daemon/memory_prompt.rs`
      produces no output.
- [ ] `grep -c "unwrap_or_else(|e| e.into_inner())" src/ai/mod.rs` prints `0`.
- [ ] `grep -c "unwrap_or_log" src/ai/mod.rs` prints `4`.
- [ ] `grep -n "let _ = tx.send" src/daemon/scheduled.rs` produces no output.
- [ ] `src/tmux/ansi.rs` is unchanged (`git diff --stat src/tmux/` is empty).
- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes (existing suite, including the circuit-breaker tests
      `circuit_breaker_*` in `src/ai/mod.rs`).

## Test plan

These three edits are **behavior-preserving hardening** — they add diagnostics
(poison ERROR log, dropped-notification DEBUG log) and replace a redundant
lookup with an equivalent expression. None introduces new observable behavior
that can be asserted hermetically (poison logging requires a poisoned mutex;
the dropped-send log requires a dropped receiver mid-`run_scheduled_job`, which
needs a live tmux window). Per STANDARDS §3.2 (pure plumbing / equivalence
rewrites) no new unit tests are required.

Coverage is held by the **existing** suite proving equivalence:

- `circuit_breaker_opens_after_threshold` and `circuit_breaker_closes_on_success`
  in `src/ai/mod.rs` — exercise all four converted lock sites (`record_failure`,
  `record_success`, `allow`, `state_str`); must still pass unchanged.

If you believe a meaningful hermetic test is possible for any of the three
changes, that is a judgment call you may make — but do not contort the
production code to make it testable, and do not add a `sleep`- or
wall-clock-dependent test (STANDARDS §3.3).

## End-to-end verification

Not applicable — phase ships no new runtime-loadable artifact. All three edits
are internal hardening on existing code paths with behavior preserved; the
acceptance greps + `cargo test` are the verification.

## Authorizations

None.

## Out of scope

- Any change under `src/tmux/` — the module is clean; the two `ansi.rs` unwraps
  are invariant-proven and must stay.
- The invariant-proven unwraps named in Current state (`ai/mod.rs:126`,
  `gemini.rs:79`/`:89`, `args.rs:280`) and all `#[cfg(test)]` unwraps.
- Sweeping `unwrap_or_else(|e| e.into_inner())` anywhere outside `ai/mod.rs` (the
  pattern exists only there).
- Introducing a shared `notify(...)` helper in `scheduled.rs` — keep the five
  edits inline; a helper extraction is not part of this phase.
- The two deferred orchestration `TODO(M2)` markers (phase-09) and any other M3
  backlog item.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-28 20:52 (complete)

**Summary:** Implemented all three error-hardening tasks: (1) replaced the redundant double-lookup in `memory_prompt.rs` with a single Entry-API expression; (2) brought four `ai/mod.rs` circuit-breaker mutex lock sites onto the documented `.unwrap_or_log()` invariant; (3) replaced five silently-swallowed `notify_tx` sends in `scheduled.rs` with `log::debug!` diagnostics on dropped receivers. No behavior change beyond added diagnostics.

**Acceptance criteria:** all ticked above.

**Commands:**

```
cargo fmt --all
(no output — clean)

cargo build 2>&1 | tail -20
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.72s

cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -20
    Checking daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.61s

cargo test 2>&1 | tail -30
test result: ok. 27 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

**End-to-end verification:**

N/A — phase ships no new runtime-loadable artifact. All three edits are internal hardening on existing code paths with behavior preserved; the acceptance greps + `cargo test` are the verification.

Grep verification:
- `grep -n "get_mut(&info.key).unwrap()" src/daemon/memory_prompt.rs` → no output ✓
- `grep -c "unwrap_or_else(|e| e.into_inner())" src/ai/mod.rs` → `0` ✓
- `grep -c "unwrap_or_log" src/ai/mod.rs` → `4` ✓
- `grep -n "let _ = tx.send" src/daemon/scheduled.rs` → no output ✓
- `git diff --stat src/tmux/` → empty ✓

**Files changed:**
- `src/daemon/memory_prompt.rs` — replaced double-lookup `.or_insert(); get_mut().unwrap()` with single Entry-API expression
- `src/ai/mod.rs` — added `use crate::util::UnpoisonExt;`, replaced four `unwrap_or_else(|e| e.into_inner())` with `unwrap_or_log()`
- `src/daemon/scheduled.rs` — replaced five `let _ = tx.send(…)` with `if let Err(e) = tx.send(…) { log::debug!(…) }`
- `docs/dev/milestones/M3-polish-maintenance/phase-06-error-hardening.md` — status `todo` → `review`, added completion entry
- `docs/dev/milestones/M3-polish-maintenance/README.md` — phase-06 status `todo` → `review`

**New tests:** none required (behavior-preserving hardening per STANDARDS §3.2)

**Commits:**
- `feat: harden error handling — Entry API, unwrap_or_log, logged notify sends`

**Notes for review:** None.
