# Phase 21c — program-status-emit

**Milestone:** M36 — Chat UX and audit fixes
**Status:** in-progress
**Drafted:** 2026-11-11
**Depends on:** phase 21a (its parser checks the reports this phase writes)
**Estimated diff:** ~400 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Decision 9's emit side. The chat client (`daemoneye chat` and `daemoneye
ask`, which share `ask_with_session_ratatui`) reports its own state to the
user's terminal over OSC 7501. That lets a 7501-aware terminal (Ghostty,
Rex, or a DaemonEye shell host) show at a glance that DaemonEye is working,
or that it is waiting on the user. This is the milestone exit criterion's
last clause:

> during a chat approval prompt a 7501-aware outer terminal sees the client
> as `blocked`.

## Architecture references

- [`osc7501-spec.md`](osc7501-spec.md), § Syntax, § States and § Keys. This
  phase only writes reports. It never sends the feature query and never
  reads a reply.
- `src/shell/program_status.rs` (`parse`) and `src/shell/screen.rs`
  (`ShellScreen::feed`, `programs`). This is 21a's consumer, which the tests
  use to check that what the client writes is a valid report.
- `src/cli/commands/stream_ask.rs` (`ask_with_session_ratatui`, one arm per
  `Response`), `src/cli/commands/stream.rs` (`RatatuiQueryCtx`),
  `src/cli/commands/chat.rs`, `chat_loop.rs` and `ask.rs`, which build that
  context.
- `src/cli/terminal_guard.rs` (`TerminalGuard`, `restore_terminal`,
  `install_panic_restore`), the one place every exit, panics included,
  passes through.
- `src/cli/palette.rs` (`detect_from_env`): how the client already reads
  `$TMUX`. An empty value means not in tmux. `src/tmux/session.rs`
  (`client_supports_rgb`): the one tmux call the client makes today, through
  `process::bounded_output`, and the shape for the two new ones.

## Behaviour

1. **States.** Every report carries `app=daemoneye` and no `id` (the root
   record). It carries no `msg` and no `title`, so nothing from the
   conversation reaches the terminal.
   - `working` from the moment a turn is sent until it ends, except while a
     prompt waits.
   - `blocked:kind=permission` while any approval prompt waits:
     `ToolCallPrompt`, `EditFilePrompt` and the script, runbook and schedule
     write/delete prompts.
   - `blocked:kind=auth` while a `CredentialPrompt` waits.
   - `blocked:kind=question` while a `PaneSelectPrompt` waits.
   - As soon as the user's reply to a prompt is sent, the state is
     `working` again. A long command approved at the prompt must not leave
     the client reading `blocked` while it runs.
   - `idle` when a turn ends on any path (done, error, cancelled).
   - `clear` when the process exits.
2. **No repeats.** A report is written only when the state changes. A
   stream of tokens writes one `working`, not one per token.
3. **Clear only what was set.** `clear` is written on exit only if this
   process wrote at least one report. Exit includes the panic path, so hang
   it off the terminal-restore duty. A process that never reported, such as
   the daemon or `top --once`, writes nothing new.
4. **Framing.** A report is `ESC ] 7501 ; <pairs> BEL`.
5. **tmux (the passthrough decision, PE 2026-11-11).** When `$TMUX` is
   set and non-empty, each report is wrapped in tmux's passthrough
   envelope: `ESC P tmux ; <report with every ESC doubled> ESC \`. tmux
   forwards it only where `allow-passthrough` is on, and the default is
   `off`, so the client turns it on itself, for its own pane only: at
   start, `tmux set -p -t $TMUX_PANE allow-passthrough on`; at exit, the
   same with `-pu`, which unsets the pane option and leaves the window's
   and the global value exactly as they were. The pane option is set only
   when `$TMUX_PANE` is set. A failing tmux call is ignored and the reports
   still go out wrapped. Nothing else about tmux is touched: no window or
   global option, no other pane. Outside tmux the bare report is written.
6. **Only to a terminal.** Reports go to stdout, and only when stdout is a
   terminal. Piped `ask` output gets no escape sequences.

## Constraints

- **The emitter is testable without a terminal.** It writes through an
  injected writer (stdout for real, a buffer in tests). The choice of state
  per `Response` frame is a pure function, so the tests can enumerate it.
- **Gotcha (21a):** a valid report's pairs are `key=value` joined by `:`.
  Values may use only `[A-Za-z0-9_.,+/=-]`, and `state` is required. The
  round-trip test feeds the written bytes through `ShellScreen::feed`. If
  the record doesn't come back, the bytes are wrong. Don't loosen the
  parser to make the test pass, because 21a is done.
- Writing a report must never fail the turn. A write error is ignored.
- `ask_with_session_ratatui` is on the hygiene function allowlist. Keep the
  per-frame logic in helpers. If the function still grows, raise its entry
  and name the raise in Findings.

## Spec

1. **Red tests first.** Write the tests in the Test plan. Run
   `cargo test --lib program_status_emit` and paste the real failing output
   (compile errors count) into an `(red runs)` entry. The step ends when the
   run is pasted.
2. **The emitter** (Behaviour 2–6): a new `src/cli/program_status_emit.rs`.
3. **Wiring** (Behaviour 1): the emitter reaches `ask_with_session_ratatui`
   through `RatatuiQueryCtx`. Its builders in `chat.rs`, `chat_loop.rs` and
   `ask.rs` construct it, and exit clears it via `TerminalGuard`.
4. **Docs:** one paragraph in README.md under the chat section. It names
   the states, says reports are on by default, and says that inside tmux
   the client sets `allow-passthrough` on its own pane for its lifetime.
   Step ends when the paragraph is in.
5. **End-to-end:** run § End-to-end verification once, redirected to
   `.rexymcp/output/p21c-e2e.txt`, and paste it verbatim into an
   `(end-to-end verification)` entry that names it.
6. **Findings:** two or three lines, or `none`. Name any allowlist raise.
7. **Commit** as
   `feat: OSC 7501 emit — the chat client reports working/blocked/idle to its terminal, tmux passthrough (M36 phase-21c)`.

## Test plan

Every test name starts `program_status_emit_`:

- `…_each_state_round_trips_through_the_host_parser`: the bare report for
  `working`, each `blocked` kind and `idle`, fed to a `ShellScreen`, comes
  back as one root record with that state and kind and `app` `daemoneye`.
  `clear` empties the table.
- `…_tmux_wraps_and_doubles_escapes`: the tmux frame starts `ESC P tmux ;`
  and ends `ESC \`. Every ESC inside is doubled. Un-doubling the inside
  gives exactly the bare report.
- `…_empty_tmux_var_means_bare`.
- `…_tmux_pane_option_argv` (pure): the start call is
  `set -p -t <pane> allow-passthrough on` and the exit call is
  `set -pu -t <pane> allow-passthrough`, both for the pane `$TMUX_PANE`
  names, and no call at all when `$TMUX_PANE` is absent.
- `…_tmux_pane_option_set_and_unset_live`: against a private tmux server
  (the `-S` socket shape in `tests/harness/mod.rs`), a pane whose window has
  the option `off` reads `on` after the start call and reads `off` again
  after the exit call, and the window's own value is `off` throughout.
- `…_repeats_write_nothing`: the same state twice writes one report.
- `…_clear_only_after_a_report`: an emitter that never reported writes
  nothing at finish. One that reported writes exactly one `clear`.
- `…_frame_mapping` (pure): every prompt frame named in Behaviour 1 maps to
  its `blocked` kind. `Token`, `ToolStarted` and `ToolResult` map to
  `working`. `KeepAlive` maps to no change.
- `…_reply_returns_to_working`: after a prompt's reply, the emitter reads
  `working` with no further frame.
- `…_not_a_terminal_writes_nothing`.

## Acceptance criteria

- [ ] Every test in the Test plan passes, as printed by the end-to-end run.
      None of them exists today.
- [ ] `readme_mentions=1` or more: README.md names `allow-passthrough`.
      Today 0.
- [ ] `tmux_calls=2`: `src/tmux/session.rs` has both pane-option calls.
      Today 0.
- [ ] Hygiene, `doc_truth`, `bug_tracker` and `lock_invariant` are ok.
      `build_warnings=0`, clippy is clean, `fmt_exit=0`, `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
