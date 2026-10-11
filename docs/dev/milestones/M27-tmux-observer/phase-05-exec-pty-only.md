# Phase 05: exec-pty-only — no command ever runs through tmux

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** done
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

## Edit order (escalation, 2026-10-10) — read this before anything else

Run 1 spent 216 turns reading and made **zero source edits**. This is a
deletion phase, so the compiler finds the call sites for you. **Your first
edit lands within 15 turns. Do not open a file unless `cargo build` names
it, or this list does.** Do these steps in order. Run `cargo build` after
steps 1 to 4, and let its errors drive the rest.

1. `git rm -r src/daemon/background/`, and delete the line `pub mod
   background;` in `src/daemon/mod.rs`.
   Leave `src/daemon/context/mod.rs`'s `pub mod background;` alone. It is a
   different module.
2. In `src/tmux/` (`pane.rs`, `window.rs`, `session.rs`), delete the 11
   functions named in § Spec, together with their tests.
3. In `src/daemon/executor/foreground.rs`:
   - Move `LOCAL_CMD_TIMEOUT` and `exit_status_annotation` into
     `shell_exec.rs`, and update its two `foreground::` references.
   - Delete `run_foreground` and every private helper that only it uses.
   - In `run_background`, keep the three ghost PTY branches: remote host,
     `ghost_sandbox_pty` and `ghost_local_pty`. Replace everything after the
     `ghost_local_pty` branch with a refusal: send a `Response::ToolResult`
     whose output says the command has no PTY route, and name the
     unresolved `ssh_target` when there is one. Return it as
     `ToolCallOutcome::Result`.
4. In `src/config/types.rs`:
   - Delete `uses_pty()`.
   - `validate()` warns once when the normalised backend is `"tmux"`
     ("the tmux backend was removed; commands run in PTY shells"), and keeps
     the existing invalid-value warning.
   - Delete the `uses_pty` tests in `src/config/mod.rs`.
5. `cargo build`. For every error:
   - **Delete the dead caller.** Never restore the deleted function.
   - Where a call site passed `uses_pty`, it is now always `true`. Drop the
     parameter from `ghost_remote_host`, `ghost_local_pty` and
     `ghost_sandbox_pty`.
   - In `executor/mod.rs`, the `PendingCall::Foreground` fall-through after
     the PTY branch becomes the same refusal as in step 3.
6. In `src/daemon/ghost.rs`, delete `ghost_tmux_session`, the
   `ensure_incident_session` call, `ghost_session_alive` and the per-turn
   `session_exists` check, along with their tests. In
   `src/daemon/utils/sudo.rs`, delete the `send_keys` injection branch.
   `cargo build` again.
7. `tests/doc_truth.rs`: delete the `BACKGROUND_RUN_SRC` constant and the
   test that uses it. `tests/isolation.rs`: delete the
   `ensure_incident_session` test.
8. Run `cargo clippy`. Fix only what it names.
9. Add the § Tests. Then run § End-to-end verification **once** and paste
   the output.

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

### Update — 2026-10-10 20:33 (started)

Executor dispatched for phase 05 (exec-pty-only). Status flipped to
`in-progress` here and in the M27 README phase table. Plan: make the refusal
front in `PendingCall::Foreground`/`PendingCall::Background` dispatch, delete
`foreground::run_foreground`/`run_background`'s tmux tail and the
`src/daemon/background/` module, drop the ghost incident session and the
per-turn `session_exists` check, remove the tmux sudo injection, delete the
named `src/tmux/` functions, retire `[execution] backend = "tmux"` with a
startup warning, then add the three § Tests.

### Update — 2026-10-10 (escalation)

**Chosen lever:** refined re-dispatch.
**Rationale:** NoProgressStall after 216 turns, with no source edits, so
there is no partial work to resume. The spec described *what* to delete but
not a way in. The executor tried to understand the whole call graph before
its first edit. The refinement is the § Edit order block: an ordered,
compiler-driven deletion recipe with a 15-turn first-edit rule.

### Update — 2026-10-10 (escalation 2)

**Chosen lever:** resume.
**Rationale:** Run 2 followed the edit order and did most of the phase
(−2,638 lines over 16 files, `src/daemon/background/` deleted, and
`cargo build` clean). Then it tripped NoProgressStall at turn 322 while it
was still editing. Its edits were python heredocs run through `bash`, and
the governor counts every `bash` call as read-only, so this was a false
stall.

Architect gates on the partial tree:
- build: ok.
- clippy: fails on the `include_str!` in `doc_truth.rs`, a duplicate
  `#[test]` at `executor/mod.rs:2211`, a type error in the
  `run_foreground` refusal test (`ReadHalf<SimplexStream>` is not
  `AsyncBufRead`), and `manifest_tests.rs` calling the deleted
  `related_knowledge_hints`.
- fmt: fails.
- The criterion grep still hits 10 lines.

**Scope accepted:** removing `related_knowledge_hints` (dead after
`run_foreground`) and `BG_COMMAND_MAP` in `hook.rs` (dead after
`background/`). Both were compiler-driven.

### Update — 2026-10-10 (escalation 3)

**Chosen lever:** session takeover.
**Rationale:** Third NoProgressStall on this phase (turn 387), after a
refined re-dispatch and a resume with exact guidance. This one was genuine:
after its last edit at call 266 the executor spent 115 `bash` greps looking
for a fixture to drive `run_background` for the ghost `ssh_target` test, and
made no edit. Same class after refinement is the takeover row. What remained
was small: two tests, the evidence block, the commit.

Architect gates on the partial tree before takeover:
- fmt, build, clippy: clean. Criterion grep: no hits. `background/`: gone.
- Two § Tests missing (`old_tmux_backend_config_still_loads`,
  `ghost_unresolved_ssh_target_is_refused_without_sandbox`).
