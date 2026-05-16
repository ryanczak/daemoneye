# Cost Accounting — Implementation Plan

## Engineering Standards (mandatory — read first)

These standards apply to **every phase**. PRs that violate them will be sent back.

### Definition of Done

A phase is done only when **all** of the following are true:

1. **All exit criteria for the phase are satisfied** (listed at the end of each phase).
2. **`cargo build` succeeds** in both debug and release profiles.
3. **`cargo clippy --all-targets -- -D warnings` exits zero.** No new warnings, no
   `#[allow(...)]` suppressions added without an inline justification comment.
4. **`cargo test` passes 100%** — no new ignored tests, no flakes. The exact pre-phase
   pass count must be preserved or grow.
5. **`cargo fmt --check`** is clean.
6. **No new runtime dependencies** without an explicit justification in the PR
   description. Prefer the stdlib. If a new crate is unavoidable, it must already
   appear elsewhere in `Cargo.lock`.
7. **Backwards compatibility** — existing `~/.daemoneye/etc/config.toml`,
   `events.jsonl`, schedule store, memory frontmatter, runbook frontmatter, and
   IPC wire formats must continue to load. Add fields with `#[serde(default)]` or
   `Option<T>`; never remove or rename existing fields.
8. **No regressions in token tracking, session persistence, ghost shell lifecycle,
   or status reporting.** Manually verify a chat session end-to-end before
   declaring done.
9. **`CLAUDE.md` updated** if the change introduces a new architectural concept,
   global static, or wire-protocol variant.
10. **Memory updated** in `/home/matt/.claude/projects/-home-matt-src-daemoneye/memory/`
    if the change affects how future agents should reason about cost-related code.

### Code Style

- **Locks**: every `Mutex` / `RwLock` lock call uses `.unwrap_or_log()` (the
  `UnpoisonExt` trait from `src/util.rs`). Never `.unwrap()` directly.
- **File writes**: atomic write (`.de_tmp` → rename) for any file under
  `~/.daemoneye/`. Use the existing helpers in `src/scripts.rs` or
  `src/session_store.rs` as references.
- **Errors**: `anyhow::Result<T>` at module boundaries; typed errors only where
  callers must branch on the failure. Always add context with `.with_context(|| …)`
  when surfacing an I/O error.
- **Logging**: `log::{info, warn, error}` only — no `eprintln!` outside of
  CLI entry points. Cost events use `log_event()` from `daemon/utils.rs`.
- **Naming**: cost fields use `_usd` suffix when the unit is dollars
  (`total_cost_usd`, `input_cost_usd`). Token counts use `_tokens` suffix.
  Per-million-token rates use `_per_mtok` suffix.
- **Numeric types**: token counts are `u32`; costs are `f64` (USD, never cents).
  Justification: integer-cents would require careful scaling across rate
  arithmetic and introduces rounding bugs at sub-cent precision — f64 with USD
  units is the standard across all major provider SDKs.
- **Comments**: follow the existing CLAUDE.md guidance — no narration of WHAT
  the code does, only WHY a non-obvious choice was made. Doc comments on every
  `pub` item.
- **Imports**: group `std` → external crates → `crate::`. No glob imports
  except in `tests` modules.

### Test Coverage

- **Every new module** has a `#[cfg(test)] mod tests` block exercising the
  happy path, at least one edge case, and at least one error path.
- **Unit tests live in the module** they test; integration tests go in
  `tests/integration.rs` only when they exercise IPC, persistence across
  daemon restart, or multi-module orchestration.
- **Tests that mutate `$HOME`** must acquire `crate::TEST_HOME_LOCK` (see
  `src/main.rs`) to prevent parallel test races.
- **Cost calculation tests** must include: zero tokens, all-cache-read tokens,
  all-cache-write tokens, asymmetric input/output costs, and a "missing pricing"
  fallback case.
- **Snapshot tests** for `daemoneye costs` table output — use a deterministic
  fixture of cost events with known totals; assert formatted output line-by-line.
- **Test naming**: `<unit>_<scenario>_<expected>`, e.g.
  `compute_cost_with_cache_read_uses_cache_rate`.
- **No `#[ignore]`** on new tests. If a test needs the daemon running, write
  it as a deterministic unit test against the underlying function instead.

### Documentation

- Every new `pub fn`, `pub struct`, and `pub enum` carries a doc comment.
- The first line is one sentence summarizing the purpose; subsequent lines
  document fields, parameters, return value, and panics/errors.
