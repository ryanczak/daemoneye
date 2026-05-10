# DaemonEye Roadmap & Project Review

*Drafted 2026-05-09 against `master` at v0.9.1.*

This document is a candid review of the project — both code and product —
followed by a prioritised list of opportunities. It is not a commitment;
it is a working artifact for future planning sessions.

---

## 1. State of the Project

DaemonEye has matured into a feature-rich, opinionated AI operations
assistant with a coherent product vision (graduated trust, terminal-native,
audit-first) and substantial implementation depth.

| Metric | Value |
|---|---|
| Version | 0.9.1 (heading toward 1.0) |
| Source | ~38,300 lines of Rust across 72 files |
| Tests | 596 passing (586 unit + 10 integration) |
| Targets | Linux only, tmux 2.6+, Rust 1.79+ (edition 2024) |
| AI providers | Anthropic, OpenAI, Gemini, Ollama, LM Studio |

**Recent themes (per CHANGELOG):**
- 0.7 — pane discovery & persistence, pipe-pane log, ANSI semantic markers
- 0.8 — `daemoneye status`, circuit breaker, supervised tasks, catch-up brief, cross-session context
- 0.9 — Ghost-shell architecture convergence, scheduled ghosts, sudoers tooling
- 0.9.1 / Unreleased — structured memory frontmatter, configurable tool limits, named session persistence

The product is doing a lot of things right: every approval path has a
visible UI, ghost shells are gated behind two independent locks, the
masking filter is non-disableable, and auditability is structural rather
than bolted on.

---

## 2. Code Quality Assessment

### 2.1 What's working well

- **Module layout is clean.** The `daemon/`, `cli/`, `tmux/`, and `ai/`
  trees are well-bounded; recent refactors (server.rs split, executor
  decomposition, stream.rs extraction) show ongoing investment in
  shape.
- **Concurrency is disciplined.** `unwrap_or_log()` poison recovery is
  applied uniformly. Background tasks run under `supervise()` with
  exponential backoff. Channels (`broadcast`, `mpsc`) are used in
  preference to shared mutable state where streaming is involved.
- **Security primitives are first-class.** The masking filter has its
  own atomic counters per category, reported in `daemoneye status`.
  Ghost-shell policy is enforced at multiple layers
  (`GhostPolicy::is_safe`, `auto_approve_scripts`, sudoers gating).
- **Documentation is excellent.** PRODUCT_DEFINITION, REQUIREMENTS,
  ARCHITECTURE, CHANGELOG, and CLAUDE.md are kept in sync to a degree
  that is unusual for a single-maintainer project.
- **Tests are inline and granular.** `_tests.rs` siblings keep large
  modules' tests organised; pure helpers (`format_other_sessions`,
  `plan_gc_actions`, `parse_ghost_trigger`) are factored out
  specifically for testability.

### 2.2 Active issues (worth fixing soon)

| # | Issue | Severity | Pointer |
|---|---|---|---|
| ~~C1~~ | ~~**CI lint job is broken on a fresh toolchain.**~~ | **Fixed** | — |
| ~~C2~~ | ~~**12 `cargo test --no-run` warnings.**~~ | **Fixed** | — |
| ~~C3~~ | ~~**Two competing logging facades.**~~ | **Fixed** | — |
| ~~C4~~ | ~~**125+ `unwrap()` calls outside test code.**~~ | **Fixed** | — |
| C5 | **Files trending past 1000 lines.** `server.rs` (1634), `config.rs` (1381), `background.rs` (1369), `render.rs` (1245), `webhook.rs` (1207), `cli/commands/mod.rs` (1199). Each has natural seams (e.g. `config.rs` has ~60 inline test cases). | Low | Largest files |
| C6 | **`tests/integration.rs` exists but is shallow** — covers serde round-trips and on-disk format only. The IPC `Request`/`Response` types are re-declared locally rather than imported from the crate, so production drift goes undetected; schedule and session tests hand-write JSON instead of calling `ScheduleStore` / `session_store`; no end-to-end Ask → ToolCall → Result loop. See Phase A.5 below. | Medium | `tests/integration.rs` |
| ~~C7~~ | ~~**Stringly-typed tool dispatch.**~~ `dispatch_tool_event` parses JSON arg names; a typo in a backend's tool definition surfaces as a runtime error rather than a compile-time miss.~~ | **Fixed** | `src/ai/tools.rs` |
| C8 | **`anyhow` everywhere; no `thiserror` at module boundaries.** Recovery decisions cannot be made by callers — every error is opaque. | Low | repo-wide |

