# M19 — Sandbox Completion

**Goal:** Finish what M18 started — scripts reach sandboxed commands, ghost
shells actually run in containers, an operator can see and steer sandbox state
from the chat surface, and a command that genuinely needs the host has one
explicit, audited way out.

**Status:** planning — **proposed, awaiting PE sign-off. Not open.**

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
  containers — verified live, not only in tests.
- `is_ghost` is derived by a **pure, directly tested predicate**: mutating it
  to a constant fails a named test. (Today hardcoding `is_ghost: true` leaves
  all 1454 tests green.)
- `daemoneye status` reports sandbox state — runtime reachable, image id vs
  lockfile, live sandboxed containers — and `Request::ContainerStatus` carries
  it over IPC.
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
| 01 | is-ghost-predicate | todo (not drafted) |
| 02 | staging-integration | todo (not drafted) |
| 03 | ghost-container-execution | todo (not drafted) |
| 04 | ghost-scoped-teardown | todo (not drafted) |
| 05 | container-status-ipc | todo (not drafted) |
| 06 | escape-hatch | todo (not drafted) |
| 07 | live-verification-and-close | todo (not drafted) |

**Ordering.** 01 is first and deliberately small: it closes a *known* coverage
gap before 03/04 start depending on the value it produces. 02 is independent
of 01. 03 depends on 01; 04 depends on 03. 05 and 06 are independent of
everything else. 07 is the close-out.

Phase intents:

- **01 is-ghost-predicate** — extract `is_ghost_session()` as a pure function
  with its own tests, and call it from `src/daemon/background/run.rs:187`.
  Small, mechanical, and it converts an untested expression into a mutable
  seam **before** teardown starts trusting it.
- **02 staging-integration** — give `stage_args` a caller so a sandboxed
  command can run a script, then **remove** the module `#[allow(dead_code)]`.
  Removal is the phase's real acceptance test.
- **03 ghost-container-execution** — route ghost background commands through
  the sandbox. Today ghosts are *labelled*, not sandboxed.
- **04 ghost-scoped-teardown** — reclaim one ghost's containers on exit, using
  `de.ghost=1` plus a per-job label. Must not touch another ghost's or an
  interactive session's containers; the negative case is the point.
- **05 container-status-ipc** — `Request::ContainerStatus` +
  `daemoneye status` output.
- **06 escape-hatch** — `GhostPolicy.escape_allowlist`, the `escape_hatch`
  flag on `ToolCallPrompt`, park-and-notify, and an `events.jsonl` record.
  The highest-risk phase in the milestone: it is the one feature whose bug
  runs a command on the host.
- **07 live-verification-and-close** — the two unrun M18 live checks plus this
  milestone's own, then the doc sweep and retrospective.

## Notes

- **The egress proxy is deliberately NOT in this milestone.** M18 measured
  that a host-bound proxy is unreachable under `--disable-host-loopback`, so
  the design's answer is a *containerized* proxy — which is a network
  architecture piece, not a completion task. Scoping it here would repeat
  M18's own mistake of carrying an unbounded item to the end. It gets its own
  milestone or an explicit PE decision to fold it in.

- **`container:shell` (interactive tty relay) stays deferred** — still an open
  question in the design doc, and nothing in M18 settled it.

- **Method carried forward from M18, because it worked:** stand the thing up
  and *run* it before writing the phase that specifies it. Four defects that
  every green test missed were found that way, three of them in code that
  passed all four gates. Budget drafting time for a live pass per phase.

- **Executor model:** `deepseek-v4-flash-0731` via rexyMCP (`brain:8888`).
  M18 record: 6 first-try, 4 approved_after_1, none escalated. Two calibration
  items are **held at 2 occurrences each** — proceeding past its own filed
  blocker, and misdescribing its own bookkeeping in a way a reader cannot
  detect without `git show`. **A third of the latter should change how this
  model is dispatched, not just how it is reviewed.** Standing hazard from
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

- **One upstream-only section we do not have:** § "The E2E block: runnable,
  complete, and seeded as a Spec task". This is M14's calibration item 3
  (seeded FAIL→blocker task), which we **held at 2 occurrences** and upstream
  has since adopted. Recommend pulling it before phase-01 — M18's mutation
  work would have been cleaner with it, and it is already battle-tested
  elsewhere. **PE decision.**
