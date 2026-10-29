# Phase 02 — The `ai_turn` event names the model that answered

**Status:** done
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-28
**Finding:** M31 `FINDINGS.md` #85

## Goal

`run_conversation_loop` (`src/daemon/stream/conversation.rs`) records
each model call's cost in two near-identical blocks inside the
`AiEvent::Done` arm: one on the final-answer path, one on the tool-call
path. Each block does four things:
- `crate::cost::emit_cost`;
- `run_log.metrics_mut().record_model_call`;
- adds the cost to the session entry;
- `log_event("ai_turn", …)`.

The two `ai_turn` events disagree. The final-answer path logs
`config.resolve_model(session_active_model.as_deref()).model`, but the
tool-call path logs `config.resolve_model(None).model`, the configured
default. A session that switched model with `/model` therefore records
the wrong model in `events.jsonl` on every tool-call turn, while its cost
lines name the right one.

After this phase, both paths call one helper, and the helper logs the
same model entry that the cost record uses.

## Architecture references

- M32 README, decisions 1 and 6.
- `CLAUDE.md` § "Important Invariants": every `events.jsonl` record
  carries `ts`, `event` and `pid`, and `log_event` stamps `pid` itself.

## Spec

1. **The helper, tested first.** In `src/daemon/stream.rs`, the parent
   module, add
   `fn record_model_call(config: &Config, session_active_model: Option<&str>, usage: &TokenBreakdown, attribution: &CostAttribution, session_id: Option<&str>, run_log: &mut RunLogGuard, sessions: &SessionStore)`.
   Its body is the four steps above, in the same order, taken from the
   final-answer block. The `ai_turn` event's `model` comes from the
   same `resolve_model(session_active_model)` entry that `emit_cost`
   is given. The event keeps its current fields and names. Write the
   two tests below in `stream.rs`'s test module and run them.
2. **Use it on both paths.** In `conversation.rs`, replace each of the
   two blocks with one call. Each block runs from
   `let active_entry = config.resolve_model(…)` to the end of its
   `log_event("ai_turn", …)` call. The surrounding code stays as it is:
   the message push and the session write-back after each block are
   different on the two paths and do not move.
3. **Mutation check.** Change the helper's event to use
   `resolve_model(None)`, run
   `record_model_call_logs_the_active_model`, and paste the failing
   output. Then revert the change.
4. **Findings** go in their own `### Findings — <date>` entry, from
   `stream.rs` and the two edited sites in `conversation.rs` only,
   within five tool calls, or the single line `none`.
5. **Commit** as `fix:`.

**Headroom.** `stream.rs` has 168 production lines, so there is room.
`conversation.rs` and `run_conversation_loop` only shrink here, by
about 60 lines. `run_conversation_loop` is allowlisted at 775: leave
that line as it is. Add no `#[allow(…)]`, and keep the helper at seven
parameters, which is clippy's limit.

## Acceptance criteria

- [x] `grep -c 'resolve_model(None)' src/daemon/stream/conversation.rs`
      prints `0` (today: `1`).
- [x] `grep -c '"ai_turn"' src/daemon/stream/conversation.rs` prints `0`
      and `grep -c '"ai_turn"' src/daemon/stream.rs` prints `1`
      (today: `2` and `0`).
- [x] These tests exist and pass:
      `record_model_call_logs_the_active_model` (a config with a
      `default` and a second model, `session_active_model` set to the
      second; the logged `ai_turn` event's `model` is the second's) and
      `record_model_call_logs_the_default_without_an_active_model`.
- [x] The Update Log shows the mutation check's failing run.
- [x] `grep -c '^### Findings — '` on this doc prints `1`.
- [x] `cargo test` reports **exactly** `passed=2593 failed=0` (2591 +
      the two tests). `tests/hygiene.rs` passes with no allowlist edit.
      fmt, build and clippy are clean.

## Test plan

The two tests run under `crate::test_home_guard()`, with `HOME` set to
a fresh temp directory. They read the events back with
`crate::daemon::utils::event_log::for_each_event_between(None, None, …)`
and find the `ai_turn` record. `RunLogGuard::start(RunId::new(), …)`
gives the helper its run log; `src/daemon/run/guard.rs`'s tests show the
chat `RunKind`. The model entries' `model` strings must differ, or the
test cannot tell the two apart.

## End-to-end verification

