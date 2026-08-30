# M19 — Sandbox Completion

**Goal:** Finish what M18 started — scripts reach sandboxed commands, ghost
shells actually run in containers, a sandboxed agent gets audited network
egress through a proxy it cannot bypass, an operator can see and steer sandbox
state from the chat surface, and a command that genuinely needs the host has
one explicit, audited way out.

**Status:** in-progress — **opened 2026-08-29 on PE sign-off.**

**Depends on:** M18 — Container-sandboxed Agents (closed 2026-08-29, 10 phases
done).

**Exit criteria:**

- A sandboxed background command can execute a script from
  `~/.daemoneye/scripts/` — `stage_args` has a caller, and
  `src/daemon/executor/mod.rs`'s module-level `#[allow(dead_code)]` is
  **removed** with `cargo clippy --all-targets --all-features -- -D warnings`
  still green (repo-wide count 7 → 6).
- A ghost shell's background command runs in a container carrying
  `de.ghost=1`, and a ghost-scoped teardown reclaims only that ghost's
  containers — verified live, not only in tests. **With the sandbox enabled a
  ghost has no unsandboxed door out**: neither `background=false` nor
  `retry_in_pane` reaches the host uncontained (phase-03).
- `is_ghost` is derived by a **pure, directly tested predicate**: mutating it
  to a constant fails a named test. (Today hardcoding `is_ghost: true` leaves
  all 1454 tests green.)
- `daemoneye status` reports sandbox state — runtime reachable, image id vs
  lockfile, live sandboxed containers — and `Request::ContainerStatus` carries
  it over IPC.
- A profile declaring `network = "proxy"` runs the agent container attached
  **only** to a dedicated user-defined network carrying a proxy container;
  the agent reaches an allowlisted host through it, a non-allowlisted host is
  refused, and every request is recorded in `events.jsonl`. The negative case
  is load-bearing: with the proxy in place the container must still reach
  **neither** the host loopback nor the wider LAN — `--disable-host-loopback`
  stays on.
- A command the operator explicitly escalates runs on the host with the
  escape recorded in `events.jsonl`; one that is not escalated cannot.
