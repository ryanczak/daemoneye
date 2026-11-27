# NEXT

**Active milestone:** M41 Correctness & reliability
([`README.md`](milestones/M41-correctness/README.md)), target `2.13.0`,
signed off by the PE 2026-11-27. Then M42 Performance and M43
Architecture, then the MCP surface (M44) and Knowledge (M45). M40 (Security
hardening) closed 2026-11-27 as `2.12.0`, pushed.

**Active phase:** [phase 01 test-hazards](milestones/M41-correctness/phase-01-test-hazards.md)
(Q9 Q10 Q11), `todo`, drafted 2026-11-27. Q9 and Q11 reproduced at
drafting; Q10 is latent and gets a preservation test.

**Held by PE decision (2026-11-27):** M40 fold A. It would make every
spec claim about current behaviour get a probe at drafting, using the
criterion's exact input. There are 5 occurrences, all in the M40
retrospective.

**Absorbed by M41 (PE 2026-11-27):**
- the M39 live check's interrupt-grace finding (phase 17);
- GitHub issues #13 (18), #15 and #11 (19), #12 (20), and #17 and #18
  (21);
- #16 (phase 09, with C6).

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
2026-11-21 as M41 phase 10b. It must ship before the 2027-03-14 DST
change; if M41 is late, ship 10b early as a patch.

M35 closed 2026-11-01 (`v2.7.0`, pushed). M34 (Function caps) was withdrawn
2026-10-31 after phase 01. M33 closed 2026-10-31 (`v2.6.0`, pushed). M32
closed 2026-10-30 (`v2.5.0`, pushed). M31 closed 2026-10-28 (`v2.4.0`,
pushed). M30 closed 2026-10-25 (`v2.3.0`, pushed). M29 closed 2026-10-20
(`v2.2.0`, pushed). Governor & guards (former M25) and F8 (ghost
foreground) remain unscheduled.

Fold queue: M28 proposal B (held), M27 proposal B (held) and M23's two
proposals.
