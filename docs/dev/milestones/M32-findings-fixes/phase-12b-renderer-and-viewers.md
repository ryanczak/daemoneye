# Phase 12b — Renderer and viewers

**Status:** review
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-30
**Depends on:** phase-12
**Estimated diff:** ~115 insertions, ~145 deletions
**Tags:** language=rust, kind=refactor, size=s
**Findings:** M31 `FINDINGS.md` #27, #30, #55, #56 (#28 is rejected
below)

## Goal

The chat renderer and the full-screen viewers repeat their setup code.
After this phase:

- **#55:** one `render_status_bar` in `src/cli/render_ratatui.rs` draws
  the dim ` session:… · model · up … ` line. It replaces six copies:
  three in the live-region renderers and three in the panel methods of
  `render_ratatui_draw.rs`.
- **#56:** one `alert_panel` builds the red-bordered shell the approval
  and credential dialogs share. The picker keeps its plain border: it
  asks for a choice, not an approval, and the helper's doc says so.
- **#27:** `src/cli/viewer.rs` gains `enter_alt_screen` and
  `resize_events`. The `/tail`, `/log` and attach viewers call them
  instead of each entering the alternate screen, building the teardown
  guard, the terminal and the resize stream by hand.
- **#30:** a new test, `render_tail_keeps_the_log_status_line_while_a_note_is_set`,
  pins one ordering in `render_tail`'s status match that no test held.
  The other load-bearing ordering, replay before log, is already pinned
  by `render_tail_shows_the_replay_status_line`.

No behaviour changes. The suite goes from **2609** to **exactly 2610**:
one test added, none removed.

**Rejected:** **#28**, the empty reader `run_log_viewer` passes to
`tail_loop` as "no frame source". The function's doc comment names it,
`tail_loop`'s contract makes a closed stream mean exactly that, and the
log tests drive the loop the same way. An `Option` would add a branch to
the loop for one caller.

## Architecture references

- `src/cli/viewer.rs` § `AltScreenGuard`: the guard the new entry
  returns.
- M32 README, decision 2.

## Spec

1. **Run this, once, as your first command after the started entry:**

   ```sh
   python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-12b-renderer-and-viewers.py && cargo fmt --all
   ```

   **Do not open the script, and do not read any source file before
   running it.** It was run on a scratch worktree of `840ee61c`: fmt and
   clippy clean, hygiene passing, the full suite at
   `passed=2610 failed=0`. It asserts its own anchors and prints three
   lines, the last
   `render_tail_keeps_the_log_status_line_while_a_note_is_set added`. If
   an assert fires, stop and report it in the Update Log. Do not
   hand-edit anything it touches, and do not edit the script.
2. **Log it at once.** Append an `### Update — <date> (cuts done)` entry
   quoting the script's output, with an edit tool.
3. **Findings** go in their own `### Findings — <date>` entry. Run
   exactly `git diff --stat`, write what it shows in one or two lines, or
   write `none`. Open no file.
4. **Run the End-to-end block** and paste its output into an
   `### Update — <date> (end-to-end verification)` entry, as a fenced
   block of the output itself, not a description of it.
5. **Commit** as `refactor:`.

The script is
`docs/dev/milestones/M32-findings-fixes/scripts/phase-12b-renderer-and-viewers.py`
(sha256 begins `16f70f90e8e05d60`), kept in the repo as the record of the change.

### The red run is done — do not repeat it

The new test was run red at drafting, with the log arm moved below the
note-carrying tail arm. A log view stepped to a command then showed the
tail's status line:

```
"tail s3-17 · 5×20 · exited · command 1/3 · exit 0 · cmd-0 · n/N older/newer · esc to close"
```

and the test failed on `bottom.starts_with("log s3-17")`. With the arm
order as it stands, it passes.

## Acceptance criteria

- [ ] The End-to-end block prints
      `status_bars=1 alert_shells=0 alt_entries=0 order_test=1` (today:
      `status_bars=6 alert_shells=2 alt_entries=6 order_test=0`).
- [ ] `cargo test` reports **exactly** `passed=2610 failed=0` (today
      2609).
- [ ] Preservation: `tests/hygiene.rs` and `cargo test --test bug_tracker`
      pass; `tests/hygiene_allowlist.txt` is unchanged. fmt, build and
      clippy are clean.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`, and the
      end-to-end entry holds the fenced output.

## End-to-end verification

```sh
bash <<'E2E'
echo "status_bars=$(cat src/cli/render_ratatui.rs src/cli/render_ratatui_draw.rs | grep -c '" session:{} · {} · up {} ",') alert_shells=$(grep -c '.border_style(Style::default().fg(red))' src/cli/render_ratatui_draw.rs) alt_entries=$(cat src/cli/tail_view.rs src/cli/attach.rs | grep -c 'EnterAlternateScreen') order_test=$(grep -c 'fn render_tail_keeps_the_log_status_line_while_a_note_is_set' src/cli/tail.rs)"
git diff --quiet HEAD -- tests/hygiene_allowlist.txt && echo allowlist=unchanged
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `status_bars=1 alert_shells=0 alt_entries=0 order_test=1`,
`allowlist=unchanged`, hygiene `ok. 16 passed`, bug tracker
`ok. 6 passed`, clippy clean, `fmt_exit=0`, `passed=2610 failed=0`. If
the suite stops early on a failure, paste that output, run the block once
more and paste both.

