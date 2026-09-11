# Phase 07b: a ghost shell can be stopped, and reports a RunResult

**Milestone:** M24 — Run Manager
**Status:** in-progress
**Depends on:** phase-04 (`register_run`, `watch_stop_sentinel`,
`STOP_POLL_INTERVAL`), phase-03 (`RunResult`, `Cancellation`), phase-06b
(`CancelDoor`, `take_cancel_door`) and phase-07a (the ghost's `RunLogGuard`,
`RunId` and terminal-state mapping). All **done**.
**Estimated diff:** ~330 lines incl. tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Phase-07a gave a ghost shell a run identity. It still cannot be stopped, and
its mailbox still reports a bare `MailboxStatus`. This phase closes both:

1. **`daemoneye stop <run>` stops a ghost shell.** The run registers its cancel
   signal and spawns the sentinel watcher; the turn loop checks the signal at
   the top of each turn and breaks.
2. A cancelled ghost records `RunState::Cancelled { stage, turns }`, with the
   stage naming the door — the same `CancelDoor` mark a chat turn uses.
3. **The mailbox carries a `RunResult`**, so a coordinator reading
   `await_agent_result` sees how the delegate's run actually ended, not just
   `Complete`/`Failed`.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.3, "Cooperative cancellation" row —
  "checked at turn top and inside the model-call `select!`; … filesystem
  sentinel `var/run/stop-<run>` polled at 500 ms so `daemoneye stop <run>`
  works from any terminal." **This phase does the turn-top half only** — see
  § Out of scope for why the in-turn `select!` is deferred.
- M24 README § exit criteria, "Cooperative cancellation for every run kind."

## Pre-flight

Confirm on the current tree (values as of drafting, 2026-09-11, commit
`b98740f`):

```sh
git status --porcelain | wc -l                                       # 0
cargo test --lib 2>&1 | grep '^test result'                          # 1752 passed
cargo test --lib daemon::ghost 2>&1 | grep '^test result'            # 8 passed
cargo test --lib agents::mailbox 2>&1 | grep '^test result'          # 4 passed
cargo test --lib daemon::run::result 2>&1 | grep '^test result'      # 11 passed
cargo test --lib daemon::executor::knowledge 2>&1 | grep '^test result'  # 46 passed
grep -c 'register_run' src/daemon/ghost.rs                           # 0
grep -c 'pub run:' src/agents/mailbox.rs                             # 0
```

## Current state

### The wrapper 07a built — where everything attaches

```rust
// src/daemon/ghost.rs:340-... (abridged; this is the block you edit)
    let (_runbook_name, kind_input) = with_sessions(sessions, |store| { /* … */ })
        .unwrap_or((None, ghost_run_kind(None, None, None, "")));
    let mut run_log = RunLogGuard::start(RunId::new(), kind_input);
    let mut budget_exhausted = false;
    let result = do_ghost_turn(
        session_id, sessions, config, cache, schedule_store,
        run_log.run_id(), &mut budget_exhausted,
    ).await;
    match &result {
        Ok(()) => {
            if budget_exhausted { run_log.finish(RunState::BudgetExceeded(/* … */)); }
            else { run_log.finish(RunState::Complete); }
        }
        Err(e) => { run_log.finish(RunState::HardFail(/* … */)); }
    }
    write_mailbox_on_exit(session_id, sessions, result.as_ref().err()).await;
```

**Nit to fix while you are here:** `_runbook_name` is bound and never read, so
`entry.ghost_runbook_name.clone()` inside the closure is a wasted clone. Have
the closure return the `RunKind` alone.

### The cancel wiring a chat turn uses — copy this shape

```rust
// src/daemon/stream.rs:114-128
    let (_cancel_guard, mut cancel_signal) = crate::daemon::run::register_run(run_log.run_id());
    let _active_guard;
    if let Some(sid) = session_id.as_deref() {
        _active_guard = Some(crate::daemon::run::register_active(sid, run_log.run_id()));
        tokio::spawn(crate::daemon::run::watch_stop_sentinel(
            run_log.run_id().clone(),
            crate::daemon::run::STOP_POLL_INTERVAL,
        ));
    } else {
```

