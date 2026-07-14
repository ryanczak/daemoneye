# Phase 04: Append-only session archive

**Milestone:** M4 — Context Management Overhaul
**Status:** in-progress
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
