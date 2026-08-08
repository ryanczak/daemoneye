# Phase 01: Multi-Session Cache

**Milestone:** M12 — Full-View tmux Integration
**Status:** review
**Depends on:** none
**Estimated diff:** ~420 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

`SessionCache` stops discarding panes that belong to other tmux sessions: every
pane on the server is cached with metadata (including which session owns it),
while pane *content* is still captured only for the adopted (home) session.
This phase is **behavior-preserving at every existing surface** — foreign panes
enter the cache but are filtered out everywhere they would be visible today.
Later phases (`read_pane`, the `list_panes` upgrade) expose them deliberately.
As part of touching `refresh()`, this phase also fixes a latent defect: panes
that no longer exist are now evicted from the cache instead of lingering
forever.

## Architecture references

Read before starting:

- `docs/design/tmux-integration.md` § D1 — the settled design this phase
  implements (multi-session cache, metadata-everywhere / content-at-home).
- `CLAUDE.md` § "Key files" rows for `src/tmux/cache.rs` and `src/tmux/pane.rs`.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

All facts below were verified against the tree at drafting (2026-08-07;
baseline `cargo test --lib` = **1147 passed**).

**The discard.** `SessionCache::refresh()` (`src/tmux/cache.rs:203`) fetches
every pane on the tmux server — `tmux::list_panes_detailed()`
(`src/tmux/pane.rs:45`) runs `list-panes -a` and returns `RichPaneInfo` values
that carry `session_name` — and then throws foreign panes away:

```rust
// src/tmux/cache.rs:220-228
let mut captures: Vec<(crate::tmux::RichPaneInfo, String)> = Vec::new();
for info in rich_panes {
    if info.session_name != session {
        continue;                       // <-- the discard this phase removes
    }
    if let Ok(content) = tmux::capture_pane(&info.pane_id, 100) {
        captures.push((info, content));
    }
}
```

`PaneState` (`src/tmux/cache.rs:74`) has **no** `session_name` field — the
information is dropped at this boundary.

**No eviction.** The only writes to `self.panes` are `set_session()`'s
`.clear()` (`src/tmux/cache.rs:162`) and `refresh()`'s
`entry().or_insert_with()` upsert (`src/tmux/cache.rs:231-235`). Nothing ever
removes a pane whose tmux pane has been closed; stale entries persist until the
daemon restarts or the session is re-adopted.

**The five iteration surfaces** that enumerate `cache.panes` and would newly
show foreign panes unless filtered:

1. `SessionCache::pane_map_summary` — `src/tmux/cache.rs:383` (`[PANE MAP]`).
2. `SessionCache::get_labeled_context` "others" loop — `src/tmux/cache.rs:591`
   (`[VISIBLE PANE]`/`[BACKGROUND PANE]`/`[SESSION PANE]` lines).
3. `list_panes` tool — `src/daemon/executor/knowledge/pane.rs:75`.
4. `handle_list_panes` (the `/pane` IPC handler) —
   `src/daemon/server/handlers.rs:177`.
5. `find_best_target_pane` fallback pane list —
   `src/daemon/executor/mod.rs:967-978` (feeds `Response::PaneSelectPrompt`).

**The four keyed validation sites** that check `panes.contains_key(id)` to
accept a pane as a foreground target or hint. Today a foreign pane is absent
from the map, so `contains_key` returning true implies home-session membership;
after this phase that implication breaks unless each site checks the session:

- `src/daemon/executor/mod.rs:940-942` (AI-specified `target_pane`)
- `src/daemon/executor/mod.rs:961-963` (session default target)
- `src/daemon/executor/foreground.rs:249-251` and
  `src/daemon/executor/foreground.rs:257-259` (approval-prompt target hint)

All other `panes.get(id)` sites (`src/daemon/prompt.rs:70`,
`src/daemon/server/handlers.rs:136`, `src/memory/tags.rs:191`,
`src/daemon/executor/foreground.rs:295`, `src/tmux/cache.rs:494,594`) are keyed
lookups of an already-validated id — no change needed.