A ghost takes the `register_run` + `watch_stop_sentinel` half and **not**
`register_active`: that map answers "which run is this *chat session's* turn?"
for `Request::Cancel`, and a ghost has no chat client. `daemoneye stop <run>`
is the ghost's only door.

### Reading the door mark

```rust
// src/daemon/stream.rs:32-38 — do NOT extract this into a shared helper
fn cancel_stage(run_id: &crate::daemon::run::RunId) -> String {
    crate::daemon::run::take_cancel_door(run_id)
        .map(crate::daemon::run::CancelDoor::stage)
        .unwrap_or("unmarked_cancel")
        .to_string()
}
```

Write the equivalent three lines inline in `ghost.rs`. **This is deliberate**:
`STANDARDS.md` § 2.2 says "Three similar lines are better than a generic
helper. Abstract when the *fourth* caller appears." Two callers is not four.
Do not move `cancel_stage` out of `stream.rs`, and do not touch `stream.rs`.

### The turn loop top, after 07a

```rust
// src/daemon/ghost.rs:585-589
            wrap_up_turn = true;
            *budget_exhausted = true;
        }
        turn += 1;
        emit_turn_started(run_id, turn);
```

`turn` is a `usize` local to `do_ghost_turn`. The loop opens with the
`if wrap_up_turn { … break; }` guard at `:445`.

### The mailbox record

```rust
// src/agents/mailbox.rs:19-27
pub struct MailboxResult {
    pub job_id: String,
    pub agent: String,
    pub task: String,
    pub status: MailboxStatus,
    pub result: Option<String>,
    pub error: Option<String>,
    pub completed_at: Option<u64>,
}
```

`write_mailbox` (`src/agents/mailbox.rs:45`) masks `entry.result` before the
write. `await_agent_result`
(`src/daemon/executor/knowledge/agents.rs:280`) returns
`serde_json::to_string_pretty(&entry)` — **so a new field on this struct
reaches the coordinator with no change to `agents.rs` at all.** Do not edit
that file.

## Spec

Six tasks.

### Task 1 — `GhostLoopOutcome` replaces the bare `budget_exhausted` flag

The loop must now report three things, not one. Replace the
`budget_exhausted: &mut bool` parameter 07a added with a single out-param:

```rust
/// What the ghost turn loop reports back about how it ended. `turns` is the
/// count the loop actually ran, which `RunState::Cancelled` and the
/// `RunResult` both need.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GhostLoopOutcome {
    pub turns: u32,
    pub budget_exhausted: bool,
    pub cancelled: bool,
}
```

`do_ghost_turn`'s signature takes `outcome: &mut GhostLoopOutcome` in that
slot. The existing `*budget_exhausted = true;` becomes
`outcome.budget_exhausted = true;`. **`do_ghost_turn` keeps `Result<()>` and
its four `bail!`s** — a criterion pins that.

### Task 2 — the loop observes the cancel signal

`do_ghost_turn` takes one more parameter, `cancel: &RunCancelSignal`
(`is_cancelled(&self)` needs only a shared borrow —
`src/daemon/run/cancel.rs:108`).

At the **top of the turn loop**, before the existing
`if wrap_up_turn { … break; }` guard:

```rust
        if cancel.is_cancelled() {
            log::info!("Ghost Shell {session_id}: cancelled, stopping before turn {}", turn + 1);
            outcome.cancelled = true;
            break;
        }
```

And immediately after `turn += 1;`, keep the loop's own bookkeeping current:
`outcome.turns = turn as u32;` — so a cancel observed on the *next* iteration
reports the turns that actually ran.

**Do not** add a `select!`, do not restructure the loop body, and do not touch
the inner AI-event loop at `:551`.

### Task 3 — `trigger_ghost_turn` registers both cancel doors it has

After `RunLogGuard::start`, mirroring `stream.rs` but without the active map:

- `let (_cancel_guard, cancel_signal) = register_run(run_log.run_id());`
- `tokio::spawn(watch_stop_sentinel(run_log.run_id().clone(), STOP_POLL_INTERVAL));`
- pass `&cancel_signal` into `do_ghost_turn`.

Bind the guard as `let _cancel_guard = …`, **not** `let _ = …` — the latter
drops immediately and deregisters the run before it starts. (That exact
distinction is called out in the 06b spec for the same reason.)

**No `register_active` for a ghost.** A criterion pins its absence.

