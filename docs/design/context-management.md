# Context Management Overhaul — Design

**Status:** approved design for M4 (2026-07-07)
**Author:** architect (Claude), from a full read of the compaction path
**Milestone:** `docs/dev/milestones/M4-context-management/`

The goal: a DaemonEye daemon that runs for **hundreds of days** and chat
sessions that run for **thousands of turns**, with per-turn cost bounded
regardless of session age, nothing irreversibly lost, and compaction off the
interactive hot path.

---

## 1. Current state (as of M3 close, v0.9.9)

### 1.1 Data model

- Each session is a `Vec<Message>` in RAM (`SessionEntry.messages`,
  `src/daemon/session.rs:23`), mirrored to
  `~/.daemoneye/var/log/sessions/<id>.jsonl` — appended per message on the
  hot path (`append_session_message`), **fully rewritten** after any
  compaction (`src/daemon/stream.rs:671-677`).
- The pressure signal is `SessionEntry.last_prompt_tokens` — actual API usage
  (input + cache-read + cache-write) from the **previous** turn — divided by
  `ModelEntry::context_window()` (`src/config/types.rs:591`), a hardcoded
  per-model-family table (unknown models default to 32k).

### 1.2 The compaction ladder (`src/daemon/server/ask.rs:241-333`)

| Trigger | Action |
|---|---|
| `token_pct >= 50` and >= 20 msgs | **Elision** — tool_results > 3000 chars outside the last 8 messages become a placeholder (`digest.rs::elide_old_tool_results`) |
| `token_pct >= 60` **or** `len >= max_history` (80) | **Digest** — elide, optionally narrate, tally, compact |
| no `started_at` | fallback `trim_history` with a bare placeholder |
| floor | `DIGEST_THRESHOLD = 20` messages |

The digest pass (`src/daemon/digest.rs`):

1. **Narrative** (config `[digest] narrative_enabled`, off by default): a
   cheap model summarizes the about-to-drop slice into 8–15 lines. 20 s
   timeout, 60k-char input cap, **synchronous inside the user's turn**.
2. **Structured tally**: `tally_events` reads the **entire** `events.jsonl`
   via `read_to_string` and counts commands / edits / alerts / ghosts
   **since session `started_at`**, plus an mtime scan of the runbook /
   script / memory directories.
3. **Compaction** (`compact_with_digest`): result layout is
   `[messages[0], digest, last ~TAIL_KEEP(16) messages]`, cut at a "clean
   turn boundary" (`next_clean_turn_start` — a user message without
   tool_results). No clean boundary ⇒ compaction is **skipped entirely**.
4. The on-disk session file is rewritten to the compacted vec — the original
   transcript is destroyed.

The previous digest (always index 1) feeds the next narrative's input, so a
rolling summary exists implicitly. Subsequent-turn prompts carry a `[BUDGET]`
line (`src/daemon/prompt.rs:201`) that tells the model to wrap up at >= 75%.

Ghost sessions bypass all of this — `src/daemon/ghost.rs` sends
`entry.messages.clone()` straight to `chat()` (bounded only by the 20-turn
budget).

### 1.3 Known-stale doc note

`docs/architecture.md` and older notes describe an FTS5 memory index; the
actual `src/memory/index.rs` is a **stub** returning empty results — the real
search path is the grep scan in `src/search.rs`. This design therefore builds
recall on the grep idiom and treats FTS5 as future work (§8).

---

## 2. Failure catalog

Numbered for reference from phase docs (D1–D15).

**A. Loss is irreversible and unadvertised**

- **D1 — transcript destroyed at compaction.** `write_session_file` replaces
  the on-disk file with the compacted vec. Nothing dropped is recoverable.
- **D2 — the elision placeholder lies.** It says *"See events.jsonl for full
  output"* — but `command` events store a 200-char excerpt
  (`event_log.rs::log_command`) and `read_file`/`search_repository` outputs
  are never logged at all. There is also no tool to recall anything —
  `search_repository` does not cover transcripts.
- **D3 — telescoping summaries.** Each digest re-narrates
  `[previous digest + dropped turns]` into 8–15 lines; after k compactions
  everything old is a summary-of-a-summary^k. Worse,
  `format_messages_for_narrative` truncates its 60k-char input **from the
  oldest end**, so the *most recent* dropped turns are silently omitted from
  the narrative.

**B. Unbounded growth in the digest's inputs and output**

