# Phase 01 — bus-core

**Milestone:** M38 — The notice bus
**Status:** done
**Drafted:** 2026-11-16
**Depends on:** none
**Estimated diff:** ~500 lines, over half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Add the bus itself: the `Notice` envelope, the five-level `Severity`, the
`Filter` consumers subscribe with, and a `Bus` that journals then fans
out. No producer or consumer moves in this phase; nothing calls the bus
yet outside its tests.

## Architecture references

- `docs/dev/milestones/M38-notice-bus/README.md` § Design — the shape
  every later phase builds on. Read it first.

## Pre-flight

1. Read `docs/dev/STANDARDS.md`, the milestone README, and this doc.
2. Clean tree on `master`.

## Current state

No `src/bus/`. Relevant existing symbols: `log_event`
(`src/daemon/utils/event_log.rs`, masks its fields and asserts the test
holds the HOME guard — a thread-local flag, so it panics when called from
a thread the test spawned), `crate::test_home()` (`src/lib.rs`),
`mask_sensitive` (`src/ai/filter.rs`; `AKIAIOSFODNN7EXAMPLE` masks to
`<AWS_KEY>` with no init), `severity_rank` (`src/webhook/process.rs`,
the alias table today: `critical`; `warning`/`warn`;
`info`/`informational` — leave it in place, phase 03 retires it).

## Spec

1. **Module** — create `src/bus/` (`mod.rs`, `notice.rs`, `filter.rs`)
   and register `pub mod bus;` in `src/lib.rs`. First write within your
   first 15 turns; each later task ends at a green `cargo build`.

2. **`Severity`** — in `notice.rs`: `Debug < Info < Warning < Error <
   Critical`, deriving `Ord`, serde lowercase. `Severity::parse(&str) ->
   Option<Severity>`, case-insensitive, accepting the five names plus the
   aliases `warn`, `informational`, `err`. `Severity::from_external(&str)
   -> Severity` returns `Debug` for anything `parse` rejects, including
   `""`. `as_str()` returns the lowercase name.

3. **`Source` and `NoticeStatus`** — serde lowercase enums. `Source`:
   `Webhook, Scheduler, Watchdog, Ghost, Background, Shell, Daemon`.
   `NoticeStatus`: `Firing, Resolved, Event`.

4. **`Notice`** — fields `id: String` (uuid v4), `ts:
   chrono::DateTime<Utc>`, `severity`, `source`, `source_id: String`,
   `name: String`, `status`, `fingerprint: String`, `session:
   Option<String>`, `title: String`, `body: String`, `labels:
   BTreeMap<String, String>`. Derive `Debug, Clone, PartialEq, Serialize,
   Deserialize`. `Notice::new(severity, source, name, title)` fills a
   fresh id, `ts = now`, `status = Event`, `fingerprint = id`, empty
   `source_id`/`body`, no session, no labels; builder methods set the
   rest. **No field may be named `event` or `pid`** — the journal record
   uses those keys.

5. **`Filter`** — in `filter.rs`: `min_severity: Option<Severity>`,
   `sources: Vec<Source>`, `names: Vec<String>`, `statuses:
   Vec<NoticeStatus>`; `Default`, `Deserialize` with
   `#[serde(default, deny_unknown_fields)]` (later phases load it from
   TOML routes and IPC). `matches(&Notice) -> bool`: every set criterion
   must hold; `min_severity` is inclusive; an empty list means any; names
   compare exactly.

6. **`Bus`** — in `mod.rs`:
   - `Bus::new()` journals through `log_event("notice", <the notice as a
     JSON object>)`; `Bus::with_journal(f)` takes any `Fn(&Notice) + Send
     + Sync` (tests).
   - `publish(&self, Notice) -> Arc<Notice>`: masks `title` and `body`
     with `mask_sensitive`, journals, then delivers to each matching
     subscriber. Publishes are serialized under one lock so the journal
     and every subscriber see the same order.
   - `subscribe(&self, name, Filter, capacity) ->
     tokio::sync::mpsc::Receiver<Arc<Notice>>` — a queued consumer.
     Delivery uses `try_send`: a full queue drops the notice for that
     subscriber only and counts it; a closed receiver removes the
     subscriber.
   - `subscribe_inline(&self, name, Filter, f)` — `f: Fn(&Notice) + Send
     + Sync + 'static`, called inside `publish`, after the journal, in
     subscription order.
   - **Re-entry:** a `publish` from inside an inline subscriber on the
     same thread must not deadlock: it logs at `error`, skips journal and
     delivery, and returns the masked notice.
   - `stats(&self) -> Vec<SubscriberStats { name, inline, delivered,
     dropped }>`.
   - `bus::global() -> &'static Bus` (a `OnceLock`, `Bus::new()`) and a
     free `bus::publish(Notice) -> Arc<Notice>` over it.
   - Lock sites use `.unwrap_or_log()`.

7. **Docs** — add a `src/bus/` row to CLAUDE.md's Key files table.

8. **Gates** — `cargo fmt --all`, `cargo build`, `cargo clippy
   --all-targets --all-features -- -D warnings`, `cargo test`, all green.

## Acceptance criteria

- [ ] Every test named in the Test plan exists and passes.
- [ ] `cargo test bus::` runs them; `cargo clippy --all-targets
      --all-features -- -D warnings` exits 0; `tests/hygiene.rs` passes
      with no new allowlist line.