### Task 4 — the cancelled terminal state, and the precedence order

Extend 07a's `match &result` to four outcomes, **in this order**:

1. `Err(e)` → `RunState::HardFail(…)` — unchanged from 07a. An error is the
   most specific outcome even if the run was also cancelled or out of budget.
2. `Ok(())` and `outcome.cancelled` → `RunState::Cancelled { stage, turns }`
   where `stage` is the door mark read via `take_cancel_door(run_log.run_id())`
   (the three inline lines from § Current state) and `turns` is
   `outcome.turns`.
3. `Ok(())` and `outcome.budget_exhausted` → `RunState::BudgetExceeded(…)` —
   unchanged from 07a.
4. otherwise → `RunState::Complete` — unchanged.

Build the `RunState` into a local **before** handing it to `run_log.finish`,
because task 5 needs the same value: `RunState` is `Clone`
(`src/daemon/run/mod.rs:302`).

### Task 5 — the mailbox carries the `RunResult`

**5a.** Add to `MailboxResult`, at the end of the struct:

```rust
    /// How the delegate's run ended. `None` for entries written before the
    /// run model reached ghost shells, and for any writer that has no run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<crate::daemon::run::RunResult>,
```

`#[serde(default)]` is what keeps **existing mailbox files readable** — they
have no `run` key. A criterion and a named negative test pin that.

**5b.** `write_mailbox` masks `entry.result` today. Extend the masking to the
run's operator-visible text, since it is the same model output by another
route: `run.summary`, and the `blocker` of `run.briefing` when present. Use
the same `crate::ai::filter::mask_sensitive` the existing line uses. Nothing
else in the `RunResult` is model-authored.

**5c.** `write_mailbox_on_exit` takes a fourth parameter,
`run: Option<RunResult>`, and sets it on the record it builds. Everything else
about that function — the `MailboxStatus` mapping, `last_content`, the task
description, the agent-name early return — is **unchanged**.

**5d.** `trigger_ghost_turn` builds the `RunResult` from the state it already
has and passes it down:

```rust
    let run_result = crate::daemon::run::RunResult::from_state(
        run_log.run_id().clone(),
        kind.clone(),
        state.clone(),
        summary,
        outcome.turns,
        crate::daemon::run::Budget::unlimited(),
    )
    .ok();
```

- `kind` is the `RunKind` computed for `RunLogGuard::start` — compute it once
  into a local and clone it for both uses.
- `summary` is the ghost's last assistant message, or `String::new()` when
  there is none. `write_mailbox_on_exit` already extracts exactly this value
  (`src/daemon/ghost.rs:42-49`); read it the same way rather than inventing a
  second definition.
- `Budget::unlimited()` (`src/daemon/run/mod.rs:213`) — **real budget
  accounting is M25**, and this phase must not invent numbers.
- `.ok()` discards the `Running` error arm, which cannot occur here because
  every state built in task 4 is terminal.

### Task 6 — capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-m24-07b.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

## Acceptance criteria

Each was run against the current tree and returns the "now" value shown.

- [ ] **The outcome struct replaced the bare flag:**
      `grep -c 'pub struct GhostLoopOutcome' src/daemon/ghost.rs` → **1** (now
      `0`), and `grep -c 'budget_exhausted: &mut bool' src/daemon/ghost.rs` →
      **0** (now `1`).
- [ ] **The loop observes cancellation:**
      `grep -c 'cancel.is_cancelled()' src/daemon/ghost.rs` → **1** (now `0`),
      and `grep -c 'outcome.cancelled' src/daemon/ghost.rs` → **2 or more**
      (now `0`) — the loop sets it, the wrapper reads it.
- [ ] **The ghost registers its run and its watcher:**
      `grep -c 'register_run' src/daemon/ghost.rs` → **1** (now `0`);
      `grep -c 'watch_stop_sentinel' src/daemon/ghost.rs` → **1** (now `0`);
      `grep -c 'STOP_POLL_INTERVAL' src/daemon/ghost.rs` → **1** (now `0`).
- [ ] **A ghost does not join the chat session→run map:**
      `grep -c 'register_active' src/daemon/ghost.rs` → **0**, unchanged (now
      `0`). *`Request::Cancel` stays chat-only; `daemoneye stop <run>` is the
      ghost's door.*
