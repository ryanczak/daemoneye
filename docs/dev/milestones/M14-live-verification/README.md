# M14 — Live Verification

**Goal:** Every M12 exit criterion that asked for live-tmux or running-daemon
verification is actually verified through the user's door, against a daemon
running the current binary — closing the gap M12's retrospective stated
plainly instead of ticking.

**Status:** planning

**Depends on:** M12 — Full-View tmux Integration, M13 — Chat UX Polish

**Exit criteria:**

- The daemon is restarted onto the current binary before any check, and the
  evidence shows it: `daemoneye status` output with version and start time
  captured at the top of the verification transcript.
- **Cross-session visibility, two live tmux sessions:** through `daemoneye
  ask` (the user's door, not the capture helpers), `list_panes` shows the
  foreign-session section, `find_in_panes` with `scope: "all"` returns a match
  from a pane in the other session, and `get_terminal_context` with
  `scope: "all"` includes foreign-session metadata.
- **`read_pane` through the tool dispatch path:** a live pane's content
  returned via an `ask` turn, with the corresponding tool-call record in
  `events.jsonl` — not a direct call to the knowledge function.
- **`tmux_control` approval round trip, live:** the `ToolCallPrompt` renders
  with the window-relative `target_pane` hint and the visual highlight; `Y`
  executes the action; `N` denies it and the AI is told; the highlight is
  removed in both outcomes.
- **Status classification and `/panes` live:** `status:` labels and the
  inspector observed against real panes in at least three states (shell
  prompt, running command, dead).
- **`APPROVAL_GATED_TOOLS` budget reconciliation live:** with a
  `[limits.per_tool]` cap configured, a capped non-gated tool hits its limit
  against the running daemon, and the M12 behaviour change is observed as
  shipped — `spawn_ghost_shell` / `delete_schedule` cappable, `create_agent` /
  `delete_agent` exempt.
- Every transcript is captured mechanically per WORKFLOW.md § "End-to-end
  verification" (PIPESTATUS exit markers, PASTE MATCH self-check).
- Any defect found is filed (bug doc or new phase) and either fixed
  in-milestone or explicitly carried with a reason; four gates green at close.

## Architecture references

- `docs/design/tmux-integration.md` — the D1–D7 design these surfaces ship.
- `docs/dev/milestones/M12-tmux-integration/README.md` § "Exit criteria — what
  is verified, and what is not" — the authoritative gap list this milestone
  closes.
- `CLAUDE.md` § Request/Response lifecycle — the approval round trip under
  test.

## Phases

| #  | Phase                                                        | Status |
|----|--------------------------------------------------------------|--------|
| 01 | scripted-live-sweep (phase-01-scripted-live-sweep.md)        | done        |
| 02 | approval-roundtrip-live (phase-02-approval-roundtrip-live.md) | blocked |
| 03 | approval-state-persistence (phase-03-approval-state-persistence.md) | done        |
| 04 | per-turn-cap-scope (phase-04-per-turn-cap-scope.md) | done        |

Phase 01 covers everything drivable without a human at the approval prompt:
daemon restart onto the current binary, the two-session fixture, `list_panes`
/ `find_in_panes` / `get_terminal_context` / `read_pane` through `ask`, status
classification, `/panes`. Phase 02 covers the human-in-the-loop surfaces: the
`tmux_control` approval round trip and the per-tool budget-cap behaviour.
Defect fixes discovered by either become phase-03+ (or bug docs on the phase
that found them).

## Notes

### Why this milestone exists

M12's retrospective declined to tick three of its own exit criteria because
their wording asks for more than a unit test ("verified with two live tmux
sessions", "through the tool dispatch path", "end-to-end … round-trips the
approval flow") and no live run was ever made — the daemon running at M12
close was 21 h old and predated every M12 commit. M13 then proved the risk is
real: phase-05 was spec-correct and `approved_first_try`, and the live check
still showed the symptom, twice, producing two unplanned phases. This
milestone is deliberately small: it verifies; it does not build.

M13's own live criteria (colors, mid-turn reanchor) are already live-verified
and closed — they are **not** in scope here.

### Scope facts (derived 2026-08-10 from the M12 retrospective; re-verify at drafting)

- The unit-only surfaces: cross-session cache/`list_panes` (seeded foreign
  panes, no second real session), `read_pane` (dispatch fixture covers
  registration; the tool tests call the knowledge function directly),
  `tmux_control` (ghost-denial predicate unit-tested; prompt round trip
  never exercised), status classification / `find_in_panes` / `/panes`
  (unit-level).
- The `APPROVAL_GATED_TOOLS` reconciliation is "correct per the exemption's
  own rationale and pinned by tests, but has not run against a live daemon"
  (NEXT.md, carried item 3 out of M12).

### Design decisions

- **Verify through the user's door** (`daemoneye ask` / `daemoneye chat`), per
  the M7–M10 rule — an in-process probe records a result the shipped binary
  never produces.
- **Evidence lands in the phase docs** under the full E2E discipline,
  including the folds landed at M13 close (PIPESTATUS markers, first-fence
  PASTE MATCH).
- **Open question for phase-02 drafting:** whether the approval round trip is
  executor-drivable (a script answering its own prompt via `tmux send-keys`
  from a second pane) or a PE-assisted live session recorded by the
  architect. Decide at drafting by prototyping the send-keys approach first —
  per § "Derive every spec fact from its source", do not spec it unrun. If it
  is PE-assisted, phase-02 is executed outside the executor loop and says so;
  the telemetry point is knowingly foregone, not skipped silently.
- **A restart is a precondition, not a step to hide:** the transcript must
  open with `daemoneye status` proving version and start time, so the
  evidence cannot silently describe a stale binary — the exact failure M12
  named.
