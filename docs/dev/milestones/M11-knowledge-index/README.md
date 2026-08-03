# M11 — Unified Knowledge Index

**Goal:** Every durable knowledge store under `~/.daemoneye` — memories,
runbooks, scripts, session archives, epoch narratives, event log — is searchable
through one BM25-ranked FTS5 index, and the surfaces that matter (recall_context,
search_repository, per-turn prompt assembly) actually use it.

**Status:** planning

**Depends on:** M7 (FTS5 memory index — the v1 this extends), M9 (`daemoneye
reindex` — the rebuild command that must cover the new corpora), M4 (context
overhaul — the archive/epoch stores this indexes).

**Scoped:** 2026-08-03, PE decision, from an architect review of the context
management and memory subsystems. Design doc: `docs/design/knowledge-index.md`
— schema, write paths, and settled decisions live there; phase docs cite it
rather than restating it.

**Exit criteria:**

- [ ] **One index, all corpora.** `var/index/memory.db` at SCHEMA_VERSION 2
      holds `memories`, `artifacts`, `epochs`, `turns` (+map), `events` (+map);
      `daemoneye reindex` rebuilds all of them in a single transaction. Verified
      per corpus by deleting the DB and asserting a search hit after rebuild.
- [ ] **Contentless corpora round-trip.** A `turns`/`events` hit re-reads the
      JSONL line at the stored byte offset to build its excerpt. Verified by a
      test whose match text appears *only* in a `tool_results` body — the case
      the current substring scan mis-excerpts.
- [ ] **`recall_context` query mode is BM25-ranked** over the turns corpus, with
      no 8-match substring ceiling; range mode renders `tool_results` bodies; a
      `scope: "all"` query surfaces a hit from a *different* session, labeled
      with its session id.
- [ ] **`search_repository` gains stemming and ranking**: "restarting" finds a
      runbook containing "restart"; `kind=events` finds a `webhook_alert` record
      by free text; new kinds `turns` and `epochs` work.
- [ ] **Nothing unmasked becomes searchable.** `append_epoch` and `log_event`
      mask at the write choke point. Verified by writing a canary secret through
      each path and asserting it appears in neither the file nor any search
      result.
- [ ] **Incremental consistency.** Appending an archive message / event / epoch
      makes it searchable without a reindex; a retention sweep that unlinks an
      archive or event segment removes its rows. Verified by reconciliation-style
      tests, not construction order.
- [ ] **Prompt assembly stops walking directories.** The FTS resolution path in
      `assemble_turn_relevant_memory` does zero full memory-dir scans, uses the
      normalized BM25 score (not a flat constant), and keys its merge by
      `(namespace, key)`.
- [ ] **Docs true at close**: `CLAUDE.md` (tools table + key-files rows),
      `docs/architecture.md` § 2.3 knowledge flow, and the design doc match the
      shipped behavior; the append-only/offset invariant is stated in both the
      schema module doc and `CLAUDE.md`.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` clean;
      `cargo test` green; no regression against the M10 baseline.

## Architecture references

- `docs/design/knowledge-index.md` — the M11 design (schema, choke points,
  masking prerequisite, read surfaces).
- `docs/architecture.md` § 2.3 "Knowledge flow" — where recall sits today.
- `docs/design/context-management.md` — the M4 archive/epoch design the turns
  and epochs corpora index.
- `CLAUDE.md` § "Key files" — `src/memory/index.rs`, `src/daemon/context/`,
  `src/daemon/utils/event_log.rs`, `src/search.rs`.

## Phases

Ordering is deliberate: masking before any new corpus is indexed; schema before
hooks; hooks before the read surfaces that depend on live rows; the one
design-latitude phase (07) last, when every mechanical layer under it is proven.

| #  | Phase | Status |
|----|-------|--------|
| 01 | [write-time-masking](phase-01-write-time-masking.md) — `mask_sensitive` at the `append_epoch` and `log_event` choke points | done |
| 02a | [index-schema-v2](phase-02a-index-schema-v2.md) — all seven tables, SCHEMA_VERSION 2, `reconcile_index()` over the stored-content corpora (`artifacts`, `epochs`), per-corpus `reindex` report | done |
| 02b | [contentless-corpora](phase-02b-contentless-corpora.md) — populate `turns` + `events` with byte-offset sidecar maps; `reconcile_index()` coverage for both | todo |
| 03 | incremental-hooks — best-effort index writes at the five choke points; row deletion from the two retention sweeps | todo |
| 04 | recall-context-fts — query mode on `turns`, range-mode `tool_results` rendering, excerpt-from-matched-field, `scope: "all"` | todo |
| 05 | search-repository-fts — index routing for memory/runbooks/scripts/events; new kinds `turns`, `epochs` | todo |
| 06 | prompt-scoring-fix — BM25 scores used, `(namespace, key)` merge keys, targeted reads instead of directory walks | todo |
| 07 | situational-injections — turns/epochs lines in the dynamic block, ghost cold-start seeding, incident `relates_to` auto-linking | todo |

Phase docs are drafted one at a time via `/rexymcp:architect next`; phases 01,
02a and 02b are drafted, 03–07 are not yet.

**Why 02 split into 02a/02b:** the original phase 02 covered the schema plus
`reconcile_index()` over all five corpora. The two contentless corpora need
byte-offset scanning of every archive file and event segment — on its own that
is comparable in size to the whole rest of the phase, and it lands
> 500 lines together. 02a establishes the complete schema (so 02b touches no
DDL) and the two straightforward stored-content corpora; 02b does the
offset-computing scanners. Numbered a/b rather than renumbering so phases 03–07
keep their existing ids.

## Notes

- **Contentless decision (settled at scoping):** `turns` and `events` are
  contentless FTS5 tables (`content=''`, `contentless_delete=1`); the JSONL
  files are the content store and excerpts re-read one line at a stored byte
  offset. This makes the archives'/segments' append-only invariant load-bearing
  for the index — restate it wherever the schema is documented.
- **Events are in scope (settled at scoping):** quickly searching webhook
  events is a primary use case; the `event` name is indexed as its own column.
- **Cross-session recall is in scope:** it is one `WHERE` clause on `turns_map`;
  the marginal cost is a tool-schema param and a doc row.
- **Explicitly out of scope:** archive-retention default change (operator
  decision — the current default of 0/keep-forever stands), the
  `effective_confidence` stub, vector search, and the named-saved-sessions
  corpus. See the design doc's non-goals.
- **Adjacent defects noted at scoping, not all in scope:** the executor
  hardcodes `LimitsConfig::default()` for recall output
  (`src/daemon/executor/mod.rs`) — fold into phase 04 if the diff stays small;
  archive seeding via `fs::copy` after a compaction captures the synthetic
  slot0/slot1 head (`src/daemon/session.rs`) — file as a bug if phase 03's
  tests confirm it; `pinned` is never written; working `<id>.jsonl` /
  `.meta.json` / `.epochs.jsonl` files have no retention path. The last two are
  candidates for a future hygiene milestone, not M11.
