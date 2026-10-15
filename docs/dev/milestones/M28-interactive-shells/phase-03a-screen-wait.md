# Phase 03a: screen-wait — `read_shell_screen` can wait for a pattern or a quiet screen

**Milestone:** M28 — Interactive shells
**Status:** todo
**Depends on:** phase-02g
**Estimated diff:** ~350 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

`read_shell_screen` returns the screen as of the call. After typing a prompt
into Claude Code, the model needs to know when the answer has finished, and
today the only way is to call the tool again and again. `watch_shell` cannot
help: it matches the **byte stream**, which a TUI's cursor addressing breaks
up. This phase adds three parameters to `read_shell_screen`:

- `wait_for`, a regex: block until the **rendered** screen shows it;
- `settle_ms`: block until the rendered screen stops changing;
- `timeout_secs`: the bound on both.

A wait that runs out is a result, not an error. `top` never stops
redrawing, and that is a legitimate answer.

## Architecture references

- `docs/design/interactive-shells.md` D7: the parameters; quiescence
  measured on rendered text; the did-not-settle result and its wording.

## Spec

**Files:** `src/ai/tools/args.rs`, `src/ai/types/events.rs`,
`src/ai/types/pending.rs`, `src/daemon/stream.rs`, `src/ai/tools/defs.rs`,
`src/daemon/executor/mod.rs` (the `ReadShellScreen` arm),
`src/daemon/executor/knowledge/shell.rs`, `assets/prompts/sre.toml`, and the
`read_shell_screen` rows of `CLAUDE.md` and `README.md`. Do not touch
`src/shell/`. Any `ReadShellScreen` literal elsewhere is named by the
compiler.

1. **The parameters.** `read_shell_screen` gains `wait_for: Option<String>`,
   `settle_ms: Option<u64>` and `timeout_secs: Option<u64>`, carried through
   args → `AiEvent` → `PendingCall` → `to_tool_call` (which writes all three
   back into the arguments JSON) → the executor arm → `read_shell_screen`.
   `defs.rs` gets three `ParamDef`s. The `settle_ms` description names `top`
   as a screen that never settles and says to use `wait_for` there.
2. **Bounds.** Add a pure `wait_bounds(timeout_secs, settle_ms) -> (Duration, Option<Duration>)`.
   `timeout_secs` defaults to 30 and is clamped to 1..=300. `settle_ms`
   `None` means no settle wait; `Some(n)` is clamped to 50..=5000 ms.
3. **The wait.** Add a pure async core in `knowledge/shell.rs`:
   `wait_screen(read, wait_for: Option<&Regex>, settle: Option<Duration>, timeout, poll) -> WaitOutcome`.
   `read` is an injected `FnMut() -> Future<Output = Result<String, String>>`
   that returns the rendered, unannotated visible text. `WaitOutcome` is
   `Matched { elapsed }`, `Settled { elapsed }`, `NotMatched { timeout }`,
   `NotSettled { timeout }`, or `Failed(String)` (the read's error, passed
   through).
   - With `wait_for`: read every `poll` until the regex matches the text,
     or the timeout passes.
   - With `settle`: read, sleep `settle`, read again. Two consecutive reads
     with equal text means `Settled`. Text only: no attributes, no cursor.
   - With both: first the pattern, then the settle. One `timeout` bounds
     the whole wait.
   - With neither: `wait_screen` is not called. Today's behaviour is
     unchanged.

   The production `read` is the existing `read_screen(sock, shell, false, 0, SCREEN_TIMEOUT)`,
   keeping only `.text`. `poll` is 100 ms.
4. **The result.** After the wait, `read_shell_screen_in` does its normal
   read and formatting. Its output then starts with **one** note line for
   the wait:
   - `[waited 1.2 s: the screen shows /<pattern>/]`
   - `[waited 0.8 s: the screen settled]`
   - `[did not see /<pattern>/ within N s — showing the screen as it is now]`
   - `[did not settle within N s — the screen is still changing; use wait_for for a pattern that marks the state you need]`

   `Failed(msg)` returns `msg` with no screen. An invalid `wait_for` regex
   is an error with the same shape and wording as an invalid `pattern`,
   returned before any read.
