# Phase 13 — ghost-slot-and-sudo

**Milestone:** M36 — Chat UX and audit fixes
**Status:** todo
**Drafted:** 2026-11-07
**Depends on:** none
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=bug, size=m

## Goal

This phase fixes three findings from `FINDINGS.md`.

### B2 (bug, major): ghost runs leak their concurrency slot

`GhostManager::start_session_with_config` (`src/daemon/ghost.rs`) claims a
concurrency slot through `stats::inc_ghosts_launched`. Only two calls give it
back:

- `inc_ghosts_completed`, at the end of `do_ghost_turn`
  (`src/daemon/ghost/turn.rs`);
- `inc_ghosts_failed`, called by the callers of `trigger_ghost_turn` in their
  `Err` arms.

Three callers release in their `Err` arm:

- `src/webhook/process.rs`;
- `src/daemon/server/handlers.rs`;
- `src/daemon/stream/conversation.rs`.

Two do not:

- the scheduled job in `src/daemon/scheduled.rs`;
- the nested spawn in `turn.rs`.

A run whose future is dropped releases nothing. After three leaks,
`check_ghost_capacity` refuses every ghost until the daemon restarts.

### S6 (security, major): the sudo check covers too little

- **`GhostPolicy::is_safe`** (`src/daemon/policy.rs`) checks only the first
  token after a leading `sudo`. So `sudo ok.sh && sudo bash -c 'id'` passes
  when `ok.sh` is approved.
- **`command_has_sudo`** (`src/daemon/utils/sudo.rs`) only sees `sudo` at
  the start or after `;`, `&` or `|`. So `true\nsudo rm -rf /x` counts as
  having no sudo.

PE decision, 2026-11-01:

- A ghost command that contains sudo is auto-approved only when the whole
  command is `[sudo] <approved script> [args]`.
- Sudo is detected in every command position.

### L1: `await_agent_result` uses model-supplied names unchecked

`await_agent_result` (`src/daemon/executor/knowledge/agents.rs`) passes the
model-supplied `agent_name` and `job_id` straight into `mailbox_path`, so
`../` reaches the filesystem.

## Behaviour

### Ghost slot

- **Every run of `trigger_ghost_turn` releases exactly one slot,** however it
  ends: `Ok`, `Err`, or a dropped future.
- **The counters record the outcome.** `Ok` counts as completed; `Err` and
  drop count as failed.
- **Callers never release.** No caller of `trigger_ghost_turn`, and nothing
  in `do_ghost_turn`, releases a slot any more. Two releases would free a
  slot that another, live ghost holds.

### Sudo detection

`command_has_sudo` is true when `sudo` is a command word in any of these
positions:

- at the start;
- after `;`, `&`, `|` or a newline;
- inside `$(`, a backtick, or `(`;
- as the command `xargs` runs.

These must match:

- `true\nsudo rm -rf /x`
- `echo $(sudo cat /etc/shadow)`
- ``echo `sudo id` ``
- `(sudo id)`
- `ls | xargs sudo rm`
- `find . | xargs -0 sudo rm`
- `sudo apt install vim`, `echo hi | sudo tee /etc/hosts` and
  `cd /tmp; sudo rm -rf foo` (these match today)

These must **not** match:

- `cat /etc/sudoers`
- `ls -la /home`
- `echo pseudo`
- `visudo -c`
- `grep sudo /var/log/auth.log` (here `sudo` is an argument, not a
  command)

### `is_safe`

- **A command without sudo is safe**, as today.
- **A command with sudo is safe only when all of the following hold:**
  - after one optional leading `sudo `, its first token's basename is in
    `auto_approve_scripts`;
  - the rest of the command contains none of `;`, `&`, `|`, a newline,
    `$(`, a backtick, `(`, `)`, `<` or `>`.
- **So these are refused:**
  - `sudo ok.sh && sudo bash -c 'id'`
  - `ok.sh; sudo rm -rf /x`
  - `sudo ok.sh $(sudo id)`
  - `ok.sh\nsudo rm`
