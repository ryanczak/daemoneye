# Phase 06 — Panel borders fill by display width

**Status:** done
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-29
**Findings:** M31 `FINDINGS.md` #57 and #60 (#58 is rejected below; #55
and #56 move to phase 12)

## Goal

`commit_panel_labeled` (`src/cli/render_ratatui_draw.rs`) draws a
panel's top and bottom borders by hand. The dash fill is computed from
`title.chars().count()` and `padded.chars().count()`, but the terminal
lays out cells. A double-width title or bottom label (CJK, many emoji)
therefore makes the border wider than the terminal (#57). On a
60-column terminal, the title `漢字 title` gives a 62-cell top border.

After this phase, both widths come from ratatui's `Span::width()`, which
measures display cells, so both border rows are exactly as wide as the
terminal. The module doc's first line also gets its closing backtick
(#60).

**#58 is rejected.** `commit`, `commit_styled` and
`commit_panel_labeled` raise `inserted_rows` and record history before
they know whether `insert_before` succeeded. When it fails, the error
propagates and the caller tears the renderer down. The counters are
never read again, so nothing observable goes wrong, and ratatui's
`TestBackend` cannot fail, so no test could show a difference.

**#55 and #56 move to phase 12** (client-prompts): the three panels'
copied status-bar tail, and the picker panel's border. They belong with
the other `StatusBarState` consolidation there.

## Architecture references

- M32 README, decisions 1 and 6.

## Spec

1. **Add the test below, and run it red.** Append it to the end of
   `src/cli/render_ratatui_tests.rs`, then run
   `cargo test --lib commit_panel_borders_fill_by_display_width`. It must
   fail with `left: 62` / `right: 60` on the top border. Paste that
   output into the Update Log.
2. **The fix.** In `commit_panel_labeled`, change
   `let title_len = title.chars().count();` to
   `let title_len = Span::raw(title).width();`, and change
   `let label_vis = padded.chars().count();` to
   `let label_vis = Span::raw(padded.as_str()).width();`. Run the test
   again; it must pass.
3. **The doc.** On line 1 of `render_ratatui_draw.rs`, change
   `` `RatatuiRenderer's `` to `` `RatatuiRenderer`'s ``.
4. **Findings** go in their own `### Findings — <date>` entry, from
   `commit_panel_labeled` only, within five tool calls, or the single
   line `none`.
5. **Commit** as `fix:`.

**Do not look further.** Nothing else in the renderer changes. The
architect dry-ran this exact change on `HEAD`: red at step 1, green at
step 2, clippy and fmt clean, and the full suite at
`passed=2600 failed=0`.

```rust
/// A panel's border rows are as wide as the terminal even when the title
/// or the bottom label holds double-width characters.
#[test]
fn commit_panel_borders_fill_by_display_width() {
    let mut renderer = make_test_renderer();
    renderer
        .commit_panel_labeled("漢字 title", &["body".to_string()], false, Some("ラベル"))
        .unwrap();
    let rows: Vec<&Line<'static>> = renderer.history.iter().collect();
    let top = rows[0];
    let bottom = rows[rows.len() - 2];
    assert_eq!(top.width(), 60, "top border: {top:?}");
    assert_eq!(bottom.width(), 60, "bottom border: {bottom:?}");
}
```

## Acceptance criteria

- [ ] `grep -c 'chars().count()' src/cli/render_ratatui_draw.rs` prints
      `0` (today: `2`).
- [ ] `head -1 src/cli/render_ratatui_draw.rs` contains
      `` `RatatuiRenderer`'s `` (today it does not).
- [ ] `commit_panel_borders_fill_by_display_width` passes, and the
      Update Log shows it failing at step 1.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`.
- [ ] `cargo test` reports **exactly** `passed=2600 failed=0` (2599 + 1).
      `tests/hygiene.rs` and `cargo test --test bug_tracker` pass. fmt,
      build and clippy are clean.

## End-to-end verification

```sh
bash <<'E2E'
F=src/cli/render_ratatui_draw.rs
echo "char_counts=$(grep -c 'chars().count()' $F) doc=$(head -1 $F | grep -c 'RatatuiRenderer`.s')"
cargo test --lib commit_panel 2>&1 | grep -E '^test result'
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `char_counts=0 doc=1`, `test result: ok. 6 passed` for the
`commit_panel` tests, hygiene `ok. 16 passed`, bug tracker `ok. 6
passed`, clippy clean, `passed=2600 failed=0`. Paste the output into an
`### Update — <date> (end-to-end verification)` entry. **Known flake:**
`read_shell_screen_lines_tails_the_visible_rows` (M31 #65). If the
block shows `passed=2599 failed=1`, run it once more and paste both
outputs.

## Authorizations

- Edit `src/cli/render_ratatui_draw.rs` (the two lines and the doc line)
  and append to `src/cli/render_ratatui_tests.rs`. No other source file.

## Out of scope

- Body lines, which `wrap_line_hard` wraps by chars.
- #58 (rejected above); #55 and #56 (phase 12).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-29 12:02 (progress)

Started phase 06 (executor). Plan: append the display-width border test to
`src/cli/render_ratatui_tests.rs` and capture it failing red with
`left: 62` / `right: 60`, then switch the two fill computations in
`commit_panel_labeled` to `Span::width()` and the module doc's first line
to the fixed `` `RatatuiRenderer`'s ``, then findings, E2E capture and a
`fix:` commit.

### Update — 2026-10-29 12:07 (progress)

Task 1: the new test ran red as specified —
`assertion \`left == right\` failed: top border: Line::from_iter([Span::from("╭─ ")…, Span::from("漢字 title")…, Span::from(" ───…──╮")…])`
with `left: 62` / `right: 60` at `src/cli/render_ratatui_tests.rs:968`.
Tasks 2–3: swapped both counts to `Span::width()`
(`Span::raw(title).width()`, `Span::raw(padded.as_str()).width()`) and
restored the module doc's closing backtick; the test now passes, and
`grep -c 'chars().count()'` on `render_ratatui_draw.rs` is 0. Findings
follow in their own entry.

### Findings — 2026-10-29

none

### Update — 2026-10-29 12:15 (end-to-end verification)

Ran the phase doc's § End-to-end verification block verbatim, redirected
to `/tmp/e2e-06.txt`; the file's contents follow, between the fences.

```
char_counts=0 doc=1
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 2517 filtered out; finished in 0.00s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.12s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s
passed=2600 failed=0
```

All expected values match: `char_counts=0 doc=1`, `ok. 6 passed` for the
`commit_panel` tests, hygiene `ok. 16 passed`, bug tracker `ok. 6
passed`, clippy clean, `passed=2600 failed=0` (2599 + 1). The named M31
#65 flake did not appear — one run, first pass.

PASTE MATCH self-check:

```
D=docs/dev/milestones/M32-findings-fixes/phase-06-renderer-fixes.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-06.txt
diff /tmp/pasted-06.txt /tmp/e2e-06.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Output: `PASTE MATCH`.

### Update — 2026-10-29 19:12 (complete, server-authored)

**Summary:** </think>Everything is done. Status is still `in-progress` (I did not flip to review), README row says in-progress, no `(complete)` entry written by me, all verification passed, working tree clean.

Final message: Summary + Notes for review (2–6 sentences).</think>
**Summary:** Phase 06 is complete. I appended `commit_panel_borders_fill_by_display_width` to `src/cli/render_ratatui_tests.rs` and captured it failing red exactly as the spec predicted (`left: 62` / `right: 60` on the top border), then replaced both `chars().count()` fills in `commit_panel_labeled` with `Span::raw(...).width()` (`Span::raw(padded.as_str()).width()` for the bottom label), fixed the module doc's first line to `` `RatatuiRenderer`'s ``, and filed a `none` Findings entry. The E2E block ran verbatim and matched every expected value: `char_counts=0 doc=1`, `commit_panel` tests `ok. 6 passed`, hygiene `ok. 16 passed`, bug tracker `ok. 6 passed`, clippy clean, full suite `passed=2600 failed=0` (the M31 #65 flake did not appear). Committed as `fix:` `01d3d824` plus a `docs:` E2E-evidence commit `c7fa0d36`; working tree is clean.

**Notes for review:** The new test's bottom-border assertion (`rows[len-2]`, the label row before the trailing blank) also exercises the `Some("ラベル")` path, so both the title and bottom-label fixes are pinned by one test. Status is left at `in-progress` and the README row matches, per the lifecycle split — no `(complete)` entry was hand-written. The PASTE MATCH self-check against the pasted E2E fence prints `PASTE MATCH`.

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
k
test production_after_a_test_only_fn_is_still_counted ... ok
test functions_in_a_test_module_are_not_measured ... ok
test function_length_spans_signature_to_closing_brace ... ok
test impl_and_trait_methods_are_measured_with_their_type ... ok
test allowlist_has_no_stale_entries ... ok
test production_files_fit_the_cap ... ok
test functions_fit_the_cap ... ok
test exempt_files_exist ... ok
test test_files_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.29s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_read_and_clear ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s


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
kill: (3639358): No such process
kill: (-3640083): No such process
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
- `docs/dev/milestones/M32-findings-fixes/phase-06-renderer-fixes.md` — +56 -1
- `src/cli/render_ratatui_draw.rs` — +3 -3
- `src/cli/render_ratatui_tests.rs` — +15 -0

**Commit:** c7fa0d36cece987b381a28accfba69b5d584b3fe

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-29

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** nvidia/GLM-5.3-Flash-FP4-Local
- **Scope deviations:** none.
  - The production diff is exactly the three specified lines.
  - The test is the dictated one.
  - The work is in two commits: `fix:` `01d3d824` for the code and
    `docs:` `c7fa0d36` for the E2E evidence.
- **Reviewer's checks:**
  - fmt 0, build and clippy clean, 2600 passed / 0 failed.
  - `bug_tracker` passes, with 6 tests.
  - `char_counts=0 doc=1`, and one Findings heading.
- **Tests:** the red run is quoted inline, not as a fenced paste. It
  shows the assertion message, `left: 62` / `right: 60` and the line,
  `render_ratatui_tests.rs:968`, all matching the architect's dry run.
  Accepted.
- **Findings:** `none`.
- **Calibration:** the fourth dictated-seam phase in a row first try.

