# Phase 11b — Test tidies

**Status:** done
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-30
**Depends on:** phase-11
**Estimated diff:** ~140 insertions, ~560 deletions
**Tags:** language=rust, kind=refactor, size=s
**Findings:** M31 `FINDINGS.md` #51, #53, #54 (#5 and #6 are rejected
below)

## Goal

The two render test files build the same renderer and the same status
bar by hand. After this phase:

- **#54:** one `test_renderer(cols, rows, viewport, depth)` in
  `src/cli/render_ratatui_tests.rs` builds every test renderer. The 17
  inline `RatatuiRenderer { … }` builds become one-line calls, and
  `make_test_renderer()` calls it too.
- **#53:** `default_status()` and `approval_test_status()` move from
  the layout test file into `render_ratatui_tests.rs`, shared by both
  files. The 15 inline `StatusBarState` literals that copy one of them
  become calls. The one literal with different values stays inline.
- **#51:** the two bare `assert_eq!`s in
  `src/memory/index/incremental_tests.rs` that a failure would leave
  unexplained get messages.

This is a pure tidy (M32 decision 2). The suite stays at **exactly
2609**; a different count means something else changed.

**Rejected:**
- **#5, #6** (the PTY tests' poll loops and bare sleeps). They were
  filed as a test-speed cost, and the cost was measured at drafting. The
  four PTY test groups take 5.1 s, 3.4 s, 0.7 s and 20.0 s. The 20 s
  group (`daemon::server::shells`) is slow because its 25 tests each
  hold the HOME lock and start a real bash, so they run one after
  another, about 0.8 s each. The 10–50 ms poll intervals cost almost
  nothing, and the loops already wait on a condition with a deadline.
  The client test's 3 s sleep is the behaviour under test: a paused
  command must outlast its deadline. Rewriting about 50 timing-sensitive
  waits would buy no speed and would risk new flakes.
- **#51's other nine:** the `Corpus::from_table` table checks, such as
  `assert_eq!(Corpus::from_table("turns_map"), None)`. The expression
  names the case, and `assert_eq!` prints both sides on failure.

## Architecture references

- `src/cli/render_ratatui.rs` § `RatatuiRenderer`: the struct the
  builder fills.
- M32 README, decision 2.

## Spec

1. **Run this, once, as your first command after the started entry:**

   ```sh
   python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-11b-test-tidies.py && cargo fmt --all
   ```

   **Do not open the script, and do not read any source file before
   running it.** It was run on a scratch worktree of `4bbf45ba`: fmt and
   clippy clean, hygiene passing, the full suite at
   `passed=2609 failed=0`. It asserts its own anchors and prints two
   lines, the last `two assert_eq! messages added`. If an assert fires,
   stop and report it in the Update Log. Do not hand-edit anything it
   touches, and do not edit the script.
2. **Log it at once.** Append an `### Update — <date> (cuts done)` entry
   quoting the script's output, with an edit tool.
3. **Findings** go in their own `### Findings — <date>` entry. Run
   exactly `git diff --stat`, write what it shows in one or two lines, or
   write `none`. Open no file.
4. **Run the End-to-end block** and paste its output into an
   `### Update — <date> (end-to-end verification)` entry, as a fenced
   block of the output itself, not a description of it.
5. **Commit** as `test:`.

The script is
`docs/dev/milestones/M32-findings-fixes/scripts/phase-11b-test-tidies.py`
(sha256 begins `140156f63ba0a0e0`), kept in the repo as the record of the change.

## Acceptance criteria

- [ ] The End-to-end block prints
      `renderer_literals=1 status_literals=3 builder=1 messages=2`
      (today: `renderer_literals=18 status_literals=18 builder=0
      messages=0`).
- [ ] `cargo test` reports **exactly** `passed=2609 failed=0`, the same
      count as before the phase.
- [ ] Preservation: `tests/hygiene.rs` and `cargo test --test bug_tracker`
      pass. fmt, build and clippy are clean.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`, and the
      end-to-end entry holds the fenced output.

## End-to-end verification

```sh
bash <<'E2E'
F="src/cli/render_ratatui_tests.rs src/cli/render_ratatui_layout_tests.rs"
echo "renderer_literals=$(cat $F | grep -c 'RatatuiRenderer {') status_literals=$(cat $F | grep -c 'StatusBarState {') builder=$(grep -c 'pub(super) fn test_renderer(' src/cli/render_ratatui_tests.rs) messages=$(grep -cE 'the seeded memory is indexed before the reconcile probe|the one hit is the incident memory' src/memory/index/incremental_tests.rs)"
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `renderer_literals=1 status_literals=3 builder=1 messages=2`,
hygiene `ok. 16 passed`, bug tracker `ok. 6 passed`, clippy clean,
`fmt_exit=0`, `passed=2609 failed=0`. If the suite stops early on a
failure, paste that output, run the block once more and paste both.

## Authorizations

- Run `scripts/phase-11b-test-tidies.py`; do not edit it.
- Edit the three files it names, through the script only:
  `src/cli/render_ratatui_tests.rs`,
  `src/cli/render_ratatui_layout_tests.rs`,
  `src/memory/index/incremental_tests.rs`.

## Out of scope

- #5, #6, and #51's nine table checks (rejected above).
- The unique `StatusBarState` literal (`session_id: "abcdef12"`,
  `context_window: 0`), which copies neither fixture.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-30 07:02 (started) — executor-claude

Phase 11b start: status flipped to in-progress. Task 1 next: run
`phase-11b-test-tidies.py` as the first command, before opening any source file.

### Update — 2026-10-30 07:02 (cuts done) — executor-claude

