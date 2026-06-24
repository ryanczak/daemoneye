# DaemonEye Roadmap & Project Review

*Drafted 2026-05-09 against `master` at v0.9.1.*
*Revised 2026-06-23 against `master` at v0.9.7 — refreshed metrics, marked R2/R4 done, recorded M1 completion. See the "Status as of 2026-06-23" note in §3.*

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
| Version | 0.9.7 (heading toward 1.0) |
| Source | ~47,000 lines of Rust across 82 files |
| Tests | 785 passing + 1 ignored |
| Targets | Linux only, tmux 2.6+, Rust 1.79+ (edition 2024) |
| AI providers | Anthropic, OpenAI, Gemini, Ollama, LM Studio |

**Recent themes (per CHANGELOG):**
- 0.7 — pane discovery & persistence, pipe-pane log, ANSI semantic markers
- 0.8 — `daemoneye status`, circuit breaker, supervised tasks, catch-up brief, cross-session context
- 0.9 — Ghost-shell architecture convergence, scheduled ghosts, sudoers tooling
- 0.9.1 — structured memory frontmatter, configurable tool limits, named session persistence
- 0.9.x — cost accounting (`daemoneye costs`, closes R2), `daemoneye prompts` + `/prompt` (closes R4)
- **M1 (Agent Tooling Improvements)** — completed 2026-06-23; all 11 phases done. Remote
  execution model (daemon-host owns artifacts, remotes are execution targets only), script-exec
  hardening, namespace access control, pane-targeting + completion-detection correctness,
  error-suppression audit, tmux surface + safe verbs, on-demand tool loading. **Orthogonal to the
  R/I feature list below — closed no roadmap items.**

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
| C5 | **Files trending past 1000 lines — and growing.** As of v0.9.7, 11 files exceed 1000 lines: `ai/tools.rs` (2232), `server.rs` (1976), `config.rs` (1631), `executor/file_ops.rs` (1475), `ai/types.rs` (1413), `background.rs` (1369), `executor/knowledge.rs` (1341), `render.rs` (1305), `webhook.rs` (1210), `executor/foreground.rs` (1192), `cli/commands/mod.rs` (1181). Each has natural seams (e.g. `config.rs` has ~60 inline test cases). | Low | Largest files |
| ~~C6~~ | ~~**`tests/integration.rs` exists but is shallow**~~ — covers serde round-trips and on-disk format only. The IPC `Request`/`Response` types are re-declared locally rather than imported from the crate, so production drift goes undetected; schedule and session tests hand-write JSON instead of calling `ScheduleStore` / `session_store`; no end-to-end Ask → ToolCall → Result loop. See Phase A.5 below.~~ | **Fixed** | `tests/integration.rs` |
| ~~C7~~ | ~~**Stringly-typed tool dispatch.**~~ `dispatch_tool_event` parses JSON arg names; a typo in a backend's tool definition surfaces as a runtime error rather than a compile-time miss.~~ | **Fixed** | `src/ai/tools.rs` |
| C8 | **`anyhow` everywhere; no `thiserror` at module boundaries.** Recovery decisions cannot be made by callers — every error is opaque. Latent, not active: the codebase pre-classifies the one boundary that matters (HTTP status in `send_with_retry_inner` decides retry *before* the error is built), and the only error-string match in the tree is a test (`session_store_tests.rs:141`). The fix is *not* a repo-wide migration — define a small `thiserror` `AiError` enum (`CircuitOpen`/`RateLimited`/`Auth`/`BadRequest`/`Server`/`Network`) so callers can branch (e.g. `doctor` reporting *why* a probe failed), and keep `anyhow` as the glue type elsewhere. Best done as a rider on the next AI-touching feature (R1 or R5), not on its own. | Low | repo-wide |

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

#### R2. ~~Cost & token telemetry~~
**Done (v0.9.x).** `src/cost.rs` computes per-call USD cost from
`TokenBreakdown` + `Pricing`; cost data is written to `events.jsonl`;
`daemoneye costs --group-by day|agent|provider|model|session` aggregates
it (no daemon round-trip — reads the log directly); cost appears in
`daemoneye status`. **Remaining gap:** the `[budget] max_cost_per_day`
ceiling that opens the circuit breaker is *not* implemented — track as a
follow-up if a spend cap is wanted.

#### R3. Plugin architecture (FR-1.5)
Listed in REQUIREMENTS as MUST, but not implemented. Define a
versioned `AlertProvider` and `ToolProvider` trait that loads `.so`
plugins from `~/.daemoneye/lib/`. Start with a built-in plugin
crate so the API is forced through a single seam.

#### R4. ~~Prompt library subcommand (FR-1.3.2)~~
**Done.** `daemoneye prompts` listing subcommand (`Commands::Prompts`
in `main.rs`) and the `/prompt <name>` slash command (`cli/commands/mod.rs`)
both ship.

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

### Phase A.7 — Post-implementation cleanup (1 day) **✅ COMPLETE**

Post-A.5 audit revealed clippy errors and warnings that silently accumulated because `cargo clippy --all-targets -- -D warnings` wasn't gated. All issues resolved 2026-05-10:

A.7.1. ~~**Fix A4 test `read()` bug.**~~ Replaced `stream.read(&mut buf)` (zero-capacity buffer, `unused_io_amount` error) with `BufReader` + `read_line()` for newline-delimited JSON framing. **Done: 2026-05-10.**
A.7.2. ~~**Drop `MutexGuard` before `.await` in A5 test.**~~ Scoped `TEST_HOME_LOCK` guard to only cover the `set_var("HOME", ...)` call, dropped before `process_alert().await`. **Done: 2026-05-10.**
A.7.3. ~~**Move `#[cfg(test)] mod tests` to bottom of file.**~~ Relocated test modules in `src/daemon/ghost.rs`, `src/daemon/session.rs`, `src/daemon/utils.rs`, `src/tmux/session.rs`. **Done: 2026-05-10.**
A.7.4. ~~**Address `too_many_arguments` warnings.**~~ Added `#[allow(clippy::too_many_arguments)]` to `handle_ask()` (16 args), `run_conversation_loop()` (15 args), and `save_session()` (8 args) with rationale comments. **Done: 2026-05-10.**
A.7.5. ~~**Mechanical clippy cleanup.**~~ Fixed `field_reassign_with_default` (3 sites), `new_without_default` → `#[derive(Default)]` (`InputLine`), `collapsible_if` (2 sites), `assertions_on_constants` (2 sites → `const { assert!(..) }`), `module_name_repetitions` (unwrapped `cli/tests.rs`), `empty_line_after_doc_comment`, `struct_update_with_no_effect`, `derivable_impls`. **Done: 2026-05-10.**
A.7.6. ~~**Fix ROADMAP §1 test count.**~~ Already correct at 598 passing + 1 ignored. **Done: 2026-05-10.**
A.7.7. ~~**Add CI clippy gate to CLAUDE.md.**~~ Added `cargo clippy --all-targets -- -D warnings` to Build & Test section. **Done: 2026-05-10.**

### Phase A.5 — Finish the integration test story (1–2 days) **✅ COMPLETE**

The `tests/integration.rs` suite that landed in Phase A is real and useful, but it topped out at serde round-trips and on-disk format checks. Three structural issues prevented it from catching real regressions:

- **Production types were re-declared locally** in `tests/integration.rs:30-118` rather than imported from the crate. If `ipc.rs` adds a field or renames a variant, the test would continue to pass against its stale local copy. This is the opposite of what a contract test should do.
- **Persistence tests hand-rolled JSON** instead of calling `ScheduleStore::save()` / `session_store::save_session()`. A refactor of the on-disk format would not break these tests.
- **No daemon-loop or webhook-pipeline test existed.** The original C6 concern was the chat tool-loop and the webhook → ghost-spawn pipeline. Neither path was exercised end-to-end.

These items landed before any Phase B feature work — they are the assertion harness everything later depends on.

A1. ~~**Convert `daemoneye` to a library + binary.**~~ Add `src/lib.rs` with at minimum `pub use ipc; pub use scheduler; pub use session_store; pub use config;`. The binary stays a thin shim. This unblocks every test below and is also a precondition for plugin work in Phase E (R3 / I8). **Done: 2026-05-09.** `src/lib.rs` created with `pub mod` for test-accessible modules (`ai`, `config`, `daemon`, `ipc`, `scheduler`, `scripts`, `session_store`, `webhook`) and `pub(crate) mod` for internal modules (`cli`, `header`, `manifest`, `memory`, `pane_prefs`, `runbook`, `search`, `sys_context`, `tmux`, `util`). `src/main.rs` converted to a thin shim (`use daemoneye::{...}`). `TEST_HOME_LOCK` moved to `lib.rs`. 597 tests pass, zero warnings.
A2. ~~**Replace local IPC enums with `daemoneye::ipc::*`.**~~ Round-trip tests now catch schema drift automatically. Deleted the duplicated `Request` / `Response` definitions from `tests/integration.rs`. **Done: 2026-05-09.**
A3. ~~**Persistence tests via real APIs.**~~ Schedule and session tests now call `ScheduleStore::add()` and `session_store::save_session()` instead of writing JSON by hand, then assert against the loaded result. Event-log entries go through `daemon::utils::log_event()` rather than synthesising lines. **Done: 2026-05-09.**
A4. ~~**One real loop test.**~~ `daemon_ping_status_loop` spawns a daemon process, verifies `Request::Ping` → `Response::Ok` and `Request::Status` → `Response::DaemonStatus`. Marked `#[ignore]` because it requires tmux + a valid API key in the test environment. **Done: 2026-05-09.**
A5. ~~**One webhook → audit-log test.**~~ `webhook_alert_to_event_log` exercises `parse_payload → process_alert → log_event` with a synthetic Alertmanager payload and asserts `events.jsonl` contains a `webhook_alert` entry. **Done: 2026-05-09.**
A6. ~~**Mark C6 fully closed.**~~ C6 row in §2.2 struck through, severity changed to **Fixed**. **Done: 2026-05-09.**

**Exit criteria met:** zero local re-declarations of production types in `tests/`; integration suite imports `daemoneye::*`; one daemon-process test (ignored) and one webhook-pipeline test in CI; total test count ≥ 600 (598 passing + 1 ignored).

### Phase B — Quick product wins (weeks)
5. **R1** — Anthropic prompt caching on system prompt + manifest.
6. ~~**R2** — usage log + `daemoneye costs`.~~ **Done (v0.9.x);** budget-cap follow-up remains.
7. **R5** — `daemoneye doctor`.
8. ~~**R4** — finish the prompt library subcommand.~~ **Done.**
9. **R7** — webhook clustering.

With R2 and R4 landed, the remaining Phase B quick wins are **R1, R5, R7**
(plus the two open code issues, C5 and C8).

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

*Phase A and the M1 agent-tooling milestone are complete. Cost telemetry
(R2) has landed — Phase B ordering should now be informed by what
`daemoneye costs` reveals about real spend. The next milestone is to be
picked from the remaining R/I items and the two open code issues (C5, C8).*
