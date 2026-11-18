# M38 — The notice bus

**Goal:** every subsystem that tells someone something — webhooks, the
scheduler, the watchdog, ghost runs, background commands, terminals —
publishes one `Notice` with common fields onto one in-process bus, and
every consumer (chat history, the chat notice, the status bar and `top`,
the `on_alert` command, the ghost trigger, external subscribers) decides
what to take by filtering on those fields.

**Status:** closed 2026-11-18 — released as 2.10.0 on PE sign-off

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
| 06 | terminal-producer ([phase-06-terminal-producer.md](phase-06-terminal-producer.md)) | done |
| 07a | route-config ([phase-07a-route-config.md](phase-07a-route-config.md)) | done |
| 07b | remove-old-keys ([phase-07b-remove-old-keys.md](phase-07b-remove-old-keys.md)) | done |
| 08a | registry-replay ([phase-08a-registry-replay.md](phase-08a-registry-replay.md)) | done |
| 08b | digest-from-journal ([phase-08b-digest-from-journal.md](phase-08b-digest-from-journal.md)) | done |
| 08c | catchup-from-journal ([phase-08c-catchup-from-journal.md](phase-08c-catchup-from-journal.md)) | done |
| 09a | ghost-trigger-consumer ([phase-09a-ghost-trigger-consumer.md](phase-09a-ghost-trigger-consumer.md)) | done |
| 09b | analyze-route ([phase-09b-analyze-route.md](phase-09b-analyze-route.md)) | done |
| 09c | notice-docs ([phase-09c-notice-docs.md](phase-09c-notice-docs.md)) | done |
| 10a | notices-list ([phase-10a-notices-list.md](phase-10a-notices-list.md)) | done |
| 10b | notices-follow ([phase-10b-notices-follow.md](phase-10b-notices-follow.md)) | done |

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
- **PE decisions at phase-07 drafting (2026-11-17):** configured
  `[[notifications.route]]` tables **add to** the default routes, and
  `[notifications] default_routes = false` turns the defaults off. Once
  `[webhook] severity_threshold` is a load error, the `auto_analyze` gate
  and the away digest's alert count use a fixed `warning` until phase 09
  makes the ghost trigger a routed consumer. Nothing ships between phases,
  so the interim never reaches a release. Phase 07 split into 07a
  (additive route config) and 07b (the removals).
- **PE decision at phase-08c drafting (2026-11-18):** the catch-up brief
  lists the notices routed to history **or** the chat notice, so scheduler
  job results and watchdog analyses join it (2.9.0 showed them only as an
  overlay). Accepted delta.
- **PE decisions at phase-09 drafting (2026-11-18):** the ghost trigger
  becomes a routable sink, `sink = "analyze"`, whose default route
  reproduces today's gate (webhook alerts at or above `warning`). This
  retires the fixed-`warning` interim. Phase 09 splits into 09a (the
  queued consumer, with a fixed filter) and 09b (the route sink).
- **Gotcha for producer phases (from phase 03):** `notice_sinks::test_bus`
  spawns the queued sinks, so on a current-thread runtime it must be built
  **after** `rt.enter()` (or inside `block_on`). Built before, the spawned
  tasks never run and a test that waits on them hangs — it cost phase 03
  about an hour in `webhook_ghost_e2e_deterministic`.

## Retrospective (2026-11-18)

**Exit criteria.**
- **Met:**
  - **Every producer publishes through the bus.** The only remaining calls
    to `inject_into_sessions` / `notify_chat_panes` / `presence_broadcast`
    are inside the sink primitives themselves and in tests.
  - **The default routes reproduce 2.9.0.** The issue-#3 and
    below-threshold tests pass unchanged in meaning.
  - **Routes filter on severity, source, name and status.** Both removed
    keys fail to load, on start and on `reload`.
  - **Journal readers:** the catch-up brief and away digest read the
    journal by field, and the registry replays the last 6 h on start.
  - **The ghost trigger** is a queued consumer behind the routable
    `analyze` sink.
  - **`daemoneye notices`** lists from the journal and streams with
    `--follow`.
