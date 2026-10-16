# Phase 03c: open-shell-size — `open_shell(rows, cols)`

**Milestone:** M28 — Interactive shells
**Status:** todo
**Depends on:** phase-03b
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

Since 03b, every shell spawns at `[shells] rows` × `cols`. A wide program
such as `htop` on a big host, or a two-pane TUI, can want more. This
phase gives the model `open_shell(rows?, cols?)`, so it can open a shell
sized for the program it is about to run. The result names the size the
shell was opened at. Attached mode's resize and the `Resize` frame are
unchanged.

## Architecture references

- `docs/design/interactive-shells.md` D10: `open_shell` gains optional
  `rows` and `cols`, and the result names the size.

## Spec

**Files:** `src/ai/tools/args.rs`, `src/ai/types/events.rs`,
`src/ai/types/pending.rs`, `src/daemon/stream.rs`, `src/ai/tools/defs.rs`,
`src/daemon/executor/mod.rs` (the `OpenShell` arm),
`src/daemon/executor/knowledge/shell.rs`, `src/daemon/server/shells.rs`
(one call), `src/ai/tools/dispatch.rs` (one test), `assets/prompts/sre.toml`,
and the `open_shell` rows of `CLAUDE.md` and `README.md`.

1. **The parameters.** `open_shell` gains `rows` and `cols`, both optional.
   Deserialize them as `Option<u64>`, so an absurd value is clamped rather
   than failing argument parsing. Carry them through args → `AiEvent` →
   `PendingCall` → `to_tool_call` (which writes them back into the
   arguments JSON) → the executor arm. `defs.rs` gets two `ParamDef`s,
   saying the default is the `[shells]` size and giving the clamp range.
2. **Resolving the size.** Add a pure
   `resolve_size(rows: Option<u64>, cols: Option<u64>, default: (u16, u16)) -> (u16, u16)`
   in `knowledge/shell.rs`. A missing value comes from `default`. The
   result is clamped by **the same rule as** `ShellsConfig::size`
   (rows `10..=200`, cols `40..=500`). Do not write a second copy of
   those bounds: route the values through `ShellsConfig::size`, or move
   the clamp into one shared function that both call. The production
   default is `crate::shell::spawn_size()`.
3. **Spawning at that size.** `open_shell(caller, host, rows, cols)` and
   `open_shell_id(caller, host, rows, cols)` resolve the size once and
   spawn at it. The production local spawn is
   `spawn_detached_host(id, &[], rows, cols)`, and the named-host spawn is
   `spawn_ssh_host(id, name, rows, cols)`. Give the two non-test spawn
   helpers a size parameter, or add sized variants. The `#[cfg(test)]`
   in-process host keeps its fixed size and ignores the request. The
   user's `/shell` path (`src/daemon/server/shells.rs`) passes
   `None, None`. Every other `open_shell(…)` / `open_shell_id(…)` call in
   tests gains `None, None`, and the compiler names them.
4. **The result names the size.** `opened_shell_sentence` takes the
   resolved size and adds ` Its terminal is <rows> rows × <cols> columns.`
   after the host clause (when there is one), before ` Note:`.
5. **Docs.** Add the two parameters to the `open_shell` bullet in
   `sre.toml`, with one example (`open_shell(cols: 200)` for a wide
   program). Add one sentence to the `open_shell` rows of `CLAUDE.md` and
   `README.md`. `tests/doc_truth.rs` must pass unmodified.
6. **Capture the end-to-end evidence.** Run § End-to-end verification
   verbatim. Paste `/tmp/e2e-03c.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)`. The entry holds the
   output fence **and**, after it, the `PASTE MATCH` line the self-check
   printed.

**Gotchas.**
- One existing test's assertion changes, and that list is complete:
  `open_shell_local_result_is_unchanged` pins the exact local sentence. It
  is **renamed** `open_shell_local_result_names_the_size`, and its expected
  string gains the size sentence at the 03b default, `40 rows × 120 columns`.
  `lifecycle_home` writes no `[shells]` table, so `spawn_size()` gives
  40×120 there. If it does write one, use the size it sets.
- Do not change the `Resize` frame, attached mode or the CLI `shell-host`
  defaults.

## Tests

- `resolve_size_fills_and_clamps`: `(None, None, (40, 120))` ⇒ `(40, 120)`;
  `(Some(50), None, (40, 120))` ⇒ `(50, 120)`;
  `(Some(0), Some(99999), (40, 120))` ⇒ `(10, 500)`.
- `open_shell_names_the_requested_size`: through `open_shell_test_seam`,
  as the rename's test does. `open_shell(caller, None, Some(50), Some(160))`
  ⇒ the sentence contains `50 rows × 160 columns`.
- `open_shell_args_carry_rows_and_cols` (`dispatch.rs` tests, through
  `dispatch_tool_event`): `{rows: 50, cols: 160}` ⇒ `AiEvent::OpenShell`
  carries `Some(50)` and `Some(160)`; `{}` ⇒ both `None`.
- The rename in § Gotchas.

## Acceptance criteria

- [ ] The 3 new tests and the renamed one ⇒ `test result: ok. 1 passed`
      each.
- [ ] `cargo test` ⇒ `passed=2390 failed=0`: exactly 2387 at `e1ae6c3b`
      plus the 3 new tests. The rename adds nothing.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`, with the test file
      unmodified. Clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] The end-to-end Update Log entry contains the output fence **and** the
      `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-03c.txt; : > "$A"
for t in resolve_size_fills_and_clamps open_shell_names_the_requested_size open_shell_args_carry_rows_and_cols open_shell_local_result_names_the_size; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
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

Then run the self-check, and put the line it prints into the entry after
the fence:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-03c-open-shell-size.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-03c.txt
diff /tmp/pasted-03c.txt /tmp/e2e-03c.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

- [x] May touch `CLAUDE.md` and `README.md` (the `open_shell` rows only).

## Out of scope

- Resizing a live shell from a tool. The only resize is attached mode's.
- `run_terminal_command(interactive: true)` and the ssh auto-upgrade:
  phase 03d.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
