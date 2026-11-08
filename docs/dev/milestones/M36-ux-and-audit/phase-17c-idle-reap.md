# Phase 17c — idle-reap

**Milestone:** M36 — Chat UX and audit fixes
**Status:** todo
**Drafted:** 2026-11-08
**Depends on:** phase 17b
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=bug, size=m

## Goal

This phase fixes the last part of B11 (`FINDINGS.md`, major). Phase 17a
closes a chat session's shells after its last detach, and 17b re-adopts
shells under their owner after a restart. Some shells are still never
closed:

- shells adopted without an owner sidecar (`Owner::Adopted`), which no
  session may drive or close;
- shells re-adopted under a chat session that never re-attaches. 17a's
  detach grace starts only on a detach, so it never fires for them;
- any other shell left sitting at its prompt.

The maintenance tick now closes a shell that has sat at its prompt, with
nothing written to its recording, for `[shells] idle_reap_secs`.

## Behaviour

1. **Config.** Add `[shells] idle_reap_secs` (`u64`, default `3600`) to
   `ShellsConfig` (`src/config/shells.rs`), next to `detach_close_secs`.
   `0` means **never reap**. Document it wherever `detach_close_secs` is
   documented: `assets/etc/config.toml`, `README.md` and
   `docs/architecture.md`.