- [ ] **The cancelled state is recorded with a door-named stage:**
      `grep -c 'RunState::Cancelled' src/daemon/ghost.rs` → **1** (now `0`),
      and `grep -c 'take_cancel_door' src/daemon/ghost.rs` → **1** (now `0`).
- [ ] **`cancel_stage` was not extracted** (STANDARDS § 2.2, two callers is not
      four): `grep -c '^fn cancel_stage' src/daemon/stream.rs` → **1**,
      unchanged (now `1`), and `git diff --stat -- src/daemon/stream.rs | wc -l`
      → **0**.
- [ ] **The 760-line body was not restructured:**
      `awk '/^async fn do_ghost_turn/,/^}/' src/daemon/ghost.rs | grep -c 'anyhow::bail!'`
      → **4**, unchanged (now `4`), and
      `awk '/^async fn do_ghost_turn/,/^}/' src/daemon/ghost.rs | grep -c 'select!'`
      → **0**, unchanged (now `0`).
- [ ] **The mailbox carries the run:**
      `grep -c 'pub run: Option<crate::daemon::run::RunResult>' src/agents/mailbox.rs`
      → **1** (now `0`), and `grep -c 'serde(default' src/agents/mailbox.rs` →
      **1 or more** (now `0`).
- [ ] **The run's model-authored text is masked:**
      `awk '/pub fn write_mailbox/,/^}/' src/agents/mailbox.rs | grep -c 'mask_sensitive'`
      → **2 or more** (now `1`).
- [ ] **`agents.rs` is untouched — the field rides serde:**
      `git diff --stat -- src/daemon/executor/knowledge/agents.rs | wc -l` →
      **0**.
- [ ] **The wasted clone is gone:**
      `grep -c 'let (_runbook_name' src/daemon/ghost.rs` → **0** (now `1`).
      *Grep the binding, not the bare name: `_runbook_name` is a **substring of
      `ghost_runbook_name`**, the field 07a added, so the loose pattern matches
      three live lines that must stay.*
- [ ] **No `Budget` numbers were invented:**
      `grep -c 'Budget::unlimited()' src/daemon/ghost.rs` → **1** (now `0`),
      and `grep -c 'Budget::from_limits' src/daemon/ghost.rs` → **0**,
      unchanged (now `0`).
- [ ] **No *new* `unsafe`/`unwrap`/`expect`/`panic!` in production code:**
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/ghost.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe'`
      → **1**, unchanged (now **1** — the pre-existing
      `.expect("CostRecord serialization is infallible")` at `:863`, carrying
      an INVARIANT comment). Same check on `src/agents/mailbox.rs` → **0**,
      unchanged (now **0**).
- [ ] `cargo test --lib daemon::ghost` → **12 or more** passing, `0 failed`
      (now **8**). *Arithmetic: 8 existing + 4 in the Test plan.*
- [ ] `cargo test --lib agents::mailbox` → **7 or more** passing, `0 failed`
      (now **4**). *4 existing + 3 in the Test plan.*
- [ ] **Preservation, each must not move:** `daemon::stream` **8**,
      `daemon::run::active` **6**, `daemon::run::cancel` **13**,
      `daemon::run::result` **11**, `daemon::run::guard` **8**,
      `daemon::run::turn_log` **7**, `daemon::session` **21**,
      `daemon::executor::knowledge` **46**.
- [ ] **Nothing leaks into the developer's real HOME:**
      `ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l` → **0** and
      `ls ~/.daemoneye/var/run/ 2>/dev/null | grep -c '^stop-'` → **0**, both
      unchanged by a full suite run.
- [ ] Full lib suite **1759 or more** passing, `0 failed`, on **three
      consecutive runs** (now **1752**). *Arithmetic: 1752 + 4 + 3 = 1759.*
- [ ] All four gates pass.

## Test plan

**`src/daemon/ghost.rs` (4 new).** The first three are pure; the fourth writes
a sentinel and a turn log, so it takes `crate::test_home_guard()` + a tempdir —
reuse `crate::daemon::run::cancel::test_support::{HomeGuard, fresh_run_id}`,
exactly as `ghost_turn_log_records_a_turn_started_for_each_turn` already does.
**The cancel registry and the sentinel dir are process-global and libtest runs
in parallel — every test that registers must use its own run id.**

