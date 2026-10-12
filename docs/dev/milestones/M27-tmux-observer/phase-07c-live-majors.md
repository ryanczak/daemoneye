# Phase 07c: live-majors — chat survives a daemon restart, the model's `background` habit works, expired approvals close, honest shell-exit errors, a gate that sees abbreviations

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** todo
**Depends on:** phase-07b
**Tags:** language=rust, kind=bugfix, size=m

## Goal

These are the rest of phase 07's rehearsal findings (F4, F2, F7, F1), plus
two items carried from 07b's review. They don't block the tag the way
07b's did, but each one cost real time in the live run.

## Spec — the edit table is the whole job

**Rules.** Open only the files a row names, and do not grep `src/` for
other instances. Your first `patch` lands within 10 turns; that patch is
row 1. Do the rows in order, and run `cargo build` after each. The facts
below were verified by the architect; do not re-derive them.

| # | Finding | File · symbol | Edit |
|---|---|---|---|
| 1 | F2 | `src/ai/tools/args.rs`, `RunTerminalCommandArgs` and its `to_event` | Add `background: Option<bool>`. In `to_event`, `wait: self.wait.or(self.background.map(\|b\| !b))`. An explicit `wait` wins. Do **not** add `background` to the tool schema in `defs.rs`. GLM sends `background` from its priors. Today serde drops it, so `background: true` silently ran waited, four times in the live run. |
| 2 | F1 | `src/shell/client.rs`, the `n == 0` branch in the command read loop that bails "shell socket at … closed while running …" | Return an error whose chain contains `ShellExited { cmd }` (the type is defined in the same file), with the socket message as context. `shell_exec.rs`'s `map_err` already maps a `ShellExited` chain to "shell sN exited before the command finished". Today a closed socket falls into its "command exceeded the 45s limit" branch, which is false: the close was immediate. |
| 3 | F7 | `src/cli/commands/stream.rs`, `read_approval_input` and `read_approval_input_panel`, plus their 4 call sites | Bound each read with a client-side deadline equal to the daemon's `APPROVAL_TIMEOUT` (60 s, `src/daemon/executor/mod.rs`). Move that constant somewhere both sides can import, such as `src/ipc.rs`, and use it in both places. On expiry, return `None` (the functions now return `Option<String>`). Each call site then commits `⏱ approval timed out — the command was not run`, sends nothing, and goes back to reading daemon responses. The daemon's own timeout `SystemMsg` follows. Today the panel stays live after the daemon gave up, and the next line typed fails with `Broken pipe`. |
| 4 | F4 | `src/cli/commands/chat_presence.rs`, `HeldPresence::park` and its task; call site `src/cli/commands/chat.rs` (`held.park()`) | On EOF, do not end. Push `⚙ daemon connection lost — reconnecting…`, then retry the attach every 2 s until it succeeds or the guard is dropped. On success, push `⚙ reconnected`, forward any brief the attach returned, and keep reading. `park` needs the session id and an injectable reconnect step (a closure or trait), so tests can fake the daemon. Today a daemon restart silently orphans the chat: it gets no alerts and no notice. |
| 5 | carried from 07b | `tests/tmux_read_only.rs` | Scope the gate to what reaches tmux. For each `Command::new("tmux")` in non-test source, check the string literals in the `.args([...])` / `.arg(...)` chain that follows, possibly across lines. Fail on any write verb **or its tmux abbreviation**: `send-keys`/`send`, `new-window`/`neww`, `new-session`/`new`, `kill-window`/`killw`, `kill-session`, `set-hook`, `pipe-pane`/`pipep`, `respawn-pane`/`respawnp`, `split-window`/`splitw`, `resize-pane`/`resizep`, `select-pane`/`selectp`, `set-option`/`set`, `set-window-option`/`setw`. Literals outside tmux arg lists (help text, comments) no longer count. Keep the `TEST_CODE_FILES` exclusion. |
| 6 | carried from 07b | `src/cli/commands/setup.rs` (the `bind-key T splitw` line), `src/cli/local_cmds.rs` (the `tmux killw` hint), `src/cli/commands/pane.rs` (`sync_read_line`) | Restore `split-window` and `kill-window` in those two printed help lines. They are text for the user's tmux.conf, not tmux calls. In `sync_read_line`, move the "Restore original flags" comment to the restore. Make the doc comment say what the function does: it clears `O_NONBLOCK` around a blocking line read, then restores it. |

