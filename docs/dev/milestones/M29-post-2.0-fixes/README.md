# M29 — Post-2.0 fixes

**Goal:** fix the defects that shipped in 2.0 and 2.1, found in the M27
and M28 live closes. A session is one thing whether or not it is resident,
the context never names two active panes, bad tool arguments cannot dodge
the budget caps, and the logs stop speaking 1.x.

**Status:** in-progress (scoped 2026-10-18)

**Release:** 2.2.0

**Depends on:** M27 (presence and injection), M28 (shell results).

**Sources:** `docs/design/daemoneye-post-2.0-todo.md` (items 1–6) and the
M28 retrospective's deferred list (O3, O4, the `sbx.rs` flake). F8 (ghost
foreground) and the M28 `wait: false` note are **not** in scope.

**PE decisions (2026-10-18):**

1. **Residency:** a presence attach makes the session resident by loading
   its history from disk. One store serves injection, briefs and
   `handle_ask`. `NON_RESIDENT_DETACHES` is deleted, which closes todo
   item 5.
2. **F9:** when the active pane is the chat's own excluded pane, no pane is
   labelled `[ACTIVE PANE]`. There is never more than one active line.
3. **Budget caps:** a `PendingCall::InvalidArgs` is never budget-exempt,
   whatever tool it names.
4. **Log fields:** `command` events gain `wait` and `shell` **alongside**
   `mode` and `pane`, which stay for existing readers. The daemon.log line
   speaks in 2.0 terms.

**Exit criteria:**

- **Invalid args.** The type-mismatch message says `required` or
  `optional` according to the schema. Invalid-args calls to an
  approval-gated tool count against the per-turn, per-session and per-tool
  caps.
- **Small fixes.**
  - O3: the masker no longer rewrites the output of the `hostname`
    command, and still masks `HostName`/`IdentityFile` lines in ssh config.
  - O4: a successful `run_terminal_command` result names the shell it ran
    in.
  - The `sbx.rs` flake is fixed by holding the test `HOME`.
- **Log fields.** `command` events carry `wait` and `shell`. The README's
  audit-log section documents old and new fields. daemon.log no longer
  says `foreground command approved` or `background command approved`.
- **F9.** Inside tmux, the context holds at most one `[ACTIVE PANE]` line,
  and none when the user's focus is the chat pane.
- **Residency.** Each of these passes as an end-to-end test and is
  checked live:
  - restart → reconnect → alert → ask;
  - restart → reconnect → quit with no ask → alert → resume;
  - a fresh `chat --resume` of a session the daemon has never loaded.

  `NON_RESIDENT_DETACHES` no longer exists.
- Live close on pinky per the M14 convention. `tests/doc_truth.rs` passes.
  `2.2.0` is released per `REXYMCP.md` § Versioning.

## Architecture references

- `docs/design/daemoneye-post-2.0-todo.md` — the problem statements.
- `docs/dev/milestones/M27-tmux-observer/README.md` § Retrospective — F5,
  F8, F9 and F11.
- `docs/dev/milestones/M28-interactive-shells/README.md` § Retrospective —
  O3, O4 and the flake.

## Phases

Drafted one at a time with `/rexymcp:architect next`. Small, independent
fixes come first. Residency is last before the close, and it may split when
it is drafted.

| # | Phase | Status |
|---|---|---|
| 01 | invalid-args ([phase-01-invalid-args.md](phase-01-invalid-args.md)) — required/optional in the type-mismatch message; `InvalidArgs` never budget-exempt (todo 3, 4) | todo |
| 02 | small-fixes — O3 masker `hostname`, O4 result names its shell, `sbx.rs` flake | not drafted |
| 03 | log-fields — `wait`/`shell` on `command` events alongside `mode`/`pane`; 2.0 daemon.log wording (todo 6) | not drafted |
| 04 | active-pane — at most one `[ACTIVE PANE]`, none on the chat's own pane (todo 2, F9) | not drafted |
| 05 | residency — resident on presence attach; `NON_RESIDENT_DETACHES` gone; the three sequences as tests (todo 1, 5) | not drafted |
| 06 | live-close — architect-run, M14 convention | not drafted |

## Notes
