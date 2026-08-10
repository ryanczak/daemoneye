# Phase 07: Content-extent clear — wipe live-region debris the repin misses

**Milestone:** M13 — Chat UX Polish
**Status:** review
**Depends on:** phase-06
**Estimated diff:** ~180 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Phase-06's bottom repin works (live-checked: the input dialog pins to the
bottom), but its clear range — `min(old_viewport_top, park)` downward — is
too narrow: on a same-size window switch, fragments of *earlier* live-region
generations survive in the gap between the end of committed history and the
repinned viewport (screenshot evidence, 2026-08-10: border rules and orphaned
`│` cells from at least two stale generations). This phase makes the renderer
track where real content actually ends — every committed row passes through
`insert_before` — and clears from there, plus an env-gated trace so the next
live check produces numbers.

## Architecture references

Read before starting:

- `docs/dev/NEXT.md` § "OPEN FINDING 2 (2026-08-10)" — the evidence and the
  design this phase implements.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

(Verified 2026-08-10 against the post-phase-06 tree, all in
`src/cli/render_ratatui.rs` unless noted.)

- `repin_rows` (`:162-165`) is two-arg:

  ```rust
  fn repin_rows(old_top: u16, height: u16) -> (u16, u16) {
      let park = height.saturating_sub(VIEWPORT_ROWS);
      (old_top.min(park), park)
  }
  ```

  Its three tests (`:2069-2087`): `repin_rows_parks_at_viewport_top`,
  `repin_rows_clears_from_old_top_when_higher`,
  `repin_rows_short_terminal_saturates`.
- `reanchor` (`:224-`) computes
  `let (clear_from, park) = repin_rows(old_top, size.height);` — when the
  old viewport already sits at the bottom, `clear_from == park` and only the
  bottom `VIEWPORT_ROWS` rows are wiped. Debris above survives; tmux
  restores it faithfully on every switch because to tmux it is real grid
  content.
- **Every committed row goes through exactly three `insert_before` sites**
  (banner, streamed lines, panels — all `commit*` methods):
  `commit` (`:262`), `commit_styled` (`:282`),
  `commit_panel_labeled` (`:489`). Each passes a local `row_count` to
  `insert_before(row_count as u16, ...)`.
- The struct has fields `terminal`, `start_time`, `palette`. Adding fields
  ripples into `new()` (which builds `Ok(Self { terminal, start_time,
  palette: ... })`) and **seven** test struct-literals:
  `make_test_renderer` (`:754`) and inline constructions at `:1320`,
  `:1393`, `:1453`, `:1523`, `:1800`, `:1854`. The compiler enumerates any
  missed site; update all with the zero-initialized new fields.
- `new()` builds the `Terminal` then returns; the initial viewport top row is
  readable post-construction as `terminal.get_frame().area().y` (the same
  accessor `reanchor` already uses — `get_frame` needs `&mut`, so bind
  `let mut terminal` in `new()`).
- Grep baselines: `origin_row` 0 hits; `inserted_rows` 0 hits;
  `fn repin_rows(old_top: u16, height: u16)` exactly 1 (becomes 0).
- Semantics note (pin this): `content_end = origin_row + inserted_rows` is
  the row just past the last content-bearing row **until the screen fills**;
  once a session has inserted past `park`, the clamp in `repin_rows` makes
  `clear_from` degrade to exactly phase-06's behavior — correct there,
  because everything above the viewport is then genuinely scrolled history.
  The counter is monotone and never reset, including across `reanchor`
  rebuilds (a rebuild does not move committed content).

## Spec

### Task 1 — Track content extent

1. Add two fields to the struct:

   ```rust
   /// Viewport top row at construction — where committed content starts.
   origin_row: u16,
   /// Total rows ever passed to `insert_before` (saturating). origin_row +
   /// inserted_rows = the row just past real content, until the screen
   /// fills and the clamp in `repin_rows` takes over.
   inserted_rows: u16,
   ```

2. In `new()`, bind the terminal mutably, capture the origin, and initialize:

   ```rust
   let mut terminal = Terminal::with_options(/* unchanged */)?;
   let origin_row = terminal.get_frame().area().y;
   Ok(Self {
       terminal,
       start_time,
       palette: crate::cli::palette::Palette::from_env(),
       origin_row,
       inserted_rows: 0,
   })
   ```

