# Phase 03a: screen-wait — `read_shell_screen` can wait for a pattern or a quiet screen

**Milestone:** M28 — Interactive shells
**Status:** in-progress (bounced: bugs/bug-03a-1.md)
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
- [ ] **(bug-03a-1)** `wait_screen_with_both_waits_for_the_pattern_then_settles`
      ⇒ `test result: ok. 1 passed`: with `wait_for` and a settle, the
      outcome is `Settled`, never `Matched`.
- [ ] `cargo test` ⇒ `passed=2385 failed=0`: exactly 2384 at `b123b0a5`
      plus the one bug test. **Not** 2384: a count that did not rise means
      the bug test was not added.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`, with the test file
      unmodified. Clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] The end-to-end Update Log entry contains the output fence **and** the
      `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-03a.txt; : > "$A"
for t in wait_screen_returns_when_the_pattern_appears wait_screen_with_both_waits_for_the_pattern_then_settles wait_screen_settles_when_two_reads_agree wait_screen_that_never_settles_is_a_result_not_an_error wait_screen_never_matching_times_out wait_bounds_clamp_the_timeout_and_the_settle read_shell_screen_invalid_wait_for_is_an_error read_shell_screen_returns_the_screen_with_a_foreground_header; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
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

### Notes for executor — 2026-10-15 (bounce, bug-03a-1)

**Green gates and a clean tree are expected here and are NOT evidence the
phase is done.** Everything else in this phase is approved: the parameters,
`wait_bounds`, the notes, keepalive, docs and the six tests. Do not redo
them.

**Exactly one fix remains**, in `wait_screen`: when `settle` is `Some`, a
`wait_for` match must not return `Matched`. It must move on to the settle
phase under the same deadline, and the result is `Settled` or
`NotSettled`. With `settle` `None`, a match still returns `Matched`. Add
`wait_screen_with_both_waits_for_the_pattern_then_settles`, specified in
`bugs/bug-03a-1.md`. Mutation check: restore the early return, confirm the
new test fails, then revert, and put that red output in the Update Log.
Finish: `passed=2385 failed=0`. Re-run § End-to-end verification with the
new test name added to its `for` list, and paste the result into a new
entry together with the `PASTE MATCH` line.

### Update — 2026-10-14 20:58 (progress)

Started phase 03a (executor dispatch). Read the design D7 wording, the existing
`read_shell_screen` chain (args → `AiEvent` → `PendingCall` → `to_tool_call` →
executor arm → `knowledge::read_shell_screen`) and the `watch_shell` keepalive
wrap it must copy. Baseline full-suite count confirmed: `passed=2378 failed=0`
at 63ff8177, matching the criterion's pin. Now implementing the three
parameters, `wait_bounds`, `wait_screen`, the note lines, keepalive and docs.

### Update — 2026-10-14 22:20 (progress)

All seven spec tasks are implemented and green: the three parameters ride
args → `AiEvent` → `PendingCall` → `to_tool_call` → the executor arm (which
now wraps the call in `with_keepalive` like `WatchShell` does) →
`read_shell_screen`; `wait_bounds` clamps 1..=300 s / 50..=5000 ms with a 30 s
default; `wait_screen` is the injected-reader async core with the four-way
`WaitOutcome`; the note lines match the design's four wordings; the invalid
`wait_for` regex errors before any read with the `pattern` wording; the
`sre.toml` / `CLAUDE.md` / `README.md` rows name all three parameters.
`cargo test` ⇒ `passed=2384 failed=0` (2378 + 6 new). Clippy `-D warnings`
and `cargo fmt --all --check` are clean; `tests/doc_truth.rs` passes
unmodified. Capturing the end-to-end evidence next.

### Update — 2026-10-14 22:25 (end-to-end verification)

§ End-to-end verification run verbatim, each line once, against the real
artifacts (the `--lib` test binary and the real prompt/doc assets). Captured
mechanically to `/tmp/e2e-03a.txt`; pasted below unmodified.

```
wait_screen_returns_when_the_pattern_appears: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
wait_screen_settles_when_two_reads_agree: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
wait_screen_that_never_settles_is_a_result_not_an_error: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
wait_screen_never_matching_times_out: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
wait_bounds_clamp_the_timeout_and_the_settle: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
read_shell_screen_invalid_wait_for_is_an_error: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
read_shell_screen_returns_the_screen_with_a_foreground_header: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2384 failed=0
```

PASTE MATCH

### Update — 2026-10-15 05:29 (complete, server-authored)

**Summary:** **Summary:** Phase 03a is implemented and committed (`b123b0a5`, working tree clean). `read_shell_screen` gained `wait_for` / `settle_ms` / `timeout_secs`, carried through args → `AiEvent` → `PendingCall` → `to_tool_call` (all three written back into the arguments JSON) → the executor arm → `read_shell_screen`; the arm wraps the call in `with_keepalive` exactly as the `WatchShell` arm does. `wait_bounds` (pure) defaults the timeout to 30 s and clamps 1..=300 s, clamps `settle_ms` to 50..=5000 ms; `wait_screen` is the injected-reader async core in `knowledge/shell.rs` returning `Matched` / `Settled` / `NotMatched` / `NotSettled` / `Failed`, with quiescence measured on rendered **text** only. The result opens with one note line in the design's four wordings; an invalid `wait_for` regex errors with the `pattern` shape before any read. Docs updated in `sre.toml` (one example each), `CLAUDE.md` and `README.md`; `tests/doc_truth.rs` passes unmodified.

