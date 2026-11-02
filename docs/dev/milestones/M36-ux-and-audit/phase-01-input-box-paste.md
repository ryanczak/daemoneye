# Phase 01 — input-box-paste

**Milestone:** M36 — Chat UX and audit fixes
**Status:** review
**Drafted:** 2026-11-01
**Depends on:** none
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

Design item 1 (`docs/design/2.7.0-UX-improvements.md`): the chat input box
draws `│` on both sides of every row, so selecting several lines of it in the
terminal copies `│ text │` lines. After this phase the box keeps its top and
bottom rules and draws a **blank column** where each side border was. The
text, the wrap width and the cursor stay exactly where they are today.

## Architecture references

- `docs/architecture.md` § 1 System layers — the chat client's renderer.
- `src/cli/render_ratatui.rs` — `render_live_region`, `render_prompt_region`
  and `render_spinner_region`: the three places the input box is built, each
  with its own `Block::default().borders(Borders::ALL)…`.
- `src/cli/render_ratatui_draw.rs` — `input_content_width` (wrap width =
  terminal width − 2), which this phase must not change.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the three render functions above and the tests in
   `src/cli/render_ratatui_tests.rs` and
   `src/cli/render_ratatui_layout_tests.rs` that look for `┌` or `│`.
3. Read this entire phase doc before touching any code.

## Constraints

- **One helper builds the input box's block**, used by all three render
  functions, so the three cannot drift again.
- **Keep the inner area identical.** With `Borders::TOP | Borders::BOTTOM`
  alone, ratatui's `Block::inner` no longer insets the left and right
  columns, so text would move one column left and the wrap width would grow
  by two. ratatui 0.30's `Block` has `.padding(Padding::horizontal(1))`
  (`ratatui::widgets::Padding`), and `inner()` subtracts padding after
  borders — that keeps today's inner rectangle. The cursor arithmetic in
  `render_live_region` (`content_area.x + 1 + …`) and `input_content_width`
  must need no change; if either has to change, the inner area moved.
- **The rules have no corners.** ratatui draws a top-only or bottom-only
  border as a plain `─` run across the full width. That is the intended look.
- **Only the input box changes.** `alert_panel` (approval and credential
  panels, rounded red borders), the picker panel in `render_ratatui_draw.rs`,
  and the panels `commit_panel` / `commit_panel_labeled` write into
  scrollback keep their borders exactly as they are.
- **Tests that locate the box by `┌`** (the `find_box_top` helper in
  `render_ratatui_layout_tests.rs`, and its siblings) must find it another
  way: the box's top rule is the first row made entirely of `─`. Committed
  panels in history never produce such a row, because their top and bottom
  rows carry corners. Assertions that require `┌` or a side `│` on the input
  box assert the old behaviour and are inverted, not deleted.
- **Use `patch` with whole blocks** on these files, never `patch_lines`.

## Spec

1. **Red tests first** — in `src/cli/render_ratatui_tests.rs` (or the layout
   tests file, your choice), write the three tests below against the current
   code, run them, and keep the failing output for step 5:
   - `paste_safe_box_has_blank_side_columns` — draw the idle live region with a
     three-line input on a `TestBackend`. On every row between the box's top
     and bottom rules, the first and last columns are `" "`, and no cell in
     the live region is `│`.
   - `paste_safe_box_blank_under_spinner_and_prompt` — the same
     check for the spinner region (with typeahead text) and the prompt region
     (with a prompt string and input text).
   - `paste_safe_box_keeps_top_and_bottom_rules` — the rows directly above and
     below the box's content are `─` in every column.
2. **The helper** — add one function in `render_ratatui.rs` that returns the
   input box's `Block` (top and bottom borders, plain type, the existing gray
   border style, horizontal padding 1), and use it in all three render
   functions. The three new tests pass.
3. **Existing tests** — update `find_box_top` and every assertion that needs
   `┌` or a side `│` on the input box, per the constraint above. Every test
   that pins the cursor column, `input_content_width` (58 on a 60-wide
   backend) or the border column must still pass **unchanged in what it
   asserts about positions**; if one fails, the inner area moved — fix the
   block, not the test.
4. **Full suite** — `cargo test` passes with `failed=0`.
5. **Red runs** — one `### Update — <date> (red runs)` entry pasting step 1's
   `test result:` lines and assertion messages.