- **And these stay safe:** `sudo ok.sh --flag x` and `sudo /opt/s/ok.sh`.
  Basename matching is unchanged.

### `await_agent_result`

Before it polls, `await_agent_result` checks both names:

- **`agent_name`** must pass `validate_agent_name` (`src/agents/mod.rs`).
- **`job_id`** must pass a new `validate_job_id`. That function accepts one
  safe path component and refuses:
  - an empty string;
  - `.` or `..`;
  - a leading `.`;
  - a `/`, `\` or NUL anywhere;
  - more than 255 bytes.

  It must accept real ghost session ids, which have the form
  `ghost-<alert>-<uuid>`, and `<alert>` may contain spaces.

A failed check returns at once, without polling. The result text is
exactly one of:

- `Error: invalid agent_name: <validator message>`
- `Error: invalid job_id: <validator message>`

## Constraints

**The names below are fixed. Use them as given, and do not search the repo
for them: they do not exist yet.**

- **In `src/daemon/stats.rs`:**
  - `pub enum GhostEnd { Completed, Failed }`.
  - `pub struct GhostSlot`, a drop guard that releases exactly once.
  - `GhostSlot::held()`, which releases through `inc_ghosts_completed` or
    `inc_ghosts_failed`.
  - `GhostSlot::with_release(release: fn(GhostEnd))`, the test seam.
  - `GhostSlot::finish(self, end: GhostEnd)`.
  - `Drop`: a slot that was never finished releases as `Failed`.
- **In `trigger_ghost_turn`:**
  - Call `GhostSlot::held()` on entry.
  - Call `finish` with the outcome after `do_ghost_turn` returns.
- **In `src/agents/mailbox.rs`:** `pub fn validate_job_id(job_id: &str) ->
  anyhow::Result<()>`.
- **Test isolation:** tests must not read `GHOSTS_ACTIVE` or the global
  counters, because other tests run in parallel. Use `with_release` with a
  thread-local recorder.
- **Stale regex text:** two comments quote the old regex
  `(?:^|[;&|])\s*sudo\b`. Update both to describe the new rule:
  - `src/daemon/executor/file_ops/write.rs`;
  - `docs/design-reference.md`.

## Spec

1. **Red tests first.** Write these against the current tree, run them, and
   keep the real output.
   - **`ghost_slot_`** (in `src/daemon/stats.rs` tests). These are red only
     as compile errors.
     - `ghost_slot_finish_completed_releases_once`
     - `ghost_slot_finish_failed_releases_once`
     - `ghost_slot_drop_unfinished_releases_as_failed`
   - **`sudo_detect_`** (in `src/daemon/utils/sudo.rs` tests). These are red
     as assertion failures.
     - `sudo_detect_newline`
     - `sudo_detect_substitutions`, covering `$(`, a backtick and `(`.
     - `sudo_detect_xargs`
     - `sudo_detect_argument_is_not_sudo`, a preservation test for the
       must-not-match list. It is green today; label it as such.
   - **`sudo_whole_`** (in `src/daemon/policy_tests.rs`).
     - These are red as assertion failures:
       - `sudo_whole_chained_sudo_refused`
       - `sudo_whole_approved_then_chained_refused`
       - `sudo_whole_substitution_refused`
       - `sudo_whole_newline_refused`
     - `sudo_whole_plain_script_passes` is a preservation test.
   - **`mailbox_name_`.**
     - `mailbox_name_job_id_grammar`, in `src/agents/mailbox.rs`. It is red
       as a compile error.
     - `mailbox_name_traversal_agent_refused` and
       `mailbox_name_traversal_job_refused`, in `knowledge/agents.rs` tests
       with `timeout_secs: 1`. They are red as assertion failures after the
       timeout.
2. **Implement,** running `cargo build` after each edit:
   1. `GhostSlot`, wired into `trigger_ghost_turn`. Remove the four other
      `inc_ghosts_completed()` / `inc_ghosts_failed()` calls.
   2. `command_has_sudo`.
   3. `is_safe`.
   4. `validate_job_id` and the checks in `await_agent_result`.
   5. The two comments.
3. **Mutation check.** Delete the `GhostSlot::held()` call from
   `trigger_ghost_turn`, confirm that `cargo build` fails or a test fails,
   then restore the call. If nothing fails, say so plainly: the
   `slot_wired` criterion below is what guards the wiring.
4. **Red runs.** Add a `### Update — <date> (red runs)` entry that pastes
   the real cargo output for each group. Paste only real output.
5. **Findings.** Add a `### Findings — <date>` entry of two or three lines,
   or `none`.
6. **End-to-end.** Run § End-to-end verification once and paste its output.
7. **Commit** as
   `fix: ghost runs release their slot on every exit; whole-command sudo rule; mailbox names validated (M36 phase-13)`.

## Acceptance criteria

- [ ] `slot_wired=1` or more: `trigger_ghost_turn` claims the guard. Today
      it is 0.
- [ ] `caller_releases=0`: nothing outside `stats.rs` calls
      `inc_ghosts_completed()` or `inc_ghosts_failed()`. Today it is 4.
- [ ] `stale_regex=0`: the old regex text is gone from `src` and
      `docs/design-reference.md`. Today it is 3.
- [ ] Each test group passes in full: `ghost_slot passed=3`,
      `sudo_detect passed=4`, `sudo_whole passed=5`,
      `mailbox_name passed=3`. Today none of these tests exists.
- [ ] The red-runs entry shows the compile errors and the assertion
      failures named in Spec 1.
- [ ] Hygiene, `doc_truth` and `bug_tracker` are ok, `build_warnings=0`,
      clippy is clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
for g in ghost_slot_ sudo_detect_ sudo_whole_ mailbox_name_; do cargo test $g 2>&1 | grep -E '^test result' | awk -v g=$g '{p+=$4; f+=$6} END {print g " passed=" p, "failed=" f}'; done
grep -c 'GhostSlot::held()' src/daemon/ghost.rs | sed 's/^/slot_wired=/'
grep -rn 'inc_ghosts_failed()\|inc_ghosts_completed()' src --include=*.rs | grep -v 'src/daemon/stats.rs' | wc -l | sed 's/^/caller_releases=/'
grep -rlF '(?:^|[;&|])\s*sudo\b' src docs/design-reference.md | wc -l | sed 's/^/stale_regex=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- **Edit these files:**
  - `src/daemon/stats.rs`
  - `src/daemon/ghost.rs`
  - `src/daemon/ghost/turn.rs`
  - `src/daemon/scheduled.rs`
  - `src/webhook/process.rs`
  - `src/daemon/server/handlers.rs`
  - `src/daemon/stream/conversation.rs`
  - `src/daemon/policy.rs`
  - `src/daemon/policy_tests.rs`
  - `src/daemon/utils/sudo.rs`
  - `src/agents/mailbox.rs`
  - `src/daemon/executor/knowledge/agents.rs`
  - `src/daemon/executor/file_ops/write.rs`
  - `docs/design-reference.md`
- **Allowlist:** if the hygiene gate reports `trigger_ghost_turn` or
  `do_ghost_turn` over its entry, raise that entry and give the reason in
  Findings. Lower a count if the gate asks you to.
- **Existing tests:** if a `command_has_sudo` test in `src/cli/tests.rs`
  conflicts with the new rule, update it, and name it in Findings.

## Out of scope

- The slot that `spawn_ghost_shell` claims when the chat turn that would
  trigger it is cancelled first.
- Basename matching of approved scripts, for example `sudo /tmp/x/ok.sh`.
- Mailbox file format checks.

## Update Log

<!-- entries appended below this line -->
