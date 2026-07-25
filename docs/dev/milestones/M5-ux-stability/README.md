# M5 — UX & Stability

**Goal:** Make the chat TUI read as a coherent transcript and stop the daemon
from wedging: the user's own words appear in the history, the spinner stops
squatting in the input box, and no code path can take the daemon global by
blocking under a shared lock.

**Status:** planning

**Depends on:** M4 (Context Management Overhaul) — complete 2026-07-16.

**Design:** [`docs/design/daemon-stalls.md`](../../../design/daemon-stalls.md)
— the hang evidence log (mechanisms A/B/C, with the confirmed-vs-hypothesis
split) and the two TUI defect specs every phase references.

**Exit criteria:**

- [ ] The streaming spinner — animated frame, verb, and dot animation together
      — renders on a reserved one-row line above the input box's top border,
      outside it. The row is reserved (blank) when idle, so the box does not
      shift vertically when streaming starts or stops, in any of the three
      live-region draw modes (normal / spinner / prompt).
- [ ] Every prose query the user submits is committed to scrollback via the
      same `commit_panel()` element used for tool output, before the response
      streams. Scrolling back through a finished conversation shows both
      sides.
- [ ] No `SessionStore` critical section performs blocking work: no file I/O,
      no subprocess spawn, and no `.await` while the guard is held. Enforced
      by a test or lint, not only by review.
- [ ] Every tmux subprocess call made from an async context is either
      non-blocking (`tokio::process`) or off the runtime
      (`spawn_blocking`), and carries a timeout. A wedged tmux server
      degrades one operation instead of the whole daemon.
- [ ] The daemon self-reports a stall: if a shared lock is held or an IPC
      request goes unanswered beyond a threshold, `daemon.log` records what
      was holding it and where. A future wedge identifies itself without a
      live debugger.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` stays clean;
      `cargo test` green; no regression in the ~928 existing tests.

## Architecture references

- `docs/design/daemon-stalls.md` § 1 — hang mechanisms A (lock held across
  blocking work), B (blocking subprocess on tokio workers), C (SSE stall).
- `docs/design/daemon-stalls.md` § 2 — spinner gutter and user-input commit.
- `CLAUDE.md` § "Global statics in daemon" — the shared state the lock work
  touches.
- `CLAUDE.md` § "Important Invariants" — `.unwrap_or_log()` on all lock sites
  is an invariant; the lock work must preserve it.

## Phases

| #  | Phase                                                                  | Status |
|----|------------------------------------------------------------------------|--------|
| 01 | spinner-gutter ([phase-01-spinner-gutter.md](phase-01-spinner-gutter.md)) | review      |
| 02 | echo-user-input ([phase-02-echo-user-input.md](phase-02-echo-user-input.md)) | todo   |
| 03 | stall-instrumentation                                                   | todo   |
| 04 | unlock-blocking-paths                                                   | todo   |
| 05 | tmux-call-hardening                                                     | todo   |

Phases 03–05 are named but **not yet drafted** — phase 03's findings may
change the shape of 04 and 05, and the PE chose "these three, then reassess"
at kick-off. Draft each with `/rexymcp:architect next` when its predecessor
is `done`.

Ordering rationale: the two TUI phases are independent of the hang work and
of each other, so they go first and give the milestone early value while the
stall work is still being characterised. Within the hang work,
instrumentation precedes fixes so the next wedge is attributable even if the
first fix misses it — mechanisms A and B are both confirmed defects worth
removing regardless of which one fired in the observed incidents.

## Notes

**Kick-off scoping (2026-07-24).** PE reported three opening items: spinner
placement, missing user-input echo, and a daemon hang. Architect survey
confirmed all three in code before scoping. PE decisions at kick-off:

- Spinner: left gutter, aligned to the box's **top** row (chosen from four
  layout options).
- Scope: these three items, then reassess — not a full UX survey up front.

**Executor-shape note.** M4's retrospective records that this executor
reliably self-sabotages on large additive blocks and on compaction-path
rewires, but completed clean on synchronous, self-contained, model-call-free
work (phase 10a, `approved_first_try`). Phases 01 and 02 are exactly that
shape — small, synchronous, pure-render changes with a quoted worked example
available in `render_ratatui.rs`. They should dispatch well. Phase 04 (lock
critical sections) is closer to the rewire shape that has bounced; size it
small and quote the target call sites verbatim.

**Held calibration.** The M4 candidate fold (large additive blocks → executor
self-sabotage) is still **held for recurrence** per PE decision at the M4
boundary. If an M5 phase reproduces it, that is the third occurrence and the
fold lands in `WORKFLOW.md`.
