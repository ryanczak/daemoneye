# Phase 10a — Rehearsal fixes: chat turns in the turn log, idle-shell foreground

**Status:** todo
**Milestone:** M30 — Telemetry & dashboard
**Drafted:** 2026-10-22 (from phase 10 rehearsal 1, F1 and F2)

## Goal

Two small findings from the live rehearsal, plus one doc-comment nit. After
this phase, `daemoneye top` and `daemoneye run list` show a chat run's turn
count, and the Shells panel shows `shell prompt` for a shell whose last
waited command has finished.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.7 (`daemoneye top`: Runs show turn,
  Shells show foreground).
- `src/daemon/run/turn_log.rs` — the turn log records
  (`run_started` / `turn_started` / `note` / `run_finished`).
- `src/shell/foreground.rs` — `Foreground` and `classify_record`.

## Pre-flight

- `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`
  and `cargo test` are green at HEAD (`75365eaa`).
- The tests in `src/daemon/run/guard.rs` show how a run's turn log is
  written and read under a temp `HOME` (the `HomeGuard` there).
- The tests in `src/daemon/executor/shell_exec.rs` (for example
  `shell_exec_returns_the_real_exit_code`) show how a real PTY shell is run
  through `run_in_shell_for_test` and how its registry record is read.

## Current state

- **F1.** The chat loop (`run_conversation_loop`, `src/daemon/stream.rs`)
  counts a turn with `run_log.metrics_mut().record_turn()` before every
  model call, so the `run` record's `turns` is right (a `hostname` ask
  recorded `turns: 2`). But it writes no `turn_started` record: the chat
  turn log holds only `run_started` and `run_finished`. The ghost loop
  (`src/daemon/ghost.rs`) writes one through its private
  `emit_turn_started`. `RunSummary.turns` (`src/daemon/run/list.rs`) is the
  highest `turn_started` in the log, so `run list` and `top` show `0` for
  every chat run.
- **F2.** `ShellRecord.last_foreground` is written by exactly two paths:
  `probe_foreground` (`src/shell/session.rs`, only for a caller-named
  shell) and `read_shell_screen`. A waited command's completion — the
  daemon has just seen the END marker, so it knows the shell is back at its
  prompt — records nothing. The Shells panel renders `-` for the common
  idle shell (rehearsal: `s0 chat:… - pinky`).
- **Nit.** `src/cli/commands/top_tui.rs`: the doc comment on `run_tui`
  repeats its first line.

## Spec

1. **One `turn_started` writer.** Move the ghost's `emit_turn_started` to
   `src/daemon/run/turn_log.rs` as a `pub fn emit_turn_started(run_id:
   &RunId, turn: u32)` and re-export it from `src/daemon/run/mod.rs` like
   its neighbours; the ghost call site keeps its behaviour (it passes a
   `usize` today). The chat loop calls it immediately after
   `record_turn()`, with the tally's new `turns` value, so the log and the
   record agree: a chat run whose record says `turns: 2` has
   `turn_started` 1 and 2. Nothing else about the chat loop changes.
2. **A finished waited command records the prompt.** In
   `src/daemon/executor/shell_exec.rs`, `finish_run` is reached only with a
   marker-terminated `CommandOutcome` (the plain path and the sudo path
   both end there). There, store
   `last_foreground = Some((Foreground::Prompt, now_unix()))` on the
   shell's record through `with_shells`, as `probe_foreground` does. A
   no-op when the id is not in the registry. **Must not** be stored on the
   timeout, shell-exited, prompt-given-up or refused-before-write paths:
   those never reach `finish_run`, and the `InFlightGuard` drop is the
   wrong place because it fires on all of them.
3. **Nit.** Delete the duplicated first line of `run_tui`'s doc comment.

## Acceptance criteria

- [ ] `cargo test emit_turn_started_appends_the_turn` passes: after
      `emit_turn_started(&id, 1)` and `(&id, 2)` under a temp `HOME`, the
      run's turn log holds two `TurnStarted` records and
      `read_turn_log` reports them in order.
- [ ] `grep -c 'emit_turn_started' src/daemon/stream.rs` is at least 1,
      and `grep -c 'fn emit_turn_started' src/daemon/ghost.rs` is 0.
- [ ] `cargo test a_finished_command_leaves_the_shell_at_its_prompt`
      passes: run `echo hi` through `run_in_shell_for_test`; the shell's
      record then has `last_foreground` of `Some((Foreground::Prompt, t))`
      with `t` within the test's own wall-clock window.
- [ ] `cargo test a_busy_shell_is_not_marked_at_its_prompt` passes: a
      command refused by the busy-shell probe (the shape of
      `run_terminal_command_refuses_a_busy_named_shell`) leaves
      `last_foreground` reading `Program`, not `Prompt`.
- [ ] `grep -c 'The live loop. Enters raw mode' src/cli/commands/top_tui.rs`
      is 1.
- [ ] Preservation: `cargo test guard_` and `cargo test shells_text_` are
      all green; `cargo test` has no failures.

## Test plan

- Unit, `src/daemon/run/turn_log.rs`: `emit_turn_started_appends_the_turn`.
- Unit, `src/daemon/executor/shell_exec.rs`:
  `a_finished_command_leaves_the_shell_at_its_prompt`,
  `a_busy_shell_is_not_marked_at_its_prompt`.
- Mutation check, stated in the Update Log: remove the `last_foreground`
  write in `finish_run`, confirm the prompt test fails, restore.
- The live effect (`top` TURNS for a chat run, Shells `shell prompt`) is
  checked in phase 10's rehearsal 2, not here.

## End-to-end verification

Run with `bash`. Paste the output byte-exact into the Update Log.

```sh
A=/tmp/e2e-m30-10a.txt; : > "$A"
for t in emit_turn_started_appends_the_turn a_finished_command_leaves_the_shell_at_its_prompt a_busy_shell_is_not_marked_at_its_prompt; do cargo test "$t" 2>&1 | grep -E '^test result: ok. 1 passed' | head -1 | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
echo "stream_emits=$(grep -c 'emit_turn_started' src/daemon/stream.rs)" >> "$A"
echo "ghost_defines=$(grep -c 'fn emit_turn_started' src/daemon/ghost.rs)" >> "$A"
echo "turn_log_defines=$(grep -c 'pub fn emit_turn_started' src/daemon/run/turn_log.rs)" >> "$A"
echo "nit_lines=$(grep -c 'The live loop. Enters raw mode' src/cli/commands/top_tui.rs)" >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Expected: three `ok. 1 passed` lines, `stream_emits` ≥ 1, `ghost_defines=0`,
`turn_log_defines=1`, `nit_lines=1`, clippy finished with no warnings,
`fmt_exit=0`, `failed=0`.

## Authorizations

- Edit `src/daemon/run/turn_log.rs`, `src/daemon/run/mod.rs`,
  `src/daemon/ghost.rs` (the move and its call site only),
  `src/daemon/stream.rs` (one call after `record_turn()`),
  `src/daemon/executor/shell_exec.rs`, `src/cli/commands/top_tui.rs`.
- No new dependencies. No change to the `run` record, `RunMetrics`, the
  Shells panel renderer or the IPC types.

## Out of scope

- A `turn_started` for the scheduled wrapper run (it makes no model calls).
- `last_foreground` for `wait: false` dedicated shells and for a command
  that timed out (the shell is still busy; a later probe answers).
- Anything the rehearsal filed as an observation (O1–O5 in phase 10).

## Update Log

<!-- entries appended below this line -->