- Update `CLAUDE.md` when introducing a new global static, new IPC variant,
  or new file layout in `~/.daemoneye/`.

### Out of Scope (for every phase below)

- Budget caps / spend limits (deferred — separate effort).
- Real-time price fetching from provider APIs (no provider exposes a stable
  price endpoint; manual maintenance only).
- Multi-currency support (USD only).
- Per-user / multi-tenant cost partitioning (single-user daemon).

---

## Overview

DaemonEye supports multiple LLM providers and named agents, often running in
parallel (chat sessions + ghost shells + scheduled jobs, each potentially on a
different model). Costs vary by ~100× across providers and by ~10× within a
provider depending on cache behavior. The current `AiUsage { prompt_tokens,
completion_tokens }` is too coarse to compute spend accurately.

This plan adds:

- **Four-bucket token tracking** per AI call: `input`, `output`, `cache_read`,
  `cache_write`. (Anthropic and Gemini both report all four with different
  meanings; OpenAI and local providers report two and zero-fill the rest.)
- **Per-model pricing** declared in `[models.<name>]` config blocks, with
  built-in defaults for known models and explicit $0 for local providers.
- **Per-call cost records** written to `~/.daemoneye/var/log/events.jsonl`
  with full attribution: `session_id`, `agent_name`, `is_ghost`, `parent_job_id`,
  `provider`, `model`.
- **Aggregation** at three levels: per session (live, in memory), per agent
  / provider / day (queried from events.jsonl on demand), running daemon
  total (via `daemoneye status`).
- **User-visible displays**: status bar in the chat UI, `daemoneye costs`
  CLI command, cost summary in catch-up briefs after long detach.

**Ghost shell attribution rule**: named ghost shells (those spawned with
`agent: "foo"`) attribute costs to `agent_name = "foo"`. Unnamed ghost shells
all bucket under `agent_name = "ghost-anonymous"` as a single line item.
Interactive chat sessions use `agent_name = "chat"` unless `/agent switch` is
in effect.

---

## Phase 1 — Pricing schema and default rates

**Goal**: extend `ModelEntry` with cost fields and embed default rates for
known models so cost computation has data to work with. No cost is computed
or displayed yet — this phase is purely the data foundation.

### Files

- `src/config.rs` — extend `ModelEntry`, add `default_pricing()` helper,
  populate `default_models()`.
- `assets/etc/config.toml` — document the new fields in commented examples.
- `tests/integration.rs` — round-trip a config with custom pricing.

### Changes

1. Add to `ModelEntry`:
   ```rust
   /// Per-1M-token input cost in USD. None = unknown (will log warning).
   /// Local providers (lmstudio, ollama) should set this to Some(0.0).
   #[serde(default)]
   pub input_cost_per_mtok: Option<f64>,
   #[serde(default)]
   pub output_cost_per_mtok: Option<f64>,
   /// Cache-read rate (Anthropic ephemeral cache; Gemini implicit cache).
   /// Typically ~10% of input rate for Anthropic.
   #[serde(default)]
   pub cache_read_cost_per_mtok: Option<f64>,
   /// Cache-write rate (Anthropic cache creation; ~125% of input rate).
   #[serde(default)]
   pub cache_write_cost_per_mtok: Option<f64>,
   ```

2. Add `ModelEntry::pricing(&self) -> Pricing` returning a struct that fills
   missing rates from `default_pricing_for(provider, model)`. The struct:
   ```rust
   pub struct Pricing {
       pub input_per_mtok: f64,
       pub output_per_mtok: f64,
       pub cache_read_per_mtok: f64,
       pub cache_write_per_mtok: f64,
       pub source: PricingSource,  // UserConfig / BuiltinDefault / Unknown
   }
   ```

3. Built-in defaults — define `fn default_pricing_for(provider: &str,
   model: &str) -> Option<Pricing>` covering:
   - Anthropic: `claude-sonnet-4-6`, `claude-opus-4-7`, `claude-haiku-4-5-*`
   - OpenAI: `gpt-4o`, `gpt-4o-mini`, `o1`, `o3-mini`
   - Gemini: `gemini-2.5-pro`, `gemini-2.5-flash`
   - Local: `lmstudio`, `ollama` providers → always `Pricing::zero()`
   - Unknown model on known provider → `None` (caller decides; logs warning at
     daemon start for any configured model lacking pricing)

   Exact rates: source from each provider's published pricing page as of
   2026-05-16. Document the source URL in a comment at the top of the
   `default_pricing_for` function.

