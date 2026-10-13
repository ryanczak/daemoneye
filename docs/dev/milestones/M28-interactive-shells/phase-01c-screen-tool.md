# Phase 01c: screen-tool — `read_shell_screen`

**Milestone:** M28 — Interactive shells
**Status:** todo
**Depends on:** phase-01b
**Estimated diff:** ~400 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

The model's first use of phase 01's host truth: a read-only tool,
`read_shell_screen`, that returns a shell's rendered screen, masked, with a
header naming what holds the terminal. Ships **without** waits
(`wait_for` / `settle_ms` / `timeout_secs` are phase 03).

## Architecture references

- `docs/design/interactive-shells.md` D7 — the tool's contract.

## Spec

Every task names its one file. `read_shell` is the analogue in each; copy
its shape, do not redesign it.

1. **Executor function** — in `src/daemon/executor/knowledge/shell.rs`,
   `pub async fn read_shell_screen(shell, lines: Option<u64>, scrollback:
   Option<u16>, pattern: Option<&str>, annotated: bool) -> String`:
   unknown id ⇒ the same error `read_shell` gives (names the live ids); no
   owner check (`read_shell` has none). Probe `crate::shell::status` and
   `crate::shell::screen` over the shell's socket with the 5 s relay timeout
   its siblings use; a host that answers neither ⇒ an error naming the
   shell, never a blank screen. Classify with
   `crate::shell::foreground::classify_record` and write the record's
   `last_foreground` with the current time. Header: `Shell <id> —
   <Foreground> — <rows>x<cols>, cursor <row>,<col>[, alt screen]`, then the
   body: `scrollback` rows first (not counted by `lines`), then the visible
   rows tailed by `lines`; `pattern` is a regex over the rendered rows,
   invalid ⇒ error as `read_shell`'s `filter`. The whole result passes
   `mask_sensitive`.
2. **Pending call** — `src/ai/types/pending.rs`: the variant, its
   `to_tool_call` / `id` / `tool_name` / `summary` arms, and
   `should_emit_tool_feedback` ⇒ `true`.
3. **Event** — `src/ai/types/events.rs`: the `AiEvent` variant.
4. **Args** — `src/ai/tools/args.rs`: the args struct with serde defaults
   (`annotated` false).
5. **Tool def** — `src/ai/tools/defs.rs`: the `ToolDef`, `deferred_group:
   None`, `Category::Read`, **not** in `APPROVAL_GATED_TOOLS`. The
   description must say: this is the **live viewport**, lossy, as of the
   call; `read_shell` / `read_shell_log` are the record; inverse-video runs
   appear as `[SEL: …]`.
6. **Dispatch** — `src/ai/tools/dispatch.rs`: the arm.
7. **Stream** — `src/daemon/stream.rs`: the `AiEvent` arm.
8. **Execute** — `src/daemon/executor/mod.rs`: the `PendingCall` arm
   calling Task 1.
9. **Prompt asset** — `assets/prompts/sre.toml`: a `read_shell_screen`
   section beside `read_shell`, saying when to use which. Check whether
   `src/config/seeds.rs` embeds the file or a copy; keep both in step.
10. **Docs** — `CLAUDE.md` tool table (core row) and the count line to
    **43 tools: 29 core + 14 deferred**; `README.md` core table and its
    numbers. `tests/doc_truth.rs` must pass unmodified.
11. **Tests** — in `knowledge/shell.rs`, real PTY via the module's test
    registry (copy `watch_shell`'s fixture):
    `read_shell_screen_returns_the_screen_with_a_foreground_header` (idle
    ⇒ header says `shell prompt`, body shows the prompt),
    `read_shell_screen_names_a_running_program` (`sleep 30` ⇒ `running
    sleep`), `read_shell_screen_lines_tails_the_visible_rows`,
    `read_shell_screen_pattern_filters_rows`,
    `read_shell_screen_invalid_pattern_is_an_error`,
    `read_shell_screen_unknown_shell_is_an_error_naming_live_shells`,
    `read_shell_screen_records_last_foreground`,
    `read_shell_screen_masks_a_secret_on_screen` (echo
    `AKIAIOSFODNN7EXAMPLE`; result contains `<AWS_KEY>` and not the key —
    the filter has **no** `sk-ant-` pattern, so use this shape).
12. **Capture the end-to-end evidence** — run the block in § End-to-end
    verification verbatim and paste `/tmp/e2e-01c.txt` into a new Update
    Log entry headed `### Update — <date> (end-to-end verification)`,
    ending with the `PASTE MATCH` line. The server-authored `(complete)`
    entry does not satisfy this.

**Gotchas.**
- To put escapes on screen in a real-PTY test, send `printf` the `\033`
  **text** (`b"printf '\\033[7mX\\033[0m\\n'\n"`); raw ESC bytes are eaten
  by readline.
- An idle interactive bash reads `raw_mode: Some(true)`; `classify`
  already ignores it unless the state is `Running`. Assert on the
  `Foreground`, never on `raw_mode`.
- Tasks 2–8 are one arm each. Do them in order, one write per task; do not
  read ahead of the task you are on.

## Tests

The eight named in Task 11.

## Acceptance criteria

- [ ] Each test in Task 11 passes (`cargo test --lib <name>` ⇒ `test
      result: ok. 1 passed`).
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`; `grep -c '43 tools: 29 core
      + 14 deferred' CLAUDE.md` ⇒ `1`.
- [ ] `cargo test` ⇒ `failed=0`, `passed` greater than **2343** (phase 01b's
      exit count).
- [ ] Clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] `read_shell`'s existing tests pass unmodified (preservation).
- [ ] `PASTE MATCH` appears in the end-to-end Update Log entry.

## End-to-end verification

```sh
A=/tmp/e2e-01c.txt; : > "$A"
for t in read_shell_screen_returns_the_screen_with_a_foreground_header read_shell_screen_names_a_running_program read_shell_screen_lines_tails_the_visible_rows read_shell_screen_pattern_filters_rows read_shell_screen_invalid_pattern_is_an_error read_shell_screen_unknown_shell_is_an_error_naming_live_shells read_shell_screen_records_last_foreground read_shell_screen_masks_a_secret_on_screen; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /" >> "$A"; done
echo "claude_md_count=$(grep -c '43 tools: 29 core + 14 deferred' CLAUDE.md)" >> "$A"
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run the full `cargo test` line **once**. If it reports failures, re-run at
most twice more, then paste the last capture as it is and name every
failing test; a test this phase added is the first suspect.

Then the self-check, whose output line goes into the entry:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-01c-screen-tool.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-01c.txt
diff /tmp/pasted-01c.txt /tmp/e2e-01c.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

- [x] May touch `CLAUDE.md` and `README.md` (the tool tables and counts only).

## Out of scope

- `wait_for`, `settle_ms`, `timeout_secs` — phase 03.
- Any decision on `last_foreground` / `in_flight`; `list_shells` or the
  `[SHELLS]` block showing the foreground; removing the `interactive` flag
  — phase 02.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