3. At **each of the three** `insert_before` sites (`commit` `:262`,
   `commit_styled` `:282`, `commit_panel_labeled` `:489`), immediately
   before the `insert_before` call, add — using that site's existing
   `row_count` local, so the counter and the insert can never disagree:

   ```rust
   self.inserted_rows = self.inserted_rows.saturating_add(row_count as u16);
   ```

4. Update the seven test struct-literals (`:754`, `:1320`, `:1393`, `:1453`,
   `:1523`, `:1800`, `:1854`) with `origin_row: 0, inserted_rows: 0,`.

### Task 2 — Three-arg `repin_rows`

Replace the two-arg form — pinned exactly (the min-chain is mutation M1's
target):

```rust
/// Rows for a bottom repin: (clear_from, cursor_park).
///
/// `cursor_park` is the future viewport TOP (`height − VIEWPORT_ROWS`) —
/// see the phase-06 scroll-trap note for why never the bottom row.
/// `clear_from` starts the wipe at the highest of the safe rows: the old
/// viewport top, the end of real committed content (`content_end`), or the
/// park row — whichever is highest on screen. Clearing from `content_end`
/// is what removes stale live-region debris parked between history and the
/// bottom; the `park` clamp makes a full-scrolled session degrade to the
/// bottom-rows-only clear, which is correct there.
fn repin_rows(old_top: u16, content_end: u16, height: u16) -> (u16, u16) {
    let park = height.saturating_sub(VIEWPORT_ROWS);
    (old_top.min(content_end).min(park), park)
}
```

### Task 3 — `reanchor` uses it, plus the trace hook

In `reanchor`, replace the two-arg call with:

```rust
let old_top = self.terminal.get_frame().area().y;
let content_end = self.origin_row.saturating_add(self.inserted_rows);
let (clear_from, park) = repin_rows(old_top, content_end, size.height);
if std::env::var("DAEMONEYE_REANCHOR_TRACE").is_ok() {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/daemoneye-reanchor.log")
    {
        use std::io::Write as _;
        let _ = writeln!(
            f,
            "reanchor old_top={old_top} content_end={content_end} park={park} w={} h={}",
            size.width, size.height
        );
    }
}
```

The rest of `reanchor` (clear, park, rebuild) is unchanged.

### Task 4 — Tests

Update the three existing `repin_rows_*` tests to the three-arg form and add
the new cases — all named in § Test plan.

### Task 5 — Mutation M1 apply + restore (debris clear)

Apply a `patch` on `src/cli/render_ratatui.rs` changing
`(old_top.min(content_end).min(park), park)` to
`(old_top.min(park), park)`, then:

```sh
echo "== M1 APPLIED ==" >> /tmp/e2e-m13-07.txt
cargo test --lib repin_rows 2>&1 | tail -5 >> /tmp/e2e-m13-07.txt
```

`repin_rows_clears_debris_between_content_and_park` must show **FAILED**. If
it stays green, report a blocker — do not adjust a test to make it fail.
Restore with the inverse `patch`, then:

```sh
echo "== M1 RESTORED ==" >> /tmp/e2e-m13-07.txt
grep -c '(old_top.min(park), park)' src/cli/render_ratatui.rs >> /tmp/e2e-m13-07.txt
cargo test --lib repin_rows 2>&1 | tail -5 >> /tmp/e2e-m13-07.txt
```

The grep count must be `0` and the tests green.

### Task 6 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
append a new Update Log entry headed
`### Update — <date> (end-to-end verification)` whose fenced block is the
contents of `/tmp/e2e-m13-07.txt`, **inserted by command (`cat >>`), never
retyped**. Then run this self-check and paste its literal one-line output as
the entry's last line, outside the fence:

```sh
awk '/^### Update — .*\(end-to-end verification\)/{f=1;next} f && /^### /{exit} f' docs/dev/milestones/M13-chat-ux/phase-07-content-extent-clear.md | awk '/^```$/{n++;next} n==1' > /tmp/pasted-07.txt
diff /tmp/pasted-07.txt /tmp/e2e-m13-07.txt > /dev/null && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

