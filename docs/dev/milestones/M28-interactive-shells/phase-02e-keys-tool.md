# Phase 02e: keys-tool — `send_input` takes `keys` and `paste`

**Milestone:** M28 — Interactive shells
**Status:** in-progress
**Depends on:** phase-02d
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Phase 02d built the pure key encoder (`crate::shell::encode_keys`), the
paste rule (`crate::shell::paste_bytes`) and the screen's key modes
(`ShellResponse::ScreenText { bracketed_paste, application_cursor, .. }`).
The model still cannot use any of it: `send_input` takes only `text` and
`enter`. This phase adds `keys` and `paste` to the tool, so the model can
press Escape, arrows, `BTab` and `C-c`, and send a multi-line prompt as a
single paste. No tool is added, so **no count-pinned tool test moves**.

## Architecture references

- `docs/design/interactive-shells.md` D8: `text` is sent first, then `keys`,
  then Enter if `enter`. An unknown key is an error and nothing is sent. The
  result names which keys went out as CSI u.

## Spec

**Files:** `src/ai/tools/args.rs`, `src/ai/types/events.rs`,
`src/ai/types/pending.rs`, `src/daemon/stream.rs`, `src/ai/tools/defs.rs`,
`src/daemon/executor/mod.rs` (the `SendInput` arm),
`src/daemon/executor/knowledge/shell.rs`, `src/ai/tools/dispatch.rs` (one
test), `assets/prompts/sre.toml`, and the `send_input` rows of `CLAUDE.md`
and `README.md`. Do not touch `src/shell/`: the encoder is finished. Do
not change the foreground note (`input_target_note`) or the ghost gate;
those are phase 02f.

1. **The parameters.** `send_input` gains `keys: [String]` (optional,
   default empty) and `paste: bool` (optional, absent ⇒ the 02d default
   rule). `text` becomes optional, default `""`. `enter` stays optional,
   but its default becomes **true only when `keys` is empty**, so
   `send_input(shell, keys: ["Escape"])` does not also press Enter. Resolve
   that default in `SendInputArgs::to_event`. Downstream, `AiEvent`,
   `PendingCall` and `to_tool_call` carry `keys: Vec<String>`,
   `paste: Option<bool>` and `enter: bool`. `defs.rs` gets the two new
   `ParamDef`s, with `text` no longer required. The `keys` description
   lists the vocabulary and the `C-` / `M-` / `S-` prefixes, and the
   `enter` description states the new default.
2. **Composing the bytes.** Add a pure function to `knowledge/shell.rs`:
   `compose_input(text, paste: Option<bool>, keys: &[String], enter, modes: ShellModes) -> anyhow::Result<ComposedInput>`.
   `ComposedInput` holds `bytes`, `bracketed: bool`, `keys: Vec<String>`,
   `csi_u: Vec<String>` and `enter: bool`. The bytes are `paste_bytes(text)`,
   then `encode_keys(keys)`, then `\r` if `enter`. It returns an error when
   `text` is empty, `keys` is empty and `enter` is false
   ("nothing to send"). The encoder's errors pass through unchanged.
3. **Modes, then compose, then approve.** In the executor's `SendInput` arm,
   after the ghost gate and **before** the approval prompt, read the
   shell's modes with `crate::shell::screen(socket, false, 0, 5 s)`. If
   that read fails, or the reply is not `ScreenText`, use
   `ShellModes::default()`. Then call `compose_input`. On `Err`, return
   `Error: <message>` as the tool result **without prompting**, so nothing
   is sent. The approval panel string names the keys (`keys [Up, Enter]`)
   when there are any, and adds `as a bracketed paste` when `bracketed`.
4. **Sending.** `send_input` and `send_input_with` take
   `&ComposedInput` in place of `text: &str, enter: bool`. They send
   `input.bytes` after the existing ownership check. The 7 existing
   `send_input_with` calls in tests build their input with
   `compose_input(text, None, &[], enter, ShellModes::default())`, and
   their assertions do not change. The result sentence keeps its current
   wording and adds `, as a bracketed paste` when `bracketed`, and
   `, keys: <names>` when there are keys. When `csi_u` is non-empty it adds
   one sentence naming those keys, saying they went out as CSI u, which
   some programs ignore, so check the effect with `read_shell_screen`.
5. **Docs.** Add the two parameters to the `send_input` bullet in
   `sre.toml`, with one example each (`keys: ["Escape"]`,
   `keys: ["BTab"]`; a multi-line `text` goes as one paste when the
   program asked for it). Add one sentence to the `send_input` rows of
   `CLAUDE.md` and `README.md`. `tests/doc_truth.rs` must pass unmodified.
