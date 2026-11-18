# Phase 09c — notice-docs

**Milestone:** M38 — The notice bus
**Status:** done
**Drafted:** 2026-11-18
**Depends on:** phase-09b
**Estimated diff:** ~150 lines, docs only
**Tags:** language=markdown, kind=docs, size=s

## Goal

The config template, the seeded knowledge memories and the README describe
the notice bus as it now works. Today the `[notifications]` key tables are
current, but four things are stale or missing:
- **No defaults table.** Nothing lists the default routes, so a user
  cannot tell what to override.
- **Stale README prose** about ghost triggering, the catch-up brief and
  the alert registry.
- **Seeded guides without routes.** The webhook and ghost guides never
  mention routes.
- **The seeded config reference** does not say that two keys were removed.

## Pre-flight

1. Read `docs/dev/STANDARDS.md`, the milestone README, and this doc.
2. Clean tree on `master`.

## Edit order and the files you need

**Your first `patch` lands within your first 15 turns.** Edit **only**
these files, in this order. Each step ends at a green
`cargo test --test doc_truth`.

1. `assets/etc/config.toml`
2. `assets/memory/knowledge/config-reference.md`,
   `assets/memory/knowledge/webhook-setup.md`,
   `assets/memory/knowledge/ghost-shell-guide.md`
3. `README.md`

## Facts to document (verified against the source, 2026-11-18)

- **Severities**, low to high: `debug`, `info`, `warning`, `error`,
  `critical`. An external severity the daemon does not recognise becomes
  `debug`, below every default route.
- **Sources:** `webhook`, `scheduler`, `watchdog`, `ghost`, `background`,
  `shell`, `daemon`.
- **Default routes** (`default_routes` in `src/daemon/notice_sinks.rs`):

  | Sink | Default filters (any one matches) |
  |---|---|
  | `history` | webhook at `warning`+; ghost (any); background (any) |
  | `chat_notice` | webhook at `warning`+; ghost; scheduler; watchdog |
  | `analyze` | webhook alerts, firing or resolved, at `warning`+ |
  | `command` | none: command routes exist only when configured |

- **The alert registry** records every webhook alert, firing or resolved,
  at **every** severity. No route controls it. On daemon start it is
  rebuilt from the last 6 hours of the journal.
- **The journal.** Every notice is written to the event log as an
  `event: "notice"` record before it is delivered.
  - The catch-up brief lists the notices from the journal that history or
    the chat notice routed to that chat, so scheduler results and watchdog
    analyses appear in it.
  - The away digest counts script runs and alerts from the journal.
- **Removed in 2.10.0:** `[notifications] on_alert` and
  `[webhook] severity_threshold`. Setting either fails config load (start
  and `reload`) with a message showing the equivalent routes.

## Spec

1. **Template** (`assets/etc/config.toml`, commented lines only).
   - The `[notifications]` comment lists the default routes in a few lines,
     and names the two removed keys.
   - The `auto_analyze` comment says that analysis runs for the alerts the
     `analyze` routes match.

2. **Seeded memories.**
   - `config-reference.md` gains a short notices section: severities,
     sources, the defaults table, and the removed keys. A model editing
     the config must not write either removed key.
   - `webhook-setup.md` gains a routing step: by default alerts at
     `warning`+ reach the chat and the analysis; routes change that.
   - `ghost-shell-guide.md`'s "From a webhook" paragraph says the
     analysis runs for alerts the `analyze` route matches, `warning`+ by
     default.

3. **README.**
   - **New:** a `#### Default routes` subsection under
     `[[notifications.route]]`, carrying the defaults table, the severity
     order, the alert-registry note and the journal note.
   - **Fix** "When a critical alert matches a runbook…": the trigger is
     the `analyze` route, `warning`+ by default.
   - **Fix** the catch-up brief bullet: it lists what the routes sent to
     history or the chat notice, including scheduler results.
   - **Fix** the `auto_analyze` row: it gates the analysis of the alerts
     the `analyze` routes match.
   - **Fix** the alert-registry paragraph. Its "to `daemoneye top` in a
     later release" is stale, because `top` already shows an Alerts panel.
     Add the 6-hour replay on start.
   - **Fix** the ghost flow diagram's "injected into all active
     sessions": the lifecycle notice goes to the routes, history and chat
     notice by default.

