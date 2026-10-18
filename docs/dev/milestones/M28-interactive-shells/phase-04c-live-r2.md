# Phase 04c: live-r2 — rehearsal 2's findings F6, F7, F9

**Milestone:** M28 — Interactive shells
**Status:** todo
**Depends on:** phase-04b
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Rehearsal 2 of the live close (`phase-04-live-close.md`, Update Log)
passed B1–B9 and found three defects:

- **F6 (major).** GLM sends an empty `keys` as the **string** `"[]"`
  alongside a `text` send. `lenient_keys` (`src/ai/tools/args.rs`) treats
  an empty array-string as a one-name list `["[]"]`, so
  `send_input(text: "exit")` failed with `unknown key "[]"`. (The error
  proves the shape: a real empty array gives an empty list.)
- **F7 (minor).** The per-owner shell-cap refusal ("owner '…' already
  holds 5 shells (cap 5)") is raised **before anything is written**, but it
  still gets the generic "may still be running … do not assume it failed …
  double-apply" text. It also names a shell, `s1`, that it never touched.
- **F9 (minor).** A ghost's `run_terminal_command` with `interactive: true`
  is backgrounded silently, because ghosts are background-only (`ghost.rs`
  drops the flag). It should be refused, with the reason.

## Spec

**Files:** `src/ai/tools/args.rs` and `src/ai/tools/dispatch.rs` (tests)
for F6; `src/daemon/executor/shell_exec.rs` and `src/daemon/executor/mod.rs`
for F7; `src/daemon/ghost.rs` for F9.

1. **F6: an array-string is that array.** In `lenient_keys`, a string that
   parses as a JSON array of strings gives exactly that array, **empty
   included**. Only a string that is not a JSON array is taken as one key
   name. An empty or whitespace string gives an empty list (unchanged).
2. **F7: the cap refusal is a pre-write refusal.**
   - In `run_in_shell_core`, the error from resolving a shell for the
     owner carries `RefusedBeforeWrite` in its chain, as the busy refusal
     does. That covers **both** paths, the reuse path (`shell_for_owner`)
     and the `shell: "new"` spawn, because either can raise
     `registry.rs`'s "already holds N shells (cap N)". The rehearsal hit
     it through `shell: "new"`.
   - In `shell_run_error_text`, the `RefusedBeforeWrite` branch must not
     name a shell id it did not use. Drop the `in shell <id>` prefix from
     that branch only, so the text reads `Error: <chain>. Nothing was
     written, so it is safe to act on the message above.` The busy
     refusal's own message already names its shell inside `<chain>`.
   - Every other branch is unchanged.
3. **F9: a ghost refuses `interactive`.** In `ghost.rs`, where an
   `AiEvent::ToolCall` becomes `PendingCall::Background`, a call with
   `interactive: true` instead becomes `PendingCall::InvalidArgs` (the
   variant the `AiEvent::InvalidToolArgs` arm already uses) with
   `name: "run_terminal_command"`, the call's arguments as JSON, and this
   `error`: `interactive: true is not available to a ghost: ghosts run every
   command in the background and wait for it to exit, so a program that never
   exits (a TUI) cannot be launched. Run a command that exits.` Calls
   without `interactive` are unchanged.
4. **Capture the end-to-end evidence.** Run § End-to-end verification
   verbatim. Paste `/tmp/e2e-04c.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)`, with the `PASTE MATCH`
   line after the fence.

**Gotchas.**
- Do not change the busy refusal's message text. Only its wrapper (Task 2)
  changes.
- Test recipe for any shell-backed test, as before: `test_home()`,
  `ShellRegistry::default()`, then `run_in_shell_for_test(…,
  spawn_in_process_host)`. Find anchors with `grep -n`, and read windows,
  not lines.

## Tests

- `send_input_keys_empty_array_string_is_no_keys` (`dispatch.rs`):
  `{"shell":"s3","text":"exit","keys":"[]"}` ⇒ `AiEvent::SendInput` with
  empty `keys`; `keys:"[\"Escape\"]"` still gives `["Escape"]`; `keys:"Down"`
  still gives `["Down"]`.
- `a_cap_refusal_says_nothing_was_written_and_names_no_shell`
  (`executor/mod.rs` tests): `shell_run_error_text("s1", e)` on an error
  carrying `RefusedBeforeWrite` with the context
  `owner 'chat:x' already holds 5 shells (cap 5)` contains `Nothing was
  written` and `cap 5`, and contains neither `s1` nor `double-apply`.
- `a_ghost_interactive_call_is_refused_with_the_reason` (`ghost.rs`
  tests): converting an `AiEvent::ToolCall { interactive: true, .. }` the
  way the ghost loop does yields `PendingCall::InvalidArgs` whose `error`
  contains `not available to a ghost`; with `interactive: false` it yields
  `PendingCall::Background`. If the conversion is inline in the loop, first
  extract it into a small `fn ghost_pending_for_tool_call(…) -> PendingCall`
  and test that.

## Acceptance criteria

- [ ] The 3 new tests ⇒ `test result: ok. 1 passed` each.
- [ ] The existing `a_busy_refusal_says_nothing_was_written` and
      `send_input_keys_accept_an_array_a_json_string_or_one_name` still pass.
      Update an assertion only if Task 2's wording change requires it, and
      say so in the Update Log.
- [ ] `cargo test` ⇒ `passed=2406 failed=0` (2403 at `3bc70027` + 3). An
      extra test is fine: name it and report the count you got. **Do not
      re-run the suite to chase a mismatch.** A failure in `sbx.rs` is a
      known pre-M28 flake: run that one test alone once, and report both
      runs.
- [ ] Clippy (`-D warnings`) and `cargo fmt --all --check` exit 0. No new
      `#[allow]`.
- [ ] The end-to-end Update Log entry holds the fence **and** the
      `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-04c.txt; : > "$A"
for t in send_input_keys_empty_array_string_is_no_keys a_cap_refusal_says_nothing_was_written_and_names_no_shell a_ghost_interactive_call_is_refused_with_the_reason a_busy_refusal_says_nothing_was_written send_input_keys_accept_an_array_a_json_string_or_one_name; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run each line **once**. Then run the self-check, and put its line in the
entry after the fence:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-04c-live-r2.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-04c.txt
diff /tmp/pasted-04c.txt /tmp/e2e-04c.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- F8 (ghosts are background-only, so D9's ghost guard cannot be reached
  live): a design note for the retrospective, not a fix.
- O3 (the masker's `hostname` false positive) and O4 (a success result not
  naming its shell): predate M28.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-10-18 15:43 (progress)

Executor run for phase-04c (F6, F7, F9). Status flipped to `in-progress`; the
README row follows. All three fixes are in:

- **F6** (`src/ai/tools/args.rs`): `lenient_keys` now takes a string that
  parses as a JSON array as exactly that array, empty included; only a string
  that is not a JSON array stays a one-key name. `"[]"` ⇒ no keys,
  `"[\"Escape\"]"` ⇒ `["Escape"]`, `"Down"` ⇒ `["Down"]` unchanged.
- **F7** (`src/daemon/executor/shell_exec.rs`, `mod.rs`): a shell-resolution
  failure on either path (`shell_for_owner` reuse **and** the
  `shell: "new"` spawn) is wrapped in `RefusedBeforeWrite` by a new `pre_write`
  helper, so the per-owner cap refusal reaches the model as pre-write; the
  `RefusedBeforeWrite` branch of `shell_run_error_text` drops the
  `Error running command in shell <id>:` prefix and reads
  `Error: <chain>. Nothing was written, so it is safe to act on the message
  above.` No other branch changed. The existing
  `a_busy_refusal_says_nothing_was_written` needed one assertion loosened:
  `Nothing was written to that shell` → `Nothing was written`, which the
  wording change required.
- **F9** (`src/daemon/ghost.rs`): the ghost loop's `AiEvent::ToolCall` arm
  now routes through a new `ghost_pending_for_tool_call`; `interactive: true`
  becomes `PendingCall::InvalidArgs` (`name: "run_terminal_command"`, the call's
  arguments as JSON, the pinned reason), everything else stays
  `PendingCall::Background`.

End-to-end capture next (Task 4).

### Update — 2026-10-18 15:43 (end-to-end verification)

Ran the phase doc's § End-to-end verification block verbatim, once, into
`/tmp/e2e-04c.txt`. All three new tests pass (`1 passed` each), the two
pre-existing tests still pass, `doc_truth` is green, clippy `-D warnings` is
clean, `cargo fmt --all --check` exits 0, and the full suite counts
`passed=2406 failed=0` — the pinned 2403 + 3 new.

```text
send_input_keys_empty_array_string_is_no_keys: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
a_cap_refusal_says_nothing_was_written_and_names_no_shell: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
a_ghost_interactive_call_is_refused_with_the_reason: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
a_busy_refusal_says_nothing_was_written: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
send_input_keys_accept_an_array_a_json_string_or_one_name: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2406 failed=0
```

PASTE MATCH