- **D4 — `events.jsonl` never rotates** and `tally_events` loads the whole
  file into memory on every digest. At hundreds of days (3+ events per AI
  call) this is a multi-GB read on the interactive path.
- **D5 — tallies always rescan from `started_at`** — O(session lifetime) per
  compaction — and the digest text grows monotonically:
  `files_edited.join(", ")` / `alerts_received.join(", ")` are unbounded.
- **D6 — `scan_artifacts(since = session start)`** lists everything modified
  in months, forever.

**C. Boundary and pinning defects**

- **D7 — `messages[0]` pinned forever**: the first-turn megaprompt (host
  audit, memory block, manifest, terminal snapshot) is stale after months,
  large, and untouchable by elision (no tool_results) or compaction.
- **D8 — `TAIL_KEEP = 16` is message-count-based, not token-based.** Heavy
  tails mean compaction frees too little; token_pct stays >= 60 and the
  digest re-fires every couple of turns (full events scan + narrative call
  each time). Light tails waste the window.
- **D9 — no clean boundary ⇒ no compaction at all** — a tool-dense session
  never compacts and eventually hard-fails at the provider.
- **D10 — state resets lose the plot.** Daemon restart or the 30-minute idle
  eviction recreates the entry with `started_at = now()`,
  `last_prompt_tokens = 0`, `turn_count = 0` (`ask.rs:106-129`); the reload
  path `read_session_file` tail-slices without respecting clean boundaries
  (`session.rs:279-282`) and can produce an orphan-tool_result head.

**D. Cost and latency in the wrong place**

- **D11 — compaction is synchronous inside `handle_ask`**, before the first
  token — the 20 s narrative timeout plus the events scan stalls the user
  exactly when the session is oldest.
- **D12 — every compaction invalidates the provider prompt cache**; combined
  with D8-thrash you pay cache-write rates on a ~100k prefix every few turns.

**E. Coverage gaps**

- **D13 — ghost sessions have no compaction at all.**
- **D14 — `[BUDGET]` tells the model to wrap up on token pressure** — wrong
  for a session designed to live for months; pressure is the compactor's
  problem.
- **D15 — `context_window()` is a stale hardcoded table**; unknown models get
  32k, and the pressure signal is blind for one turn after restart.

---

## 3. Target architecture

Five ideas, layered on the current machinery (which keeps its good bones:
clean-boundary preservation, elide-before-digest ordering, the hybrid
narrative with timeout fallback, event-sourced tallies).

New code lives in `src/daemon/context/` (`estimate.rs`, `epochs.rs`,
`recall.rs`, later absorbing `digest.rs` helpers).

### 3.1 Append-only archive — separate *what happened* from *what the model sees*

- `var/log/sessions/<id>.archive.jsonl` — every message is appended here at
  the same moment it is appended to the working file; the archive is **never
  rewritten**. Compaction rewrites only the working file
  (`<id>.jsonl`).
- Seeding: the first time a session with an existing working file gains an
  archive, the working file is copied in as the archive's initial content.
- Elision placeholders become honest and actionable:
  `[elided: tool `read_file` produced 48210 chars at turn 412; archived —
  retrieve with recall_context]`.
- Retention knob (`[sessions] archive_retention_days`, 0 = keep forever)
  enforced by the session-cleanup supervisor task. Compression (zstd) is
  future work (§8) — plain JSONL first, no new dependencies.

Fixes D1, D2 (with §3.4).

### 3.2 Epoch chain — hierarchical summaries instead of one regenerated digest

- On compaction, the dropped span `[turn a..b]` becomes one **immutable epoch
  record** appended to `var/log/sessions/<id>.epochs.jsonl`:

  ```json
  {"seq": 7, "kind": "epoch", "turn_start": 121, "turn_end": 168,
   "ts_start": "...", "ts_end": "...", "msg_count": 61,
   "narrative": "…8-15 lines…",
   "tally": {"commands_ok": 41, "commands_fail": 3, "failed_cmds": [...],
              "files_edited_count": 12, "files_edited": ["…max 10…"], ...},
   "artifacts": ["runbook:disk-pressure", "memory:pg-vacuum-tuning"]}
  ```

- **Per-span tallies**: the events scan is bounded to
  `[prev_epoch.ts_end, now]` using the segment-aware event reader (§3.6) —
  O(span), not O(session lifetime). List fields are capped (top 10 + count).
  Fixes D4 (reader side), D5, D6.
