# Phase 01: invalid-args — honest wording, no budget exemption

**Milestone:** M29 — Post-2.0 fixes
**Status:** in-progress
**Depends on:** none
**Estimated diff:** ~80 lines
**Tags:** language=rust, kind=fix, size=s

## Goal

Two defects in how a tool call with bad arguments is handled.

- **The wording.** `invalid_args_event` (`src/ai/tools/dispatch.rs`) writes
  every type mismatch as `parameter '<name> (<type>, optional)' is …`, even
  for a required parameter. The missing-parameter line and
  `expected_params` already get this right.
- **The budget exemption.** `daemon::stream` exempts any call whose
  `tool_name()` is in `APPROVAL_GATED_TOOLS` from the per-turn,
  per-session and per-tool caps. `PendingCall::InvalidArgs::tool_name()`
  returns the tool the model *meant* to call. So a model that keeps
  sending bad arguments to `edit_file` is never capped, even though nothing
  prompts and nothing runs. PE decision: an `InvalidArgs` call is **never**
  exempt.

## Spec

**Files:** `src/ai/tools/dispatch.rs`, `src/daemon/stream.rs`.

1. **Required or optional.** In `invalid_args_event`, the type-mismatch
   line says `required` when `param.required` is true and `optional`
   otherwise, in the same position it holds now.
2. **One exemption predicate.** Add a pure
   `fn is_budget_exempt(call: &PendingCall) -> bool` in `stream.rs`. It is
   true only when the call is **not** `PendingCall::InvalidArgs` **and**
   its `tool_name()` is in `APPROVAL_GATED`. Both places in the tool loop
   that test `APPROVAL_GATED.contains(&tool_name)` use it instead: the cap
   check and the per-session counter bump. Do not change
   `APPROVAL_GATED_TOOLS`, `tool_name()`, or how a non-exempt call is
   counted.
3. **Capture the end-to-end evidence.** Run § End-to-end verification
   verbatim. Paste `/tmp/e2e-m29-01.txt` into a new Update Log entry
   headed `### Update — <date> (end-to-end verification)`, with the
   `PASTE MATCH` line after the fence.

**Gotchas.**
- Some arguments, such as `"false"` for a boolean, parse leniently before
  they reach `invalid_args_event`. Use a value that cannot coerce, such as
  an object or an array, for the mismatch test.
- Find anchors with `grep -n`; read windows, not single lines.

## Tests

- `a_type_mismatch_says_whether_the_parameter_is_required`
  (`dispatch.rs` tests): a `run_terminal_command` call whose `command` is
  an object yields an error containing `'command (string, required)'`. A
  call with a valid `command` and an object for `timeout_secs` yields
  `'timeout_secs (integer, optional)'`. The negative checks are
  **scoped to the parameter**: the first error does not contain
  `'command (string, optional)'`, and the second does not contain
  `'timeout_secs (integer, required)'`. Do **not** assert that the whole
  error lacks the bare word `optional` or `required`. The error ends with
  the full expected-parameter list, which correctly names other parameters
  with both words.
- `an_invalid_args_call_is_never_budget_exempt` (`stream.rs` tests):
  `is_budget_exempt` is false for a `PendingCall::InvalidArgs` named
  `edit_file`, true for a well-formed approval-gated call such as
  `edit_file`, and false for a non-gated call such as `read_file`.

## Acceptance criteria

- [ ] Both new tests ⇒ `test result: ok. 1 passed` each.
- [ ] `cargo test` ⇒ about `passed=2412 failed=0` (2410 at `d3f1a83c`
      + 2). An extra test is fine: name it and report; do not re-run the
      suite to chase a number. A failure in `sbx.rs` is a known flake: run
      that one test alone once and report both runs.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`)
      and `cargo fmt --all --check` exit 0. No new `#[allow]`.
- [ ] `grep -n 'APPROVAL_GATED.contains(&tool_name)' src/daemon/stream.rs`
      prints nothing.
- [ ] The end-to-end Update Log entry holds the fence **and** the
      `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-m29-01.txt; : > "$A"
for t in a_type_mismatch_says_whether_the_parameter_is_required an_invalid_args_call_is_never_budget_exempt; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
echo "gated_name_checks=$(grep -c 'APPROVAL_GATED.contains(&tool_name)' src/daemon/stream.rs)" >> "$A"
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run each line **once**. Then run the self-check, and put its line in the
entry after the fence:

```sh
D=docs/dev/milestones/M29-post-2.0-fixes/phase-01-invalid-args.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m29-01.txt
diff /tmp/pasted-m29-01.txt /tmp/e2e-m29-01.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- The undeclared-parameter line and `expected_params`, which are already
  right.
- Per-tool limits config validation (`LimitsConfig::validate`).

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

### Update — 2026-10-19 02:13 (progress)

Started by Claude (executor run). Read the spec and both target files:
`invalid_args_event`'s type-mismatch line hardcodes `optional`
(`src/ai/tools/dispatch.rs`), and `src/daemon/stream.rs` tests
`APPROVAL_GATED.contains(&tool_name)` directly in the cap check and the
per-session counter bump, so a `PendingCall::InvalidArgs` naming a gated tool
escapes both. Plan: fix the wording, add the pure `is_budget_exempt`
predicate in `stream.rs`, route both call sites through it, add the two named
tests, then capture the E2E transcript.

<!-- entries appended below this line -->

### Notes for executor — 2026-10-19

The first run stalled after 169 calls, the last 40 of them re-reads of
`dispatch.rs`. **The code changes are correct and stay as they are:** the
wording fix in `invalid_args_event`, `is_budget_exempt`, and both
`stream.rs` call sites routed through it.

`a_type_mismatch_says_whether_the_parameter_is_required` fails because of
a spec ambiguity, not a code bug. It asserts `!error.contains("optional")`,
but the error's trailing `expected:` list contains `optional` for other
parameters. Two edits remain:

1. In that test, make the two negative assertions parameter-scoped, as the
   Tests section now says: `!error.contains("'command (string, optional)'")`
   and `!error.contains("'timeout_secs (integer, required)'")`.
2. Add `an_invalid_args_call_is_never_budget_exempt` in `stream.rs`'s
   `mod tests`. A well-formed `PendingCall::EditFile` needs every field
   spelled out (`id`, `thought_signature`, `path`, `operation`, the five
   `Option`s set to `None`); `PendingCall::ReadFile` likewise. The variant
   definitions are in `src/ai/types/pending.rs`.

Then run § End-to-end verification once.


### Update — 2026-10-19 (escalation)

**Chosen lever:** resume.
**Rationale:** the stall came from a test assertion my spec wording
invited, and the code is done and correct. A resume with the corrected
Tests text and the one remaining test keeps the finished work, where a
re-dispatch would redo it.

### Update — 2026-10-19 (end-to-end verification)

Ran § End-to-end verification verbatim, once, redirecting to
`/tmp/e2e-m29-01.txt`. First pass reported `fmt_exit=1` (two formatting
drifts the write tool introduced); ran `cargo fmt --all`, then re-checked:
`cargo fmt --all --check` exits 0. Transcript:

```
a_type_mismatch_says_whether_the_parameter_is_required: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
an_invalid_args_call_is_never_budget_exempt: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
gated_name_checks=0
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=1
passed=2412 failed=0
```

PASTE MATCH
