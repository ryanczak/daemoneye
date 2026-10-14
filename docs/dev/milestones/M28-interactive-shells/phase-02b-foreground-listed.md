# Phase 02b: foreground-listed — listings show what holds each shell

**Milestone:** M28 — Interactive shells
**Status:** todo
**Depends on:** phase-02a
**Estimated diff:** ~200 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

`list_shells` and the ambient `[SHELLS]` block flag a shell `[interactive]`
only when the daemon itself launched ssh or mosh in it, and never clear it.
Phase 01c gave every shell record an observed `last_foreground`. This phase
shows that instead — `[running claude (TUI)]`, `[paused vim]` — and makes
the `[DEFAULT SHELL]` line skip a shell last seen busy. The model-facing
surfaces stop reading the static flag; phase 02c removes the flag itself.

## Architecture references

- `docs/design/interactive-shells.md` D4 — "prompt assembly reads the
  cache": the block does no I/O and may be one observation stale; decisions
  always probe live (phase 02a).

## Spec

**One code file:** `src/daemon/executor/knowledge/shell.rs`. Plus one
sentence each in `src/ai/tools/defs.rs`, `assets/prompts/sre.toml`,
`CLAUDE.md` and `README.md` (Task 4). No tool is added or removed, so **no
count-pinned test moves**. Do not touch `ShellRecord.interactive`,
`mark_interactive`, `src/shell/`, `src/ipc.rs`, `src/daemon/server/` or
`src/cli/` — phase 02c.

1. **The row carries the foreground** — `ShellRow` drops its `interactive`
   field and gains `foreground: Option<Foreground>`, copied from the
   record's `last_foreground` (the classification only, not the timestamp)
   in `collect_row`. Every `ShellRow` literal in the file's tests changes
   with it.
2. **Rendering** — in `format_shell_rows` and `format_shell_context`, where
   ` [interactive]` was pushed, push ` [<foreground>]` (the `Foreground`
   `Display`) **only** for `Program { .. }` and `Paused { .. }`. `Prompt`,
   `Unknown` and `None` render nothing; an exited shell keeps its existing
   ` [exited]` and nothing else from this field.
3. **The default shell** — `default_shell_for` stops reading `interactive`
   and instead skips a row whose `foreground` is `Program { .. }` or
   `Paused { .. }`. No probe: this runs on every prompt assembly and must
   do no I/O.
4. **Docs** — replace the word `interactive` in the flag list of the
   `list_shells` description (`defs.rs`), its `sre.toml` bullet, its
   `CLAUDE.md` table row and its `README.md` table row, with the observed
   foreground: e.g. "flags (`dedicated` / `exited`) and what holds the
   terminal when last observed (`running <cmd>`, `running <cmd> (TUI)`,
   `paused <cmd>`)". `tests/doc_truth.rs` must pass unmodified.
5. **Capture the end-to-end evidence** — run the block in § End-to-end
   verification verbatim and paste `/tmp/e2e-02b.txt` into a new Update Log
   entry headed `### Update — <date> (end-to-end verification)`, ending with
   the `PASTE MATCH` line. The server-authored `(complete)` entry does not
   satisfy this.

**Gotchas.**
- These three existing tests pin the old `[interactive]` rendering and are
  **the only tests whose assertions change**: rewrite their interactive case
  as a foreground case, keep everything else they assert.
  `format_shell_context_lists_every_shell_with_its_flags`,
  `format_shell_context_keeps_each_flag_on_its_own_row`,
  `formatter_marks_flags_and_exits_without_the_registry`. That list is
  complete.
- `Foreground` is `crate::shell::Foreground`; it is `Clone`.

## Tests

- `format_shell_rows_shows_the_observed_foreground` — a row with
  `Program { cmd: "claude", tui: true }` renders ` [running claude (TUI)]`;
  one with `Paused { cmd: "vim" }` renders ` [paused vim]`; rows with
  `Prompt`, `Unknown` and `None` render no bracketed foreground.
- `format_shell_context_shows_the_observed_foreground` — the same three
  cases in the `[SHELLS]` block.
- `default_shell_skips_a_shell_last_seen_busy` — the lowest-numbered row
  is `Program`, the next is `Prompt` ⇒ `[DEFAULT SHELL]` names the second;
  the lowest is `None` ⇒ it names the lowest (unobserved is not busy).
- The three rewritten tests in § Gotchas.

## Acceptance criteria

- [ ] The three new tests and the three rewritten ones pass (`cargo test
      --lib <name>` ⇒ `test result: ok. 1 passed`).
- [ ] `cargo test` ⇒ `passed=2360 failed=0` — exactly 2357 at `f083a907`
      plus the three new tests.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`, with the test file
      unmodified.
- [ ] Clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] `PASTE MATCH` appears in the end-to-end Update Log entry.

## End-to-end verification

```sh
A=/tmp/e2e-02b.txt; : > "$A"
for t in format_shell_rows_shows_the_observed_foreground format_shell_context_shows_the_observed_foreground default_shell_skips_a_shell_last_seen_busy format_shell_context_lists_every_shell_with_its_flags format_shell_context_keeps_each_flag_on_its_own_row formatter_marks_flags_and_exits_without_the_registry; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run each line **once**. Do not run module test suites separately to check
for regressions: the last line is the full suite. If it reports failures,
re-run it at most twice more, then paste the last capture as it is and name
every failing test.

Then the self-check, whose output line goes into the entry:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-02b-foreground-listed.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-02b.txt
diff /tmp/pasted-02b.txt /tmp/e2e-02b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

- [x] May touch `CLAUDE.md` and `README.md` (the `list_shells` row only).

## Out of scope

- Removing `ShellRecord.interactive`, `mark_interactive`, the reuse
  predicates that read it, `ShellInfo.interactive` and the `/shells`
  column — phase 02c.
- Refreshing `last_foreground` from `list_shells` with a live probe: the
  listing stays I/O-free in this phase.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