~~**Recommendation:** treat C1–C3 as a brief hygiene sprint; they are
small fixes whose absence undermines confidence in the rest of the
project. The CI green badge should mean something.~~

**Phase A complete (2026-05-09):** C1 (7 clippy fixes), C2 (12 warnings cleared), C3/R10 (logging consolidated to `log` crate), C4 (1 `lock().unwrap()` in production replaced with `unwrap_or_log()`), C7 (stringly-typed tool dispatch replaced with typed `Deserialize` structs + `ToolArgs` trait; `dispatch_roundtrip_all_tools` test verifies coverage). CI green on fresh toolchain. C6 partially addressed — see Phase A.5.

---

## 3. Product / Feature Opportunities

Each opportunity is ranked by **impact × fit-with-design** rather than
implementation cost. Items are deduplicated against `REQUIREMENTS.md`
and the `[Unreleased]` section of CHANGELOG.

### 3.1 Refinements (existing features done better)

#### R1. Prompt caching for Anthropic and Gemini
The system prompt, `## Available Knowledge` manifest, and runbook
content are stable across turns within a session — ideal `cache_control`
candidates. Today they are re-tokenised on every turn.
**Impact:** 30–80 % cost reduction on long conversations and watchdog
loops; faster TTFB. **Fit:** zero behavioural change.

#### R2. Cost & token telemetry
`UsageUpdate` already tracks prompt tokens per turn. Add a
`var/log/usage.jsonl` and a `daemoneye costs [--day | --week]` view.
Add `[budget] max_cost_per_day` as a daemon-wide ceiling that opens
the circuit breaker when crossed.
**Fit:** matches DaemonEye's audit-first design.

#### R3. Plugin architecture (FR-1.5)
Listed in REQUIREMENTS as MUST, but not implemented. Define a
versioned `AlertProvider` and `ToolProvider` trait that loads `.so`
plugins from `~/.daemoneye/lib/`. Start with a built-in plugin
crate so the API is forced through a single seam.

#### R4. Prompt library subcommand (FR-1.3.2)
Config + file loading exist; the `daemoneye prompts` listing
subcommand and the `/prompt <name>` slash command are noted as
"pending" in REQUIREMENTS. Close the gap.

#### R5. `daemoneye doctor`
Check tmux version, sudoers rules, webhook port, AI connectivity,
orphan ghost windows, schedules.json integrity, masking config
parse, and emit a single PASS/FAIL report. Use it from `setup`
post-install.

#### R6. Replay & forensics CLI
`daemoneye replay <session_id>` reconstructs an incident timeline
from `events.jsonl`. `daemoneye explain <event_id>` shows the
preceding three commands and the AI reasoning around an audit
record. Closes the loop on the existing structured-event log.

#### R7. Configurable webhook clustering / dedup keys
Today dedup is by Alertmanager fingerprint. Add `[webhook]
dedup_cluster_by = ["alertname", "service"]` plus a counter so a
storm shows up as `× 47` instead of 47 line items in chat.

#### R8. Knowledge freshness & TTL enforcement
`expires` field exists on memories but isn't enforced. Add a
weekly GC pass that flags stale entries and offers a `/memory
refresh <key>` review flow.

#### R9. Blast-radius hint on approval prompts
When the AI proposes a destructive command (`rm`, `kill -9`,
`drop table`, `systemctl stop`), the approval prompt should
display a one-line impact estimate (file count + size, process
name + uptime, etc.) computed by the daemon before the prompt is
sent. Operators approve or deny faster, with better information.