- **Chapters (L2 rollups)**: when uncovered epochs exceed
  `rollup_after` (default 10), the oldest 5 are summarized into one
  `kind: "chapter"` record carrying `covers: [seq_a, seq_b]` and the summed
  tally of the covered epochs. Rendering skips covered epochs. Context
  representation grows **O(log turns)** instead of collapsing to a constant.
  Fixes D3.
- **Narrative input truncation flips to keep-newest**: when the dropped slice
  exceeds the summarizer budget, keep the most recent messages, not the
  oldest (the previous epoch narrative already covers older ground).

### 3.3 Working-set layout and token budgeting

The rendered working set sent to the LLM:

```
[msg 0 — synthetic user]  "[Session Context]" block:
    compact execution header (env, daemon host, fg target)
    session ledger    — summed tallies across all epochs/chapters, capped lists
    chapter summaries — 1-2 lines each
    recent epochs     — last <= 8, narrative or tally excerpt
    recall hint       — "older turns are archived; use recall_context"
[msg 1 — synthetic assistant]  short ack ("Continuing session — context
    above covers turns 1..N.")
[tail…]  live messages from a clean turn boundary
```

- `messages[0]` is **regenerated at every compaction** — no more pinned
  first-turn megaprompt. The original first message lives in the archive.
  Fixes D7.
- **Token estimation** (`context/estimate.rs`): `chars/4 + per-message
  overhead`, times a per-session calibration scale
  `token_scale = EMA(actual_prompt_tokens / estimated_tokens)` updated each
  turn. The estimate substitutes for `last_prompt_tokens` when that is 0
  (post-restart), removing the one-turn blind spot. Fixes half of D10, D15's
  blind spot.
- **Budget-based cut with hysteresis**: crossing `compact_at_pct` (60)
  compacts down to `target_pct` (40) of the window — the boundary is chosen
  by walking token estimates back from the newest clean boundary, not by
  counting 16 messages. Compaction frees >= 20% of the window every time it
  runs ⇒ it runs rarely ⇒ prompt-cache invalidation is rare and worth it.
  Fixes D8, D12.
- **Synthesized boundary**: when no clean boundary exists in the target
  region, the cut is taken anyway and repaired — orphaned tool_results in
  the new head of the tail are stripped and noted, rather than skipping
  compaction. Fixes D9.
- `[BUDGET]` stops advising wrap-up on token pressure for interactive
  sessions (turn/tool budget warnings for ghosts remain). Fixes D14.

### 3.4 `recall_context` — eviction becomes a cache miss, not amnesia

New silent AI tool (wired per the standard add-a-tool checklist):

```
recall_context { query?: string, turn_start?: int, turn_end?: int }
```

- Grep-scan of the **current session's** archive file, in the `src/search.rs`
  idiom (case-insensitive substring, bounded results, context lines). A turn
  range without a query returns those turns' messages verbatim (bounded).
- Output passes `mask_sensitive` and the `limits.tool_result_chars` cap.
- Epoch summaries carry turn ranges, so the model navigates: read summary →
  recall originals. Fixes D2 fully.
- FTS5 indexing of transcripts is future work (§8) — the grep scan needs no
  new dependencies and matches existing search behavior.

### 3.5 Async compaction

- The epoch build (events scan + narrative call) runs as a **background
  tokio task after the turn completes**; the compacted vec is swapped into
  `SessionEntry` under lock with a staleness check (message count + last
  turn id captured at snapshot; if the session advanced, discard and retry
  after the next turn). The 60→40 hysteresis guarantees headroom for the
  next turn even if compaction is still running.
- Synchronous **emergency path** at `emergency_pct` (85): structured-only
  epoch (no narrative call), aggressive elision, hard trim. Fixes D11.

### 3.6 Event-log rotation

- `log_event` writes to `var/events/events-YYYYMMDD.jsonl` (UTC-dated
  segments). The legacy `var/events.jsonl`, if present, stays in place and is
  treated by readers as the oldest segment (no startup migration — read-both
  is simpler and keeps existing fixtures valid).
- Shared reader helpers: `event_segment_paths_between(from, to)` and
  `for_each_event_between(from, to, f)` — stream line-by-line, open only
  segments overlapping the window. All readers migrate: `digest.rs`
  (tally), `event_log.rs::sum_cost_between`, `daemon/stats.rs`,
  `server/catchup.rs`, `cli/commands/costs.rs`, `search.rs::search_events`.
- Retention: `[events] retention_days` (default 90; 0 = keep forever), swept
  by a supervisor task; the legacy segment is exempt (documented). Fixes D4.

