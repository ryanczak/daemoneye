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
[`docs/design/daemon-instance.md`](../../../design/daemon-instance.md) — the
process-lifecycle axis (instance ownership), driving phases 08–11. The two meet
in `daemon-instance.md` § 1.3: a stalled daemon is what lets a second instance
take the socket.

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
- [x] No `SessionStore` critical section performs blocking work: no file I/O,
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
- [ ] Only one daemon can run per `$HOME`, enforced by an exclusive `flock`
      acquired before any startup side effect. A second launch cannot unlink,
      overwrite, or delete anything belonging to a running daemon — including its
      socket, its pipe logs, and its global tmux hooks — whether or not the
      running daemon is answering IPC. (Added 2026-07-26; see
      `docs/design/daemon-instance.md` § 2.)
- [ ] `daemoneye ping` / `status` distinguish "not running" from "alive but not
      answering", and `daemoneye daemon` exits non-zero with the real reason when
      the forked child fails to start.
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
| 04c | convert-ask ([phase-04c-convert-ask.md](phase-04c-convert-ask.md))      | done (approved_first_try) |
| 04d | convert-executor-dispatch ([phase-04d-convert-executor-dispatch.md](phase-04d-convert-executor-dispatch.md)) — `executor/mod.rs`, 10 sites + `load_agent` hoist | done (approved_first_try) |
| 04e | convert-executor-tail ([phase-04e-convert-executor-tail.md](phase-04e-convert-executor-tail.md)) — `foreground.rs` (4) + `knowledge/{mod,pane,ghost}.rs` (4) = 8 sites | done (approved_first_try) |
| 04f | convert-context-background ([phase-04f-convert-context-background.md](phase-04f-convert-context-background.md)) — `context/background.rs`, **4 production** sites (2 found late) | done (approved_after_1) |
| 04g | convert-ghost-exit-paths ([phase-04g-convert-ghost-exit-paths.md](phase-04g-convert-ghost-exit-paths.md)) — `write_mailbox_on_exit` (3) + `briefing.rs` (1) = 4 sites | done (approved_first_try) |
| 04h | convert-ghost-turn-loop ([phase-04h-convert-ghost-turn-loop.md](phase-04h-convert-ghost-turn-loop.md)) — `start_session` (1) + `do_ghost_turn` (7) = 8 sites | done (approved_first_try) |
| 04i | convert-background-windows ([phase-04i-convert-background-windows.md](phase-04i-convert-background-windows.md)) — `run.rs` (4) + `respawn.rs` (3) = 7 mechanical sites | done (approved_first_try) |
| 04j | convert-stream-hooks ([phase-04j-convert-stream-hooks.md](phase-04j-convert-stream-hooks.md)) — `stream.rs` (8) + `hook.rs` (2) = 10 conversion sites | done (approved_first_try) |
| 05a | unlock-background-and-hook ([phase-05a-unlock-background-and-hook.md](phase-05a-unlock-background-and-hook.md)) — `helpers.rs::notify_session`, `gc.rs::gc_bg_windows`, `hook.rs:92` = 3 subprocess-under-lock restructures | done (approved_after_2) |
| 05b | unlock-webhook-and-stream ([phase-05b-unlock-webhook-and-stream.md](phase-05b-unlock-webhook-and-stream.md)) — `webhook/process.rs` (2) + `stream.rs:722` = 3 restructures | done (approved_first_try) |
| 05c | convert-stragglers-and-tests ([phase-05c-convert-stragglers-and-tests.md](phase-05c-convert-stragglers-and-tests.md)) — `ask.rs` (2, production) + `background.rs` (17, test) + `session.rs` (3, test) = 22 conversions | done (approved_after_1) |
| 05d | sessionstore-newtype ([phase-05d-sessionstore-newtype.md](phase-05d-sessionstore-newtype.md)) — alias → newtype; 16 construction sites + 16 `Arc::clone` sites; `try_lock` `#[cfg(test)]`-gated | done (approved_first_try) |
| 05e | unlock-watch-pane ([phase-05e-unlock-watch-pane.md](phase-05e-unlock-watch-pane.md)) — `pane.rs:329`: 2 file writes + a tmux spawn inside a `with_sessions` closure | done (approved_first_try) |
| 05f | unlock-ask-entry ([phase-05f-unlock-ask-entry.md](phase-05f-unlock-ask-entry.md)) — `ask.rs:97`: hoist `read_session_meta` (file read) + `pane_exists`/`start_pipe_pane` (2 subprocesses) out of the closure. **Last blocking-work site** | done (approved_first_try) |
| 05g | compaction-coverage-followup ([phase-05g-compaction-coverage-followup.md](phase-05g-compaction-coverage-followup.md)) — 04f's 3 vacuous `compaction_in_flight` assertions made real, all 4 clearing sites mutation-checked. **Adds a test: 916** | done (approved_after_1) |
| 05h | test-home-guard ([phase-05h-test-home-guard.md](phase-05h-test-home-guard.md)) — one poison-recovering accessor for `TEST_HOME_LOCK`; 62 sites. Stops one failing test failing 47 others | done (approved_first_try) |
| 06a | tmux-off-runtime ([phase-06a-tmux-off-runtime.md](phase-06a-tmux-off-runtime.md)) — the `off_runtime` adapter + `background/run.rs` (16 sites). **First of ~5** | done (approved_first_try) |
| 06b | tmux-off-runtime-respawn ([phase-06b-tmux-off-runtime-respawn.md](phase-06b-tmux-off-runtime-respawn.md)) — `respawn.rs`, 11 sites (2 `tmux::` hits are non-sites) | done (approved_first_try) |
| 06c | tmux-off-runtime-foreground ([phase-06c-tmux-off-runtime-foreground.md](phase-06c-tmux-off-runtime-foreground.md)) — `foreground.rs` **slice 1**, lines ≤460, 10 sites. Re-split after a `hard_fail` on 29-at-once | done (approved_after_1) |
| 06d | tmux-off-runtime-foreground-2 ([phase-06d-tmux-off-runtime-foreground-2.md](phase-06d-tmux-off-runtime-foreground-2.md)) — `foreground.rs` **slice 2**, poll & capture, 10 sites | done (approved_first_try) |
| 06e | tmux-off-runtime-foreground-3 ([phase-06e-tmux-off-runtime-foreground-3.md](phase-06e-tmux-off-runtime-foreground-3.md)) — `foreground.rs` **slice 3**, exit status & cleanup, 9 sites. Finishes the file | done (approved_first_try) |
| 06f–06h | tmux-off-runtime tail — `executor/knowledge/pane.rs` + `file_ops/` (17), `daemon/` core, `cli/`. Not drafted | todo |
| 07 | stall-instrumentation (rescoped — see Notes)                            | todo   |
| 08 | instance-lock ([phase-08-instance-lock.md](phase-08-instance-lock.md))  | todo   |
| 09 | fatal-bind-honest-liveness ([phase-09-fatal-bind-honest-liveness.md](phase-09-fatal-bind-honest-liveness.md)) | todo |
| 10 | lifecycle-observability ([phase-10-lifecycle-observability.md](phase-10-lifecycle-observability.md)) | todo |
| 11 | fork-readiness-handshake ([phase-11-fork-readiness-handshake.md](phase-11-fork-readiness-handshake.md)) | todo |

