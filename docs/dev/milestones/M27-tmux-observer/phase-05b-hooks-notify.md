# Phase 05b: hooks-notify — the daemon writes nothing into tmux

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** todo
**Depends on:** phase-05
**Tags:** language=rust, kind=refactor, size=m

## Goal

After phase 05 no command runs in tmux, but the daemon still writes into
it: it installs global and per-session `set-hook`s that call
`daemoneye notify …`, starts `pipe-pane` on the user's pane, and creates
(and re-creates) a managed `tmux new-session`. This phase removes every one
of those writes. What remains of tmux in the daemon is read-only: the 2 s
cache poll and `capture-pane`, used only when a tmux session is there to
read.

The hooks fed four things. Each already has a replacement, so nothing is
rebuilt here:
- Detach and attach state: chat presence (phase 03).
- The active pane, the window list and the client size: the cache poll,
  which already reads all three.
- Session cleanup: the presence cleanup pass.
- Foreground completion (`BG_DONE_TX`): deleted with the send-keys path in
  phase 05.

## Spec

1. **Delete the hook handlers** — delete `src/daemon/hook.rs`, every
   `Request::Notify*` variant in `src/ipc.rs`, their arms in
   `src/daemon/server/mod.rs`, and `BG_DONE_TX`, `bg_done_subscribe` and
   `FG_HOOK_COUNTER` in `src/daemon/session.rs`. Any `managed_session` /
   `bg_session` plumbing left with no reader goes too.
2. **Delete the hook installs** — in `src/daemon/mod.rs`, delete
   `install_session_hooks`, `log_hook_install_result`, the four global
   `set-hook -g` installs at startup and the `set-hook -gu` uninstall loop
   at shutdown.
3. **Delete `daemoneye notify`** — delete the `Notify` variant of
   `Commands` and the `NotifyCommands` enum in `src/main.rs`, its match arm,
   `src/cli/notify.rs` and the `run_notify_*` functions. The subcommand is
   gone: `daemoneye notify …` is a clap usage error. (A tmux server can
   still hold hooks an older daemon installed; they then fail inside tmux
   until the user restarts tmux, which is acceptable — the daemon never
   installs them again.)
4. **Delete pipe-pane** — delete `start_pipe_pane`, `stop_pipe_pane` and
   `pipe_log_path` in `src/tmux/pane.rs`, the R1 block in
   `src/daemon/server/ask.rs` that starts it, the shutdown loop in
   `src/daemon/mod.rs` that stops it, `SessionEntry::pipe_source_pane`,
   and `read_pipe_log` in `src/tmux/cache.rs`. Pane context then comes
   from `capture-pane` alone, which is already the fallback.
   `sweep_pane_logs` stays, so old pipe logs still age out.
5. **Never create a tmux session** — the startup block in
   `src/daemon/mod.rs` that resolves `initial_session` becomes a pure
   function, `startup_session(inside: Option<String>, configured: &str,
   exists: bool) -> Option<String>`:
   - Launched inside tmux: adopt that session.
   - A non-empty configured `[daemon] tmux_session` that exists: adopt it.
   - Anything else: `None`, with one `info` log. Never fatal.

   The daemon never runs `tmux new-session`, at startup or on session close.
6. **Delete the shell exit-status hook** — in `src/cli/commands/setup.rs`,
   delete the `_de_exit_trap` / `_de_precmd` snippets that write
   `DE_EXIT_<pane>` with `tmux set-environment`. Their only reader,
   `read_pane_exit_status`, was deleted in phase 05.
7. **Tests** — add the § Tests, delete tests that only exercised deleted
   code, and **move** (never delete) a test whose subject survives. List
   each deleted and moved test in the Update Log.
8. **Evidence** — run § End-to-end verification once, paste it into the
   Update Log, then make one `refactor:` commit.

### Gotchas

- Phase 05 deleted ten tests whose code survived, and wrote two tests that
  hung. Before deleting a test, check whether the function it calls still
  exists. A test that writes a response must use `tokio::io::sink()` as its
  writer, never a pipe whose read half is dropped.
- Edit with the `patch` / `write_file` tools, not python or `sed` through
  `bash`.
