# Phase 01b: screen-tool — `read_shell_screen`, and what the record knows

**Milestone:** M28 — Interactive shells
**Status:** todo
**Depends on:** phase-01
**Estimated diff:** ~450 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Phase 01 taught the shell host to report its screen and its foreground. This
phase gives the model its first use of that: a read-only tool,
`read_shell_screen`, that returns a shell's rendered screen with a header
naming what holds the terminal. It also gives `ShellRecord` the two fields
phase 02's decisions need — `last_foreground` (observed) and `in_flight`
(known). No existing tool changes behaviour; nothing *decides* on these
fields until phase 02.

## Architecture references

- `docs/design/interactive-shells.md` D3 (incl. the transport rule), D4
  (the two record fields), D7 (the tool; this phase ships it **without**
  `wait_for`, `settle_ms`, `timeout_secs`).

## Spec

1. **Remove the dead arm** — in `src/shell/foreground.rs`, restructure
   `classify` so no match arm is `unreachable!()`. Behaviour unchanged; the
   existing classifier tests are the proof. (Carried from phase 01's review.)
2. **Record fields** — `ShellRecord` (`src/shell/registry.rs`) gains
   `last_foreground: Option<(Foreground, u64)>` (classification + unix
   seconds) and `in_flight: Option<InFlight>`, where `InFlight { cmd:
   String, started_unix: u64 }`. Both start `None`. Keep the existing
   `interactive` flag; phase 02 removes it.
3. **Set `in_flight` around every wrapped command** — set it immediately
   before the wrapped command is written and clear it on **every** way the
   run ends (outcome, timeout, prompt given up, shell exited, error). Use a
   guard whose `Drop` clears it, so an early `?` cannot leave it set. The
   two sites: the core of `run_in_shell_*` in
   `src/daemon/executor/shell_exec.rs`, and the `run_command` call in
   `src/daemon/executor/file_ops/mod.rs`. The interactive-command early
   return in `shell_exec.rs` writes no wrapped command and sets nothing.
4. **One routing function** — `classify_record(record, status, last_line)
   -> Foreground` next to the classifier: a record with a named `host` or a
   `sandbox` goes to `classify_transport` (state from `status`, `in_flight`
   from the record, `last_line` = last non-blank rendered row); any other
   record goes to `classify(status)`. Pure.
5. **The tool** — `read_shell_screen(shell, lines?, scrollback?, pattern?,
   annotated?)`, core (`deferred_group: None`), `Category::Read`, not
   approval-gated, silent-tool feedback. Follow the CLAUDE.md "Adding a new
   AI tool" checklist end to end; `read_shell` is the analogue at every
   step. Behaviour:
   - Unknown id ⇒ the same error shape `read_shell` gives (names the live
     ids). No owner check, as `read_shell` has none.
   - Probes `Status` and `Screen { annotated, scrollback }` over the shell's
     socket (phase 01's `client::status` / `client::screen`, the 5 s relay
     timeout `read_shell`'s siblings use). A host that does not answer
     either ⇒ an error naming the shell, never a blank screen.
   - Header: `Shell <id> — <Foreground> — <rows>x<cols>, cursor
     <row>,<col>[, alt screen]`, then the body. Writes
     `last_foreground` with the current time.
   - `lines` tails the visible rows (scrollback rows prepended first are not
     counted against it); `pattern` is a regex over the rendered rows,
     invalid ⇒ error, as `read_shell`'s `filter`.
   - The whole result passes `mask_sensitive`.
   - Description must say: this is the **live viewport**, lossy; the
     recorded log (`read_shell`) is the record. Inverse-video runs appear
     as `[SEL: …]`.
6. **Docs** — `assets/prompts/sre.toml` gains a `read_shell_screen`
   section beside `read_shell` saying when to use which. `CLAUDE.md` and
   `README.md` per checklist steps 10–11: **43 tools: 29 core + 14
   deferred**. `tests/doc_truth.rs` must pass unmodified.
7. **Capture the end-to-end evidence** — run the block in § End-to-end
   verification verbatim and paste `/tmp/e2e-01b.txt` into a new Update Log
   entry headed `### Update — <date> (end-to-end verification)`, ending with
   the `PASTE MATCH` line. The server-authored `(complete)` entry does not
   satisfy this.

**Gotchas.**
- An idle interactive bash reads `raw_mode: Some(true)`: readline clears
  `ICANON` at the prompt. Do not assert otherwise anywhere; `classify`
  already ignores `raw_mode` unless the state is `Running`.
- To put escapes on screen in a real-PTY test, send `printf` the `\033`
  **text** (`b"printf '\\033[7mX\\033[0m\\n'\n"`). Raw ESC bytes written to
  the shell are eaten by readline.
- `ShellRecord` literals exist in many test modules; every one gains the
  two `None` fields.

## Tests

- `foreground.rs`: `classify_record_routes_transport_records_to_the_transport_rule`
  (a named-host record and a sandbox record whose `Status` says raw +
  `ssh` classify by `in_flight`/last line, a local record with the same
  status classifies `Program { tui: true }`).
- `shell_exec.rs`: `in_flight_is_set_while_a_command_runs_and_cleared_after`
  (observe it set during a `sleep`, `None` after),
  `in_flight_is_cleared_when_the_command_times_out`.
- `knowledge/shell.rs`, real PTY via the module's test registry:
  `read_shell_screen_returns_the_screen_with_a_foreground_header`
  (idle shell ⇒ header says `shell prompt` and the body shows the prompt),
  `read_shell_screen_names_a_running_program` (`sleep 30` ⇒ `running
  sleep`), `read_shell_screen_lines_tails_the_visible_rows`,
  `read_shell_screen_pattern_filters_rows`,
  `read_shell_screen_invalid_pattern_is_an_error`,
  `read_shell_screen_unknown_shell_is_an_error_naming_live_shells`,
  `read_shell_screen_records_last_foreground`,
  `read_shell_screen_masks_a_secret_on_screen` (echo `AKIAIOSFODNN7EXAMPLE`;
  the result must contain `<AWS_KEY>` and not the key. Use this shape: the
  filter in `src/ai/filter.rs` has **no** pattern for `sk-ant-` tokens).

## Acceptance criteria

- [ ] Each test in § Tests passes (`cargo test --lib <name>` ⇒ `test
      result: ok. 1 passed`).
- [ ] `grep -c 'unreachable!' src/shell/foreground.rs` ⇒ `0`.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`; `grep -c '43 tools: 29 core
      + 14 deferred' CLAUDE.md` ⇒ `1`.
- [ ] `cargo test` ⇒ `failed=0`, `passed` greater than **2340** (baseline
      at phase 01's approval, `42d2ec10`).
- [ ] Clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] Phase 01's classifier tests pass unmodified (preservation).
- [ ] `PASTE MATCH` appears in the end-to-end Update Log entry.

## End-to-end verification

```sh
A=/tmp/e2e-01b.txt; : > "$A"
for t in classify_record_routes_transport_records_to_the_transport_rule in_flight_is_set_while_a_command_runs_and_cleared_after in_flight_is_cleared_when_the_command_times_out read_shell_screen_returns_the_screen_with_a_foreground_header read_shell_screen_names_a_running_program read_shell_screen_lines_tails_the_visible_rows read_shell_screen_pattern_filters_rows read_shell_screen_invalid_pattern_is_an_error read_shell_screen_unknown_shell_is_an_error_naming_live_shells read_shell_screen_records_last_foreground read_shell_screen_masks_a_secret_on_screen classify_matches_the_design_matrix; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /" >> "$A"; done
echo "unreachable=$(grep -c 'unreachable!' src/shell/foreground.rs)" >> "$A"
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
D=docs/dev/milestones/M28-interactive-shells/phase-01b-screen-tool.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-01b.txt
diff /tmp/pasted-01b.txt /tmp/e2e-01b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

- [x] May touch `CLAUDE.md` and `README.md` (the tool tables and counts only).

## Out of scope

- `wait_for`, `settle_ms`, `timeout_secs` on the tool — phase 03.
- Any decision on `last_foreground` / `in_flight`: shell reuse, the busy
  refusal, the `[SHELLS]` block, `list_shells` output — phase 02.
- Removing the `interactive` flag — phase 02.
- `in_flight` for background (`wait: false`) runs: their shell is
  dedicated and never reused; phase 02 decides whether transport
  classification needs it there.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