cargo test --lib program_status_emit 2>&1 | grep -E '^test |^test result'
grep -c 'allow-passthrough' README.md | sed 's/^/readme_mentions=/'
grep -c '"allow-passthrough"' src/tmux/session.rs | sed 's/^/tmux_calls=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker lock_invariant; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

The live clause (a real chat approval prompt seen as `blocked` by a real
7501-aware terminal) is checked by the architect at milestone close: the
chat client runs inside a DaemonEye shell, and `list_shells` reads it
through 21b. It is not part of this phase's run.

## Authorizations

- **Create:** `src/cli/program_status_emit.rs` (and its tests, inline or a
  sibling `_tests.rs`). Register it in `src/cli/mod.rs`.
- **Edit:** `src/tmux/session.rs` (the two pane-option calls);
  `src/cli/commands/stream.rs`, `stream_ask.rs`, `stream_prompts.rs`,
  `chat.rs`, `chat_loop.rs`, `ask.rs`, `src/cli/terminal_guard.rs`,
  `src/cli/render_ratatui.rs` (only if the guard's construction must carry
  the clear), and README.md (one paragraph).
- **Allowlist:** if the hygiene gate reports a file or function over its
  cap, raise its entry and name it in Findings. Don't split code to fit.
- **Scratch files** go under `.rexymcp/output/`.