#### R10. ~~Custom log facade consolidation~~
~~Pick one of `log::warn!` / `log_warn!` and delete the other (see C3).~~
**Done:** `log` crate retained; `src/log.rs` deleted (2026-05-09).

### 3.2 Innovations (new directions that fit the design)

#### I1. Fleet operations
DaemonEye is single-host today. Add `daemoneye fleet add <host>`
and a `[fleet]` config section. `/fleet list`, `/fleet run <cmd>`,
and `/fleet rollout <runbook>` execute against a registry of
remote hosts via SSH or via DaemonEye instances co-operating over
a shared webhook. The runbook is the unit of fleet automation.
**Fit:** Workflow 5 in PRODUCT_DEFINITION already promises this.

#### I2. Semantic knowledge search via embeddings
Replace `search.rs` keyword matching with locally-computed
embeddings (ONNX Runtime + a small sentence-transformer model).
"DB connection refused" will surface a `postgres-failover`
runbook even with no shared keyword. Embed on write; cache in
`~/.daemoneye/var/index/`.

#### I3. Auto-learned runbooks from successful incidents
After a ghost shell completes successfully, the daemon proposes a
candidate runbook (in `runbooks/_proposed/`) reconstructed from
the ghost's tool calls plus the alert that triggered it. The
operator reviews and accepts on the next attach. Closes the loop:
alert → investigation → automation → reusable runbook.

#### I4. Postmortem auto-generation
At ghost-shell completion, generate a markdown postmortem in
`memory/incidents/` with the timeline, the alert, the root cause
(if explicit), and the resolution. This makes the incidents tier
of the knowledge system actually populate itself.

#### I5. Sandboxed runbook simulator
`daemoneye runbook simulate <name>` runs a ghost shell against a
synthetic alert payload in a throwaway tmux window with execution
disabled (commands log but never run). Operators can rehearse a
runbook before flipping `enabled: true`.

#### I6. Time-of-day & business-hours autonomy gates
Per-runbook frontmatter `enabled_during: "08:00-18:00 America/Los_Angeles"`
and `max_ghost_turns_after_hours: 5`. Lets teams be more cautious
on weekends / overnight without disabling automation entirely.

#### I7. Second-pair-of-eyes review for high-risk runbooks
Frontmatter `requires_review: true` causes the ghost trigger to
post a Slack/PagerDuty notification with an approve/deny button
that gates ghost spawn. The runbook never auto-executes; a human
co-signs. **Fit:** a natural extension of the trust spectrum.

#### I8. Alert provider plugins (Datadog, PagerDuty, OpsGenie, Sentry, Linear/Jira)
The webhook layer is hardcoded for Alertmanager + Grafana +
generic JSON. With R3's plugin architecture, ship first-party
plugins for the major sources, plus an outbound channel so a ghost
shell can resolve a PagerDuty incident or comment on a Linear
ticket as part of remediation.

#### I9. Git-backed shared knowledge store
`runbooks/`, `scripts/`, `memory/knowledge/` are individually
useful but team-scoped knowledge is the killer feature. Allow
`~/.daemoneye/` (or a subset) to be a git checkout; daemon syncs
on startup and after `write_runbook` / `add_memory`. Conflicts
become a real diff review, not a footgun.

#### I10. `--remote` mode for macOS / Windows operators
Many SREs work on macOS or Windows but ssh into Linux. Support a
`daemoneye chat --remote user@host` mode where the daemon runs on
the Linux box and the local CLI is a thin terminal-aware client.
This widens the audience without breaking the Linux-only daemon
guarantee.

#### I11. Batch API for watchdog fleets
A scheduled watchdog that runs on 100 hosts every 5 minutes is a
perfect Anthropic Batch API workload. Add
`use_batch_for_scheduled = true` per model.

#### I12. Audit log signing & SIEM forwarder
`events.jsonl` is the heart of DaemonEye's auditability. Add
optional Ed25519 line signing (key in `~/.daemoneye/etc/audit.key`)
and a `[audit] forward_to = "tcp://siem:5140"` syslog/CEF
forwarder. Compliance use cases unlock.

