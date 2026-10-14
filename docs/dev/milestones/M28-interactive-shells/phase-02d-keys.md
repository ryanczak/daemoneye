# Phase 02d: keys — the key encoder, the screen's key modes, Enter as `\r`

**Milestone:** M28 — Interactive shells
**Status:** todo
**Depends on:** phase-02c
**Estimated diff:** ~400 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

`send_input` can type text and a trailing `\n`, and nothing else. A
full-screen program cannot be driven that way: it needs Escape, arrows,
`S-Tab`, Ctrl chords and a real Enter (`\r`). This phase builds the pure
encoder that turns tmux-style key names into the bytes a terminal sends,
has the host report the two terminal modes that change those bytes, and
makes `send_input`'s Enter a `\r`. The `keys` and `paste` tool parameters
that use the encoder are phase 02e. No tool is added, so no count-pinned
tool test moves.

## Architecture references

- `docs/design/interactive-shells.md` D8 — the key vocabulary, the
  modifier grammar, CSI u, and the paste rule.

## Spec

**Files:** `src/shell/keys.rs` (new), `src/shell/mod.rs`,
`src/cli/attach.rs`, `src/shell/screen.rs`, `src/shell/proto.rs`,
`src/shell/backend.rs`, `src/daemon/executor/knowledge/shell.rs`,
`src/daemon/executor/mod.rs` (one panel string), `src/ai/tools/defs.rs`
(one param description). Plus the struct literals the compiler names when
`ScreenText` grows (Task 3). Do not touch `src/ai/types/`,
`src/ai/tools/args.rs`, `src/daemon/stream.rs`, `assets/prompts/sre.toml`,
`CLAUDE.md` or `README.md` — phase 02e.

1. **`ShellModes` moves** — move `ShellModes` (and its `of`) from
   `src/cli/attach.rs` into `src/shell/keys.rs`, re-export it from
   `src/shell/mod.rs`, and have `attach.rs` import it. `attach_keys` and its
   tests are otherwise unchanged.
2. **The encoder** — in `keys.rs`, a pure
   `encode_keys(names: &[String], modes: ShellModes) -> anyhow::Result<EncodedKeys>`
   where `EncodedKeys { bytes: Vec<u8>, csi_u: Vec<String> }` (`csi_u` names,
   in order, the keys that went out as CSI u). Any bad name fails the whole
   call: no partial bytes. The encoding is § Key table below.
3. **The paste rule** — in `keys.rs`, a pure
   `paste_bytes(text: &str, paste: Option<bool>, modes: ShellModes) -> anyhow::Result<(Vec<u8>, bool)>`
   (bytes, bracketed?). `None` brackets iff `text` contains `\n` **and**
   `modes.bracketed_paste`; `Some(true)` with the mode off is an error naming
   "bracketed paste"; `Some(false)` never brackets. Brackets are
   `ESC [ 200 ~` … `ESC [ 201 ~`. Before wrapping, remove every
   `ESC [ 201 ~` from `text`, or the text could end the paste early and run
   the rest as typed keys.
4. **The screen reports its key modes** — `ShellScreen` gains
   `modes() -> ShellModes` (vt100's `bracketed_paste()` and
   `application_cursor()`). `ShellResponse::ScreenText` gains
   `bracketed_paste: bool` and `application_cursor: bool`, each
   `#[serde(default)]` so a host from before this phase reads as both off,
   which is the safe default. The backend fills them in its `ScreenText`
   reply.
5. **Enter is `\r`** — `send_input_with` appends `\r`, not `\n`, when
   `enter`. A cooked shell turns it into `\n` (`ICRNL`); a raw-mode app gets
   the byte a real Enter key sends. The result reads `, Enter sent` /
   `, no Enter` instead of `, newline appended` / `, no newline`; the
   approval panel string in `executor/mod.rs` reads `(Enter sent)`; the
   `enter` param description in `defs.rs` says it presses Enter (sends a
   carriage return). `wrap_command`'s terminator does not change.
6. **Capture the end-to-end evidence** — run § End-to-end verification
   verbatim and paste `/tmp/e2e-02d.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)`. The entry holds **only**
   the output fence (the self-check reads the first fence), followed by the
   `PASTE MATCH` line.

