# Phase 02 — open-shell-gate

**Milestone:** M39 — Critical fixes
**Status:** todo
**Drafted:** 2026-11-22
**Depends on:** none
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=bugfix, size=m

## Statement of work

A ghost run must not get an unsandboxed shell through `open_shell`, and
every shell a ghost run creates or uses through the executor must belong
to that ghost, so the run's teardown closes it.

There are two defects today, both in `src/daemon/executor/`:

1. **No sandbox gate.** `open_shell` called from a ghost never checks
   `[sandbox] enabled` or the startup-config failure. With the sandbox on,
   a ghost that is refused a foreground command can open a plain host PTY
   and work in it. A `run_terminal_command` foreground call from a ghost
   is refused in exactly that case.
2. **Wrong owner.** The executor arms build the shell owner with
   `chat_owner(session_id)`, which gives `Owner::Chat(<id>)` even when the
   caller is a ghost. The ghost's end-of-run teardown closes only
   `Owner::Ghost(<id>)` shells, so a shell the ghost opened, or one created
   for its foreground commands with the sandbox off, outlives the run. The
   ghost's sandbox, remote and local-PTY command paths already use
   `Owner::Ghost`.

The fix is one owner rule for executor calls. A ghost call's owner is
`Owner::Ghost(<session id>)`, and a chat or ask call's owner stays what
`chat_owner` gives today. Apply it in **every** arm, not only
`open_shell`. The shell tools check ownership ("shell sN is not yours"),
so if `open_shell` alone moved to `Owner::Ghost`, a ghost could no longer
run commands in, type into, list or close the shell it just opened.

**Scoping hints.** Start here, and read further only when the work
cannot be finished from these. Record in the Update Log what you read
beyond them and why.

- `src/daemon/executor/dispatch_open_shell.rs`: the arm missing the gate.
- `src/daemon/executor/dispatch_foreground.rs`: `ghost_foreground_refusal`
  is the gate to match. Its tests show how the sandbox-on and
  startup-failure cases are set up.
- `src/daemon/executor/mod.rs`: `chat_owner` and its tests.
  `src/daemon/executor/route.rs` is where `is_ghost` is known for every
  arm.
- `grep -rn 'chat_owner(' src/daemon/executor` lists every site the owner
  rule must reach. Each site is in an arm module under
  `src/daemon/executor/`.
- `src/daemon/ghost.rs`, `teardown_ghost_resources`: read it only to
  confirm which owner it closes. It needs no change.
- `src/daemon/executor/ghost_exec_helpers.rs`: the analogue for how ghost
  paths already build `Owner::Ghost`.
- Not needed: `src/shell/registry.rs` internals, the sbx modules, the AI
  backends.

**Budget.** You have **1200 turns** to finish, so use them efficiently.
Make an edit as soon as you know what it is, and run a gate rather than
re-reading a file you have already read.

## Evidence

Checked against the tree at drafting (`70369c29`):

- `dispatch_open_shell.rs` runs the ghost host fence (`refuse_ghost_host`)
  and then opens the shell. It reads neither `config.sandbox.enabled` nor
  `crate::config::startup_error()`. The foreground arm reads both,
  through `ghost_foreground_refusal`, before anything else.
- `dispatch_open_shell.rs` passes `&chat_owner(session_id)` to
  `knowledge::open_shell`. So does `dispatch_foreground.rs`, in the
  reuse pick and in `run_approved`, whether or not the caller is a ghost.
  `chat_owner` always returns `Owner::Chat`.
- `teardown_ghost_resources` (`src/daemon/ghost.rs`) calls
  `close_owner_shells` with `Owner::Ghost(session_id)` only.
- The other `chat_owner(session_id)` sites are `dispatch_send_input.rs`,
  `dispatch_shells.rs` (`list_shells`, `find_in_shells`, `close_shell`),
  `dispatch_context.rs` and `dispatch_background.rs`. In
  `dispatch_background.rs`, ghosts are routed away before the owner is
  built, so confirm whether that site needs anything.
- Ownership refusals use the record's owner key, in
  `knowledge/shell_input.rs`, `knowledge/shell_life.rs` and
  `shell_exec.rs`.

**Gotcha:** `crate::test_home_guard` is not re-entrant. A test takes it
once. A test that needs `[sandbox] enabled = true` in its config writes
that config inside its own temp HOME.

## Definition of done

Each behaviour is covered by a test that fails on the current tree. Show
that test's red run in the Update Log before the fix lands.

- [ ] A ghost's `open_shell` is refused while `[sandbox] enabled = true`,
      and also while the daemon is running on fallback defaults after a
      failed startup load. No shell is spawned in either case. The refusal
      says why and suggests `wait: false`, as the foreground refusal does.
- [ ] A chat (non-ghost) `open_shell` still opens a shell with the sandbox
      on (preservation).
- [ ] A shell a ghost opens with `open_shell` is owned by
      `Owner::Ghost(<its session id>)`. So is a shell created for a ghost's
      foreground command with the sandbox off.
- [ ] The ghost can then use that shell. A foreground command with
      `shell: <id>`, `list_shells`, and `close_shell` on it all succeed, with
      no "is not yours" refusal.
- [ ] After `close_owner_shells(Owner::Ghost(<id>))`, no shell from the
      two cases above remains in the registry.
- [ ] Chat and ask calls keep their owners. The existing `chat_owner_*`
      tests pass unchanged.
- [ ] `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets
      --all-features -- -D warnings` and `cargo test` all pass, with
      `tests/hygiene.rs` and `tests/doc_truth.rs` included.

## Spec

1. **Break down** — read the statement of work, the evidence and the
   definition of done. Then write a numbered task list into a progress
   entry in the Update Log, one line per task, each naming the DoD item it
   closes. Ends when that entry is written.
2. **Implement** — work through your task list, writing each DoD test red
   before its fix. Ends at a green `cargo build --tests`.
3. **Meet the DoD** — check off every DoD item against your tests and the
   gates. Ends when every item is checked, with its evidence named.
4. **Capture the end-to-end evidence** — run the four gate commands, then
   paste each `test result:` line and your red runs into a new Update Log
   entry. Do not paste per-test `… ok` lines.

## Authorizations

- May touch `src/daemon/executor/**` and its test files. Nothing else
  needs to change. If a change outside that seems necessary, record why
  in the Update Log before making it.

## Out of scope

- `send_input`'s approval host and shell-id parsing (phase 03).
- Running an opened shell inside the ghost's sbx sandbox. The rule here
  is refusal, not relocation.
- Per-owner cap values, and the scheduler and user owners.

## Update Log

<!-- entries appended below this line -->