6. **Findings** — one `### Findings — <date>` entry: anything you noticed
   outside this phase's scope, two or three lines, or `none`.
7. **End-to-end** — run the block in § End-to-end verification once before
   committing and paste its output, fenced, in an
   `### Update — <date> (end-to-end verification)` entry. Required; the
   reviewer checks for it.
8. **Commit** as `feat: the chat input box has blank side columns, so a multi-line copy has no │ (M36 phase-01)`.

## Acceptance criteria

- [ ] `paste_safe_box_has_blank_side_columns`,
      `paste_safe_box_blank_under_spinner_and_prompt` and
      `paste_safe_box_keeps_top_and_bottom_rules` exist and pass, and the red-runs
      entry shows them failing on the pre-phase code.
- [ ] `input_content_width_matches_draw_width`,
      `cursor_sits_on_next_free_cell_of_short_input`,
      `cursor_matches_glyph_on_word_wrapped_input` and
      `cursor_clamp_never_reaches_border` pass with their position assertions
      unchanged (preservation; the last one's border-cell check already
      accepts `" "`).
- [ ] The approval panel tests (`approval_panel_*`) pass unchanged
      (preservation: `alert_panel` keeps its rounded border).
- [ ] The E2E block's first line is `paste_safe passed=3 failed=0`
      (today `paste_safe passed=0 failed=0`).
- [ ] The E2E block shows `hygiene`, `doc_truth` and `bug_tracker` `ok`,
      `build_warnings=0`, clippy clean, `fmt_exit=0`, and `failed=0`.

## Test plan

The three new tests carry the behaviour; the preserved position tests prove
the inner area did not move. No live run is needed in this phase — the
milestone's live check pastes three lines into the box on pinky.

## End-to-end verification

```sh
bash <<'E2E'
cargo test paste_safe_box_ 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "paste_safe passed=" p, "failed=" f}'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/cli/render_ratatui.rs`, `src/cli/render_ratatui_tests.rs` and
  `src/cli/render_ratatui_layout_tests.rs`. `src/cli/render_ratatui_draw.rs`
  only if a test helper there locates the box by `┌`.

## Out of scope

- The status bar under the box (phase 02).
- Borders of committed scrollback panels (the user-echo panel and tool
  output keep their `│`), the approval and credential panels, and the picker.
- Any change to the input editor, wrapping or cursor logic.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-02 03:18 (started)

Executor: executor-2 (Claude Opus 4.5). Flipped Status to `in-progress` and the
milestone README row to match. Starting with the red tests, then the block
helper.

### Update — 2026-11-01 19:47 (red runs)

Step 1's three tests, written against the pre-phase code and run before the
helper existed:

```
running 3 tests
test cli::render_ratatui::layout_tests::paste_safe_box_keeps_top_and_bottom_rules ... FAILED
test cli::render_ratatui::layout_tests::paste_safe_box_has_blank_side_columns ... FAILED
test cli::render_ratatui::layout_tests::paste_safe_box_blank_under_spinner_and_prompt ... FAILED
failures:
---- cli::render_ratatui::layout_tests::paste_safe_box_keeps_top_and_bottom_rules stdout ----
thread '...' panicked at src/cli/render_ratatui_layout_tests.rs:43:9:
no full-width ─ rule row found in buffer
---- cli::render_ratatui::layout_tests::paste_safe_box_has_blank_side_columns stdout ----
thread '...' panicked at src/cli/render_ratatui_layout_tests.rs:43:9:
no full-width ─ rule row found in buffer
---- cli::render_ratatui::layout_tests::paste_safe_box_blank_under_spinner_and_prompt stdout ----
thread '...' panicked at src/cli/render_ratatui_layout_tests.rs:328:9:
spinner region must not contain │, row 2: "│queued text                                               │"
test result: FAILED. 0 passed; 3 failed; 0 ignored; 0 measured; 2558 filtered out; finished in 0.00s
```

The `keeps_top_and_bottom_rules` / `has_blank_side_columns` failures show the
pre-phase box drew its rules as `┌────┐` / `└────┘` rows with corners — never a
full-width plain `─` run — and the spinner failure shows `│queued text…│`
side borders in the live region, exactly the paste hazard.

### Findings — 2026-11-01 19:47

none — everything I touched stayed inside the phase's authorized files.

### Update — 2026-11-01 19:50 (end-to-end verification)