## Authorizations

- Run `scripts/phase-12b-renderer-and-viewers.py`; do not edit it.
- Edit the six files it names, through the script only:
  `src/cli/render_ratatui.rs`, `src/cli/render_ratatui_draw.rs`,
  `src/cli/viewer.rs`, `src/cli/tail_view.rs`, `src/cli/attach.rs`,
  `src/cli/tail.rs`.

## Out of scope

- #28 (rejected above).
- `run_transcript_viewer` and `daemoneye top`, which enter the
  alternate screen with mouse capture or raw mode and so differ from the
  three viewers this phase joins.

## Update Log

### Update — 2026-10-30 16:53 (started — executor)

Phase 12b started. Spec task 1 ran first:
`python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-12b-renderer-and-viewers.py && cargo fmt --all`,
both clean (exit 0). Script output, quoted verbatim:

```
one render_status_bar for six status bars; one alert_panel for two dialogs
one enter_alt_screen and resize_events for the tail, log and attach viewers
render_tail_keeps_the_log_status_line_while_a_note_is_set added
```

<!-- entries appended below this line -->

### Findings — 2026-10-30

`git diff --stat` after the cuts: the phase doc plus the six source files
the script names — `render_ratatui.rs` (84 lines moved into the shared
`render_status_bar`), `render_ratatui_draw.rs` (-66, the three panel
duplicates replaced by `alert_panel`), `tail.rs` (+26, the new ordering
test), `tail_view.rs` (-32), `attach.rs` (-16), `viewer.rs` (+36, the
shared `enter_alt_screen`/`resize_events` entries). 128 insertions,
146 deletions. Nothing outside those seven files.

### Update — 2026-10-30 17:00 (end-to-end verification)

Ran the phase doc's End-to-end block verbatim; its output, unedited:

```
status_bars=1 alert_shells=0 alt_entries=0 order_test=1
allowlist=unchanged
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.98s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 22.14s
fmt_exit=0
passed=2610 failed=0
```

All six acceptance criteria read off that output: `status_bars=1
alert_shells=0 alt_entries=0 order_test=1`; `passed=2610 failed=0`;
hygiene `ok. 16 passed` with `allowlist=unchanged`; bug tracker
`ok. 6 passed`; clippy clean; `fmt_exit=0`.

### Update — 2026-10-30 17:01 (complete, server-authored)

**Summary:** **Summary**

Phase 12b executed entirely through the authorized script. Task 1 ran first, before any file read: `python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-12b-renderer-and-viewers.py && cargo fmt --all` exited 0, printing its three lines (ending `render_tail_keeps_the_log_status_line_while_a_note_is_set added`), and touched exactly the six source files the spec names. The started entry, the `git diff --stat` findings entry, and the end-to-end entry are all in the phase doc's Update Log; the end-to-end entry quotes the block's output verbatim, including the `Finished \`dev\` profile` line clippy prints. Committed as `refactor:` (7e3d8d22) plus the start-of-phase README status flip (c5e6f1da); working tree clean. Status left at `in-progress` in both the phase doc and the README row — no `(complete)` entry written, per the contract.

**Notes for review**

- End-to-end block output, unedited: `status_bars=1 alert_shells=0 alt_entries=0 order_test=1`, `allowlist=unchanged`, hygiene `ok. 16 passed`, bug tracker `ok. 6 passed`, clippy `Finished dev profile` (clean, `-D warnings`), `fmt_exit=0`, `passed=2610 failed=0` — matching every expected value, including the exact suite count 2610 (2609 + the one new test, none removed).
- Acceptance criterion 4 verified mechanically: `grep -c '^### Findings — '` on the phase doc prints `1`, and the findings entry reports `git diff --stat` honestly (doc + six source files, 128 insertions / 146 deletions at that point; final commit is 157 insertions after the doc entries accumulated).
- No deviations from the spec: the script owned all source edits, nothing was hand-edited, the script was not opened or modified, and #28 remains rejected as the doc directs. `run_transcript_viewer` and `daemoneye top` were left untouched per the out-of-scope list.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.4.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 19.21s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.18s


TEST
ok
test production_files_fit_the_cap ... ok
test test_files_fit_the_cap ... ok
test functions_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.27s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
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
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_installs_no_tmux_hooks ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-7593688ba66ac3a2)
kill: (667912): No such process
kill: (-668637): No such process
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
- `docs/dev/milestones/M32-findings-fixes/phase-12b-renderer-and-viewers.md` — +42 -1
- `src/cli/attach.rs` — +2 -14
- `src/cli/render_ratatui.rs` — +42 -42
- `src/cli/render_ratatui_draw.rs` — +5 -61
- `src/cli/tail.rs` — +26 -0
- `src/cli/tail_view.rs` — +4 -28
- `src/cli/viewer.rs` — +36 -0

**Commit:** c5e6f1dad2a638b6482b86dbc29be33c50ea5b32

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
