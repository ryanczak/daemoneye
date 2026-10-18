# Phase 04a: arg-tolerance — the argument shapes GLM actually sends parse

**Milestone:** M28 — Interactive shells
**Status:** todo
**Depends on:** phase-04 rehearsal 1 (findings F2, F5)
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

The live-close rehearsal drove the daemon with GLM and lost most of its
blocks to argument parsing. **Read the rehearsal's findings before you
start** (`phase-04-live-close.md`, Update Log, F2 and F5):

- **F2.** `send_input`'s `keys` is declared as a string in the schema, so
  GLM sends `"Down"` or the JSON-encoded string `"[\"Down\",\"Enter\"]"`.
  Neither deserializes into `Vec<String>`, so `keys` could never be used.
- **F5.** GLM adds `"background":"false"` (a **string**) to almost every
  `run_terminal_command`. That fails `Option<bool>`. The invalid-arguments
  error names only declared parameters, never `background`, so the model
  cannot see what to fix and retries 6–9 times.

This phase makes those shapes parse, and makes the error name the field
when a value really is wrong.

## Spec

**Files:** `src/ai/tools/args.rs`, `src/ai/tools/dispatch.rs` (the
invalid-arguments builder and tests), `src/ai/tools/defs.rs` (the `keys`
description only).

1. **Lenient booleans.** Add a serde helper in `args.rs`, used through
   `deserialize_with`, that reads an `Option<bool>` from a JSON boolean, from
   the string `"true"` or `"false"` (case-insensitive, surrounding
   whitespace ignored), or from null/absent (`None`). Any other string is an
   error. Apply it to `RunTerminalCommandArgs`'s `wait`, `background` and
   `interactive`, and to `SendInputArgs`'s `paste` and `enter`. Keep
   `#[serde(default)]` wherever a field already has it.
2. **Lenient `keys`.** `SendInputArgs.keys` accepts, in this order:
   - a JSON array of strings;
   - a string that parses as a JSON array of strings (`extract_string_vec`
     already does both of these);
   - any other non-empty string, taken as **one** key name.

   An empty string or an absent field gives an empty list. Do **not**
   split a plain string on commas or spaces: `C-,` and `M- ` are valid key
   names. A wrong name such as `"Down,Enter"` reaches `encode_keys`, whose
   error lists the vocabulary, which is the honest answer.
3. **The error names every offending field.** In `invalid_args_event`
   (`dispatch.rs`), after the declared-parameter checks, add one problem
   for each **undeclared** key in the arguments object:
   `unexpected parameter '<name>' (<json type>): not in this tool's schema; remove it`.
   Keep every existing message as it is. With Tasks 1–2 in place,
   `background:"false"` no longer reaches this path. A garbage `background`
   (prose) still does, and is now named.
4. **The `keys` description** (`defs.rs`) gains one sentence: pass a JSON
   array such as `["Down", "Enter"]`; a single key name such as `"Escape"`
   also works.
5. **Capture the end-to-end evidence.** Run § End-to-end verification
   verbatim. Paste `/tmp/e2e-04a.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)`, with the `PASTE MATCH`
   line after the fence.

**Gotchas.**
- These shapes come from `daemon.log` in the rehearsal, not from a guess:
  `{"enter":false,"keys":"Down","shell":"s1"}`,
  `{"keys":"[\"Down\",\"Enter\"]","shell":"s1"}`,
  `{"background":"false","command":"echo hi"}`. Use them verbatim in the
  tests.
- `background: "false"` means "not in the background": `to_event` maps it
  to `wait: Some(true)`, as a real `false` does today.

## Tests

All in `src/ai/tools/dispatch.rs` tests, through `dispatch_tool_event`.
Assert on the resulting `AiEvent`:
- `send_input_keys_accept_an_array_a_json_string_or_one_name`: the two
  logged shapes above plus a real `["Down","Enter"]` give
  `["Down"]`, `["Down","Enter"]` and `["Down","Enter"]`; `"keys":""` gives
  an empty list.
- `string_booleans_parse_where_glm_sends_them`: the logged
  `background:"false"` command dispatches to `ToolCall` with
  `wait: Some(true)`; `interactive:"true"` gives `interactive: true`;
  `send_input` with `enter:"false"` and `paste:"TRUE"` gives `enter: false`
  and `paste: Some(true)`.
- `a_garbage_value_is_still_invalid_and_named`: `run_terminal_command` with
  `{"command":"x","background":"then hostname with host zoidberg"}` gives
  `InvalidToolArgs`, and its `error` contains `background`.

## Acceptance criteria

- [ ] The 3 new tests ⇒ `test result: ok. 1 passed` each.
- [ ] `cargo test` ⇒ `passed=2398 failed=0` (2395 at `fafa2862` + 3). If you
      add a further test, keep it, name it in your Update Log entry, and
      report the count you got. **Do not re-run the suite to chase a
      mismatch.**
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`) and
      `cargo fmt --all --check` exit 0. No new `#[allow]`.
- [ ] The end-to-end Update Log entry holds the fence **and** the
      `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-04a.txt; : > "$A"
for t in send_input_keys_accept_an_array_a_json_string_or_one_name string_booleans_parse_where_glm_sends_them a_garbage_value_is_still_invalid_and_named invalid_args_event_names_the_mistyped_param send_input_enter_defaults_on_only_without_keys; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run each line **once**. Do not run module test suites separately, and do
not write throwaway scripts under `/tmp`. Then run the self-check, and put
its line in the entry after the fence:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-04a-arg-tolerance.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-04a.txt
diff /tmp/pasted-04a.txt /tmp/e2e-04a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- F1, F3 and F4 (launch settle, refusal wording, the `top` docs): phase 04b.
- A real array type in `ParamTy`. The schema keeps `keys` as a string,
  because all three providers render from it. Parsing is where the slack
  goes.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
