# M29 — Post-2.0 fixes

**Goal:** fix the defects that shipped in 2.0 and 2.1, found in the M27
and M28 live closes. A session is one thing whether or not it is resident,
the context never names two active panes, bad tool arguments cannot dodge
the budget caps, and the logs stop speaking 1.x.

**Status:** done (scoped 2026-10-18, closed 2026-10-20; released 2.2.0). Every exit criterion verified live on pinky, § Retrospective. **PE signed off on the close 2026-10-20; fold A landed in `WORKFLOW.md`.**

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
| 01 | invalid-args ([phase-01-invalid-args.md](phase-01-invalid-args.md)) — required/optional in the type-mismatch message; `InvalidArgs` never budget-exempt (todo 3, 4) | done (approved_first_try, 2026-10-19) |
| 02 | small-fixes ([phase-02-small-fixes.md](phase-02-small-fixes.md)) — O3 masker `hostname`, O4 result names its shell, `sbx.rs` flake | done (approved_after_1, 2026-10-19) |
| 03 | log-fields ([phase-03-log-fields.md](phase-03-log-fields.md)) — `wait`/`shell` on `command` events alongside `mode`/`pane`; 2.0 daemon.log wording (todo 6) | done (approved_first_try, 2026-10-19) |
| 04 | active-pane ([phase-04-active-pane.md](phase-04-active-pane.md)) — at most one `[ACTIVE PANE]`, none on the chat's own pane (todo 2, F9) | done (approved_first_try, 2026-10-19) |
| 05 | residency ([phase-05-residency.md](phase-05-residency.md)) — resident on presence attach; `NON_RESIDENT_DETACHES` gone; the three sequences as tests (todo 1, 5) | done (approved_after_2, 2026-10-19) |
| 06 | live-close ([phase-06-live-close.md](phase-06-live-close.md)) — architect-run, M14 convention | done (architect-run; rehearsals 1–2, PE run passed 2026-10-20) |
| 06a | live-r1 ([phase-06a-live-r1.md](phase-06a-live-r1.md)) — F1 the focus gets `[ACTIVE PANE]` whoever calls (`source_pane` removed); F2 the catch-up names the alert under a bare tag; doc-comment nit | done (approved_first_try, 2026-10-20) |

## Retrospective (2026-10-20)

**Outcome.** Every exit criterion is met and checked live on pinky.
- A session is one thing: a presence attach loads its history from disk,
  and the same store serves injection, briefs and `handle_ask`.
  `NON_RESIDENT_DETACHES` is gone. All three residency sequences pass as
  tests and live (L5–L7).
- The context never names two active panes, and none when the focus is
  the chat's own pane (L4).
- `PendingCall::InvalidArgs` is never budget-exempt; the type-mismatch
  message says `required` or `optional`.
- O3, O4 and the `sbx.rs` flake are fixed; `command` events carry `wait`
  and `shell` beside `mode` and `pane`, and daemon.log speaks 2.0.

The PE run passed L1–L7; L8 is unit-covered. The suite went from 2410 to
2430.

**Numbers** (from the telemetry store). 6 executor phases, 10 runs, 1,289
executor turns, about 5 hours of executor wall clock. Plus the
architect-run live close: two rehearsals and the PE run.
- **4 `approved_first_try`:** 01 (after a hard fail and a resume), 03, 04,
  06a.
- **`approved_after_1`:** 02. **`approved_after_2`:** 05.
- **2 hard-failed runs:** 01 and 02, both `NoProgressStall`, both traced
  to the spec.
- **Bug docs:** bug-05-1 and bug-05-2.

**What the live close found that nothing else did.** Two findings, both
fixed in 06a and re-checked in rehearsal 2 and the PE run.
- **F1 (major):** `get_shell_context` could never label the focus. Phase
  04's rule labelled only a `source_pane` that was both "not the chat
  pane" and "the focus", and its "callers are unchanged" left every caller
  passing the chat pane. Half the exit criterion was unreachable, and the
  unit tests, written to the same rule, passed. 06a drops `source_pane`:
  the cache's `active_pane` gets the block whoever calls.
- **F2 (minor, pre-dates M29):** the catch-up brief kept each event's
  first line, and a webhook message's first line is the bare
  `[Webhook Alert]` tag. The alert's name never reached the brief.