## Out of scope

- The feature query (`OSC 7501 ; ?`) and `CSI c` detection. The spec makes
  detection optional, and terminals must ignore unknown OSCs.
- `progress`, `msg` and `title` on the client's reports.
- A config key or flag to turn reports off.
- A `doctor` line for `allow-passthrough`, or setting it anywhere but the
  client's own pane.
- `done` or `error` from the client. A finished turn is `idle`.

## Update Log

### Update — 2026-11-11 20:03 (started)

Status flipped to `in-progress`; milestone README row matches. Executor:
GLM (glm-5.3-flash), headless (phase-21c-program-status-emit).


### Notes for executor — 2026-11-11 (refined re-dispatch)

The first run was stopped by the PE after 446 turns. It **changed no source
file**: 289 `grep` calls, 71 `sed -n` reads, and 21 `tmux` commands. Every
symbol you need is in the map below. **Read those files and write.**

**Rules for this run:**

- Your **first edit lands within your first 15 turns**. It is step 1 of the
  edit order: create `src/cli/program_status_emit.rs`.
- **Never run `tmux` without `-S <socket>`.** This machine's default tmux
  server is the PE's live session, and the first run changed options on its
  pane `%0`. Every tmux command you run, by hand or in a test, names a
  private socket under a temp dir.
- Grep only for a symbol named in this doc or one you saw in a file you
  read. Run `cargo build` after every edit, and
  `cargo test --lib program_status_emit` after every test you add.

**Symbol map** (all of these exist today):

| What | Where |
|---|---|
| The 21a consumer for the round-trip test | `crate::shell::screen::ShellScreen::new(rows, cols, scrollback)`, `.feed(&[u8])`, `.programs()`, which returns `Vec<crate::shell::program_status::Record>` (fields `id`, `state`, `kind`, `app`, …) |
| State and kind enums to reuse | `crate::shell::program_status::{State, Kind}` |
| A tmux call | `crate::process::bounded_output(&mut std::process::Command)` in `src/process.rs`. `client_supports_rgb` in `src/tmux/session.rs` is the shape |
| `$TMUX` and empty-means-absent | `detect_from_env` in `src/cli/palette.rs` |
| Is stdout a terminal | `std::io::IsTerminal`, used in `src/cli/commands/top.rs` |
| The exit path every renderer shares | `restore_terminal()` in `src/cli/terminal_guard.rs`. `TerminalGuard::arm(restore_terminal)` is in `RatatuiRendererStdout::new` (`src/cli/render_ratatui.rs`), and the panic hook calls it too. It is a plain `fn()`, so the clear goes through process-wide state in the new module, which `restore_terminal` calls |
| The turn | `ask_with_session_ratatui` in `src/cli/commands/stream_ask.rs`, one `Response::…` arm per frame (`ToolCallPrompt`, `CredentialPrompt`, `PaneSelectPrompt`, `EditFilePrompt`, the script/runbook/schedule prompts, `Token`, `ToolStarted`, `ToolResult`, `KeepAlive`, `Error`, `Ok`) |
| The turn's context | `RatatuiQueryCtx` in `src/cli/commands/stream.rs`, built in `ask.rs` and twice in `chat_loop.rs` |
| A private tmux server in a test | `tmux -S <tempdir>/sock new-session -d -s t`, then target the pane as `t:0.0`, and `tmux -S <sock> kill-server` at the end. Read the window value with `show -wv -t t:0 allow-passthrough` |

**Edit order.** Each step ends at a green `cargo build`.

1. Create `src/cli/program_status_emit.rs` and register it in `src/cli/mod.rs`.
   It holds the pure parts:
   - the payload for a state,
   - the bare frame,
   - the tmux frame,
   - the mapping from a `&Response` to a state change,
   - the two pane-option argv builders.
2. Add the emitter, which writes through an injected writer and keeps the
   last state and whether it has reported. Then add the process-wide hook
   that `restore_terminal` calls to write `clear` and unset the pane option.
