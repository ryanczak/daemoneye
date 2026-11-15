# NEXT

**Active milestone:** M37 — Alerts out of history, onto the status bar and
`top` ([`README.md`](milestones/M37-alerts-status-bar/README.md)). Scoped
2026-11-12 for GitHub issue #3, target `2.9.0`. After M37 comes the MCP
surface (M38), then Knowledge (M39).

**Active phase:** [phase-09-status-bar-words](milestones/M37-alerts-status-bar/phase-09-status-bar-words.md) — words on every status-bar item and grants kind (PE decision 7, 2026-11-15), drafted before the M37 live check so the check sees the final bar. Phases 01 through 08 are done.

**Held by PE decision (2026-11-01):** the WORKFLOW fold candidate A (count
criteria) and the closing of GitHub issue #1 — M36 phase 12 (mid-turn
messages) likely answers the issue's remaining half, so the issue stays
open until that phase's live check.

**Held, triaged 2026-11-15:** GitHub issue #4 (cron is UTC-only, no DST
awareness). Scope: option 1 only — optional `tz` (IANA) on the cron schedule
and on `schedule_command`, validated at creation, `list_schedules` showing UTC
and the job's zone; new dependency `chrono-tz` or `jiff`. Not in M37; schedule
into a later milestone before March 2027.

M35 closed 2026-11-01 (`v2.7.0`, pushed). M34 (Function caps) was withdrawn
2026-10-31 after phase 01. M33 closed 2026-10-31 (`v2.6.0`, pushed). M32
closed 2026-10-30 (`v2.5.0`, pushed). M31 closed 2026-10-28 (`v2.4.0`,
pushed). M30 closed 2026-10-25 (`v2.3.0`, pushed). M29 closed 2026-10-20
(`v2.2.0`, pushed). Governor & guards (former M25) and F8 (ghost
foreground) remain unscheduled.

Fold queue: M28 proposal B (held), M27 proposal B (held) and M23's two
proposals.
