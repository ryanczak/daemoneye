# Phase 02c — grants-segment

**Milestone:** M37 — Alerts out of history, onto the status bar and `top`
**Status:** review
**Drafted:** 2026-11-14
**Depends on:** phase 02a (the `StatusModel` and the glyph dialects)
**Estimated diff:** ~300 lines, about half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

The bar's grants segment speaks the same grammar as every other slot
(decision 3, amended 2026-11-14): a lead glyph coloured by the riskiest
grant, a count of the non-default grants, per-kind glyphs with counts when
room, hidden entirely in the default state, flush right. The segment is
built from a summary struct, not a pre-formatted string, so it speaks the
chosen glyph dialect and degrades by tier. Today it is a ~55-column
sentence (`⚡approvals: all, scripts: 2 · Ctrl+C revokes`) that ignores the
dialect and drops all at once.

## Spec

Numbered tasks in execution order; the executor's task list is seeded from
this section, so mark each `active` when you start it and `done` when its
`cargo build` is green.

1. **The summary struct** — in `src/cli/commands/approval.rs`, a pure
   `GrantSummary` (`Copy`, `Default`, `PartialEq`) with: `cmds:
   CmdGrant` (`Gated` / `Auto` / `Sudo` / `All`), one `Scope` (`None` /
   `Named(u32)` / `All`) each for `scripts`, `runbooks`, `files`, `agents`,
   `schedules: bool`, `deletes: bool`, and `hosts: u32` (distinct hosts in
   `hosts_regular ∪ hosts_sudo`). `SessionApproval::summary()` builds it;
   `hint()` is deleted along with its tests, replaced by
   `summary_counts_each_kind`. The three types live in
   `src/cli/status_bar.rs` (or a sibling the bar re-exports), and
   `approval.rs` imports them: the bar must not depend on
   `crate::cli::commands`.
2. **The model** — `StatusModel.approval_hint: String` becomes
   `StatusModel.grants: GrantSummary`. Every `sb.approval_hint =
   approval.hint()` site (`chat_loop.rs`, `stream_ask.rs`,
   `stream_prompts.rs`, `slash*.rs`, `agents.rs`; `cargo build` names them)
   becomes `sb.grants = approval.summary()`.
3. **The glyphs** — `GlyphSet` gains one field per grant kind, in all three
   dialects, none containing U+FE0F / U+FE0E; `grants` stays the lead:

   | field | emoji | symbols | ascii |
   |---|---|---|---|
   | `grants` (lead, exists) | ⚡ | ⚡ | `A` |
   | `grant_gated` | 🔒 | ⊘ | `!` |
   | `grant_sudo` | 🔑 | # | `#` |
   | `grant_scripts` | 📜 | § | `s` |
   | `grant_runbooks` | 📖 | ¶ | `r` |
   | `grant_files` | 📄 | ▤ | `f` |
   | `grant_schedules` | 📅 | ◷ | `c` |
   | `grant_agents` | 🤖 | ⚉ | `g` |
   | `grant_deletes` | 🗑 | ⌫ | `d` |
   | `grant_hosts` | 🌐 | ⌂ | `h` |
   | `grant_all` (the "every one" mark after a kind) | ∗ | ∗ | `*` |

   `glyph_sample` includes the new fields so `doctor` shows them.
4. **The segment** — `hint_part` becomes `grants_parts(model, g, palette)`.
   The *default state* (`cmds == Auto` and every other field at its
   `Default`) returns nothing: the segment is hidden like a zero counter.
   Otherwise:
   - **Core** (the current `RANK_HINT`): the lead glyph, then the count of
     active kinds — `Gated`, `Sudo`/`All` cmds, each non-`None` scope,
     `schedules`, `deletes`, and `hosts > 0` each count one. The lead glyph
     is **red** when `cmds == Sudo | All`, `deletes`, or any scope is `All`;
     **amber** when the only grants are `Named(_)` scopes or `hosts`;
     **green** when `cmds == Gated` and nothing else is active (`Palette`
     has `red()` and `yellow()` today; add `green()` beside them in the same
     shape). Colour sits on the glyph only, as the other slots do.
   - **Detail** (tier 3, a new `RANK_GRANT_DETAIL` that drops with the
     velocities, before `RANK_ALERT_NAME`): one part per active kind in the
     table's order, each the kind glyph followed by its count, `grant_all`
     for `All`, or nothing for a boolean kind. `Sudo` shows `grant_sudo`;
     `All` cmds shows `grant_sudo` too (regular is the default and carries
     no glyph). Parts are separated by a single space inside the segment.
   - The segment stays flush right, as `join_slots` places it today. No
     line ever contains `Ctrl+C`; the revoke hint stays in `/approvals`.
