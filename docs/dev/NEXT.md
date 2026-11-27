# NEXT

**Active milestone:** none. M40 Security hardening
([`README.md`](milestones/M40-security-hardening/README.md)) finished its
phases 2026-11-27. The retrospective and live check are written. It waits on
the PE's sign-off, then the `2.12.0` release per `REXYMCP.md` § Versioning.
Next come M41 Correctness, M42 Performance and M43 Architecture, then the MCP
surface (M44) and Knowledge (M45). M39 (Critical fixes) closed 2026-11-25 as
`2.11.0`.

**Active phase:** none.

**Pending PE decisions from the M40 close:**
- Fold A: probe every claim about current behaviour at drafting. It would
  extend `WORKFLOW.md` § "Reproduce a defect before drafting its phase".
- Whether to re-record the M40 bounces logged as `false_completion`.

**Deferred from the M39 live check:** a command that times out and stops
on Ctrl-C always waits the full 5 s interrupt grace, because the end
marker never prints. That makes `timeout_secs: 5` return after about 10 s.
Candidate for M41 or M42.

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
