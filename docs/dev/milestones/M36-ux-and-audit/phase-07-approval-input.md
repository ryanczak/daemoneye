# Phase 07 — approval-input

**Milestone:** M36 — Chat UX and audit fixes
**Status:** todo
**Drafted:** 2026-11-05
**Depends on:** phase 06 (the Ctrl+O viewer at write/edit approvals)
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=bug, size=s

## Goal

Fix findings S2 and S3 (`FINDINGS.md`).

- **S2:** `stream_prompts.rs` · `read_approval_input_with` decides on the
  first key. A redirect typed as `abort that` or `actually …` approves the
  command, and `a` also records a session-wide approval. The rest of the
  message lands in the next reader.
- **S3:** that reader and `prompt_credential_ratatui` read raw bytes while
  the renderer has bracketed paste (mode 2004) and focus reporting (mode 1004)
  turned on.
  - A pasted sudo password starts with ESC, so the reader sends an empty
    credential, and `[200~hunter2…` lands in the next chat input.
  - A focus event or arrow key at an approval starts a typed message.
  - Bytes ≥ 0x80 are pushed as `b as char`, so non-ASCII passwords and
    redirects arrive mangled.

## Architecture references

- `src/cli/input/tty.rs` · `read_key`, which the chat input already uses.
  It decodes UTF-8 and returns bracketed paste as `Key::Paste(String)`,
  focus reports as `FocusGained`/`FocusLost`, a bare Escape as
  `Key::Char('\x1b')` and other control bytes as `Key::Char('\0')`. Its tests
  feed it through `make_pipe_stdin`.
- `src/cli/commands/stream_prompts.rs` · `read_approval_input_with`, the one
  approval reader behind the command panel, the yes/no and pane prompts and
  the write/edit prompts; `approval_first_key` / `ApprovalKey` /
  `VIEW_SIGNAL` (phase 06); `parse_approval_response`;
  `prompt_credential_ratatui`.

## Behaviour

**Approval line (PE decision 2026-11-01): y/n/a take effect on Enter.**
Every key edits a line, and only Enter submits it. The submitted text goes to
`parse_approval_response` unchanged, so `y`/`yes` approve, `a` approves for
the session, `n`/`no`/empty deny, and anything else is a redirect.
`y` followed by another character is a redirect, not an approval.

- A printable `Char` is inserted, decoded, so `é` arrives as `é`.
  Backspace removes one char.
- `Paste(text)` inserts the text as one value. It never submits, even if it
  contains a newline: CR/LF inside a paste become spaces.
- Ctrl+C or a bare Escape submits the empty line (a deny), as today.
- Ctrl+O opens the diff viewer where it is offered (the write/edit prompts)
  and is ignored elsewhere. It never edits the line.
- Everything else is ignored and leaves the line unchanged: focus events,
  arrows, Home/End, PageUp/PageDown, mouse reports, `Char('\0')` and the
  other Ctrl keys.
- The approval deadline still bounds the whole read.

**Credential.**
- A printable `Char` is appended, decoded.
- `Paste(text)` appends the text with its CR/LF removed. It never submits.
- Backspace removes one char.
- Enter submits. Ctrl+C or a bare Escape clears the value and submits it
  empty, as today.
- Everything else is ignored, as above.
- The masked display shows one `•` per char.

## Constraints

- Put the key handling in pure step functions that take a `Key` and return
  pending / done(text) / view, one for the approval line and one for the
  credential. Tests drive them with `Key` sequences. The async readers only
  loop `read_key` → step → redraw. Bound the approval read by its deadline
  the way `read_byte_by` does (`tokio::time::timeout_at`).
- `approval_first_key` and `ApprovalKey` go away. The phase 06 test
  `approval_diff_ctrl_o_is_not_an_answer` keeps its name and meaning (Ctrl+O
  views where offered and is ignored elsewhere, and is never an answer) but
  is rewritten against the approval step function. Folding `VIEW_SIGNAL`
  into an enum return is allowed but not required.
- `read_key` itself is unchanged.
- Preservation: the `parse_approval_decision_*` tests,
  `approval_read_times_out` and the `read_key_*` tests stay green without
  being edited, and so do the other `approval_diff_*` tests.
