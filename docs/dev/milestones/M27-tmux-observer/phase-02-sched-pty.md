# Phase 02: sched-pty — scheduled scripts run in a PTY shell; `Command` jobs refuse to fire

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** todo
**Depends on:** phase-01
**Tags:** language=rust, kind=feature, size=m

## Goal

`ActionOn::Script` scheduled jobs run today in a `de-sj-*` tmux window
(`run_scheduled_job_inner` in `src/daemon/scheduled.rs`). Move them onto a
PTY shell, as chat and ghost commands already are, so a scheduled script
works with no tmux. Per PE decision 2, `ActionOn::Command` (deprecated) keeps
loading from `schedules.json` but **refuses to fire**, with a logged reason.

`ActionOn::Alert` and `ActionOn::Ghost` are unchanged.

## Spec

- **Where:** only `src/daemon/scheduled.rs` (the `Script` and `Command`
  arms of `run_scheduled_job_inner`) plus a new `Owner` variant in
  `src/shell/registry.rs`.
- **A scheduler owner.** Add an `Owner::Scheduler(String)` keyed by the job
  id, with its own `key()` prefix as the other variants have. Give it a
  per-owner cap like `Ghost`.
- **Script.** Run the resolved script in a shell owned by
  `Owner::Scheduler(job.id)` on the daemon host. Reuse the ghost path's shell
  helper: `run_ghost_remote` in `src/daemon/executor/ghost_remote.rs`, called
  with `HostId::Local`, does exactly "run in an owner's PTY shell and return
  `(output, exit_code)`". Make it reachable from `scheduled.rs` if it isn't.
  Close the owner's shells when the job ends, as the ghost exit path does
  with `close_owner_shells`. Keep the job's existing result handling: status,
  watchdog/runbook analysis, notifications and `log_event`, fed from the PTY
  result instead of the captured window.
- **Real exit codes.** A script that exits 3 records exit 3. The window path
  inferred it from the pane.
- **Command.** Do not run it. Log `scheduled_job_refused` via `log_event`
  with the job name and the reason
  `ActionOn::Command is no longer supported; migrate the job to ActionOn::Script`,
  mark the job failed, and fire the same failure notification a failed job
  gets.
- **Sandbox.** With `[sandbox] enabled`, a scheduled script must not quietly
  run on the host. Either route it through the chat sandbox's staged-script
  path, or refuse it with the existing `sandbox unavailable:`-style reason.
  The architect's preference is **refuse**: scheduled jobs are unattended
  and their policy is undecided. State which you chose in the Update Log.

Constraint: no new `tmux::` call in `scheduled.rs`. The window path for
`Script` and `Command` is deleted, not kept as a fallback.

## Tests (in `src/daemon/scheduled.rs` or alongside the helper)

- `scheduled_script_runs_in_a_scheduler_pty_shell`: a real in-process PTY
  host (see `spawn_in_process_host` in `src/daemon/executor/bg_shell.rs`
  tests) runs a test script that prints `SCHED_OK` and exits 3. The result
  carries `SCHED_OK` and exit code 3, and the shell's owner key is the
  scheduler's.
- `scheduled_command_job_refuses_to_fire`: an `ActionOn::Command` job
  produces no shell and no spawned process, and returns the refusal reason.
- `owner_scheduler_key_is_distinct`: `Owner::Scheduler("j1")` and
  `Owner::Ghost("j1")` have different `key()`s.

## Acceptance criteria

- [ ] The three tests above each report `1 passed` alone. Today each reports
      `0 passed`.
- [ ] `grep -n 'create_job_window\|kill_job_window' src/daemon/scheduled.rs`
      prints nothing. Today it prints lines.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0;
      `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports `0 failed` and exactly `passed=2303`: today's 2300
      plus the three new tests. If an existing test of the old window path
      has to go, delete it, lower the count by one per deleted test, and
      list each deletion in the Update Log.
- [ ] The Update Log entry contains the § End-to-end verification output,
      every line as printed, and a line `PASTE MATCH: yes`.

## End-to-end verification

```sh
for t in scheduled_script_runs_in_a_scheduler_pty_shell scheduled_command_job_refuses_to_fire owner_scheduler_key_is_distinct; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /"; done
grep -n 'create_job_window\|kill_job_window' src/daemon/scheduled.rs ; echo "grep_done"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff --stat
```

Each line runs **once**. Expect **only** `test result:` summary lines from
the `cargo test` lines; rexyMCP's output filter removes the individual
`test … ok` lines. Do not grep the full test output.

## Authorizations

- [x] May edit `src/daemon/scheduled.rs` and `src/shell/registry.rs`, and
      change the visibility of the shell helper in `ghost_remote.rs`.

## Out of scope

`ActionOn::Ghost` and `ActionOn::Alert`; the `de-sj-*` prefix constants
(row 05); `daemoneye schedule windows` (row 05).

## Update Log

<!-- entries appended below this line -->
