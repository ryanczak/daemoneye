# Phase 10b — One padding for every panel title

**Status:** review
**Milestone:** M30 — Telemetry & dashboard
**Drafted:** 2026-10-25 (from the L8 rehearsal and PE run, finding F1)

## Goal

Every bordered title in `daemoneye top` reads `╭ <title> ─`, one space on
each side. Today Runs, Shells, Activity and the detail view read
`╭  1 Runs  ─` while Budget reads `╭ 2 Budget ─`.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.7 (`daemoneye top`).
- `src/cli/commands/top_tui.rs` — `draw_at`, `draw_detail_at`,
  `draw_panel`, `panel_title`, `panel_title_line`.

## Pre-flight

- `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`
  and `cargo test` are green at HEAD.
- `screen_draws_the_four_panels` (`top_tui.rs` tests) shows how a screen is
  rendered into a `TestBackend` and read back as text;
  `enter_opens_the_selected_runs_detail` shows how a detail view is opened
  on a model.

## Current state

`panel_title` returns a title already padded with one space on each side,
and `draw_panel` pads its `title` argument again. Budget does not go
through `draw_panel` (it styles `panel_title_line` itself), so it alone is
single-padded. The detail view passes a pre-padded ` Run <id> ` to
`draw_panel` and is doubled too.

## Spec

1. One place pads. On screen, the four panel titles and the detail title
   are each preceded by `╭ ` and followed by ` ─`, with exactly one space
   on each side of the text, at every terminal size and whether or not the
   panel is focused.
2. No other change to what a title says (`<n> <Name>`, `Run <id>`) or how
   it is styled (bold; cyan when focused).

## Acceptance criteria

- [ ] `cargo test every_title_is_padded_once` passes (`1 passed`): a
      screen drawn at 80×24 and at 160×48 contains `╭ 1 Runs ─`,
      `╭ 2 Budget ─`, `╭ 3 Shells ─` and `╭ 4 Activity ─` as substrings of
      its rows, and contains neither `╭  ` nor `  ─` on a title row.
- [ ] `cargo test the_detail_title_is_padded_once` passes (`1 passed`):
      with a run's detail open, a row contains `╭ Run <id> ─` and no row
      contains `╭  Run`.
- [ ] Preservation: `cargo test top_tui` has no failures (the focus-colour
      and title tests of phases 12–14 among them); `cargo test` has no
      failures.

## Test plan

- Unit, `src/cli/commands/top_tui.rs`: `every_title_is_padded_once`,
  `the_detail_title_is_padded_once`. Build the rows from the
  `TestBackend` buffer as `screen_draws_the_four_panels` does, row by row,
  so `╭ 1 Runs ─` is matched within one row.
- Mutation check, stated in the Update Log: with the fix in place,
  reintroduce one extra space in the padding, confirm
  `every_title_is_padded_once` fails, restore.
- The live look is checked by the PE at the milestone close, not here.

## End-to-end verification

Run with `bash`. Paste the output byte-exact into the Update Log.