- [ ] No file outside `src/bus/`, `src/lib.rs`, `CLAUDE.md` and this
      phase doc changes (`git status --short`).

## Test plan

All in `src/bus/`. Tests other than the journal test use
`Bus::with_journal` over a recording `Vec`, never the global bus.

- `severity_orders_debug_to_critical`
- `severity_parse_accepts_names_and_aliases_case_insensitive` —
  `WARN`, `Informational`, `err`, `Critical` parse; `crit`, `warning2`,
  `""` do not.
- `severity_from_external_unknown_is_debug`
- `notice_new_defaults` — two notices get distinct ids; status `Event`,
  fingerprint equals id, session `None`.
- `notice_serde_uses_lowercase_enums` — the JSON has `"severity":
  "warning"`, `"source":"webhook"`, `"status":"firing"` and round-trips.
- `filter_default_matches_everything`
- `filter_min_severity_is_inclusive` — `warning` passes warning and
  critical, not info or debug.
- `filter_lists_are_any_of_and_empty_is_any` — sources, names, statuses.
- `filter_deserializes_from_toml` and `filter_rejects_unknown_field`
  (`min_severty = "info"` is an error).
- `bus_journals_before_inline_delivery` — the inline subscriber sees the
  journal already holding its notice.
- `bus_delivers_only_to_matching_subscribers`
- `bus_masks_title_and_body` — `AKIAIOSFODNN7EXAMPLE` in both reaches
  the journal and the subscriber as `<AWS_KEY>`, never raw.
- `bus_full_queue_drops_and_counts` — capacity 1, three publishes, no
  receive: that subscriber gets 1 and `dropped == 2`; a second subscriber
  with room gets all 3.
- `bus_closed_subscriber_is_removed`
- `bus_concurrent_publishers_same_order_everywhere` — 4 threads × 50
  publishes; the journal and two queued subscribers hold the same 200 ids
  in the same order.
- `bus_reentrant_publish_does_not_deadlock` — an inline subscriber that
  publishes; the outer call returns and the journal holds one notice.
- `bus_new_journals_a_notice_event` — under `crate::test_home()`,
  `Bus::new().publish(..)` writes one line in the current event segment
  with `"event":"notice"` and the notice's `id` and `severity`.

## End-to-end verification

Not applicable — phase ships no runtime-loadable artifact. Nothing calls
the bus outside its own tests until phase 02.

## Authorizations

- May touch `CLAUDE.md` (the Key files table row only).

## Out of scope

- Any producer or consumer change: `process_alert`, `scheduled.rs`,
  `inject_ghost_event`, `bg_shell.rs`, the presence channel.
- Config (`[[notifications.route]]`) and IPC (`Request::Subscribe`).
- Removing or changing `severity_rank`.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-16 19:53 (progress)

Started phase 01. Read STANDARDS, the milestone README and this doc; confirmed a clean tree. Flipping the status to in-progress and writing `src/bus/` (`mod.rs`, `notice.rs`, `filter.rs`) first, per the edit order: module registration, then the types, then the bus, then the tests.

### Update — 2026-11-16 (escalation)

**Chosen lever:** resume
**Rationale:** hard_fail on IdenticalToolCallRepetition (the concurrent-order test re-run six times); the tree builds, clippy is clean, 23/24 `bus::` tests pass, and the one failure is a single diagnosed defect in `Bus::publish`, not a spec gap.

### Update — 2026-11-16 (escalation, round 2)

**Chosen lever:** none — the work is complete on disk; hand to review.
**Rationale:** the resume applied the per-thread re-entry guard, then hard-failed on IdenticalToolCallRepetition (`cargo test --lib bus::` six times after it was already green). Architect ran the gates against the tree: fmt clean, clippy clean, `bus::` 24/24, full suite 2964 passed / 0 failed. No third dispatch; `/rexymcp:review phase-01` next.

### Review verdict — 2026-11-16

- **Verdict:** escalated
- **Bounces:** 1 (bug-01-1)
- **Executor:** GLM-5.3-Flash (two runs, both `hard_fail` on
  IdenticalToolCallRepetition), then Claude (direct) for bug-01-1
- **Scope deviations:** `Severity` lives in its own `src/bus/severity.rs`
  rather than `notice.rs` (re-exported; behaviour as specified). Reviewed
  from `in-progress` after the round-2 escalation found the work complete.
  Extra tests beyond the plan: `filter_partial_toml_defaults_the_rest`,
  `notice_builder_sets_the_rest`, `bus_stats_reports_inline_and_dropped`,
  `bus_inline_subscribers_run_in_subscription_order`, `severity_alias_smoke`.
- **Fixes at review:** bug-01-1 (`swap_remove` → `remove`, with a test
  seen red first); dropped a no-op `remove`/`insert` of `id` in
  `Bus::new`'s journal closure.
- **Evidence:** the concurrency test was observed red on the executor's
  first tree (153 of 200 notices, the `try_lock` re-entry defect) and green
  after the thread-local guard. Gates at approval: fmt, build, clippy clean;
  `bus::` 25/25; full suite 3065 passed, 0 failed.
- **Calibration:** glm stalled twice by re-running one test after its
  answer was already in hand (red, then green). Same class as the
  read-paralysis memory; a fold candidate if it recurs: specs name the
  single command that ends each task and say "run it once".