**Gotcha:** every TOML line you add uses only keys that already exist.
Phase 07a's executor added a `route = []` line that would have failed to
load. Do not invent keys, sinks, sources or severities beyond the lists
above.

## Acceptance criteria

- [ ] `grep -c 'in a later release' README.md` prints `0`.
- [ ] `grep -c 'When a critical alert matches' README.md` prints `0`.
- [ ] `grep -c '^#### Default routes' README.md` prints `1`.
- [ ] `grep -c 'on_alert' assets/memory/knowledge/config-reference.md`
      prints at least `1`.
- [ ] `grep -c 'severity_threshold' assets/etc/config.toml` prints at
      least `1`.
- [ ] `grep -c 'analyze' assets/memory/knowledge/ghost-shell-guide.md`
      prints at least `1`.
- [ ] `grep -c 'notifications.route' assets/memory/knowledge/webhook-setup.md`
      prints at least `1`.
- [ ] All four gates pass, including `tests/doc_truth.rs`.

## Test plan

No new tests: this phase changes docs only. `tests/doc_truth.rs` and the
config-template tests guard the edits. The template must still load as
the default config.

## End-to-end verification

Paste `cargo test --test doc_truth` and the acceptance greps into the
Update Log.

## Authorizations

- May touch exactly the files listed in "Edit order".

## Out of scope

- `docs/REQUIREMENTS.md` and `docs/design-reference.md`. Their
  `on_alert` / `severity_threshold` text is swept at milestone close.
- `daemoneye notices` and `Request::Subscribe` (phase 10 documents its
  own CLI).
- **Reseeding existing installs.** Seeded memories are written only when
  absent, so an upgraded install keeps its old copies.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-18 (escalation)

**Chosen lever:** session takeover.
**Rationale:** the run made no edits in 428 turns and ended `hard_fail`
(`IdenticalToolCallRepetition`: the same bash `grep` for `briefing` in
`src/daemon/ghost/prompt.rs`, six times in a row). It read the four target
files early on, then spent the run in source files the spec never named. The
spec already carried every anti-stall lever from earlier phases: a closed
file list, a first edit due by turn 15, an edit order, and the facts
pre-verified. The executor ignored all of them. A refined re-dispatch would
meet the same read-without-editing failure, and the phase is ~150 lines of
docs.

### Review verdict — 2026-11-18

- **Verdict:** escalated
- **Bounces:** none
- **Executor:** GLM-5.3-Flash (428 turns, `hard_fail`, no diff), then
  Claude (direct)
- **Scope deviations:** none.
  - The ghost guide's lifecycle-events sentence now says what the routes do.
  - The README's `Default routes` subsection notes that a `shell` notice
    matches no default route. Both are covered by the spec's facts.
- **Checked:**
  - Every fact against `default_routes`, `Filter`, `Source` and
    `NoticeStatus` (the lowercase serde names) and `webhook.auto_analyze`
    in `trigger.rs`.
  - The new TOML examples parse.
  - All seven acceptance greps pass (0, 0, 1, 1, 1, 1, 2).
  - Gates: 3022 lib tests, fmt and clippy `-D warnings` pass, and every
    integration binary is green.
- **Calibration:** on GLM-5.3-Flash, a docs-only phase over the README is a
  read-paralysis risk even with a closed file list. The README is ~1400
  lines and every claim invites a source check. Pre-verified facts did not
  stop it checking them again. For docs phases on this model, prefer an
  architect takeover, or split the work into one file per phase.

