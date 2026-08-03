# Unified Knowledge Index

Design for extending the FTS5 index at `var/index/memory.db` from a single-corpus
memory index into a unified knowledge index covering every durable store under
`~/.daemoneye`. Scoped as M11 (2026-08-03). Decisions here were settled in the
architect session that scoped the milestone; phase docs cite this file.

## Motivation

After the M4 context overhaul, exactly one store is indexed (memories) and it is
consumed from exactly one place (the per-turn dynamic prompt block). Every other
knowledge store is read by brute force:

| Store | Contents | Search today |
|---|---|---|
| `var/log/sessions/<id>.archive.jsonl` | Full never-rewritten turn history | `recall_context`: linear scan, case-insensitive substring, max 8 matches (`src/daemon/context/recall.rs`) |
| `var/log/sessions/<id>.epochs.jsonl` | Epoch/chapter narratives + tallies | not searchable at all |
| `var/log/events/events-*.jsonl` | All daemon events (webhook alerts, job completions, …) | substring over the flattened tail of the last 10 000 lines |
| `runbooks/`, `scripts/` | Operator knowledge | non-recursive substring scan (`src/search.rs`) |
| `memory/**.md` | Persistent facts | FTS5 on the prompt path; substring scan on the tool path |

The compaction slot0 message explicitly tells the model to retrieve originals
with `recall_context`, but that lands on the substring scan — the weakest search
in the system backs the feature long-running sessions depend on most. And the
model's explicit search tool (`search_repository`) is strictly weaker than the
implicit prompt-assembly search, because it never touches the index.

## Schema (SCHEMA_VERSION 2)

Five FTS5 tables plus two sidecar maps, all in the existing `memory.db`. The
existing drop-and-recreate on `PRAGMA user_version` mismatch is the whole
migration story: the index is derived, rebuild is always safe.

Small corpora store their content in the FTS table (as `memories` does today).
High-volume corpora are **contentless** (`content=''`, `contentless_delete=1` —
available: the tree bundles SQLite 3.50.x via libsqlite3-sys 0.38, well past the
3.43 floor): the JSONL files on disk are the content store, and excerpts are
built by re-reading one line at a stored byte offset.

| Table | Content | One row per | Notes |
|---|---|---|---|
| `memories` | stored | memory file | unchanged from v1 |
| `artifacts` | stored | runbook or script | columns: `kind`, `name`, `tags`, `body` |
| `epochs` | stored | epoch/chapter record | narrative + tally text (failed cmds, artifact names); narratives are model-written and dense |
| `turns` | contentless | archived **message** (not turn — user and assistant messages share a turn number) | body = `content` **plus all `tool_results` text** — tool output is where the archived bulk lives |
| `events` | contentless | event record | `event` name indexed as its own column so "webhook_alert" + free text composes; `body` = the flattened `k=v` form `json_to_readable` (`src/search.rs`) already produces |

Contentless FTS5 rows can return nothing but their rowid — not even UNINDEXED
columns — so each contentless table pairs with a sidecar map whose
`INTEGER PRIMARY KEY` is used as the FTS rowid:

```sql
CREATE TABLE turns_map (
    id         INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL,
    turn       INTEGER NOT NULL,
    offset     INTEGER NOT NULL      -- byte offset of the line in <id>.archive.jsonl
);
CREATE TABLE events_map (
    id      INTEGER PRIMARY KEY,
    segment TEXT NOT NULL,           -- 'events-20260803' or 'legacy'
    offset  INTEGER NOT NULL
);
```

Query path for contentless corpora: `MATCH` → rowids → join map → open the file,
`seek(offset)`, read one line, build the excerpt from the actual message. O(1)
excerpt retrieval, no rescan.

**Load-bearing invariant:** byte offsets are only valid because archives and
event segments are append-only and never rewritten or truncated — retention
deletes whole files only (`src/daemon/session.rs` archive doc-comment;
`sweep_event_segments`). The index makes that invariant load-bearing for a new
consumer; it must be restated in the schema module doc and `CLAUDE.md`.

FTS5 `snippet()`/`highlight()` are unavailable on contentless tables. Excerpts
locate the query terms in the re-read text directly — which also fixes the
current `recall.rs` defect where the excerpt is built from `msg.content` even
when the match was inside a tool result.

Tokenizer everywhere: `porter unicode61 remove_diacritics 2`, matching v1.
`build_match_expr` (per-term quoting, OR-join, 32-term cap) is reused as-is.

