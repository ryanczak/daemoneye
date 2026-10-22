# NEXT

**Active milestone: M30 — Telemetry & dashboard** (release `2.3.0`), scoped
2026-10-20 from `docs/design/daemoneye-2.0.md` § 2.3 / § 2.7 / § 6. See
[`README.md`](milestones/M30-telemetry-dashboard/README.md) for the PE
decisions and the planned phases.

**Active phase:** [phase-08-top-shells](milestones/M30-telemetry-dashboard/phase-08-top-shells.md)
— todo: the Shells panel of `daemoneye top`. `ShellInfo` gains
`foreground` and `last_line`, filled in `handle_shell_list`, and `top
--once` gains a Shells section (`daemon not running` when down). The TUI
is phase 09 and the live close is 10. Dispatch with
`/rexymcp:dispatch phase-08`. Phases 01–07 are done (2026-10-20 to 22).

M29 closed 2026-10-20 (`v2.2.0`, pushed). M31 (MCP surface) and M32
(Knowledge) follow; Governor & guards (former M25) and F8 (ghost
foreground) remain unscheduled.

Still queued: the M29 deferred list (O5, O6, F8), the M28 `wait: false`
note. Fold queue: M28 proposal B (held), M27 proposal B (held) and M23's
two proposals.
