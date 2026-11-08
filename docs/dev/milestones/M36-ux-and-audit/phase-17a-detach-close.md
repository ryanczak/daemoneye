# Phase 17a — detach-close

**Milestone:** M36 — Chat UX and audit fixes
**Status:** todo
**Drafted:** 2026-11-07
**Depends on:** phase 16
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=bug, size=m

## Goal

Phase 17 was split on 2026-11-07. This half closes a chat session's shells
when the session goes away, and stops exited shell hosts becoming zombies.
The other half, 17b, persists each shell's owner and host so that a restart
re-adopts the shell under its owner, and reaps idle adopted shells.

- **B11, first half (bug, major).** A chat session's shells are never
  closed. `close_owner_shells` (`src/daemon/executor/knowledge/shell_life.rs`)
  runs only from the ghost and scheduler paths. Every `daemoneye chat` that
  ran a command leaves a `daemoneye shell-host` + bash pair running until
  the daemon exits. pinky had 12 of them, 1 to 8 days old.
- **B12 (bug, minor).** `spawn_detached` (`src/shell/launch.rs`) does
  `drop(child)` on the `std::process::Child`. Nothing waits on the child, so
  a shell host that exits while the daemon lives (after `close_shell`, or a
  finished dedicated background shell) stays a zombie until the daemon
  exits.

## Behaviour

1. **The last detach starts a grace.** In `handle_chat_attach`
   (`src/daemon/server/chat_presence.rs`), when `presence_unregister`
   reports the session's last connection closed, the daemon spawns a task
   that runs `close_after_detach`. The detach bookkeeping it already does is
   unchanged.
2. **`close_after_detach(reg, session_id, grace)`** is a new function in
   `shell_life.rs`, `pub(crate)`. It sleeps `grace`, then checks
   `presence_count(session_id)`:
   - **0:** it closes every shell whose owner is
     `Owner::Chat(session_id)` and that is **not** `dedicated`. It uses the
     same close as `close_owner_shells`, hangup and all.
   - **Above 0:** the session came back in time. It closes nothing.
   It logs one `info` line naming the session and the ids it closed.
3. **The grace is a config key.** `[shells] detach_close_secs` (`u64`,
   default `300`) goes in `ShellsConfig` (`src/config/shells.rs`), next to
   `exited_retention_secs`. `0` means **never close** on detach, which
   keeps today's behaviour. The attach handler reads it the way
   `dispatch_foreground.rs` reads `[shells] max_per_owner`: the loaded
   config, falling back to `Config::default()`.
4. **What is never closed by this path:**
   - a `dedicated` shell, which carries a background run and may still be
     running it;
   - an `Owner::User` (`/shell`) shell;
   - another session's shells;
   - ghost, scheduler and adopted shells.
5. **Exited hosts are reaped.** `spawn_detached` no longer drops its child
   unwaited. Something waits on every shell-host child it spawns, off the
   async runtime, so that when the host exits its process entry is released
   while the daemon lives. `await_ready` still decides whether the spawn
   succeeded, so the readiness contract is unchanged.

## Constraints

- **No SIGCHLD handler and no `SIG_IGN`.** Ignoring SIGCHLD process-wide
  would break every `Command::output()`/`status()` the daemon runs, because
  their waits would fail with `ECHILD`. Wait on the specific child.
- The close goes through the existing close path (`close_shell_with` or
  `close_owner_shells`). Do not add a second way to hang a shell up.
- The detach task must not hold the presence map lock or the registry lock
  across its sleep or its awaits.
- Do not change `presence_register` / `presence_unregister`'s semantics or
  the catch-up brief.
- Tests use short graces, 100 to 300 ms, never the 300 s default.

## Spec