#### I13. Multi-agent pairing for incidents
A Ghost Shell that gets stuck mid-incident could spawn a
"second-opinion" sibling running a different model and prompt
("be skeptical of the first agent"). Both transcripts land in the
same incident record. Useful when a single model gets fixated.

#### I14. Cross-session memory transfer
When the AI realises one session's discovery (e.g. "this host runs
postgres on 5433") is useful to other ongoing sessions, it can
write a knowledge memory and other sessions auto-pick it up via
`relates_to` traversal. Today knowledge is added but never
proactively surfaced across concurrent sessions.

#### I15. Local thought-cache replay
For Ollama / LM Studio users on commodity hardware, cache the
last N turns' generated outputs keyed on `(model, system_prompt,
user_message)`. Useful in dev/testing and during model swaps.

#### I16. Onboarding skill: `daemoneye init-team`
Bootstrap a starter set of runbooks (disk, memory, nginx, postgres),
example knowledge memories, and a sample sudoers config based on
detected services. Time-to-first-ghost-shell drops from "a day"
to "ten minutes".

---

## 4. Suggested Sequencing

Ordered by ratio of (impact × fit) to effort.

### Phase A — Hygiene sprint (days, not weeks) **✅ COMPLETE**
1. ~~**C1** — fix the 7 clippy errors and re-run on a fresh toolchain.~~
2. ~~**C2** — clear unused-import warnings; rename the snake_case test.~~
3. ~~**C3 / R10** — pick `log` or `log_*!` macros; delete the other.~~
4. ~~**C6** — add a minimal `tests/integration_*.rs` for the
    webhook → ghost spawn path and the chat `Ask → ToolCall →
    Approval → Result` loop.~~

All four items landed 2026-05-09. 596 tests pass (586 unit + 10 integration), clippy clean, zero warnings.

### Phase A.5 — Finish the integration test story (1–2 days)

The `tests/integration.rs` suite that landed in Phase A is real and useful, but it tops out at serde round-trips and on-disk format checks. Three structural issues prevent it from catching real regressions:

- **Production types are re-declared locally** in `tests/integration.rs:30-118` rather than imported from the crate. If `ipc.rs` adds a field or renames a variant, the test will continue to pass against its stale local copy. This is the opposite of what a contract test should do.
- **Persistence tests hand-roll JSON** instead of calling `ScheduleStore::save()` / `session_store::save_session()`. A refactor of the on-disk format would not break these tests.
- **No daemon-loop test exists.** The original C6 concern was the chat tool-loop and the webhook → ghost-spawn pipeline. Neither path is exercised end-to-end.

These items should land before any Phase B feature work — they are the assertion harness everything later depends on.

A1. ~~**Convert `daemoneye` to a library + binary.**~~ Add `src/lib.rs` with at minimum `pub use ipc; pub use scheduler; pub use session_store; pub use config;`. The binary stays a thin shim. This unblocks every test below and is also a precondition for plugin work in Phase E (R3 / I8). **Done: 2026-05-09.** `src/lib.rs` created with `pub mod` for test-accessible modules (`ai`, `config`, `daemon`, `ipc`, `scheduler`, `scripts`, `session_store`, `webhook`) and `pub(crate) mod` for internal modules (`cli`, `header`, `manifest`, `memory`, `pane_prefs`, `runbook`, `search`, `sys_context`, `tmux`, `util`). `src/main.rs` converted to a thin shim (`use daemoneye::{...}`). `TEST_HOME_LOCK` moved to `lib.rs`. 597 tests pass, zero warnings.
A2. **Replace local IPC enums with `daemoneye::ipc::*`.** Round-trip tests now catch schema drift automatically. Delete the duplicated `Request` / `Response` definitions from `tests/integration.rs`.
A3. **Persistence tests via real APIs.** Schedule and session tests should call `ScheduleStore::save_atomic()` and `session_store::save_session()` instead of writing JSON by hand, then assert against the loaded result. Same for event-log entries — go through `daemon::utils::log_event()` rather than synthesising lines.
A4. **One real loop test.** Spawn a daemon process bound to a tempdir socket. Verify `Request::Ping` → `Response::Ok`, then `Request::Status` → `Response::DaemonStatus` shape. ~50 lines, but it covers an entire category of regressions (socket setup, IPC framing, lifecycle) that A1–A3 cannot.
A5. **One webhook → audit-log test.** POST a synthetic Alertmanager payload to an in-process axum router (no socket bind required, axum is testable as a `Service`), assert that `events.jsonl` contains a `webhook_alert` entry with the expected fingerprint and that masking ran. This is the highest-value integration test we can write cheaply because it exercises the dedup map, masking filter, and event logger in a single path.
A6. **Mark C6 fully closed only after A1–A5 land.** The current row in §2.2 should stay `Medium` severity until then.

**Exit criteria:** zero local re-declarations of production types in `tests/`; integration suite imports `daemoneye::*`; at least one daemon-process test and one webhook-pipeline test in CI; total test count ≥ 600.

### Phase B — Quick product wins (weeks)
5. **R1** — Anthropic prompt caching on system prompt + manifest.
6. **R2** — usage log + `daemoneye costs`.
7. **R5** — `daemoneye doctor`.
8. **R4** — finish the prompt library subcommand.
9. **R7** — webhook clustering.

### Phase C — Trust & observability (a release)
10. **R6** — replay & forensics CLI.
11. **I12** — audit log signing + SIEM forwarder.
12. **R9** — blast-radius hint.
13. **I7** — second-pair-of-eyes review path.

### Phase D — Knowledge step-change (a release)
14. **I4** — automatic postmortem generation.
15. **I3** — auto-learned runbooks.
16. **I2** — semantic search via embeddings.
17. **I9** — git-backed knowledge sync.

### Phase E — Scale story (next major version)
18. **R3 + I8** — plugin architecture and first-party alert
    provider plugins.
19. **I1** — fleet operations on top of the runbook abstraction.
20. **I11** — Batch API for scheduled watchdogs.
21. **I10** — `--remote` mode for macOS/Windows operators.

### Phase F — Speculative
22. **I5** — runbook simulator.
23. **I6** — business-hours gates.
24. **I13** — multi-agent pairing.
25. **I14 / I15 / I16** — round-out items.

---

## 5. Non-goals to be explicit about

A few things that are tempting but probably *don't* fit DaemonEye's
design philosophy and are best deferred or declined:

- **A web UI.** DaemonEye is terminal-native by deliberate choice;
  a web dashboard would split focus and dilute the value
  proposition. Use existing tools (`grep`, `jq`, Grafana) on the
  audit log instead.
- **Long-running agents that own infrastructure.** The whole point
  of the trust spectrum is that an agent has to earn each rung.
  Avoid features that flatten the spectrum.
- **"Auto-pilot mode" / disable approvals globally.** Even when
  every class is session-approved, a Ctrl+C reset is cheap. A
  global kill switch that disables the audit + approval system is
  not on the roadmap.
- **Cross-host orchestration without runbooks.** Fleet operations
  (I1) are scoped to runbook-mediated execution. Free-form `ssh
  $host $cmd` against fleets is a footgun.

---

## 6. Open questions

- Should knowledge memories converge on a single backend (sqlite +
  embeddings) or stay file-per-entry? File-per-entry is grep- and
  git-friendly; sqlite is faster and supports better indexing. The
  current design defends file-per-entry — confirm before I2.
- Is "ghost shells call other ghost shells" desirable? `spawn_ghost_shell`
  exists today as an AI tool, but cross-spawn semantics
  (capacity, parent/child accounting) are not fully specified.
- What is the smallest viable "team mode"? Git-backed knowledge
  (I9) is the obvious step, but RBAC and per-operator sudoers
  policy are larger questions.

---

*This roadmap should be re-reviewed after Phase A lands. Phase B
ordering is likely to shift once cost telemetry (R2) reveals where
real spend lives.*
