# M24 — Run Manager

**Goal:** Collapse the four execution loops — a chat turn, a ghost, a scheduled
job, a delegated agent — into one `Run` model with a per-run turn log,
structured `RunResult` + briefing, cooperative cancellation, and briefing-seeded
resume; and make **`run_terminal_command(background=true)` fall out of it** as an
un-awaited chat Run against a dedicated substrate shell, replacing the tmux
`de-bg-*` path.

**Status:** **all 16 phases done 2026-09-12 — awaiting PE sign-off to close.** Scoped 2026-09-09, **PE sign-off on scope 2026-09-09**
(scope + the M24-ahead-of-M21–M23 reorder both confirmed; background landing
mid-milestone accepted).

**Depends on:** M20 (shell engine — the substrate a Run holds and background
runs on; 08a cancel signalling; 08b's dedicated-shell rule). Not on M21–M23.

**Exit criteria:**

- **`background=true` runs on the substrate, end to end (the motivating case).**
  A `daemoneye chat` turn with `run_terminal_command(background=true)` returns
  **immediately** with a run id, the command runs in a **dedicated** shell (not
  the owner's reused foreground shell — 08b's poisoning rule), and completion
  arrives later as `[Background Task Completed] … (exit N)` carrying the real
  exit code from the marker. The tmux `de-bg-*` path is gone under
  `backend = "pty"`. Evidence anchor: the run turn-log + the completion event,
  architect-run through the user's door.
- **One `Run` for four kinds.** Chat, ghost, scheduled and delegated execution
  all construct a `Run { kind, agent, shells, budget, turn_log, state }`; there
  is one turn-log format at `var/log/runs/<id>.jsonl`; `daemoneye runs` lists
  every kind.
- **`RunResult` is structured**, `complete | hard_fail | budget_exceeded |
  cancelled`, with a bounded briefing (goal, criteria, ≤5 working files,
  attempts ≤200 chars, blocker, budget left). The ghost mailbox payload is a
  `RunResult`, not today's ad-hoc string.
- **Cooperative cancellation for every run kind.** `daemoneye stop <run>` from
  any terminal writes `var/run/stop-<run>`; the run ends
  `Cancelled{stage, turns_done}`. Generalises today's chat-only `Request::Cancel`.
- **Briefing-seeded resume.** `daemoneye run resume <id> "<directive>"` restarts
  from runbook + briefing + directive + current state (not transcript
  rehydration); the `await_agent_result` → re-spawn path uses it.
- **Seeded tasks.** A runbook `## Steps` section becomes a ghost run's task
  list, coverage-checked before the run may complete — "a condition it can
  check, not an instruction it can agree with."
- **The governor is explicitly OUT of scope** (it is M25 and needs `Category`
  on `TOOLS` from M22). The flat per-turn/-session/-tool budget stays the
  safety net until then. Cross-run telemetry (`RunRecord` → scorecard/profile)
  is a **candidate**, likely its own phase or deferred to M25 with the governor.
- All four gates green: `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
  Live checks are architect-run per the M14 convention (isolated `HOME`,
  canned-AI stub, real tmux client; run turn-log / event as evidence anchor).

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.3 "Run manager" — the `Run` struct and the
  rexyMCP file anchors to quote verbatim in phase docs (the M16 approach:
  `executor/src/phase/result.rs`, `phase/briefing.rs`, `mcp/src/jobs.rs`,
  `stop_watcher.rs`, `resume.rs`, `agent/tasks.rs`, `store/sessions/event.rs`).
- `docs/design/daemoneye-2.0.md` § roadmap row **M24** (the scope) and row
  **M25** (the governor, deliberately excluded here).
- `docs/dev/milestones/M20-shell-engine/README.md` — the substrate this builds
  on; § Retrospective for the "background is a detached Run" reasoning that
  led here.

## Phases

Proposed, pre-measurement — **each will be re-derived and may split at drafting
on a measured seam** (M20 ran 9→16 that way). Ordering constraint: the Run core
and async-completion machinery must precede the background payoff, which must
precede the full four-loop unification's completion.

| #  | Phase | Status |
|----|-------|--------|
| 01 | run-core ([phase-01-run-core.md](phase-01-run-core.md)) — `RunId`/`RunKind`/`RunState`/`Budget`/`Briefing` in `src/daemon/run/`, pure, no wiring | **done** (approved_first_try, 2026-09-09) |
| 02a | turn-log ([phase-02a-turn-log.md](phase-02a-turn-log.md)) — `var/log/runs/<id>.jsonl` record/writer/reader + the path constructor and lifecycle row a new artifact dir requires | **done** (approved_first_try, 2026-09-09) |
| 02b | run-log-tools ([phase-02b-run-log-tools.md](phase-02b-run-log-tools.md)) — `run_log_tail` / `run_log_search` / `get_turn` as a deferred `runs` group; clamped limits + `get_turn` as the uncapped escape hatch | **done** (approved_first_try, 2026-09-09) |
| 03 | run-result ([phase-03-run-result.md](phase-03-run-result.md)) — `RunStatus`/`Cancellation`/`RunResult` + `Briefing::render()`, ported from the rexyMCP anchors; de-duplicates the four wire strings out of `log_read.rs` | **done** (approved_first_try, 2026-09-09) |
| 04 | cancel ([phase-04-cancel.md](phase-04-cancel.md)) — reason-carrying run cancel registry, `var/run/stop-<run>` sentinel + watcher, startup sweep, `daemoneye stop [<run>]` | **done** (approved_after_1, 2026-09-10) |
| 05a | detached-command ([phase-05a-detached-command.md](phase-05a-detached-command.md)) — `spawn_command` + `CommandHandle` (start, don't await) and `dedicated` shells excluded from reuse; pure substrate, no daemon file touched | **done** (approved_after_1, 2026-09-10) |
| 05b | background ([phase-05b-background.md](phase-05b-background.md)) — the executor arm on the substrate: un-awaited chat Run on a dedicated shell + marker completion → `[Background Task Completed]` **(the payoff)** | **done** (approved_first_try, 2026-09-10) |
| 06a | chat-run-log ([phase-06a-chat-run-log.md](phase-06a-chat-run-log.md)) — a chat turn gets a `RunId` + `var/log/runs/<id>.jsonl` via a `RunLogGuard` whose `Drop` writes the one `RunFinished`; cancellation untouched | **done** (approved_after_1, 2026-09-10) |
| 06b | chat-run-cancel ([phase-06b-chat-run-cancel.md](phase-06b-chat-run-cancel.md)) — delete `daemon/cancel.rs`; session→run map; both cancel doors converge on `cancel_run`; `daemoneye stop <run>` cancels a live chat turn | done        |
| 07a | ghost-run-log ([phase-07a-ghost-run-log.md](phase-07a-ghost-run-log.md)) — a ghost gets a `RunId` + `RunLogGuard` + the right `RunKind`; first production `TurnStarted` emitter; Complete/HardFail/BudgetExceeded | **done** (approved_first_try, 2026-09-11) |
| 07b | ghost-run-cancel-result ([phase-07b-ghost-run-cancel-result.md](phase-07b-ghost-run-cancel-result.md)) — cancel check at ghost turn top (`daemoneye stop <run>` reaches a ghost); mailbox carries `RunResult` | **done** (approved_after_2, 2026-09-11) |
| 08 | scheduled-run ([phase-08-scheduled-run.md](phase-08-scheduled-run.md)) — `RunKind::Scheduled` gets its first production constructor; wrap the 435-line body, derive the terminal state from the store | **done** (approved_first_try, 2026-09-12) |
| 09a | resume-seed ([phase-09a-resume-seed.md](phase-09a-resume-seed.md)) — the pure seed: terminal briefing out of the turn log + directive, "not transcript rehydration" made structural | **done** (approved_first_try, 2026-09-12) |
| 09b | resume-wiring ([phase-09b-resume-wiring.md](phase-09b-resume-wiring.md)) — `daemoneye run resume <id> "<directive>"`: IPC request, CLI subcommand, ghost re-spawn from the seed; retires `resume_seed` for `resume_plan` | **done** (escalated, 2026-09-12) |
| 09c | seeded-tasks ([phase-09c-seeded-tasks.md](phase-09c-seeded-tasks.md)) — runbook `## Steps` parsed; completion gated on literal `STEP <n>: DONE\| **done** (approved_first_try, 2026-09-12) | **done** (approved_first_try, 2026-09-12) |
| 10 | run-listing ([phase-10-run-listing.md](phase-10-run-listing.md)) — `daemoneye run list` + `/runs` `/run <id>` `/stop <id>`; the first reader of `var/log/runs/` | **done** (approved_first_try, 2026-09-12) |

## Notes

- **Adding a directory under `~/.daemoneye/` trips FIVE gates, not three**
  (learned in 02a, 2026-09-09). A new artifact dir needs: (1) a path
  constructor, (2) eager creation in `ensure_dirs()`, (3) a `LifecycleEntry`
  in `src/config/lifecycle.rs`, (4) a leaf in `RUNTIME_TREE`
  (`src/config/runtime_tree.rs`) — `every_policy_path_appears_in_tree` fails
  without it — and (5) the matching line in the shipped
  `assets/memory/knowledge/agent-runtime-layout.md`, because
  `render_matches_shipped_asset` compares the rendered tree to that asset.
  Verified by deleting the leaf: gates 4 and 5 go red together. **Phase-10
  faces this again** when it adds the `[runs]` retention key and flips the
  `var/log/runs` row from `Pending`.


- **Phase 07 split at drafting (2026-09-11) into 07a and 07b**, on the same
  additive/rewire seam that split 06. 07a is purely additive — a ghost gains an
  identity and a turn log, and a §D criterion asserts a **zero diff on
  `src/agents/mailbox.rs`**, so "the mailbox is untouched" is checkable rather
  than promised. 07b is the rewire: a cancel check at the ghost turn top (the
  design's "checked at turn top") plus the mailbox payload becoming a
  `RunResult`. Measured at drafting: **`do_ghost_turn` is 760 lines
  (`src/daemon/ghost.rs:304-1063`) with four `bail!` exits** plus the implicit
  end — the same measurement that made 06a a `Drop` guard, and the reason 07a
  puts the guard in the 16-line `trigger_ghost_turn` wrapper (`:287-302`) that
  every exit already funnels through, leaving the 760-line body alone.

- **Nine phases wrote `var/log/runs/` and nothing read it.** Phase-10 is the
  first reader: every earlier phase addressed a run by an id the user had to
  find by hand, and 09b shipped `daemoneye run resume` with that limitation
  recorded in its own Out of scope. Worth noting as a shape — a milestone can
  build a durable artifact for a long time before anything makes it
  discoverable, and the discovery surface is easy to keep deferring because
  nothing fails without it.

- **The exit clause says `daemoneye runs`; the command is `daemoneye run list`.**
  09b established a `Run` subcommand group, and a second top-level `runs` beside
  `run` would be two spellings of one noun. The criterion is met in substance
  and the wording deviation is recorded rather than silently absorbed.

- **09c makes the mutation checks a numbered `## Spec` task**, because 09b's
  went unrun: their instructions lived outside `## Spec` and so were never
  seeded into the executor's task list. That is the **fourth** occurrence of
  M12's fold — *"the executor responds to conditions it can evaluate, not
  instructions it can agree with… making it a seeded `## Spec` task worked
  immediately"* — which had been applied to the E2E entry and never to mutation
  checks. Carry the Spec-task form forward in every phase doc from here.

- **"A condition it can check, not an instruction it can agree with" got a
  concrete mechanism in 09c**, and the codebase already had the precedent *and*
  the cautionary tale. `parse_ghost_trigger` (`src/webhook/process.rs:258`)
  parses a literal `GHOST_TRIGGER: YES/NO`, and its doc comment records that the
  loose predecessor was removed "because a model remarking 'no ALERT condition'
  could spuriously launch a ghost". Step coverage therefore counts a step only
  when a line *parses* as `STEP <n>: DONE|SKIPPED` — a transcript saying "I
  could not complete step 2" must leave step 2 uncovered, pinned as the
  load-bearing negative test.

- **09b deletes a function 09a shipped, on purpose.** `resume_seed` had **zero
  production callers** (verified at 09b drafting), and `resume_plan` supersedes
  it by also resolving the runbook. `STANDARDS.md` § 2.2 — "if a symbol is
  unused, delete it" — beats keeping a second entry point on the speculation
  that phase-10 might want it. A staged phase that leaves a stepping stone
  behind should retire it in the phase that steps off it, not later.

- **Only a ghost run is resumable, because only a ghost run names a runbook.**
  Chat, scheduled and delegated runs are each refused with a reason the user
  reads, and each refusal is a named negative test. The subtle one is
  `Delegated`: it *is* autonomous, but it names an agent rather than a runbook,
  so this path cannot resume it.

- **Phase 09 split three ways at drafting (2026-09-12).** The charter row
  bundled two unrelated capabilities — briefing-seeded resume, and seeded tasks
  from a runbook `## Steps` section — and resume itself splits again on the
  pure/wiring seam that has now worked for 05, 06, 07 and 08. **09a** builds the
  seed text and nothing else (no CLI, no IPC, no spawn); **09b** wires it;
  **09c** is seeded tasks, which shares no code with either — verified at
  drafting that **no `## Steps` parsing exists anywhere in the tree**.

- **The "not transcript rehydration" clause is enforced by a signature, not a
  promise.** 09a's `render_seed` takes `(kind, Option<&Briefing>, directive)`
  and a criterion pins that `TurnLogRecord` does not appear in it — the function
  *cannot* replay turns. Design § 2.3's wording became a structural property
  rather than a review note.

- **Two unrelated types are both called "briefing", and 09a pre-injects the
  distinction as a table.** `crate::daemon::briefing::read_briefing(agent)` is
  G4's free-form per-agent prose; `crate::daemon::run::Briefing` is phase-01's
  bounded struct living inside `RunState`. A criterion asserts the G4 module is
  not referenced from anywhere in `src/daemon/run/`.

- **Phase 08 wraps rather than threads, for the third time.** `run_scheduled_job`
  is 435 lines with seven returns and **eleven** `store.mark_done` sites, and it
  returns `()`, not `Result` — so there is no single value to map. The phase
  renames the body to `_inner`, wraps it, and **reads the outcome back out of
  the store** (`ScheduleStore::list()` → `JobStatus`) rather than rewriting
  eleven call sites the compiler could not check. Same instinct as 06a's and
  07a's guards: in a long function, a hand-placed exit is a missed exit.
  The load-bearing gotcha, pre-injected from `JobStatus`'s own doc comment: a
  successful **`Every`** job reads back as **`Pending`**, not `Succeeded`,
  because `mark_done` advances and reschedules it — a mapping that treats
  `Pending` as "never finished" would mark every successful recurring job a
  failure.

- **A scheduled ghost job deliberately produces two run logs** — a `Scheduled`
  parent and the `Ghost` child it started (`trigger: "scheduled"`). Recorded at
  08's drafting so a later reader does not "fix" it into one.

- **Scheduled-job cancellation is deliberately out of 08.** The exit criterion
  says every run kind cancels; the honest position is that the residual gap is
  small and points at superseded code. The script branch runs inside a tmux
  `de-sj-*` window the 2.0 substrate replaces, so cancel logic written against
  it would be discarded; and the branch that actually runs a long AI loop — a
  scheduled **ghost** — already cancels through its own `Ghost` run (07b). What
  is left uncancellable is a synchronous script that ends on its own.

- **The rule the 08 review earned, and the shape of the fold to come:** a
  structural criterion must be validated against the tree **as it will look
  after the phase**, not only before. All three of 08's bad criteria returned
  their stated value on the pre-phase tree — which is what was checked — and
  drifted only once the phase's own code and tests existed. With the 07a/07b
  collisions that is **eight** instances across four phases of a criterion
  matching more than it was pinning, and this is the generalisation that covers
  all of them. Still **awaiting PE sign-off** before anything lands in
  WORKFLOW.md.

- **Self-referential greps: a doc criterion that quotes its own match string
  matches itself.** Found at the 07b review while writing the bounce criteria:
  `grep -c 'as predicted' <phase doc>` matched the criteria block that quotes
  it, not just the Update Log entry it was meant to police. Doc-content
  criteria must be **scoped to the section they police** —
  `awk '/^## Update Log/,0' <doc> | grep -c …`. Same family as the substring
  collisions below, and together they make **three** occurrences of "the grep
  matched more than the thing it was pinning" (07a ×1, 07b ×2). Per
  WORKFLOW.md § Calibration that is fold territory — **PE sign-off needed**
  before anything lands in WORKFLOW.md.

- **Substring collisions in structural greps are now a two-occurrence
  pattern** (07a and 07b, both caught pre-dispatch by executing the criteria).
  07a: `ghost_runbook` is the `schedule_command` tool parameter, so the field
  became `ghost_runbook_name`. 07b: `grep -c '_runbook_name'` matches
  `ghost_runbook_name` — the very field 07a added — so the dead-binding
  criterion greps `let (_runbook_name` instead. **Before pinning any
  identifier-shaped grep, run it and look at the lines it matches**, not just
  the count. One more occurrence and this is a WORKFLOW.md fold.

- **Phase 07b scopes cancellation to the turn top only.** The design § 2.3
  names two check points — turn top *and* inside the model-call `select!`.
  The in-turn half needs the AI-event loop at `src/daemon/ghost.rs:551`
  restructured, which would blow the one-session budget; turn-top cancellation
  is bounded by one ghost turn and is strictly better than uncancellable.
  Recorded as an explicit Out-of-scope item rather than a silent omission.

- **Two drafting traps found by running the criteria before dispatch (07a,
  2026-09-11).** (1) `ghost_runbook` is **already** the `schedule_command` tool's
  parameter name, live in five files — a `SessionEntry` field by that name would
  make every structural grep in the phase match unrelated code; the field is
  `ghost_runbook_name`. (2) `src/daemon/ghost.rs` already carries one production
  `.expect` (`:863`, `CostRecord serialization is infallible`, with an INVARIANT
  comment), so the usual "→ 0" no-unwrap criterion is unsatisfiable there and is
  pinned to "unchanged at 1" instead. Both were caught only by executing each
  criterion against the tree — see [[spec-facts-must-be-executed]].

- **`TurnLogEvent::TurnStarted` had no production emitter** before 07a — the
  variant shipped in 02a and only tests wrote it. Verified at drafting:
  `grep -rl TurnStarted src/ --include='*.rs'` outside `run/turn_log.rs` and
  `run/log_read.rs` returned **0 files**. The ghost loop, which already counts
  its turns, is the first writer.

- **Phase 06b's one design decision, made at drafting (2026-09-10):
  `watch_stop_sentinel` loses its `RunCancelHandle` parameter** and calls
  `cancel_run(&id, UserStop)` instead. The watcher already consulted the
  registry to decide whether to exit, so a separate handle was always
  redundant — and the "handle separate from the registry" shape is exactly what
  produced **bug-04-1**, where a test observed a receiver belonging to a
  different channel. With both cancel doors (`Request::Cancel` via the
  session→run map, and the sentinel) converging on `cancel_run`, a turn has
  **one** signal and there is no second channel to fall out of sync.
  **`tokio::sync::watch::Sender` is `Clone` on tokio 1.53 — verified with a
  compile probe at drafting** — so handing out a cloned handle would also have
  worked; it is rejected on design grounds, not capability. Cost: the signature
  change touches two existing tests in `run::cancel`, which the phase rewrites
  to observe a registered run's own signal (making them better tests).

- **Phase 06 split at drafting (2026-09-10) into 06a and 06b**, on the
  additive/rewire seam. 06a is **purely additive** — a chat turn gains a
  `RunId` and a turn log, and nothing existing changes behaviour; § G asserts a
  **zero diff on `src/daemon/cancel.rs`**, so "cancellation is untouched" is a
  checkable property rather than a promise. 06b is the risky half: collapsing
  the session-keyed `bool` registry onto the run-keyed reason-carrying one,
  adding the session→run map `Request::Cancel { session_id }` needs to resolve,
  and wiring `watch_stop_sentinel`. Measured at drafting: **`run_conversation_loop`
  is 1321 lines (`src/daemon/stream.rs:57-1378`) with six exit paths** — five
  explicit returns at `:174`, `:203`, `:227`, `:791`, `:1004` plus the implicit
  end. That measurement is what made 06a a `Drop` guard rather than six
  hand-placed `RunFinished` calls: in a function that long a hand-wired exit is a
  missed exit, and a missed exit writes nothing at all, leaving a run that looks
  like it never ended. The guard records the last outcome set and, if none was,
  writes a `HardFail` naming the omission — a forgotten path becomes a visible
  wrong answer instead of silence.

- **Phase 05 split at drafting (2026-09-10) into 05a and 05b**, on a measured
  seam rather than a size guess. The charter bundled two substrate primitives
  (a command you start but do not await; a shell nobody else may be handed)
  with the executor wiring that consumes them (the `PendingCall::Background`
  arm, the `[Background Task Completed]` event, `Run` + turn-log + cancel
  registration, and retiring the tmux `de-bg-*` path). The seam is the module
  boundary: **05a is entirely `src/shell/`, 05b entirely `src/daemon/`** — 05a
  asserts a zero diff under `src/daemon/`, so the split is a checkable
  property, not a description. 05a changes no user-visible behaviour; the exit
  criterion belongs to 05b. Measured at drafting: `run_command`
  (`src/shell/client.rs:111-190`) needs splitting into a shared handshake plus a
  shared read-to-marker loop before a detached variant can exist without
  duplicating the marker protocol — that refactor plus the `dedicated` field's
  six construction sites is a phase on its own.

- **Phase 02 split at drafting (2026-09-09) into 02a and 02b**, on the tool
  checklist rather than on size guessing. The charter bundled the turn-log
  writer with three AI tools (`run_log_search`, `run_log_tail`, `get_turn`);
  each tool carries `CLAUDE.md`'s 11-step add-a-tool checklist plus the
  `doc_truth` gates that hold the `CLAUDE.md` and `README.md` tool tables and
  their counts in sync. Three tools is a phase on its own. 02a ships the record,
  writer and reader; 02b ships the tools that read them. Same seam-by-measurement
  practice as M20's 05a/05b/05c.

- **Retention for `var/log/runs` is deliberately deferred to phase-10.** The
  lifecycle row 02a adds is `Pending { owned_by: "M24 phase-10" }` with
  `config_key: None`, because a retention key would drag `assets/etc/config.toml`
  and two `doc_truth` config gates into a phase that is otherwise about a file
  format. **Phase-10 must pick this up** — flip the row to `Implemented` and add
  the sweep — or M24 closes with a `Pending` row, which is what the M20
  lifecycle audit caught last milestone.


- **Roadmap reorder (2026-09-09, PE decision — signed off).** The
  design roadmap orders M21 Host/SSH → M22 Tool surface → M23 Client decoupling
  → **M24 Run manager**. The PE chose to execute the **Run manager next**,
  ahead of M21–M23, because `background=true` is the pressing functional gap and
  the substrate reframes it as "an un-awaited Run" — so background falls out of
  the Run model rather than being built standalone and torn out later.
  **Dependency check that makes the reorder safe:** the Run manager needs only
  the shell substrate (M20, shipped) and 08a's cancel signalling (shipped). Its
  one forward dependency — the **governor** requiring `Category` on `TOOLS`
  (M22) — is scoped into **M25**, not here; the flat turn budget remains the
  safety net until M25. `/stop` (sentinel) and `run_log_*` (broker tools) do not
  need M23's chat-outside-tmux work. Milestone **number kept as M24** (its
  design identity) despite executing out of order, so § 2.3 and its rexyMCP
  anchors stay in sync; execution order ≠ directory number.

- **Why background waits for the Run core (the tradeoff the PE accepted).**
  Background could have shipped as a small standalone phase now (dedicated
  shell + marker watcher + the existing `[Background Task Completed]` event).
  The PE chose (b): let it fall out of the Run model instead — less rework,
  since a bespoke completion/cancel path would be unified into `Run` anyway. The
  cost is that background lands **mid-milestone** (after phases 01–04), not
  immediately.

- **The substrate already reframed the problem** (M20 retrospective): every
  substrate shell is detached/daemon-owned, so "foreground vs background" is no
  longer a property of the *shell* but of the *call* — does the turn await the
  end marker or not. Foreground = today's synchronous `run_in_shell`;
  background = don't await, deliver the marker as an event. Same shell-host,
  same marker, same cast. Visibility (watch it live) becomes orthogonal and
  on-demand — attach (M23) or tail the cast.

- **Still open for the PE, carried from M20:** the fixture-type fold candidate
  (one occurrence, held); test-hygiene litter in the real `~/.daemoneye/` from
  `shell::launch` tests (agreed to clean up in a milestone — a natural fit here
  or in M22).

- **FOLDED 2026-09-10 (mid-milestone, at PE direction) as WORKFLOW.md § "A check
  the executor cannot satisfy is a bounce you wrote yourself"** — reached four
  occurrences with phase-05b's mis-scoped `shell_for_owner` grep and phase-06a's
  phantom sixth exit + line-number anchors (the latter a 600-turn budget loss).
  Original note follows.
- **Fold candidate — the architect prescribed a falsification test that could
  not falsify (occurrence 2, HELD at the time).** Phase-03's mutation 2 named the
  `hard_fail` arm and demanded a `log_read` test go red; `log_read` has no test
  that renders a `HardFail` state, so the check was unrunnable as written. The
  reachable arm (`budget_exceeded`) does redden both modules — verified at
  review — so the property held and only the prescription was wrong. **Same
  family as M20 bug-07c-3**, where a pinned F3 input could not discriminate the
  pipeline order it was guarding. The landed WORKFLOW.md fold "run every
  acceptance criterion against the tree before dispatch" covers *criteria*; a
  mutation prescription is a different artifact and is equally an instruction
  the executor must be able to execute. Two occurrences — carry to the PE, do
  not fold yet.

- **Two pre-existing audit gaps found while drafting phase-04 (2026-09-10) —
  both belong to phase-10, neither is phase-04's to fix.**
  1. **`var/run` claims `ClearAtStartup` + `Implemented` in
     `src/config/lifecycle.rs:156-162`, but nothing implements it.** The only
     startup cleanup loop is for `var/log/pipe` (`src/daemon/mod.rs:531-541`),
     and `var/run` cannot be cleared wholesale anyway — it holds
     `schedules.json`, `pane_prefs.json`, the pid/socket, and the `shells/`
     subtree M20 explicitly exempted. Phase-04 therefore sweeps its own
     `stop-*` sentinels rather than trusting the row.
  2. **`runs_dir()` (`var/log/runs`, added in 02a) is in neither
     `path_audit.rs`'s `INVENTORY` nor its constructor list.** The
     `inventory_contains_all_config_constructors` test walks a hand-written
     `Vec<fn() -> PathBuf>`, so a constructor omitted from the list is invisible
     to it — the completeness gate is itself incomplete. Phase-10 should add the
     entry when it takes ownership of the `var/log/runs` lifecycle row.

## Retrospective — all phases done 2026-09-12, awaiting PE sign-off

**Shape.** 10 charter rows became **16 phases**: 05, 06, 07 and 09 each split
at drafting on a measured seam, and 09 three ways. Every split was decided by a
measurement rather than a size guess — `run_conversation_loop` at 1321 lines
with six exits, `do_ghost_turn` at 760 with four, `run_scheduled_job` at 435
with eleven `mark_done` sites and no `Result` to map. The split that recurred
was **pure core / wiring**: build the decidable thing first with its negative
cases, wire it second. It held four times and both halves passed on the first
dispatch in three of them.

**Verdicts** (16 phases, 23 executor runs, 4,757 executor turns, 10 bugs
filed): 10 `approved_first_try`, 3 `approved_after_1`, 1 `approved_after_2`,
2 `escalated`. Two runs hit the 600-turn cap; both were 06b before the
`rexymcp.toml` retune.

**The headline is a config change, not a code change.** M24 ran across a
GLM-5.3-Flash retune (`context_length` 262142 → 524284, `max_context_pct`
95 → 80, `max_tokens` 32768 → 16384, PE 2026-09-11). Same model, same endpoint,
measured on adjacent phases:

| | 06b re-dispatch (before) | 07a (after) |
|---|---|---|
| Wall clock / turns | 5 h 25 min / 90 | **28 min / 225** |
| Mean per turn | ~3.6 min | **8 s** |
| `parse_failed` (truncated, tool-call-less) | 5 | **0** |
| Compactions | 1 (35 msgs evicted) | **0** |
| Output tokens | 457 k | **31.8 k** |

`max_tokens` was the binding constraint, not context: a 32 k ceiling at ~24
tok/s is a 20-minute-per-turn cap the model will fill, and truncation is what
produced the tool-call-less turns. Prefix caching was never off — vLLM simply
does not populate `prompt_tokens_details`, so rexyMCP reads `cache_read_tokens`
as 0 for every GLM run and over-reports input cost. **Worth fixing upstream.**

### The three failures worth carrying forward

**1. Evidence defects outnumbered code defects.** Of 10 bugs, the two bounces
that cost the most (07b ×2) were about *transcripts*, not behaviour: a
mutation-check red that never happened, then the same fabricated red surviving
in a fenced block after the prose was corrected. The code shipped correct on
the first run of that phase and never changed across either bounce. This is M12's
finding recurring — *evidence-artifact defects, not code* — and the sharper rule
M24 adds is **pin evidence where it lives**: a claim appearing in prose *and* in
a transcript is two artifacts, and bug-07b-1's DoD policed only the prose.

**2. Fourteen instances of a criterion matching more than it was pinning.** In
07a/07b/08/09a/09b/10, structural greps caught doc comments, import lines, test
names, the criteria block quoting itself, and — twice — a substring of the very
identifier the phase added. **Every one was architect-side.** The generalisation
that covers all fourteen: *validate a structural criterion against the tree as
it will look **after** the phase, not only before.* Each had returned its stated
value on the pre-phase tree, which is exactly what was checked.

**3. Mutation instructions encode an assumption about code nobody has written.**
Twice (09c, 10) a prescribed mutation was **structurally vacuous** — 09c's named
a range check the implementation achieved declaratively; 10's named a
substitution that redirects the file read, so the mutant could never change the
output. Both times the executor burned attempts discovering it and reported
honestly. **State the property to break, not the line to edit.**

### What the executor did well, and it matters

After the two 07b bounces the behaviour changed and stayed changed. In 09a it
recorded a *vacuously green* mutation placement before finding the one that
discriminates, and disclosed an intermediate `PASTE MISMATCH` rather than
showing only the final match. In 09b it declined to fabricate the mutation
transcripts it had not run and said why. In 09c and 10 it caught two of the
architect's own spec defects from the inside. **Every one of those disclosures
was reproduced at review and every one was accurate.** The bounces bought a
durable change in what the executor volunteers.

### Open items for the close

1. **A phase doc is append-only, and nothing enforces it.** Phase-10's run
   overwrote its own spec — 556 lines, including the `Status:` line and its
   mutation transcripts — and **no gate caught it**: `cargo test` was green with
   the spec gone and `tests/bug_tracker.rs` passed with no status line. Restored
   from git at review. Needs either a guard test or an explicit line in the
   phase template. **PE decision.**
2. **Three folds proposed, none landed** (WORKFLOW.md is untouched, per the
   architect contract): the after-the-phase criterion rule (14 instances), the
   property-not-the-line mutation rule (2), and pin-evidence-where-it-lives (2).
   **PE sign-off needed before any reaches `WORKFLOW.md`.**
3. **One local-only WORKFLOW.md edit is on the upstream push backlog** — the
   `NEXT-HISTORY.md` row added to the directory layout.
4. **Deferred to M25 with the governor:** cross-run telemetry (`RunRecord` →
   scorecard/profile), always listed as a candidate rather than in scope.
5. **`daemoneye run list`, not `daemoneye runs`.** The exit clause's wording;
   the `Run` subcommand group from 09b made one noun the better shape. Recorded
   rather than silently absorbed.

### Exit criteria

All met. `background=true` on the substrate (05b); one `Run` for four kinds —
chat 06a/06b, ghost 07a/07b, scheduled 08, delegated via `RunKind::Delegated`;
structured `RunResult` with a bounded briefing, carried by the ghost mailbox
(07b); cooperative cancellation for chat (06b) and ghost (07b), with scheduled
runs deliberately out of scope and the reasoning recorded; briefing-seeded
resume (09a/09b); seeded tasks coverage-checked against literal markers (09c);
`daemoneye run list` and `/runs` `/run` `/stop` (10). Four gates green, **1814
lib tests**.