4. `Config::validate_pricing(&self)` walks `[models.*]` at startup and
   logs `warn!` for any model where pricing cannot be resolved. Called from
   `Config::load()`.

5. `Pricing::zero()` constructor for local providers — returns a `Pricing`
   with all rates set to 0.0 and source = `PricingSource::Local`.

### Tests

- `model_entry_with_explicit_pricing_overrides_defaults`
- `model_entry_without_pricing_falls_back_to_builtin`
- `model_entry_unknown_model_returns_none`
- `local_provider_pricing_is_zero`
- `pricing_partial_override_merges_with_default` (user sets input rate only;
  other three come from defaults)

### Exit criteria

- `cargo test` passes.
- `daemoneye setup` writes the new commented examples to `etc/config.toml`.
- A model with no pricing logs a single warn at startup, not per call.

---

## Phase 2 — Cache token capture and `TokenBreakdown`

**Goal**: replace `AiUsage { prompt_tokens, completion_tokens }` with a
four-bucket breakdown, and update all three backends to populate it correctly.

### Files

- `src/ai/types.rs` — replace `AiUsage` with `TokenBreakdown`.
- `src/ai/backends/anthropic.rs` — parse `cache_creation_input_tokens` and
  `cache_read_input_tokens` from the `message_delta` / `message_start` events.
- `src/ai/backends/gemini.rs` — parse `cachedContentTokenCount` from
  `usageMetadata`.
- `src/ai/backends/openai.rs` — parse `prompt_tokens_details.cached_tokens`
  from `usage`.
- Every consumer of `AiUsage` — daemon stream loop, status reporting,
  session JSONL writes.

### Changes

1. New struct:
   ```rust
   #[derive(Debug, Clone, Default, Serialize, Deserialize)]
   pub struct TokenBreakdown {
       pub input_tokens: u32,        // billed at input rate
       pub output_tokens: u32,       // billed at output rate
       pub cache_read_tokens: u32,   // billed at cache-read rate
       pub cache_write_tokens: u32,  // billed at cache-write rate
   }
   impl TokenBreakdown {
       pub fn total(&self) -> u32 { ... }
       pub fn billable_input(&self) -> u32 { ... }  // input - cache_read - cache_write
   }
   ```

   Note: providers report `prompt_tokens` as the *total* tokens sent (including
   cache reads and writes); the `input_tokens` field here is the **uncached**
   remainder. Backends must subtract cache totals before populating.

2. Replace `AiEvent::Done(AiUsage)` with `AiEvent::Done(TokenBreakdown)`.

3. Per-backend parsing:
   - **Anthropic**: read `cache_creation_input_tokens` and
     `cache_read_input_tokens` from `message_start.message.usage`; subtract
     both from the `input_tokens` field.
   - **OpenAI**: `usage.prompt_tokens_details.cached_tokens` → `cache_read_tokens`;
     OpenAI has no cache_write concept (always 0).
   - **Gemini**: `usageMetadata.cachedContentTokenCount` → `cache_read_tokens`;
     Gemini has no separate cache_write (always 0; implicit caching is
     transparent to billing).

4. Backwards compatibility: session JSONL files contain historical messages
   with `AiUsage` shape. Add a `#[serde(rename = "AiUsage")]` deserializer
   helper or write a custom `Deserialize` that accepts both shapes.

5. Anywhere that displays "prompt tokens" in the UI (status bar
   `prompt_tokens / context_window`) continues to show
   `breakdown.input_tokens + breakdown.cache_read_tokens + breakdown.cache_write_tokens`
   so the context-window math remains correct.

### Tests

- `token_breakdown_total_sums_all_buckets`
- `anthropic_parses_cache_creation_and_read_tokens`
- `openai_parses_cached_tokens_from_details`
- `gemini_parses_cached_content_token_count`
- `legacy_ai_usage_jsonl_deserializes_into_token_breakdown`
- `token_breakdown_zero_cache_when_provider_omits_field`

### Exit criteria

- All backends produce non-zero `cache_read_tokens` after a warm prompt
  (verify manually with a multi-turn chat).
- Existing session JSONL files load without error.
- The status bar still shows correct prompt-token totals.

---

## Phase 3 — Cost computation and event emission

