# M38 — The notice bus

**Goal:** every subsystem that tells someone something — webhooks, the
scheduler, the watchdog, ghost runs, background commands, terminals —
publishes one `Notice` with common fields onto one in-process bus, and
every consumer (chat history, the chat notice, the status bar and `top`,
the `on_alert` command, the ghost trigger, external subscribers) decides
what to take by filtering on those fields.

**Status:** in-progress

**Release:** 2.10.0

**Depends on:** M37 (the tree as released in 2.9.0).

**Why now.** Each producer hand-wires its own subset of seven outputs
(event log, chat history, presence notice, tmux overlay, `on_alert`,
alert registry, ghost analysis) in its own order, and only webhooks carry
a severity. Issue #3 was one producer gating after it had delivered. The
catch-up brief recognises events by text prefix (`"[Background Task
Completed"`), and the alert registry is lost on restart. Adding a
producer or a consumer today means editing every path by hand.

## Design (architect recommendation, PE-approved 2026-11-16)

**Content-filtered publish/subscribe with a journal.** Not topic-based:
producers do not decide who cares. A consumer subscribes with a filter
over the fields every notice carries.

- **`Notice`** — `id`, `ts`, `severity`, `source`, `source_id`, `name`,
  `status` (`firing | resolved | event`), `fingerprint` (correlates a
  firing with its resolve; defaults to `id`), `session` (`None` = every
  chat, `Some(id)` = one chat), `title`, `body`, `labels`.
- **`Severity`** — `debug < info < warning < error < critical`. External
  severities parse with aliases; an unrecognised one is `debug`, which
  keeps M37's fail-closed rule (below `info`, below any threshold).
- **Journal first.** `publish` masks `title`/`body`, appends a `notice`
  record to `events.jsonl`, then delivers. The journal is the durable,
  ordered record: readers (catch-up brief, away digest, registry replay,
  `daemoneye notices`) query it by field.
- **Two kinds of consumer.** *Inline* consumers run inside `publish`,
  must be cheap and non-blocking (history append, alert registry) and
  keep today's synchronous semantics. *Queued* consumers get a bounded
  queue each (tmux overlay, `on_alert`, ghost trigger, IPC subscribers);
  a full queue drops and counts for that consumer only — the journal is
  the replay.
- **Routes are config.** `[[notifications.route]]` tables pair a sink
  with a filter. Defaults reproduce 2.9.0 delivery.
- **Not this:** no external broker, no bus across hosts, no high-rate
  traffic (streamed tokens and tool feedback stay on IPC).

**PE decisions (2026-11-16, at scoping):**

1. **Numbering.** The bus is M38 / 2.10.0. The MCP surface moves to M39,
   Knowledge to M40.
2. **Five severities**: `critical`, `error`, `warning`, `info`, `debug`.
3. **External consumers in scope**: `Request::Subscribe { filter }` and a
   `daemoneye notices [--follow] [--min-severity] [--source] [--name]`
   CLI. External *publish* is not in scope.
4. **Old keys removed.** `[webhook] severity_threshold` and
   `[notifications] on_alert` become config-load errors whose message
   names the equivalent `[[notifications.route]]`. No translation layer.

**Exit criteria:**
- Every producer (webhook alerts and analyses, scheduler job results,
  watchdog, ghost lifecycle, background completions, OSC 7501
  `blocked`/`error`/`done` transitions) publishes through the bus, and no
  producer calls `inject_into_sessions`, `notify_chat_panes`,
  `presence_broadcast` or `fire_notification` itself.
- With default routes, delivery matches 2.9.0: the issue-#3 tests and the
  M37 below-threshold tests pass unchanged in meaning.
- `[[notifications.route]]` filters on severity, source, name and status;
  the two removed keys fail config load with a migration message, on
  start and on `reload`.
- The catch-up brief and the away digest read notices from the journal
  by field, not by text prefix; the alert registry rebuilds from the
  last `ACTIVE_ALERT_TTL_SECS` of journal on daemon start.
- `auto_analyze`'s watchdog → ghost trigger is a queued bus consumer.
- `daemoneye notices` lists past notices from the journal and, with
  `--follow`, streams live ones over `Request::Subscribe`.
- Live check on pinky: an Alertmanager alert, a failing scheduled
  script, a background command and a terminal `blocked` report each land
  where the routes say, and `daemoneye notices --follow` shows all four.

## Architecture references

- `docs/architecture.md#22-event-driven--asynchronous-flows`
- `src/webhook/process.rs` (`process_alert`, `inject_into_sessions`,
  `notify_chat_panes`, `inject_ghost_event`), `src/daemon/scheduled.rs`
  (`notify_job_result`), `src/daemon/executor/bg_shell.rs`,
  `src/shell/program_status.rs`, `src/daemon/server/catchup.rs`,
  `src/daemon/away_digest.rs`, `src/webhook/active.rs`.

## Phases

Drafted one at a time; the list past 01 is the plan, not a commitment.

| #  | Phase | Status |
|----|-------|--------|
| 01 | bus-core ([phase-01-bus-core.md](phase-01-bus-core.md)) | done |
| 02 | sinks ([phase-02-sinks.md](phase-02-sinks.md)) | done |
| 03 | webhook-producer ([phase-03-webhook-producer.md](phase-03-webhook-producer.md)) | done |
| 04 | scheduler-producers ([phase-04-scheduler-producers.md](phase-04-scheduler-producers.md)) | done |
| 05 | ghost-background-producers ([phase-05-ghost-background-producers.md](phase-05-ghost-background-producers.md)) | done |
| 06 | terminal-producer ([phase-06-terminal-producer.md](phase-06-terminal-producer.md)) | review      |
| 07 | route config; the two old keys become load errors | planned |
| 08 | journal readers: catch-up, away digest, registry replay | planned |
| 09 | ghost trigger as a consumer | planned |
| 10 | `Request::Subscribe` and `daemoneye notices` | planned |

## Notes

- Held items that this milestone absorbs: the M37 registry-restart gap
  (phase 08).
- Held items it does not: issue #4 (cron time zones), #7, #10, #11.
- **Known delivery deltas from 2.9.0, accepted at phase-02 drafting:**
  scheduler and watchdog notices gain the tmux overlay (2.9.0 pushed them
  over presence only), and `on_alert` gains `DAEMONEYE_SEVERITY` /
  `DAEMONEYE_SOURCE`. History text is `title` or `title\nbody`; the chat
  line is the title's first line — producers (03–05) choose title/body so
  the catch-up brief's prefixes still match until phase 08 retires them.
- **Accepted delivery deltas from 2.9.0, phase 03 (webhook producer):**
  an alert whose severity is `error`/`err` now ranks and passes a
  `warning` threshold (2.9.0 treated it as unrankable and failed closed);
  and the history text's first line is `[Webhook Alert] [FIRING] …`
  instead of a bare `[Webhook Alert]` line — the status tag moves into the
  title so the chat-notice overlay and the catch-up brief carry it.
- **Gotcha for producer phases (from phase 03):** `notice_sinks::test_bus`
  spawns the queued sinks, so on a current-thread runtime it must be built
  **after** `rt.enter()` (or inside `block_on`). Built before, the spawned
  tasks never run and a test that waits on them hangs — it cost phase 03
  about an hour in `webhook_ghost_e2e_deterministic`.
