# NEXT

**Active milestone: M30 — Telemetry & dashboard** (release `2.3.0`), scoped
2026-10-20 from `docs/design/daemoneye-2.0.md` § 2.3 / § 2.7 / § 6. See
[`README.md`](milestones/M30-telemetry-dashboard/README.md) for the PE
decisions and the planned phases.

**Active phase:** [phase-10a-rehearsal-fixes](milestones/M30-telemetry-dashboard/phase-10a-rehearsal-fixes.md)
— todo. Phase 10's rehearsal 1 (2026-10-22) passed L1–L7 and filed two
minor findings: chat runs write no `turn_started`, so `top` shows 0 turns
(F1); a finished waited command never records `Foreground::Prompt`, so the
Shells panel shows `-` (F2). Phase 10a fixes both and the `run_tui` doc
nit; then phase 10 (in-progress) resumes with rehearsal 2, the PE run, the
retrospective and the `2.3.0` release. Phases 01–09 are done (2026-10-20
to 22).

M29 closed 2026-10-20 (`v2.2.0`, pushed). M31 (MCP surface) and M32
(Knowledge) follow; Governor & guards (former M25) and F8 (ghost
foreground) remain unscheduled.

Still queued: the M29 deferred list (O5, O6, F8), the M28 `wait: false`
note. Fold queue: M28 proposal B (held), M27 proposal B (held) and M23's
two proposals.