### Key table

Names are tmux's. Named keys match case-insensitively (`enter` = `Enter`);
the prefixes `C-`, `M-`, `S-` are upper-case and compose in any order
(`C-S-p` = `S-C-p`). After the prefixes comes either a named key or exactly
one printable ASCII character. Anything else — `Ctrl-c`, `F13`, `C-`,
`X-a`, an empty string — is an error that lists the vocabulary.

Plain keys (application cursor mode in brackets):

| Key | Bytes |
|---|---|
| `Enter` | `\r` |
| `Escape` | `\x1b` |
| `Tab` | `\t` |
| `BTab` (= `S-Tab`) | `\x1b[Z` |
| `BSpace` | `\x7f` |
| `Space` | ` ` |
| `Up` `Down` `Right` `Left` | `\x1b[A` `[B` `[C` `[D` (app: `\x1bOA` … `OD`) |
| `Home` `End` | `\x1b[H` `\x1b[F` (app: `\x1bOH` `\x1bOF`) |
| `PageUp` `PageDown` `Delete` | `\x1b[5~` `\x1b[6~` `\x1b[3~` |
| `F1`–`F4` | `\x1bOP` `\x1bOQ` `\x1bOR` `\x1bOS` |
| `F5`–`F12` | `\x1b[15~` `[17~` `[18~` `[19~` `[20~` `[21~` `[23~` `[24~` |
| a printable char | itself |

Modifiers, first rule that applies wins:

1. With `C-` as the only modifier: `C-<letter>` (either case) and `C-@ C-[ C-\ C-] C-^ C-_` → the control
   byte (`char & 0x1f`; `C-c` = `0x03`). `C-Space` = `0x00`.
2. With `S-` as the only modifier: `S-<letter>` → the upper-case letter. `S-Tab` → `\x1b[Z`.
3. Cursor, `Home`/`End`, `PageUp`/`PageDown`/`Delete` and `F1`–`F12` with any
   modifiers → xterm's parameter form with `m = 1 + (S?1) + (M?2) + (C?4)`:
   `\x1b[1;<m>A` for arrows/`Home`/`End` (final byte as the plain form),
   `\x1b[<n>;<m>~` for the `~` keys (`S-F5` = `\x1b[15;2~`), and
   `\x1b[1;<m>P`…`S` for `F1`–`F4`. Application cursor mode does not apply
   to these.
4. `M-<x>` where `<x>` alone (without `M-`) has a byte under the plain table
   or rules 1–2 → `\x1b` followed by those bytes (`M-b` = `\x1bb`,
   `M-Enter` = `\x1b\r`, `M-C-x` = `\x1b\x18`, `M-S-a` = `\x1bA`).
5. Everything else → **CSI u**, `\x1b[<cp>;<m>u` with `m` as in rule 3 and
   `cp` the key's code point: `Enter` 13, `Tab` 9, `Escape` 27, `BSpace` 127,
   `Space` 32, a letter's **lower-case** code point, any other char its own.
   `S-Enter` = `\x1b[13;2u`, `C-Enter` = `\x1b[13;5u`,
   `C-S-p` = `\x1b[112;6u`, `C-Tab` = `\x1b[9;5u`, `C-1` = `\x1b[49;5u`.
   The key's name goes into `csi_u`.

**Gotchas.**
- The two tests whose assertions change in `knowledge/shell.rs` are
  `send_input_appends_a_newline_by_default` — **renamed** to
  `send_input_enter_sends_a_carriage_return`, asserting the bytes are
  exactly `b"hello\r"` and the result says `Enter sent` — and
  `send_input_without_enter_sends_no_newline`, whose wording assertion
  becomes `no Enter`. That list is complete; `send_input` callers in other
  tests type via `crate::shell::send_input` directly and keep `\n`.
- Exhaustive `ScreenText { .. }` literals and patterns outside these files
  (tests in `src/shell/client.rs`, `src/shell/proto.rs`,
  `src/daemon/executor/knowledge/shell.rs`) gain the two fields with
  `false`; the compiler names them.

## Tests

In `src/shell/keys.rs`:
- `keys_encode_the_plain_vocabulary` — every row of the plain table,
  application cursor off.
