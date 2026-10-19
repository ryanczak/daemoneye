# Phase 03: log-fields — `wait` and `shell` on command events; 2.0 wording in daemon.log

**Milestone:** M29 — Post-2.0 fixes
**Status:** todo
**Depends on:** phase-02
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=fix, size=s

## Goal

The audit log and daemon.log still use 1.x terms. A `command` event says
`mode: foreground` or `mode: background` for an approval decision and
`mode: shell` for a run, and its `pane` field holds a shell id, a host
name or nothing. daemon.log says `foreground command approved: <cmd>`.

In 2.0 the distinction is **waited vs `wait: false`**, and commands run in
**shells**. PE decision: add the 2.0 fields **alongside** the old ones, so
existing readers of `mode`/`pane` keep working, and reword daemon.log.

## Spec

**Files:** `src/daemon/utils/event_log.rs`, `src/daemon/executor/mod.rs`,
`README.md` (§ "Command audit log").

1. **`log_shell_command` takes `wait: bool`** and writes two new keys:
   `"wait": <wait>` and `"shell": <shell_id>`. `mode` (`"shell"`) and
   `pane` (the shell id) are unchanged. Update every caller. The compiler
   names them; all are in `executor/mod.rs`:
   - the waited `run_terminal_command` arms (local and named host),
     including their `host-error` refusals, pass `true`;
   - the chat **`wait: false`** start (`[Background Task Completed]`) and
     its `shell-error`, pass `false`.
2. **`log_command` writes `wait` derived from `mode`:** `true` for
   `"foreground"`, `false` for `"background"`, `null` for any other value.
   It also writes `"shell": null`, because its `pane` is empty or a host
   name, never a shell id. Its signature and `mode`/`pane` are unchanged.
3. **daemon.log in 2.0 terms.** Add a pure
   `fn approval_log_line(background: bool, decision: &str, cmd: &str)
   -> String` in `executor/mod.rs`. It returns
   `command <decision> (waited): <cmd>`, or
   `command <decision> (wait: false): <cmd>` when `background` is true.
   Use it in `prompt_and_await_approval` for both the approved line and
   the denied/timeout line, replacing the two `log::info!` formats that
   lead with `mode`. The `command_approval` events and the stats counters
   are unchanged.
4. **README.** In § "Command audit log", add `"wait"` and `"shell"` to both
   example lines, keeping the keys alphabetical, as `serde_json` writes
   them. Replace the paragraph after the examples so it says:
   - `wait` is `true` for a waited command and `false` for `wait: false`;
   - `shell` is the PTY shell id for a run and `null` for an approval
     decision;
   - `mode` and `pane` are kept for existing readers, and new readers
     should use `wait` and `shell`.
5. **Capture the end-to-end evidence.** Run `cargo fmt --all` first. Then
   run § End-to-end verification verbatim, **once**. Paste the **output**
   (`/tmp/e2e-m29-03.txt`) into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)` as the **first**
   fenced block in that entry, then the `PASTE MATCH` line. Do not paste
   the command block.

**Gotchas.**
- Event keys serialize alphabetically (see CLAUDE.md § Important
  Invariants), so never assert on key position.
- `foreground.rs` passes `"background"` to `log_command` on the ghost
  paths. Ghost runs are background-only, so `wait: false` is correct
  there. Do not change those calls.
- Find anchors with `grep -n`; read windows, not single lines.

## Tests

All in `event_log.rs` tests, using the fixture that
`log_shell_command_writes_the_exit_code` already uses, except the last one.

- `log_shell_command_records_wait_and_shell`: a call with `wait: false`
  and shell `s3` writes a record with `"wait": false`, `"shell": "s3"`,
  `"mode": "shell"` and `"pane": "s3"`.
- `log_command_derives_wait_from_mode`: `"foreground"` gives
  `"wait": true`, `"background"` gives `false`, and `"other"` gives `null`.
  Each record has `"shell": null` and keeps its `mode` and `pane`.
- `the_approval_log_line_speaks_2_0` (`executor/mod.rs` tests, pure):
  - `approval_log_line(false, "approved", "ls")` equals
    `command approved (waited): ls`;
  - `approval_log_line(true, "denied", "rm x")` equals
    `command denied (wait: false): rm x`;
  - neither contains `foreground` or `background`.

## Acceptance criteria

- [ ] The three new tests ⇒ `test result: ok. 1 passed` each.
- [ ] `grep -c 'command approved: {}' src/daemon/executor/mod.rs` ⇒ `0`
      (end-to-end line `old_log_lines=0`).
- [ ] README § "Command audit log" mentions `"wait"` and `"shell"`
      (end-to-end line `readme_wait=` is at least 2).
- [ ] `cargo test` ⇒ about `passed=2418 failed=0` (2415 at `e5bb5291`
      + 3). An extra test is fine: name it and report; do not re-run the
      suite to chase a number.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`)
      and `cargo fmt --all --check` exit 0. No new `#[allow]`.
- [ ] The end-to-end Update Log entry holds the output fence **first**,
      then the `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-m29-03.txt; : > "$A"
for t in log_shell_command_records_wait_and_shell log_command_derives_wait_from_mode the_approval_log_line_speaks_2_0 log_shell_command_writes_the_exit_code; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
echo "old_log_lines=$(grep -c 'command approved: {}' src/daemon/executor/mod.rs)" >> "$A"
echo "readme_wait=$(awk '/^## Command audit log/{f=1} /^---/{f=0} f' README.md | grep -c '"wait"')" >> "$A"
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Then run the self-check, and put its line in the entry after the output
fence:

```sh
D=docs/dev/milestones/M29-post-2.0-fixes/phase-03-log-fields.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m29-03.txt
diff /tmp/pasted-m29-03.txt /tmp/e2e-m29-03.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- Removing or renaming `mode` / `pane`, which the PE decided to keep.
- The `command_approval` event, the stats counters (`commands_fg_*` /
  `commands_bg_*`) and `foreground.rs`.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