- **S1 (doc):** the phase doc had the `/approvals` words inverted (`on`
  is auto-approve). Caught in rehearsal 1, fixed before the PE run.
- **O5, O6** (observations, deferred below).

Every one of these passed its unit tests. The lesson is M26's, M27's and
M28's: an entry point is verified only when it is driven live.

**Architect errors, recorded.** Every bounce and both stalls this
milestone were spec errors of one class: **a fact stated about the tree
without reading it at its consumer.**
- 01: a negative assertion said "for that parameter"; the executor read it
  as "anywhere in the message", which the trailing `expected:` list makes
  impossible. Stall, 169 calls.
- 02: the shell line was put in `finish_run`, whose text also feeds remote
  file reads and the ghost paths. Stall, 145 turns, no source edit.
- 05 (bug-05-1): the gotcha allowed building the entry inside a
  `with_sessions` closure, so `from_disk` read files under the store lock.
- 05 (bug-05-2): the worked example said the entry "now exists" after the
  insert, inviting a production `.expect()` that `cleanup_pass` can fire.
- 04 → F1: "callers are unchanged" was never checked against what the
  callers pass.
- 06 → S1: `/approvals off` carried over from M28 without reading
  `cmd_approvals`.

What worked: 03's spec named the two `wait: false` `log_shell_command`
callers after reading them, and the executor hit exactly those. It was
the first clean first dispatch of the milestone.

**Process calibration.**
- **Self-check fence order.** 02's executor pasted the command block
  before the output, so the documented `awk` extracted the script. From
  03 on, the phase docs say "output fence first", and the self-check
  matched on the first fence every time. One occurrence, fixed in the
  template text; held as data.
- **Green bounces stay small.** 05's two bounces ran 59 and 33 turns
  against 255 for the first run. A bounce that names one fact and one
  test lands fast.
- **Pane-fixture cost.** 04 ran 268 turns and ~330 lines against an
  estimated 150, mostly `PaneState` literals; a shared pane-building test
  helper would have halved it. One occurrence, data.

**Fold proposals** (PE sign-off needed before any lands in `WORKFLOW.md`).
- **Proposal A: every Spec fact about the tree is read at its consumer
  before dispatch.** **Folded 2026-10-20 with PE sign-off**, as
  `WORKFLOW.md` § "Pre-dispatch check" rule 5. Four occurrences (01, 02, 05-1, 05-2) plus F1 and
  S1, so this is a fix. Proposed as pre-dispatch rule 5 under
  § "Pre-dispatch check", as a trigger for the existing § "Spec facts
  must be executed": *"For every fact the Spec states about the tree — a
  code site, a caller set, a symbol's lock or lifetime semantics, a
  substring an assertion targets — open the consumer that depends on it
  and read what it does with it. 'Callers are unchanged' names each
  caller and what it passes. A spec fact that was not read is a guess the
  executor cannot tell from a fact."* The M29 occurrences are the
  evidence; M28's "03e said two approval sites where there is one" is the
  same class.

**rexyMCP issues seen this milestone** (for the rexyMCP backlog, not this
repo).
1. **Executor self-reports the wrong model.** 06a's started entry named
   `claude-opus-4-6`; `executor_health` names the GLM endpoint. Second
   occurrence after M28's 03d.
2. **Leaked reasoning in the completion summary.** 04's server-authored
   summary began with `</think>Everything is committed…`.
3. **`NoProgressStall` reads as a code bug to the executor.** 01 re-read
   `dispatch.rs` 40 times for a defect that was in the spec. Nothing in
   the briefing distinguishes "the spec is impossible" from "I have not
   found it yet".

**Deferred to a later milestone.**
1. **O5:** a `wait: false` `command` record carries `exit: 0`, the
   launch's code, not the command's; and a denial's `mode` is still the
   legacy `foreground`. Readers were told to use `wait`/`shell`.
2. **O6:** the status bar reads `turn 1` after a resume, and `session:`
   is blank while an approval panel is up.
3. **F8, ghosts are background-only** (from M28). Unscheduled.
4. **M28's per-result `wait: false` note.** Left as is by PE decision.
5. **GLM fabricating a tool run in prose** (PE run, L1). Model behaviour;
   the history is authoritative and the daemon is not at fault. Worth a
   prompt line if it recurs.

## Notes