1. **Red tests first.** Write the tests below, run them, and paste the
   real output into an Update Log entry `(red runs)`. A test that names the
   new function or config field is red as a compile error.
   - `shell_life.rs` tests (or its existing test file), with in-process
     hosts. Use `scheduled_tests.rs`'s `spawn_in_process_host` as the model:
     - `last_detach_closes_the_sessions_shells_after_the_grace`: a session
       with two non-dedicated chat shells and no presence. After
       `close_after_detach` with a 100 ms grace, both hosts are gone
       (`host_is_alive` on their locks is false).
     - `a_reattach_within_the_grace_keeps_the_shells`: a presence is
       registered for the session before the grace ends. Its shell host is
       still alive afterwards.
     - `detach_close_leaves_other_owners_and_dedicated_shells`: the session's
       dedicated shell, a `User` shell and another session's chat shell all
       survive. The session's own non-dedicated shell is closed.
   - `detach_close_secs_defaults_to_300_and_zero_is_allowed`: `src/config`
     tests. A config with no `[shells]` table gives 300, and
     `detach_close_secs = 0` parses as 0.
   - `an_exited_detached_child_is_reaped`: `launch.rs` tests. Spawn
     `sh -c 'exit 0'` through the waiting mechanism. Within 2 s,
     `/proc/<pid>/stat` either no longer exists or does not show state `Z`.
     Today's `drop(child)` leaves `Z`.
2. **Implement** Behaviour 1–5, running `cargo build` after each step.
3. **Mutation check:** remove the `presence_count` check from
   `close_after_detach` and run
   `cargo test --lib a_reattach_within_the_grace_keeps_the_shells`. Confirm
   that it fails, restore the check, and confirm that it passes. Paste both
   real results.
4. **Docs:**
   - A `detach_close_secs` row in `README.md`'s `[shells]` table, next to
     `exited_retention_secs`, with the commented line in its example block.
   - The commented line in `assets/etc/config.toml`.
   - One sentence in `docs/architecture.md` where it describes
     `exited_retention_secs`.
5. **Findings:** add a `### Findings — <date>` entry of two or three lines,
   or `none`.
6. **End-to-end:** run § End-to-end verification once, and paste its
   output.
7. **Commit** as
   `fix: a chat session's shells close after its last detach, and exited shell hosts are reaped (M36 phase-17a)`.

## Acceptance criteria

- [ ] All five named tests pass, each printed by the end-to-end loop with
      `failed=0` and `passed=1` or more.
- [ ] `unwaited_drop=0`: `launch.rs` no longer contains `drop(child)`.
      Today it is 1.
- [ ] `detach_key=1` or more: `src/config/shells.rs` names
      `detach_close_secs`. Today it is 0.
- [ ] `readme_key=1` or more: `README.md` names `detach_close_secs`.
      Today it is 0.
- [ ] Existing tests pass, unmodified.
- [ ] Hygiene, `doc_truth` and `bug_tracker` are ok, `build_warnings=0`,
      clippy is clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
for t in last_detach_closes_the_sessions_shells_after_the_grace a_reattach_within_the_grace_keeps_the_shells detach_close_leaves_other_owners_and_dedicated_shells detach_close_secs_defaults_to_300_and_zero_is_allowed an_exited_detached_child_is_reaped; do
  cargo test --lib "$t" 2>&1 | grep -E '^test result' | awk -v t="$t" '{p+=$4; f+=$6} END {print t, "passed=" p, "failed=" f}'
done
grep -c 'drop(child)' src/shell/launch.rs | sed 's/^/unwaited_drop=/'
grep -c 'detach_close_secs' src/config/shells.rs | sed 's/^/detach_key=/'
grep -c 'detach_close_secs' README.md | sed 's/^/readme_key=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- **Edit:**
  - `src/daemon/server/chat_presence.rs`;
  - `src/daemon/executor/knowledge/shell_life.rs` and its tests;
  - `src/daemon/executor/knowledge/mod.rs`, for the re-export only;
  - `src/config/shells.rs` and its tests;
  - `src/shell/launch.rs`;
  - `README.md` (the `[shells]` table and example);
  - `assets/etc/config.toml`;
  - `docs/architecture.md` (one sentence).
- **Allowlist:** if the hygiene gate reports a file or function over its
  cap, add or raise its entry and give the reason in Findings. Do not split
  code to fit.

## Out of scope

- The owner/host sidecar, re-adoption under the owner, and the idle reaper
  for adopted shells: phase 17b.
- Closing a dedicated background shell after its run completes.
- Ghost and scheduler shell teardown, which already happens.

## Update Log

<!-- entries appended below this line -->
