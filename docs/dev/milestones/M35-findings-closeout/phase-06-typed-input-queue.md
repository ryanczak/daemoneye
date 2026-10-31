# Phase 06 — typed-input-queue

**Status:** todo
**Milestone:** M35 — Findings closeout
**Drafted:** 2026-10-31
**Depends on:** phase 05 (done)
**Tags:** language=rust, kind=fix, size=m

## Goal

M32 #18 is closed. Today, text typed while a chat turn is streaming is
read and dropped: `select_stream` (`src/cli/commands/stream.rs`) passes each
key only to `key_outcome` and `InterruptState`, and a `Key::Char` matches
neither. PE decision (2026-10-31, M35 README decision 1): **typed text is
queued.** After this phase:

- **During the turn,** printable characters, Backspace and Enter typed while
  the spinner runs are kept in a per-turn `Typeahead`, and the input box
  under the spinner shows the kept text. Today the box is drawn empty by
  `render_spinner_region` (`src/cli/render_ratatui.rs`).
- **When the turn ends:**
  - If Enter was pressed, the kept text is sent as the next message at once,
    with no second Enter.
  - Otherwise the next input line starts pre-filled with it, cursor at the
    end, for the user to finish or edit.
- **Every other key behaves as today.** Ctrl-C's warn and abort, Ctrl-O, and
  focus events reach `key_outcome` and `InterruptState` unchanged. A paste
  (`Key::Paste`) is out of scope and still ignored.
- **After Enter is pressed mid-turn,** further characters are ignored for
  that turn. Only one message is queued.

The suite goes from **2628 to exactly 2631**.

## Constraints

- **`Typeahead` is a small pure type in `stream.rs`.** It offers
  `feed(&Key) -> bool`, which returns true when it consumed the key: `Char`,
  `Backspace` and `Enter`, the last only once. Its accessors give the kept
  text and whether Enter was pressed. `select_stream` takes `&mut Typeahead`
  and feeds it before `key_outcome`. A key it consumed goes no further. Every
  other key follows today's path.
- **Where the state lives:** `run_chat_ratatui` (`chat_loop.rs`) creates a
  fresh `Typeahead` each turn and hands it down through
  `ask_with_session_ratatui` to its `select_stream` call. After the turn it
  either submits or pre-fills through
  `InputState::current_line_mut().insert_str`. Any other `select_stream`
  caller passes a throwaway `Typeahead::default()`.
- **Display:** `draw_spinner` (`src/cli/render_ratatui_draw.rs`, three call
  sites) and `render_spinner_region` take the kept text and draw it in the
  input box, where today they draw `""`.
- **Headroom:** `render_ratatui.rs` 689 of 800 production lines,
  `chat_loop.rs` 725, `stream.rs` 234. `run_chat_ratatui` (500) and
  `ask_with_session_ratatui` (673) are allowlisted. If a fix grows one, raise
  its `tests/hygiene_allowlist.txt` line and say why in the Update Log (M35
  README).
- **Use `patch` with whole blocks,** never `patch_lines`.

## Spec

1. **The type** — add `Typeahead` with `feed` returning false for every key,
   and write `typeahead_keeps_text_and_one_enter`: feeding `h`, `i`,
   Backspace, `o` keeps `"ho"` with no Enter; Enter is consumed and marks it
   sent; a later `x` is not consumed; `Key::CtrlC` and `Key::FocusGained` are
   never consumed. Run it, keep the failing output, then implement `feed`.
   The test passes.
2. **The stream** — give `select_stream` the `&mut Typeahead` parameter,
   unused at first. Write `select_stream_keeps_typed_text_for_the_next_turn`
   beside `select_stream_first_interrupt_press_warns` and built the same way:
   write `hi` then `\r` to the stdin pipe, then a full daemon line to the
   socket. Assert the outcome is `Msg`, and that the typeahead holds `"hi"`
   with Enter pressed. Run it, keep the failing output, then feed the
   typeahead in both `select!` arms. The test passes, and the existing
   `select_stream_*` tests still do.
3. **The box** — give `render_spinner_region` and `draw_spinner` the text
   parameter, still drawing `""`. Write
   `spinner_draws_the_typeahead_in_the_input_box` in
   `render_ratatui_tests.rs`: draw the spinner with `"queued text"` on a
   `TestBackend` and assert the buffer shows it. Run it, keep the failing
   output, then draw the text. The test passes.
4. **Wire the turn** — `run_chat_ratatui` creates the `Typeahead`, passes it
   down, and after the turn submits (Enter pressed) or pre-fills (no Enter).
5. **Mark the ledger** — in `docs/dev/milestones/M32-findings-fixes/FINDINGS.md`
   row #18, replace the triage prefix `act: M35 phase-06` with
   `fixed: M35 phase-06, typeahead`. Keep the `(was: …)` text.
6. **Red runs** — one `### Update — <date> (red runs)` entry pasting steps
   1–3's `test result:` lines and assertion messages.
7. **Findings** — one `### Findings — <date>` entry from `git diff --stat`
   alone, two or three lines, or `none`.
8. **End-to-end** — run the block once before committing; paste its output
   fenced in an `### Update — <date> (end-to-end verification)` entry. This
   step is required. The reviewer checks for the entry.
9. **Commit** as `fix: text typed during a turn is kept and sent when it ends (M32 #18)`.

## Acceptance criteria

- [ ] The E2E block's first line prints
      `typeahead=1 empty_box=0 new_tests=3 fixed=1`
      (today `typeahead=0 empty_box=1 new_tests=0 fixed=0`).
- [ ] The red-runs entry shows all three new tests failing before their fixes.
- [ ] hygiene, doc_truth and bug_tracker pass; `build_warnings=0`; clippy
      clean; `fmt_exit=0`.
- [ ] `cargo test` reports exactly `passed=2631 failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
echo "typeahead=$(grep -c 'struct Typeahead' src/cli/commands/stream.rs) empty_box=$(sed -n '/^fn render_spinner_region/,/^}/p' src/cli/render_ratatui.rs | grep -c 'Paragraph::new("")') new_tests=$(( $(grep -cE 'fn (typeahead_keeps_text_and_one_enter|select_stream_keeps_typed_text_for_the_next_turn)\(' src/cli/commands/stream.rs) + $(grep -c 'fn spinner_draws_the_typeahead_in_the_input_box' src/cli/render_ratatui_tests.rs) )) fixed=$(grep -cE '^\| 18 \|.*fixed: M35 phase-06' docs/dev/milestones/M32-findings-fixes/FINDINGS.md)"
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/cli/commands/stream.rs`, `stream_ask.rs`, `chat_loop.rs`,
  `src/cli/render_ratatui.rs`, `render_ratatui_draw.rs`,
  `render_ratatui_tests.rs`, row #18 of the M32 ledger, and
  `tests/hygiene_allowlist.txt` (only per the rule above).

## Out of scope

- Pastes during a turn, more than one queued message, and typing during an
  approval or credential prompt (those prompts read keys themselves).

## Update Log

<!-- entries appended below this line -->
