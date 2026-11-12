# Phase 21d — live-check-fixes

**Milestone:** M36 — Chat UX and audit fixes
**Status:** in-progress
**Drafted:** 2026-11-12
**Depends on:** phases 01, 04b and 17a (it corrects each one's live result)
**Estimated diff:** ~150 lines, more than half of it tests
**Tags:** language=rust, kind=fix, size=s

## Goal

The milestone's live check on 2026-11-12 (README § Retrospective) failed
three exit criteria. Each is one small defect in a phase already `done`:

1. **Paste.** A bracketed paste whose lines end in CR (what tmux and most
   terminal emulators send) arrives in the input box as one joined line and
   reaches the model as `one\rtwo\rthree`. Only an LF paste arrives as
   three lines. Criterion: "a three-line paste into the input box arrives as
   three lines".
2. **Chat shells after exit.** The detach grace defaults to 300 s, so a
   chat session's shells are still listed four minutes after it exits.
   Criterion: "After `daemoneye chat` exits, its shells are gone from
   `list_shells` within a minute".
3. **`top` attach.** The attach view reads `exited` the moment it opens,
   on a shell whose command is still running, and never updates. Its
   opener drops the connection's write half on return, the daemon reads
   that as the client hanging up and stops relaying, and the stream's end
   marks the model exited. Criterion: "in `top`, Enter on a shell row shows
   that shell's screen updating live".

This phase fixes the three and nothing else, so the milestone can close.

## Architecture references

- `src/cli/input/tty.rs`: `read_key`, `read_bracketed_paste` (returns the
  bytes between `ESC[200~` and `ESC[201~` as a `String`), `Key::Paste`, and
  the `read_key_bracketed_paste_*` tests, which feed bytes through a pipe.
- `src/cli/input/editor.rs`: `insert_str`, the one consumer that puts a
  paste in the input box; `src/cli/commands/stream_prompts.rs`
  (`approval_step`, `credential_step`): the two prompt consumers of
  `Key::Paste`.
- `src/config/shells.rs`: `default_shells_detach_close_secs`;
  `src/config/tests.rs`: `detach_close_secs_defaults_to_300_and_zero_is_allowed`.
  The number is also documented in `assets/etc/config.toml` (the seeded
  config's comment), `assets/memory/knowledge/config-reference.md` (the
  seeded memory's table) and `README.md` (two places: the shells paragraph
  and the `[shells]` reference table and example).
- `src/cli/commands/top_tui_attach.rs`: `open_attach_frames` (connects,
  sends `Request::ShellTail { follow: true }`, reads the first frame, spawns
  `forward_frames`), `AttachView`, `AttachView::apply_frame`, and the test
  seam `opened`, which drives `forward_frames` over a `tokio::io::duplex`.
  `src/cli/tail_view.rs` is the `/tail` viewer, whose `tx` stays in scope for
  the whole loop, so `/tail` does not have this defect.
- `src/daemon/server/shells.rs`, the `ShellTail` follow loop: a client
  `next_line()` of `Ok(None)` ends the relay ("the client hung up").
  That is the daemon-side behaviour the attach view must not trigger.
  Not edited in this phase.

## Behaviour

1. **Line ends in a paste.** `Key::Paste`'s text has LF line ends only:
   `\r\n` becomes `\n`, and a bare `\r` becomes `\n`. A paste with no CR is
   unchanged. The conversion happens where the paste is read, so every
   consumer (the input box, the approval reader, the credential reader)
   sees the same text. An input-box paste of `a\rb\rc` is then three lines,
   exactly as `a\nb\nc` is today.
2. **The detach grace.** `[shells] detach_close_secs` defaults to **60**.
   `0` still means never close. Every document that states the default
   says 60.
3. **The attach view stays connected.** The attach view keeps its
   connection's write half for as long as the view is open, so the daemon
   keeps relaying. A frame the daemon sends after the view opened reaches
   the view, and the model reads attached, not exited, until the daemon
   sends `ShellExited` or really closes the stream. Closing the view drops
   the write half with the rest of the view, as it does today, so the
   daemon still sees the hang-up then.

## Constraints

- Do **not** convert CR in `Key::Char` or `Key::Enter` handling; a typed
  Enter is already `Key::Enter`. Only the paste text changes.
- Do not make the tail model or `forward_frames` ignore stream end; a
  stream that really ends must still mark the view exited
  (`top_attach_stream_end_marks_exited_and_keeps_the_view` keeps passing).
  The fix is on the opener's side: keep the write half alive.
- `OwnedWriteHalf` shuts the socket's write side down when dropped. Holding
  it in the view is the plain fix; `forget()` leaks the half and is not
  wanted.
- Never run `tmux` without `-S <socket>`. No test in this phase needs tmux.

## Spec

1. `src/cli/input/tty.rs`: normalise the paste text's line ends per
   Behaviour 1 where `Key::Paste` is built (one place).
2. `src/config/shells.rs`: the default is 60. Rename the config test to
   `detach_close_secs_defaults_to_60_and_zero_is_allowed` and assert 60.
   Update every site the e2e's `default_300` grep finds (seven lines across the four files named under Architecture references, the doc comment on the default fn included).
3. `src/cli/commands/top_tui_attach.rs`: `open_attach_frames` returns the
   write half along with the model, receiver and task, and `AttachView`
   holds it (a field, dropped with the view). The `opened` test seam gives
   the view a write half too; a `duplex` is one stream, so split it or hold
   the client's write side in a way that drops with the view. Then add a
   seam a test can use to prove the daemon-side behaviour: one function
   takes an already-connected `tokio::net::UnixStream` and does everything
   after the connect (the request, the first frame, the task, the view).
   `open_attach_frames` connects and calls it.

## Test plan

New tests, each named so `cargo test <name>` runs it alone:

- `read_key_bracketed_paste_cr_line_ends_become_lf` (`src/cli/input/tty.rs`
  tests): a paste `a\rb\rc` yields `Key::Paste("a\nb\nc")`.
- `read_key_bracketed_paste_crlf_line_ends_become_lf`: `a\r\nb\r\n` yields
  `Key::Paste("a\nb\n")`, one LF per line end, no doubled newline.
- `read_key_bracketed_paste_lf_is_unchanged`: `a\nb` stays `a\nb`
  (preservation; the existing multiline test covers part of this, so this
  test may extend it instead of adding a new one).
- `detach_close_secs_defaults_to_60_and_zero_is_allowed` (renamed from the
  300 test; the 0 and 42 cases stay).
- `top_attach_a_frame_after_open_reaches_the_view`
  (`src/cli/commands/top_tui_attach.rs` tests): over a
  `tokio::net::UnixStream::pair()`, a fake daemon reads the request line,
  writes a `ShellScreen` first frame, then **reads again and must not get
  EOF**: it writes a `ShellChunk` after the view opened, and the test reads
  that chunk from `view.frames` and sees `view.tail.exited == false`.
  Against today's tree the fake daemon reads EOF right after the first
  frame (that is the defect), so the test fails before the fix. Keep the
  fake daemon's reads bounded with `tokio::time::timeout`.
- `top_attach_close_hangs_up` (same file): after `drop(view)`, the fake
  daemon's next read gets EOF (preservation of the daemon-side hang-up on
  close).