5. **Tests** — in `src/cli/status_bar_tests.rs` (all pure, `now` passed
   in): `grants_hidden_in_the_default_state`,
   `grants_lead_colour_follows_the_riskiest_grant` (green for gated-only,
   amber for one named script, red for `files: All`, red for `deletes`),
   `grants_count_active_kinds_and_show_detail_when_room` (a summary with
   sudo, two scripts, files all and deletes shows the count 4 and, at 160
   columns, each of the four kind glyphs in order),
   `grants_detail_drops_before_the_core` (narrow until the detail is gone
   and the core still shows, then until the core goes after the tier-2
   counters), `grants_speak_each_dialect` (the ascii line contains no `⚡`
   and shows `A`), `no_status_line_mentions_ctrl_c`. Update the existing
   `full_model()` fixture and the tier-order test to build a `GrantSummary`
   instead of a string. In `approval.rs`: `summary_counts_each_kind`.
6. **Docs** — the README's slot table row for `grants` and its 120/80
   examples; CLAUDE.md's `src/cli/` row if it names the hint.
7. **Capture the mutation check and the end-to-end evidence** — run the
   mutation check named in § Acceptance criteria and the block in
   § End-to-end verification verbatim, and paste both outputs into one
   Update Log entry.

## Constraints

- Pin behaviour, not pixels: what a line contains or omits at a width, and
  its measured width, never a full string of the bar.
- Tests call the production seam (`status_bar_line`, `summary()`); a copy
  of the colour or count rule inside a test is a fake test.
- The `/approvals` command's body and the approval prompt panel are
  unchanged.
- No glyph in the table collides with an existing `GlyphSet` field's glyph
  in the same dialect; `glyph_sets_contain_no_variation_selectors` must
  still pass.

## Notes for executor

**Symbol map.** Every one of these exists today:
- `src/cli/commands/approval.rs`: `SessionApproval`, `hint`,
  `host_approval_lines`, `approvals_list_body`.
- `src/cli/status_bar.rs`: `StatusModel` (`approval_hint`), `hint_part`,
  `alert_parts` (the shape to copy: a lead glyph with colour, a count, detail
  parts at a lower rank), `glyph_sample`, `gray`.
- `src/cli/status_bar_fit.rs`: the `RANK_*` constants (`RANK_HINT` = 12,
  the tier-3 ranks 1–7), `line_fits`, `join_slots`.
- `src/cli/status_glyphs.rs`: `GlyphSet`, `EMOJI`, `SYMBOLS`, `ASCII`.
- `src/cli/status_bar_tests.rs`: `full_model`, `line_at`, `line_width`,
  `status_line_drops_tier_three_before_tier_two_before_the_hint`.
- `src/cli/palette.rs`: `Palette` (`red()`, `yellow()`; the context slot
  uses both).

**Edit order.** Each step ends at a green `cargo build`:
1. **First write:** the `GrantSummary` types and `summary()`; keep `hint()`
   for this step so nothing else breaks.
2. The glyph fields in all three dialects and `glyph_sample`.
3. `StatusModel.grants`, `grants_parts`, the new rank; switch every call
   site; delete `hint()` and its tests.
4. The new tests and fixture update.
5. Docs.
6. Spec task 7: the mutation check and the end-to-end block, pasted.

**Your first edit lands within your first 15 turns.** Never run `tmux`.
Never repeat a read whose output you already have; when unsure of a call
site, let `cargo build` name it.

