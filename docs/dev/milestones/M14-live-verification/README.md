# M14 — Live Verification

**Goal:** Every M12 exit criterion that asked for live-tmux or running-daemon
verification is actually verified through the user's door, against a daemon
running the current binary — closing the gap M12's retrospective stated
plainly instead of ticking.

**Status:** done — closed 2026-08-11

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
| 02 | approval-roundtrip-live (phase-02-approval-roundtrip-live.md) | done        |
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

## M14 retrospective — closed 2026-08-11

**Four phases, all `done`** — two of them unplanned, born from live findings,
which is this milestone succeeding rather than slipping. Verdicts: 01
`approved_after_1` (1 hard_fail + 1 bounce), 02 `approved_first_try` (three
verbatim rounds — the spec never bounced; each failed round surfaced a real
defect a sibling phase fixed), 03 `approved_first_try`, 04
`approved_first_try`. One bug doc (bug-01-1, retyped ANSI evidence). Run
inside a single `/rexymcp:auto` session with two blocker stops for PE
decisions, both answered "fix it" — the first hands-off run to cross an
architect↔executor↔PE decision loop twice.

| Phase | Verdict | Rounds / notes |
|---|---|---|
| 01 scripted-live-sweep | approved_after_1 | r1 hard_fail (2 architect spec defects), r2 bounced (bug-01-1), r3 clean |
| 02 approval-roundtrip-live | approved_first_try | r1 blocker → defect #1; r2 hard_fail after surfacing defect #2 (architect takeover ran S5); r3 clean, 7/7 |
| 03 approval-state-persistence | approved_first_try | fix for defect #1; live CHECK-P |
| 04 per-turn-cap-scope | approved_first_try | fix for defect #2; live CHECK-T |

### The headline: two real defects were invisible to 1200+ green tests

Both were found only by going through the user's door, and both had been
green-gated for months:

1. **Runtime approval state reset every turn** (`stream.rs`, in since
   `93fa228`, 2026-06-24): `/approvals revoke`/`on`/`off` lasted one turn;
   an `[A]pprove for session` answer evaporated for config-`false` classes.
   Fixed by phase-03 (delete the turn-end re-derive; semantics pinned in a
   doc comment).
2. **Per-tool/per-turn caps enforced per batch** (`stream.rs`): counters
   were reborn on every assistant message, so a model that sequences its
   calls — which is what models do — never tripped them. The cap was a
   no-op in practice. Fixed by phase-04 (hoist to turn scope).

M12's retrospective declined to tick its live criteria and called the gap "a
claim nobody executed." M14 executed the claims; two of them were false.
That is the entire justification for evidence phases, stated as a result.

### Exit criteria — all verified, all live

Restart-onto-current-binary with sha256 triple identity (phases 01/03/04
transcripts); cross-session visibility through `ask`→chat (phase-01 A/B/D);
`read_pane` through the dispatch path with session-JSONL proof (phase-01 C);
the `tmux_control` approval round trip — prompt with `→ target:` hint, `y`
executes, `n` denies with `User denied execution` (phase-02 F/G/H); status
classification in three states + `/panes` (phase-01 A2-A4/E); the
`APPROVAL_GATED_TOOLS` reconciliation — startup warning for a capped gated
tool, cap enforced on a silent tool, two gated executions under cap=1
(phase-02 S1/J/K); every transcript mechanically captured with a re-runnable
`PASTE MATCH`; both defects fixed in-milestone; four gates green at close. ✓
on all — no unticked criteria this time.

### What worked (keep doing)

- **Prototype-first drafting.** Phases 02 and 04 were specced only after the
  architect ran the mechanism live / compiled the exact patch. Phase-04's
  compile-verified worked examples produced a 43-turn `approved_first_try`;
  phase-02's prototyped mechanics ran verbatim through all three rounds.
- **Verdict-line self-checks + session-JSONL evidence anchors.** Every
  claim reduced to a greppable condition; reviews re-ran them all and found
  zero discrepancies in approved rounds.
- **Flag-not-game, second confirmed occurrence**: the executor reported the
  stale S6 first-fence anchor instead of gaming it (round: phase-01 r3).

### Calibration inventory at close (fold decisions for the PE)

1. **Amend the PASTE MATCH fold (landed only this morning): the extraction
   must anchor the *last* end-to-end entry, not the first fence.** A bounced
   round's superseded entry stays in the doc, so first-match diffs the wrong
   round. Caught by the executor, validated both ways, already applied to
   all three M14 evidence phase docs — the WORKFLOW.md § E2E recipe needs
   the same correction.
2. **The spec must never demand byte-exact pasting of bytes an LLM cannot
   round-trip.** bug-01-1's root cause was shared: ANSI escapes in the
   artifact made the executor's only compliant path impossible. Strip at
   generation (`sed` in the pipe — part of the mechanical generator, not a
   post-edit). Candidate clause for § E2E.
3. **The read-only src-diving stall cost two hard_fails *despite* an
   explicit written ban** (phase-01 r1 before the ban; phase-02 r2 with the
   ban in force and no blocker entry written). The ban held only when every
   remaining action was mechanical. Per § "Give the executor a condition it
   can check": the FAIL→blocker duty probably needs to be a *seeded task*
   ("Task N: if any verdict is FAIL, write the blocker entry and stop"),
   not a prose rule. Hold or fold — two occurrences.
4. **False completion-narrative, one occurrence** (phase-03: "the import
   was already absent" — the diff shows this run deleted it). Existing
   "read the diff, not the self-report" rule caught it at review; no new
   fold, tally it.
5. **Live-environment facts worth keeping** (documented in the phase docs):
   numeric tmux session names make bare `-t <n>` a window-index reference
   (use `"$HS:"`); `daemoneye chat` hangs in a clientless session; one-shot
   `ask` persists no session JSONL — chat is the evidence-bearing door.