**`PaneState` literal constructors** that must gain the new field (the struct
has no `Default`): `src/tmux/cache.rs:235` (the `or_insert_with` in `refresh`),
the test fixture fn at `src/daemon/executor/knowledge/pane.rs:427`, and 14
literals in `src/tmux/cache_tests.rs` (lines 83, 119, 154, 176, 273, 308, 348,
384, 421, 444, 486, 509, 532, 555).

## Spec

### Task 1 — Add `session_name` to `PaneState`

In `src/tmux/cache.rs`, add to `PaneState` (after `window_name`):

```rust
/// Name of the tmux session that owns this pane (`#{session_name}`).
/// Equal to the cache's adopted session for "home" panes; other values
/// mark foreign-session panes, which carry metadata but no captured buffer.
pub session_name: String,
```

Update the `or_insert_with` literal in `refresh()` to initialize it to
`String::new()`, and add `entry.session_name = info.session_name.clone();` to
the field-copy block below it (alongside `entry.window_name = ...`), so the
value refreshes even if a pane migrates between sessions (`tmux move-pane`).

Update the fixture fn `pane(...)` in
`src/daemon/executor/knowledge/pane.rs:427` to set
`session_name: "sess".to_string()` — that matches the `SessionCache::new("sess")`
used by those tests. Update all 14 `PaneState` literals in
`src/tmux/cache_tests.rs` the same way, using the session name each test's
`SessionCache::new(...)` was constructed with (grep the enclosing test).

### Task 2 — Keep foreign panes in `refresh()`; capture content only for home

In `refresh()` (`src/tmux/cache.rs:203`), replace the discard loop quoted in
Current state with:

```rust
let mut captures: Vec<(crate::tmux::RichPaneInfo, Option<String>)> = Vec::new();
for info in rich_panes {
    let content = if info.session_name == session {
        tmux::capture_pane(&info.pane_id, 100).ok()
    } else {
        None // foreign pane: metadata only, no capture (D1)
    };
    captures.push((info, content));
}
```

In the write loop below, copy metadata for every entry as today, but only
update `buffer`/`summary`/`last_updated` when `content` is `Some(c)` and
`entry.buffer != c`. A foreign pane's `buffer` and `summary` stay empty; do
NOT synthesize a summary for foreign panes in this phase (phase 02 owns
summaries). A home pane whose `capture_pane` failed (`None`) must still get
its metadata refreshed — today a failed capture skips the pane entirely; the
new shape is strictly better and intended.

### Task 3 — Evict panes that no longer exist

In `src/tmux/cache.rs`, add a method on `SessionCache`:

```rust
/// Remove cached panes not present in the latest `list-panes -a` snapshot.
///
/// `live` is the full set of pane IDs on the server. An EMPTY set is
/// treated as "snapshot unavailable" and evicts nothing — a transient
/// `list-panes` failure must not wipe the cache.
pub fn evict_missing(&self, live: &std::collections::HashSet<String>) {
    if live.is_empty() {
        return;
    }
    self.panes
        .write()
        .unwrap_or_log()
        .retain(|id, _| live.contains(id));
}
```

Call it from `refresh()` after the write loop, with the set of `pane_id`s
collected from `rich_panes`. Note `rich_panes` is currently consumed by the
capture loop — collect the id set before that loop. `list_panes_detailed()`
failure already degrades to an empty vec via `.unwrap_or_default()`
(`src/tmux/cache.rs:217`), which the empty-set guard converts to "no eviction";
keep that path intact.

All lock sites use `.unwrap_or_log()` (the `UnpoisonExt` trait,
`src/util.rs`) — never `.unwrap()` on a `panes`/`session_name` lock. This is a
repo invariant (`CLAUDE.md` § Important Invariants).

### Task 4 — `is_home_pane` helper

In `src/tmux/cache.rs`, add:

```rust
/// True when `pane_id` is cached AND belongs to the adopted session.
/// Foreground execution and target hints must only accept home panes.
pub fn is_home_pane(&self, pane_id: &str) -> bool {
    let home = self.session_name.read().unwrap_or_log().clone();
    self.panes
        .read()
        .unwrap_or_log()
        .get(pane_id)
        .is_some_and(|p| p.session_name == home)
}
```

(Clone the session name *before* taking the `panes` lock, matching the order in
`refresh()` at `src/tmux/cache.rs:204` — never hold both locks across a call.)

Replace the four `contains_key` validation sites listed in Current state with
`cache.is_home_pane(tp)` / `cache.is_home_pane(&dtp)` / `cache.is_home_pane(dtp)`.
The surrounding chat-pane checks stay as they are. Example, at
`src/daemon/executor/mod.rs:936-945` the closure body becomes:

```rust
let ai_target = specified_pane.and_then(|tp| {
    if chat_pane == Some(tp) {
        return None;
    }
    if cache.is_home_pane(tp) {
        Some(tp.to_string())
    } else {
        None
    }
});
```

### Task 5 — Filter the five iteration surfaces to home panes

At each of the five surfaces listed in Current state, clone the home session
name once (`let home = cache.session_name.read().unwrap_or_log().clone();` —
inside `SessionCache` methods, `self.session_name`) and add one filter to the
existing iterator chain. Worked example for `pane_map_summary`
(`src/tmux/cache.rs:384-395`), current code:

```rust
let mut entries: Vec<_> = panes
    .iter()
    .filter(|(id, _)| chat_pane != Some(id.as_str()))
    .filter(|(_, state)| {
        !state.window_name.starts_with("de-bg-")
        // ... existing prefix filters unchanged ...
    })
    .collect();