2. **`reap_idle_shells(reg, idle_secs, now_unix) -> Vec<String>`** is a
   new `pub(crate)` async fn in
   `src/daemon/executor/knowledge/shell_life.rs`, beside
   `close_after_detach`. It returns the ids it closed. A shell is closed
   only when **all** of the following hold:
   - `idle_secs > 0`;
   - its owner is not `Owner::User` (the user's `/shell` shells are never
     reaped);
   - it has no `in_flight` command;
   - `exited_unix` is `None` (a dead host is `reap_exited`'s job);
   - it is not an `Owner::Chat(s)` shell whose session is present
     (`presence_count(s) > 0`);
   - its recording is silent: the modification time of its cast
     (`launch::resolve_cast(id, started_unix)`) is `idle_secs` or more
     before `now_unix`. A missing or unreadable cast **skips** the shell,
     because its silence cannot be measured;
   - a live `probe_foreground` reads `Foreground::Prompt`. `Program`,
     `Paused`, `Unknown` and `Exited` all skip the shell. A silent
     long-running command is never killed.
3. **The close** goes through `close_shell_with`, with the record's own
   owner as the caller. It logs one `info` line naming the closed ids,
   only when there are any.
4. **Wired into the tick.** The hourly block of the maintenance loop
   (`src/daemon/startup.rs`, beside `reap_exited`) calls it with the
   `startup_config` value and the same `now`.

## Constraints

- **Never hold the registry lock across an await.** Copy the candidate
  facts out under `with_shells`, then probe and close with the lock
  released. `probe_foreground` already follows this rule.
- **One way to hang up a shell.** Use `close_shell_with`. Do not add a
  second close path.
- **Tests do not touch the real `~/.daemoneye`.** Every test holds
  `crate::test_home()`. Use the in-process hosts
  `shell_lifecycle_tests.rs` already uses (`spawn_in_process_host` /
  `spawn_with_cast`), and inject `now_unix` rather than sleeping out an
  idle limit.
  - **Gotcha:** in 17b, `test_home_guard()` (the lock without the `HOME`
    redirect) let a test write into the real shells dir. It must be
    `crate::test_home()`.
- Presence in tests: register through `presence_register`, as the 17a
  tests in `shell_lifecycle_tests.rs` do, with a session id unique to the
  test, because the presence map is process-global.
- Do not change `close_after_detach`, `reap_exited` or `Owner::may_drive`.

## Spec

1. **Red tests first**, in `shell_lifecycle_tests.rs`. Run them and paste
   the real output into an Update Log entry `(red runs)`. A test naming the
   new function or field is red as a compile error.
   - `reap_idle_closes_a_silent_adopted_shell_at_its_prompt`
   - `reap_idle_skips_a_user_shell`
   - `reap_idle_skips_a_shell_with_a_command_in_flight`
   - `reap_idle_skips_a_present_chat_sessions_shell`
   - `reap_idle_closes_an_absent_chat_sessions_shell`
   - `reap_idle_skips_a_recently_written_shell` (`now_unix` within the
     limit of the cast's mtime)
   - `reap_idle_skips_a_shell_not_at_its_prompt` (a foreground program
     such as `sleep 30`)
   - `reap_idle_does_nothing_when_idle_secs_is_zero`
   - in `src/config/tests.rs`:
     `idle_reap_secs_defaults_to_3600_and_zero_is_allowed`
2. **Implement** Behaviour 1–4, running `cargo build` after each step.
3. **Mutation check:** remove the `Foreground::Prompt` requirement, run
   `cargo test --lib reap_idle_skips_a_shell_not_at_its_prompt`, and confirm
   that it fails. Restore the requirement and confirm that it passes.
   **Paste both real results** into an Update Log entry `(mutation)`. The
   summary sentence alone is not evidence; 17b's review recorded that gap.
4. **Findings:** add a `### Findings — <date>` entry of two or three lines,
   or `none`.
5. **End-to-end:** run § End-to-end verification once, and paste its
   output.
6. **Commit** as
   `fix: the maintenance tick closes shells idle at their prompt past [shells] idle_reap_secs (M36 phase-17c)`.

## Acceptance criteria

- [ ] All nine named tests pass, each printed by the end-to-end loop with
      `failed=0` and `passed=1` or more.
- [ ] `reap_fn=1` or more: `shell_life.rs` defines
      `fn reap_idle_shells`. Today it is 0.
- [ ] `tick_call=1` or more: `src/daemon/startup.rs` calls
      `reap_idle_shells`. Today it is 0.
- [ ] `docs=3`: `idle_reap_secs` appears in each of
      `assets/etc/config.toml`, `README.md` and `docs/architecture.md`.
      Today it is 0.
- [ ] Existing tests pass unchanged.
- [ ] `real_owner_files=0` after the full test run.
- [ ] Hygiene, `doc_truth` and `bug_tracker` are ok, `build_warnings=0`,
      clippy is clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
for t in reap_idle_closes_a_silent_adopted_shell_at_its_prompt reap_idle_skips_a_user_shell reap_idle_skips_a_shell_with_a_command_in_flight reap_idle_skips_a_present_chat_sessions_shell reap_idle_closes_an_absent_chat_sessions_shell reap_idle_skips_a_recently_written_shell reap_idle_skips_a_shell_not_at_its_prompt reap_idle_does_nothing_when_idle_secs_is_zero idle_reap_secs_defaults_to_3600_and_zero_is_allowed; do
  cargo test --lib "$t" 2>&1 | grep -E '^test result' | awk -v t="$t" '{p+=$4; f+=$6} END {print t, "passed=" p, "failed=" f}'
done
grep -c 'fn reap_idle_shells' src/daemon/executor/knowledge/shell_life.rs | sed 's/^/reap_fn=/'
grep -c 'reap_idle_shells' src/daemon/startup.rs | sed 's/^/tick_call=/'
n=0; for f in assets/etc/config.toml README.md docs/architecture.md; do grep -q 'idle_reap_secs' "$f" && n=$((n+1)); done; echo "docs=$n"
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
ls ~/.daemoneye/var/run/shells/*.owner 2>/dev/null | wc -l | sed 's/^/real_owner_files=/'
E2E
```

## Authorizations

- **Edit:**
  - `src/daemon/executor/knowledge/shell_life.rs` and
    `shell_lifecycle_tests.rs`;
  - `src/config/shells.rs` and `src/config/tests.rs`;
  - the hourly block of `src/daemon/startup.rs`;
  - the three docs named in Behaviour 1.
- **Allowlist:** if the hygiene gate reports a file or function over its
  cap, add or raise its entry and give the reason in Findings. Do not split
  code to fit.

## Out of scope

- Removing a closed shell's `<id>.owner` / `<id>.sandbox` sidecars on close
  (sweep_stale does it).
- Handing an `Owner::Adopted` shell to a session.
- A reap cadence other than the existing hourly block.

## Update Log

<!-- entries appended below this line -->
