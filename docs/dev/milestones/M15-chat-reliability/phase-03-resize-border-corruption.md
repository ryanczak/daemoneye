# Phase 03: width-flip ghost borders — width-change-aware clear band in reanchor

**Milestone:** M15 — Chat Reliability & Dialog UX
**Status:** review
**Depends on:** none
**Estimated diff:** ~120 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Switching or resizing tmux windows leaves corrupted border rows (`┌…` wider
than the pane, no closing corner) in chat history. The generator was caught
on tape at a previous milestone's close: a transient pane-width change makes
tmux rewrap the live-region rows (the 6-row inline viewport) into scrollback
as permanent ghosts. The documented fix shape — implemented here — is a
**width-change-aware clear band** in `reanchor`: track the last-drawn width,
and when the width changed, extend the wipe to cover the rows the old live
region occupies after rewrap, which are guaranteed non-history.

## Architecture references

Read before starting:

- `src/cli/render_ratatui.rs:129–280` — `VIEWPORT_ROWS`, `repin_rows`,
  `RatatuiRenderer` fields, and `reanchor` — the entire surface of this
  phase.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

**The mechanism (live-verified previously, trace on record):** `reanchor`
(`render_ratatui.rs:235`) fires on focus-gain and SIGWINCH
(`stream.rs:230`). It wipes from `clear_from` down and re-pins the viewport:

```rust
let old_top = self.terminal.get_frame().area().y;
let content_end = self.origin_row.saturating_add(self.inserted_rows);
let (clear_from, park) = repin_rows(old_top, content_end, size.height);
```

with the pure helper (`render_ratatui.rs:164`):

```rust
fn repin_rows(old_top: u16, content_end: u16, height: u16) -> (u16, u16) {
    let park = height.saturating_sub(VIEWPORT_ROWS);
    (old_top.min(content_end).min(park), park)
}
```

This wipe is **width-blind**. When the pane width changes (a window switch
transiently rearranges layouts — a taped trace shows `w=255` on a 127-col
pane with no user zoom), tmux rewraps the old live region's rows at the new
width. Rewrapped, those `VIEWPORT_ROWS` old-width rows occupy
`ceil(VIEWPORT_ROWS × old_w / new_w)` rows above the bottom of the screen —
**below real committed content, guaranteed non-history** — and any of them
the wipe does not cover survive, then scroll up into history as permanent
ghost borders. That is the corruption the user sees.

The renderer does not currently remember the width it last drew at —
`RatatuiRenderer` (`render_ratatui.rs:177–187`) has only `terminal`,
`start_time`, `palette`, `origin_row`, `inserted_rows`.

**The trace line** (`render_ratatui.rs:253–257`) currently logs:

```rust
"reanchor old_top={old_top} content_end={content_end} park={park} w={} h={}",
```

**Struct construction sites that must gain the new field** (the struct is
built literally in one production and several test constructors):
`render_ratatui.rs:219` (production `new()`), and the test constructors near
lines 788, 1354, 1431, 1493 (each currently ends `origin_row: 0,
inserted_rows: 0,`).

**Existing pure-helper test idiom** (`render_ratatui.rs:2111`):

```rust
#[test]
fn repin_rows_parks_at_viewport_top() {
    assert_eq!(repin_rows(10, 18, 24), (10, 18));
}
```

## Spec

### 1. `ghost_band_rows` pure helper — in `src/cli/render_ratatui.rs`

Add directly below `repin_rows` (do **not** change `repin_rows` itself or
its tests — no signature churn):

```rust
/// Rows the old live region occupies after tmux rewraps it at a new pane
/// width: ceil(VIEWPORT_ROWS × old_w / new_w). These rows sit at the bottom
/// of the screen, below committed content — guaranteed non-history — so a
/// reanchor after a width change may clear them freely. Returns 0 when the
/// width did not change or either width is 0 (no band; the width-blind wipe
/// is already correct). Capped at 4 × VIEWPORT_ROWS so a pathological
/// old_w/new_w ratio cannot wipe most of the screen.
fn ghost_band_rows(old_w: u16, new_w: u16) -> u16 {
    if old_w == new_w || old_w == 0 || new_w == 0 {
        return 0;
    }
    let band = (u32::from(VIEWPORT_ROWS) * u32::from(old_w)).div_ceil(u32::from(new_w));
    (band.min(4 * u32::from(VIEWPORT_ROWS))) as u16
}
```