**Notes for review:**
- End-to-end verification ran verbatim against the real artifacts; the capture and the literal `PASTE MATCH` line are in the phase doc's `(end-to-end verification)` entry — the self-check prints `PASTE MATCH`.
- Full suite: `passed=2384 failed=0` (2378 at baseline + the 6 new tests, matching the criterion's pin); clippy `-D warnings` clean, `cargo fmt --all --check` exit 0.
- One adaptation: the not-settled note's wording is `use wait_for for a pattern…` (design D7's sentence) rather than the spec's sketch fragment `use wait_for for a pattern that marks the state you need` — same sentence, taken from the architecture doc as authoritative.
- The `Matched` note reads `[waited N s: the screen shows /<pattern>/]` with the regex source interpolated; the two timeout notes follow the spec's wording.
- Two pre-existing `cargo test --lib` processes from earlier sessions were found hung on this host (unrelated to this change — started before this run) and killed so the suite could run; noting it since it touched host state outside the repo.
- Phase doc status left at `in-progress` and no `(complete)` entry written, per the contract — the server authors both.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 9.61s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
st readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test seeded_assets_name_no_retired_tools ... ok
test docs_do_not_carry_retired_index_claims ... ok
test readme_documents_every_chat_slash_command ... ok
test seeded_config_template_documents_every_config_field ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 34 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test cancel_request_roundtrip ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test default_server_unchanged ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.29s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-1317d86dbab8a67a)
kill: (183455): No such process
kill: (-184205): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-58e4c018b6115a52)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-fa77b3165037b91b)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-b44e63b56dae5611)
     Running tests/integration.rs (target/debug/deps/integration-cc4ec88fa7c13534)
     Running tests/isolation.rs (target/debug/deps/isolation-91fd5762b61d94c8)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-62d50a1c46611977)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +1 -1
- `README.md` — +1 -1
- `assets/prompts/sre.toml` — +10 -2
- `docs/dev/milestones/M28-interactive-shells/README.md` — +1 -1
- `docs/dev/milestones/M28-interactive-shells/phase-03a-screen-wait.md` — +46 -1
- `src/ai/tools/args.rs` — +9 -0
- `src/ai/tools/defs.rs` — +35 -1
- `src/ai/types/events.rs` — +6 -1
- `src/ai/types/pending.rs` — +69 -1
- `src/daemon/executor/knowledge/shell.rs` — +395 -30
- `src/daemon/executor/mod.rs` — +16 -7
- `src/daemon/stream.rs` — +6 -0

**Commit:** b123b0a584cfcca77816595a7a5ecc6c5cee11c4

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Update — 2026-10-15 12:35 (progress, bug-03a-1 fix)

Bounce fix for bug-03a-1 (executor re-dispatch). In `wait_screen` the
pattern loop's match arm returned `WaitOutcome::Matched` unconditionally;
with `settle` `Some` it now breaks out of the pattern loop and hands over
to the settle phase under the same deadline, so the combined case yields
`Settled` (or `NotSettled`), never `Matched`. `wait_for` alone still
returns `Matched` (`wait_screen_returns_when_the_pattern_appears`
unmodified and passing). Added
`wait_screen_with_both_waits_for_the_pattern_then_settles` per
`bugs/bug-03a-1.md` (reader yields `loading`, `done 1`, `done 2`, then
`done 3`; asserts `Settled` and that the reader was called past the first
`done` read).

Mutation check: reverting the fix (early return on match) makes the new
test fail —
`panicked at src/daemon/executor/knowledge/shell.rs:6212:9: with both
waits the pattern match hands over to the settle phase, which needs two
equal reads after 'done 1': Matched { elapsed: 11.320816ms }` —
`test result: FAILED. 0 passed; 1 failed`. Re-applied the fix; the test
passes and `wait_screen*` ⇒ `5 passed; 0 failed`.

### Update — 2026-10-15 12:40 (end-to-end verification)

§ End-to-end verification re-run verbatim with the new test name added to
the `for` list, each line once, against the real artifacts. Captured
mechanically to `/tmp/e2e-03a.txt`; pasted below unmodified.

```
wait_screen_returns_when_the_pattern_appears: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
wait_screen_with_both_waits_for_the_pattern_then_settles: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
wait_screen_settles_when_two_reads_agree: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
wait_screen_that_never_settles_is_a_result_not_an_error: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
wait_screen_never_matching_times_out: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
wait_bounds_clamp_the_timeout_and_the_settle: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
read_shell_screen_invalid_wait_for_is_an_error: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
read_shell_screen_returns_the_screen_with_a_foreground_header: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2385 failed=0
```

PASTE MATCH