- `keys_cursor_keys_follow_application_cursor_mode` — arrows and
  `Home`/`End` switch to `ESC O`; `PageUp` and `F5` do not.
- `keys_control_and_meta_use_legacy_bytes` — `C-c` 0x03, `C-A` 0x01,
  `C-Space` 0x00, `C-[` 0x1b, `S-a` `A`, `M-b`, `M-Enter`, `M-C-x`, `S-Tab`.
- `keys_modified_cursor_and_function_keys_use_xterm_parameters` — `C-Up`
  `\x1b[1;5A`, `S-F5` `\x1b[15;2~`, `M-F1` `\x1b[1;3P`, `C-S-Home`
  `\x1b[1;6H`, and `C-Up` unchanged with application cursor on.
- `keys_without_a_legacy_byte_go_out_as_csi_u` — the five rule-5 examples,
  and `csi_u` names exactly those keys in order; `["Up", "S-Enter"]` gives
  `csi_u == ["S-Enter"]`.
- `keys_unknown_name_is_an_error_naming_the_vocabulary` — each bad name in
  the table's last sentence errs and the message contains `BTab`;
  `["Up", "Bogus"]` errs (nothing partial is returned).
- `paste_brackets_only_when_the_shell_asked` — the five cases of Task 3.
- `paste_strips_an_embedded_end_bracket` — `"a\x1b[201~b\n"` with the mode
  on comes out as one paste of `ab\n` (the inner `ESC [ 201 ~` removed).

Elsewhere:
- `screen_reports_bracketed_paste_and_application_cursor` (`screen.rs`) —
  both off on a fresh screen; on after `\x1b[?2004h\x1b[?1h`; off after the
  matching `l`.
- `screen_text_without_key_modes_defaults_them_off` (`proto.rs`) — a
  `ScreenText` JSON line with no mode keys parses with both `false`.
- `backend_screen_text_reports_the_key_modes` (`backend.rs`) — after the
  backend is fed `\x1b[?2004h`, its `ScreenText` reply has
  `bracketed_paste: true, application_cursor: false`.

## Acceptance criteria

- [ ] Each of the 11 new tests and the renamed
      `send_input_enter_sends_a_carriage_return` ⇒ `test result: ok. 1 passed`.
- [ ] `grep -n 'struct ShellModes' -r src` ⇒ one hit, in
      `src/shell/keys.rs`.
- [ ] `cargo test` ⇒ `passed=2367 failed=0` — exactly 2356 at `1f74f814`
      plus the 11 new tests (the rename adds nothing).
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`; clippy (`-D warnings`)
      and `cargo fmt --all --check` exit 0.
- [ ] `PASTE MATCH` appears in the end-to-end Update Log entry.

## End-to-end verification

```sh
A=/tmp/e2e-02d.txt; : > "$A"
for t in keys_encode_the_plain_vocabulary keys_cursor_keys_follow_application_cursor_mode keys_control_and_meta_use_legacy_bytes keys_modified_cursor_and_function_keys_use_xterm_parameters keys_without_a_legacy_byte_go_out_as_csi_u keys_unknown_name_is_an_error_naming_the_vocabulary paste_brackets_only_when_the_shell_asked paste_strips_an_embedded_end_bracket screen_reports_bracketed_paste_and_application_cursor screen_text_without_key_modes_defaults_them_off backend_screen_text_reports_the_key_modes send_input_enter_sends_a_carriage_return; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
echo "shellmodes_defs=$(grep -rln 'struct ShellModes' src)" >> "$A"
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run each line **once**. Do not run module test suites separately, and do
not write throwaway scripts under `/tmp`: the last line is the full suite.
If it reports failures, re-run it at most twice more, then paste the last
capture as it is and name every failing test.

Then the self-check:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-02d-keys.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-02d.txt
diff /tmp/pasted-02d.txt /tmp/e2e-02d.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- The `keys` and `paste` parameters on `send_input`, the panel naming the
  keys, and the prompt/doc tables — phase 02e.
- The foreground guard, the screen tail in the panel, the ghost refusal and
  `allow_tui_input` — phase 02f.
- Moving `attach_keys`'s own application-cursor rewrite: it rewrites the
  user's raw bytes, a different job from naming keys.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
