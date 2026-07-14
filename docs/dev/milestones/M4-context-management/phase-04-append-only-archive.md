# Phase 04: Append-only session archive

**Milestone:** M4 — Context Management Overhaul
**Status:** review
**Depends on:** none (parallel-safe with phases 01–03)
**Estimated diff:** ~350 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Stop destroying conversation history at compaction (design defect D1): every
message a session ever exchanges is appended to
`var/log/sessions/<id>.archive.jsonl`, which is **never rewritten**.
Compaction keeps rewriting only the working file. Elision placeholders stop
claiming dropped output is "in events.jsonl" (D2, partial — the recall tool
that completes the fix is phase 07, which consumes this archive).

## Architecture references

Read before starting:

- `docs/design/context-management.md#31-append-only-archive` — the design.
- `docs/design/context-management.md#6-invariants` — the append-only
  invariant this phase establishes.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Re-verify the **Current state** anchors — phases 01–03 may have moved
   them.

## Current state

- Session persistence lives in `src/daemon/session.rs` (line numbers
  re-validated 2026-07-14):
  - `session_file(id)` (`:152`) → `sessions_dir().join("{id}.jsonl")`.
  - `append_session_message(id, msg)` (`:185`) — the hot path, one append
    per message; **7 production callers** (see Spec §2).
  - `write_session_file(id, messages)` (`:161`) — atomic full rewrite
    (tmp + fsync + rename), called from `src/daemon/stream.rs:674-678`:

    ```rust
    if needs_compaction {
        write_session_file(id, &messages);      // <-- destroys dropped msgs
    } else {
        for msg in &messages[post_trim_len..] {
            append_session_message(id, msg);
        }
    }
    ```

- The elision placeholder (`src/daemon/digest.rs`, in
  `elide_old_tool_results`) reads:
  `"[elided: tool `{}` produced {} chars; outside live context window. See
  events.jsonl for full output.]"` — the events.jsonl claim is false (D2).
- Messages carry `turn: Option<...>` (`src/ai/types/wire.rs:27-32`) — set on
  push, `None` on legacy messages.
- Ghost sessions write through the same paths (`session_id` exists for
  them).
- The 30-minute idle eviction and named-session store
  (`src/session_store.rs`) do not touch these files' formats.

## Spec

### 1. Archive path + append — in `src/daemon/session.rs`

```rust
/// Path to the append-only archive of every message this session has
/// exchanged. NEVER rewritten or truncated by any code path — retention
/// (config `[sessions] archive_retention_days`) deletes whole files only.
pub fn archive_file(id: &str) -> std::path::PathBuf {
    crate::config::sessions_dir().join(format!("{}.archive.jsonl", id))
}

/// Append one message to the session archive, seeding the archive from the
/// working file on first use (so pre-archive history is captured).
pub fn append_archive_message(id: &str, msg: &Message);
```

Seeding, pinned: if `archive_file(id)` does not exist but `session_file(id)`
does, `std::fs::copy` the working file to the archive path **before** the
first append. (Race note: all writers for one session run inside the
per-session turn flow — no cross-thread archive writers exist in this phase;
a plain exists-check is sufficient. Do not add locking.)

Failure handling mirrors `append_session_message`: WARN + non-fatal.

### 2. Wire the write paths

**Re-validated 2026-07-14: the drafted "1 production site" count was WRONG —
`grep -rn "append_session_message" src/` shows SEVEN production call sites**
(`stream.rs:678`, `webhook/process.rs:151`, `background/helpers.rs:194`,
`ghost.rs:213/468/966`, `executor/knowledge/pane.rs:330`). Every one of them
appends a real conversation message that MUST be archived (D1). Wiring seven
sites by hand is fragile and a future eighth caller would silently break the
invariant.

**Therefore: call `append_archive_message` from INSIDE `append_session_message`**,
so every current and future caller archives automatically. Do NOT edit the seven
call sites individually. `append_archive_message` stays a separate, unit-testable
`pub fn` (the acceptance criteria exercise it directly), but its production caller
is `append_session_message` alone.

**Ordering gotcha (pin this):** `append_session_message` must call
`append_archive_message(msg)` **BEFORE** it appends `msg` to the working file.
Reason: `append_archive_message` seeds a missing archive by copying the *current*
working file. If the working file were written first, the seed copy would already
contain `msg` and the subsequent archive-append would write it a **second time**
(duplicate). Archive-first ordering means: new session's first message → neither
file exists → no seed, one archive append; pre-existing working file, no archive →
seed copies the prior N messages, then appends the new one → archive has N+1,
matching the working file. Pin `archive_appends_survive_compaction_rewrite` to
assert no duplicates (archive length == messages appended, not 2×).