```sh
A=/tmp/e2e-m30-10b.txt; : > "$A"
for t in every_title_is_padded_once the_detail_title_is_padded_once; do cargo test "$t" 2>&1 | grep -E '^test result: ok. 1 passed' | head -1 | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Expected: two `ok. 1 passed` lines, clippy finished with no warnings,
`fmt_exit=0`, `failed=0`.

## Authorizations

- Edit `src/cli/commands/top_tui.rs` only.
- Add the two tests. No new dependencies.

## Out of scope

- Any change to `src/cli/commands/top.rs`, the layout, the footer, the
  Budget bar or the detail text.
- The `top --once` plain-text output, which has no borders.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-25 11:06 (progress)

Started phase 10b (executor, headless run). Flipped the phase and the M30
README's 10b row to `in-progress`. Plan: make `draw_panel` render the title
text as given, drop the padding from `panel_title`, let `draw_at`'s four
call sites and `panel_title_line` pad once, have `draw_detail_at` pass the
bare `Run <id>`, then the two tests, the mutation check and the end-to-end
capture.

### Update — 2026-10-25 11:13 (progress)

All Spec tasks implemented in `src/cli/commands/top_tui.rs`: `draw_panel`
renders its `title` argument with the one padding (`format!(" {title} ")`),
`panel_title` returns the bare `<n> <Name>` (no padding), the Budget block's
`panel_title_line` pads once itself, and `draw_detail_at` passes the bare
`Run <id>`; doc comments updated to match. Two tests added:
`every_title_is_padded_once` (80×24 and 160×48; the four `╭ <n> <Name> ─`
needles per row, plus no `╭  ` and no `  ─` on any row) and
`the_detail_title_is_padded_once` (`╭ Run <id> ─` present, no `╭  Run`); the
phase-14 test `screen_draws_the_detail_view`'s title needle was updated to
the bordered form `╭ Run <id> ─` (it asserted the old doubled padding ` Run
<id> `). Mutation check done: an extra space in `draw_panel`'s padding made
both new tests fail (`a row contains "╭ 1 Runs ─"`, `the panel is titled
with the run id`), restored, both pass. `cargo fmt --all`, `cargo build`,
`cargo clippy --all-targets --all-features -- -D warnings` clean;
`cargo test` 2570 passed, 0 failed (`cargo test top_tui` 45 passed).
Next: the end-to-end capture entry.

### Update — 2026-10-25 11:14 (end-to-end verification)

Ran the block in § End-to-end verification verbatim, unmodified; the
artifact `/tmp/e2e-m30-10b.txt` was generated mechanically by the block
(`A=/tmp/e2e-m30-10b.txt; : > "$A"`, the two `cargo test <name>` lines, the
clippy tail, the `cargo fmt --all --check` exit and the full-suite awk) and
its contents are pasted byte-exact below.

```
every_title_is_padded_once: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
the_detail_title_is_padded_once: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2570 failed=0
```

Expected vs actual: two `ok. 1 passed` lines — present; clippy finished
with no warnings — the tail line is a bare `Finished`, no warnings;
`fmt_exit=0` — got 0; `failed=0` — got 0. Every line of the expected shape
is present.

### Update — 2026-10-25 18:16 (complete, server-authored)

**Summary:** **Summary + Notes for review:**

Implemented phase 10b in `src/cli/commands/top_tui.rs` (the only file authorized): `draw_panel` now owns the one padding — it renders `format!(" {title} ")` for whatever title it is handed — and `panel_title` returns the bare `<n> <Name>`, so Runs/Shells/Activity (which were doubled) and Budget (which styles `panel_title_line` itself, now padding once) all read `╭ <title> ─`; the detail view passes the bare `Run <id>` to the same path. Title text and styling are untouched (`<n> <Name>`, `Run <id>`; bold, cyan when focused). Added `every_title_is_padded_once` (draws at 80×24 and 160×48, asserts the four `╭ <n> <Name> ─` needles within single rows via a per-row buffer read, and rejects any row containing `╭  ` or `  ─`) and `the_detail_title_is_padded_once` (`╭ Run <id> ─` present, no `╭  Run`); the mutation check was run as specified — an extra space in `draw_panel`'s padding failed both tests, then was restored. The phase-14 test `screen_draws_the_detail_view`'s title needle had to be updated from ` Run <id> ` (the old doubled padding) to `╭ Run <id> ─`, a preservation-forcing change I flag for review. End-to-end block ran verbatim, output pasted byte-exact in the Update Log's `(end-to-end verification)` entry: two `ok. 1 passed` lines, clippy clean, `fmt_exit=0`, `passed=2570 failed=0` (`cargo test top_tui` 45 passed, 0 failed). Status left at `in-progress`; committed as `5888a786` with a clean tree.

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
st readme_tools_tables_match_the_code ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_documents_every_config_field ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test readme_documents_every_chat_slash_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test runtime_docs_describe_no_tmux_execution ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s


running 34 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test cancel_request_roundtrip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
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
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.13s


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

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.37s


running 4 tests
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_chained_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-a5bf14860b8766a6)
kill: (508919): No such process
kill: (-509626): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-fa3aae999c224501)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-af9329f81467cdee)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-a44fb637fa3308a3)
     Running tests/integration.rs (target/debug/deps/integration-cf27c4f796c5d8c0)
     Running tests/isolation.rs (target/debug/deps/isolation-0a0c421b4b2b9fc2)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-76069814ada3e33b)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M30-telemetry-dashboard/README.md` — +1 -1
- `docs/dev/milestones/M30-telemetry-dashboard/phase-10b-title-padding.md` — +50 -1
- `src/cli/commands/top_tui.rs` — +94 -11

**Commit:** 5888a786c703250a692ffdfda5661af0a926f3b5

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