Phases 05b–07 are named but **not yet drafted**. Draft each with
`/rexymcp:architect next` when its predecessor is `done`.

**The 04d tail was split on 2026-07-26** — first into five phases (04d–04i),
replacing the earlier "04d×3" estimate, then **the ghost group was split again**
(04g exit paths / 04h turn loop) when a site-by-site read found three individually
hard cases in `do_ghost_turn`. **The conversion tail ran 04d–04j and is now
complete.** Undrafted phases were renumbered at each split, which costs nothing.
Per-file counts, verified against the tree with a multi-line-aware scan:

**⚠ Counts corrected twice, and the grep itself was the problem the second time.**

**Third pass (2026-07-26, at 04f review).** `grep -c "sessions\.lock()"` — used by
every count criterion in phases 04a–04f — **cannot see acquisitions that split
`sessions` and `.lock()` across lines.** A multi-line-aware scan found **5 such
production sites** that every prior survey and criterion missed:

| File:line | Consequence |
|---|---|
| `context/background.rs:118`, `:137` | 04f bounced — see `bugs/bug-04f-1.md` |
| `server/ask.rs:519`, `:686` | **04c was approved as "fully converted" and is not** — see its verdict correction |
| `stream.rs:896` | belongs to **04j** (post-split); already folded into its inventory |