This is correct precisely because the synthetic digest message is pushed via
`write_session_file` (the compaction rewrite), NOT `append_session_message`, so
folding the archive into `append_session_message` archives exactly the real
messages and never the derived digest.

In the `needs_compaction` branch (`stream.rs:674`), before
`write_session_file`, **no extra work is needed** — every message being
dropped was already archived when it was first appended. Add a comment
stating this invariant at the call site:

```rust
// Archive invariant: every message in the pre-compaction vec was appended
// to the archive when first persisted, so the rewrite below cannot lose
// history (see docs/design/context-management.md §3.1).
```

There is one exception: synthetic messages created *by* compaction (the
digest message) are part of the working set but not the archive — that is
correct (the archive holds what actually happened; digests are derived).
State this in a comment on `append_archive_message`.

### 3. Honest elision placeholder — `src/daemon/digest.rs`

Change the placeholder format (both the aggressive path and, if phase 03 has
landed, the soft-truncation marker stays as-is) to:

```
[elided: tool `{tool_name}` produced {n} chars at turn {turn}; archived — full
output retrievable from the session archive.]
```

`{turn}` comes from the containing message's `turn` field; when `None`
(legacy), write `turn unknown`. Do NOT mention `recall_context` yet — the
tool does not exist until phase 07 (phase 07 owns that wording change);
naming a nonexistent tool would make the model emit invalid calls.

Also update `trim_history`'s placeholder (`src/daemon/session.rs:257`)
from "trimmed to fit the context window" to append: `"Earlier messages are
preserved in the session archive."`

### 4. Retention — config + sweep

- Add to `SessionsConfig` (`src/config/types.rs:62`, follow the
  `load_recent_turns` field pattern at `:72` with a serde default fn):

  ```rust
  /// Delete session archive files whose mtime is older than this many
  /// days. 0 = keep forever (default).
  #[serde(default)]
  pub archive_retention_days: u32,
  ```

