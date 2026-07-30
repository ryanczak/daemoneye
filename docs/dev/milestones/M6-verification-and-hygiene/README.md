# M6 — Verification & Hygiene

**Goal:** Make DaemonEye able to prove its own pipelines work, keep the agent's
documented beliefs matching the code, and stop the runtime tree growing without
bound — so the next "is this broken?" question is answered by running something
rather than by reading source.

**Status:** planning

**Depends on:** M5 (UX & Stability) — closed 2026-07-30.

**Scoped:** 2026-07-30, PE decision, after a live webhook→ghost-shell test produced
three reported symptoms of which **two were measurement errors and one was a real
defect**. That ratio is the milestone's thesis: the system was partly working and
the tooling could not tell. See Notes § "Defect inventory".

**Exit criteria:**

- [ ] A **test-isolation harness** exists that runs an end-to-end scenario against
      a throwaway `HOME` **and** a private tmux server (`tmux -L`), touching neither
      the operator's `~/.daemoneye/` nor their default tmux server. Demonstrated by
      running a scenario with a daemon already live on the default server and
      showing that server's hooks and session list unchanged afterwards.
- [ ] The **webhook→ghost-shell pipeline is verified end-to-end** by a scenario in
      that harness: a generic payload with **no severity field** reaches a ghost
      shell, and the run is observable in `events/events-<date>.jsonl` as a
      `webhook_alert` followed by a `ghost_*` lifecycle event.
- [ ] **No alert is dropped silently.** Every gate on the webhook path that
      discards an alert logs at WARN naming the alert and the reason, and emits an
      event. Verified by a test that drives a below-threshold alert and asserts on
      the emitted record — not merely that nothing crashed.
- [ ] **`daemoneye audit-prompts`** (name provisional) reports every filesystem
      path asserted by the installed prompt and knowledge memories, marking each
      *resolves* / *does not resolve* against the `config` module, and exits
      non-zero when any does not. **It never rewrites the user's files** — local
      prompt edits are the operator's, and the command's contract is to report
      only.
- [ ] A test in the repo fails if any path literal in `assets/prompts/sre.toml` or
      `assets/memory/knowledge/*.md` does not correspond to a path the `config`
      module constructs. Stale prompt facts become a red gate, not a discovery.
- [ ] **Every artifact class under `~/.daemoneye/` has a stated lifecycle** —
      rotate, delete, archive, or keep-forever-by-design — recorded in one place, with
      the default value for each. No class is unmanaged by omission. Verified by a
      test that enumerates the artifact directories and fails on any not covered by
      the policy table.
- [ ] **`daemon.log` is bounded** by that policy, and the bound is exercised by a
      test rather than asserted in prose.
- [ ] **A sweep that is off by default says so where the operator will see it.**
      `sessions.archive_retention_days` defaults to `0` (keep forever) while
      `events.retention_days` defaults to `90`; today nothing surfaces that
      asymmetry.
- [ ] **Pane-preference persistence is re-specified against stable identity**, or
      deliberately reduced in scope. The exit condition is a recorded decision plus
      its implementation — not a specific mechanism, which is the phase's to choose.
      Whatever lands must not silently target a pane the user did not pick, and stale
      entries must not accumulate indefinitely.
- [ ] **`~/.daemoneye/` contains no orphaned or undocumented entries.** Every path
      present in a fresh install and in the maintainer's live tree is either
      produced by a named `config::` function or deliberately removed; `lib/` is
      either populated or dropped from both the tree and the docs.