- `ghost_loop_outcome_defaults_to_zero_turns_and_no_flags` — `Default` gives
  `turns: 0`, both flags false. Cheap, and it pins the `Default` derive the
  wrapper relies on.
- `ghost_terminal_state_prefers_hard_fail_over_cancel_and_budget` — drive the
  task-4 precedence with a small helper (extract the `match` into a pure
  `fn ghost_terminal_state(result: &Result<()>, outcome: &GhostLoopOutcome,
  stage: String, session_id: &str) -> RunState` so it is testable without the
  loop): an `Err` with **both** flags set still yields `HardFail`.
- `ghost_terminal_state_prefers_cancel_over_budget` — `Ok(())` with both
  `cancelled` and `budget_exhausted` yields `Cancelled`, carrying the stage
  string and `outcome.turns` as its `turns`.
- `ghost_cancel_signal_stops_the_run_and_names_the_sentinel_door` — register a
  fresh run, `request_stop` its id, spawn `watch_stop_sentinel` with a short
  poll, advance the paused clock, then assert (a) the signal reports
  `is_cancelled()`, and (b) `take_cancel_door` on that id yields the
  **sentinel** door's stage, not the chat door's. *This is the property task 3
  exists for: the ghost's run and the sentinel watcher share one signal.*

**`src/agents/mailbox.rs` (3 new).**

- `mailbox_round_trips_a_run_result` — write an entry whose `run` is a
  `RunResult::complete(...)`, read it back, assert the `run` survives with its
  `status` and `turns`.
- `mailbox_reads_an_entry_written_before_the_run_field_existed` —
  **negative case, and the load-bearing one.** Write a mailbox JSON file *by
  hand* with the pre-07b key set (`job_id`, `agent`, `task`, `status`,
  `result`, `error`, `completed_at` and **no `run` key**), then `read_mailbox`
  it: the read must succeed and `run` must be `None`. Do **not** produce that
  JSON by serializing a `MailboxResult` — the point is that a file this code
  never wrote still parses.
- `mailbox_masks_the_run_summary` — put a string the filter redacts into
  `run.summary`, write, and assert the raw file on disk no longer contains it.
  Use the input the existing `masking_applied` test
  (`src/agents/mailbox.rs:186`) already proves the filter catches:

  ```rust
  result: Some("Found key: AKIAIOSFODNN7EXAMPLE in logs".to_string()),
  // …
  let content = std::fs::read_to_string(&path).unwrap();
  assert!(!content.contains("AKIAIOSFODNN7EXAMPLE"), "sensitive data should be masked");
  ```

  Do the same for the summary — same key, same on-disk assertion — so the test
  cannot pass against a filter that happens to ignore your chosen input.

## Mutation checks

Three. Use the `patch` tool to break and revert — **`sed -i`, `perl -i` and
`> file` are forbidden by the executor contract and `bash` refuses them.**
Record the red-then-green transcript for each in the Update Log, naming **the
assertion message in the failure output**, not just the test.

1. **Cancel must win over budget.** In the task-4 precedence, move the
   `budget_exhausted` arm above the `cancelled` arm →
   `ghost_terminal_state_prefers_cancel_over_budget` must go **red** showing
   `BudgetExceeded` where `Cancelled` was expected.
2. **Old mailbox files must still parse.** Remove `#[serde(default, …)]` from
   the new `run` field →
   `mailbox_reads_an_entry_written_before_the_run_field_existed` must go
   **red** on its "an entry written before the run field must still parse"
   assertion (a serde `missing field` error).
3. **The run summary must be masked.** Drop the new `mask_sensitive` call for
   `run.summary` in `write_mailbox` → `mailbox_masks_the_run_summary` must go
   **red** on its "sensitive data should be masked" assertion, with
   `AKIAIOSFODNN7EXAMPLE` present in the on-disk JSON.

## End-to-end verification

The real artifacts are two files: a mailbox JSON carrying a `RunResult`, and a
`var/run/stop-<run>` sentinel that a live watcher turns into a cancelled
signal. Sections B and C exercise both through their public readers in a
throwaway HOME.

Run from the repo root, verbatim:

```sh
A=/tmp/e2e-m24-07b.txt
: > "$A"
RUNS_BEFORE=$(ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l)
STOP_BEFORE=$(ls ~/.daemoneye/var/run/ 2>/dev/null | grep -c '^stop-')
{
echo "== A. build =="
echo "real HOME run logs BEFORE: $RUNS_BEFORE ; stop- sentinels BEFORE: $STOP_BEFORE"
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"

echo "== B. the ghost module =="
cargo test --lib daemon::ghost -- --nocapture 2>&1 \
  | sed 's/\x1b\[[0-9;]*m//g' | grep -E '^test daemon::ghost|^test result'
echo "cargo_exit=${PIPESTATUS[0]}"

echo "== C. the mailbox =="
cargo test --lib agents::mailbox -- --nocapture 2>&1 \
  | sed 's/\x1b\[[0-9;]*m//g' | grep -E '^test agents::mailbox|^test result'
echo "cargo_exit=${PIPESTATUS[0]}"

echo "== D. structural greps (each must print the stated number) =="
printf 'GhostLoopOutcome           (1): %s\n' "$(grep -c 'pub struct GhostLoopOutcome' src/daemon/ghost.rs)"
printf 'bare bool param gone       (0): %s\n' "$(grep -c 'budget_exhausted: &mut bool' src/daemon/ghost.rs)"
printf 'loop checks the signal     (1): %s\n' "$(grep -c 'cancel.is_cancelled()' src/daemon/ghost.rs)"
printf 'outcome.cancelled used   (>=2): %s\n' "$(grep -c 'outcome.cancelled' src/daemon/ghost.rs)"
printf 'ghost register_run         (1): %s\n' "$(grep -c 'register_run' src/daemon/ghost.rs)"
printf 'ghost watch_stop_sentinel  (1): %s\n' "$(grep -c 'watch_stop_sentinel' src/daemon/ghost.rs)"
printf 'ghost STOP_POLL_INTERVAL   (1): %s\n' "$(grep -c 'STOP_POLL_INTERVAL' src/daemon/ghost.rs)"
printf 'no register_active         (0): %s\n' "$(grep -c 'register_active' src/daemon/ghost.rs)"
printf 'RunState::Cancelled        (1): %s\n' "$(grep -c 'RunState::Cancelled' src/daemon/ghost.rs)"
printf 'take_cancel_door           (1): %s\n' "$(grep -c 'take_cancel_door' src/daemon/ghost.rs)"
printf 'cancel_stage NOT extracted (1): %s\n' "$(grep -c '^fn cancel_stage' src/daemon/stream.rs)"
printf 'stream.rs untouched        (0): %s\n' "$(git diff --stat -- src/daemon/stream.rs | wc -l)"
printf 'do_ghost_turn bails        (4): %s\n' "$(awk '/^async fn do_ghost_turn/,/^}/' src/daemon/ghost.rs | grep -c 'anyhow::bail!')"
printf 'do_ghost_turn no select!   (0): %s\n' "$(awk '/^async fn do_ghost_turn/,/^}/' src/daemon/ghost.rs | grep -c 'select!')"
printf 'mailbox run field          (1): %s\n' "$(grep -c 'pub run: Option<crate::daemon::run::RunResult>' src/agents/mailbox.rs)"
printf 'mailbox serde(default    (>=1): %s\n' "$(grep -c 'serde(default' src/agents/mailbox.rs)"
printf 'write_mailbox masking    (>=2): %s\n' "$(awk '/pub fn write_mailbox/,/^}/' src/agents/mailbox.rs | grep -c 'mask_sensitive')"
printf 'agents.rs untouched        (0): %s\n' "$(git diff --stat -- src/daemon/executor/knowledge/agents.rs | wc -l)"
printf 'wasted clone gone          (0): %s\n' "$(grep -c 'let (_runbook_name' src/daemon/ghost.rs)"
printf 'Budget::unlimited          (1): %s\n' "$(grep -c 'Budget::unlimited()' src/daemon/ghost.rs)"
printf 'no invented budget         (0): %s\n' "$(grep -c 'Budget::from_limits' src/daemon/ghost.rs)"
printf 'ghost unwrap/panic         (1): %s\n' "$(awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/ghost.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe')"
printf 'mailbox unwrap/panic       (0): %s\n' "$(awk '/^#\[cfg\(test\)\]/{exit} {print}' src/agents/mailbox.rs | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe')"

echo "== E. preservation: stream 8, active 6, cancel 13, result 11, guard 8, turn_log 7, session 21, knowledge 46 =="
for m in daemon::stream daemon::run::active daemon::run::cancel daemon::run::result daemon::run::guard daemon::run::turn_log daemon::session daemon::executor::knowledge; do
  printf '%-32s ' "$m"
  cargo test --lib "$m" 2>&1 | grep '^test result'
done

echo "== F. lib suite, three consecutive runs =="
for i in 1 2 3; do cargo test --lib 2>&1 | grep '^test result'; done

echo "== G. gates =="
cargo fmt --all --check 2>&1 | tail -2; echo "fmt_exit=${PIPESTATUS[0]}"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2; echo "clippy_exit=${PIPESTATUS[0]}"

echo "== H. no HOME litter from the suite =="
printf 'real HOME run logs AFTER (==BEFORE): %s\n' "$(ls ~/.daemoneye/var/log/runs/ 2>/dev/null | wc -l)"
printf 'real HOME stop- AFTER (==BEFORE): %s\n' "$(ls ~/.daemoneye/var/run/ 2>/dev/null | grep -c '^stop-')"
} >> "$A" 2>&1
cat "$A"
```