Separate tables per corpus rather than one mega-table: BM25 statistics must not
mix across corpora (a term rare in memories but common in tool output would rank
nonsensically), update patterns differ completely, and the proven `memories`
table stays untouched apart from the shared version bump. Cross-corpus queries
merge at the query layer, interleaved by per-corpus rank.

## Write paths

All index writes are **best-effort** (log a warning, never fail the caller) —
the established v1 convention. Each corpus has a single choke point:

| Corpus | Incremental hook | Delete path |
|---|---|---|
| `turns` | `append_archive_message` (`src/daemon/session.rs`) — the offset is the file length before the write | `sweep_session_archives` deletes rows by `session_id` when it unlinks an archive |
| `epochs` | `append_epoch` (`src/daemon/context/epochs.rs`) | none in steady state (epochs files are not swept today); reconcile covers drift |
| `events` | `log_event` (`src/daemon/utils/event_log.rs`) | `sweep_event_segments` deletes rows by segment name |
| `artifacts` | the approval-gated `write_script` / `write_runbook` / delete executor paths | delete hook mirrors the file delete |
| `memories` | unchanged (`index_memory_file` / `remove_from_index`) | unchanged |

`reconcile_index()` extends to all corpora (single transaction, as today), which
keeps `daemoneye reindex` the one-command full rebuild and keeps
reconcile-on-empty working for fresh installs. Offsets are recomputed from
scratch during reconcile, so a corrupted map is never fatal.

**Connection cost:** `open_index()` opens a fresh connection per call. Fine at
memory-CRUD frequency; `log_event` fires several times per turn. Ship per-call
opens first (still ~ms, still best-effort); a shared
`OnceLock<Mutex<Connection>>` with `unwrap_or_log` is the measured optimization
if it shows up, not a prerequisite.

## Masking prerequisite

Nothing unmasked may become searchable. Two write paths violate the mask-on-write
convention today and must be fixed **before** their corpora are indexed:

- `append_epoch` — narratives, failed-command strings and artifact names land raw.
- `log_event` — no masking at the choke point; some call sites pre-mask, which is
  per-site discipline, not enforcement.

Both get `mask_sensitive` at the write choke point, matching every other store.
Archives are already upstream-masked at production; `recall` additionally masks
on read, which is kept.

## Read surfaces

1. **`recall_context`** — query mode moves from the substring scan to BM25 over
   `turns`. Range mode stays a direct archive read (exact retrieval), with its
   rendering defect fixed: `tool_results` bodies are rendered, not just
   `msg.content`. New optional `scope: "all"` widens query mode across sessions
   (default remains the current session); cross-session hits are labeled with
   their session id/name. Per-session vs. all-sessions is one `WHERE` clause on
   `turns_map` — the capability falls out of the schema.
2. **`search_repository`** — memory / runbooks / scripts / events route through
   the index (stemming and ranking); new kinds `turns` and `epochs`. The
   filename-match behavior is preserved. The old substring scan survives only
   where the index cannot answer (it is not a fallback tier — if the index is
   present it is authoritative).
3. **Prompt assembly** (`assemble_turn_relevant_memory`) — three fixes: use the
   BM25 score (normalized) instead of the flat 0.2; key the merge by
   `(namespace, key)` instead of bare key; resolve FTS hits with targeted reads
   instead of full directory walks (v1 does 3–4 full memory-dir scans per turn).
4. **Situational awareness** (the design-latitude phase):
   - the dynamic block may add one or two budget-capped lines from `turns` /
     `epochs` when the current turn matches past failures ("this error appeared
     at turn 214 of session X");
   - incident-response ghosts seed their first prompt with an FTS query over
     `incidents` memories + past ghost epochs matching the alert text;
   - `add_memory` to `incidents` auto-populates `relates_to` from an FTS query
     for similar prior incidents (`expand_relates_to` already consumes the field;
     nothing fills it today).

## Non-goals / out of scope for M11

- **Archive retention default** stays 0 (keep forever). Changing it deletes user
  data — an operator decision, not a milestone side effect. The index tolerates
  either setting; sweep-driven row deletion is wired regardless.
- **`effective_confidence`** stays a stub; scoring fixes in this milestone are
  orthogonal to it.
- **Embedding / vector search.** FTS5 + BM25 only.
- **Named saved sessions** (`var/sessions/<name>/messages.jsonl`) are not a
  corpus in v2 — their content is a subset of the archives. Revisit if import
  workflows make orphaned named sessions common.