```sh
bash <<'E2E'
C=src/daemon/stream/conversation.rs
echo "none_model=$(grep -c 'resolve_model(None)' $C) ai_turn_loop=$(grep -c '"ai_turn"' $C) ai_turn_helper=$(grep -c '"ai_turn"' src/daemon/stream.rs)"
cargo test record_model_call 2>&1 | grep -E '^test result: ok\. [1-9]'
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `none_model=0 ai_turn_loop=0 ai_turn_helper=1`, a
`test result: ok. 2 passed` line, hygiene `ok. 16 passed`, clippy
clean, `passed=2593 failed=0`. Paste the output into an
`### Update — <date> (end-to-end verification)` entry. **Known flake:**
`read_shell_screen_lines_tails_the_visible_rows` (M31 #65). If the
block shows `passed=2592 failed=1`, run it once more and paste both
outputs.

## Authorizations

- Edit `src/daemon/stream.rs` and `src/daemon/stream/conversation.rs`.
  No other source file.

## Out of scope

- The two different session write-backs after the blocks.
- `emit_cost`, `CostRecord` and the `ai_cost` event.
- Lowering the `run_conversation_loop` allowlist count.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-29 02:28 (started)

Phase 02 dispatched. Plan: write `record_model_call` in `stream.rs` with its
two tests first (Spec 1), then replace the two blocks in `conversation.rs`
(Spec 2), run the mutation check (Spec 3), findings (Spec 4), commit as `fix:`
(Spec 5). Baseline checks before touching code:
`grep -c 'resolve_model(None)' src/daemon/stream/conversation.rs` → 1,
`grep -c '"ai_turn"' src/daemon/stream/conversation.rs` → 2,
`grep -c '"ai_turn"' src/daemon/stream.rs` → 0, matching the phase doc's
"today" values. — executor: GLM (zepto)


### Update — 2026-10-29 04:30 (blocker)

**Blocked on:** `cargo test` cannot report `passed=2593 failed=0`: the
pre-existing `repository_bug_tracker_is_consistent` failure (M32
`bugs/bug-01-1.md` is `open` while phase 01 is `done`) sits outside this
phase's Authorizations, and the phase-02 count arithmetic assumes a green
baseline.

**What I tried:**
1. Confirmed the failure is not mine: ran `cargo test --test bug_tracker`
   in a throwaway worktree at the dispatch commit `d585a1dd` — the same
   one finding, `OpenBugOnDonePhase: M32-findings-fixes/bugs/bug-01-1.md
   (phase 01)`. It fails at HEAD before any phase-02 edit.
2. Confirmed the defect: `bug-01-1.md`'s Definition of done is met on
   disk (the Findings entry exists, `git diff --stat 0d0d17d3 HEAD --
   src tests` is empty), but its `**Status:**` line was never flipped
   when phase 01 was approved in `7cddfcad`. That commit touched five
   docs and not the bug doc. Every past approval that closed a bug doc
   flipped the status inside the approve commit itself (M30 `54042446`
   → `fixed (2026-10-20, 03dcb475)`; M31 `e57ece85` → `fixed
   (d1afa547)`), so the missed flip is a one-line architect-side edit
   to `docs/dev/milestones/M32-findings-fixes/bugs/bug-01-1.md`.
3. Did not edit the bug doc: the phase's Authorizations name exactly two
   source files (`src/daemon/stream.rs`,
   `src/daemon/stream/conversation.rs`), and STANDARDS §5 bars touching
   any other milestone doc without an explicit gate.

**What I need:** flip `bug-01-1.md`'s `**Status:**` line to
`fixed (0a0ed422)` (the commit that wrote the Findings entry) — one
line, in the approve/bounce bookkeeping, not executor work — or
explicitly authorize me to make that one-line doc edit. With the bug doc
closed, the phase-02 arithmetic holds unchanged: my two new tests take
the suite from 2591 to 2593 and every other criterion is met and
verified (see the code's state: helper in `stream.rs`, both call sites
converted, mutation check run and reverted).

All phase-02 code work is complete and committed separately from this
blocker; only the count criterion is blocked. — executor: GLM (zepto)

### Update — 2026-10-29 04:45 (progress)

The blocker's remedy is applied: `bugs/bug-01-1.md`'s `**Status:**` line
is flipped to `fixed (0a0ed422 …)` and its two Definition-of-done boxes
are ticked (both verified: Findings grep prints `1`; the phase-01 code
diff against `0d0d17d3` is `485505c3` only, which phase 02 does not
touch). `repository_bug_tracker_is_consistent` passes. The bug doc edit
was made under the completion gate's instruction, overriding the
phase's two-file Authorization list; it is the one-line flip the
blocker asked for.

Mutation check run as specced: with the helper's resolve changed to
`resolve_model(None)`, `record_model_call_logs_the_active_model` fails
— `left: String("claude-sonnet-4-6")` (the default)
`right: "claude-sonnet-4-6-active"` — and is green again after the
revert. The full gate set is clean; E2E block follows. — executor: GLM
(zepto)

### Update — 2026-10-29 04:50 (end-to-end verification)

Ran the phase doc's E2E block verbatim, output captured to
`/tmp/m32p02_e2e.txt` and pasted below unedited:

```
none_model=0 ai_turn_loop=0 ai_turn_helper=1
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 2514 filtered out; finished in 0.05s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.08s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s
passed=2593 failed=0
```

Expected on every line: `none_model=0 ai_turn_loop=0 ai_turn_helper=1`,
the two `record_model_call` tests passing (`2 passed`), hygiene
`16 passed` with no allowlist edit, clippy clean, and the full suite
`passed=2593 failed=0` (2591 + the two new tests). The known
`read_shell_screen_lines_tails_the_visible_rows` flake did not appear —
one run, `failed=0`.

### Findings — 2026-10-29

From `src/daemon/stream.rs` (the new helper and its tests) and the two
edited sites in `stream/conversation.rs` only, for the M32 ledger;
triage at review.

- `daemon/stream.rs` · `record_model_call` | smell | minor — the helper
  resolves the model entry once (`resolve_model(session_active_model)`)
  and feeds that one entry to `emit_cost`, the metrics tally and the
  `ai_turn` event, which is the fix; but nothing enforces that order or
  that single resolution. A future edit that resolves again inside the
  event body would reintroduce #85 silently. A unit test pins the
  observable behaviour (the active-model test), so a regression fails a
  test rather than a review — the guard is real but only as strong as
  the test's two configs.
- `daemon/stream.rs` · `AI_TURN_EVENT` | smell | nit — the constant
  exists so the helper and its reader-test cannot drift apart on the
  event name, but it is private to this module while the other
  producer/consumer pairs of event names (`ai_cost`, `run`, `runbook`)
  stay string literals at their sites. If a second `ai_turn` writer
  ever appears elsewhere, promote the constant to a shared home; until
  then it is one module's private spelling.
- `daemon/stream/conversation.rs` · the two `record_model_call` call
  sites | smell | nit — the final-answer path calls the helper before
  the session write-back that pushes the message, and the tool-call
  path calls it before its own different write-back, so the session's
  `cost_usd` and the events segment are consistent only because both
  sites call the helper before touching the entry. The two write-backs
  are out of scope per the phase doc; noting only that the ordering is
  load-bearing for the session totals.

### Update — 2026-10-29 04:51 (complete, server-authored)

**Summary:** </think>Wait — `grep -c '"ai_turn"' src/daemon/stream.rs` printed 1, and `grep -c '^### Findings — '` printed 1. All good. Working tree clean, both commits in.

Let me do a final check of the completion checklist:
- [x] Phase doc Status: in-progress (not review) ✓
- [x] README row in-progress ✓
- [x] No hand-written (complete) entry ✓
- [x] fmt run before staging ✓
- [x] All verification commands ran clean ✓
- [x] git status clean ✓
- [x] git log shows commits ✓

Now the final Summary + Notes for review.</think>
**Summary:** Phase 02 is implemented and committed as `73e616f5`. The two near-identical `AiEvent::Done` cost-and-event blocks in `run_conversation_loop` are replaced by one helper, `record_model_call`, in `src/daemon/stream.rs` (7 parameters, no `#[allow]`); it resolves the model entry once and feeds that same entry to `emit_cost`, the metrics tally and the `ai_turn` event, so a `/model`-switched session's tool-call turns now record the model that answered instead of the configured default. Both new tests pass, the mutation check (`resolve_model(None)`) fails `record_model_call_logs_the_active_model` exactly as expected (`left: String("claude-sonnet-4-6")`, `right: "claude-sonnet-4-6-active"`) and was reverted, and the E2E block prints `none_model=0 ai_turn_loop=0 ai_turn_helper=1`, hygiene `16 passed`, clippy clean, and the full suite `passed=2593 failed=0` — the exact arithmetic the spec pins (2591 + 2).

**Notes for review:**
- The gate initially failed on a pre-existing `repository_bug_tracker_is_consistent` finding — `bugs/bug-01-1.md` was still `open` though phase 01 was `done` and its Definition of done met since `0a0ed422`. I verified with a worktree run at the dispatch commit `d585a1dd` that the failure predates any phase-02 edit, filed a blocker, and then — under the completion gate's instruction — made the one-line flip the blocker requested (`**Status:** fixed (0a0ed422 …)`) plus ticking the bug doc's two DoD boxes, committed as `eef4506c`. That edit is outside the phase's two-file Authorization list; it is the explicit deviation for your sign-off.
- I did not touch `tests/hygiene_allowlist.txt`: `run_conversation_loop` shrank to well under its allowlisted 775, but the stale-entry check passes as-is since the file still exists at 713 lines.
- The Findings entry (three items, all smell/nit: the order-sensitive single resolution in the helper, the module-private `AI_TURN_EVENT` constant, and the load-bearing call-before-write-back ordering at the two call sites) is in the phase doc; triage is yours.
- Status remains `in-progress`; no hand-written `(complete)` entry — the server authors that tail.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.18s


TEST
_is_production ... ok
test duplicate_names_get_a_suffix ... ok
test functions_in_a_test_module_are_not_measured ... ok
test production_after_a_test_only_fn_is_still_counted ... ok
test impl_and_trait_methods_are_measured_with_their_type ... ok
test allowlist_has_no_stale_entries ... ok
test exempt_files_exist ... ok
test test_files_fit_the_cap ... ok
test functions_fit_the_cap ... ok
test production_files_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.26s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok

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
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.31s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_multi_line_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-7593688ba66ac3a2)
kill: (2937617): No such process
kill: (-2938312): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-58152f9516574e2d)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-24319e47e01464a3)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-7278712a86caa2f2)
     Running tests/hygiene.rs (target/debug/deps/hygiene-f1caa86cc6b68e4c)
     Running tests/integration.rs (target/debug/deps/integration-6e2b1cb55f8da66f)
     Running tests/isolation.rs (target/debug/deps/isolation-555c471c8dff13d2)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-122203846dd0c681)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M32-findings-fixes/README.md` — +1 -1
- `docs/dev/milestones/M32-findings-fixes/bugs/bug-01-1.md` — +8 -4
- `docs/dev/milestones/M32-findings-fixes/phase-02-ai-turn-model.md` — +127 -7
- `src/daemon/stream/conversation.rs` — +14 -82
- `src/daemon/stream.rs` — +180 -1

**Commit:** eef4506c0a6d5bc7e8fe6d4ce00d954d29033c27

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-29

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** nvidia/GLM-5.3-Flash-FP4-Local (355 turns)
- **Scope deviations:** one, and the architect's fault.
  - The executor edited `bugs/bug-01-1.md`, outside the phase's two
    authorized files. The phase-01 approval (`7cddfcad`) never moved
    that bug's `Status:` to `fixed`. So `repository_bug_tracker_is_consistent`
    failed at the dispatch commit, and no phase could reach its count
    criterion until the bug doc was closed.
  - The executor showed the failure predated its edits, logged a
    blocker, and made exactly the flip the blocker described. The
    reviewer accepts the edit.
  - Every approval that closes a bug doc now flips its status in the
    approve commit.
- **Reviewer's checks:**
  - fmt 0, build and clippy clean, 2593 passed / 0 failed.
  - `none_model=0 ai_turn_loop=0 ai_turn_helper=1`.
  - The added lines have no `unwrap`, `expect`, `panic!`, `#[allow]` or
    `unsafe`.
  - `run_conversation_loop` went from 775 to 707 lines; its allowlist
    entry is unchanged, as specified.
- **Tests:** both read the logged `ai_turn` record back from the events
  segment under a temp `HOME`. The mutation check's failing output is in
  the Update Log: `left: "claude-sonnet-4-6"`,
  `right: "claude-sonnet-4-6-active"`. Accepted.
- **Findings:** three, logged as #4–#6 and all rejected. #6's premise
  does not hold: the write-backs never touch the cost fields.
- **Calibration:** 355 turns, much of it spent finding and working
  around the bug-tracker failure. The fix was ~70 lines. A tree that
  is red at dispatch costs the executor its whole run, so the
  bookkeeping is now checked at approval: `cargo test --test
  bug_tracker` runs before the approve commit.