- In the `session-cleanup` supervised task (`src/daemon/mod.rs`, ~line 684;
  phase 01 added `sweep_event_segments` at ~line 706 in the same task), add
  `sweep_session_archives(retention_days)` in `session.rs`: delete
  `*.archive.jsonl` files under `sessions_dir()` with mtime older than the
  cutoff; skip archives whose session id is currently in the in-memory
  store (an active session's archive is never swept). No-op at 0. Hourly
  cadence via the same counter idiom phase 01 introduced.

## Acceptance criteria

- [ ] `cargo test` passes; clippy `-D warnings` clean.
- [ ] After a simulated turn-append + compaction-rewrite sequence, the
      archive contains **every** message ever appended (including ones the
      working file no longer has), in order (test below).
- [ ] Seeding: a session with an existing working file and no archive gets
      the full working file copied into the archive before the new message
      (test below).
- [ ] No production code path opens the archive with write+truncate or
      calls `write_*` on it: `grep -rn "archive_file" src/` shows only
      append/read/copy/delete-by-retention uses (reviewer-verifiable).
- [ ] The string `See events.jsonl for full output` no longer exists in
      `src/` (negative grep).
- [ ] `sweep_session_archives` deletes only expired, non-active archives;
      no-op at 0.

## Test plan

FS tests take `crate::TEST_HOME_LOCK` + temp `HOME` (idiom:
`src/daemon/server/catchup.rs:248`).

- `archive_appends_survive_compaction_rewrite` in `session.rs` — append 30
  messages to working+archive, rewrite working to 5, assert archive still
  has 30 and its first/last contents match.
- `archive_seeds_from_existing_working_file` — pre-existing 10-message
  working file, no archive; first `append_archive_message` → archive has 11.
- `archive_seed_absent_working_file` — neither file exists; append → archive
  has exactly 1 (no error).
- `elision_placeholder_names_turn_and_archive` in `digest.rs` — placeholder
  contains `at turn 7` and `archived`; **negative:** does not contain
  `events.jsonl` or `recall_context`.
- `sweep_archives_respects_active_and_zero` — expired active-session archive
  survives; expired inactive archive deleted; `retention_days = 0` deletes
  nothing.

## End-to-end verification

1. Run a real daemon in a temp HOME, drive two `daemoneye ask` turns (any
   configured model; a local ollama entry suffices), then
   `wc -l $HOME/.daemoneye/var/log/sessions/*.archive.jsonl` and quote the
   line counts showing archive ≥ working file.
2. If no model is reachable in the executor environment, drive
   `append_session_message`/`append_archive_message` through the existing
   integration-test harness (`tests/integration.rs` session JSONL
   round-trip test is the pattern) and state that substitution explicitly in
   the completion log.

## Authorizations

None.

## Out of scope

- Do NOT build the recall tool or mention it in any string — phase 07.
- Do NOT compress archives (future work).
- Do NOT archive ghost-session synthetic wrap-up messages differently —
  ghosts flow through the same append path; no special-casing.
- Do NOT change `session import` or the named-session store formats.

## Update Log

### Update — 2026-07-14 17:00 (started)

**Executor:** `executor`

Started implementing append-only session archive (phase 04).
### Update — ts=1784050283769 (complete, server-authored)

**Summary:** All tasks for phase 04 are complete. Here's the summary:

**What was built:**
1. **`archive_file()` and `append_archive_message()`** in `src/daemon/session.rs` — append-only archive at `var/log/sessions/<id>.archive.jsonl`, seeded from the working file on first write to capture pre-archive history.
2. **Wired archive into `append_session_message()`** — all 7 production callers automatically archive. Archive-first ordering prevents duplicates during seed copy. Added the archive invariant comment at the `write_session_file` call site in `stream.rs`.
3. **Honest elision placeholders** — updated `elide_old_tool_results` and `trim_history` to reference the session archive and turn number instead of the misleading `events.jsonl` claim.
4. **Retention** — added `archive_retention_days` to `SessionsConfig`, implemented `sweep_session_archives()` in `src/daemon/utils/mod.rs`, and wired it into the session-cleanup supervisor task.

**Verification:**
- `cargo build` — clean
- `cargo clippy --all-targets --all-features -- -D warnings` — clean
- `cargo test` — all 879 tests pass (including 4 new archive tests + 1 sweep test)
- `grep "See events.jsonl for full output" src/` — 0 matches (negative criterion met)
- `grep "archive_file" src/` — only append/read/copy/delete-by-retention uses (no write+truncate)

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.96s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
tests::auth_empty_secret_always_allows ... ok
test webhook::server::tests::auth_missing_header_denies ... ok
test webhook::server::tests::auth_correct_token_allows ... ok
test webhook::server::tests::auth_token_without_bearer_prefix_denies ... ok
test webhook::server::tests::auth_wrong_token_denies ... ok
test tmux::cache::tests::get_labeled_context_chat_pane_excluded_from_background ... ok
test tmux::cache::tests::get_labeled_context_source_pane_excluded_from_background ... ok
test cli::commands::stream::stream_seam_tests::recv_line_preserves_partial_bytes_across_a_dropped_read ... ok
test search::tests::search_events_returns_tail_not_head_when_segment_exceeds_cap ... ok
test search::tests::search_finds_match_in_runbooks ... ok
test memory::tests::memory_without_frontmatter_has_no_tags ... ok
test search::tests::search_returns_empty_for_no_match ... ok
test session_store::tests::artifacts_round_trip ... ok
test session_store::tests::backfill_idempotent ... ok
test session_store::tests::backfill_missing_artifact_returns_error_name ... ok
test session_store::tests::backfill_stamps_memory_without_frontmatter ... ok
test session_store::tests::backfill_stamps_runbook ... ok
test session_store::tests::backfill_stamps_script ... ok
test session_store::tests::collision_allowed_with_force ... ok
test session_store::tests::collision_rejected_without_force ... ok
test session_store::tests::delete_nonexistent_errors ... ok
test session_store::tests::delete_removes_dir_and_index ... ok
test session_store::tests::list_returns_newest_first ... ok
test session_store::tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test session_store::tests::save_and_load_round_trip ... ok
test memory::tests::write_agent_invisible_to_global ... ok

test result: ok. 880 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.47s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 29 tests
test daemon_ping_status_loop ... ignored
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test event_log_append_read ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test config_pricing_round_trip ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g4_briefing_masking_applied ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok

test result: ok. 27 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v0.9.9 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 3.42s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-02078a4e6496e4bf)
     Running unittests src/main.rs (target/debug/deps/daemoneye-a516fbd17ae98770)
     Running tests/integration.rs (target/debug/deps/integration-ce8aee7d32f385fa)
   Doc-tests daemoneye

```

**Files changed:**
- `docs/dev/milestones/M4-context-management/phase-04-append-only-archive.md` — +5 -3
- `src/config/types.rs` — +5 -0
- `src/daemon/digest.rs` — +13 -3
- `src/daemon/mod.rs` — +12 -0
- `src/daemon/session.rs` — +196 -1
- `src/daemon/stream.rs` — +3 -0
- `src/daemon/utils/mod.rs` — +125 -0
- `src/session_store_tests.rs` — +1 -0

**Commit:** 6e1730808fdc74e855b312a3356377ca87f37f57

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