## Acceptance criteria

- [ ] Every test in the Test plan passes, as printed by the end-to-end
      run. None of them exists today.
- [ ] `cr_sites=0`: no `\r` survives in a paste: the e2e's `paste_probe`
      prints `Paste("a\nb\nc")`. Today it prints `Paste("a\rb\rc")`.
- [ ] `default_300=0`: none of the four files that state the detach grace
      (`src/config/shells.rs`, `assets/etc/config.toml`,
      `assets/memory/knowledge/config-reference.md`, `README.md`) says 300
      within three lines of naming it. Today 7 lines.
- [ ] `default_60=7`: the same seven lines say 60. Today 0.
- [ ] `detach_close_secs_defaults_to_300_and_zero_is_allowed` no longer
      exists (`old_test=0`). Today 1.
- [ ] Hygiene, `doc_truth`, `bug_tracker` and `lock_invariant` are ok.
      `build_warnings=0`, clippy is clean, `fmt_exit=0`, `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
cargo test --lib read_key_bracketed_paste 2>&1 | grep -E '^test |^test result'
cargo test --lib detach_close_secs 2>&1 | grep -E '^test |^test result'
cargo test --lib top_attach 2>&1 | grep -E '^test |^test result'
cargo test --lib read_key_bracketed_paste_cr_line_ends_become_lf -- --nocapture 2>&1 | grep -oE 'Paste\("[^"]*"\)' | head -1 | sed 's/^/paste_probe=/'
F="src/config/shells.rs assets/etc/config.toml assets/memory/knowledge/config-reference.md README.md"
grep -hE -A3 'detach_close_secs|detach-close grace' $F | grep -cE '\b300\b' | sed 's/^/default_300=/'
grep -hE -A3 'detach_close_secs|detach-close grace' $F | grep -cE '\b60\b' | sed 's/^/default_60=/'
grep -rc 'detach_close_secs_defaults_to_300' src | grep -v ':0' | wc -l | sed 's/^/old_test=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker lock_invariant; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

`paste_probe` needs the CR test to print the key it read (a `println!` in
the test body is fine; tests may print). The three live criteria are
re-checked by the architect at milestone close with the throwaway harness,
not by this run.

## Authorizations

- **Edit:** `src/cli/input/tty.rs`, `src/config/shells.rs`,
  `src/config/tests.rs`, `src/cli/commands/top_tui_attach.rs`,
  `src/cli/commands/top_tui.rs` (only if the view's construction or close
  must change there), `assets/etc/config.toml`,
  `assets/memory/knowledge/config-reference.md`, `README.md` (the number
  only).
- **Allowlist:** if the hygiene gate reports a file or function over its
  cap, raise its entry and name it in Findings. Don't split code to fit.
- **Scratch files** go under `.rexymcp/output/`.

## Out of scope

- A chat shell closed by the detach grace stays listed at `shell prompt`
  until `reap_exited` stamps it (`exited_retention_secs`). By design.
- The `wait: false` dedicated shell surviving chat exit. By design (17a).
- Word-wrap of AI prose breaking inside a word at 80 columns.
- `/tail`, which already holds its write half.
- Any change to the daemon's `ShellTail` relay.

## Update Log

<!-- entries appended below this line -->
### Update — 2026-11-12 19:39 (started, executor claude-sonnet-4-5-20250929)

Status flipped to in-progress. Reading the four touched files, then implementing the
three fixes and their tests per the Spec.

### Update — 2026-11-12 19:52 (end-to-end verification)

Ran the phase's E2E block mechanically, redirecting all output to
`.rexymcp/output/e2e-21d.log` and pasting it below verbatim:

```
test cli::input::tty::tests::read_key_bracketed_paste_multiline ... ok
test cli::input::tty::tests::read_key_bracketed_paste_single_line ... ok
test cli::input::tty::tests::read_key_bracketed_paste_no_enter_mid_paste ... ok
test cli::input::tty::tests::read_key_bracketed_paste_crlf_line_ends_become_lf ... ok
test cli::input::tty::tests::read_key_bracketed_paste_cr_line_ends_become_lf ... ok
test cli::input::tty::tests::read_key_bracketed_paste_lf_is_unchanged ... ok
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 2799 filtered out; finished in 0.00s
test config::tests::detach_close_secs_defaults_to_60_and_zero_is_allowed ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2804 filtered out; finished in 0.00s
test cli::commands::top_tui::attach::tests::top_attach_enter_on_the_first_shell_opens_it ... ok
test cli::commands::top_tui::attach::tests::top_attach_open_error_leaves_the_panels_and_names_it ... ok
test cli::commands::top_tui::attach::tests::top_attach_enter_or_a_opens_only_on_a_selected_shell ... ok
test cli::commands::top_tui::attach::tests::top_attach_close_hangs_up ... ok
test cli::commands::top_tui::attach::tests::top_attach_keys_close_on_q_esc_ctrl_o_and_ctrl_c_quits_top ... ok
test cli::commands::top_tui::attach::tests::top_attach_view_draws_the_shell_and_close_restores_the_panels ... ok
test cli::commands::top_tui::attach::tests::top_attach_footer_names_enter_attach_and_the_open_error ... ok
test cli::commands::top_tui::attach::tests::top_attach_stream_end_marks_exited_and_keeps_the_view ... ok
test cli::commands::top_tui::attach::tests::top_attach_a_frame_after_open_reaches_the_view ... ok
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 2796 filtered out; finished in 0.50s
paste_probe=Paste("a\nb\nc")
default_300=0
default_60=7
old_test=0
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.03s
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.43s
fmt_exit=0
passed=2887 failed=0
```

Every Test-plan test passes as printed (`paste_probe=Paste("a\nb\nc")`,
`default_300=0`, `default_60=7`, `old_test=0`), hygiene / doc_truth /
bug_tracker / lock_invariant are ok, `build_warnings=0`, clippy clean,
`fmt_exit=0`, full suite `passed=2887 failed=0`.