- [ ] `docs/architecture.md` § 5 names the shipped milestones through M6 and no
      longer points at a superseded "active milestone".
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` clean; `cargo test`
      green; no regression against the 947 lib + 27 integration baseline M5 closed
      at.

## Architecture references

- `docs/architecture.md` § 2 "Major data flows" — the webhook and ghost-shell paths
  this milestone verifies.
- `docs/architecture.md` § 3 "The Ghost Shell subsystem" — what a triggered ghost
  is supposed to do once the gate lets it through.
- `docs/architecture.md` § 5 — the roadmap entry this milestone must correct
  (it still names M4 as active).
- `CLAUDE.md` § "Ghost Shell conventions" — the detection signal
  (`GHOST_TRIGGER: YES/NO`), `evaluate_watchdog_response`, and the concurrency cap,
  all downstream of the gate and therefore currently unexercised in practice.
- `docs/dev/TODO.md` § 1 — the pre-dispatch criteria-check item. M6's
  path-audit test is the same instinct applied where it is cheaper to build; worth
  reading before designing it.

## Phases

Named, **not drafted**. Draft each with `/rexymcp:architect next` when its
predecessor is `done`. Ordering is deliberate — see Notes § "Why this order".

| #  | Phase | Status |
|----|-------|--------|
| 01 | [test-isolation-harness](phase-01-test-isolation-harness.md) — throwaway `HOME` + private tmux server; one scenario proving the operator's live server is untouched | done |
| 02 | prompt-path-audit-test — repo-side test that fails on any unresolvable path literal in the prompt and knowledge-memory assets | todo |
| 03 | fix-stale-prompt-paths — correct the three known stale `events.jsonl` references; whatever else 02 surfaces | todo |
| 04 | audit-prompts-command — the operator-facing `daemoneye audit-prompts`, report-only, never rewriting | todo |
| 05 | severity-gate-honesty — an absent severity is not the lowest severity; every discard logs and emits | todo |
| 06 | webhook-to-ghost-e2e — the pipeline scenario in the 01 harness, severity-less payload through to a `ghost_*` event | todo |
| 07 | artifact-lifecycle-policy — **design-discovery**: one policy table covering every artifact class, with defaults; the test that fails on an uncovered class | todo |
| 08 | daemon-log-rotation — bound the 25.8 MB unrotated log under the 07 policy | todo |
| 09 | pane-and-archive-retention — `panes/` (264 files, unswept) and the off-by-default `archive_retention_days` | todo |
| 10 | pane-prefs-redesign — **design-discovery**: stable identity for the session→pane mapping, or a deliberate scope reduction | todo |
| 11 | runtime-tree-hygiene — orphan removal, the `lib/` decision, doc-comment corrections | todo |
| 12 | roadmap-correction — `docs/architecture.md` § 5 through M6 | todo |

Phases beyond 06 may be re-split or dropped once 01–06 land; the inventory below is
what is *known*, and 01/06 will very likely add to it.

## Notes

### Defect inventory (2026-07-30 survey)

Everything here was verified against the tree or the live runtime during scoping.
Nothing is inferred.

**Pipeline correctness**

1. **An alert with no severity is silently discarded.** `severity_rank`
   (`src/webhook/process.rs:14`) maps anything unrecognised — including `""` — to
   `0`. With the shipped default `severity_threshold = "warning"` (rank 2), the gate
   `if alert_rank >= threshold_rank || threshold_rank == 0` is false, so
   `fire_notification` **and** `maybe_analyze_alert` are both skipped. **No log line
   and no event records the drop.** Consequence: a generic webhook that supplies no
   severity label can never trigger a ghost shell under the default config, and the
   operator sees `Webhook alert: '…' [firing]` in `daemon.log` and reasonably
   concludes it was processed.
   **Not an M5 regression** — both `severity_rank` and the gate were last touched in
   `3fde6cd` (2026-03-07), four months before M5 opened.
2. **Everything downstream of that gate is unexercised in practice.**
   `maybe_analyze_alert`, the watchdog `GHOST_TRIGGER` parse, `check_ghost_capacity`,
   and `GhostManager::start_session` have never run for a severity-less alert. Phase
   06 may surface further defects behind the gate; the inventory cannot pre-empt them.
3. **The integration test cannot catch it.** `webhook_alert_to_event_log`
   (`tests/integration.rs:679`) sends `severity: "critical"` — rank 3, passing any
   threshold — and asserts on `log_event`, which runs *before* the gate. It would
   pass no matter what the gate did. This is a textbook case of the discipline folded
   into `WORKFLOW.md` on 2026-07-30: a test pinning a property on a path that avoids
   the branch that matters.

**Agent-belief accuracy**

4. **The prompt names a path that has not existed since July 9** —
   `assets/prompts/sre.toml:320`: ``- `var/log/events.jsonl` — structured event log
   (prefer `search_repository(kind:"events")`)``. The parenthetical is **correct**:
   `search_events_in_segments` (`src/search.rs:173`) reads the dated segments via
   `event_segment_paths_between`. So the prompt recommends the right tool and names
   the wrong path in the same sentence — and a grep-oriented agent takes the path.
   This is what produced two of the three wrong conclusions in the live test.
5. **The knowledge memory the prompt defers to repeats the error twice** —
   `assets/memory/knowledge/agent-runtime-layout.md:51` (ASCII tree) and `:79` (path
   list). The prompt explicitly sends the agent there "for the full layout", so the
   authoritative reference is the most wrong.
6. **Installed copies drift indefinitely.** `overwrite_sre_prompt()` and
   `overwrite_knowledge_memories()` (`src/config/seeds.rs:147`, `:103`) have exactly
   one caller each, both in `src/cli/commands/setup.rs`. First-run seeding is
   `if !exists`. Any install predating a change keeps the stale prompt, and nothing
   tells the operator.
   **PE constraint (2026-07-30): auto-refresh is ruled out** — it would clobber local
   prompt modifications, which belong to the operator. The remedy is an *audit* that
   reports drift, never a write.
7. **Prompt facts are never tested.** The only assertion on the prompt is
   `src/config/mod.rs:147` — `assert!(def.is_ok(), "SRE_PROMPT_TOML must be valid
   TOML")`. Syntax, never accuracy.
8. **`lib/` is documented and empty.** `agent-runtime-layout.md:30` describes it as
   "shared SDK modules (de_sdk, Python helpers)". `~/.daemoneye/lib/` has been empty
   since creation (Mar 26). Either the feature was dropped or never built; the docs
   never noticed.

**Artifact lifecycle** — *scope expanded 2026-07-30 on PE instruction: "all
daemoneye log files and other artifacts need to be managed (logs rotated, old files
deleted or archived, etc.)."*

Survey of every artifact class, measured against the maintainer's live tree. **Only
two sweep functions exist in the entire codebase**
(`sweep_event_segments`, `sweep_session_archives`), both called from one site
(`src/daemon/mod.rs:821`) on every 60th cleanup tick:

| Class | Live size | Files | Oldest | Lifecycle |
|---|---|---|---|---|
| `var/log/daemon.log` | 25.8 MB | 1 | 2026-05-08 | **none — no rotation logic anywhere** |
| `var/log/events/` | 167 KB | 10 | 2026-07-13 | swept, `events.retention_days` **default 90** |
| `var/log/sessions/` | 3.2 MB | 141 | 2026-05-08 | sweep exists but `archive_retention_days` **defaults to 0 = keep forever** |
| `var/log/panes/` | 1.9 MB | **264** | 2026-05-09 | **none** |
| `var/log/pipe/` | ~0 | 0 | — | deleted at daemon start ✓ |
| `agents/*/mailbox/` | — | 3 | — | **none** — `grep` for mailbox cleanup returns only a doc comment |
| `scripts/`, `runbooks/`, `memory/` | 1.0 MB | 88 | 2026-03-27 | user content — arguably correct to keep, but no stated policy either way |

Three distinct problems, not one:

9. **`daemon.log` is unbounded.** 25.8 MB and growing in the maintainer's tree, and
   `grep -rn 'daemon\.log' src/ | grep -iE 'rotat|truncat|size|sweep'` returns
   **nothing** — there is no rotation logic at all. By contrast the event log *is*
   bounded (`sweep_event_segments`, `events_retention_days`) and session archives are
   (`sweep_session_archives`). `panes/` (1.9 MB) and `pipe/` (487 KB) need checking.
9b. **A sweep that is off by default, with no signal.** `archive_retention_days`
    defaults to `0` (`src/config/types.rs:115`, documented in-code as "keep forever")
    while `events.retention_days` defaults to `90` (`:97`). Opposite defaults for two
    adjacent artifact classes, and nothing tells the operator which is which. 141
    session archives back to May 8 are the result.
9c. **Two classes have no sweep at all** — `panes/` (264 files) and agent mailboxes.
    Mailbox files are written per `job_id` on every ghost exit
    (`write_mailbox_on_exit`), so they accumulate one-per-ghost forever.

10. **An orphaned file the code no longer reads.**
    `~/.daemoneye/pane_prefs.json` (12 bytes, Jun 25) vs the live
    `~/.daemoneye/var/run/pane_prefs.json` (64 bytes, Jul 25). `prefs_path()`
    (`src/pane_prefs.rs:10`) correctly returns `var_run_dir().join(...)`, but the
    module's own doc comment at `:4` still says `~/.daemoneye/pane_prefs.json` —
    pointing at the dead file. Same drift class as the prompt, in a doc comment.
11. **The legacy 34 MB `var/log/events.jsonl` was deleted by the PE during
    scoping**, which is what made defect 4's cost concrete. `sweep_event_segments`
    explicitly never deletes it (`event_log.rs:226`), so it would have sat there
    indefinitely.

**Process / design docs**

12. **`docs/architecture.md` § 5 still names M4 as the active milestone.** M5 shipped
    46 phases since. The design doc has the same drift the prompt does.
13. **E2E work has no isolated environment.** Across M5 phases 08–11, every
    real-artifact verification started, stopped, `SIGSTOP`ed or `SIGKILL`ed the
    operator's daemon and repointed global tmux hooks. The architect worked around it
    with throwaway `HOME`s, but the tmux server was always the live one, and one
    scenario (09's) could not be re-run at review for that reason. A milestone of E2E
    work needs this fixed first or its results will not be trusted.

### Why this order

**01 first, because everything else needs it.** Axes 2–4 all want end-to-end
verification, and M5 demonstrated repeatedly that doing that against the live
environment is disruptive and self-limiting — a review that cannot re-run a
scenario has to take the executor's word for it, which this project otherwise
refuses to do.

**02 before 03.** Write the gate, watch it fail on the three known stale paths, then
fix them. Fixing first would leave the gate unproven — and a path-audit test that
has never failed is exactly the vacuous-coverage shape folded into `WORKFLOW.md`
this week. The failing run *is* the mutation proof.

**04 after 02/03**, because the operator-facing command should reuse whatever
path-extraction the test establishes rather than inventing a second one.

**05 before 06.** The gate fix is small and fully understood; the E2E scenario is
the thing that proves it *and* opens the unexplored ground behind it. Doing 06 first
would mean writing a scenario whose expected outcome is a known bug.

### Sizing and expectations

Axes 2 and 4 are mechanical and well-specified — the shape this executor handles
well. **01 and 06 are design-discovery**, and `WORKFLOW.md` § "Front-load by task
shape" says to pre-inject the load-bearing constraint for those: for 01 that is how
a private tmux server is addressed by every `Command::new("tmux")` in the tree
(there is no `-L` plumbing today — that is the design question, not a detail), and
for 06 it is what a passing scenario asserts on when the ghost's own behaviour
depends on a live AI call.

**01's constraint was resolved at drafting (2026-07-30) and is no longer open.**
tmux takes its server socket from `$TMUX_TMPDIR` and `std::process::Command`
children inherit the environment, so setting it on the spawned daemon gives all
**82** call sites a private server with zero `src/` changes. Verified live; the
probe transcript is quoted in the phase doc, and plumbing `-L` is now an explicit
scope violation rather than a design option. 06 remains genuinely open.

**Defect 13 was then observed live rather than inferred.** During drafting the
operator's tmux server exited repeatedly, taking the daemon with it — stale
`daemoneye.pid` and `daemoneye.sock` on disk, no process, no tmux server. The
four server-wide `-g` hooks (`src/daemon/mod.rs:563`–`:620`) keep firing
`daemoneye notify …` at a socket that may be gone. Phase 01 contains exactly this.

**Expect the inventory to grow.** Thirteen items came from one afternoon aimed at a
single webhook. Phases 01 and 06 are the ones most likely to add to it, and the
milestone should be re-scoped rather than stretched if they do.

### Calibration carried in from M5

Two folds landed on 2026-07-30 and both bear directly on this milestone:

- **Observable-property discipline** (`WORKFLOW.md`, folded): when a spec pins a
  property, confirm it is observable; when it names a branch, describe a sequence
  that reaches it. Defect 3 above is precisely the failure it guards.
- **The pre-dispatch criteria check** (`docs/dev/TODO.md` § 1, parked as tooling):
  eight defective acceptance criteria in M5, three of which cost a run. M6's phase
  02 is a narrower instance of the same idea — turn a discipline into a gate — and
  is worth building partly as a cheap experiment in whether that generalises.

### Pane preferences — the use case needs revisiting, not just the orphan

*Scope addition 2026-07-30 on PE instruction: "there may be better ways of
remembering pane preferences."* This is why phase 10 is design-discovery rather than
a bug fix.

The stated goal (`src/pane_prefs.rs:1-5`) is "the user is never asked to pick a pane
more than once per session", persisted across daemon restarts. The implementation is
a flat `HashMap<session_name, pane_id>`. **Both sides of that mapping are unstable
identities**, which is the core problem.

The maintainer's live file, verbatim:

```json
{"2":"%5","0":"%0","1":"%1","daemoneye":"%0","de-phase01":"%20"}
```

- **Session names are not identities.** `"0"`, `"1"`, `"2"` are tmux's default
  numeric names. They are reused constantly and refer to nothing in particular. A
  preference stored for session `"0"` will be offered for *any* future session that
  happens to be named `"0"`.
- **Pane IDs are not identities either.** `%5`, `%20`, `%22` are per-server and
  recycled across tmux server restarts. `resolve_target_pane` does validate with
  `pane_exists` (`src/cli/commands/pane.rs:19`), which is better than nothing — but
  it proves only that *a* pane holds that ID, not that it is the pane the user chose.
  After a server restart `%0` almost certainly exists and is something else.
- **Net effect: the preference is reliable only within one continuous tmux server
  lifetime** — precisely the case where persistence is least needed.
- **Nothing prunes.** `de-phase01` is a long-dead rexyMCP phase session still in the
  file. Entries accumulate for every session name ever seen.
- **`pub fn get()` is implemented as `all.remove(session_name)`**
  (`src/pane_prefs.rs:35`). Non-destructive today only because the mutated map is
  never written back — one refactor away from silently deleting preferences.

Directions worth weighing in the phase (none chosen — this is the design question):
store a *fingerprint* alongside the pane ID (window name, pane index, cwd) and accept
the preference only if it still matches; key on window/pane *index* rather than ID;
scope the memory to a tmux-server generation and discard on change; or reduce the
requirement — "ask once per daemon run" may be enough and is far simpler than
persistence that is wrong across restarts.

**Constraint carried in:** whatever lands must not silently execute a foreground
command in a pane the user did not pick. That is the failure this feature can
actually cause, and it is worse than asking again.

### Why the artifact work is one design phase before three mechanical ones

Phase 07 is deliberately a policy phase with no rotation code in it. The survey above
found the classes are inconsistent in *kind*, not just in coverage: one is swept with
a sane default, one is swept with an off default, two are unswept, one is cleared at
startup, and three hold user content that probably *should* persist. Writing rotation
for `daemon.log` first would produce a fourth independent convention.

So 07 states the policy and lands the test that fails on any class not covered by it;
08–09 then implement against a decided table. The test is the durable part — it is
what stops the next artifact class from being unmanaged by omission, which is how all
four of the current gaps arose.