**Tests:**

- `run_terminal_command_maps_legacy_background_to_wait`, in
  `src/ai/tools/dispatch.rs` tests. Through `dispatch_tool_event`:
  `background: true` gives `wait == Some(false)`, and `background: false`
  gives `Some(true)`. When both `wait: true` and `background: true` are
  given, the result is `Some(true)`.
- `socket_close_mid_command_reports_shell_exit`: the error from row 2's
  path has a `ShellExited` in its chain, and when rendered through
  `shell_exec`'s mapping it says "exited before the command finished", not
  "exceeded". Exercise the mapping at its smallest testable seam; extracting
  the `map_err` closure into a named function is fine.
- `approval_read_times_out` in `stream.rs` tests: with a stdin that never
  sends a byte and a short test deadline (make the deadline a parameter),
  the read returns `None`.
- `parked_presence_reconnects_after_eof` **replaces** today's
  `parked_presence_ends_on_eof` (change it, don't add a second test). A
  fake daemon closes the connection, and the fake reconnect step succeeds.
  The receiver gets the "connection lost" push, then "reconnected", and a
  push sent on the new connection is forwarded.
- `source_issues_no_tmux_write_verbs` keeps its name. Add
  `gate_catches_an_abbreviated_write`, which runs the gate's scanner over an
  inline source string `Command::new("tmux").args(["splitw", "-h"])` and
  expects one hit. Run the same scanner over `println!("bind-key T
  split-window …")` and expect none.

**Mutation check.** Delete row 1's `.or(…)`, run the background test,
confirm it fails, and revert. Paste the failing line.

## Acceptance criteria

- [ ] `cargo test run_terminal_command_maps_legacy_background_to_wait`,
      `cargo test socket_close_mid_command_reports_shell_exit`,
      `cargo test approval_read_times_out`,
      `cargo test parked_presence_reconnects_after_eof` and
      `cargo test gate_catches_an_abbreviated_write` each report `1 passed`.
      None of them exists today.
- [ ] `cargo test parked_presence_ends_on_eof` reports `0 passed` (renamed).
      `cargo test --test tmux_read_only` is all green.
- [ ] `grep -c 'bind-key T split-window' src/cli/commands/setup.rs` prints
      `1`. Today it prints `0`.
- [ ] clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] `cargo test` reports `failed=0`. `passed` is today's 2313 plus the
      tests you add (the rename is net zero); name them in the Update Log.
- [ ] The Update Log has the mutation-check failing line, § End-to-end
      verification's output, and `PASTE MATCH: yes`. The server does not
      write it for you.

## End-to-end verification

```sh
for t in run_terminal_command_maps_legacy_background_to_wait socket_close_mid_command_reports_shell_exit approval_read_times_out parked_presence_reconnects_after_eof gate_catches_an_abbreviated_write; do cargo test $t 2>&1 | grep -E '^test result: ok. 1 passed' | sed "s/^/$t: /"; done
cargo test --test tmux_read_only 2>&1 | grep -E '^test result'
grep -c 'bind-key T split-window' src/cli/commands/setup.rs
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

## Authorizations

- [x] May edit `src/ai/tools/args.rs`, `src/ai/tools/dispatch.rs` (tests),
      `src/shell/client.rs`, `src/daemon/executor/shell_exec.rs`,
      `src/daemon/executor/mod.rs` (moving the constant only), `src/ipc.rs`,
      `src/cli/commands/stream.rs`, `src/cli/commands/chat_presence.rs`,
      `src/cli/commands/chat.rs`, `src/cli/commands/setup.rs`,
      `src/cli/local_cmds.rs`, `src/cli/commands/pane.rs`,
      `tests/tmux_read_only.rs`.

## Out of scope

- The `[Background Task Completed]` on-screen display. It is by design.
- README and other docs. Phase 07a covers them.

## Update Log

<!-- entries appended below this line -->
