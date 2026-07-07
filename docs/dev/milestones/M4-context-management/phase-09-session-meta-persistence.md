# Phase 09: Session meta persistence and boundary-safe reload

**Milestone:** M4 — Context Management Overhaul
**Status:** todo
**Depends on:** phase-02 (token_scale exists)
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Stop daemon restarts and the 30-minute idle eviction from resetting a
session's world (design defect D10): persist the small per-session
continuity state to `<id>.meta.json` and load it when the entry is
recreated. Also make the disk-reload path land on a clean turn boundary so a
restarted session can never present an orphan tool_result to a provider.

## Architecture references

Read before starting:

- `docs/design/context-management.md#37-session-meta-persistence`
- `docs/design/context-management.md#2-failure-catalog` — D10.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Re-verify the **Current state** anchors — phases 02–08 have all touched
   these files.

## Current state

- Entry recreation: the `or_insert_with` in `src/daemon/server/ask.rs:106`
  builds a fresh `SessionEntry` with `started_at: chrono::Utc::now()`,
  `turn_count: 0`, `last_prompt_tokens: 0`, `token_scale: 1.5` (phase 02),
  `saved_name: None`, `tool_calls_this_session: 0` — losing all of these
  when the daemon restarted or the 30-minute eviction
  (`src/daemon/mod.rs:~680`) fired, even though the message history itself
  survives on disk.
- Disk reload: `read_session_file(id, max_history)`
  (`src/daemon/session.rs:270-283`) tail-slices with
  `msgs[msgs.len() - cap..]` — no boundary check; the slice can start on a
  user message carrying `tool_results` whose tool_call is gone (provider
  400s).
- `next_clean_turn_start(messages, start)` (`session.rs:202`) is the
  existing boundary finder.
- Atomic-write idiom: `write_session_file` (`session.rs:156-175`) — tmp
  file → `sync_all` → rename; copy this shape for the meta write.
- End-of-turn write-back: `src/daemon/stream.rs` (~657-678) is where
  per-turn state (tokens, scale, dirty) is already updated under the store
  lock — the natural place to also persist meta.
- Epoch span-start derivation (phase 05) reads
  `epochs.last().ts_end` and only falls back to `started_at` for the first
  epoch — so meta persistence of `started_at` matters mainly for
  first-epoch spans and the `/costs`-style displays.

## Spec

### 1. Meta type + IO — in `src/daemon/session.rs`

```rust
/// Per-session continuity state that must survive daemon restarts and
/// idle eviction. Serialized to `sessions_dir()/<id>.meta.json`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionMeta {
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub turn_count: usize,
    pub last_prompt_tokens: u32,
    pub token_scale: f64,
    pub tool_calls_this_session: usize,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub saved_name: Option<String>,
}

pub fn meta_file(id: &str) -> PathBuf;              // sessions_dir()/<id>.meta.json
pub fn write_session_meta(id: &str, meta: &SessionMeta);  // atomic tmp+rename; WARN on failure
pub fn read_session_meta(id: &str) -> Option<SessionMeta>; // None on absent/corrupt
```

Corrupt/partial JSON → `None` + WARN (never panic; never block session
creation).

### 2. Persist at end of turn — `src/daemon/stream.rs`

In the end-of-turn write-back block (where `entry.last_prompt_tokens` /
`token_scale` / `dirty` are updated under the lock), build a `SessionMeta`
from the entry and, **after releasing the lock**, call
`write_session_meta`. One write per turn; ghost sessions included (their
continuity matters for the same reasons).

### 3. Load at entry recreation — `src/daemon/server/ask.rs`

In the `or_insert_with` path: attempt `read_session_meta(id)` first; when
`Some(meta)`, seed the new entry's `started_at`, `turn_count`,
`last_prompt_tokens`, `token_scale`, `tool_calls_this_session`,
`saved_name` from it (all other fields keep their fresh defaults). The
closure passed to `or_insert_with` can do the read directly — it only runs
on a miss.

Mirror the same seeding at the **ghost** entry-construction site
(`src/daemon/ghost.rs`) if the ghost path can recreate an entry for an
existing session id (re-verify; if ghost sessions are always fresh ids,
note that in the completion log and skip).

### 4. Boundary-safe reload — `read_session_file`

Replace the bare tail slice: after slicing to the last `cap` messages,
advance the slice start with `next_clean_turn_start(&msgs, slice_start)`;
if `None` (no clean boundary in the tail at all), apply phase 03's
`repair_tail_head` to the sliced vec instead of returning it raw. Return
type/signature unchanged. The result must always pass the orphan checker.

### 5. Cleanup coupling

Where session artifacts are deleted (`/session delete`,
`delete_saved_session`, and any eviction-time file cleanup — grep
`session_file(` for deletion sites), delete the sibling `.meta.json` too.
Retention: the phase-04 archive sweep does NOT touch meta files (a
session's meta is tiny and its working file may still exist).

## Acceptance criteria

- [ ] `cargo test` passes; clippy `-D warnings` clean.
- [ ] Round-trip: write meta, drop the entry, recreate → `started_at`,
      `turn_count`, `token_scale` match the persisted values (test below).
- [ ] Corrupt meta file → entry creation succeeds with fresh defaults +
      WARN (**negative case**).
- [ ] Reload of a history whose natural tail slice starts on a
      tool_results message produces an orphan-free vec (phase 03's shared
      orphan checker — test below).
- [ ] A reload where NO clean boundary exists in the tail is repaired, not
      returned raw (**negative case** for the fallback).
- [ ] `turn_count` shown to the client (`SessionInfo.turn_count`) continues
      across a simulated restart (state-level test: recreate entry from
      meta, assert the next increment yields `persisted + 1`).
- [ ] Deleting a session removes its meta file.

## Test plan

FS tests take `TEST_HOME_LOCK` + temp HOME.

- `meta_roundtrip` / `meta_corrupt_returns_none` in `session.rs`.
- `entry_recreation_seeds_from_meta` — exercise the seeding logic (extract
  it as `fn seed_entry_from_meta(entry: &mut SessionEntry, meta:
  SessionMeta)` so it is testable without the full ask flow).
- `read_session_file_lands_on_clean_boundary` — fixture history engineered
  so the raw slice starts mid-tool-chain; orphan checker green.
- `read_session_file_repairs_when_no_boundary` — all-tool-result tail.
- `delete_session_removes_meta`.

## End-to-end verification

1. Run a real daemon in a temp HOME, complete one `daemoneye ask` turn (or
   drive the write-back through the integration harness if no model is
   reachable — state which), kill the daemon, restart, and quote
   `cat <id>.meta.json` plus the second turn's `SessionInfo`
   `turn_count` showing continuity.

## Authorizations

None.

## Out of scope

- Do NOT persist the full `SessionEntry` (bg_windows, approval state, pane
  prefs all have their own lifecycles).
- Do NOT change the 30-minute eviction policy itself.
- Do NOT migrate the named-session store (`src/session_store.rs`) — it has
  its own `meta.toml`; this phase is the *ephemeral* session layer.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