### 3.7 Session meta persistence

- `var/log/sessions/<id>.meta.json` — `{started_at, turn_count,
  last_prompt_tokens, token_scale, tool_calls_this_session, saved_name}` —
  written atomically after each turn, loaded by the `or_insert` path in
  `ask.rs` so restart/eviction no longer resets the world.
- The reload path advances to a clean turn boundary before returning a
  tail-sliced history (no orphan tool_result head). Fixes D10.

### 3.8 Ghost coverage and memory extraction

- The ghost turn loop applies elision + structured-only epoch compaction
  (no narrative call — cheap, non-blocking) using estimated tokens. Fixes
  D13.
- Optional (`[compaction] extract_memories`, off by default): at epoch
  creation, one extra digest-model call proposes 0–3 durable facts written
  via the existing memory CRUD (`source: "compaction"`) — long sessions
  distill into the memory system that is already tiered into prompts.

---

## 4. Configuration

New `[compaction]` section (the name `[context]` is taken by the existing
`ContextConfig.environment`):

```toml
[compaction]
elide_at_pct   = 50     # elide oversized old tool_results
compact_at_pct = 60     # build an epoch and cut the working set
target_pct     = 40     # post-compaction working-set size target
emergency_pct  = 85     # synchronous structured-only fallback
rollup_after   = 10     # uncovered epochs before a chapter rollup
extract_memories = false

[events]
retention_days = 90     # 0 = keep segments forever

[sessions]
archive_retention_days = 0   # 0 = keep archives forever
```

`[digest] narrative_enabled` keeps its meaning (narrative per epoch) and its
default flips to **true** once compaction is async (phase 08) — the cost
argument for defaulting off was the synchronous stall.

`[limits] max_history` remains as the message-count safety net.

## 5. Migration & compatibility

- **Session files**: unchanged format. The archive is seeded from the working
  file on first write; sessions with an existing digest message simply have
  it superseded by the first epoch render (the old digest text is archived).
- **Epoch/meta files**: new, additive; absence means "no epochs yet" /
  "derive meta from defaults" — no migration step required.
- **events.jsonl**: one-time move into the segment directory at daemon
  startup; readers treat the legacy segment as the oldest.
- **IPC**: no breaking changes; compaction notices continue as
  `Response::SystemMsg`.
- Old `[digest]` config keys keep parsing.

## 6. Invariants

- The archive file is append-only — no code path may rewrite or truncate it
  (retention deletes whole files only).
- `epochs.jsonl` is append-only; correction happens by appending, never by
  editing (chapters *cover* epochs; they don't delete them).
- Every working set sent to a provider preserves tool_call ↔ tool_result
  pairing and starts its tail on a clean user turn (existing invariant,
  extended by the synthesized-boundary repair).
- Compaction must never lose a message that has not already been appended to
  the archive.
- All new lock sites use `.unwrap_or_log()` (`src/util.rs` `UnpoisonExt`).

## 7. Phase map (M4)

| Phase | Delivers | Fixes |
|---|---|---|
| 01 events-rotation | dated segments + shared streaming readers + retention | D4 |
| 02 token-estimation | `context/estimate.rs`, calibration scale, restart blind spot | D15 (partial), D10 (partial) |
| 03 budget-compaction | `[compaction]` config, budget cut + hysteresis, boundary synthesis, `[BUDGET]` rewording | D8, D9, D12, D14 |
| 04 append-only-archive | archive file, honest placeholders, retention | D1, D2 (partial) |
| 05 epoch-records | epoch chain, per-span tallies, regenerated head, keep-newest narrative | D3 (partial), D5, D6, D7 |
| 06 ledger-rollups | session ledger + chapter rollups | D3 |
| 07 recall-context | the recall tool + placeholder/prompt wiring | D2 |
| 08 async-compaction | background epoch build + emergency path; narrative default on | D11 |
| 09 session-meta-persistence | meta.json + boundary-safe reload | D10 |
| 10 ghost-and-memory | ghost working-set coverage; opt-in memory extraction | D13 |

## 8. Future work (explicitly out of M4 scope)

- **FTS5 transcript index** (rusqlite dependency) replacing the grep scan in
  `recall_context`; would also un-stub `src/memory/index.rs`.
- **zstd compression** of closed archive/event segments (new dependency).
- **Cross-session recall** (querying other sessions' archives) — needs a
  privacy/namespace story first.
- Accurate provider-reported context windows (query the provider's model
  listing instead of the static table).