- If a prompt's hint text says a single key answers at once, correct it.
  `build_approval_prompt` and the panel's options line need no change
  otherwise.

## Spec

1. **Red tests first.** Write these against the current tree and run them.
   Keep the failing output for step 4. All carry the `approval_input_`
   prefix:
   - `approval_input_letter_waits_for_enter`: `a` leaves the line pending.
     `abort that` + Enter submits `abort that`, which
     `parse_approval_response` turns into a redirect: not approved, no
     session grant. `y` + Enter submits `y`.
   - `approval_input_paste_is_one_value`: `Paste("yes\nplease")` stays
     pending, and Enter then submits `yes please`.
   - `approval_input_ignores_focus_and_arrows`: `FocusGained`, `FocusLost`,
     `Up`, `Left`, a `Mouse` and `Char('\0')` leave the line empty and
     pending. `y` + Enter then submits `y`.
   - `approval_input_decodes_utf8`: `c a f é` + Enter submits `café`.
   - `approval_input_ctrl_c_and_escape_deny`: `CtrlC` submits empty, and so
     does `Char('\x1b')`.
   - `approval_input_credential_paste_is_the_value`: `Paste("hunter2\n")`
     stays pending, and Enter then submits `hunter2`.
   - `approval_input_credential_ignores_focus_and_arrows`: focus events and
     arrows leave the value empty. `p ä ß` Backspace Enter submits `pä`.
   - `approval_input_credential_escape_clears`: `x y` then `Char('\x1b')`
     submits empty.
2. **Implement** per § Behaviour, in this order, running `cargo build`
   after each: the approval step function, the approval reader on
   `read_key`, the credential step function, the credential reader.
3. **Run tests**: `cargo test` passes with `failed=0`.
4. **Red runs.** Add one `### Update — <date> (red runs)` entry. Paste
   step 1's `test result:` lines and assertion or compile messages verbatim,
   with nothing elided.
5. **Findings.** Add one `### Findings — <date>` entry, two or three lines,
   or `none`.
6. **End-to-end.** Run § End-to-end verification once. Paste its output,
   fenced, in an `### Update — <date> (end-to-end verification)` entry.
7. **Commit** as `fix: approvals and the sudo prompt read keys, not bytes — y/n/a on Enter, paste as one value (M36 phase-07)`.

## Acceptance criteria

- [ ] The E2E block's first line reads `approval_input passed=P failed=0`
      with P ≥ 8. Today it reads `passed=0`.
- [ ] The E2E block shows `raw_byte_reads=0`: neither reader in
      `stream_prompts.rs` calls `stdin.read_byte()` directly. Today the
      count is 3.
- [ ] The E2E block shows `first_key_fn=0`: `approval_first_key` is gone.
      Today it is above 0.
- [ ] The red-runs entry shows the new tests failing on the pre-phase code.
- [ ] Preservation: `parse_approval_decision_*`, `approval_read_times_out`,
      `read_key_*` and `approval_diff_*` pass.
- [ ] The E2E block shows `hygiene`, `doc_truth` and `bug_tracker` `ok`,
      `build_warnings=0`, clippy clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
cargo test approval_input_ 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "approval_input passed=" p, "failed=" f}'
grep -c 'stdin\.read_byte()' src/cli/commands/stream_prompts.rs | sed 's/^/raw_byte_reads=/'
grep -rc 'approval_first_key' src/ | awk -F: '{s+=$2} END {print "first_key_fn=" s}'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/cli/commands/stream_prompts.rs`, `src/cli/commands/stream.rs`
  (tests only), and `src/cli/render_ratatui.rs` only if a hint string
  claims a single key answers at once.
- A new or raised `tests/hygiene_allowlist.txt` entry is acceptable if a
  touched file or function crosses its cap. State the reason in the
  Findings entry.

## Out of scope

- `read_key` and the chat input loop.
- Cursor movement inside the approval line. Arrows stay ignored.
- A deadline on the credential prompt. It has none today and gets none here.

## Update Log

<!-- entries appended below this line -->
