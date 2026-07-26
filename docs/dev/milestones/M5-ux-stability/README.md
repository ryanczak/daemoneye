# M5 — UX & Stability

**Goal:** Make the chat TUI read as a coherent transcript and stop the daemon
from wedging: the user's own words appear in the history, the spinner stops
squatting in the input box, and no code path can take the daemon global by
blocking under a shared lock.

**Status:** in-progress

**Depends on:** M4 (Context Management Overhaul) — complete 2026-07-16.

**Design:** [`docs/design/daemon-stalls.md`](../../../design/daemon-stalls.md)
— the hang evidence log (mechanisms A/B/C, with the confirmed-vs-hypothesis
split) and the two TUI defect specs every phase references.

**Exit criteria:**

- [x] The streaming spinner — animated frame, verb, and dot animation together
      — renders on a reserved one-row line above the input box's top border,
      outside it. The row is reserved (blank) when idle, so the box does not
      shift vertically when streaming starts or stops, in any of the three
      live-region draw modes (normal / spinner / prompt).
- [x] Every prose query the user submits is committed to scrollback via the
      same `commit_panel()` element used for tool output, before the response
      streams. Scrolling back through a finished conversation shows both
      sides.
- [ ] No `SessionStore` critical section performs blocking work: no file I/O,
      no subprocess spawn, no `.await`, **and no re-entrant re-acquisition**
      while the guard is held. Enforced by a test or lint, not only by review.
      (The re-entrancy clause was added 2026-07-25: the confirmed root cause of
      the hang was a double-lock with no blocking work between the two
      acquisitions, which no existing lint catches.)
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
| 01 | spinner-gutter ([phase-01-spinner-gutter.md](phase-01-spinner-gutter.md)) | done (approved_after_2) |
| 02 | cleanup-deadlock ([phase-02-cleanup-deadlock.md](phase-02-cleanup-deadlock.md)) | done (approved_first_try) |
| 03 | echo-user-input ([phase-03-echo-user-input.md](phase-03-echo-user-input.md)) | done (approved_first_try) |
| 04a | with-sessions-accessor ([phase-04a-with-sessions-accessor.md](phase-04a-with-sessions-accessor.md)) | done (approved_first_try) |
| 04b | convert-handlers ([phase-04b-convert-handlers.md](phase-04b-convert-handlers.md)) | done (approved_first_try) |
| 04c | convert-ask ([phase-04c-convert-ask.md](phase-04c-convert-ask.md))      | in-progress |
| 04d | convert-tail (background.rs + ghost.rs + executor + stream, ~60 sites — likely splits further) | todo |
| 04e | sessionstore-newtype (enforce; converts the 13 Arc::clone sites)        | todo   |
| 05 | unlock-blocking-paths (webhook/process.rs — mechanism A)                | todo   |
| 06 | tmux-call-hardening (mechanism B)                                       | todo   |
| 07 | stall-instrumentation (rescoped — see Notes)                            | todo   |

Phases 03–06 are named but **not yet drafted**. Draft each with
`/rexymcp:architect next` when its predecessor is `done`.

**Ordering was revised 2026-07-25** after the hang's root cause was found (see
Notes). The deadlock fix jumps to phase 02 — it is a live production defect that
takes the daemon down every hour, and it is small and fully specified.
Instrumentation, originally phase 03, drops to phase 06 and is rescoped: its
purpose was to make an unattributable wedge attributable, and this one is now
attributed. What remains for it is narrower — a watchdog for *future* wedges —
and it should only be drafted if phases 04–05 leave a real gap.

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

**Root cause found, plan revised (2026-07-25).** During the phase-01 end-to-end
check the architect hit a wedged daemon, captured its state, and the PE captured
gdb stacks. The hang is a **re-entrant acquisition of the global `SessionStore`
mutex** in the `session-cleanup` supervisor (`src/daemon/mod.rs:693` and `:709`)
— the guard from the first lock is still alive at the second, and
`std::sync::Mutex` is not reentrant. It fires ≈60 minutes after every daemon
start, deterministically, independent of load. Full evidence in
`docs/design/daemon-stalls.md` § 1.5b–1.5c.

Three consequences for this milestone:

1. The fix became **phase 02** and jumped ahead of the remaining UX work. A
   defect that takes the daemon down hourly outranks a transcript improvement.
2. **Instrumentation dropped from 03 to 06 and was rescoped.** Its original
   justification — "make the next wedge attributable" — is largely spent now
   that this wedge is attributed. Draft it only if 04–05 leave a real gap.
3. The "no blocking work under the lock" exit criterion was **widened to include
   re-entrancy**. The actual bug had *no* blocking work between the two
   acquisitions; a criterion phrased only around I/O and subprocesses would have
   declared this code compliant.

Worth recording as a general lesson: `clippy::await_holding_lock` gave false
comfort here. It targets guards held across suspension points, and this bug has
no `.await` between the two locks — so the lint gate was green for the entire
life of the defect.


**Lock-accessor plan (PE decision, 2026-07-25).** After two independent
re-entrant `sessions`-lock defects — one fixed during the M4 phase-08 takeover,
one root-caused as the M5 hang — the PE chose a structural answer over a third
point fix: a `with_sessions(|store| …)` accessor, ending in a newtype with a
private inner so the compiler enforces it. Survey, shape, and the four-phase
ordering are in `docs/design/daemon-stalls.md` § 3.

Two survey findings shaped the split:

- **100** `sessions.lock()` sites, and **zero** guards held across `.await`
  (confirmed via the clean `clippy -D warnings` gate, since
  `await_holding_lock` is warn-by-default). Every site is therefore
  closure-convertible without touching async control flow.
- **13** `Arc::clone(&sessions…)` sites, 9 of them in `daemon/mod.rs`. These
  break the instant a newtype is introduced, which is why **the newtype lands
  last (04d), not first** — otherwise 04a becomes an accidental 100-site sweep.

The re-entrancy assertion inside the accessor is deliberately **always on**, not
`debug_assert`: re-entrancy here is never legitimate, `supervise` restarts a
panicked task, and the deadlock it replaces took the daemon down for 12 hours.
A `debug_assert` would be compiled out of exactly the build where it matters.

**04b/04c split made while drafting (2026-07-25).** The original plan paired
`handlers.rs` and `ask.rs` in one conversion phase. Surveying them showed they
are not the same job:

- `handlers.rs` — 15 sites, all variations on
  `if let Ok(store) = sessions.lock() { … }`. Uniform and mechanical; the
  compiler verifies each one.
- `ask.rs` — 13 sites, several of the form `sessions.lock().ok()?` inside
  `.and_then(…)` closures. Wrapping those in `with_sessions` changes what `?`
  returns from, so each needs individual reasoning rather than pattern
  substitution.

Mixing a mechanical sweep with a set of judgement calls in one phase is how a
clean conversion turns into a bounce. They are now 04b and 04c.

The tail (04d) is ~60 sites across `background.rs`, `ghost.rs`,
`executor/mod.rs`, and `stream.rs`. That is very likely too large for one phase
and will be split when it is drafted — deliberately not pre-planned now, since
04b and 04c will show how fast this executor gets through mechanical conversion
and that is the number worth sizing against.