# NEXT

**Active milestone: M30 — Telemetry & dashboard** (release `2.3.0`), scoped
2026-10-20 from `docs/design/daemoneye-2.0.md` § 2.3 / § 2.7 / § 6. See
[`README.md`](milestones/M30-telemetry-dashboard/README.md) for the PE
decisions and the planned phases.

**Active phase:** [phase-09-top-tui](milestones/M30-telemetry-dashboard/phase-09-top-tui.md)
— done (2026-10-22, approved_after_1): the live ratatui screen of `daemoneye top`. It polls at 500 ms,
re-reads on a fingerprint change or every 5 s, the selection follows the
newest live run until the user scrolls, `/` filters Activity and `q`
quits. It is verified end to end in a private tmux. The live close is
phase 10, drafted next with `/rexymcp:architect next`. Phases 01–09 are
done (2026-10-20 to 22).

M29 closed 2026-10-20 (`v2.2.0`, pushed). M31 (MCP surface) and M32
(Knowledge) follow; Governor & guards (former M25) and F8 (ghost
foreground) remain unscheduled.

Still queued: the M29 deferred list (O5, O6, F8), the M28 `wait: false`
note. Fold queue: M28 proposal B (held), M27 proposal B (held) and M23's
two proposals.