**Goal**: compute the per-call cost from `TokenBreakdown` + `Pricing` and emit
a structured `ai_cost` record to `events.jsonl` after each AI completion.

### Files

- `src/cost.rs` *(new)* — `Cost`, `CostRecord`, `compute_cost()`.
- `src/daemon/stream.rs` — call `compute_cost()` at the end of each AI turn
  and emit the event.
- `src/daemon/executor/mod.rs` — pass agent attribution context down to
  the stream loop.
- `tests/integration.rs` — round-trip a `CostRecord` through events.jsonl.

### Changes

1. New module `src/cost.rs`:
   ```rust
   #[derive(Debug, Clone, Default, Serialize, Deserialize)]
   pub struct Cost {
       pub input_cost_usd: f64,
       pub output_cost_usd: f64,
       pub cache_read_cost_usd: f64,
       pub cache_write_cost_usd: f64,
       pub total_cost_usd: f64,
   }

   pub fn compute_cost(tokens: &TokenBreakdown, pricing: &Pricing) -> Cost { ... }
   ```

2. `CostRecord` is the event-log payload:
   ```rust
   pub struct CostRecord {
       pub timestamp: DateTime<Utc>,
       pub session_id: String,
       pub agent_name: String,        // "chat" | "ghost-anonymous" | <named agent>
       pub is_ghost: bool,
       pub parent_job_id: Option<String>,
       pub provider: String,
       pub model: String,
       pub tokens: TokenBreakdown,
       pub cost: Cost,
       pub pricing_source: PricingSource,  // BuiltinDefault / UserConfig / Local / Unknown
   }
   ```

3. Attribution rules in `executor/mod.rs`:
   - Interactive chat session → `agent_name = "chat"`.
   - Chat after `/agent switch foo` → `agent_name = "foo"`.
   - Named ghost shell (`agent: "foo"` in runbook or spawn call) →
     `agent_name = "foo"`, `is_ghost = true`.
   - Unnamed ghost shell → `agent_name = "ghost-anonymous"`, `is_ghost = true`.
   - Spawned-by-coordinator ghost → `parent_job_id = <coordinator job_id>`.

4. Event emission: at the `AiEvent::Done(breakdown)` arm in
   `run_conversation_loop()`, build the `CostRecord` and call
   `log_event("ai_cost", serde_json::to_value(&record)?)`.

5. Unknown-pricing handling: if `Pricing::source == Unknown`, the record is
   still emitted with `cost = Cost::default()` (all zeros) and a
   `pricing_source: "unknown"` field so aggregation queries can surface
   "$X spent, $Y untracked" rather than silently undercounting.

### Tests

- `compute_cost_zero_tokens_is_zero`
- `compute_cost_basic_input_output`
- `compute_cost_with_cache_read_uses_cache_rate`
- `compute_cost_with_cache_write_uses_cache_write_rate`
- `compute_cost_anthropic_sonnet_4_6_sample_turn` (deterministic fixture
  matching a real Anthropic invoice line item)
- `cost_record_serializes_to_events_jsonl_round_trip`
- `unknown_pricing_emits_zero_cost_record_with_flag`
- `ghost_unnamed_attributes_to_ghost_anonymous`
- `ghost_named_attributes_to_agent_name`

### Exit criteria

- A 10-turn chat session produces exactly 10 `ai_cost` events in
  `events.jsonl`.
- Sum of `total_cost_usd` across all events matches manual calculation from
  token counts × pricing.
- Ghost shells produce events with `is_ghost: true`.

---

## Phase 4 — Session-level aggregation and IPC

**Goal**: maintain running cost totals in `SessionEntry` for live display, and
expose them over IPC for the status bar and `daemoneye status`.

### Files

- `src/daemon/session.rs` (or wherever `SessionEntry` lives) — add cost
  aggregation fields.
- `src/ipc.rs` — extend `Response::SessionInfo` and `Response::DaemonStatus`.
- `src/cli/commands/stream.rs` — consume the new fields.
- `src/cli/commands/status.rs` — display the new fields.

### Changes

1. Add to `SessionEntry`:
   ```rust
   /// Cumulative cost of this session so far. Reset on /clear or new session.
   pub cost_usd: f64,
   /// Per-agent breakdown for this session (key = agent_name).
   pub cost_by_agent: HashMap<String, f64>,
   /// Whether any AI call in this session had Unknown pricing — surfaces a
   /// warning in the UI.
   pub has_untracked_cost: bool,
   ```

