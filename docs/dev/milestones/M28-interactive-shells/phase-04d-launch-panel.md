# Phase 04d: launch-panel — the approval panel says a launch keeps the shell

**Milestone:** M28 — Interactive shells
**Status:** in-progress
**Depends on:** phase-04c
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

The PE run of the live close (`phase-04-live-close.md`, Update Log)
decided two things:

- **D6's panel line, fix before 2.1.0.** The approval panel for
  `run_terminal_command("claude", interactive: true)` shows only
  `$ claude`. Nothing tells the user that the command will keep the shell
  and that nothing waits for it. The same is true of an auto-upgraded
  `ssh zoidberg`.
- **F10 (nit).** The busy-shell refusal reads `Error: Error: shell s1 …`:
  the `RefusedBeforeWrite` branch of `shell_run_error_text` prefixes
  `Error: ` to a chain whose own message already starts with `Error: `.

The constraint on the panel: **the command string is not changed.** It is
what the approval classification, the ghost `is_safe` check and the user's
session approval all read. The launch fact travels as its own field.

## Spec

**Files:** `src/ipc.rs`, `src/daemon/executor/mod.rs`,
`src/daemon/executor/foreground.rs` (one literal), `src/cli/commands/stream.rs`,
plus the `Response::ToolCallPrompt` literals the compiler names in
`src/cli/commands/slash.rs` and `src/ipc_tests.rs` (5).

1. **The frame carries the fact.** `Response::ToolCallPrompt` gains
   `#[serde(default)] interactive: bool`, so an older daemon's frame still
   parses with `false`. `ApprovalRequest` gains `interactive: bool`, and
   `prompt_and_await_approval` copies it into the frame. Every other
   `ApprovalRequest` literal (`send_input`, `foreground.rs`, the named-host
   arm) passes `false` except the two `run_terminal_command` arms in Task 2.
2. **What counts as a launch.** Add a pure
   `fn is_launch(interactive: bool, cmd: &str) -> bool` in
   `executor/mod.rs`: true when `interactive` is set **or**
   `crate::daemon::utils::is_interactive_command(cmd)` recognises the
   command, so an auto-upgraded `ssh host` is labelled too. Both
   `run_terminal_command` approval sites (local and named host) pass
   `is_launch(*interactive, cmd)`.
3. **The panel line.** `tool_call_panel_body` (`stream.rs`) takes
   `interactive: bool` and, when true, pushes
   `→ interactive: the command keeps the shell; nothing waits for it`
   right after the `$ <cmd>` line, before any classification line.
   `prompt_tool_call_ratatui` takes and forwards it; the client's
   `ToolCallPrompt` arm destructures the new field. Every existing call of
   `tool_call_panel_body` passes `false`; the compiler names them, and their
   assertions do not change.
4. **F10, one prefix.** In the `RefusedBeforeWrite` branch of
   `shell_run_error_text`, strip one leading `Error: ` from the chain
   before formatting, so the result starts with exactly one `Error: `. A
   chain that does not start with it (the cap refusal: `owner '…' already
   holds …`) is unchanged.
5. **Capture the end-to-end evidence.** Run § End-to-end verification
   verbatim. Paste `/tmp/e2e-04d.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)`, with the `PASTE MATCH`
   line after the fence.

**Gotchas.**
- Do not touch `prompt_classification`, `ipc::classify` or the ghost
  `is_safe` path. The panel line is a display fact only.
- `tool_call_panel_label` ("terminal · visible to you") is unchanged: the
  launch still runs in a shell the user can see.
- Find anchors with `grep -n`; read windows, not lines.

## Tests

- `tool_call_prompt_without_interactive_parses_as_false` (`ipc_tests.rs`):
  a `ToolCallPrompt` JSON line with no `interactive` key deserialises with
  `interactive == false`; one with `"interactive":true` gives `true`.
- `a_launch_is_the_flag_or_a_recognised_interactive_command`
  (`executor/mod.rs` tests): `is_launch(true, "vim")` and
  `is_launch(false, "ssh host")` are true; `is_launch(false, "ls")` is
  false.
- `the_launch_panel_says_the_command_keeps_the_shell` (`stream.rs` tests,
  next to `the_local_command_panel_is_unchanged`): with `interactive: true`
  the body is `$ claude` then the `→ interactive: …` line; with `false` the
  body has no `interactive` line.
- `a_refusal_starts_with_exactly_one_error_prefix` (`executor/mod.rs`
  tests): `shell_run_error_text` on a `RefusedBeforeWrite` error whose
  context starts `Error: shell s1 is running claude (TUI)…` starts with
  `Error: shell s1` and does not contain `Error: Error:`; on one whose
  context is `owner 'chat:x' already holds 5 shells (cap 5)` it starts with
  `Error: owner`.

## Acceptance criteria

- [ ] The 4 new tests ⇒ `test result: ok. 1 passed` each.
- [ ] `cargo test` ⇒ `passed=2410 failed=0` (2406 at `9713ec5b` + 4). An
      extra test is fine: name it and report the count you got. **Do not
      re-run the suite to chase a mismatch.** A failure in `sbx.rs` is a
      known pre-M28 flake: run that one test alone once and report both runs.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`) and
      `cargo fmt --all --check` exit 0. No new `#[allow]`.
- [ ] The end-to-end Update Log entry holds the fence **and** the
      `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-04d.txt; : > "$A"
for t in tool_call_prompt_without_interactive_parses_as_false a_launch_is_the_flag_or_a_recognised_interactive_command the_launch_panel_says_the_command_keeps_the_shell a_refusal_starts_with_exactly_one_error_prefix the_local_command_panel_is_unchanged a_busy_refusal_says_nothing_was_written; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run each line **once**. Then run the self-check, and put its line in the
entry after the fence:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-04d-launch-panel.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-04d.txt
diff /tmp/pasted-04d.txt /tmp/e2e-04d.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- The per-result `wait: false` note (PE decision: leave as is).
- Any change to approval classification or session-approval matching.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-10-19 01:05 (progress)

Started phase-04d (executor). Status flipped to `in-progress` here and in the
milestone README's phase table. Plan: add `interactive` to
`Response::ToolCallPrompt` and `ApprovalRequest`, wire `is_launch` through both
`run_terminal_command` approval sites, render the panel line in
`tool_call_panel_body`, fix the F10 double `Error: ` prefix, add the four
spec'd tests, then capture the end-to-end transcript verbatim.

### Update — 2026-10-19 01:20 (end-to-end verification)

Ran § End-to-end verification once, top to bottom, with each command's output
appended to `/tmp/e2e-04d.txt`. The pasted block below is that file verbatim.

```
tool_call_prompt_without_interactive_parses_as_false: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
a_launch_is_the_flag_or_a_recognised_interactive_command: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
the_launch_panel_says_the_command_keeps_the_shell: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
a_refusal_starts_with_exactly_one_error_prefix: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
the_local_command_panel_is_unchanged: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
a_busy_refusal_says_nothing_was_written: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2410 failed=0
```

PASTE MATCH
