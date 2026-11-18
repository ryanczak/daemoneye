# NEXT

**Active milestone:** M38 — The notice bus
([`README.md`](milestones/M38-notice-bus/README.md)). Scoped 2026-11-16,
target `2.10.0`. After M38 comes the MCP surface (M39), then Knowledge
(M40). M37 closed as `2.9.0` (2026-11-16).

**Active phase:** none drafted — next is 09b (the `analyze` route sink in
`[[notifications.route]]`), then 10 (`Request::Subscribe` and `daemoneye
notices`). 01–08c `done`; 09a `done` (escalated: remainder finished by the
architect).

**Absorbed by M38:** the M37 registry-restart gap (alert registry rebuilt
from the journal at start, phase 08).

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