(`div_ceil` on `u32` is stable — do not hand-roll the ceiling division.)

### 2. Track the last-drawn width — in `src/cli/render_ratatui.rs`

Add a field to `RatatuiRenderer` after `inserted_rows`:

```rust
    /// Pane width at the last construction or reanchor. A change between
    /// reanchors means tmux rewrapped the old live region — see
    /// `ghost_band_rows`.
    last_width: u16,
```

Initialize it in **every** construction site:

- Production `new()` (`render_ratatui.rs:219`): add
  `last_width: terminal.size().map(|s| s.width).unwrap_or(0),` — note this
  line must be placed **before** the `terminal` field moves into the struct
  literal, or bind `let last_width = terminal.size()...` above the `Ok(Self
  {...})` and use the binding.
- Each test constructor near lines 788, 1354, 1431, 1493: add
  `last_width: 0,` alongside the existing `origin_row: 0, inserted_rows: 0,`.

### 3. Width-aware clear band in `reanchor` — in `src/cli/render_ratatui.rs`

In `reanchor` (`render_ratatui.rs:235`), after the existing `repin_rows`
call:

```rust
let (clear_from, park) = repin_rows(old_top, content_end, size.height);
let band = ghost_band_rows(self.last_width, size.width);
let clear_from = clear_from.min(size.height.saturating_sub(band));
```

(When `band == 0`, `size.height.saturating_sub(0)` is the full height and
the `min` is a no-op — the width-unchanged path is byte-identical in
behavior to today.)

At the **end** of `reanchor` (after the Terminal rebuild), record the new
width: `self.last_width = size.width;` — unconditionally, including on the
early-return error paths *after* `size` is known? No: keep it simple —
update it once, immediately after the `let Ok(size) = ... else { return; }`
binding **and after** `band` has been computed from the old value. Concretely:
compute `band` from `self.last_width`, then `self.last_width = size.width;`,
then use `band` in the `min`.

### 4. Extend the trace line — in `src/cli/render_ratatui.rs`

Change the trace `writeln!` (`render_ratatui.rs:253–257`) to include the old
width and the band, keeping every existing field name unchanged (the live
verification at review greps these):

```rust
"reanchor old_top={old_top} content_end={content_end} park={park} w={} h={} old_w={} band={band} clear_from={clear_from}",
size.width, size.height, old_w
```

where `old_w` is the pre-update `self.last_width` captured into a local
before it is overwritten. (Order the statements so the trace sees the same
`band`/`clear_from` values `reanchor` actually uses.)

### 5. Unit tests — in the `mod tests` of `src/cli/render_ratatui.rs`

Alongside the existing `repin_rows_*` tests (`render_ratatui.rs:2111`),
write the tests named in § Test plan.

### 6. Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output
into a new Update Log entry titled
`### Update — <date> (end-to-end verification)`.

## Acceptance criteria

- [ ] `ghost_band_rows(w, w) == 0` for any `w` — an unchanged width leaves
      `reanchor`'s clear row exactly as today.
- [ ] `ghost_band_rows(254, 127) == 12` and `ghost_band_rows(255, 127) == 13`
      (ceiling division, VIEWPORT_ROWS = 6).
- [ ] `ghost_band_rows(127, 255) == 3` — a widening produces a band smaller
      than `VIEWPORT_ROWS`, so `clear_from.min(h − 3)` never rises above the
      park row (the min keeps the lower of the two).