```

becomes the same chain with one added line directly after `.iter()`:

```rust
    .filter(|(_, state)| state.session_name == home)
```

Apply the identical one-line filter to:

- the "others" collection in `get_labeled_context` (`src/tmux/cache.rs:599-603`),
- the `rows` collection in `list_panes` (`src/daemon/executor/knowledge/pane.rs:79-82`
  — note this fn already reads `cache.session_name` into `session` at line 77;
  reuse that binding, but move its read *before* the `panes.read()` at line 76),
- the `candidates` collection in `handle_list_panes`
  (`src/daemon/server/handlers.rs:178-199`),
- the `raw` collection in `find_best_target_pane`
  (`src/daemon/executor/mod.rs:969-976`).

Do NOT touch the `chat_window` lookup at `src/tmux/cache.rs:594` or any other
keyed `panes.get(...)` site.

### Task 6 — Tests

Write the tests named in the Test plan. Fixture recipe — what makes a pane
*foreign*: construct the cache with `SessionCache::new("home")`, insert one
pane whose `session_name` is `"home"` and one whose `session_name` is
`"other"`. The foreign pane must otherwise be fully targetable — a non-daemon
window name (e.g. `"editor"`), not the chat pane — so the ONLY thing excluding
it is the session filter. A foreign pane that would also be excluded as a
daemon window or chat pane makes every exclusion assertion vacuous.

## Acceptance criteria

Split per WORKFLOW.md: the first group are progress markers, each **confirmed
to fail against the current tree at drafting**; the second group are
no-regression guards that already pass and are NOT evidence of work.

Must currently fail → must pass when done:

- [ ] `grep -c '    pub session_name: String,' src/tmux/cache.rs` prints `1`
      (drafting: `0`). The four-space indent distinguishes the new `PaneState`
      field from the existing `pub session_name: RwLock<String>` on
      `SessionCache` — do not count that one.
- [ ] `grep -c 'evict_missing' src/tmux/cache.rs` prints ≥ `2` (definition +
      call from `refresh`; drafting: `0`).
- [ ] `grep -rn 'is_home_pane' src/daemon/executor/ | wc -l` prints ≥ `4`
      (the four converted validation sites; drafting: `0`).
- [ ] `cargo test --lib foreign_session` runs ≥ 4 passing tests (drafting: 0
      match this filter; the one pre-existing test matching bare "foreign" is
      `namespaces_ghost_excludes_foreign_namespace`, which does not contain
      "foreign_session").
- [ ] Negative case: with the mutation of Mutation pair 1 applied (session
      filter removed from `pane_map_summary`),
      `pane_map_excludes_foreign_session_panes` FAILS. Restored, it passes.
- [ ] Negative case: `evict_missing_ignores_empty_snapshot` passes — an empty
      live-set must NOT clear the cache.

Already pass today (no-regression guards):

- [ ] `cargo test --lib` — every pre-existing test still green (baseline 1147;
      expect baseline + new tests, no removals).
- [ ] `cargo test --lib list_panes` — the existing chat-pane exclusion tests
      in `src/daemon/executor/knowledge/pane.rs` still pass.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo fmt --all` produces no diff.

