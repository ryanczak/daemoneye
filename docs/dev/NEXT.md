# NEXT

**Active milestone:** M39 — Critical fixes
([`README.md`](milestones/M39-critical-fixes/README.md)), scoped
2026-11-21 from the 2.10.0 audit
([`daemoneye_audit.md`](../design/daemoneye_audit.md), triaged in
[`daemoneye_audit_ledger.md`](../design/daemoneye_audit_ledger.md)). The
audit is five milestones: M39 Critical fixes, M40 Security hardening, M41
Correctness & reliability, M42 Performance & concurrency, M43
Architecture & debt. The MCP surface moves to M44 and Knowledge to M45
(PE decision 2026-11-21). M38 (The notice bus) closed 2026-11-18 as
`2.10.0`; M37 closed as `2.9.0` (2026-11-16).

**Active phase:** M39 phase-06 — wrap-remote
([`phase-06-wrap-remote.md`](milestones/M39-critical-fixes/phase-06-wrap-remote.md)),
drafted 2026-11-24, `todo`. Dispatch with `/rexymcp:dispatch phase-06`.
Phases 01–05 are `done`. The scoping decisions in the M39 README still
need PE sign-off.

**Open from the M38 live check:** GitHub issue #13. A `wait: false` shell
that reports `blocked` within its first 2 s probe never produces a notice
(`ProgramWatcher` first-sight baseline).

**Absorbed by M38:** the M37 registry-restart gap (alert registry rebuilt
from the journal at start, phase 08).

**Held by PE decision (2026-11-01):** the WORKFLOW fold candidate A (count
criteria) and the closing of GitHub issue #1 — M36 phase 12 (mid-turn
messages) likely answers the issue's remaining half, so the issue stays
open until that phase's live check.

**Held, triaged 2026-11-15:** GitHub issue #4 (cron is UTC-only, no DST
awareness). Scope: option 1 only — optional `tz` (IANA) on the cron schedule
and on `schedule_command`, validated at creation, `list_schedules` showing UTC
and the job's zone; new dependency `chrono-tz` or `jiff`. Scheduled
2026-11-21 as M41 phase 10b
([`README.md`](milestones/M41-correctness/README.md)). It must ship before
the 2027-03-14 DST change; if M41 is late, ship 10b early as a patch.

M35 closed 2026-11-01 (`v2.7.0`, pushed). M34 (Function caps) was withdrawn
2026-10-31 after phase 01. M33 closed 2026-10-31 (`v2.6.0`, pushed). M32
closed 2026-10-30 (`v2.5.0`, pushed). M31 closed 2026-10-28 (`v2.4.0`,
pushed). M30 closed 2026-10-25 (`v2.3.0`, pushed). M29 closed 2026-10-20
(`v2.2.0`, pushed). Governor & guards (former M25) and F8 (ghost
foreground) remain unscheduled.

Fold queue: M28 proposal B (held), M27 proposal B (held) and M23's two
proposals.