Ran the phase doc's § End-to-end verification block once, output redirected to
`.rexymcp/out/e2e-phase01.txt` and pasted below verbatim:

```
paste_safe passed=3 failed=0
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.80s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.43s
fmt_exit=0
passed=2640 failed=0
```

First line is `paste_safe passed=3 failed=0` (was `passed=0 failed=0`), the
hygiene / doc_truth / bug_tracker gates are `ok`, `build_warnings=0`, clippy
clean, `fmt_exit=0`, full suite `failed=0` — every acceptance criterion's
command line is green. The new `input_box_block` helper is used at all three
render sites (`render_live_region`, `render_spinner_region`,
`render_prompt_region`), and `input_content_width` plus every cursor
assertion passed unchanged.

### Update — 2026-11-02 03:53 (complete, server-authored)

**Summary:** </think>Everything is complete:

- Task 1-8 done
- All acceptance criteria met
- Committed, clean tree
- Status left at in-progress (server flips to review)

Final message: Summary + Notes for review.</think>
**Summary**

The chat input box no longer draws `│` on its sides: one new helper, `input_box_block()`, builds the block for all three render sites (`render_live_region`, `render_spinner_region`, `render_prompt_region`) with `Borders::TOP | Borders::BOTTOM`, plain type, the existing gray border style, and `Padding::horizontal(1)` — ratatui's `inner()` subtracts padding after borders, so the inner rectangle, wrap width (`input_content_width` = 58 on a 60-wide backend) and cursor arithmetic are exactly where they were, and no cursor/wrap code changed. The rules now render as full-width plain `─` runs without corners, which is the signature the box's tests locate it by: `corner_row`/`box_rules` find the first rows made entirely of `─` instead of searching for `┌`, and the four assertions that required `┌` or a side `│` on the input box were inverted to assert the paste-safe behaviour (blank side columns, no `│` anywhere in the live region). The three new `paste_safe_box_*` tests were written first and are shown failing on the pre-phase code in the red-runs entry. E2E block output: `paste_safe passed=3 failed=0`, `build_warnings=0`, hygiene/doc_truth/bug_tracker all `ok`, clippy clean, `fmt_exit=0`, full suite `passed=2640 failed=0`. Committed as `571b8cb0`; working tree clean; phase doc left at `in-progress` for the server's completion tail.

**Notes for review**

- The spec's constraint said "the first and last columns of the row above the box's content are `" "`" — with top/bottom-only borders, no glyph is drawn on the content rows' side columns at all (they render as reset cells, displayed as space), which satisfies the intent; the tests assert `" "` either way.
- ratatui 0.30's `Padding` lives at `ratatui::widgets::Padding` (re-exported from `ratatui-widgets`), exactly as the spec sketched — no divergence.
- The old `corner_row` helper and `box_rules` now share the "first full-width `─` row" logic; `corner_row` is kept as the single-row variant for its existing callers.
- No live run was done, per the phase's test plan — the milestone's live check (paste three lines on pinky) still covers that.

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

test production_files_fit_the_cap ... ok
test test_files_fit_the_cap ... ok
test functions_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.10s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g5_child_inherits_depth_and_parent ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_session_info_round_trip ... ok
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
test g6_agent_namespace_field_persisted ... ok
test g6_agent_config_roundtrip ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok
test g5_mailbox_write_and_read ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test config_contains_webhook_and_stub_url ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok
test webhook_ghost_e2e_deterministic ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_multi_line_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-947e630ce8e8635d)
kill: (3091843): No such process
kill: (-3092532): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-55151dc2a3f1453f)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-f920d215d69f41da)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-820c53c972d9b621)
     Running tests/hygiene.rs (target/debug/deps/hygiene-448dcbf0c0dd5ebc)
     Running tests/integration.rs (target/debug/deps/integration-a704381cf0548134)
     Running tests/isolation.rs (target/debug/deps/isolation-0272e2131c7fc4f4)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-8f8b55305473d8e9)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-b7e6e4bc38231a68)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M36-ux-and-audit/README.md` — +1 -1
- `docs/dev/milestones/M36-ux-and-audit/phase-01-input-box-paste.md` — +63 -1
- `src/cli/render_ratatui.rs` — +15 -16
- `src/cli/render_ratatui_layout_tests.rs` — +172 -10
- `src/cli/render_ratatui_tests.rs` — +4 -8

**Commit:** 571b8cb0d2016926d42e4c959b11353be765b6ea

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
