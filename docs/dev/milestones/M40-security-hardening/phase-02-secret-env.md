# Phase 02 — secret-env

**Milestone:** M40 — Security hardening
**Status:** todo
**Drafted:** 2026-11-25
**Depends on:** none
**Estimated diff:** ~200 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Audit row **S5**. The daemon's provider API keys (`ANTHROPIC_API_KEY`,
`OPENAI_API_KEY`, `GEMINI_API_KEY`, the names `ModelEntry::api_key_env_var`
returns) are inherited by every process it spawns. That includes the shells
the model runs commands in, so `env` or `echo $ANTHROPIC_API_KEY` in a
model-run command hands the key back to the model.

The architect **reproduced it** on 2026-11-25 with scratch tests, since
reverted. With a sentinel key set in the daemon's environment:
- a `PtyShell::spawn` child printed `KEY=[sk-ant-PROBE-SENTINEL]`;
- a `deliver_command` child printed `KEY=[sk-PROBE-SENTINEL]`.

The architect traced every spawn site. A command the model chooses reaches
a process through exactly three of them:
- **`PtyShell::spawn`** (`src/shell/pty.rs`): every shell, plus the ssh
  connect step in `drive_connect`.
- **`spawn_detached`** (`src/shell/launch.rs`): the `shell-host` process,
  whose PTY children inherit from it.
- **`deliver_command`** (`src/daemon/notice_sinks.rs`): the command sink's
  `sh -c`.

The other spawns (`ssh -O check`, the sbx CLI, sudoers `id`/`sudo`,
`sys_context`) run fixed programs and are out of scope.

## Statement of work

1. **One list.** Add `pub const PROVIDER_KEY_VARS: &[&str]` to
   `src/config/models.rs`, beside `api_key_env_var`, with the three names.
   A test ties the two together, so a provider added later cannot be
   missed.
2. **Remove each listed variable** from the child's environment at all
   three sites:
   - `PtyShell::spawn` removes it from its `CommandBuilder`;
   - `spawn_detached` and `deliver_command` remove it from their
     `Command`.

   **Remove only these names.** Never `env_clear`: `PATH`, `HOME`, `TERM`,
   `SSH_AUTH_SOCK` and the `DAEMONEYE_*` variables the command sink sets
   must still reach the child.
3. **`spawn_detached`'s command gets a builder.** Move its
   `std::process::Command` construction into a function
   `shell_host_command(exe, id, shell, args, rows, cols)` that returns
   the `Command`, and make `spawn_detached` build through it. The argv must
   not change; the existing `spawn_detached_*` tests pin it.
4. **Docs.**
   - Add a short guard paragraph to `docs/security.md` § "Guards, in order
     of strength", as `### 8`: what is removed, from which spawns, and that
     a user who exports a key in their own shell rc file still gets it in
     login shells, by their own choice.
   - Correct `deliver_command`'s doc comment. It names `fire_notification`,
     which no longer exists.

**Constraints:**

- No `unsafe`, `.unwrap()`, `.expect()` or `panic!` in production code.
- **Tests that set a key** hold `crate::test_home_guard()`. Take it **once**
  per test, at the top; it is not re-entrant. They use an `unsafe
  { std::env::set_var }`, the suite's accepted idiom under that guard.
  They **restore each variable's original value** on drop, even when the
  test fails, because a developer's shell may export real keys.
- **Gotcha:** a test subscribes to a `PtyShell`'s output *after* the spawn.
  Delay the child's `echo` (`sleep 0.3; echo …`), or the output can
  arrive before the subscription and the test times out.
- Every wait in a new test is bounded. No assertion may be true by
  construction.

**Closed file list:** `src/config/models.rs`, `src/shell/pty.rs`,
`src/shell/launch.rs`, `src/daemon/notice_sinks.rs` (each with its own
tests, or `src/config/tests.rs` for models), and `docs/security.md`. Read
at most **3** files before your first edit.

**Edit order:** item 1, then `pty.rs`, `launch.rs`, `notice_sinks.rs`,
then the docs.

**Budget.** You have **1200 turns**, so use them efficiently. Your
**first tool call** after reading writes the first red test.

## First red test

`pty_child_sees_no_provider_keys`, in `src/shell/pty.rs` tests.
- Set all three variables to sentinels, and `DE_ENV_PROBE_KEEP=kept`.
- Spawn `sh -c 'sleep 0.3; echo "K=[$ANTHROPIC_API_KEY$OPENAI_API_KEY$GEMINI_API_KEY] KEEP=[$DE_ENV_PROBE_KEEP]"'`.
- Wait, bounded, for the exact text `K=[] KEEP=[kept]`. The `KEEP` half is
  the must-stay case: it proves the fix removes names rather than clearing
  the environment.

It is red today: the line carries the three sentinels. Paste the red run.

## Acceptance criteria

- [ ] **PTY:** `pty_child_sees_no_provider_keys` passes. Red run pasted.
- [ ] **Command sink:** `command_sink_child_sees_no_provider_keys` (in
      `notice_sinks.rs` tests) passes. It sets the same sentinels and
      `DE_ENV_PROBE_KEEP`, then calls `deliver_command` with a command that
      writes the same `K=[…] KEEP=[…]` line, plus `$DAEMONEYE_JOB`, to a
      temp file. It polls for the file under a timeout. The line must read
      `K=[] KEEP=[kept]` and carry the notice's name. Red run pasted.
- [ ] **Shell host:** `shell_host_command_removes_provider_keys` (in
      `launch.rs` tests) passes. Each name in `PROVIDER_KEY_VARS` appears
      in `shell_host_command(…).get_envs()` as `(name, None)`, an explicit
      removal, whatever the test's own environment holds.
- [ ] **One list:** `provider_key_vars_cover_every_provider` passes. For
      every provider `api_key_env_var` knows (`anthropic`, `openai`,
      `gemini`, `ollama`, `lmstudio`), a non-empty name is in
      `PROVIDER_KEY_VARS`.
- [ ] **Wiring:** run this command **once**. It prints **1** (it prints 0
      today):
      `awk '/pub async fn spawn_detached/,/^}/' src/shell/launch.rs | grep -c 'shell_host_command('`
- [ ] **Preserved:** the three `spawn_detached_*` argv tests pass unchanged.
- [ ] **Mutation check,** run once with the output pasted:
      1. remove the variable-removal step from `PtyShell::spawn`;
      2. run `cargo test --lib pty_child_sees_no_provider_keys` once and see
         it fail;
      3. restore it.
- [ ] **Gates:** all four pass, run once each. The lib suite reports **at
      least 3136** passed (3132 plus four).

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is written.
2. **Implement:** in the edit order, writing each red test before its fix.
   Done when `cargo build --tests` is green.
3. **Meet the acceptance criteria:** check off every item. Run each
   command **once**. Done when every item is checked with its evidence
   named.
4. **Capture the evidence:** run the four gates, once each. Paste into a
   new Update Log entry:
   - each `test result:` line, without the per-test `… ok` lines;
   - the two red runs;
   - the wiring grep;
   - the mutation check.

   Commit as one `fix:` commit.

## Authorizations

- You may touch the files in the closed file list, and no others.

## Out of scope

- Removing keys from the daemon's own process environment. The keys are
  re-read on reload.
- Other secret-bearing variables, such as `OPENAI_API_BASE` (not a
  secret) and user-chosen `api_key` values in `config.toml`, which are not
  in the environment.
- The fixed-program spawns named in the Goal.
- Re-pointing the existing `child_argv` test helper at the new builder.

## Update Log

<!-- entries appended below this line -->
