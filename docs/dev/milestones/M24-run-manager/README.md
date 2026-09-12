# M24 — Run Manager

**Goal:** Collapse the four execution loops — a chat turn, a ghost, a scheduled
job, a delegated agent — into one `Run` model with a per-run turn log,
structured `RunResult` + briefing, cooperative cancellation, and briefing-seeded
resume; and make **`run_terminal_command(background=true)` fall out of it** as an
un-awaited chat Run against a dedicated substrate shell, replacing the tmux
`de-bg-*` path.

**Status:** in-progress — scoped 2026-09-09, **PE sign-off 2026-09-09**
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
| 08 | scheduled-run ([phase-08-scheduled-run.md](phase-08-scheduled-run.md)) — `RunKind::Scheduled` gets its first production constructor; wrap the 435-line body, derive the terminal state from the store | review      |
| 09 | resume — briefing-seeded `daemoneye run resume` + seeded tasks from runbook `## Steps` | todo (not drafted) |
| 10 | cli — `/runs`, `/run`, `/stop`; cross-run telemetry (`RunRecord`) if not deferred | todo (not drafted) |

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