2. Update on every `ai_cost` emission in `stream.rs`:
   ```rust
   session.cost_usd += record.cost.total_cost_usd;
   *session.cost_by_agent.entry(record.agent_name.clone()).or_insert(0.0)
       += record.cost.total_cost_usd;
   if record.pricing_source == PricingSource::Unknown {
       session.has_untracked_cost = true;
   }
   ```

3. Extend `Response::SessionInfo`:
   ```rust
   pub session_cost_usd: f64,
   pub has_untracked_cost: bool,
   ```
   Emitted on every `Ask` response so the status bar can refresh.

4. Extend `Response::DaemonStatus`:
   ```rust
   pub daemon_session_costs: Vec<(String, f64)>,  // (session_id, cost_usd)
   pub daemon_total_cost_today_usd: f64,          // aggregated from events.jsonl
   ```

5. The daemon-total field is computed lazily on `Request::Status` by scanning
   today's `ai_cost` events. Cache the result for 5s to avoid re-scanning on
   every status call.

### Tests

- `session_entry_accumulates_cost_across_turns`
- `session_entry_per_agent_split` (a session with `/agent switch foo` mid-flow)
- `daemon_total_cost_today_aggregates_events_jsonl`
- `daemon_total_cost_today_excludes_yesterday`
- `unknown_pricing_sets_has_untracked_cost`

### Exit criteria

- After a 5-turn chat, `Response::SessionInfo.session_cost_usd` is non-zero
  and matches the sum of the turn-level cost events.
- `daemoneye status` shows `Daemon cost today: $X.XX` (next phase displays
  it; this phase just exposes it over IPC).

---

## Phase 5 — `daemoneye costs` CLI

**Goal**: a CLI command that slices `events.jsonl` cost data by day, agent,
provider, or session.

### Files

- `src/cli/commands/costs.rs` *(new)*.
- `src/cli/mod.rs` — register the `costs` subcommand.
- `src/main.rs` — route.

### Command surface

```
daemoneye costs                            # last 7 days, by day
daemoneye costs --since 2026-05-01         # date range
daemoneye costs --since 2026-05-01 --until 2026-05-16
daemoneye costs --by agent                 # group by agent_name
daemoneye costs --by provider              # group by provider
daemoneye costs --by model                 # group by model
daemoneye costs --by session               # group by session_id
daemoneye costs --by day                   # default
daemoneye costs --agent architect          # filter to one agent
daemoneye costs --json                     # machine-readable output
```

Default output (human-readable, table format with right-aligned dollar
columns):
```
Cost summary — last 7 days
                              Calls   Tokens (in/out/cache)   Cost (USD)
2026-05-10                       42   58k / 12k / 4k           $0.41
2026-05-11                       18   23k / 5k / 0             $0.18
…
                              ─────   ─────────────────────   ─────────
Total                           120   180k / 38k / 12k         $1.23
Untracked (unknown pricing):    3 calls, ~45k tokens
```

JSON output emits one record per group with full token/cost breakdown.

### Implementation notes

- No daemon round-trip — the CLI reads `events.jsonl` directly. This is
  consistent with `daemoneye logs` and means `costs` works even when the
  daemon is down.
- Use streaming line-by-line reading (`BufReader`) — never load the whole
  file. The file can grow to 100MB+ over months.
- Date filtering is done by parsing the `timestamp` field per line; stop
  reading early once timestamps exceed `--until` if events are append-only
  in time order (they are — `log_event` writes synchronously).
- Sort the final grouped output by total cost descending for non-day
  groupings; by date ascending for day grouping.
- Locale-independent number formatting; always use `.` as the decimal
  separator.

### Tests

- `cli_costs_default_groups_by_day`
- `cli_costs_by_agent_aggregates_correctly`
- `cli_costs_filter_by_agent_excludes_others`
- `cli_costs_date_range_inclusive`
- `cli_costs_json_output_matches_schema`
- `cli_costs_untracked_calls_surface_in_summary`
- `cli_costs_empty_events_file_shows_zero_total`
- Snapshot test on a deterministic fixture (~20 events) for default output.

### Exit criteria

- `daemoneye costs` produces correct output against a fixture events file.
- Performance: reading 100k events completes in <500ms on a modern laptop.
- `--json` output round-trips through `serde_json::from_value`.

---