## Test plan

The seven tests named in Spec task 5, plus `summary_counts_each_kind`.

## Acceptance criteria

- [ ] Every test in the test plan passes.
- [ ] `grep -c 'approval_hint' src/cli/status_bar.rs` prints 0 (today 4).
- [ ] `grep -c 'fn hint' src/cli/commands/approval.rs` prints 0 (today 5:
      `hint` and its four tests).
- [ ] `grep -c 'Ctrl+C revokes' src/cli/commands/approval.rs` prints 0
      (today 1).
- [ ] `grep -c 'grant_sudo' src/cli/status_glyphs.rs` prints 4 (the field
      and three dialects; today 0).
- [ ] Mutation check, pasted with its `test result:` line: make the lead
      colour ignore `deletes`, confirm
      `grants_lead_colour_follows_the_riskiest_grant` fails, then revert.
- [ ] fmt, build (0 warnings), clippy and `cargo test` are green. Hygiene,
      `doc_truth` and `bug_tracker` are ok.

## End-to-end verification

Run mechanically, with output redirected to a file and pasted:

```sh
cargo test --lib grants 2>&1 | grep -E '^test result'
cargo test --lib summary_counts_each_kind 2>&1 | grep -E '^test |^test result'
grep -c 'approval_hint' src/cli/status_bar.rs | sed 's/^/approval_hint=/'
grep -c 'Ctrl+C revokes' src/cli/commands/approval.rs | sed 's/^/ctrl_c=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

## Authorizations

- Edit the files in the symbol map, every file `cargo build` names for the
  `approval_hint` → `grants` switch, `README.md` and `CLAUDE.md`.
- Add `RANK_GRANT_DETAIL` to `status_bar_fit.rs` and renumber the tier-3
  ranks if needed (they are relative).

## Out of scope

- `/approvals` output, the approval prompt panel, and how grants are made
  or revoked.
- The counters (phase 02b) and `top` (phase 03).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-14 05:47 (started)

Executor Claude (direct) starting phase 02c. Plan: the `GrantSummary` types
first (keeping `hint()` until the switch compiles), then the glyph fields,
then the `StatusModel.grants` switch and `grants_parts`, then the tests and
docs.

### Update — 2026-11-14 14:40 (escalation)

**Chosen lever:** resume
**Rationale:** hard_fail at turn 601 ("identical bash call repeated 6
times") with tasks 1–6 done and verified on disk by the architect: fmt,
build (0 warnings) and clippy are clean, the grants tests and
`summary_counts_each_kind` pass, and the acceptance greps hold. The one wall
is `production_files_fit_the_cap`: `src/cli/status_bar.rs` has 880
production lines (cap 800), and the executor looped re-measuring it. That's a
single mechanical move into a sibling module the spec already allowed, so a
resume beats a re-dispatch. `grant_sudo` counts 5 in `status_glyphs.rs`, not
4: `glyph_sample` uses it too, which is a spec miscount and not a defect.

### Update — 2026-11-14 15:45 (resume, tasks 6–7)

Resumed for the one remaining failure, `production_files_fit_the_cap`
(`src/cli/status_bar.rs` 880 lines, cap 800). Moved `CmdGrant`, `Scope`,
`GrantSummary` (with its impls) and `grants_parts` / `grants_lead_colour` /
`grants_active_kinds` verbatim into a new `src/cli/status_grants.rs`, declared
as `#[path = "status_grants.rs"] mod grants;` beside the existing `mod fit`,
with `pub use grants::{CmdGrant, GrantSummary, Scope, grants_active_kinds,
grants_lead_colour};` in `status_bar.rs` so `approval.rs` and the tests keep
their import paths. `status_bar.rs` is now under the cap; hygiene reports
`test result: ok. 16 passed`. CLAUDE.md's status-bar row names the new file;
`doc_truth` passes. Note: `grants_lead_colour` lives in `status_grants.rs`,
so the mutation check below edits that file.

### Update — 2026-11-14 15:45 (end-to-end verification)

