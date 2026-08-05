# Phase 03b: Sweep deletions — retention removes index rows, not just files

**Milestone:** M11 — Unified Knowledge Index
**Status:** todo
**Depends on:** phase-03a (done — the four append choke points index incrementally)
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

When a retention sweep unlinks a session archive or an event segment, the index
rows that point into that file must go with it. Today they survive, so the
`turns` and `events` corpora accumulate rows whose stored byte offsets reference
files that no longer exist. This is the deletion half of the milestone's
"incremental consistency" exit criterion; phase 03a did the append half.

## Architecture references

Read before starting:

- `docs/design/knowledge-index.md` § "Write paths" — the choke-point table and
  the best-effort contract.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Two sweep functions unlink files and touch nothing else.

`sweep_session_archives(retention_days, active_sessions)` in
`src/daemon/utils/mod.rs` walks `sessions_dir()`, filters to `*.archive.jsonl`,
skips files newer than the cutoff and sessions in `active_sessions`, and removes
the rest. The session id is derived from the filename:

```rust
// Extract session id from `id.archive.jsonl`
let session_id = name.trim_end_matches(".archive.jsonl");
if active_sessions.contains(session_id) {
    continue;
}

log::info!("sessions: deleting expired archive {}", path.display());
if let Err(e) = std::fs::remove_file(&path) {
    log::warn!("sessions: failed to delete archive {}: {}", path.display(), e);
}
```

`sweep_event_segments(retention_days)` in `src/daemon/utils/event_log.rs` walks
`events_dir()`, parses each filename with `segment_date_from_path`, and removes
segments older than the cutoff:

```rust
if let Some(date) = segment_date_from_path(&path)
    && date < cutoff
{
    log::info!("events: deleting expired segment {}", path.display());
    if let Err(e) = std::fs::remove_file(&path) {
        log::warn!("events: failed to delete {}: {}", path.display(), e);
    }
}
```

Both return `()` and log-and-continue on every error. **Preserve that contract** —
an index failure must never abort a sweep or propagate, exactly as in 03a.

Both are called from the daemon's maintenance tick at `src/daemon/mod.rs:833`
and `:836`. **Do not change the call sites**; the new behavior lives inside the
two functions.

### The identifiers the index stores

- `turns_map.session_id` is the bare session id — the filename with
  `.archive.jsonl` stripped, which is exactly what the sweep already computes.
- `events_map.segment` is the segment label 02b/03a established: `"legacy"` when
  the path equals `crate::config::events_path()`, otherwise the path's
  `file_stem` (e.g. `events-20260803`). `sweep_event_segments` only ever deletes
  dated segments — `segment_date_from_path` returns `None` for anything not
  matching `events-YYYYMMDD` — so the label you need is always the `file_stem`,
  never `"legacy"`.

## Spec

### 1. Two public removal functions — `src/memory/index.rs`

Add, beside 03a's `remove_artifact`:

- `remove_session_turns(session_id: &str) -> Result<()>`
- `remove_event_segment(segment: &str) -> Result<()>`

Each opens the index itself and returns `anyhow::Result<()>` so the call site
logs and swallows — the same shape as `remove_artifact`
(`src/memory/index.rs`, added in 03a):

```rust
pub fn remove_artifact(kind: &str, name: &str) -> Result<()> {
    let conn = open_index()?;
    conn.execute(
        "DELETE FROM artifacts WHERE kind = ?1 AND name = ?2",
        (kind, name),
    )
    .context("deleting artifact row from index")?;
    Ok(())
}
```

**The contentless corpora need two statements, in a transaction, in this order.**
`turns` and `events` are `content='', contentless_delete=1` tables with no
`session_id` / `segment` column of their own — the only link is the map table's
`id`, which is the FTS `rowid`. So:

```rust
pub fn remove_session_turns(session_id: &str) -> Result<()> {
    let mut conn = open_index()?;
    let tx = conn.transaction().context("beginning transaction")?;
    tx.execute(
        "DELETE FROM turns WHERE rowid IN (SELECT id FROM turns_map WHERE session_id = ?1)",
        (session_id,),
    )
    .context("deleting turns FTS rows")?;
    tx.execute(
        "DELETE FROM turns_map WHERE session_id = ?1",
        (session_id,),
    )
    .context("deleting turns_map rows")?;
    tx.commit().context("committing turns removal")
}
```

`remove_event_segment` is the same shape against `events` / `events_map`, keyed
on `segment`.

**GOTCHA — the order is load-bearing and getting it wrong fails silently.** The
FTS delete reads the ids out of the map table via the subquery. If you delete the
map rows first, the subquery matches nothing, the FTS delete affects **zero**
rows, and the orphaned content stays searchable forever — with no error, no
warning, and a green test suite if the test only checks the map. Executed on this
build, both orders, same fixture:

```
correct order:  deleted 1 fts rows, deleted 1 map rows  -> alpha=0 beta=1  PASS
wrong order:    deleted 1 map rows, deleted 0 fts rows  -> alpha=1 beta=1  FAIL
```

(`alpha` is the removed session's match count, `beta` a second session that must
survive.) A targeted `DELETE ... WHERE rowid IN (...)` **does** work on a
`contentless_delete=1` table — that was executed, not assumed.

### 2. Hook `sweep_session_archives` — `src/daemon/utils/mod.rs`

After a **successful** `remove_file`, call `remove_session_turns(session_id)` and
log-and-continue on error. Do not call it when the unlink failed — the file is
still there and its rows still describe real content.

### 3. Hook `sweep_event_segments` — `src/daemon/utils/event_log.rs`

After a successful `remove_file`, derive the segment label from the path's
`file_stem` and call `remove_event_segment(label)`, log-and-continue on error.
Same rule: only after the unlink actually succeeded.