## Phase 6 — Status bar and `daemoneye status` display

**Goal**: surface running cost in the chat UI and in `daemoneye status`.

### Files

- `src/cli/render.rs` — add cost segment to the status bar.
- `src/cli/commands/stream.rs` — pass cost through `StatusBarState`.
- `src/cli/commands/status.rs` — render `daemon_total_cost_today_usd`.

### Changes

1. Add `cost_usd: f64` and `has_untracked: bool` to `StatusBarState`.
   Update from `Response::SessionInfo` on every refresh.

2. Status bar format — append a cost segment to the existing right-side
   info block:
   ```
   turn 7 · 12.3k / 200k (6%) · tools: 4 · $0.08
   ```
   With untracked flag:
   ```
   turn 7 · 12.3k / 200k (6%) · tools: 4 · $0.08+
   ```
   (The `+` indicates at least one call had unknown pricing — full detail
   available via `daemoneye costs`.)

3. `daemoneye status` output gains a cost section:
   ```
   Cost (today)
     Total:           $1.23
     By provider:     anthropic $0.95 · gemini $0.21 · openai $0.07
     By agent:        chat $0.78 · architect $0.30 · ghost-anonymous $0.15
   ```
   Field width: dollar columns right-aligned to widest entry. Hide the
   provider/agent breakdown if total is $0.

### Tests

- `status_bar_renders_cost_segment`
- `status_bar_renders_untracked_marker_when_flag_set`
- `daemoneye_status_renders_cost_breakdown`
- `daemoneye_status_hides_breakdown_when_zero`

### Exit criteria

- Status bar updates cost in real time as turns complete.
- `daemoneye status` shows the breakdown correctly against a fixture.

---

## Phase 7 — Catch-up brief cost integration

**Goal**: when the user reattaches after a ≥30s detach during which ghost
shells ran, the catch-up brief includes a one-line cost summary.

### Files

- `src/daemon/server.rs` — `build_catchup_brief()`.
- `src/daemon/utils.rs` — helper to sum cost between two timestamps.

### Changes

1. Add `sum_cost_between(from: DateTime<Utc>, to: DateTime<Utc>) -> CostSummary`
   helper in `daemon/utils.rs`. Streams `events.jsonl`, filters by timestamp,
   sums per-agent.

2. In `build_catchup_brief()`, after computing the existing event tallies:
   - Compute cost incurred during the detach window:
     `sum_cost_between(session.last_detach.unwrap(), now)`
   - If the total is > $0.001, append a line to the brief:
     ```
     Cost during detach: $0.34 (architect $0.20 · ghost-anonymous $0.14)
     ```
   - If total is exactly $0.00 (only local providers ran), still surface as:
     ```
     Cost during detach: $0.00 (local providers only)
     ```
   - If no AI calls ran during detach, omit the line entirely.

3. Untracked spend during detach gets a `+` marker as in the status bar.

### Tests

- `catchup_brief_includes_cost_when_ghosts_ran`
- `catchup_brief_omits_cost_line_when_no_ai_calls`
- `catchup_brief_local_only_shows_zero_explicitly`
- `catchup_brief_marks_untracked_spend`
- `sum_cost_between_excludes_events_outside_window`

### Exit criteria

- A staged detach test (mock `last_detach` to 1 hour ago, log known-cost
  events, reattach) produces the expected line in the brief.
- The line is omitted when no AI activity occurred during the detach window.

---

## Final acceptance

When all seven phases are complete and merged:

1. Run a real multi-agent scenario (chat session + 2 ghost shells with
   different providers) and verify:
   - Status bar shows accumulating cost.
   - `daemoneye costs` shows the day's spend broken down by agent and
     provider.
   - `events.jsonl` contains one `ai_cost` record per AI completion with
     full attribution.
   - `daemoneye status` reports today's daemon-wide total.
2. Detach for 30+ seconds while a ghost shell runs; reattach and verify
   the catch-up brief includes the cost line.
3. Reload the daemon and verify that historical `ai_cost` records remain
   queryable via `daemoneye costs --since <past-date>`.
4. Run `cargo test`, `cargo clippy --all-targets -- -D warnings`,
   `cargo fmt --check` — all clean.
5. Update `CLAUDE.md` with: the `src/cost.rs` module, the new IPC fields,
   the `ai_cost` event-log record format, and the attribution rules
   (chat / agent / ghost-anonymous / named ghost).