- **Live checks, architect-run at close** (the M14/M18 convention — through
  the user's door, session JSONL as the evidence anchor): the startup sweep
  runs through a real daemon; an AI-driven background command completes in a
  container from a real `daemoneye chat` turn.
- All four gates green: `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.

## Architecture references

- `docs/design/agent-container-sandboxing.md` — the design of record. § D0 is
  the tool disposition table; § D4 (staging) and § D5 (egress) were both
  **corrected from measurement** during M18 and are the versions to build to.
- `CLAUDE.md` § "Container sandbox" — what M18 actually shipped.
- `docs/dev/milestones/M18-container-sandboxing/README.md` § Retrospective —
  the carries, and the four defects that live measurement found.

## Phases

Proposed decomposition; each drafted on demand via `/rexymcp:architect next`.

| #  | Phase | Status |
|----|-------|--------|
| 01 | is-ghost-predicate ([phase-01-is-ghost-predicate.md](phase-01-is-ghost-predicate.md)) | **done** (approved_first_try, 2026-08-29) |
| 02 | staging-integration ([phase-02-staging-integration.md](phase-02-staging-integration.md)) | **done** (approved_after_1, 2026-08-29) |
| 03 | ghost-container-execution ([phase-03-ghost-container-execution.md](phase-03-ghost-container-execution.md)) | **done** (approved_first_try, 2026-08-29) |
| 04 | ghost-scoped-teardown ([phase-04-ghost-scoped-teardown.md](phase-04-ghost-scoped-teardown.md)) | in-progress (bounced, bug-phase-04-1) |
| 05 | container-status-ipc | todo (not drafted) |
| 06 | proxy-network-and-image | todo (not drafted) |
| 07 | proxy-profile-wiring | todo (not drafted) |
| 08 | proxy-allowlist-and-audit | todo (not drafted) |
| 09 | escape-hatch | todo (not drafted) |
| 10 | live-verification-and-close | todo (not drafted) |

**Ordering.** 01 is first and deliberately small: it closes a *known* coverage
gap before 03/04 start depending on the value it produces. 02 is independent
of 01. 03 depends on 01; 04 depends on 03. 05 is independent of everything
else. 06 → 07 → 08 is a hard chain (no wiring without a network; no allowlist
without wiring). 09 is independent but scheduled late deliberately. 10 is the
close-out.

Phase intents:

- **01 is-ghost-predicate** — extract `is_ghost_session()` as a pure function
  with its own tests, and call it from `src/daemon/background/run.rs:187`.
  Small, mechanical, and it converts an untested expression into a mutable
  seam **before** teardown starts trusting it.
- **02 staging-integration** — give `stage_args` a caller so a sandboxed
  command can run a script, then **remove** the module `#[allow(dead_code)]`.
  Removal is the phase's real acceptance test.
- **03 ghost-container-execution** — close the two paths by which a ghost
  reaches the host with no container: `background=false` (foreground execution
  ignores `is_ghost` entirely) and `retry_in_pane` (`respawn.rs` has **zero**
  sandbox code). **Corrected from measurement at drafting, 2026-08-29:** the
  original intent line here said *"today ghosts are labelled, not sandboxed"* —
  false. A ghost's ordinary background command *is* containerized;
  `run_background_in_window` wraps every enabled-sandbox command and phase-01
  already routes the `de.ghost=1` decision through `resolve_is_ghost`. The two
  bypasses above are the actual hole.
- **04 ghost-scoped-teardown** — reclaim one ghost's containers on exit, using
  `de.ghost=1` plus a new `de.session=<session_id>` label, and wire
  `[sandbox.ghost_defaults] destroy_on_exit`. Must not touch another ghost's or
  an interactive session's containers; the negative case is the point.
  **Refined from measurement at drafting (2026-08-29):** the intended selector
  was "a per-job label" — a *session* label is the right grain, because the
  hook fires once when the ghost exits, not once per job. Live-measured on the
  daemon host: `--filter label=k=v` is an **exact** value match (a sibling
  ghost sharing a name prefix does not match), repeated `--filter` clauses are
  ANDed, and `rm -f` reclaims a still-running container.
- **05 container-status-ipc** — `Request::ContainerStatus` +
  `daemoneye status` output.
- **06 proxy-network-and-image** — a dedicated user-defined docker network
  and a proxy container image, plus the argv builders that create and tear
  them down. Pure-decision-logic-first, exactly as `container.rs` is built.
- **07 proxy-profile-wiring** — `network = "none" | "proxy"` in the sandbox
  profile; when `proxy`, attach the agent container to the proxy network
  **only** and set `HTTP(S)_PROXY` to the proxy's service name.
- **08 proxy-allowlist-and-audit** — per-profile hostname allowlist enforced
  in the proxy, every request logged to `events.jsonl`. A refused host must
  be observably refused, not silently dropped.
- **09 escape-hatch** — `GhostPolicy.escape_allowlist`, the `escape_hatch`
  flag on `ToolCallPrompt`, park-and-notify, and an `events.jsonl` record.
  The highest-risk phase in the milestone: it is the one feature whose bug
  runs a command on the host.
- **10 live-verification-and-close** — the two unrun M18 live checks plus this
  milestone's own, then the doc sweep and retrospective.

## Notes

- **PE DECISION 2026-08-29 — the egress proxy IS in scope.** I had scoped it
  out, arguing a containerized proxy is a network-architecture piece rather
  than a completion task; the PE overruled that and it is in, as phases 06–08.
  Recorded because the reasoning that argued against it is still the risk to
  manage: it is the largest unbuilt piece here, so it gets **three** phases
  rather than one, and the chain is ordered so each has a runnable end state.
  Design of record is § D5, whose mechanism was already corrected from
  measurement — a host-bound proxy is unreachable under
  `--disable-host-loopback`, and disabling that flag to reach one would expose
  every host loopback service to every container.

- **Gap found drafting phase-02 (2026-08-29): scheduled script jobs are not
  sandboxed.** `ActionOn::Script` runs the script's host path in a `de-sj-*`
  host shell (`src/daemon/scheduled.rs`) and never enters
  `run_background_in_window`, so neither the sandbox wrap nor the staging
  phase-02 adds reaches it. D0 says *"scheduled commands execute sandboxed
  when they fire"*; today they do not. Not in any phase's scope — the design
  claim needs correcting in the phase-10 doc sweep, or a phase adding.

- **Gap found drafting phase-03 (2026-08-29): `[sandbox.ghost_defaults]` is
  parsed and consulted by nothing.** `grep -rn ghost_defaults src/` returns
  only `src/config/mod.rs` tests. `destroy_on_exit` is phase-04's business;
  **`mount_scripts` has no phase** — staging always mounts `/de/scripts:ro`,
  so a config that sets `mount_scripts = "rw"` is silently ignored. Either
  wire it or delete the field in the phase-10 sweep; a config key that does
  nothing is worse than no key.

- **`container:shell` (interactive tty relay) stays deferred** — still an open
  question in the design doc, and nothing in M18 settled it.

- **Method carried forward from M18, because it worked:** stand the thing up
  and *run* it before writing the phase that specifies it. Four defects that
  every green test missed were found that way, three of them in code that
  passed all four gates. Budget drafting time for a live pass per phase.

- **Executor model:** `deepseek-v4-flash-0731` via rexyMCP (`brain:8888`).
  M18 record: 6 first-try, 4 approved_after_1, none escalated. Two calibration
  items were **held at 2 occurrences each** — proceeding past its own filed
  blocker, and misdescribing its own bookkeeping in a way a reader cannot
  detect without `git show`.
  **The third of the latter arrived at phase-03 (2026-08-29)** — the
  completion summary claimed the M1 mutation failed exactly one test while
  the artifact it had just pasted showed two — so, per the note that
  anticipated it, the change is now on the **dispatch** side:
  **every phase doc dispatched to this model gets a mechanically checkable
  criterion for each claim its summary will make.** A count the reviewer can
  `grep` cannot be misdescribed; prose about a count can, and twice now has.
  Concretely: pin the *set* of expected failing test names, not a count of
  them; and where a summary claim has no grep, do not ask for the claim.
  A second, narrower fold from the same phase: the status flip is the one
  edit this model makes that **no** `grep -c` guards — it mis-anchored and
  ate the `**Milestone:**` line (bug-phase-03-1). Reviewers check the
  phase-doc header (one `**Status:**`, one `**Milestone:**`) as part of the
  DoD walk. Standing hazard from
  M16: this model has acted destructively when a gate kept bouncing, so the
  phase doc's criteria and the gate must agree *before* dispatch.

- **Template drift, checked at milestone start 2026-08-29 — and the recorded
  method is wrong.** The practice on file is to `comm` the `^#{2,3} ` headings
  both ways. Done: 5 local-only headings, 1 upstream-only. But local
  `WORKFLOW.md` is **1802 lines against upstream's 1259** — 543 lines the
  heading diff cannot see, because most folds (including both landed at M18
  close) are *paragraphs inside existing sections*. Probing by content:
  `PIPESTATUS` 2/0, `validate every mechanical criterion` 1/0, and both M18
  folds 1/0. **Heading `comm` badly understates drift; probe by content for
  anything folded into an existing section.**

- **The upstream-only section was approved for pulling, and on inspection
  there is nothing to pull — my claim that we lacked it was wrong.**
  § "The E2E block: runnable, complete, and seeded as a Spec task" was folded
  *upstream from DaemonEye* on 2026-08-09; local `WORKFLOW.md` carries all
  three of its rules in **fuller** form, inside § "End-to-end verification"
  rather than under that heading — the runnable-block rule, the mutation-pairs
  -as-`## Spec`-tasks rule with its `patch` worked example, and the
  seeded-capture-task rule naming `executor/src/agent/tasks.rs`. Local also
  carries three things upstream does not: the `${PIPESTATUS[0]}` requirement,
  the no-unpastable-bytes clause, and the last-entry-anchored PASTE MATCH
  recipe. Pasting the upstream copy would have inserted a weaker duplicate of
  guidance already here.
  **How I got it wrong is the useful part:** I concluded "we do not have it"
  from a *heading* diff, in the same breath as documenting that heading diffs
  cannot see paragraph-level folds. My content probe then compounded it —
  `grep -ci "Prove it applied"` returned 0 because local words that rule as
  "a `grep -c` of the mutated text after each direction". A blind instrument
  reporting clean is § "Run every count criterion"'s own second corollary.
