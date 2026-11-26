# Bug 1 on phase-06b: the chat-approval tests hang, not fail, when the gate is skipped

**Severity:** major
**Status:** open
**Filed:** 2026-11-26

## What's wrong

`run_spawn` (`src/daemon/executor/dispatch_meta.rs:200`) drives
`spawn_ghost_from_chat_asks_first` and
`approved_spawn_proceeds_to_load_the_runbook`.

- **The daemon side** (`let daemon = async { … }`, line 210) **borrows**
  `daemon_tx`. It is not an `async move` block that owns it.
- **The client side** (line 224) reads the prompt with a bare
  `read_bounded_line`, with no timeout.

When the gate is skipped, the daemon sends no prompt and `dispatch`
returns. `daemon_tx` is still alive in `run_spawn`'s scope, so the client
read never sees EOF and waits forever.

**The reviewer's mutation run:**
- **Mutation:** `if true { return Ok(true); }` at the top of
  `ask_chat_approval`, so no prompt is sent.
- **Command:** `cargo test --lib --no-run`, then
  `timeout 60 cargo test --lib dispatch_meta::tests`.
- **Result:** it **exits 124** (killed by the timeout) with no
  `test result:` line. The tests hang instead of failing.

The phase spec's gotcha (Notes for executor, item 4) named this exact
hang.

**The Update Log's red-run and mutation-run outputs (`no GhostSpawnPrompt
arrived within 5s`) do not describe the committed code.**
1. At turn 160 the executor added a 5 s `tokio::time::timeout` to the
   client read.
2. At turn 161 it ran the mutation.
3. At turn 162 it "restored" with
   `cp /tmp/dm_good.rs src/daemon/executor/dispatch_meta.rs`. That backup
   was taken **before** the turn-160 edit, so the copy reverted the timeout
   along with the mutation.

The string `within 5s` appears nowhere in `src/`. The "red run against the
tree before the gate existed" is the same mutation output, pasted twice.
The completion summary says the client read is wrapped in a 5 s timeout.
That is false for the commit.

## What should happen

With the prompt skipped, `cargo test --lib spawn_ghost_from_chat_asks_first`
**fails promptly** with `test result: FAILED`. It does not hang. The pasted
evidence is produced by the committed tree.

## Root cause

The daemon future borrows the writer, which therefore outlives `dispatch`,
and the client's prompt read has no time bound.

## Definition of done

- [ ] Skip the prompt as above. Then `cargo test --lib --no-run`, then
      `timeout 120 cargo test --lib spawn_ghost_from_chat_asks_first`
      prints `test result: FAILED` and exits non-zero but **not 124**.
      Today it exits 124. Restore with `git checkout` of that one file or
      an undo of the one edit, **not** a copy of an older backup.
- [ ] Without the mutation, all `dispatch_meta::tests` pass.
- [ ] `grep -c 'within 5s\|tokio::time::timeout' src/daemon/executor/dispatch_meta.rs`
      matches what the Update Log claims about the test. Alternatively,
      owning the writer in an `async move` daemon block gives an EOF
      instead of a timeout. In that case, the pasted failure is whatever
      that run prints.