Mutation check — made `grants_lead_colour` ignore `deletes`
(removed `|| s.deletes` from the `red` arm), reverted after:

```
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2856 filtered out; finished in 0.00s
```

End-to-end block, verbatim:

```
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 2849 filtered out; finished in 0.00s
test cli::commands::approval::tests::summary_counts_each_kind ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2856 filtered out; finished in 0.00s
approval_hint=0
ctrl_c=0
build_warnings=0
passed=2947 failed=0
```

Also green: `cargo build` 0 warnings, `cargo clippy --all-targets
--all-features -- -D warnings` clean, `cargo fmt --all` clean,
`--test hygiene` ok (16 passed, `production_files_fit_the_cap` included) and
`--test doc_truth` ok (19 passed). `grant_sudo` counts 5 in
`status_glyphs.rs` — the field, three dialects and `glyph_sample` — matching
the architect's escalation note.

### Update — 2026-11-14 15:58 (complete, server-authored)

**Summary:** **Summary + Notes for review**

Resumed phase 02c and cleared its single remaining failure: `production_files_fit_the_cap` had `src/cli/status_bar.rs` at 880 lines (cap 800). I moved `CmdGrant`, `Scope`, `GrantSummary` (with impls) and `grants_parts`/`grants_lead_colour`/`grants_active_kinds` verbatim into a new `src/cli/status_grants.rs`, declared as `#[path = "status_grants.rs"] mod grants;` beside the existing `mod fit`, with `pub use` re-exports in `status_bar.rs` so `approval.rs` and the test files compile unchanged — no behavior changed. Hygiene now reports `test result: ok. 16 passed`, `doc_truth` ok (19 passed), and I appended `status_grants.rs` to CLAUDE.md's status-bar row as the resume guidance specified. Spec task 7 is done: the mutation check (removing `|| s.deletes` from the lead-colour's red arm) produced `test result: FAILED. 0 passed; 1 failed; ... 2856 filtered out` and was reverted; the end-to-end block, pasted into the Update Log, reads `approval_hint=0`, `ctrl_c=0`, `build_warnings=0`, `passed=2947 failed=0`, plus the grants tests (8 passed) and `summary_counts_each_kind` (ok). Gates green: fmt clean, build 0 warnings, clippy `--all-targets --all-features -D warnings` clean, full `cargo test` failed=0; status left at `in-progress` with no hand-written `(complete)` entry, one commit `f4a93640`, working tree clean. Note for review: `grant_sudo` counts 5 in `status_glyphs.rs` (field + three dialects + `glyph_sample`), matching the escalation note's spec-miscount finding, and `grants_lead_colour` lives in `status_grants.rs`, so the mutation check edits that file.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
cy_runbook_precedence_over_agent ... ok
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
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test alert_list_request_returns_the_snapshot ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test attach_sends_session_counters_after_ok ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test presence_attach_sends_alert_state_when_alerts_are_active ... ok
test presence_attach_sends_ok_before_alert_state ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_at_threshold_is_injected ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_below_threshold_is_not_injected ... ok
test webhook_alert_below_threshold_is_recorded_active ... ok
test webhook_alert_no_severity_is_below_threshold ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok

test result: ok. 40 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-e4eaed9c20b7817e)
kill: (3037265): No such process
kill: (-3037980): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-84596a3046ce91b6)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-61684e0b51107e5b)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c69cc98b7b8dfe7a)
     Running tests/hygiene.rs (target/debug/deps/hygiene-1cb80d9fbff8faf7)
     Running tests/integration.rs (target/debug/deps/integration-9d0880a1c76a698b)
     Running tests/isolation.rs (target/debug/deps/isolation-882adad6d3253c89)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-bad9d4ccd89b2daf)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-3f0be99859fd1842)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +1 -1
- `docs/dev/milestones/M37-alerts-status-bar/phase-02c-grants-segment.md` — +55 -0
- `src/cli/status_bar.rs` — +7 -174
- `src/cli/status_grants.rs` — +182 -0

**Commit:** f4a93640f3d8e11a5922dd2b470e2ef865d6269f

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