(The extraction scopes to the entry's **first** fence — the server-authored
`(complete)` entry's block must not be swept in.) The run is finished only
when this prints `PASTE MATCH`, and that line must appear in the entry. The
server-authored `(complete)` entry does not satisfy Task 6.

## Acceptance criteria

Progress markers — each **fails against the current tree** (verified at
drafting):

- [ ] `grep -c 'inserted_rows.saturating_add' src/cli/render_ratatui.rs`
      prints `3` — one per `insert_before` site. (Currently: 0.)
- [ ] `grep -c 'fn repin_rows(old_top: u16, content_end: u16, height: u16)'
      src/cli/render_ratatui.rs` prints `1`, and the two-arg form
      `fn repin_rows(old_top: u16, height: u16)` prints `0`.
      (Currently: 0 and 1.)
- [ ] `grep -c 'DAEMONEYE_REANCHOR_TRACE' src/cli/render_ratatui.rs` prints
      `1`. (Currently: 0.)
- [ ] Tests `repin_rows_clears_debris_between_content_and_park`,
      `repin_rows_content_past_park_clamps`, and
      `commit_methods_count_inserted_rows` pass. (Currently: none exist.)

No-regression guards — these **already pass** and must still pass (they are
not evidence of new work):

- [ ] The three existing `repin_rows_*` tests still pass, updated to the
      three-arg call with a `content_end` that preserves each test's
      original scenario (pass `u16::MAX`-free values ≥ the old expectations
      so the new min-term is not the binding one — e.g. `content_end = park`
      or higher-clamped values).
- [ ] The phase-04/05/06 suites still pass (`cursor_*`, `focus_*`,
      `select_stream_*`, `input_box_row_is_stable_across_draw_modes`).
- [ ] Four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.

## Test plan

In `src/cli/render_ratatui.rs` `mod tests`:

- `repin_rows_clears_debris_between_content_and_park` —
  `repin_rows(18, 10, 24)` == `(10, 18)`: old viewport at the bottom, real
  content ends at row 10 → the wipe starts at 10, removing everything in the
  debris gap. (Mutation M1 target — dropping the `content_end` term makes
  this return `(18, 18)`.)
- `repin_rows_content_past_park_clamps` — `repin_rows(10, 30, 24)` ==
  `(10, 18)` and `repin_rows(20, 30, 24)` == `(18, 18)`: a full-scrolled
  session degrades to phase-06 behavior.
- `commit_methods_count_inserted_rows` — on a fresh test renderer
  (`origin_row: 0, inserted_rows: 0`): `commit("a\nb\nc")` →
  `inserted_rows == 3`; then `commit_panel("t", &[one body line], false)` →
  `inserted_rows == 7` (top border + body + bottom border + spacer = 4);
  then `commit_styled(&[two lines])` → `inserted_rows == 9`. Assert after
  each step, not just at the end.
- Updated existing: `repin_rows_parks_at_viewport_top`
  (`repin_rows(10, 18, 24)` == `(10, 18)`),
  `repin_rows_clears_from_old_top_when_higher` (`repin_rows(3, 18, 24)` ==
  `(3, 18)`; `repin_rows(20, 18, 24)` == `(18, 18)`),
  `repin_rows_short_terminal_saturates` (`repin_rows(0, 0, 4)` == `(0, 0)`).

## End-to-end verification

```sh
: > /tmp/e2e-m13-07.txt
echo "== GATES ==" >> /tmp/e2e-m13-07.txt
cargo fmt --all 2>&1 | tail -2 >> /tmp/e2e-m13-07.txt; echo "fmt exit=${PIPESTATUS[0]}" >> /tmp/e2e-m13-07.txt
cargo build 2>&1 | tail -2 >> /tmp/e2e-m13-07.txt; echo "build exit=${PIPESTATUS[0]}" >> /tmp/e2e-m13-07.txt
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2 >> /tmp/e2e-m13-07.txt; echo "clippy exit=${PIPESTATUS[0]}" >> /tmp/e2e-m13-07.txt
cargo test 2>&1 | grep -E '^test result' >> /tmp/e2e-m13-07.txt; echo "test exit=${PIPESTATUS[0]}" >> /tmp/e2e-m13-07.txt
echo "== SURFACES ==" >> /tmp/e2e-m13-07.txt
echo "counter sites: $(grep -c 'inserted_rows.saturating_add' src/cli/render_ratatui.rs)" >> /tmp/e2e-m13-07.txt
echo "trace hooks: $(grep -c 'DAEMONEYE_REANCHOR_TRACE' src/cli/render_ratatui.rs)" >> /tmp/e2e-m13-07.txt
echo "two-arg repin_rows left: $(grep -c 'fn repin_rows(old_top: u16, height: u16)' src/cli/render_ratatui.rs)" >> /tmp/e2e-m13-07.txt
wc -l /tmp/e2e-m13-07.txt >> /tmp/e2e-m13-07.txt
```

(The Task 5 mutation runs append into the same file in task order.)

**The decisive verification is live:** rebuild/reinstall, run with
`DAEMONEYE_REANCHOR_TRACE=1 daemoneye chat`, accumulate some history, switch
tmux windows away and back several times (same-size layout — the screenshot
case). Expect: no border debris between history and the input box, and
`/tmp/daemoneye-reanchor.log` lines whose `content_end` sits between the
history end and `park`. That check happens at the milestone gate.

## Authorizations

- The env-gated trace write to `/tmp/daemoneye-reanchor.log` is explicitly
  authorized — it is a documented diagnostic, dormant unless
  `DAEMONEYE_REANCHOR_TRACE` is set, and not a debug `println!`.

## Out of scope

- Any change to the clear/park/rebuild sequence itself — phase-06's
  mechanism stands; only the *range* input changes.
- Re-rendering scrollback (standing non-goal), `chat.rs`, `stream.rs`,
  `editor.rs`, anything under `src/daemon/`.
- Resetting or persisting the counters — monotone for the process lifetime.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-10 03:15 (started)

**Executor:** claude-code
**Status:** in-progress

Implementing content-extent clear: tracking `origin_row` and `inserted_rows` on the renderer, converting `repin_rows` to three-arg form, updating `reanchor` with the trace hook, and adding tests.

### Update — 2026-08-10 03:15 (end-to-end verification)

```
== GATES ==
fmt exit=0
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.06s
build exit=0
    Checking daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.34s
clippy exit=0
test result: ok. 1234 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.97s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test exit=0
== SURFACES ==
counter sites: 3
trace hooks: 1
two-arg repin_rows left: 0
20 /tmp/e2e-m13-07.txt
== M1 APPLIED ==
    cli::render_ratatui::tests::repin_rows_clears_debris_between_content_and_park

test result: FAILED. 4 passed; 1 failed; 0 ignored; 0 measured; 1229 filtered out; finished in 0.00s

error: test failed, to rerun pass `--lib`
== M1 RESTORED ==
0
test cli::render_ratatui::tests::repin_rows_short_terminal_saturates ... ok
test cli::render_ratatui::tests::repin_rows_parks_at_viewport_top ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 1229 filtered out; finished in 0.00s
```

PASTE MATCH

### Update — ts=1786332643975 (complete, server-authored)

**Summary:** ## Summary + Notes for review

Implemented content-extent clear in `render_ratatui.rs`: added `origin_row` and `inserted_rows` fields to `RatatuiRenderer`, updated all three `insert_before` sites (`commit`, `commit_styled`, `commit_panel_labeled`) to increment the counter, converted `repin_rows` from two-arg to three-arg form (`old_top`, `content_end`, `height`) so the clear range starts at the end of real committed content rather than the old viewport top, and added the `DAEMONEYE_REANCHOR_TRACE` env-gated diagnostic in `reanchor`. All seven test struct-literals updated. Mutation M1 confirmed the new `repin_rows_clears_debris_between_content_and_park` test fails when the `content_end` min-term is dropped, and passes with the fix. All four gates green (fmt, build, clippy, test — 1234 tests pass). Working tree clean, one commit.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
cludes_other_windows ... ok
test tmux::bounded_output_tests::bounded_output_times_out_and_kills_the_child ... ok

test result: ok. 1234 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_strips_trailing_prose ... ok
test header_status_reads_bare_word ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test header_status_uses_first_occurrence_only ... ok
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
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
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

- `docs/dev/milestones/M13-chat-ux/README.md` — +1 -1
- `docs/dev/milestones/M13-chat-ux/phase-07-content-extent-clear.md` — +48 -1
- `src/cli/render_ratatui.rs` — +113 -9

**Commit:** f11dbcaaaa0c20fe1e65a6862cdaa7525ceebbe2

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