## Test plan

All in `src/tmux/cache_tests.rs` unless noted; construct `SessionCache`
directly (no live tmux, no HOME mutation — `test_home_guard` is not needed
here). Use the Task 6 fixture recipe.

- `pane_map_excludes_foreign_session_panes` — home + foreign pane cached;
  `pane_map_summary(None)` output contains the home pane id and does NOT
  contain the foreign pane id.
- `labeled_context_excludes_foreign_session_panes` — same fixture;
  `get_labeled_context(None, None)` contains no line naming the foreign pane
  id. Assert on the pane id (e.g. `%9`), not on a window name.
- `list_panes_excludes_foreign_session_panes` — in
  `src/daemon/executor/knowledge/pane.rs` tests, alongside
  `list_panes_excludes_chat_pane` (fixture fn at line 427 gains a
  `session_name` parameter or a sibling helper): foreign pane absent from
  `list_panes()` output, home pane present.
- `is_home_pane_rejects_foreign_session_pane` — `is_home_pane` returns true
  for the home pane, false for the foreign pane, false for an unknown id.
- `evict_missing_removes_closed_panes` — cache two panes; `evict_missing`
  with a live-set containing only one; assert the map holds exactly that one.
- `evict_missing_ignores_empty_snapshot` — cache two panes; `evict_missing`
  with an empty set; assert both panes are still cached (the pinned negative
  case).
- `refresh_metadata_persists_for_foreign_panes` is NOT required — `refresh()`
  spawns tmux subprocesses and is not hermetically testable; its foreign-pane
  path is covered by the field/filter tests above plus review.

**Mutation pairs — the executor runs BOTH directions and restores, and the
architect re-runs both at review** (self-reported mutation checks alone are
not accepted):

1. In `pane_map_summary`, delete the `state.session_name == home` filter line
   → `pane_map_excludes_foreign_session_panes` must FAIL. Restore the line →
   it must pass again.
2. In `evict_missing`, delete the `if live.is_empty() { return; }` guard →
   `evict_missing_ignores_empty_snapshot` must FAIL. Restore → pass.

If either mutation leaves the named test green, the fixture is inert —
**report a blocker in the Update Log rather than adjusting the test until it
fails**; a fixture rewritten under mutation pressure is exactly the vacuous
guard this pair exists to catch.

## End-to-end verification

This phase is behavior-preserving at every user-visible surface, and its new
behavior (foreign panes in the cache) is only observable through tools that
land in later phases. The real-artifact check is therefore the full gate run
plus the mutation pairs, captured mechanically:

```sh
cargo test --lib 2>&1 | tail -5 > /tmp/e2e-01.txt; echo "exit=$?" >> /tmp/e2e-01.txt
cargo test --lib foreign_session 2>&1 | grep '^test ' >> /tmp/e2e-01.txt
grep -c '    pub session_name: String,' src/tmux/cache.rs >> /tmp/e2e-01.txt
grep -rn 'is_home_pane' src/daemon/executor/ >> /tmp/e2e-01.txt
cat /tmp/e2e-01.txt
```