Ran, once, as the first command after the started entry (with `cargo fmt --all`
after it, per the spec):

```
$ python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-11b-test-tidies.py && cargo fmt --all
one test_renderer builder; 17 inline renderers and 15 status literals replaced
two assert_eq! messages added
```

Exit 0, no assert fired. Per the spec I have not opened the script or any source
file; task 3's `git diff --stat` is next.

### Findings — 2026-10-30 — executor-claude

`git diff --stat`: the script's three files changed as specified —
`src/cli/render_ratatui_layout_tests.rs` (392), `src/cli/render_ratatui_tests.rs`
(299), `src/memory/index/incremental_tests.rs` (10) — 160 insertions, 565
deletions overall, the remainder being this phase doc and the milestone README's
status flip.

### Update — 2026-10-30 07:02 (end-to-end verification) — executor-claude

Ran the spec's End-to-end block once, verbatim, and pasted its output below
unmodified:

````
renderer_literals=1 status_literals=3 builder=1 messages=2
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.18s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 10.93s
fmt_exit=0
passed=2609 failed=0
````

Matches the spec's expected line-for-line: `renderer_literals=1
status_literals=3 builder=1 messages=2`, hygiene `ok. 16 passed`, bug tracker
`ok. 6 passed`, clippy clean, `fmt_exit=0`, and the suite at exactly
`passed=2609 failed=0`.

### Update — 2026-10-30 14:08 (complete, server-authored)

**Summary:** </think>Everything is done. Working tree clean, all tasks complete. Final message: Summary + Notes for review, 2–6 sentences.

Completion checklist:
- [x] Phase doc status says in-progress (I never flipped to review).
- [x] README row in-progress.
- [x] No (complete) entry hand-written.
- [x] cargo fmt --all run before git add (ran right after the script; nothing edited after; e2e check confirmed fmt_exit=0).
- [x] All verification commands clean.
- [x] Final message substantive.
- [x] git status --short shows nothing (confirmed post-commit — it printed nothing after commit).
- [x] git log -1 --stat shows all five files.

Note on the grep-literal rule: the spec pins `pub(super) fn test_renderer(` and the two message strings — those are exactly what the E2E block grepped, output shows builder=1 messages=2, captured in the e2e Update Log entry. Good.

Write the final summary.</think>
**Summary**

Phase 11b executed as specified: I flipped the phase doc and milestone README row to `in-progress`, appended the started entry, then ran `phase-11b-test-tidies.py` (once, as the first post-start command, with `cargo fmt --all` after it) without opening the script or any source file — it exited 0 printing `one test_renderer builder; 17 inline renderers and 15 status literals replaced` / `two assert_eq! messages added`. The script's three named files changed as intended (#54 one `test_renderer(cols, rows, viewport, depth)` builder, #53 `default_status()`/`approval_test_status()` shared by both render test files, #51 two `assert_eq!` messages in `incremental_tests.rs`); #5, #6, and #51's nine table checks stay rejected as drafted. The End-to-end block was run once verbatim and its output pasted unmodified into the `(end-to-end verification)` entry: `renderer_literals=1 status_literals=3 builder=1 messages=2`, hygiene `ok. 16 passed`, bug tracker `ok. 6 passed`, clippy clean, `fmt_exit=0`, and the suite at exactly **`passed=2609 failed=0`**. Committed as `9ede0073` (`test:`); `git status --short` is clean.

**Notes for review**

- No deviations from the spec; the script's printed lines match its expected two-line output, so no blocker was needed. The diff is net −378 lines (187 insertions, 565 deletions), close to the estimated ~140/~560.
- The status_literal count lands at 3 (`status_literals=3`), not 1: the two moved fixtures plus the one out-of-scope unique literal (`session_id: "abcdef12"`, `context_window: 0`), which the spec says stays inline.
- The `builder=1`/`messages=2` greps also serve as the spec-pinned-literal check — both literals the spec names (`pub(super) fn test_renderer(` and the two assert messages) verified present in the e2e transcript, not unit tests alone.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
ok
test test_files_fit_the_cap ... ok
test production_files_fit_the_cap ... ok
test functions_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.27s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test ipc_tool_call_response_round_trip ... ok
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
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.10s


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
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.29s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s


running 4 tests
test gate_catches_a_multi_line_write ... ok
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-7593688ba66ac3a2)
kill: (505944): No such process
kill: (-506677): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-58152f9516574e2d)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-24319e47e01464a3)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-7278712a86caa2f2)
     Running tests/hygiene.rs (target/debug/deps/hygiene-f1caa86cc6b68e4c)
     Running tests/integration.rs (target/debug/deps/integration-6e2b1cb55f8da66f)
     Running tests/isolation.rs (target/debug/deps/isolation-555c471c8dff13d2)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-084f36955f09af75)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-122203846dd0c681)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M32-findings-fixes/README.md` — +1 -1
- `docs/dev/milestones/M32-findings-fixes/phase-11b-test-tidies.md` — +48 -1
- `src/cli/render_ratatui_layout_tests.rs` — +41 -351
- `src/cli/render_ratatui_tests.rs` — +89 -210
- `src/memory/index/incremental_tests.rs` — +8 -2

**Commit:** 9ede0073a5a788ab910b608791b1a5eed4c1c3f4

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-30

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** nvidia/GLM-5.3-Flash-FP4-Local
- **Scope deviations:** none. The source diff is byte-identical to the
  architect's dry run; the suite holds at exactly 2609 and the shape line
  moved from 18/18/0/0 to 1/3/1/2.
- **Calibration:** the script-as-file shape from phase 11 held: 27 turns,
  no source read before the run.
