# Phase 04b: launch-settle — a launch waits for the program to draw; honest refusal wording

**Milestone:** M28 — Interactive shells
**Status:** todo
**Depends on:** phase-04a
**Estimated diff:** ~200 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Rehearsal 1 of the live close (`phase-04-live-close.md`, Update Log) found
three more defects:

- **F1.** `run_terminal_command("claude", interactive: true)` returned a
  screen that showed only `matt@pinky:~$ claude`. The launch's 500 ms settle
  compares two reads, and both reads saw the echoed command line before
  Node had drawn anything, so the screen counted as "settled". The point of
  returning the screen is lost.
- **F3.** The busy-shell refusal, which happens **before anything is
  written**, is wrapped in the generic run-error text: "the command may
  still be running … do not assume it failed … retrying could
  double-apply". That contradicts the refusal.
- **F4.** Plain `top` redraws every 3 s, so a 500 ms settle passes between
  redraws. The `settle_ms` description and the `sre.toml` example both name
  `top` as a screen that never settles, which is wrong.

## Spec

**Files:** `src/daemon/executor/shell_exec.rs`,
`src/daemon/executor/knowledge/shell.rs` (and its `mod.rs` re-export),
`src/daemon/executor/mod.rs` (`shell_run_error_text` and its tests),
`src/ai/tools/defs.rs` (the `settle_ms` description), and
`assets/prompts/sre.toml`.

1. **Wait for the first change (F1).** Add a pure
   `wait_screen_change(read, baseline: &str, timeout, poll) -> bool` next
   to `wait_screen` in `knowledge/shell.rs`. It returns true as soon as a
   read's text differs from `baseline`, and false when `timeout` passes
   first. `read` is injected the same way as for `wait_screen`. Also expose
   `pub(crate) async fn screen_text(shell, timeout) -> Result<String, String>`,
   the rendered, unannotated visible text, built on the existing private
   `read_screen`.
2. **The launch uses it.** In `launch_interactive` (`shell_exec.rs`):
   1. send `cmd` + `\r` (unchanged);
   2. sleep 100 ms;
   3. take `baseline = screen_text(…)`, which already shows the echoed
      command;
   4. `wait_screen_change(…, baseline, 3 s, 100 ms)`;
   5. then the existing settled `read_shell_screen_in` call (500 ms, 5 s),
      unchanged.

   If no change came within 3 s, the result gets one more line after the
   header:
   `[the program has not drawn anything yet; read it again with read_shell_screen]`.
   If `screen_text` fails, skip the change wait and go straight to the
   settled read.
3. **Honest refusal wording (F3).** Add an error marker type
   `RefusedBeforeWrite`, following the pattern of `crate::shell::ShellExited`
   / `PromptGivenUp` (a unit struct with `Display` and `std::error::Error`),
   in `shell_exec.rs`. The busy-shell refusal in `run_in_shell_core` (the
   one naming "a command written there would be typed into it") carries it
   in its error chain, for example `Err(anyhow::Error::new(RefusedBeforeWrite).context(msg))`,
   so the message text is unchanged. `shell_run_error_text` gains a branch,
   checked before the generic one:
   `Error: <chain>. Nothing was written to that shell, so it is safe to act on the message above.`
   That branch has no "may still be running" or "double-apply" wording.
4. **`top` is not the example (F4).** In the `settle_ms` description
   (`defs.rs`) and in `sre.toml`, replace plain `top` with a screen that
   really redraws faster than the settle window: `top -d 0.2`, or
   `watch -n 0.1 date`. Add that plain `top` refreshes every 3 s, so a
   500 ms settle can pass between its redraws.
   `tests/doc_truth.rs` must pass unmodified.
5. **Capture the end-to-end evidence.** Run § End-to-end verification
   verbatim. Paste `/tmp/e2e-04b.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)`, with the `PASTE MATCH`
   line after the fence.

**Gotchas.**
- The slow-draw test must not use a command whose **echo** contains the
  string it asserts on, or it passes on the echo alone. Use
  `sleep 1; printf 'DR%sN\n' AW`: the screen shows `DRAWN` only once
  `printf` has run.
- Test recipe for the shell tests, as in 03d: `test_home()`,
  `ShellRegistry::default()`, then
  `run_in_shell_for_test(&reg, owner.clone(), "true", 5, None, None, spawn_in_process_host)`
  to get the shell, then `launch_interactive(&reg, &id, &socket_path(id.as_str()), cmd)`.
  Do not read files a line at a time.

## Tests

- `wait_screen_change_returns_on_the_first_difference`
  (`knowledge/shell.rs`): the reader yields `"$ claude"`, `"$ claude"`, then
  `"UI"` from then on ⇒ true. A reader that always yields `"$ claude"` ⇒
  false within the timeout (use ~10 ms polls and a ~200 ms timeout).
- `launch_waits_for_a_slow_program_to_draw` (`shell_exec.rs`):
  `launch_interactive` with `sleep 1; printf 'DR%sN\n' AW` ⇒ the text
  contains `DRAWN`. **Mutation check:** remove the Task 2 change wait,
  confirm this test fails, restore it, and paste the red output in the
  Update Log.
- `a_busy_refusal_says_nothing_was_written` (`executor/mod.rs` tests, next
  to `a_run_error_never_doubles_its_full_stop`): `shell_run_error_text` on
  an error that carries `RefusedBeforeWrite` contains `Nothing was written`
  and does **not** contain `double-apply`.

## Acceptance criteria

- [ ] The 3 new tests ⇒ `test result: ok. 1 passed` each, and the mutation
      check is in the Update Log.
- [ ] The `settle_ms` description and `sre.toml` give a fast-redrawing
      example (`top -d 0.2` or `watch -n 0.1`) for a screen that never
      settles, and neither claims that plain `top` never settles.
- [ ] `cargo test` ⇒ `passed=2403 failed=0` (2400 at `21b99f0a` + 3). An
      extra test is fine: name it in the Update Log and report the count you
      got. **Do not re-run the suite to chase a mismatch.**
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`) and
      `cargo fmt --all --check` exit 0. No new `#[allow]`.
- [ ] The end-to-end Update Log entry holds the fence **and** the
      `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-04b.txt; : > "$A"
for t in wait_screen_change_returns_on_the_first_difference launch_waits_for_a_slow_program_to_draw a_busy_refusal_says_nothing_was_written a_run_error_never_doubles_its_full_stop launch_interactive_returns_the_settled_screen; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run each line **once**. Then run the self-check, and put its line in the
entry after the fence:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-04b-launch-settle.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-04b.txt
diff /tmp/pasted-04b.txt /tmp/e2e-04b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- Other pre-write refusals (a named shell on another host, an unknown
  shell id). Only the busy-shell refusal was seen live.
- D6's approval panel line and 03e's `wait: false` prefix: decisions for
  the PE run.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
