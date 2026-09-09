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
| 02b | run-log-tools ([phase-02b-run-log-tools.md](phase-02b-run-log-tools.md)) — `run_log_tail` / `run_log_search` / `get_turn` as a deferred `runs` group; clamped limits + `get_turn` as the uncapped escape hatch | **in-progress** (started 2026-09-09) |
| 03 | run-result — `RunResult` + bounded briefing (port from rexyMCP anchors) | todo (not drafted) |
| 04 | cancel — cooperative cancel + `var/run/stop-<run>` sentinel + `daemoneye stop <run>` | todo (not drafted) |
| 05 | background — `run_terminal_command(background=true)` as an un-awaited chat Run on a dedicated shell + marker-watcher completion → `[Background Task Completed]` **(the payoff)** | todo (not drafted) |
| 06 | chat-run — wire chat turns onto `Run` | todo (not drafted) |
| 07 | ghost-run — `trigger_ghost_turn` onto `Run`; mailbox carries `RunResult` | todo (not drafted) |
| 08 | scheduled-run — scheduled jobs onto `Run` | todo (not drafted) |
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