3. Add the two tmux calls to `src/tmux/session.rs`, taking an optional
   socket path so the live test can point them at its private server.
4. Wire it: add a `RatatuiQueryCtx` field and its three builders. In
   `ask_with_session_ratatui`, set the state from each frame through the
   mapping, and set `working` after each prompt's reply is sent. Set `idle`
   at every return.
5. Write the tests (each named `program_status_emit_…`), then the README
   paragraph, then the end-to-end block.

The red-runs step may record the first compile failure of the new tests.
It doesn't need to come before step 1.

### Update — 2026-11-11 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** The first run was read paralysis: the PE stopped it at 446
turns with no source edit. No spec contradiction surfaced. The 21b
treatment (a symbol map, an edit order that starts with a write) got this
model writing code on 21b, so it comes before a takeover. The run also
changed options on the PE's live tmux pane (each one undone, and checked
clean afterwards), so the notes now forbid `tmux` without `-S`.


### Update — 2026-11-12 02:15 (red runs)

`cargo test --lib program_status_emit` against a tree that carries the new
module and tests but none of the wiring (`.rexymcp/output/p21c-red-run.txt`):

```
error[E0425]: cannot find function `set_pane_passthrough` in module `crate::tmux`
error[E0425]: cannot find function `unset_pane_passthrough` in module `crate::tmux`
error[E0425]: cannot find function `pane_option_argv` in module `crate::tmux`
error[E0425]: cannot find function `pane_option_argv` in module `crate::tmux`
error[E0425]: cannot find function `set_pane_passthrough` in module `crate::tmux`
error[E0425]: cannot find function `unset_pane_passthrough` in module `crate::tmux`
error: could not compile `daemoneye` (lib test) due to 6 previous errors
```

An earlier red run against the same worktree with the emitter still
unimplemented (`.rexymcp/output/p21c-red.txt`) failed at 35 errors: every
test symbol missing (`tests_harness`, `run_tmux`, `pane_value`, `Emitter`,
`report_to`, `finish_to`, `frame_state`, …), which is the failing-before-
passing state the red step asks for.

### Update — 2026-11-12 02:15 (end-to-end verification)

Ran the block in § End-to-end verification once, redirected to
`.rexymcp/output/p21c-e2e.txt`; pasted verbatim:

```
test cli::program_status_emit::tests::program_status_emit_not_a_terminal_writes_nothing ... ok
test cli::program_status_emit::tests::program_status_emit_clear_only_after_a_report ... ok
test cli::program_status_emit::tests::program_status_emit_empty_tmux_var_means_bare ... ok
test cli::program_status_emit::tests::program_status_emit_tmux_pane_option_argv ... ok
test cli::program_status_emit::tests::program_status_emit_frame_mapping ... ok
test cli::program_status_emit::tests::program_status_emit_repeats_write_nothing ... ok
test cli::program_status_emit::tests::program_status_emit_reply_returns_to_working ... ok
test cli::program_status_emit::tests::program_status_emit_tmux_wraps_and_doubles_escapes ... ok
test cli::program_status_emit::tests::program_status_emit_each_state_round_trips_through_the_host_parser ... ok
test cli::program_status_emit::tests::program_status_emit_tmux_pane_option_set_and_unset_live ... ok
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 2788 filtered out; finished in 0.14s
readme_mentions=1
tmux_calls=2
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.02s
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 12.11s
fmt_exit=0
passed=2880 failed=0
```

### Findings — 2026-11-12

- Allowlist raised twice per the Authorizations, both reasons in place:
  `fn src/cli/commands/stream_ask.rs ask_with_session_ratatui` 725 → 745
  (the per-frame `status.frame(&msg)` call, the nine `status.reply()` calls
  after each prompt's send, and the two `status.idle()` error returns land
  inside the function) and `fn src/cli/commands/chat_loop.rs
  run_chat_ratatui` 537 → 544 (the emitter's one-line construction and the
  two `status: &emit_status` context fields). No code was split to fit.
- The e2e gate's `tmux_calls=2` counts the `"allow-passthrough"` literal in
  `src/tmux/session.rs`: both pane-option argv entries live in
  `tmux::pane_option_argv` (the spec's pinned pure shape), and
  `set_pane_passthrough`/`unset_pane_passthrough` carry that argv out.
- Red-run note: the first red artifact (`.rexymcp/output/p21c-red.txt`) was
  captured mid-implementation; the entry above pastes the final red run
  (`.rexymcp/output/p21c-red-run.txt`), taken against a HEAD tree carrying
  only the new module and tests.