6. **Capture the end-to-end evidence.** Run § End-to-end verification
   verbatim. Paste `/tmp/e2e-02e.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)`. The entry holds **only**
   the output fence, followed by the `PASTE MATCH` line.

**Gotchas.**
- `PendingCall::SendInput`'s `to_tool_call` must put `keys` and `paste`
  into the arguments JSON. A replayed or resumed history that drops them
  would re-run a different call from the one the user approved.
- `paste: true` while the shell's bracketed paste mode is off is an error
  from `paste_bytes`. Pass it through. Do not downgrade it to an unbracketed
  send.

## Tests

- `compose_input_orders_text_keys_then_enter` (`knowledge/shell.rs`):
  `"ls"`, `["Up"]`, enter ⇒ `b"ls\x1b[A\r"`. `""`, `["C-c"]`, no enter ⇒
  `b"\x03"`.
- `compose_input_brackets_a_multiline_paste_when_the_shell_asked`:
  `"a\nb"` with bracketed paste on ⇒ the bytes are wrapped and `bracketed`
  is true. With the mode off ⇒ raw bytes, and `bracketed` is false.
- `compose_input_refuses_bad_input_before_anything_is_sent`: an unknown key
  errors and the message contains `BTab`. `paste: Some(true)` with the mode
  off errors and the message names bracketed paste. Empty text, no keys and
  no enter errors with `nothing to send`.
- `send_input_result_names_csi_u_keys_and_the_paste`: `send_input_with`
  with a composed `["S-Enter"]`, and then with a bracketed paste. The
  result names `S-Enter` and `CSI u` in the first case and `bracketed
  paste` in the second. The bytes the injected `send` saw equal
  `input.bytes`.
- `send_input_enter_defaults_on_only_without_keys` (`dispatch.rs` tests,
  through `dispatch_tool_event`): `{shell, text}` ⇒ `enter: true`.
  `{shell, keys: ["Escape"]}` ⇒ `enter: false`, `text: ""`.
  `{shell, keys: ["Up"], enter: true}` ⇒ `enter: true`.

## Acceptance criteria

- [ ] The 5 new tests ⇒ `test result: ok. 1 passed` each.
- [ ] `cargo test` ⇒ `passed=2372 failed=0`, exactly 2367 at `80261992`
      plus the 5 new tests.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`, with the test file
      unmodified. Clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] `PASTE MATCH` appears in the end-to-end Update Log entry.

## End-to-end verification

```sh
A=/tmp/e2e-02e.txt; : > "$A"
for t in compose_input_orders_text_keys_then_enter compose_input_brackets_a_multiline_paste_when_the_shell_asked compose_input_refuses_bad_input_before_anything_is_sent send_input_result_names_csi_u_keys_and_the_paste send_input_enter_defaults_on_only_without_keys send_input_enter_sends_a_carriage_return; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run each line **once**. Do not run module test suites separately, and do
not write throwaway scripts under `/tmp`. The last line runs the full
suite. If it reports failures, re-run it at most twice more, then paste
the last capture as it is and name every failing test.

Then the self-check:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-02e-keys-tool.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-02e.txt
diff /tmp/pasted-02e.txt /tmp/e2e-02e.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

- [x] May touch `CLAUDE.md` and `README.md` (the `send_input` rows only).

## Out of scope

- The foreground guard, the screen tail in the panel, the ghost refusal and
  `allow_tui_input` are phase 02f.
- Changes to the encoder or the paste rule (phase 02d, done).

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-10-14 16:04 (started)

Executor: Claude (M28 phase-02e run). Status flipped to `in-progress`; implementing the Spec in order — the `keys`/`paste` parameters through `SendInputArgs` → `AiEvent` → `PendingCall` → `stream.rs` → `defs.rs`, `compose_input` in `knowledge/shell.rs`, the executor's modes-then-compose-then-approve arm, then the docs and the end-to-end capture.

### Update — 2026-10-14 16:15 (end-to-end verification)

```text
compose_input_orders_text_keys_then_enter: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
compose_input_brackets_a_multiline_paste_when_the_shell_asked: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
compose_input_refuses_bad_input_before_anything_is_sent: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
send_input_result_names_csi_u_keys_and_the_paste: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
send_input_enter_defaults_on_only_without_keys: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
send_input_enter_sends_a_carriage_return: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2372 failed=0
```

PASTE MATCH
