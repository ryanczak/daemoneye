# Phase 05: exec-pty-only — no command ever runs through tmux

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** todo
**Depends on:** phase-02, phase-04
**Tags:** language=rust, kind=refactor, size=l

## Goal

The milestone's first exit criterion is "No execution through tmux". Most
commands already run in PTY shells. Chat foreground and background
commands, local, remote and sandboxed ghosts, and scheduled scripts all use
the PTY substrate.

What remains is the tmux **fallback**: every call the PTY branches do not
claim. Today that is `[execution] backend = "tmux"` and a ghost whose
`ssh_target` does not resolve to a configured host. These calls fall through
to `run_foreground` (send-keys into a pane) or to the tail of
`run_background` (a `de-bg-*` / `de-gs-*` window via
`run_background_in_window`). Ghosts also still create the tmux incident
session at start.

This phase deletes the fallback. Every command runs in a PTY shell, or it is
refused with a one-line reason.

Hooks, `pipe-pane`, `daemoneye notify`, the client's own tmux calls and the
read-only capture gating are phase 05b. Leave them alone here.

## Spec

- **Routing.** `PendingCall::Command` and `PendingCall::Background`
  (`src/daemon/executor/mod.rs`) no longer consult a backend switch.
  - A call that today reaches a PTY branch keeps that behaviour unchanged.
  - A call that today falls through to tmux returns a refusal
    `ToolCallOutcome::Result` naming why. It never reaches tmux.
  - A ghost with an unresolved `ssh_target` already gets this refusal while
    `[sandbox] enabled`. It now gets it in every mode.
- **Delete the tmux execution paths.**
  - The foreground send-keys path: `run_foreground` and its pane
    highlight/select/exit-status helpers.
  - The tmux tail of `run_background`. Its ghost remote, sandboxed and
    local PTY branches stay.
  - The whole `src/daemon/background/` module: window runs, `gc_bg_windows`
    and its startup call, the `de-bg-*` archive capture.
  - The tmux sudo credential injection (`send_keys` in
    `src/daemon/utils/sudo.rs`). The PTY path has its own credential round
    trip.
  - The now-unused functions in `src/tmux/` (`send_keys`,
    `create_job_window`, `kill_job_window`, `rename_window`,
    `set_remain_on_exit`, `highlight_pane`, `unhighlight_pane`,
    `select_pane`, `read_pane_exit_status`, `clear_pane_exit_status`,
    `ensure_incident_session`).
- **Ghosts start no tmux session.** `GhostManager::start_session_with_config`
  no longer ensures the incident session. The per-turn `session_exists`
  check on it goes too. A ghost's identity is its session id.
- **`[execution] backend`.** The `"tmux"` value is gone. Drop `uses_pty()`
  and its call sites. An old config that still says `backend = "tmux"` must
  still load. The daemon logs one warning at startup that the tmux backend
  was removed and commands run in PTY shells.
- **Gotchas.**
  - `shell_exec.rs` imports `foreground::LOCAL_CMD_TIMEOUT` and
    `foreground::exit_status_annotation`. Both are live PTY-path code, so
    move them and do not delete them.
  - `tests/doc_truth.rs` does `include_str!` of
    `src/daemon/background/run.rs` for a phantom-`target=` check. Point that
    check at wherever the surviving background result text now lives, or
    delete it when no such text survives.
- **Tests that exercised only a deleted path** are deleted with it. Name
  every deleted test in the Update Log.

## Tests

- `old_tmux_backend_config_still_loads`: a config with `[execution] backend
  = "tmux"` parses, and its commands route to the PTY substrate. Today the
  test does not exist.
- `ghost_unresolved_ssh_target_is_refused_without_sandbox`: with the
  sandbox off, a ghost whose `ssh_target` names no configured host gets a
  refusal result naming the target, and no tmux call is made.
- `chat_command_without_session_is_refused`: the call that today falls to
  `run_foreground` because there is no `session_id` returns a refusal
  result. Find the real shape that reaches it. If no such call can exist
  any more, say so in the Update Log and drop this test.

## Acceptance criteria

- [ ] `test -d src/daemon/background; echo $?` prints `1`. Today it prints
      `0`.
- [ ] `grep -rnwE 'send_keys|create_job_window|kill_job_window|rename_window|set_remain_on_exit|highlight_pane|unhighlight_pane|select_pane|read_pane_exit_status|clear_pane_exit_status|ensure_incident_session|run_background_in_window|run_foreground|uses_pty|gc_bg_windows' src tests`
      prints nothing. Today it prints many lines.
- [ ] Each test under § Tests reports `1 passed` alone, apart from a
      `chat_command_without_session_is_refused` that is explained as
      unreachable.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0;
      `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports `failed=0`. The baseline is `passed=2332`. The
      Update Log states the new total and explains it as `2332 − deleted +
      added`, naming every deleted and added test.
- [ ] The Update Log entry contains the § End-to-end verification output,
      every line as printed, and a line `PASTE MATCH: yes`.

## End-to-end verification

```sh
test -d src/daemon/background; echo "background_dir=$?"
grep -rnwE 'send_keys|create_job_window|kill_job_window|rename_window|set_remain_on_exit|highlight_pane|unhighlight_pane|select_pane|read_pane_exit_status|clear_pane_exit_status|ensure_incident_session|run_background_in_window|run_foreground|uses_pty|gc_bg_windows' src tests | wc -l
for t in old_tmux_backend_config_still_loads ghost_unresolved_ssh_target_is_refused_without_sandbox chat_command_without_session_is_refused; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /"; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff --stat HEAD~1
```

Each line runs **once**. The rexyMCP output filter removes the individual
`test … ok` lines, so expect only `test result:` summaries. Do not grep the
full test output.

## Authorizations

- [x] May edit or delete anything under `src/daemon/executor/`,
      `src/daemon/background/`, `src/daemon/ghost.rs`,
      `src/daemon/utils/sudo.rs`, `src/daemon/mod.rs`, `src/daemon/policy.rs`,
      `src/config/`, `src/tmux/` (only the functions named above) and
      `tests/isolation.rs`.
- [x] May edit `tests/doc_truth.rs`, only for the `BACKGROUND_RUN_SRC` check.
- [x] May edit `CLAUDE.md`, only where `tests/doc_truth.rs` fails on a
      deleted path. The full docs pass is phase 06.

## Out of scope

- `src/daemon/hook.rs`, the `set-hook` installs, `pipe-pane`,
  `cli/notify.rs`, client-side tmux calls (`send_cancel`,
  `resize_pane_width`), the `is_daemon_window` predicates, and gating
  capture/list on `$TMUX`. All of these are phase 05b.
- The prose docs. Those are phase 06.

## Update Log

<!-- entries appended below this line -->