- The executor's two refusal tests in `foreground.rs` **hung forever**: they
  wrote the refusal into a 64-byte `tokio::io::simplex` whose read half was
  dropped.
- `get_pane_remote_host_still_has_its_callers` failed: it pinned the
  `foreground.rs` caller that `run_foreground` took with it.
- Ten deleted tests covered code that survives: the four-test
  `refusal_gate_tests` module (`run_background`'s `hosts:` fence and
  `sudo_scripts` gate, no tmux needed), the three `exit_status_annotation`
  tests (function moved to `shell_exec.rs`), and the three `is_shell_prompt`
  tests (function lives in `shell/status.rs`). Count noted below.

### Update — 2026-10-10 (takeover complete)

**Executor:** Claude (direct), on top of the executor's run-3 tree.
**Commit:** `d6e9667e` refactor: no command runs through tmux (M27 phase-05).

What the takeover added to the executor's work:
- `old_tmux_backend_config_still_loads` (`src/config/mod.rs`): an old
  `backend = "tmux"` config parses, keeps the value, validates without
  panicking, and the executor no longer reads `execution.backend`.
- `ghost_unresolved_ssh_target_is_refused_without_sandbox`
  (`foreground.rs`): with the sandbox off, an unresolved `ssh_target` takes
  none of the three PTY branches, and the tail's refusal names the target.
  The tail's text is now the pure `no_pty_route_refusal`, so the test needs
  no config, approval or stats. Mutation check: making the function ignore
  `ssh_target` turns the test red (`FAILED. 0 passed; 1 failed`); restored.
- `chat_command_without_session_is_refused` is reachable: `refuse_foreground`
  handles a chat call with no `session_id`. Kept.
- Fixed the hang in the executor's two refusal tests (`tokio::io::sink()`
  in place of a dropped-reader `simplex`).
- `get_pane_remote_host_still_has_its_callers` now pins the one live caller
  (`prompt.rs`) and the absence of the deleted `foreground.rs` one.
- Restored the ten tests whose code survives (see escalation 3).

**Count.** `2332 − 42 + 12 = 2302` passed; ignored 13 before and after.
Taken from `cargo test -- --list` at the parent commit and at `d6e9667e`.

Moved or renamed (6 + 2, counted in both columns):
- `foreground::tests::exit_status_annotation_{nonzero_notes_code,unknown_is_silent,zero_is_silent}`
  → `shell_exec::tests::…`
- `foreground::tests::is_shell_prompt_{recognises_common_shells,rejects_commands,trims_whitespace}`
  → `shell::status::tests::…`
- `ghost_remote::tests::a_ghost_reaches_the_pty_path_only_on_a_resolved_host_with_the_pty_backend`
  → `a_ghost_reaches_the_remote_pty_only_on_a_resolved_host`
- `ghost_remote::tests::a_local_ghost_reaches_the_pty_path_only_unsandboxed_with_the_pty_backend`
  → `a_local_ghost_reaches_the_pty_only_unsandboxed_and_unresolved`

Deleted with their code (34):
- `config::tests::execution_invalid_value_falls_back_to_the_default`,
  `execution_tmux_is_the_only_opt_out` (`uses_pty`).
- `background::gc::tests::` × 8 (`gc_*`), `background::helpers::tests::`
  × 4 (`trim_*`), `background::run::tests::tmux_window_refusal_names_the_fix`.
- `foreground::run_background_resolves_ghost_script_paths` (drove a tmux
  window), `foreground::tests::looks_like_shell_prompt_*` × 7.
- `executor::tests::exclude_chat_pane_sorts_by_id`,
  `excludes_chat_pane_from_selection`, `keeps_all_panes_when_no_chat_pane`
  (`find_best_target_pane`).
- `ghost::tests::an_empty_tmux_session_is_never_checked`,
  `ghost_tmux_session_starts_no_tmux_when_tmux_is_off`.
- `manifest::tests::hints_matches_summary_text`, `related_hints_*` × 5
  (`related_knowledge_hints`).

New (4): `old_tmux_backend_config_still_loads`,
`ghost_unresolved_ssh_target_is_refused_without_sandbox`,
`chat_command_without_session_is_refused`,
`ghost_turn_without_a_pty_shell_is_refused`.

**End-to-end verification** (run once, against `d6e9667e`):

```
background_dir=1
0
old_tmux_backend_config_still_loads: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2247 filtered out; finished in 0.00s
ghost_unresolved_ssh_target_is_refused_without_sandbox: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2247 filtered out; finished in 0.00s
chat_command_without_session_is_refused: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2247 filtered out; finished in 0.04s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s
fmt_exit=0
passed=2302 failed=0
 27 files changed, 400 insertions(+), 3817 deletions(-)
```

PASTE MATCH: yes

### Review verdict — 2026-10-10

- **Verdict:** escalated
- **Bounces:** none (three escalations: refined re-dispatch, resume, takeover)
- **Executor:** GLM-5.3-Flash (runs 1–3), then Claude (direct)
- **Scope deviations:** `related_knowledge_hints` and `BG_COMMAND_MAP`
  removed (compiler-driven, accepted at escalation 2).
- **Calibration:** Run 3's stall was genuine: the executor could not find a
  way to drive `run_background` for a unit test and searched for 115 calls.
  The spec named the test's behaviour but not its seam; "extract the
  refusal text and test the predicates" would have been one sentence. The
  executor also deleted ten tests whose code survives, and wrote two tests
  that hang. A deletion phase needs a criterion that pins "tests of
  surviving code are moved, not deleted". Runs 1 and 2 were the spec gap
  and the governor bug (rexyMCP #14). Every run also had an empty task list
  (rexyMCP #15).
