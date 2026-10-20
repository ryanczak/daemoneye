# NEXT

**Active milestone: M30 — Telemetry & dashboard** (release `2.3.0`), scoped
2026-10-20 from `docs/design/daemoneye-2.0.md` § 2.3 / § 2.7 / § 6. See
[`README.md`](milestones/M30-telemetry-dashboard/README.md) for the PE
decisions and the planned phases.

**Active phase:** [phase-02-run-record](milestones/M30-telemetry-dashboard/phase-02-run-record.md)
— todo: one `run` event per finished run, emitted by the run guard, with
`failure_class` computed from the terminal state and the tally; the ghost
`RunResult` budget carries real spend. Dispatch with
`/rexymcp:dispatch phase-02`. Phase 01 (run-metrics) done 2026-10-20,
approved after one bounce.

M29 closed 2026-10-20 (`v2.2.0`, pushed). M31 (MCP surface) and M32
(Knowledge) follow; Governor & guards (former M25) and F8 (ghost
foreground) remain unscheduled.

Still queued: the M29 deferred list (O5, O6, F8), the M28 `wait: false`
note. Fold queue: M28 proposal B (held), M27 proposal B (held) and M23's
two proposals.