Every remaining phase must use a multi-line-aware check, not `grep -c`. The bug
doc carries a working one.

**Second pass (2026-07-26).** The first survey used a plain `grep -c` that counted
`#[cfg(test)]` modules as production. Re-derived by splitting each file at its
`#[cfg(test)]` line:

| Group | Production | Test-only | Phase |
|---|---|---|---|
| `executor/mod.rs` | 10 | 0 | 04d — `done` |
| `executor/foreground.rs` + `executor/knowledge/*` | 8 | 0 | 04e — `done` |
| `context/background.rs` | **4** (13 → 2 → 4) | **11** | 04f |
| `ghost.rs` (11) + `briefing.rs` (1) | 12 | 0 | **04g** (4) + **04h** (8) |
| `background/{run,respawn}.rs` | 7 | 0 | 04i |
| `background/{helpers,gc}.rs` — **not conversions** | 2 | 0 | **05a** |
| `stream.rs` (8 + **1 multi-line** = 9) + `hook.rs` (3) | 12 | 0 | **04j** (10) + **05a** (`hook.rs:92`) + **05b** (`stream.rs:722`) |
| `server/ask.rs` — **2 multi-line stragglers from 04c** | 2 | 0 | unassigned |
| `webhook/process.rs` | 2 | 0 | **05b** (mechanism A/B, not a conversion phase) |

**Only `context/background.rs` was wrong** — the ghost, `background/`, and
`stream.rs`+`hook.rs` figures were already correct, because those files hold no
`sessions.lock()` in their test modules. So
the true total is **54 production sites** (18 already converted by 04d+04e, 34
remaining, plus webhook's 2 in 05b), not the 65 previously recorded.

`src/daemon/session.rs` still contributes **zero** production conversions: of its
four hits, `:432` is `with_sessions`'s own acquisition (correct and permanent),
`:443` is a doc comment, and `:1204`/`:1226` are tests.

**13 test-module sites now belong to 05c** (11 in `context/background.rs`, 2 in
`session.rs`). The newtype makes raw `.lock()` stop compiling, so 05c must convert
them — its scope is larger than "the 13 `Arc::clone` sites" implies.

**Phases 08–11 were added 2026-07-26** after a live incident (two daemons
sharing one `~/.daemoneye` tree; one took the socket from the other). They are
drafted and independent of the 04x lock-conversion sequence — 08 depends on
nothing and can be dispatched at any time. Design:
[`docs/design/daemon-instance.md`](../../../design/daemon-instance.md). See
Notes § "Instance ownership (2026-07-26)".

**Ordering was revised 2026-07-25** after the hang's root cause was found (see
Notes). The deadlock fix jumps to phase 02 — it is a live production defect that
takes the daemon down every hour, and it is small and fully specified.
Instrumentation, originally phase 03, drops to phase 06 and is rescoped: its
purpose was to make an unattributable wedge attributable, and this one is now
attributed. What remains for it is narrower — a watchdog for *future* wedges —
and it should only be drafted if phases 04–05 leave a real gap.

## Notes

**Phase 05 split in two, and the newtype moved behind it (2026-07-26, PE
decision).** The numbering had `04k` (newtype) sorting before `05` (the
restructures), which was backwards: the newtype makes raw `.lock()` stop
compiling, so it cannot land while any raw acquisition remains. Resolved by
renumbering the undrafted phases — nothing on disk changed names, since 04k was
never drafted:

| Was | Now | Scope |
|---|---|---|
| `05` (6 restructures) | **05a** | `background/helpers.rs::notify_session`, `background/gc.rs::gc_bg_windows`, `hook.rs:92` — 3 sites |
| — | **05b** | `webhook/process.rs` (2) + `stream.rs:722` — 3 sites |
| `04k` | **05c** | sessionstore-newtype + enforce — runs **last** |

**The split is by file, not by fix shape**, which was the opposite of the first
plan. Surveying the six sites showed `webhook/process.rs` holds **one of each
shape** — `notify_chat_panes` spawns a `tmux display-message` per session under the
guard, while `inject_into_sessions` only does `append_session_message` per session
under it. Splitting by shape would have put two phases into the same file; splitting
by file keeps each file wholly owned by one phase, which matters because the
non-zero-count criteria that guard these splits get fiddly when two phases share a
file.

**05a is uniform in shape.** All three of its sites spawn tmux subprocesses while
holding the guard, and all three take the same fix: collect what is needed under the
lock, release, then act. `cleanup_pass` (`src/daemon/session.rs`) is the worked
example — it is the fix that resolved the confirmed production hang, and
`hook.rs:92` is the same defect that was simply never fixed there.

**05b carries both shapes across 3 small sites** — one subprocess loop
(`notify_chat_panes`), two file writes (`inject_into_sessions`, `write_session_meta`).

**Line numbers moved.** These sites were recorded earlier as `stream.rs:719` and
`hook.rs:91`; after 04j's conversions they are **`stream.rs:722`** and
**`hook.rs:92`**. Re-derive with the multi-line-aware scan before drafting either
phase — `grep -c` remains retired for this purpose.

**Instance ownership (2026-07-26) — phases 08–11 added mid-milestone.** A live
incident: on 2026-07-25 two daemons ran concurrently against one
`~/.daemoneye/` tree for ~64 seconds, serving two different chat sessions, and
the second one unlinked the first's socket to bind its own. Full timeline and
root cause in `docs/design/daemon-instance.md` § 1.

The root cause is a design defect, not a coding slip: `daemon_is_running()`
inferred liveness from *responsiveness* (a 2 s Ping timeout) and the caller then
acted on that inference destructively. "Alive but busy" and "not running at all"
were the same `false`. There was no PID file and no lock anywhere in the tree —
the probe was the entire mutual-exclusion mechanism.

**This composes with the milestone's existing subject.** A `SessionStore`
deadlock — the confirmed defect phase 02 fixed, `daemon-stalls.md` § 1.5b — puts
the daemon in exactly the state the probe misreads: every thread `futex`-parked,
socket still listening, nothing answered. So a stall invites a second instance,
and the second instance shares the session store, `schedules.json`, and the
memory index with the first. The instance work is the blast-radius limiter for a
failure mode already observed in production, which is why it belongs in M5
rather than waiting for M6.

Three findings from the code read that shaped the phase split:

- **The existing guard, even when it fires, is too late.** It sits at
  `mod.rs:739`, but a duplicate reaching it has already deleted the live
  daemon's `de-pipe-*.log` files, repointed all four global tmux hooks at its
  own binary, run a memory migration, and spawned three pollers — and
  `anyhow::bail!` restores none of it. A duplicate launch was destructive
  whether the guard worked or not. Hence phase 08's central task is *ordering*,
  not just adding a lock.
- **A second, unambiguous duplicate signal was already being swallowed.** The
  webhook `TcpListener::bind` returns `EADDRINUSE` for a duplicate, but it lives
  inside `supervise(...)`, which retries forever with backoff. Phase 09.
- **`flock`, not a bare PID file.** The kernel releases a `flock` on process
  death including `SIGKILL`, so there is no stale-lock recovery path. A PID file
  alone needs "is that PID alive, is it really ours, was it recycled" guesswork —
  the same class of inference that caused the incident.

**Sizing:** 08 ~260 lines, 09 ~210, 10 ~130, 11 ~220. All four are mechanical
against a fully-specified design, which is the shape this executor handles well
(M4 retrospective). 08 is the only one that closes the hijack; 09–11 make the
next occurrence diagnosable in minutes rather than the several hours the
2026-07-25 forensics took.

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