- This is a deletion phase. Delete the leaf first (`hook.rs`, then the
  `Notify*` variants), then let `cargo build` name each dangling caller.

## Tests

- `startup_session_never_creates` (`src/daemon/mod.rs`): test the pure
  `startup_session` from Spec 5 directly.
  - Inside tmux: returns that session.
  - A configured name that exists: returns it.
  - A configured name that does not exist: returns `None`.
  - An empty configured name: returns `None`.
- `notify_subcommand_is_gone` (`src/main.rs`): `Cli::try_parse_from` on
  `["daemoneye", "notify", "activity", "%3", "0", "dev"]` returns `Err`.
  Negative: `["daemoneye", "status"]` still parses.

## Acceptance criteria

- [ ] `test -f src/daemon/hook.rs; echo $?` prints `1`. Today it prints `0`.
- [ ] `grep -rnE '"(set-hook|pipe-pane|new-session|set-environment)"|start_pipe_pane|install_session_hooks|BG_DONE_TX|FG_HOOK_COUNTER|NotifyActivity|run_notify_' src/daemon src/tmux src/cli src/main.rs src/ipc.rs src/ipc_tests.rs`
      prints nothing. Today it prints many lines. (The paths are narrowed on
      purpose: `header.rs` and `session_store_tests.rs` use `"new-session"`
      as a session *name*, and the integration tests start their own tmux
      fixture. Leave those alone.)
- [ ] `grep -n 'tmux set-environment' src/cli/commands/setup.rs` prints
      nothing. Today it prints two lines.
- [ ] `startup_session_never_creates` and `notify_subcommand_is_gone` each
      report `1 passed` alone.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0;
      `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports `failed=0`. The baseline is `passed=2302`. The
      Update Log states the new total as `2302 − deleted + added`, and names
      every deleted, moved and added test.
- [ ] No deleted test calls a function that still exists after the phase.
- [ ] The Update Log contains the § End-to-end verification output, every
      line as printed, and a line `PASTE MATCH: yes`.

## End-to-end verification

```sh
test -f src/daemon/hook.rs; echo "hook_rs=$?"
grep -rnE '"(set-hook|pipe-pane|new-session|set-environment)"|start_pipe_pane|install_session_hooks|BG_DONE_TX|FG_HOOK_COUNTER|NotifyActivity|run_notify_' src/daemon src/tmux src/cli src/main.rs src/ipc.rs src/ipc_tests.rs | wc -l
for t in startup_session_never_creates notify_subcommand_is_gone; do cargo test "$t" 2>&1 | grep -E '^test result: ok. 1 passed' | sed "s/^/$t: /"; done
cargo build 2>&1 | tail -1 && ./target/debug/daemoneye notify activity %3 0 dev 2>&1 | head -1; echo "notify_exit=${PIPESTATUS[0]}"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

Each line runs **once**. Expect `hook_rs=1`, `0`, one line per test, a
clap `error: unrecognized subcommand 'notify'` line followed by
`notify_exit=2`, and `failed=0`.

## Authorizations

- [x] May edit or delete anything under `src/daemon/` (including
      `hook.rs`, `mod.rs`, `session.rs`, `server/`), `src/ipc.rs`,
      `src/main.rs`, `src/cli/notify.rs`, `src/cli/mod.rs`,
      `src/cli/commands/setup.rs`, `src/tmux/pane.rs` and
      `src/tmux/cache.rs`.
- [x] May edit `tests/` where a test names a deleted symbol.
- [x] May edit `CLAUDE.md`, only where `tests/doc_truth.rs` fails on a
      deleted path. The full docs pass is phase 06.

## Out of scope

- The chat client's own tmux calls, for example sizing the chat pane
  (`resize_pane_width`) and the viewer. Phase 04 already gated them on
  `$TMUX`, and they write no hooks and run no commands.
- The `is_daemon_window` predicates and the `de-*` window prefixes. They
  still filter windows an older daemon left behind.
- The `[daemon] tmux_session` config key itself (it still names a session
  to adopt), and the prose docs. Both are phase 06.

## Update Log

<!-- entries appended below this line -->