5. **Keepalive.** The executor arm wraps the call in
   `crate::daemon::utils::with_keepalive(tx, …)`, the same way the
   `WatchShell` arm does, because a wait can block for minutes.
6. **Docs.** Add the three parameters to the `read_shell_screen` bullet in
   `sre.toml`, with one example each: `settle_ms: 1500` after typing into
   Claude Code, and `wait_for` for `top`. Add one sentence to the
   `read_shell_screen` rows of `CLAUDE.md` and `README.md`.
   `tests/doc_truth.rs` must pass unmodified.
7. **Capture the end-to-end evidence.** Run § End-to-end verification
   verbatim. Paste `/tmp/e2e-03a.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)`. The entry holds the
   output fence and, **on the line after it, the literal `PASTE MATCH` line
   the self-check printed**. Both are required: phase 02g's entry left the
   line out.

**Gotchas.**
- Compare **text**, never the annotated form. A cursor blink or an
  SGR-only redraw must count as settled.
- Do not use `tokio::time::pause` in the `wait_screen` tests. The reader is
  injected, so use real short durations (a `poll` and `settle` of ~10 ms,
  a timeout of ~200 ms) and keep each test under a second.

## Tests

In `knowledge/shell.rs`:
- `wait_screen_returns_when_the_pattern_appears`: the reader yields
  `"loading"`, `"loading"`, then `"done $"` from then on. With
  `wait_for` `done` ⇒ `Matched`.
- `wait_screen_settles_when_two_reads_agree`: the reader yields `"a"`,
  `"b"`, then `"c"` from then on. With a settle ⇒ `Settled`.
- `wait_screen_that_never_settles_is_a_result_not_an_error`: the reader
  yields a new counter value on every call. ⇒ `NotSettled` within the
  timeout, and the note for it contains `did not settle` and `wait_for`.
- `wait_screen_never_matching_times_out`: ⇒ `NotMatched`, whose note names
  the pattern. A reader that returns `Err("gone")` ⇒ `Failed("gone")`.
- `wait_bounds_clamp_the_timeout_and_the_settle`: `None` ⇒ 30 s with no
  settle; 0 and 9999 s ⇒ 1 s and 300 s; settle 1 and 99999 ms ⇒ 50 ms and
  5000 ms.
- `read_shell_screen_invalid_wait_for_is_an_error`: modelled on
  `read_shell_screen_invalid_pattern_is_an_error`.

## Acceptance criteria

- [ ] The 6 new tests ⇒ `test result: ok. 1 passed` each.
- [ ] `cargo test` ⇒ `passed=2384 failed=0`: exactly 2378 at `63ff8177`
      plus the 6 new tests.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`, with the test file
      unmodified. Clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] The end-to-end Update Log entry contains the output fence **and** the
      `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-03a.txt; : > "$A"
for t in wait_screen_returns_when_the_pattern_appears wait_screen_settles_when_two_reads_agree wait_screen_that_never_settles_is_a_result_not_an_error wait_screen_never_matching_times_out wait_bounds_clamp_the_timeout_and_the_settle read_shell_screen_invalid_wait_for_is_an_error read_shell_screen_returns_the_screen_with_a_foreground_header; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
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
D=docs/dev/milestones/M28-interactive-shells/phase-03a-screen-wait.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-03a.txt
diff /tmp/pasted-03a.txt /tmp/e2e-03a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

- [x] May touch `CLAUDE.md` and `README.md` (the `read_shell_screen` rows
      only).

## Out of scope

- `run_terminal_command(interactive: true)` and its settle-then-screen
  result: phase 03c (it will reuse `wait_screen`).
- `[shells] rows` / `cols` and `open_shell(rows, cols)`: phase 03b.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