## Acceptance criteria

- [ ] A swept archive's rows are gone: index a session's turns, run
      `sweep_session_archives` with a retention that expires it, and assert the
      `turns` corpus no longer matches its text **and** `turns_map` has no rows
      for that session id.
- [ ] **A second session is untouched.** The same test asserts an unexpired
      session's rows still match — a sweep that empties the whole corpus passes a
      single-session test and is wrong.
- [ ] A swept event segment's rows are gone, keyed by `file_stem` label, with a
      second segment surviving.
- [ ] **A file the sweep skips keeps its rows.** A session in `active_sessions`,
      and a file newer than the cutoff, both retain their index rows.
- [ ] **`retention_days == 0` removes nothing** — neither files nor rows. Both
      functions already early-return; pin it so the index hook cannot be hoisted
      above that guard.
- [ ] **A failing index never breaks a sweep.** With the index unwritable, both
      sweeps still unlink their files and return normally.
- [ ] After a sweep, `reconcile_index()` produces the same per-corpus counts as
      the incremental path left behind — no orphan rows, no missing rows.
- [ ] `cargo fmt --all` clean, `cargo build` clean,
      `cargo clippy --all-targets --all-features -- -D warnings` clean,
      `cargo test` green, no existing test removed or `#[ignore]`d.

## Test plan

Use the home-guard convention already in each module (`crate::test_home_guard()`
plus a tempdir `HOME`). `src/daemon/utils/mod.rs:208` and
`src/daemon/utils/event_log.rs:519` already have sweep tests that build expired
files by setting mtimes / dated filenames — follow those fixtures rather than
inventing new ones.

- `sweeping_an_archive_removes_its_turns_rows`
- `sweeping_an_archive_leaves_other_sessions_indexed` — the two-session case.
- `sweeping_a_segment_removes_its_events_rows`
- `sweeping_a_segment_leaves_other_segments_indexed`
- `active_session_archive_keeps_its_rows`
- `zero_retention_removes_no_rows` — for both sweeps.
- `sweep_survives_unwritable_index` — the best-effort guarantee. Follow
  `index_failure_does_not_break_append` in `src/memory/index.rs`, which chmods the
  index dir to `0o000` and restores the permissions at the end.
- `sweep_then_reconcile_agree` — snapshot `per_corpus` after the sweep, run
  `reconcile_index()`, assert identical counts.

**Negative cases to pin** (each must NOT happen):

- Deleting one session's rows must **not** delete another's. Assert the surviving
  session's count is exactly its original value, not merely non-zero.
- An FTS row must not outlive its map row. After removing a session, assert the
  `turns` corpus does not match that session's text — **checking `turns_map`
  alone will pass against the wrong delete order** and is the single most likely
  way this phase ships broken.
- `retention_days == 0` must leave both the file and its rows in place.

## End-to-end verification

Run exactly this block and paste both files verbatim into your Update Log entry:

```sh
cargo test --lib memory::index > /tmp/phase03b-tests.txt 2>&1; echo "exit=$?" >> /tmp/phase03b-tests.txt
cargo test --lib daemon::utils >> /tmp/phase03b-tests.txt 2>&1; echo "exit=$?" >> /tmp/phase03b-tests.txt
{ echo "--- delete order: FTS before map at every removal site ---";
  grep -n -A6 "DELETE FROM turns WHERE rowid\|DELETE FROM events WHERE rowid" src/memory/index.rs;
  echo "--- sweep hooks are best-effort, never ?-propagated ---";
  grep -n "remove_session_turns(\|remove_event_segment(" \
    src/daemon/utils/mod.rs src/daemon/utils/event_log.rs;
} > /tmp/phase03b-checks.txt 2>&1; echo "exit=$?" >> /tmp/phase03b-checks.txt
```

The first file must show both test groups passing. The second must show each
`DELETE FROM turns/events WHERE rowid` **preceding** its `DELETE FROM
*_map` counterpart, and every sweep call site wrapped in `if let Err(e) = …`
rather than terminated with `?`.

Put the pasted output in an Update Log entry titled
`### Update — <date> (end-to-end verification)`. **`docs/dev/WORKFLOW.md`
requires one such entry per dispatch** — an entry from an earlier round of this
phase does not carry forward, and the server-authored `(complete)` entry and its
"Command output tails" block never satisfy it. **Paste the files whole. Do not
summarise the test runs to a line each** — that fails `STANDARDS.md` §1 even when
every claim in it is true.

## Mutation check before reporting complete

Swap the two statements in `remove_session_turns` so the map delete runs first,
confirm `sweeping_an_archive_removes_its_turns_rows` **fails**, then restore the
correct order and confirm it passes. State both results in your Update Log. A
test that passes in both orders is not testing the thing this phase exists to get
right.

## Authorizations

- Modify: `src/memory/index.rs`, `src/daemon/utils/mod.rs`,
  `src/daemon/utils/event_log.rs`.
- No new dependencies. No DDL or `SCHEMA_VERSION` change.
- Do **not** change the sweep call sites in `src/daemon/mod.rs`, and do not
  change either sweep's signature, return type, or retention semantics.

## Out of scope

- Any read surface: `recall_context`, `search_repository`, `fts5_search` are
  untouched — phases 04 and 05.
- The other sweeps (`sweep_pane_logs`, `sweep_agent_mailboxes`,
  `sweep_session_archives`' non-archive siblings) — they feed no corpus.
- Retention defaults. The archive default of `0` (keep forever) stands; this
  phase only makes deletion consistent when retention *is* configured.
- The working `<id>.jsonl` / `.meta.json` / `.epochs.jsonl` files having no
  retention path at all — noted at scoping as a future-hygiene item, not M11.

## Update Log

<!-- entries appended below this line -->
