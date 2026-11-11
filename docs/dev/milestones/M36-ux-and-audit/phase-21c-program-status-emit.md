# Phase 21c — program-status-emit

**Milestone:** M36 — Chat UX and audit fixes
**Status:** todo
**Drafted:** 2026-11-11
**Depends on:** phase 21a (its parser checks the reports this phase writes)
**Estimated diff:** ~350 lines, about half of it tests
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
  `$TMUX`. An empty value means not in tmux.

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
5. **tmux (the passthrough decision).** When `$TMUX` is set and non-empty,
   each report is wrapped in tmux's passthrough envelope instead:
   `ESC P tmux ; <report with every ESC doubled> ESC \`. DaemonEye never
   changes tmux options, because tmux is a read-only observer. tmux forwards
   the envelope to the outer terminal only when the user has set
   `allow-passthrough on` (the default is `off`, and then tmux drops it
   silently). Outside tmux the bare report is written.
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
   the states, says reports are on by default, and gives the tmux
   requirement `set -g allow-passthrough on`. Step ends when the paragraph
   is in.
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
- [ ] Hygiene, `doc_truth`, `bug_tracker` and `lock_invariant` are ok.
      `build_warnings=0`, clippy is clean, `fmt_exit=0`, `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
cargo test --lib program_status_emit 2>&1 | grep -E '^test |^test result'
grep -c 'allow-passthrough' README.md | sed 's/^/readme_mentions=/'
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
- **Edit:** `src/cli/commands/stream.rs`, `stream_ask.rs`, `stream_prompts.rs`,
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
- Setting or probing tmux's `allow-passthrough`, or a `doctor` line for it.
- `done` or `error` from the client. A finished turn is `idle`.

## Update Log

<!-- entries appended below this line -->