- **Live check, 2026-11-18 (met).** Release binary at `f5e9bc2b`,
  throwaway `HOME=/tmp/de38-live`, private tmux, GLM-5.3-Flash. The config
  was a copy of the PE's real 2.9.0 config, migrated. It kept the default
  routes and added one `command` route for `scheduler` + `webhook` that
  appends to a file. `daemoneye notices --follow` ran from before the
  first event and printed all four:

  ```
  2026-11-18 11:03:40 warning  webhook    firing DiskPressureLive [Webhook Alert] [FIRING] DiskPressureLive — disk /data at 91% (alertmanager)
  2026-11-18 11:06:00 error    scheduler  event fail-check `fail-check.sh` failed (exit 3)
  2026-11-18 11:06:47 warning  shell      event livecheck [Shell s2] Program livecheck (blocked (question)) msg: continue the migration?
  2026-11-18 11:09:51 error    background event 4dbc717457a3407c [Background Task Completed]
  ```

  | Notice | History | Chat notice | Alert registry | Command route |
  |---|---|---|---|---|
  | webhook `warning` (repeated as `LiveWarnCheck` once a chat was open) | yes | yes (`⚙` line) | yes (status bar `⚠ 2 warn`) | yes |
  | webhook `info` (`LiveInfoCheck`) | no | no | yes | yes (the route has no floor) |
  | scheduler failure (repeated as `fail-check-2` with a chat open) | no | yes (`⚙` line) | — | yes |
  | background completion (`wait: false`, exit 4) | yes | no | — | — |
  | shell `blocked` (OSC 7501) | no | no | — | — |

  Every row matches `default_routes(Warning)`: only the journal and the
  follower saw the shell notice. The analyze sink was not exercised live:
  no runbook matched the alert names. The isolation tests cover it.

**Found in the live check:**
- **Fixed (`f5e9bc2b`):** a config that still set either removed key fell
  back to the defaults. The daemon then failed on the default provider's
  missing API key, and the migration message reached only `daemon.log`.
  The daemon now refuses to start with every removed key's migration
  printed on the terminal.
- **Not fixed — first-sight baseline.** `ProgramWatcher` treats a shell's
  first probe as baseline (phase 06, as specified). The `wait: false`
  shell `s3` reported `blocked` within its first 2 s tick, so it never
  produced a notice; only `s2`, which already existed, did. A background
  program that asks a question at once is never announced. Candidate
  issue.
- **Not a bug:** one-shot `daemoneye ask` has no session, so
  `wait: false` is refused there. The refusal reads "Run it in the chat
  session's shell instead", which is misleading for `ask`.

**Outcomes:** 16 phases.
- 9 `approved_first_try`: 02, 03, 06, 07b, 08a, 08c, 09b, 10a, 10b. In
  five of these (08a, 08c, 09b, 10a, 10b) the reviewer fixed defects
  instead of bouncing, per the PE's standing preference for small fixes.
- 3 `approved_after_1`: 04, 05, 07a.
- 4 `escalated`:
  - 01: two `hard_fail` runs.
  - 08b: `hard_fail` on a repeated grep.
  - 09a: `budget_exceeded`, a test-guard deadlock.
  - 09c: `hard_fail`, docs-only, no edits.

The executor was GLM-5.3-Flash throughout. The split of 07, 08, 09 and 10
into sub-phases held: no sub-phase exceeded one run.

**What went wrong, by class:**
- **Tests that restate the code under test:** 08a, 10a, and 08b's journal
  fixture. A spec saying "do not restate the rules" was ignored. Naming the
  seam function each test must call worked in 10b.
- **Read paralysis:** 09c read for 428 turns without an edit, despite a
  closed file list, an edit deadline and pre-verified facts. Two other
  stalls came from repeated tool calls: 08b on a grep, and 09c's last 60
  turns on another. Docs-only phases on this model are cheaper taken over.
- **Bus/journal test mechanics cost three phases their finish:**
  - 03: a queued consumer spawned before `rt.enter`.
  - 09a: `test_home_guard` taken twice; a consumer registered after the
    publish.
  - 08b and 08c: fixture `ts` not pinned, or set in the future.
- **Order-for-correctness was reversed in 10b.** `--follow` read the
  backlog before subscribing; the reviewer fixed it.
- **Cap pressure.** 10b cut doc comments to fit `src/ipc.rs` under 800
  lines; the reviewer restored them and allowlisted the file at 806.

**Fixed at the close:**
- **Tests wrote into the real event log** (issue #10). The leak dated from
  2026-10-31: 1,661 `ssh_connect_failed` / `scheduler_job_recovered`
  fixture records across 19 segments, plus 29 pid-`4242` notice fixtures
  from M38 executor runs. The PE approved removing them all. Five tests
  now take `test_home()` (`fd81466c`). `log_event`'s `cfg(test)` assert
  now refuses a test whose `HOME` is not redirected, not just one without
  the lock (`a5d951ef`). A full `cargo test` leaves the real event
  directory byte-identical.
- **Stale docs:** `REQUIREMENTS.md` FR-1.2.11 / FR-1.2.14 and
  `design-reference.md` §2.7 / §2.9 describe routes and sinks
  (`215a56b8`).

**Calibration folded (PE sign-off, `95573e2d`):** a test-plan entry names
the function the test must call, not just the behaviour, and a spec that
orders steps for correctness says what breaks if the order is reversed.
See `WORKFLOW.md` § "Name the function each test must call; say why an
order matters".