Then, as the last task, the paste self-check:

```sh
D=docs/dev/milestones/M24-run-manager/phase-07b-ghost-run-cancel-result.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m24-07b.txt
diff /tmp/pasted-m24-07b.txt /tmp/e2e-m24-07b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

The executor may modify **only**:

- `src/daemon/ghost.rs` — tasks 1, 2, 3, 4, 5c's call site, 5d and the Test
  plan. **Do not restructure `do_ghost_turn`**: the cancel check at the loop
  top, `outcome.turns = turn as u32;` after `turn += 1`, and the
  `budget_exhausted` → `outcome.budget_exhausted` rename are the only edits
  inside it.
- `src/agents/mailbox.rs` — tasks 5a, 5b and the Test plan. Do not change
  `MailboxStatus`, `mailbox_dir`, `mailbox_path` or `read_mailbox`'s signature.
- `docs/dev/milestones/M24-run-manager/phase-07b-ghost-run-cancel-result.md` —
  the Update Log.

No other file. In particular do not touch `src/daemon/stream.rs`,
`src/daemon/executor/knowledge/agents.rs`, `src/daemon/run/` (any file),
`src/daemon/session.rs`, `src/daemon/scheduled.rs` or `src/webhook/`. Two
criteria assert zero diffs on the first two.

## Out of scope

- **Cancelling a ghost *mid-turn*.** The design's "inside the model-call
  `select!`" half needs the AI-event loop at `src/daemon/ghost.rs:551`
  restructured, which the Authorizations forbid and which would put this phase
  well over one session. Turn-top cancellation is strictly better than today
  (uncancellable) and bounded by one turn; the in-turn `select!` is its own
  phase if it proves needed. **Say so in the Update Log** rather than
  attempting it.
- **`Request::Cancel` reaching a ghost.** The session→run map answers a chat
  question; a ghost has no chat client. `daemoneye stop <run>` is the door, and
  a criterion pins `register_active`'s absence.
- **Real `Budget` accounting.** `Budget::unlimited()` is a placeholder by
  instruction — turn/token/USD accounting is M25.
- **`RunKind::Scheduled`.** Phase-08.
- **Surfacing the ghost's run id** so a user can discover what to stop
  (`daemoneye runs`, the `[Ghost Shell Started]` event text). Phase-10 — until
  then the id is in `var/log/runs/` and the daemon log.
- **Extracting `cancel_stage` into a shared helper.** Two callers; STANDARDS
  § 2.2 says abstract at four.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-09-11 16:36 (started)

Executor: opus-4.6-executor
Status flipped todo → in-progress in this doc and in the milestone README
phase table. Pre-flight confirmed: clean tree at `a54c0f3`,
`daemon::ghost` 8 passed, `agents::mailbox` 4 passed,
`daemon::run::result` 11 passed, `grep -c register_run src/daemon/ghost.rs`
→ 0, `grep -c 'pub run:' src/agents/mailbox.rs` → 0.