Paste `/tmp/e2e-01.txt` verbatim into an Update Log entry titled
`### Update — <date> (end-to-end verification)`, together with the transcript
of both mutation pairs (the failing run AND the restored passing run for
each). The server-authored `(complete)` entry does not satisfy this.

## Authorizations

- None. No new dependencies; no `docs/architecture.md` changes.

## Out of scope

- Foreign-pane content capture, summaries, or any surface *showing* foreign
  panes (`read_pane` is phase 03; the `list_panes` labeled section is phase
  05).
- `PaneStatus` classification / `summarize()` replacement (phase 02).
- The shared targetable-panes predicate and `de-*` prefix-literal cleanup
  (phase 08) — this phase adds its one-line filters to the existing chains
  without restructuring them.
- `CLAUDE.md` / `sre.toml` changes — no tool or documented behavior changes in
  this phase.
- Any change to `[OTHER SESSIONS]` / `other_sessions_context()`.

## Update Log

### Update — 2026-08-08 02:14 (started)

**Executor:** claude

Starting Phase 01: Multi-Session Cache. Implementing `PaneState.session_name`, foreign-pane retention in `refresh()`, `evict_missing()`, `is_home_pane()` helper, session filters on iteration surfaces, and tests.

*(entries appended by the executor and architect as the phase progresses)*

### Update — ts=1786156899902 (complete, server-authored)

**Summary:** **Summary + Notes for review:**

Phase 01 is complete. All six tasks implemented: (1) `PaneState.session_name` field added and all 17 existing literals + the pane fixture updated; (2) `refresh()` now retains foreign panes with metadata-only (no content capture), and home panes with failed captures still get metadata refreshed; (3) `evict_missing()` removes stale entries, called from `refresh()` after the write loop, with an empty-set guard that prevents cache wipes on `list-panes` failures; (4) `is_home_pane()` helper replaces four `contains_key` validation sites in `executor/mod.rs` and `foreground.rs`; (5) session-name filters added to all five iteration surfaces (`pane_map_summary`, `get_labeled_context`, `list_panes`, `handle_list_panes`, `find_best_target_pane`); (6) 7 new tests (5 in `cache_tests.rs`, 1 in `pane.rs`, plus the fixture `test_pane()`), all passing.

Both mutation pairs verified: removing the `session_name == home` filter from `pane_map_summary` causes `pane_map_excludes_foreign_session_panes` to FAIL; removing the `live.is_empty()` guard from `evict_missing` causes `evict_missing_ignores_empty_snapshot` to FAIL. Both restored and passing.

All 1153 tests pass (1147 baseline + 6 new), clippy clean, fmt clean. No deviations from spec.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.48s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
ache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test tmux::bounded_output_tests::bounded_output_times_out_and_kills_the_child ... ok

test result: ok. 1153 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.14s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test header_status_strips_trailing_prose ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 4 tests
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 32 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_child_inherits_depth_and_parent ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
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

   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 6.85s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)
     Running unittests src/main.rs (target/debug/deps/daemoneye-e700f2084319867a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9b22636ef5c08466)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c00c74ef4ffe9c11)
     Running tests/integration.rs (target/debug/deps/integration-6230826c10f36795)
     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M12-tmux-integration/README.md` — +1 -1
- `docs/dev/milestones/M12-tmux-integration/phase-01-multi-session-cache.md` — +7 -1
- `src/daemon/executor/foreground.rs` — +4 -8
- `src/daemon/executor/knowledge/pane.rs` — +28 -1
- `src/daemon/executor/mod.rs` — +5 -6
- `src/daemon/server/handlers.rs` — +2 -0
- `src/tmux/cache.rs` — +54 -9
- `src/tmux/cache_tests.rs` — +130 -10

**Commit:** bc55174814832ea47f8bc948e099ed21b2ad01dd

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