- [ ] `ghost_band_rows(0, 127) == 0` and `ghost_band_rows(127, 0) == 0`.
- [ ] `ghost_band_rows(u16::MAX, 1) == 24` (the 4 × VIEWPORT_ROWS cap).
- [ ] The trace line contains `old_w=` and `band=` and `clear_from=` fields.
- [ ] `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
      and `cargo test` all pass (including the untouched `repin_rows_*`
      tests).

## Test plan

All in `mod tests` of `src/cli/render_ratatui.rs`, following the
`repin_rows_*` naming idiom:

- `ghost_band_rows_zero_when_width_unchanged` — `(127, 127) == 0`,
  `(255, 255) == 0`.
- `ghost_band_rows_narrowing_ceils` — `(254, 127) == 12`,
  `(255, 127) == 13`.
- `ghost_band_rows_widening_small_band` — `(127, 255) == 3`; additionally
  assert the composition: with `h = 61`, `park = 55`,
  `park.min(61u16.saturating_sub(3)) == 55` (the band never raises the
  clear row above the park on a widening).
- `ghost_band_rows_zero_width_guard` — `(0, 127) == 0`, `(127, 0) == 0`.
- `ghost_band_rows_capped` — `(u16::MAX, 1) == 24`.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
cargo fmt --check 2>&1 | tail -5; echo "exit=${PIPESTATUS[0]}"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5; echo "exit=${PIPESTATUS[0]}"
cargo test 2>&1 | tail -10; echo "exit=${PIPESTATUS[0]}"
cargo test --lib ghost_band 2>&1 | tail -12; echo "exit=${PIPESTATUS[0]}"
cargo test --lib repin_rows 2>&1 | tail -10; echo "exit=${PIPESTATUS[0]}"
grep -n 'old_w={} band={band}' src/cli/render_ratatui.rs; echo "exit=$?"
```

The final grep must print the trace line (exit=0) — it proves the trace
carries the new fields for the live check.

Live verification (scripted window-switch/resize sequence under
`DAEMONEYE_REANCHOR_TRACE=1`, confirming band-extended clears fire on width
flips and no fresh ghost borders land in scrollback) is performed
**architect-side at review** — it needs an attached tmux client and a
running chat session, outside this phase's authorizations.

## Authorizations

- Edit `src/cli/render_ratatui.rs` only.
- Run the gate commands. No daemon restart, no tmux interaction, no files
  outside the repo.

## Out of scope

- Cleaning ghosts already planted in existing scrollback — nothing app-side
  can rewrite tmux history after the fact.
- Changing `repin_rows`'s signature or its existing tests.
- Reanchor trigger plumbing (`stream.rs` focus/SIGWINCH handling) — the
  triggers are live-verified and unchanged.
- Width flips that occur without a subsequent reanchor event — no event, no
  wipe; accepted residual, assessed at the live check.
- The other M15 issues (dialogs; phases 04–05).

## Update Log

### Update — 2026-08-14 (created)

Phase drafted by the architect. Implements the fix shape documented at a
prior milestone's close for the taped `w=255` width-flip ghost generator:
width-change-aware clear band in `reanchor`, tracked via a new `last_width`
field, pure `ghost_band_rows` helper, trace extended for the live check.
Status: todo.

### Update — 2026-08-14 20:38 (started)

Executor started phase-03. Status flipped to in-progress; milestone README
phase-table row updated. Implementing the width-change-aware clear band in
`reanchor` per spec.

### Update — 2026-08-14 (end-to-end verification)

Ran the § End-to-end verification block verbatim:

```
$ cargo fmt --check 2>&1 | tail -5; echo "exit=${PIPESTATUS[0]}"
exit=0

$ cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5; echo "exit=${PIPESTATUS[0]}"
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s
exit=0

$ cargo test 2>&1 | tail -10; echo "exit=${PIPESTATUS[0]}"
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s

   Doc-tests daemoneye

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

exit=0

$ cargo test --lib ghost_band 2>&1 | tail -12; echo "exit=${PIPESTATUS[0]}"
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)

running 5 tests
test cli::render_ratatui::tests::ghost_band_rows_capped ... ok
test cli::render_ratatui::tests::ghost_band_rows_narrowing_ceils ... ok
test cli::render_ratatui::tests::ghost_band_rows_widening_small_band ... ok
test cli::render_ratatui::tests::ghost_band_rows_zero_width_guard ... ok
test cli::render_ratatui::tests::ghost_band_rows_zero_when_width_unchanged ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 1251 filtered out; finished in 0.00s

exit=0

$ cargo test --lib repin_rows 2>&1 | tail -10; echo "exit=${PIPESTATUS[0]}"

running 5 tests
test cli::render_ratatui::tests::repin_rows_clears_debris_between_content_and_park ... ok
test cli::render_ratatui::tests::repin_rows_clears_from_old_top_when_higher ... ok
test cli::render_ratatui::tests::repin_rows_content_past_park_clamps ... ok
test cli::render_ratatui::tests::repin_rows_parks_at_viewport_top ... ok
test cli::render_ratatui::tests::repin_rows_short_terminal_saturates ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 1251 filtered out; finished in 0.00s

exit=0

$ grep -n 'old_w={} band={band}' src/cli/render_ratatui.rs; echo "exit=$?"
280:                "reanchor old_top={old_top} content_end={content_end} park={park} w={} h={} old_w={} band={band} clear_from={clear_from}",
exit=0
```

Full `cargo test` lib suite: 1256 passed; 0 failed (includes the 5 new
`ghost_band_rows_*` tests and the untouched `repin_rows_*` tests).

### Update — ts=1786740768877 (complete, server-authored)

**Summary:** All spec tasks are complete and committed.

**Summary**

Implemented the width-change-aware clear band in `reanchor` (`src/cli/render_ratatui.rs`): added the pure `ghost_band_rows(old_w, new_w)` helper (ceil(VIEWPORT_ROWS × old_w / new_w), capped at 4×VIEWPORT_ROWS, 0 on unchanged/zero widths) directly below `repin_rows`; added a `last_width` field to `RatatuiRenderer` initialized in `new()` (via a `let` bound before the struct literal) and in all 8 test constructors; in `reanchor`, `band` is computed from the pre-update `self.last_width` (captured as `old_w`), `last_width` is then updated, and `clear_from` is extended with `clear_from.min(size.height.saturating_sub(band))` — a no-op when `band == 0`, so the width-unchanged path is behavior-identical to before. The trace line now carries `old_w=`, `band=`, and `clear_from=` alongside all existing fields. Added the five `ghost_band_rows_*` unit tests per the test plan; `repin_rows` and its tests are untouched.

**Notes for review**

- All acceptance criteria verified: `(254,127)==12`, `(255,127)==13`, `(127,255)==3` with the park-composition check, zero guards, and the `(u16::MAX,1)==24` cap all pass; full lib suite 1256 passed / 0 failed.
- End-to-end verification block ran verbatim (all gates exit=0; grep prints the trace line at line 280) and the captured output is in a dedicated `(end-to-end verification)` Update Log entry.
- No deviations from the spec; the only adaptation was binding `last_width` in a local before the struct literal in `new()` as the spec anticipated.
- Committed as `46972e3`; working tree clean; phase doc left at `in-progress` for the server's completion tail.

**Executor:** Qwen/Qwen3.8-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
cludes_other_windows ... ok
test tmux::bounded_output_tests::bounded_output_times_out_and_kills_the_child ... ok

test result: ok. 1256 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.98s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 8 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_tools_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_tables_match_the_code ... ok
test claude_md_tools_table_matches_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 32 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
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

test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test default_server_unchanged ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)
     Running unittests src/main.rs (target/debug/deps/daemoneye-e700f2084319867a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9b22636ef5c08466)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c00c74ef4ffe9c11)
     Running tests/integration.rs (target/debug/deps/integration-6230826c10f36795)
     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M15-chat-reliability/README.md` — +1 -1
- `docs/dev/milestones/M15-chat-reliability/phase-03-resize-border-corruption.md` — +67 -0
- `src/cli/render_ratatui.rs` — +68 -2

**Commit:** 46972e3514fa3926c1f601e83becec9fb765b5f9

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
