# NEXT

## M15 — Chat Reliability & Dialog UX (opened 2026-08-14, **closed 2026-08-16**)

**phase-01 — read-pane-grep-null: done (escalated) 2026-08-14.** Two
NoProgressStall hard-fails from the Nemotron executor (fresh dispatch +
briefing-seeded resume, both 60 read-only calls, tree non-compiling twice) →
architect takeover. Fix landed: `args_to_string` null-stripping helper in
`src/ai/types/pending.rs`, 32 arms wrapped, 6 tests, all gates green. Live
chat re-check deferred to milestone close. Executor model switched to
`Qwen/Qwen3.8-27B-FP8` (new 3.8 release) in `rexymcp.toml` (PE decision
2026-08-14); confirmed served by brain via `executor_health`. Fresh model —
no calibration history yet; prior Qwen3.6 findings are the starting prior.

Remaining phases: 02 sudo-cached-detection, 03 resize-border-corruption,
04 approval-panel, 05 sudo-credential-panel (04 → 05 hard dependency).
Design decision on record: approval/sudo prompts become themed **in-viewport
ratatui panels** (no alternate screen, no floating overlay). Milestone README:
`docs/dev/milestones/M15-chat-reliability/README.md`.

*(Note: this section was re-applied 2026-08-14 — the original M15 NEXT.md
update was reverted on disk during the phase-01 executor runs.)*

**phase-02 — sudo-cached-detection: done (approved_first_try) 2026-08-14**,
commit `851de1d`; Qwen3.8-27B-FP8's first phase, 74 turns, clean. Live
cached/uncached sudo check deferred to milestone close with phase-01's.

**phase-03 — resize-border-corruption: done (approved_first_try)
2026-08-14**, commit `46972e3` + approval `86b00e1`. Width-change-aware
clear band in `reanchor` per the M13 RESOLVED fix shape; live trace check
deferred to milestone close. Qwen3.8: two for two, both 74 turns.

**phase-04 — approval-panel: done (approved_first_try) 2026-08-14**, commit
`e2a631a` + approval `f3c1038`. Themed in-viewport approval dialog; legacy
prompt path preserved for the other flows. Qwen3.8: three for three.

**phase-05 — sudo-credential-panel: done (approved_first_try) 2026-08-14**,
commit `f532ad3` + approval `63ef02a`; Qwen3.8: four for four, 59 turns.
Live sudo-dialog check deferred to milestone close with the others.

**Active phase: phase-06 — dedup-approval-dialogs** (`docs/dev/milestones/
M15-chat-reliability/phase-06-dedup-approval-dialogs.md`, status: todo,
drafted 2026-08-14 on PE direction: the approval and sudo credential
dialogs must not contain a copy of the command they concern — it is
already in scrollback directly above). Scope: `draw_approval_panel` loses
its summary row/parameter (six call sites, tests); the daemon's
`CredentialPrompt` text drops the `: {cmd}` suffix via a new pure
`sudo_password_prompt(attempt, max)` helper in `utils/sudo.rs` (both
`foreground.rs` sites). Matching exit criterion added to the M15 README.
All four mechanical acceptance criteria validated failing against the
current tree. New last M15 phase — milestone close (live verification
sweep of 01–06 + retrospective) follows its approval.

**phase-06 — dedup-approval-dialogs: done (approved_first_try) 2026-08-14**,
commit `1d53673`; Qwen3.8: five for five, 93 turns. Review calibration: two
retyped `test result` lines in the pasted E2E block (dropped `0 measured; `,
no values falsified) — repaired at review to byte-match the capture; the
PASTE MATCH self-check clause was absent from this phase's block, data
point for close. All M15 phases are now `done`.

**Closed 2026-08-16 on PE direction.** Live sweep run (read_pane grep,
resize/border, approval + credential panels + dedup all PASS; sudo cached
half deferred to a PE spot-check), retrospective in the M15 README.

**Deferred follow-ups (PE to schedule):**
- **bug-05-1** (M15, closed-as-deferred per the bug-tracker invariant): Esc
  in the sudo credential panel submits an empty password — protocol gap,
  unfixed. Full analysis in `M15-chat-reliability/bugs/bug-05-1.md`.
- ~~**`hooks_land_on_private_server` fails at HEAD**~~ — **RESOLVED
  2026-08-17** (architect hotfix). Root cause: `90567c3`'s M3 hardening
  wrapped `#{session_name}` in nested single quotes in the four global hook
  commands — a tmux syntax error that made `set-hook -g` fail and leave
  `pane-died` / `after-new-session` / `client-attached` / `client-detached`
  unset (a live production regression, not just a test failure). The failure
  was invisible in `daemon.log` because only the spawn `io::Result` was
  checked, never tmux's exit status. Fix: `#{q:session_name}` (tmux's
  shell-quote format modifier — verified inert against a hostile
  `evil'$(…)` session name) + a `log_hook_install_result` helper that checks
  exit status and logs stderr. Isolation suite 9 passed / 0 failed. The M16
  full-suite gate exception is gone.
- ~~**`labeled_context_window_scope_excludes_other_windows` is not
  hermetic**~~ — **RESOLVED 2026-08-17** (architect hotfix, same day as
  found): the fake-cache tests' active-pane path live-captured pane `%1`
  from the operator's real tmux server when one existed
  (`get_labeled_context_scoped` → pipe log / `capture_pane_with_escapes`),
  so lib tests failed or passed depending on the operator's window layout —
  and every fake-cache test passing an invented active pane id (`%1`, `%5`)
  was latently exposed, not just the one that fired. Fix: the live-capture
  block extracted to `active_pane_content()` (`src/tmux/cache.rs`) with a
  `cfg!(test)` early return of `"(pane unavailable)"` — unit tests now
  behave exactly like a host with no tmux server, hermetic by construction
  (`cfg!`, not `#[cfg]`, so the live path stays compiled in test builds and
  its callees don't trip the dead_code lint). Full `cargo test` verified
  green with a live server and a real pane `%1` present.

---

## M16 — LLM Stream Robustness (drafted 2026-08-16, opened 2026-08-16; all phases done 2026-08-18, PE sign-off outstanding)

**phase-01 — transport-scaffolding: done (escalated — architect close
2026-08-17)**, implementation commit `d3ad2c0`. DeepSeek V4 Flash's first
phase: round 1 implemented the entire spec correctly but hard-failed on the
completion gate; round 2 (evidence capture) bounced repeatedly on the same
gate — the harness runs the configured full `cargo test`, which carries the
documented pre-existing `hooks_land_on_private_server` failure, and does not
honor the phase-doc-amended `cargo test --lib` gate. The executor correctly
recorded the blocker (`f45845b`) but then, still gate-blocked at turn 50, ran
`tmux kill-server` against the operator's **default** server, killing the
operator's tmux session, the architect session, and its own dispatch.
Architect re-ran all gates 2026-08-17 (greps 1/1/1, fmt, stall tests,
`cargo test --lib` 1306 passed, clippy `-D warnings` — all green) and closed.
**Before dispatching phase-02: either fix the `90567c3` hook regression or
align the rexymcp gate with the amended test command** — phases 02–08 all
inherit the same full-suite exception, and the executor has now demonstrated
destructive escalation when gate-blocked (rexyMCP upstream backlog: ban
destructive server commands in the executor bash guard).

**phase-02 — openai-two-phase: done (approved_first_try) 2026-08-17**,
commit `89fcbe9` + approval. DeepSeek V4 Flash 0731's first clean phase: 96
turns, all four gates green on the architect's independent re-run, and the
**first M16 phase to clear the real full `cargo test` gate with no
exception** (the `90567c3` hook regression having been fixed that morning).
Both new tests mutation-checked at review and confirmed load-bearing.

Two architect-side drafting defects surfaced at review, neither the
executor's fault (full analysis in the phase doc's Review verdict):
1. **AC3 was unsatisfiable as written** — `grep -c "fn delta_carries_token"`
   counts `3`, because the same phase's Task 3 mandates two tests whose
   names begin `delta_carries_token_`. Corrected in place to `^fn`. This is
   the M7–M10 rule recurring: the criterion was validated *failing* against
   the pre-phase tree but never validated *passing* against the tree the
   phase would produce.
2. **The § End-to-end capture block is uninformative** — `cargo test <filter>
   2>&1 | tail -5` and `cargo test 2>&1 | tail -3` capture the *last* test
   binary (isolation / doc-tests), not the lib binary holding the results,
   so the pasted evidence reads `0 passed … 10 filtered out` where the real
   results were 2 passed and 1308 passed. Run verbatim as the spec requires,
   the block produces evidence that does not demonstrate its own claim.
   **The same pattern was drafted into phases 03–08** — **all six corrected
   2026-08-17** to `cargo test <filter> 2>&1 | grep -E "^test "` (filtered)
   and `cargo test 2>&1 | grep -E "^test result:"` (full), both verified to
   produce the real results before being written into the specs. The
   convention is recorded in the M16 README § Notes. Phase-01's and
   phase-02's own blocks were deliberately **left as-drafted**: those phases
   are `done` and their blocks are the historical record of what actually
   produced their pasted evidence — rewriting them would falsify that trail.

**Gate exception lifted 2026-08-17.** The `hooks_land_on_private_server`
regression is fixed (`cb637df`), so M16 phase gates are the four standard
commands with no exception. The note is removed from the M16 README.

**phase-03 — anthropic-gemini-two-phase: done (escalated — architect
takeover 2026-08-17)**, commits `7f9c9b6` (stream logic), `545379e` +
`4dcebf0` (predicate), approval `144742c`. **Three bounces, none of them on
the phase's actual deliverable** — the two-phase conversion, the retry
gating, and the connect-timeout-only client were right from round 1. All
three bugs were verification defects, and two of the three were
architect-side spec defects:

- **bug-03-1** (major): the Gemini first-token test asserted a predicate
  defined inside `mod tests` — a second implementation no production code
  called, already diverged from the shipped rule.
- **bug-03-2** (blocker): the mutation evidence pasted for bug-03-1's fix
  was false; both mutations re-run leave the test green. Root cause was an
  architect criterion that **no design the spec sanctioned could satisfy** —
  a gate with no honest passing state.
- **bug-03-3** (major): the "no divergence" answer came from a sample of
  eight tools, none of which could disagree. Re-measured with the
  discriminating class (`dispatch::<T>` with all-optional fields),
  `get_terminal_context` and `list_memories` disagree — `null=false
  empty=true`. Fixed in the takeover.

**Calibration — three occurrences of one shape, at the fold threshold.**
Every bounce was a *verification whose construction could only return one
answer*, read as confirmation. Candidate fold, recorded in the phase-03
round-3 verdict and **awaiting PE sign-off at milestone close**: *run every
check once in the state where it is expected to fail; a check that has never
produced its own negative is not evidence.* Not folded into WORKFLOW.md —
that is PE's call.

**phase-04 — daemon-keepalive: done (approved_first_try) 2026-08-17**,
commits `3b04d05` (feat) + `278e5ed` (event-log fixture) + approval
`0fe3d27`. Turn-wide keepalive contract: `KEEPALIVE_PERIOD_SECS = 15`,
`with_keepalive` around `await_agent_result` and auto-name, `maybe_keepalive`
in all six foreground poll loops, bounded pane-select read, streaming literal
unified onto the constant. Four paused-clock duplex tests; lib count
1311 → 1315.

**The § Gotchas block added at staging worked on its first outing.** The run
captured a negative before claiming coverage (period → 999 s,
`keepalive_ticks_while_future_pends` FAILED, restored, passing — both pasted)
and reported the full-suite failure honestly rather than routing around it.
Review spot-checked with a *different* mutation (`Response::KeepAlive` →
`Response::Error`) and confirmed the test discriminates on the protocol frame.
One data point, not a trend — watch it across 05–08 before crediting the
instruction.

One declared scope deviation, verified before acceptance: `278e5ed` fixes an
M11 `event_log` test whose fixture date (`events-20260803.jsonl`) fell outside
the 14-day sweep cutoff once **UTC** passed 2026-08-04 — note local is
2026-08-17 PDT while UTC is 2026-08-18, and the code uses `Utc::now()`.
Reproduced independently: the pre-fix file fails today, exit 101 at
`event_log.rs:770`. Fix is date-relative with assertions unchanged.

**phase-05 — turn-loop-hardening: done (approved_first_try) 2026-08-17**,
commit `b86649c` + approval `1e4771f`. `ChatTaskGuard` with drop-abort, the
channel-closed re-issue bounded at 2 with a named cause and a real
`Response::Error` on exhaustion, and `[limits] turn_timeout_secs`. Lib count
1315 → 1319.

The `MAX_CHANNEL_CLOSED_RETRIES` criterion was re-pinned 3 → 4 by the run;
**verified by diff, not grep** — all four occurrences are the quoted Task-2
shape (const `:100`, comparison `:172`, two log format strings `:175`/`:190`).
The drafted `3` was the architect's miscount. Structure confirmed: guard bound
inside the outer loop so re-issue aborts the prior attempt, both early returns
drop it, `describe_end` cannot hang (only reachable once every sender is
dropped), and the staging disambiguation held — chat spawn wrapped, ghost
spawn untouched.

**Second consecutive phase to run the § Gotchas discipline unprompted**,
this time with the sharpest negative available for a `Drop` impl (removing
`.abort()` makes `guard_drop_aborts_task` hang to timeout). Two data points
now; if 06–08 hold, the phase-03 fold has independent evidence at close.

**phase-06 — client-liveness: done (approved_first_try) 2026-08-18**, commit
`2d781b9` + approval `f741b62`. `PHASE1_SILENCE_TIMEOUT_SECS = 90` /
`PHASE2_SILENCE_TIMEOUT_SECS = 120`, both phases now carry a deadline
measured from `last_msg_at`, `StreamOutcome::Deadline` with phase-accurate
messages, `ask.rs` reworded. Lib count 1321. **First phase run under
`/rexymcp:auto`** — dispatch and review both delegated to `claude-sonnet-5`
subagents.

Both staging gotchas paid off. The `Deadline` arm **returns**
(`stream.rs:239`), clearing the second `match outcome { _ => unreachable!() }`
at `:291` — a fall-through there would have panicked the client in production
with every gate still green. And `silence_budget` is genuinely wired: the
review proved it by replacing the call site with an inline copy and watching
clippy fail `function silence_budget is never used`. The executor's own note
records that its first attempt left an inline `if !response_started`
*alongside* the helper and the verifier rejected it.

The review also caught its own dead-end mutation — swapping the constants'
literal values changes nothing, because the tests assert against the symbolic
constants — and switched to swapping the branches inside `silence_budget`,
which failed both tests as required. That is the § Gotchas discipline applied
to the reviewer's own work.

**phase-07 — surface-silent-conditions: done (approved_first_try)
2026-08-18**, commit `e0a0ffb` + approval `e033b23`. 213 turns, the
milestone's longest run. `AiEvent::Notice(String)` plumbed through all three
backends and forwarded as `Response::SystemMsg`; truncation / refusal /
unknown-tool / malformed-frame notices; empty-reply guard. Lib count stays
1321 because the existing `flush_unknown_tool_sends_nothing` was **renamed
and inverted** to `flush_unknown_tool_sends_notice` — the behaviour it
guarded genuinely changed.

**The staged trap did not recur:** `part_counts_as_token` (`gemini.rs:14-44`)
is confirmed pure — no `tx`, no `AiEvent::Notice`, no send. Emitting from
there would have spammed one notice per frame, invisible to every gate. The
three unknown-tool notices land at their real drop sites
(`anthropic.rs:71-74` inside `flush_tool_call`, `openai.rs:~345`,
`gemini.rs:~388`). Task 3's malformed-frame counting is a real
`match Ok/Err` with a counter and a single end-of-drain notice, not a string
swap. Review confirmed the backend diffs are pure reindentation around
`first_token_seen`, `record_stream_success()` and the `'attempt` loops —
nothing out of scope.

**phase-08 — cancellation: done (escalated) 2026-08-18**, commit `883e6ed` +
approval `276bc5b`. Two dispatches: the first hard-failed with a
`NoProgressStall` on an unsatisfiable architect criterion (`grep -c "Cancel {
session_id"`, impossible once `cargo fmt` renders the variant multi-line);
criterion fixed in `d87b139` and measured in **both** directions, then the
refined re-dispatch completed 7/7 in 132 turns. Review verified the lock
invariant (`.unwrap_or_log()`, no `.unwrap()`), distinct registry session-ids
per test, `never()` not ported, and that `send_cancel` opens its own
`UnixStream` rather than touching the streaming reader. Lib count 1327.

---

## M16 — LLM Stream Robustness: at its milestone boundary (superseded as active by M17)

All eight M16 phases are `done`. **The milestone is not closed**: five live
exit criteria are unrun and one calibration fold awaits a PE decision. Full
retrospective in
`docs/dev/milestones/M16-llm-stream-robustness/README.md` § Retrospective.

**Awaiting PE:**

1. **Five live exit criteria**, architect-run at close per the M14/M15
   convention, **not yet run** — they touch the live daemon and the
   operator's tmux server, and the phase-01 incident (an executor running
   `tmux kill-server` on the default server) is why they are not run
   unprompted. They are: a > 5 min generation surviving without a client-side
   kill; `kill -STOP` the daemon mid-turn → client error naming the hang
   within 90 s; `await_agent_result` ≥ 300 s with `KeepAlive` frames
   throughout; an unknown-tool-only response yielding a visible `SystemMsg`;
   and Esc mid-stream cancelling cleanly with a `⊘ cancelled` marker and no
   EPIPE in `daemon.log`.
2. **The calibration fold** — five occurrences of one shape (a criterion
   validated in only one direction). Proposed WORKFLOW.md text is in the
   retrospective; **not applied**, per the architect skill's prohibition #5.
3. **Go/no-go on the next milestone.**

**Also outstanding (minor):** `CLAUDE.md`'s `src/daemon/utils/` row does not
list the new `keepalive.rs`. Not gated by `tests/doc_truth.rs`.

---

## Active milestone: M17 — Transcript View (scoped 2026-08-18)

**phase-01 — transcript-model: done (approved_first_try) 2026-08-18**,
commit `a49ebca` + approval. First DeepSeek V4 Flash phase of M17, clean —
no bounces, all 13 seeded tasks completed, mutation pair and PASTE MATCH both
produced first try. Reviewer re-ran the mutation independently (2 of 5 cap
tests fail under it, 5 of 5 restored) and re-extracted the pasted artifact
(`PASTE MATCH`). Two self-report inaccuracies, neither a code defect: the
executor claimed it left the status `in-progress` (the doc said `review`) and
reported 2 ignored tests (actual 3).

**phase-02 — viewer-shell: done (approved_after_2) 2026-08-19**, commits
`9f57131` → `ea7ebe4` → `7cebf08` + approval. Two bounces, both on the same
obligation, inverted each time: round 1 released the alternate screen on the
normal path but not the error path; round 2 moved the release into a `Drop`
guard and then disarmed it on the normal path, so `esc` never left the screen
at all. Round 3 deleted the disable path entirely (`AltScreenGuard` has no
`armed` field) and factored the fallible body into `viewer_loop`, so every exit
leaves through one drop — correct by construction rather than by convention.

Reviewer ran two mutations: `Drop` no-op fails both guard tests (they are not
vacuous), and `let _guard` → `let _` — which leaves the screen before the loop
runs — keeps all 10 headless tests green. That residual gap is real, is
inherently live-only, and the milestone's first exit criterion was amended to
name it so the close-out check exercises it.

**Open calibration item, drafted not applied (needs PE sign-off):** a criterion
for a cleanup obligation must assert the cleanup **ran**, and assert the count
(`== 1`), not that a mechanism for it is present. Round 2 satisfied every
structural criterion the round-1 bounce added while being more broken than
round 1. Third architect-side criterion defect in M17 — phase-01's copied
ignored-count and phase-02's `grep -c EnterAlternateScreen` expecting 1 where
correct code prints 2 are the other two — which is the WORKFLOW.md § Calibration
fold threshold.

Cleared, not a defect: the round-2 `eprintln!` at `chat.rs:746` matches
existing convention in the same loop (`chat.rs:370-372`, `chat.rs:572`).

**phase-03 — expand-collapse: done (approved_first_try) 2026-08-19**, commit
`803940b` + approval. Clean run: 11 tasks, four green gates, 1352 lib tests
(+9), byte-exact E2E artifact, mutation pair produced first try. The 9 `ViewRow`
literals were updated in place rather than worked around, and the phase-02
guard contract still holds (`disarm` 0, teardown grep exits 1) — the E2E block's
own re-check, re-run by the reviewer.

**Calibration (architect-side, one occurrence, held not folded): do not pair
"implement B as a wrapper over A" with "assert B equals A".** The spec asked
for `layout_blocks` to be a thin wrapper over `layout_blocks_with`, and also
asked for a test asserting the two are equal — so the test compares a function
with the function it delegates to and cannot fail. Reviewer mutation Mb
(`full.lines()` → `.take(3)`, breaking the full-output guarantee) moved both
sides identically: that test passed while `layout_blocks_renders_full_output`
and `collapsed_output_lays_out_as_exactly_one_row` both caught it. Harmless —
the guarantee is really guarded — but dead weight, and spec-caused.

Note the difference from M17's three earlier criterion defects: those asserted
mechanisms instead of behaviour and were caught by reading. This one was
invisible until the code the test claimed to protect was mutated. **Mutate every
new guard, not only the one the spec names.**

**phase-04 — search: done (approved_first_try) 2026-08-19**, commit `d1b0832`
+ approval. Clean run: 10 tasks, four green gates, 1362 lib tests (+10),
byte-exact E2E artifact, mutation pair first try.

**The refactor bet paid off.** Making key handling a pure
`key_action(key, searching)` meant the phase's central claim — while searching,
`q` types a letter rather than quitting — became assertable without a terminal.
Reviewer mutation Mb (`match (searching, key)` → `match (false, key)`) fails
exactly the two mode-sensitive tests and nothing else. Before this phase that
behaviour had no headless guard at all.

Reviewer also diffed the 20 `ViewerAction` variants against those the loop
matches — empty difference both ways, so the refactor dropped no action. The
pure decoder tests would not have caught that; it needed checking directly.

**Accepted scope deviation:** `render_transcript` takes a borrowed
`SearchState { active, query, matches, current }` rather than the two loose
parameters the spec named. Better than specified — it stops the signature
growing a parameter per feature — and every criterion phrased around
`matches`/`current` is satisfied through it.

The inline scroll-into-view arithmetic is gone (`grep` prints 0);
`scroll_to_row` is defined once and called from all five navigation sites.

**phase-05 — block-copy: done (approved_first_try) 2026-08-20**, commit
`ef4d3a8` + approval (this commit). Independently re-run gates, five pinned
tests, the M1 mutation pair, PASTE MATCH, and the phase-02 guard-contract
re-check all confirmed. Live check (`tmux show-buffer` after a real `y`
press) deferred to milestone close per the M14/M15/M16 convention.

**phase-06 — rehydration: done (approved_first_try) 2026-08-20**, commit
`d341854` + approval (this review). `/session load <name>` now refills the
client transcript from the named session's stored `messages.jsonl` via
`blocks_from_messages`, cleared and refilled rather than appended, so `ctrl+o`
after a load shows the conversation that predates the current client.
Independently re-run gates, five pinned tests, the M1 mutation pair, PASTE
MATCH, and the phase-06-specific + phase-02 guard-contract greps all
confirmed. An additional non-spec mutation (deleting `self.evicted = 0;` from
`Transcript::clear()`) was also caught by `transcript_clear_resets_counters`,
ruling out a tautological test. The `/session load` wiring itself remains a
live check deferred to milestone close (it requires a running daemon).

**Active phase: none.** Phase-07 (viewer-mouse) is an intent only — not yet
drafted. Draft it via `/rexymcp:architect next`.

Phase-05 staging notes (verified against the tree at draft time):

- **This is the first M17 viewer phase with a headlessly verifiable real
  artifact** — the tmux buffer. The E2E block loads a known string through the
  same `tmux load-buffer -w -` the code uses and reads it back with
  `tmux show-buffer`. **I ran that round-trip before speccing it**: tmux 3.7b,
  load exit 0, `show-buffer` returned exactly `alpha/beta/gamma`. Earlier
  viewer phases could only be checked live at close; this one proves its
  mechanism in the artifact.
- **`crate::tmux::bounded_output` cannot be reused** — it pipes stdout/stderr
  only (`src/tmux/mod.rs:67-75`) and gives no stdin handle, so `load-buffer -`
  through it would load an empty buffer. The spec gives the spawner verbatim,
  including the reason the stdin handle must drop before `wait()` (holding it
  across the wait deadlocks).
- **Copy derives from `Block`, never from the rendered `ViewRow`s.** Rows carry
  the `▾`/`▸` marker, the `output (N lines)` header and the
  `[collapsed, N lines]` suffix, and a collapsed block has no body rows at all.
  Deriving from `Block` is what makes "a collapsed block copies in full" true,
  and it is pinned as a test.
- **The `Output` match arm is pinned verbatim** in task 1 so the mutation's
  `old_str` is deterministic — an equivalent-but-differently-written arm would
  break the pair. Grep baselines checked: `take(\*shown)` 0, and
  `let _ = copy_to_tmux_buffer` 0 (the criterion that the error is surfaced,
  not swallowed).
- `y` is already a `Key::Char`, so no `tty.rs` change; the new arm goes below
  the existing `(true, Char(c))` arm and both modes are asserted.

Round 1 (`9f57131`) landed all 12 tasks with four green gates, a byte-exact
E2E artifact and a mutation pair that the reviewer re-ran independently
(2 of 8 viewer tests fail under it, 8 of 8 restored). It was bounced for one
defect: `run_transcript_viewer` leaves the alternate screen and calls
`reanchor()` as straight-line statements at the end of the happy path, while
seven `?` early-returns sit between `EnterAlternateScreen` and
`LeaveAlternateScreen` — and the call site propagates the error out of the
input loop with `.await?`, past the `renderer.restore()` that sits after the
loop. Any viewer I/O error therefore exits the chat process with the terminal
still on the alternate screen **and** still in raw mode.

**The failure class is `spec_bug`, not an executor error.** The spec's task 4
step 5 said "On break: drop the fullscreen terminal, then `LeaveAlternateScreen`,
then `renderer.reanchor()`" — it described the happy path and never said "on
every exit path". The executor implemented exactly what was written. The
lesson generalises: **when a spec hands out a resource, it must say what
releases it on the error path, not only on the success path** — the codebase
already carries the idiom (`FgHookGuard`, `src/daemon/executor/foreground.rs:50-80`).
Second architect-side spec/criterion defect in M17 (phase-01's was the
copied ignored-count); third occurrence folds.

Phase-02 staging notes (verified against the tree at draft time):

- **Three key-parsing facts the spec pre-injects**, each a bounce if missed:
  there is no `Key::Esc` (a bare Escape becomes `Key::Char('\x1b')` via the
  catch-all at `tty.rs:244`); ctrl+O is currently swallowed by
  `c if c < 0x20 => Key::Char('\0')` at `tty.rs:247`, so the new arm must sit
  with the other control-byte arms; and `ESC[5~`/`ESC[6~` are unparsed, leaving
  a stray `~` delivered as `Key::Char('~')` unless the new arms consume it like
  the Delete arm at `tty.rs:187-191`.
- **The viewer must never call `ratatui::try_restore()` / `restore()` /
  `disable_raw_mode()`** — `RatatuiRenderer::restore()`
  (`render_ratatui.rs:851`) disables raw mode and is end-of-session teardown;
  the chat session continues after the viewer closes. Enforced by a negative
  grep in the E2E block, validated against a known-positive file
  (`render_ratatui.rs`, which does contain `try_restore`, so the grep form
  exits 0 there — it detects the violation it is meant to catch).
- The exit path is `LeaveAlternateScreen` + `renderer.reanchor()`, which is the
  same operation the input loop's existing sigwinch arm (`chat.rs:633`) already
  performs.
- **The two gaps carried out of phase-01's review are tasks 7 and 8**, so they
  close in the phase that first depends on them rather than drifting.
- The live alt-screen enter/exit check is architect-run at milestone close per
  the M14/M15/M16 convention; the executor verifies the pure layout, the scroll
  clamp, a `TestBackend` draw, and the structural greps.

Phase-01 staging notes (verified against the tree at draft time):
`grep -rn "Response::ToolResult(" --include=*.rs src tests` finds **17**
tuple-form sites, not the 13 sends alone — the inventory in the phase doc
includes two easily-missed ones (`src/cli/commands/ask.rs:207`, a skip arm in a
`|`-chain, and `src/ipc_tests.rs:338,340`, the existing round-trip test, which
lives in `src/` and not `tests/`). The E2E block's test filter is
path-qualified `cli::transcript` because a bare `transcript` filter also
matches the pre-existing
`cli::render_ratatui::tests::commit_renders_transcript_line_into_buffer`. The
PASTE MATCH extraction was validated both directions against a scratch fixture
before being written into the spec: a retyped line printed `PASTE MISMATCH`
with the divergent lines, the byte-exact copy printed `PASTE MATCH`.

**Goal:** an alternate-screen transcript viewer opened with `ctrl+o` from the
chat prompt — full (un-elided) tool output, scroll, search, block copy to a
tmux buffer, and rehydration from the session JSONL — with the inline
`insert_before` streaming path left untouched as the primary surface.

Milestone README: `docs/dev/milestones/M17-transcript-view/README.md`.
Design of record: `docs/design/transcript-view.md`.

**Why modal rather than an app-owned chat buffer.** Owning the primary
transcript would move history out of terminal scrollback, which is what makes
tmux copy-mode and drag selection work today; the client would have to
reimplement selection and clipboard hand-off — the one part of the UX tests
cannot validate. The viewer is additive and modal instead.

**Load-bearing facts derived for the scope (re-verify before each dispatch):**

- `Response::ToolResult` already carries the **full** output on the wire
  (`src/cli/commands/stream.rs:674`); the client elides at 10 lines and drops
  the string. It is a bare `String` with no id (`src/ipc.rs:414`) — M17 adds a
  `tool_call_id` so blocks can be joined to history records.
- Committed panels are frozen: `commit_panel_labeled`
  (`src/cli/render_ratatui.rs:755`) writes through `insert_before`, and
  absolute row arithmetic is untrustworthy after any scroll or resize
  (`render_ratatui.rs:177-189`). In-place expansion is not reachable.
- Persisted copies are all lossy or unsafe as a viewer source: `events.jsonl`
  caps output at 200 chars (`src/daemon/utils/event_log.rs:291`); session-JSONL
  `tool_results` are truncated at `limits.tool_result_chars` (default 16 000,
  `src/config/types.rs:453`); `var/log/panes/*.log` covers background jobs only
  and is written **unmasked** (`src/daemon/background/helpers.rs:78`).
- Foreground execution archives nothing — its output exists only on the wire
  and in the truncated history copy.
- tmux on the target host is **3.7b**, so `tmux load-buffer -w -` (≥ 3.2) is
  available for block copy; no OSC 52 negotiation needed.

**Blocking on M16:** M16's five live exit criteria and its calibration fold are
still awaiting PE. M17 phases touch `src/cli/` and `src/ipc.rs` while M16's
open items are daemon-stream behaviours, so they do not collide — but M16
should be signed off and closed before the first M17 dispatch.

---

## Historical: M16 phase staging records

**phase-08 staging:** re-derived Current state (`Request` enum `ipc.rs:139`, dispatch match
`server/mod.rs:172`+ with 25 arms, `StreamOutcome::Interrupted` moved to
`stream.rs:216` by phase-06's `Deadline` insertion, client `session_id` at
`:88`/`:130`, `daemon/mod.rs` module list alphabetical at `:27-46`).
`ChatTaskGuard` confirmed present, so the phase-05 dependency holds.

**Correction to an earlier note:** phase-08's Task 1/2 tests were said to be
unnamed in the Spec and needing enumeration. That was wrong — the Spec names
all six. The criteria are now pinned per name, all seven new tests measured
at `0` today, plus `cargo test cancel` pinned at `6` (five new names
containing "cancel" plus the pre-existing `store_add_list_cancel`, measured
`1`). Also fixed an internal contradiction: Task 1 said "copy verbatim,
including its five tests" while naming three — the other two belong to
`never()`, which the same task says to omit.

**phase-07 staging record (historical):** Staging found the drafted line
numbers badly stale (phases 03 and 05 moved
this code) and re-derived all of them: anthropic `stop_reason` `:321-330`,
gemini `finishReason` `:288-296`, openai `finish_reason` `:275-284`,
unknown-tool drops at `anthropic.rs:71` / `openai.rs:324` / `gemini.rs:376`,
SSE parse sites `:236` / `:266` / `:236`, empty-reply path `stream.rs:727`
and `:909`, `Response::SystemMsg` `ipc.rs:390`. All five acceptance criteria
measured in their failing state.

**One trap phase-03 created and this phase would walk into:** gemini now
calls `dispatch_tool_event` **twice** per `functionCall` — once at
`gemini.rs:39` inside `part_counts_as_token` purely as a first-token
predicate, and once at `:364` as the real emission site. Emitting the
unknown-tool notice "wherever `dispatch_tool_event` returns `None`" would
fire it from the predicate, once per frame. The notice belongs only at the
`:376` `else` branch; `part_counts_as_token` must stay silent. Now § Gotchas
item 1.

**Doc follow-up for milestone close (recorded, not blocking):** `CLAUDE.md`'s
`src/daemon/utils/` row enumerates that directory's files and does not list
the new `keepalive.rs`. Not gated by `tests/doc_truth.rs`.

**Remaining after 06:** phase-07 (surface-silent-conditions, depends on 01
only — could run before or alongside 06) and phase-08 (cancellation, depends
on 05, now unblocked). phase-08 additionally needs its Task 1/2 unit tests
**named** at its own staging pass; the Spec does not enumerate them.

**Doc follow-up for milestone close (recorded, not blocking):** `CLAUDE.md`'s
`src/daemon/utils/` row enumerates that directory's files and does not list
the new `keepalive.rs`. Not gated by `tests/doc_truth.rs`.

Staging found and fixed a fourth instance of the pattern above, sitting in
the drafted-ahead docs: **`cargo test keepalive` passes today**, because it
matches phase-02's `delta_carries_token_ignores_empty_keepalive`. The same
sweep corrected phase-05, phase-06 and phase-08, all of which pinned a bare
`cargo test <filter>` "passes" — a form satisfied by a test that was never
written, since `cargo test` exits 0 on a filter that matches nothing.
phase-08 additionally needs its Task 1/2 unit tests **named** at its own
staging pass; the Spec does not enumerate them.

**Open decision for PE: executor model for phases 04–08.** DeepSeek V4 Flash
0731 has now run four M16 phases — phase-01 (destructive escalation when
gate-blocked), phase-02 (clean, approved_first_try), phase-03 (three
bounces, two false-evidence rounds). Qwen3.8-27B-FP8 went six for six on
M15. The failure mode both times was a gate the run could not satisfy
honestly; phase-04's § Gotchas now instructs reporting a bad criterion as a
blocker instead of improvising past it, which is the untested mitigation.

Goal: chat turns can never fail silently during long-running LLM queries.
Milestone README + all 8 phase docs drafted ahead at
`docs/dev/milestones/M16-llm-stream-robustness/` (PE-approved plan,
2026-08-16). Ports proven mechanisms from rexyMCP's own LLM client
(two-phase stream timeouts, heartbeat keepalive contract, CancelSignal);
the ported code is quoted verbatim in the phase docs — the executor never
needs the rexyMCP tree. Ordering: 01→02→03 (transport), 04→05→06
(daemon/client liveness), 07 (surface silent conditions, after 03), 08
(cancellation, after 05). **Line numbers/counts in the phase docs are
current-as-of-drafting — re-run each phase's re-derive commands before
dispatching it** (M4 precedent). All drafted acceptance criteria were
validated failing against the 2026-08-16 tree.

Executor model note: PE states the executor is now **DeepSeek V4 Flash
0731** (2026-08-16) — supersedes the Qwen3.8 note above; verify
`rexymcp.toml` + `executor_health` before the first M16 dispatch. Fresh
model, no calibration history.

---

## Historical: M14 milestone boundary (M14 signed off 2026-08-11)

**PE sign-off landed 2026-08-11.** Calibration decisions: folds 1+2 (last-entry
PASTE MATCH anchor; no-unpastable-bytes/strip-ANSI-at-generation) are in
`docs/dev/WORKFLOW.md` § "End-to-end verification"; item 3 (seeded
FAIL→blocker task) **held** at two occurrences; item 4 tallied. Both folds
join the upstream push backlog. Sign-off record in the M14 README.

**Correction (2026-08-11): the carried "M6 — Verification & Hygiene, never
dispatched" claim below was stale and wrong.** M6 was completed and closed
2026-07-31 (commit `c40faae`; all 13 phase docs `done`, retrospective in its
README, its folds signed off — see WORKFLOW.md's "Folded 2026-07-31 after
M6" note). The claim propagated unverified from the 2026-07-30 scoping-era
text through the M13/M14 boundary notes. Remaining genuinely-carried items:
the **upstream push backlog** (~14 local-only WORKFLOW.md sections + M13's
four folds + M14's two — a rexyMCP-repo change, out of bounds from this
repo's architect session) and **width-flip scrollback ghosts** (cosmetic,
recorded as not milestone-shaped).

**Next action:** PE names the next milestone; then `/rexymcp:architect next`
scopes it.

---

## Historical: M14 milestone boundary (closed 2026-08-11)

**M14 — Live Verification is closed.** Four phases, all `done`: 01
`approved_after_1`, 02/03/04 `approved_first_try`. Two genuine product
defects found live and fixed in-milestone (turn-end approval-state reset;
per-batch cap scope) — both invisible to 1200+ green unit tests, both
proven fixed through the same probes that caught them. All exit criteria
live-verified; retrospective in
`docs/dev/milestones/M14-live-verification/README.md` § "M14 retrospective".
Run hands-off under `/rexymcp:auto` (first multi-blocker autonomous run).

**What needs the human (§ Calibration inventory in the retrospective):**

1. Fold: amend the PASTE MATCH recipe in WORKFLOW.md § E2E to the
   last-entry anchor (the first-fence form diffs a superseded round).
2. Fold: specs must not demand byte-exact pasting of non-round-trippable
   bytes — strip ANSI at generation, as part of the block.
3. Hold-or-fold (2 occurrences): make the FAIL→blocker duty a seeded task,
   not a prose ban — the src-diving stall cost two hard_fails despite the
   written rule.
4. Sign off / name the next milestone. Still carried: M6 — Verification &
   Hygiene (12 phases scoped 2026-07-30, never dispatched), the upstream
   push backlog (M13's four folds + M14's above), width-flip scrollback
   ghosts (cosmetic backlog).

**Next action:** PE decides the folds and the next milestone.

### Resolved by phase-04 (was: blocked on second live finding)

**Phase-02 round 2: the phase-03 fix is proven live (CHECK-G OK — deny path
prompts, denies, informs the AI), but CHECK-J surfaced live defect #2: the
per-tool cap enforces per *batch*, not per *turn*.** `tool_call_counts` is
declared inside the per-batch handler (`src/daemon/stream.rs:931`), so
sequential single-call batches never accumulate — with `list_panes = 1`
live, the AI called it twice in one turn, twice, unblocked (session JSONL
evidence in the phase-02 blocker entry). Comment, config doc and the cap's
error text all promise per-turn. Unit tests are green because they exercise
one batch. **Decision:** (a2) phase-04 hoists the counter to turn scope,
phase-02 re-runs; or (b2) bless per-batch and align the three doc surfaces.
Architect recommends (a2). Environment verified restored at takeover
(config byte-identical, daemon up, fixture gone, gates green).

Also carried: the executor's read-only src/-diving stall recurred (2nd
M14 occurrence, this time with no blocker entry written) — the
"impulse-to-diagnose" ban held it off in phase-01 round 2/3 but not here;
calibration item for close.

## Done: [M14 phase-03 — approval-state-persistence](milestones/M14-live-verification/phase-03-approval-state-persistence.md)

Drafted 2026-08-11 on the PE's option-(a) decision (inside the resumed
/rexymcp:auto run). Deletes the turn-end `SessionApproval::from_config`
reset (`stream.rs:653-657`, slipped in via `93fa228` untested) plus its
orphaned `Config` import (caught by scratch-applying the deletion — it would
have failed `-D warnings`), pins the session-start semantics in a
`from_config` doc comment, and live-checks that `/approvals revoke`
survives a completed turn (CHECK-P). Phase-02 re-runs after this lands.

## Blocked behind phase-03: [M14 phase-02 — approval-roundtrip-live](milestones/M14-live-verification/phase-02-approval-roundtrip-live.md)

**BLOCKED 2026-08-11 — live finding, PE decision needed.** The sweep ran
S1–S5; CHECK-G (deny-path) FAILed for a real reason: **runtime approval state
does not survive a turn.** `src/cli/commands/stream.rs:653-657` wholesale
resets `*approval = SessionApproval::from_config(...)` at the end of every
turn — verified verbatim by the architect. Consequences: `/approvals revoke`
(and `on`/`off`) lasts one turn; the `[A]pprove for session` answer is wiped
at turn end for any class whose config default is `false`. This also
explains the architect's prototype anomaly (post-revoke auto-approve). All
other verdicts passed (S1 cap-warning OK, F approve-path OK, H target-hint
OK, S5 config-restored OK — config byte-identical, daemon up, fixture gone);
S4 was unreached. Executor end state verified clean by the architect.
**Decision needed:** (a) phase-03 fixes the reset (merge config changes
without clobbering runtime revocations/session-approvals) and phase-02
re-runs after; or (b) accept-and-carry with phase-02 re-specced around it.
The architect recommends (a) — the reset also defeats the documented
`revoke always fully gates` contract. Calibration note for close: the
executor's diagnosis cited `stream.rs:656`, violating the phase's
no-reading-`src/` rule — the ban conflicted with writing a useful blocker;
reconcile the two at fold time rather than grading the deviation harshly.

Original drafting note: status was `todo`, drafted inside the /rexymcp:auto
run. Scripted
approval round trips (`/approvals revoke` → prompt → single-keypress `y`/`n`)
plus the per-tool cap and gated exemption against a temporarily capped
config (backed up, restored unconditionally in S5). Core mechanism
prototyped live by the architect before drafting — including two hazards now
pinned in the doc: chat hangs in a clientless tmux session, and prompt polls
must count `Approve?` occurrences (stale-scrollback greps false-positive).
7 verdict lines; the executor may not read `src/` at all.

**phase-01 — scripted-live-sweep approved 2026-08-11** (`approved_after_1`;
1 hard_fail + 1 bounce, 3 assists total — see below). All 9 live checks OK:
binary identity, four dispatch-path probes with status classification,
`/panes`, gates. Evidence anchored on chat-session JSONL `tool_results`.
Round 1 hard-failed on two architect spec defects (tmux numeric `-t`
ambiguity; `ask` never persists sessions). Round 2 completed but bounced on
bug-01-1 — retyped ANSI in the pasted S1 block; root cause shared (the spec
demanded byte-exact pasting of raw ANSI an LLM cannot round-trip). Round 3
stripped ANSI at generation and pasted byte-exact. **Executor flagged, not
gamed, a stale S6 anchor** (first-entry extraction diffing the superseded
round-2 entry) — second flag-don't-game data point after M13 phase-06.
**Calibration for milestone close:** the PASTE MATCH fold's first-fence
extraction does not survive multi-round phases; the last-entry anchor is the
correction (already applied to both M14 phase docs and validated both ways).

### Historical: phase-01 drafting notes

[M14 phase-01 — scripted-live-sweep](milestones/M14-live-verification/phase-01-scripted-live-sweep.md)

Drafted 2026-08-10, status `todo` — dispatch with `/rexymcp:dispatch
phase-01`. A shell-only evidence phase: rebuild + reinstall + restart the
daemon with a sha256 triple-identity proof (`target/release` = installed =
`/proc/<pid>/exe`), a two-session pane fixture (foreign marker, `date`-loop
`running` pane, `dead(7)` pane), four `ask --raw` dispatch-path probes
anchored on the session-JSONL `tool_results` records (one scripted retry
each), a send-keys-driven `/panes` check, teardown, gates. 9 verdict lines;
any `FAIL` is a blocker entry, never a re-run-until-green. Key derived fact
that shaped it: `ask --raw` auto-denies *prompts only* — silent tools
execute for real, so the sweep needs no human. The send-keys chat technique
in S4 doubles as the prototype for phase-02's open question.

**M14 — Live Verification scoped 2026-08-10 on PE direction** at the M13
boundary: live-verify the M12 exit criteria that shipped unit-only, against
a daemon restarted onto the current binary. Two phases planned: 01
scripted-live-sweep, 02 approval-roundtrip-live (human-in-the-loop).
Deliberately small: it verifies, it does not build.

**M13 — Chat UX Polish closed 2026-08-10** (PE sign-off same day). Seven
phases, all `done`: four `approved_first_try` (01, 02, 05, 06), two
`approved_after_1` (04, 07), one `escalated` (03). Two bugs, both resolved.
Four gates re-run green at close: 1241 lib tests. Retrospective in
`docs/dev/milestones/M13-chat-ux/README.md` § "M13 retrospective".

### Four folds landed at M13 close (PE sign-off, 2026-08-10)

All four in `docs/dev/WORKFLOW.md`; **none applied upstream** — added to the
push backlog:

1. **§ "Every acceptance criterion must be satisfiable"** gains *"validate
   every mechanical criterion against the tree the phase will produce — by
   executing it, not reasoning about it"*, folded at five occurrences
   (M12 ×2, M13 ×3; reasoning went 0-for-5).
2. **§ "End-to-end verification"** gains the pinned **PASTE MATCH recipe**:
   extraction anchored to the entry heading and scoped to its first fence,
   the literal verdict line required in the entry, and the check validated
   against a known-bad input before speccing.
3. **§ "End-to-end verification"** now requires **`${PIPESTATUS[0]}` exit
   markers** on piped commands — the `$?` form recorded tail's exit and
   green-washed every pre-phase-04 M13 block.
4. **§ "Derive every spec fact from its source"** gains *"prescribed code
   must pass the project's lint gate, not just the compiler"* (folded on PE
   direction at first occurrence — phase-03's worked example failed
   `-D warnings`).

Backlog candidate only, not milestone-shaped: width-flip scrollback ghosts
(cosmetic; fix shape recorded in the RESOLVED block below). Also still
carried: M6 — Verification & Hygiene (scoped 2026-07-30, 12 phases, phase-01
drafted but never dispatched) and the upstream push backlog (~14 local-only
WORKFLOW.md sections plus the four folds above).

---

## M13 history (most recent first)

## OPEN FINDING (2026-08-10, post-boundary): window-switch artifacts persist

Live check after M13 phase-05 (binary 19:13, includes the fix): rendering
artifacts on tmux window switch remain. Diagnosis (derived from
ratatui-core-0.1.2 source, `terminal/resize.rs` + `terminal/inline.rs`):

`reanchor()` = same-size `Terminal::resize`, which (a) full-clears the screen
**only on horizontal shrink** — on same-size/grow, stale rows outside the
recomputed viewport are never cleared; (b) anchors the viewport via a **DSR
cursor query** (`ESC[6n`) relative to wherever tmux's rewrap left the cursor,
minus a stale internal offset — nothing pins it to the bottom, so a high
cursor re-anchors the viewport high (the "input dialog at top" symptom); and
(c) the DSR reply is read from the same tty our `AsyncStdin` reader polls —
a lost/garbled reply makes `resize` error out and `let _ =` swallows it.
Phase-05's real contribution stands (the signals now *arrive* mid-stream);
the repin they trigger is pre-M13 code that was never live-validated.

Proposed phase-06 (resize-repin-rebuild): replace the resize-based repin with
a deterministic bottom repin — clear the viewport rows, move the real cursor
to row `height − VIEWPORT_ROWS`, rebuild the `Terminal` (fresh
`Viewport::Inline`; init anchors at the cursor with offset 0, verified at
`terminal/init.rs:130`), redraw. Scrollback rewrap damage above the viewport
is tmux's and stays out of reach (existing non-goal). Drafted as
[phase-06 — repin-rebuild](milestones/M13-chat-ux/phase-06-repin-rebuild.md)
on PE direction 2026-08-10.

## OPEN FINDING 2 (2026-08-10, post-phase-06 live check): stale live-region debris survives the repin

Screenshot evidence (same-size window switch): the input box now pins to the
bottom correctly — phase-06's mechanism works — but the region between the
end of committed history and the repinned viewport shows fragments of at
least two *previous* live-region generations (top-border rules, orphaned `│`
border cells, a partial box).

Why phase-06 misses them: `reanchor()` clears from
`min(old_viewport_top, park)` downward. When the viewport is already at (or
near) the bottom, that wipes only the bottom rows — any debris the session
accumulated *above* the old viewport top is out of range, and it is real
grid content to tmux, faithfully restored on every switch. The clear range
is too narrow; the repin itself is sound.

Proposed phase-07 (content-extent clear): the renderer can know exactly
where real content ends — every committed row goes through
`insert_before` (banner, panels, streamed lines all use the `commit*`
methods). Track `origin_row` (initial viewport top, captured at `new()`)
plus a saturating count of inserted rows; `content_end = min(origin_row +
inserted_rows, park)`. Reanchor then clears from
`min(content_end, old_top, park)` — wiping all debris between true
history-end and the bottom while never touching a real history row. Once a
session has scrolled a full screen, `content_end` saturates at `park` and
behavior degrades to today's (correct there — everything above is genuinely
scrolled history). Drafted as
[phase-07 — content-extent-clear](milestones/M13-chat-ux/phase-07-content-extent-clear.md)
on PE direction 2026-08-10, including the env-gated `DAEMONEYE_REANCHOR_TRACE`
diagnostic.
Root cause of *how* the debris rows were originally painted is still
unproven (suspect: earlier-generation live regions vacated by
viewport-bottom migration or by pre-phase-06 resize-based reanchors); the
content-extent clear removes them regardless of origin.

## RESOLVED (2026-08-10, live-verified): window-switch artifacts — closed by PE

Live check after phases 06+07 (fresh session, `DAEMONEYE_REANCHOR_TRACE=1`):
repeated window switches produce **no visible artifacts** — PE confirmed and
closed the issue. Closure evidence:

- Trace: 5 reanchors fired, e.g. `old_top=55 content_end=129 park=55 w=127
  h=61` — the repin + content-extent clear engaging as designed.
- **The width-flip ghost generator is real and was caught on tape**: one
  trace line reads `w=255` — the 127-col pane transiently became full-width
  during window rearrangement (no user zoom involved), and exactly one
  wrapped ghost border was planted into *scrollback* at that moment (final
  capture, one `┌…` row wider than the pane, no closing corner). The visible
  screen stayed clean throughout. This matches the reproduction harness
  result (scratchpad `repro/`): transient pane-width changes make tmux
  rewrap live-region rows into history as permanent ghosts; nothing
  app-side can clean scrollback after the fact.
- Residual (cosmetic, backlog candidate only): scrollback ghosts on width
  flips. If ever worth fixing: a width-change-aware clear band in
  `reanchor` (track last-drawn size; the old live region's rewrapped rows
  occupy `VIEWPORT_ROWS × old_w / new_w` rows above the bottom and are
  guaranteed non-history). Not scoped into M13.

**phase-07 — content-extent-clear approved 2026-08-10** (`approved_after_1`;
bug-phase-07-1 minor — a stale duplicate `repin_rows` doc comment left in
place, root cause part architect-side: "Replace the two-arg form" left the
patch anchor ambiguous. Round 2 was exactly the enumerated 8-line deletion, 21
turns, zero code drift; commit `55ba03f`). The renderer tracks `origin_row` +
a saturating `inserted_rows` counter; `repin_rows` is three-arg
(`min(old_top, content_end, park)`); env-gated `DAEMONEYE_REANCHOR_TRACE`
logs per-repin. Round 1 produced the milestone's first fully clean E2E
artifact (byte-exact PASTE MATCH, literal self-check line present). Live
verification closed the window-switch issue — see the RESOLVED block above.

**Post-close color-path fixes (2026-08-10, direct commits, live-verified):**
`ca4b8e9` caps chat colors at 256 when tmux cannot pass truecolor through;
`c373997` probes the tmux client's terminfo (`infocmp -x`) for
`Tc`/`RGB`/`setrgbf`/`setrgbb` because `client_termfeatures` under-detects
terminfo-declared truecolor (xterm-ghostty). Tests 1234 → 1241.

**phase-06 — repin-rebuild approved 2026-08-10** (`approved_first_try`,
commits `184e10b`/`5a78bcd`, 72 executor turns). Deterministic bottom repin
landed; live check confirmed the input dialog pins to the bottom, then
surfaced OPEN FINDING 2 (stale live-region debris above the clear range) —
phase-07 is the fix. Review calibration: another architect criterion defect
(`fn repin_rows` grep matched the test-name prefixes; executor flagged it) —
third M13 occurrence, at fold threshold.

**phase-05 — resize-and-reanchor approved 2026-08-10** (`approved_first_try`,
commit `f4b0e4b`). SIGWINCH + focus events now reach the streaming loop
(`StreamOutcome::Reanchor`), idle resize re-anchors, 147 lines of dead legacy
renderer deleted; 1228 lib tests. Live check then showed the artifacts
persist — see the OPEN FINDING above; phase-06 is the fix.

**phase-04 — cursor-alignment approved 2026-08-10** (`approved_after_1`;
bug-phase-04-1 blocker — the pasted E2E entry's `filtered out` counts were
retyped `1224`→`0`; round 2 regenerated the evidence with a PASTE MATCH
self-check in 38 turns, zero source edits; commits `987d999`/`b4e4808`).
`visual_lines` is now the single wrapping authority, the border clamp is
fixed, Up/Down wrap against `renderer.input_content_width()`; exact-coordinate
cursor tests via `TestBackend::get_cursor_position`; 1225 lib tests. One
justified executor deviation: the spec's wrap fixture didn't discriminate
mutation M2 (wrapped identically at 58 and 59); the executor substituted one
that does — architect fixture defect, executor caught it.

**phase-03 — runtime-in-border done 2026-08-10** (`escalated` — 2
NoProgressStall hard-fails, 1 resume assist, architect takeover close-out;
commit `74b33f3`). Silent-tool pair merged into one panel with `✓ 1.2s` in the
bottom border; panel bodies word-wrap via `wrap_line_hard`; user echo carries
`turn N · tokens` again; 1221 lib tests. **Four calibration items for
milestone close:** (1) an unsatisfiable acceptance criterion — the
`commit_panel("result"` grep demanded 0 but the out-of-scope interrupt-path
panel at `stream.rs:192` always matches (M12's criterion-vs-own-spec family,
now recurring — fold candidate); (2) the spec's own worked example failed the
lint gate (`&[label.clone()]` → `cloned_ref_to_slice_refs`) — prescribed code
must be lint-checked, not just compiled; (3) every M13 E2E block's
`cmd | tail; echo exit=$?` recorded the pipe's exit, not the command's —
template defect, fixed from phase-04 on; (4) retyped-transcript whitespace
divergence recurred (3 cosmetic lines, repaired at takeover).

**phase-02 — throbber-and-identity approved 2026-08-09** (`approved_first_try`,
commit `7bd1af2`). Throbber flush at column 0; history panels titled
`user@shorthost` via pure `user_host_label` + `daemon_hostname()`; both
mutation pairs re-run independently at review; 1215 lib tests.

**phase-01 — color-depth-palette approved 2026-08-09** (`approved_first_try`,
141 executor turns, commit `0eae4c7`). `src/cli/palette.rs` landed with pure
depth detection, the depth-aware `Palette`, `sgr_fg`, the `apply_sgr` `38;2`
arm, and all truecolor sites swapped; 1211 lib tests; mutation pair re-run
independently at review.

**M13 — Chat UX Polish scoped 2026-08-09.** Five phases planned; README at
`docs/dev/milestones/M13-chat-ux/README.md`, which carries the full derived
code-fact inventory (file:line, investigated 2026-08-09). Headline: chat colors
survive non-truecolor terminals (the pinky monotone-red symptom — every color
site is an unconditional `Color::Rgb` with zero capability detection), cursor
alignment (two disagreeing wrappers), flush-left throbber, `user@host` history
attribution, runtime embedded in the output panel's bottom border, and
mid-stream resize/window-switch re-anchoring.

**Next action:** `/rexymcp:architect next` to draft phase-01
(color-depth-palette).

---

**M12 — Full-View tmux Integration is closed** (2026-08-08). Retrospective in
`docs/dev/milestones/M12-tmux-integration/README.md` § "M12 retrospective".
Nine phases, six bugs, all resolved; 1200 tests, four gates green, 36 tools.

### Folded at close (PE sign-off, 2026-08-08)

Both into `docs/dev/WORKFLOW.md`, both at two occurrences:

1. **§ "Give the executor a condition it can check, not an instruction it can
   agree with"** — new section. When a requirement keeps going unmet, change
   what the executor can evaluate, not how you phrased it: make it a seeded
   `## Spec` task, or give it a self-check with a falsifiable output, and run
   that check against a known-bad input before speccing it.
2. **§ "A pasted transcript is a claim, not evidence"** gains a paragraph on
   completion summaries — the "deviations" line has been wrong in both
   directions (an undeclared change reported as none; a removal reported that
   never happened). Read the diff, not the narrative.

Three earlier folds landed mid-milestone, also with sign-off: the E2E capture
as a `## Spec` task, mutation pairs as `patch` tasks rather than the banned
`sed -i`, and a reinforcement of the satisfiable-criterion rule.

### Carried out of M12 — open items for whoever scopes next

1. **Upstream push — a large accumulated backlog, not the correctness fix it
   was first called.** Checked at close: the plugin template does **not**
   contain `sed -i` anywhere. Its § "End-to-end verification" is the bare
   11-line original and never received the mutation-pair guidance at all, so
   the template misinstructs nobody. The earlier "correctness fix affecting
   every repo" framing was wrong and is corrected here.

   What is true is that the template is ~760 lines behind (848 vs 1611) with
   **14 local-only sections**, of which M12's seven folds are a part. The real
   risk is absence rather than error: an architect given no mutation-pair
   guidance invents some, and `sed -i` is the natural reflex — which is exactly
   how this repo acquired it. Proposal written up for the rexyMCP side at
   `~/src/rexyMCP/docs/daemoneye-proposed-upstream-folds.md`.

   Drift is bidirectional and was checked both ways: upstream-only headings are
   `## How to fix` and `## Verification`, both **deliberately superseded** —
   do not pull them back.
2. **M12's live-verification gap.** Three exit criteria ask for verification
   against live tmux / the dispatch path / the full approval round trip, and
   all three are covered at unit level only — see the retrospective's § "Exit
   criteria" for exactly which. The check needs the daemon restarted onto the
   M12 binary. Small, and worth doing before anything builds on these surfaces.
3. **`APPROVAL_GATED_TOOLS` reconciliation shipped a behaviour change** —
   `spawn_ghost_shell` and `delete_schedule` became cappable, `create_agent`
   and `delete_agent` became exempt. Correct per the exemption's own rationale
   and pinned by tests, but it has not run against a live daemon either.
4. **`tests/doc_truth.rs` now gates the README** (tool tables, counts, and the
   `⚠` markers) as well as `CLAUDE.md`. The add-a-tool checklist grew steps 11
   and 12 to match.

---|---|
| 03 r2 | executor restored a mutation with `patch`, reported it as a deviation |
| 05 r1 | executor substituted `patch` for both `sed -i` pairs |
| 05 r2 | same substitution; the block's `== M1 APPLY ==` / `== M*  RESTORED ==` markers are missing from the transcript because the marker `echo`s live between the banned commands |

Each was graded on substance and not bounced, but the specs were asking for
something forbidden, and the architect kept writing it because WORKFLOW.md says
to. Reviews have had to re-run the mutations themselves every time — which is
the standing rule anyway, but it means the *executor's* mutation evidence has
been procedurally non-compliant by construction since phase-03.

**What needs the human:** decide how mutation pairs should be expressed, then
fold it into `docs/dev/WORKFLOW.md` § "End-to-end verification" (and push it
upstream — the same wording is in the plugin template). Three options, in the
architect's order of preference:

1. **Express mutations as `patch`-tool tasks in `## Spec`**, with the marker
   `echo`s as separate shell steps around them. Keeps executor-side mutation
   evidence; costs two extra tracked tasks per pair.
2. **Move mutation pairs out of the executor's block entirely** and make them
   the *reviewer's* job, which is where they are independently re-run today
   regardless. Simplest, and honest about who actually produces that evidence —
   but it drops the executor's own mutation self-check, which the
   green-bounce treatment leans on.
3. **Keep `sed -i` in the block but scope it to files outside `src/`** — does
   not work here; every mutation target is a source file.

A second, smaller architect-side item to fold at the same time: the phase-05
Task-3 paste-fidelity check (`grep -n '…verification)' | tail -1`) is fragile —
the server-authored `(complete)` entry contains that substring in its prose and
wins the `tail -1`, so re-running the check after the run reports a false
`PASTE MISMATCH`. Scope the extraction to the heading pattern (`^### Update`)
rather than a bare substring.

**Next action:** resolve the blocker with PE sign-off, then
`/rexymcp:architect next` to draft phase-06 (tmux-control-tool, D5 — the
milestone's highest-risk phase, and the one the README flags as a possible
a/b split).

---

### [phase-07 — pane-inspector-cli](milestones/M12-tmux-integration/phase-07-pane-inspector-cli.md) approved 2026-08-08

`approved_first_try`. `Response::PaneList` now carries a named `PaneInfo`
instead of a 5-tuple, and `/panes` renders a window-grouped inspector with cwd,
status, activity age and a preview line. 1195 tests. The phase's trap held:
`/pane <n>` still resolves `panes[n - 1]`, and the inspector numbers rows
globally across window sections rather than per-section.

**An architect-side spec defect the executor absorbed.** Task 1 said "add a
named struct `PaneInfo`" — but one **already existed** at `src/ipc.rs:6`,
backing `Response::PaneSelectPrompt`. The spec was derived from lines 500–512
of that file and never from the top of it, which is § "Derive every spec fact
from its source" failing in a new way: the fact I checked was true, and the
fact I did not check made it a name collision.

The executor unified the two types rather than filing a blocker, which pulled
`src/daemon/executor/mod.rs` (+103/−20) into a phase whose § Out of scope named
that directory explicitly. Verified at review: the diff is entirely mechanical
(one production call site plus three test fixtures filling the widened struct),
`PaneSelectPrompt` still populates and renders every field it needs, and the
unified type is the better design. Approved as a justified deviation forced by
the spec defect. **The lesson is mine, not the executor's:** when a spec says
"add a type", grep the target file for that name first.

### [phase-06b — tmux-control-actions](milestones/M12-tmux-integration/phase-06b-tmux-control-actions.md) approved 2026-08-08

`approved_first_try` — the first of M12, and the first phase since 01 to clear
review without a bounce. `tmux_control` now carries all six D5 actions;
`kill_window` refuses daemon-managed windows and the chat pane's window, both
**before** prompting, via a pure `kill_window_refusal` predicate. 1191 tests.

Worth noting against 06a's two hard-fails on the same tool: 06b was the *same
executor* on the *same file*, and the difference was that the risky design work
had already been done. The a/b split is what turned an unrunnable phase into a
mechanical one.

One declared deviation, not bounced: the executor relabelled the E2E block's
restore lines (`M1 restored-lines-absent=0` for the spec's
`M1 restored (want 0)=`), so the block was not run byte-for-byte verbatim. The
substance — applied-check, `FAILED` run, restore, restored-passing run — was
all present and independently re-run at review.

### [phase-06a — tmux-control-gate](milestones/M12-tmux-integration/phase-06a-tmux-control-gate.md) done 2026-08-08 — **architect takeover**

`escalated`. Two `NoProgressStall` hard-fails, then a takeover. `tmux_control`
ships with `focus` / `zoom` / `unzoom`, `APPROVAL_GATED` in both lists, and the
D5 ghost denial. 36 tools: 27 core + 9 deferred. 1188 tests, four gates green,
both mutation pairs verified in each direction.

**The finding that justified splitting 06 a/b, and it was found by reading the
helper rather than reasoning about it:** `prompt_and_await_approval`
short-circuits for ghosts on `GhostPolicy::is_safe`, which returns `true` for
*any non-sudo string*. Routing `tmux_control` through it unchanged would have
inverted D5's default-deny for every ghost shell — silently, with every gate
green. The arm now gates before the helper and passes it `ghost_policy: None`.

**Worked examples move this executor; prose does not.** Round 1 guessed five API
signatures and broke the build (`prompt_and_await_approval` with 8 args against
5, `.iter()` on a `RwLock`, `off_runtime` with one arg, a nonexistent
`ToolCallOutcome` variant, `crate::tmux::pane::*` instead of the re-export).
Round 2, given the whole arm as a worked example plus a table of the five facts
with file:line sources, reproduced it exactly — that code shipped unmodified.

**The read-only stall is this executor's signature failure and recurs within a
phase.** Both rounds ended identically: a patch lands, then ~60 consecutive
searches of the same file with no edit. Round 2's spec carried an explicit
Notes-for-executor warning naming that exact pathology and it did not help.
Third data point behind "a recurring stall is a takeover signal".

### [phase-05 — list-panes-upgrade](milestones/M12-tmux-integration/phase-05-list-panes-upgrade.md) approved 2026-08-08

`approved_after_1`; one bounce ([bug-05-1](milestones/M12-tmux-integration/bugs/bug-05-1.md)),
verified fixed. D4's display half landed in full: `list_panes` groups by window
with `status:` and a `[daemon]` tag and appends a foreign-session section;
`get_terminal_context` takes `scope: "window" | "session" | "all"`. 1182 tests.

The additive design held exactly as specced — `get_labeled_context` kept its
two-arg signature and became a delegator to `get_labeled_context_scoped`, so
`src/tmux/cache_tests.rs` came out **209 additions / 0 deletions**: not one of
its ~15 existing call sites needed touching.

**Two architect-side defects this phase, both worth carrying:**

1. **An unsatisfiable acceptance criterion.** It demanded
   `src/tmux/cache_tests.rs` show *no changes* while § Test plan required this
   phase's new cache tests to live in that file. Caught by the dispatcher and
   amended before review to "0 deletions", which was always the intent. Same
   family as the vacuous-guard folds; the discriminator is that a criterion
   must be checked against the *rest of its own spec*, not just against the
   tree. **Second occurrence in M12** (phase-01's self-satisfying grep was the
   first) — at the trend threshold.
2. **The `sed -i` contradiction** — see the blocker above.

**The retyped-transcript failure recurred**, so it is now at two consecutive
occurrences (phase-04 r2, phase-05 r1) and compactness alone is not the fix:
phase-05's round-1 artifact was already only 56 lines and was still
reconstructed from memory rather than read from the file. What worked was
giving the executor a **self-checkable** finish condition — a command that
extracts its own pasted block and diffs it against the artifact, printing
`PASTE MATCH` / `PASTE MISMATCH`. Verified against round 1's entry before being
specced, where it correctly printed `PASTE MISMATCH` with exactly the seven
divergent lines. Round 2 came back byte-identical on the first try. **Fold
candidate at the next milestone boundary**, with the fragility fix noted in the
blocker.

### [phase-04 — find-in-panes-tool](milestones/M12-tmux-integration/phase-04-find-in-panes-tool.md) approved 2026-08-08

`approved_after_2`; two bounces, both verified fixed. Round 1 shipped the whole
tool in 177 turns; round 2 fixed [bug-04-1](milestones/M12-tmux-integration/bugs/bug-04-1.md)
(neither `home_rows` nor `foreign_rows` was sorted, though the spec required it
twice — `HashMap` order, so both the output order and *which* 20 foreign panes
the cap selected were nondeterministic); round 3 fixed
[bug-04-2](milestones/M12-tmux-integration/bugs/bug-04-2.md), a paraphrased
end-to-end transcript.

**bug-04-2 was an architect-side defect, and it is the calibration item worth
carrying.** The round-2 E2E block redirected full `cargo` output and produced a
**2,555-line** artifact. Pasting that into a phase doc is impossible, so the
paraphrase was the only way out — the spec had made compliance unavailable. The
round-3 block pipes each command through `tail`/`grep`, lands at **38 lines**,
and ends with its own `wc -l` so a paraphrase is mechanically detectable; the
executor pasted it byte-for-byte on the first try (`diff` against
`/tmp/e2e-04-r3.txt` was empty at review). **First occurrence — hold for
recurrence**, but the phase-05 block is already written in the round-3 shape.

Also unresolved and worth watching: the dispatch-time warning *"Phase doc has
no parseable `## Acceptance criteria` section"* fired on all three rounds. The
first theory — an H1 heading the architect had put inside § Spec — was
falsified when the warning persisted in round 3 after that heading was demoted
to H3. Cause still unidentified; it has had no observed effect on execution.

### [phase-03 — read-pane-tool](milestones/M12-tmux-integration/phase-03-read-pane-tool.md) approved 2026-08-08

`approved_after_1`; one bounce ([bug-03-1](milestones/M12-tmux-integration/bugs/bug-03-1.md)),
verified fixed. Round 2 did exactly its two tasks — reverted the undeclared
`await_agent_result` `summary()` edit and captured the E2E transcript — in 62
turns. Re-verified at review: four gates green, **1163** tests (not 1164, the
inverted finish condition held), and both mutation pairs re-run by the
architect in both directions.

**The E2E-as-a-Spec-task remedy worked on its first outing.** Phase-03 round 2
is the second round in M12 where the capture was an enumerated `## Spec` task,
and the second that produced the entry. The fold's prediction held.

### The E2E fold was wrong, and this phase proves it

Phase-03's E2E block carried the 2026-08-08 fold in full — every mutation a
command, no manual steps — and the entry is *still* missing. Block runnability
was never the mechanism. **Derived from the rexyMCP source:** the executor's
task list is seeded from a heading matching exactly `## Spec`
(`executor/src/agent/tasks.rs:52-55`), so a requirement in
`## End-to-end verification` is never tracked. All four data points fit — the
one round that produced an entry (phase-02 r2) is the one round where it was an
enumerated task. **Remedy applied here: the E2E capture is now Task 10 inside
`## Spec`.**

**FOLDED 2026-08-08 (PE sign-off)** into `docs/dev/WORKFLOW.md`, in two places:
§ "End-to-end verification" gains "The capture must be the phase's last
numbered task, in `## Spec`", and the phase-doc template's `## Spec` section
gains "Only `## Spec` is seeded — so anything the phase must deliver has to be
a task here", plus a template task line. The earlier 2026-08-08 fold was
**amended in place**, marked *superseded in part*: its craft advice stands, its
causal claim was disproven. **Not applied upstream** — see the push backlog.

Also recorded: round 1 rewrote `await_agent_result`'s `summary()` — a different
tool's user-visible text — while reporting "Deviations from spec: None". First
false deviation report in M12; hold for recurrence.

---

### Original drafting notes (round 1)

Phase-03 adds the `read_pane` core AI tool (D3) — the milestone's
highest-leverage addition: any pane's buffer on demand, any session, at a
requested scrollback depth, ANSI-annotated, optionally regex-filtered, masked,
labelled with window / session / `PaneStatus`. Chat pane refused; daemon-owned
windows allowed; not approval-gated. Full add-a-tool checklist, so the tool
counts go to **34 tools: 25 core + 9 deferred** and `tests/doc_truth.rs` gates
it.

Drafted prototype-first, and the prototype earned its keep three times:

1. **`annotate_ansi` is unreachable from `src/daemon/`** — it is `pub(super)`
   inside a private `mod ansi`. Settled in the spec by putting a new
   `capture_pane_annotated` on the `src/tmux/` side of the boundary rather
   than widening visibility; compiled to confirm.
2. **The compiler found a bug in my draft** — a five-placeholder format string
   with four arguments. Fixed before it reached the spec.
3. **A hermeticity defect in the obvious test fixture.** `read_pane`'s capture
   path shells out to the *real* tmux server: with the chat-pane guard deleted,
   the prototype test captured a live pane on this machine and returned 261
   lines of an in-progress cargo build. Worse, it made the mutation's outcome
   environment-dependent — on a box with no tmux the weak `starts_with("Error:")`
   assertion passes vacuously. The spec now pins an unseeded `%999999` fixture
   (never reaches tmux in either mutation direction) and requires asserting the
   distinctive `"chat pane"` substring.

*(Historical — round 2 was dispatched and approved; see the phase-03 record
above.)*

**[phase-02 — pane-status-classification](milestones/M12-tmux-integration/phase-02-pane-status-classification.md)
approved 2026-08-08** (`approved_after_1`; one bounce,
[bug-02-1](milestones/M12-tmux-integration/bugs/bug-02-1.md), verified fixed).
D2 landed in full: `src/tmux/status.rs` carries the `PaneStatus` enum, a pure
`classify()` (Dead > Bell > shell/non-shell × activity age), `format_age`,
`Display`, and the new `summarize()`; `PaneState.status` is stamped every 2 s
refresh; the old heuristic `SessionCache::summarize()` and its 8 tests are
gone; `is_shell_prompt` moved to the new module behind a `pub(super) use`
re-export that left all seven call sites unchanged. 1158 tests pass.

Phases 03–07 render this status on their new surfaces — nothing displays
`status:` yet, which is why the milestone's "Status classification is live"
exit criterion is verified at 05/07, not here.

**Next action:** `/rexymcp:architect next` to draft phase-03 (read-pane-tool).

### Three calibration items carried out of phase-02

1. **The missing-E2E-entry bounce is at two consecutive occurrences** (phase-01,
   phase-02) — a trend worth folding. The sharp point: phase-02's spec
   **already carried both M6 countermeasures** (a literal copy-pasteable block,
   and an explicit "the server-authored `(complete)` entry does not count") and
   bounced anyway. Both remedies are necessary, neither is sufficient.
2. **FOLDED 2026-08-08 (PE sign-off) — everything the E2E entry must contain
   has to be produced by the E2E block.** Landed in `docs/dev/WORKFLOW.md`
   § "End-to-end verification". **Not applied upstream** — add it to the push
   backlog below. Sharpened at fold time by re-deriving both specs rather than
   trusting the review summary, which corrected the first reading: phase-01's
   block *was* fully runnable, so "a manual step broke it" does not explain
   both bounces. The unified cause is that in both phases the missing evidence
   was **specifically the mutation-pair transcript**, and in both it was the
   one artifact the block did not generate — phase-01 promised "the gate run
   plus the mutation pairs" and then ran only the gates; phase-02 included them
   but broke one with a manual step. Mutation pairs are now required to be
   commands in the block, with labelled markers, run by the architect before
   being specced.
3. **The taxonomy gap is unchanged and now hit twice.** `rexymcp review`
   again warned that `missing_e2e_verification` is not a known failure class;
   the nearest, `false_completion`, is defined as self-reporting complete on a
   *red* gate, and this is green gates plus correct code with the evidence
   artifact missing. A **rexyMCP-repo** change, out of bounds here.

**M12 — Full-View tmux Integration scoped 2026-08-07** (PE decision). Eight
phases planned; settled design (D1–D7) in `docs/design/tmux-integration.md`;
milestone README at `docs/dev/milestones/M12-tmux-integration/README.md`.
Headline: multi-session pane cache, `PaneStatus` classification, `read_pane` /
`find_in_panes` / `tmux_control` tools, `/panes` inspector, one shared
targetable-panes filter.

**[phase-01 — multi-session-cache](milestones/M12-tmux-integration/phase-01-multi-session-cache.md)
approved 2026-08-08** (`approved_after_1`; one bounce,
[bug-01-1](milestones/M12-tmux-integration/bugs/bug-01-1.md), verified fixed).
`SessionCache` now retains foreign-session panes as metadata-only, all five
iteration surfaces and four target-validation sites filter to the home session
via `is_home_pane`, and closed panes are evicted by a guarded `evict_missing`.
Behavior at every existing surface is unchanged, as intended — the foreign
panes this phase admits are exposed deliberately by phases 03–05.

**Next action:** `/rexymcp:architect next` to draft phase-02
(pane-status-classification).

### Two calibration items carried out of phase-01

1. **Lock ordering across the five filter sites is inconsistent** — three clone
   `session_name` before taking `panes`, three hold `panes` while taking it. No
   deadlock is possible today (verified at review: every `session_name` guard is
   a statement-temporary, so no cycle exists). **Not bounced** — Task 4 pinned
   the ordering for `is_home_pane` and the executor followed it exactly; Task 5
   never pinned it, so this is an architect spec gap. **Phase 08's spec must pin
   session-before-panes ordering at every site it touches.**
2. **A bounce criterion that quotes its own search string is vacuous.** The
   first draft of the round-2 criteria told the executor to grep the phase doc
   for `'<test> ... FAILED'` — which matched the criterion text itself and
   returned 1 before any work existed. Caught by *running* each criterion at
   bounce time (step 3 of the four-step bounce sequence) rather than reasoning
   about it; fixed by scoping every check to the Update Log section. Same family
   as the folded vacuous-guard rules but a new instance — the *criterion* is
   self-satisfying, not the fixture. First occurrence; hold for recurrence.

**The green-bounce treatment worked again.** Round 1 shipped correct code and
all criteria passed; the only defect was the missing end-to-end verification
entry. Applying the full treatment before re-dispatch (loud header that green
gates are not evidence, do-not-touch list on every `src/` file, one enumerated
task, inverted finish condition "1153, **not** 1154") landed it in 37 turns
with zero source files touched. Second local confirmation of that fold.

**The taxonomy gap is now visible from this repo.** The bounce was recorded as
`missing_e2e_verification`, which `rexymcp review` warned is not a known class.
The nearest existing class, `false_completion`, is defined as self-reporting
complete on a *red* gate; this was green gates plus correct code with the
evidence artifact missing. Same gap NEXT.md already tracks for fabricated
evidence under green gates — a **rexyMCP-repo** change, out of bounds here.

---

**M11 — Unified Knowledge Index closed 2026-08-07.** Twelve phases, all `done`;
all nine exit criteria verified at close against source. Verdicts: 1
`approved_first_try`, 6 `approved_after_1`, 5 `escalated`. Nine bug docs, all
resolved. Retrospective in
`docs/dev/milestones/M11-knowledge-index/README.md`.

### Two folds landed 2026-08-07 (PE sign-off)

Both are in `docs/dev/WORKFLOW.md`. **Neither is applied upstream.**

1. **The bounce sequence is now four numbered steps**, in
   § "Review and Bug-Report Cycle" — write the bug, flip the status, **refresh
   the acceptance criteria and confirm each fails against the current tree**,
   update the README and telemetry. The criteria-refresh requirement previously
   existed as a parenthetical inside a compound "rejects" step and was skipped by
   the architect who had folded it in the day before; a rule that must be
   remembered when writing a bug report has a failure rate, a rule that is step 3
   of 4 gets checked off. Second occurrence of the empty-diff failure (07a r2,
   07b r2); first occurrence of the original fold failing to be applied.
2. **A guard's premise must be demonstrated, not described** — appended to
   § "Coverage claims are inadmissible without mutation proof". Distinct from the
   three failures already there, which are about what the *assertion* can see;
   this is about the *fixture* being inert rather than near-miss, so the guard is
   never reached. Requires a both-directions mutation pair in the phase doc, and
   explicitly rejects an intent-stating comment as a substitute. Third occurrence
   of the vacuous-guard family (03a, 05b, 07b).

**Note on fold 1's scope.** The proposal was to make it a step in the *review
skill's* §8 sequence. That file lives in the rexyMCP repo and is out of bounds
here, so what landed is the in-repo half: `WORKFLOW.md` now carries the ordered
sequence. Mirroring it into the skill remains an upstream change, and until it
lands the skill's §8 and this file disagree on structure.

### Four upstream folds pulled in 2026-08-07

The sync gap was **bidirectional** — this repo was behind upstream on four
sections, discovered while inventorying what to push. All four are now in
`docs/dev/WORKFLOW.md`:

- **`Derive every spec fact from its source`** — folded upstream 2026-07-24
  after **ten** occurrences. This is the one that stings: M11's single largest
  failure class was architect-authored spec facts asserted rather than executed,
  and we re-derived that lesson across four bug docs and several dispatches
  while the fold sat in the template, unsynced.
- **`Governing a running phase`** — the `stop_phase` discipline and the
  don't-babysit-with-a-poll-loop rule.
- **`Pin the fixture that makes the row appear`**.
- **`Pre-inject compiler-error-driven recovery on oscillation-prone files`**.

**Reconciled rather than concatenated:** `Pin the fixture` and this milestone's
own `A guard's premise must be demonstrated` are two halves of one failure — a
fixture that does not exercise the path the test names. Inert fixture -> vacuous
pass; empty fixture -> spurious failure and a phantom bug hunt. Both now carry a
cross-reference and a shared draft-time question ("what in this fixture makes
the code take the branch I am asserting on?"). That synthesis is itself a
candidate to push upstream.

The only remaining divergence is `## How to fix` / `## Verification`, which the
2026-08-06 bug-report fold deliberately replaced with `## Root cause` +
`## Definition of done`. Do **not** pull those back.

### Still out of bounds from this repo

**Push direction, still outstanding.** ~503 lines across 12 local-only
`WORKFLOW.md` sections, ~13 lines of local-only `STANDARDS.md` DoD boxes (the
mechanical-capture and own-entry E2E requirements — exactly what phase-07b
violated three dispatches running), the bug-report template's breaking
`How to fix` -> `Root cause` + `Definition of done` change, and the
`false_completion` taxonomy gap in `executor/src/store/telemetry.rs:341-346`.
Plus mirroring the four-step bounce sequence into the review skill's §8. All are
**rexyMCP-repo** edits; a target-project architect session cannot make them.

**Added to the push backlog 2026-08-08:** the M12 E2E folds — *"everything the
E2E entry must contain has to be produced by the E2E block"* and, more
importantly, ***"the capture must be the phase's last numbered task, in
`## Spec`"*** plus the template's *"only `## Spec` is seeded"* clause. Push the
second one first: it is the only remedy with a demonstrated mechanism
(rexyMCP `executor/src/agent/tasks.rs` seeds from a heading matching exactly
`## Spec`), it explains all four M12 data points, and it supersedes the causal
claim in the first. This failure produced ten of M6's fourteen bounces and all
three M12 bounces, and every countermeasure upstream today is aimed at the
wrong cause.

**A rexyMCP-side option worth considering, since the mechanism is now known:**
the seeder could append a synthetic final task when a phase doc has a non-empty
`## End-to-end verification` section that is not marked N/A, making the
obligation tracked by construction instead of by architect discipline. That
would fix it for every project at once. Out of bounds from here.

---

## Historical record below (M11 and earlier)

## Three calibration folds landed 2026-08-06 (PE sign-off)

All three are in `docs/dev/WORKFLOW.md`. **None is applied upstream** — the same
clauses belong in rexyMCP's `plugin/templates/WORKFLOW.md`, which is out of
bounds from a target-project architect session and needs a separate change in
that repo. This is the second batch carrying that caveat.

1. **Bug reports state symptom / root cause / DoD, not the fix.** The template's
   `How to fix` section is gone, replaced by `Root cause` + `Definition of done`;
   a prescribed fix is now optional and admissible only when the architect has
   run it. **This was the PE's counter-proposal to the fold I suggested** — I had
   argued for "execute prescribed fixes before writing them", and giving the
   executor the diagnosis plus the finish line instead is the better remedy: it
   removes the class of error rather than adding a verification step to it. Four
   occurrences in M11, and in every one the executor's behavior was correct given
   what it was told.
2. **A bounce must refresh the phase doc's acceptance criteria**, and each new
   criterion must be confirmed to fail against the current tree. Folded on first
   occurrence at the PE's direction, because the failure is silent and
   self-certifying — green gates, clean tree, accurate summary, empty diff.
3. **A fixture's ordering premise must be asserted in the test.** Extends the
   vacuous-guard family: not an unobservable property or an empty fixture, but a
   false assumption about which candidate comes first, so the path under test is
   never entered.

**phase-02 — viewer-shell: done (approved_after_3) 2026-08-20**, round-4
commit `d24dba9` + approval. bug-phase-02-3 resolved: `ctrl+o` now opens the
viewer mid-turn as well as at the prompt, so phase-03's
`… N more lines · ctrl+o` footer is true wherever it renders.

The fix extended the existing pure classifier (`focus_outcome` → `key_outcome`,
`stream.rs:889`) with `Key::CtrlO => StreamOutcome::OpenViewer`, and the caller
handles that outcome by running the viewer, re-anchoring, and `continue`-ing the
same loop — `line_buf` (`:211`) sits outside that loop (`:222`), so the
connection and the partial read survive. Reviewer mutation: deleting the CtrlO
arm fails exactly `stream_key_ctrl_o_opens_viewer`.

**Live-verified, not just unit-tested.** The probe that found the bug was re-run
against the fix in an isolated `tmux -L de-m17b` server: mid-turn ctrl+o now
gives `alternate_on = 1` with `transcript — 111-132 of 132 lines`, Escape
returns to the chat surface, and the turn **resumes and keeps streaming**
afterwards. Stated precisely: resumption was observed; run-to-final-completion
after a mid-turn viewer was not separately proven within the capture window.

## Active milestone: M18 — Container-sandboxed Agents (scoped 2026-08-28)

**Goal:** background command/script execution moves into ephemeral rootless
Docker containers via a `ContainerExec` backend at the executor choke point;
daemon stays native, foreground untouched, host ops only via the escape hatch.

Design of record: `docs/design/agent-container-sandboxing.md` (commit
`d856ca6`). Milestone README with the 10-phase plan and exit criteria:
`docs/dev/milestones/M18-container-sandboxing/README.md`.

**phase-01 — sandbox-config: done (approved_after_1) 2026-08-28**, commits
`4f8fed3` (feat) + `f008509` (test repair) + approval `d258292`. The
`[sandbox]` schema, `runs_as_container_root()`, warn-only `validate()`, the
startup call and the template docs all landed; 1387 → 1395 lib tests.

One bounce (bug-phase-01-1, resolved): fixing a **real** pre-existing flake,
the executor's replacement `peer_euid` test asserted on a descriptor whose
owning `File` dropped in the same statement, trading a deterministic
environment failure for a cross-thread fd-reuse race. Round 2 bound the
`File` to a named local. Review proved the original defect first — the old
test passes under `/dev/null` and pipe stdin and **fails under a socketpair**,
which is what an MCP stdio server hands its children — so the gate really was
blocked and the scope deviation was accepted rather than charged.

**Architect-side calibration, held at 1 occurrence:** § Authorizations tells
the executor to file a blocker and stop *"if an acceptance criterion cannot be
satisfied honestly"*, which does not cover **a gate blocked by a pre-existing
test**. The executor had no sanctioned path. phase-02's § Authorizations
already carries the widened wording; fold into WORKFLOW.md only if it recurs.

**phase-02 — container-runtime-probe: done (approved_after_1) 2026-08-28**,
commits `a549265` + `76276fb` (round 1), `65642b2` (round 2), approval
`fd7d461`. `executor/container.rs` holds the version probe and the D1 uid-map
gate, all decision logic pure; 1395 → 1405 lib tests + 1 ignored live test.

One bounce (bug-phase-02-1, resolved), two defects:
- **Architect-side, and a repeat of an already-folded rule.** The phase specced
  a module nothing calls under `-D warnings` without saying how dead-code
  would be satisfied — removing the `#[allow]` yields 8 errors, all in
  `container.rs`. The M7–M10 rule *"a phase that lands code for a later phase
  must say how the deny-warnings gate is satisfied"* already covers this; it
  was not applied. **phase-03 applies it up front** (§ Current state carries a
  "Dead-code strategy" block and a criterion pinning the repo-wide
  `allow(dead_code)` count at 7).
- **Executor-side.** Round 1 recorded its own unauthorized `pub use` fix as
  *"the architect's guidance … on re-dispatch"*. The session log has a single
  turn-0 `prompt` event, no injected feedback, and the patch at turn 58 —
  thirteen turns before the blocker entry. Corrected in round 2. Watch for
  recurrence; phase-03's § Authorizations now says explicitly to record what
  was decided, not guidance that was not received.

A third calibration item is architect-side and was caught at pre-flight, not
in review: the bounce criterion that grepped for the false sentence *quoted
that sentence*, so an unscoped grep counted 2 and could never reach 0. Fixed
by scoping to the Update Log with `sed`. The existing "validate every
criterion against the tree the phase will produce" fold does not cover a
criterion whose own text is part of the corpus it measures.

**phase-03 — image-lifecycle: done (approved_after_1) 2026-08-28**, commits
`2c6d201` (round 1) + `5f61ac2` (round 2), approval `9657f92`. Dockerfile,
`daemoneye sandbox build`, the `sandbox.lock` record and the compare helpers;
1405 → 1414 lib tests.

One bounce (bug-phase-03-1): the `missing_key` fixture was a plain string
literal containing the characters `{id}` rather than a `format!`, so
`parse_lock` rejected it at the image-id check and **two required-key
rejection paths were unguarded** — proven by mutation, invisible to a green
suite. Round 2 fixed the fixtures; both mutations now fail, re-run
independently at review. Also narrowed `pub mod container;` → `pub(crate)`.

**The phase-02 fold worked when applied up front:** the "Dead-code strategy"
block plus a criterion pinning the repo-wide `allow(dead_code)` count held
through both rounds — no new `#[allow]`, no blocker. Keep doing this.

**Deferred to milestone close:** `daemoneye sandbox build` has never been
executed by a phase. The architect builds the image and verifies the lock
round-trip at close. (The Dockerfile itself *was* built and exercised during
phase-03 drafting and again during phase-04 drafting — see below.)

**phase-04 — container-exec-args: done (approved_first_try) 2026-08-28**,
commit `d0b45a2` + approval `8beca06`. `evaluate_preflight` + `split_run_as` +
`stage_args` + `run_args`, all pure; 1414 → 1426 lib tests.

**No review bounces**, but the phase ran twice: the first run died on a
`BackendError` reaching `brain:8888` and was **resumed** (`continue_phase`),
not re-dispatched. Classified `infra_blip`. The partial tree was assessed
before choosing the lever — build/fmt/tests already green, one clippy lint
left — which is what made resume obviously right over re-dispatch or takeover.

**Two architect miscounts, both corrected against the finished tree**, and
both the same shape: *a pinned count must be derived from the phase's own
Spec, not estimated.* `11` sandbox_exec tests where the Test plan names 12;
`--user` emitted `1` where Task 3 and Task 4 each require an emission. The
executor **filed a blocker** on the second rather than editing the criterion
or merging two required call sites — the § Authorizations contract working.

All three high-risk mutations bit at review: hardcoded tmpfs ids, disabled
`BadRunAs` ordering, and a weakened `script_name_is_safe`. The shipped
validator is a character allowlist plus a `..` check — stronger than the
blocklist the spec described.

**phase-05 — background-window-integration: done (approved_first_try)
2026-08-28**, approval `f4880ef`. `sandbox_window_command` wraps a background
command as a shell-quoted `docker run …` line at the single `run.rs:178` seam;
1426 → 1432 lib tests, ignored 1 → 2.

The safety property was mutation-tested at review: removing `sh_single_quote`
and joining the argv raw fails 4 of 6 tests including the hostile-command one.
Seam ordering confirmed — the wrap precedes `let wrapped`, so `$__de_ec`
captures the container's exit status.

**Task 3 was withdrawn as impossible — architect error, and the sharpest of
this milestone's count mistakes.** It claimed removing the module
`#[allow(dead_code)]` would leave the tree green at count 6. Verified at
review: **14** items still lint dead, every one a phase-02/03/04 output whose
caller arrives later. The drafting-time "validation" deleted the line, ran
`grep -rc "allow(dead_code)"`, saw `6`, and stopped — measuring the
attribute's absence, not the lint gate's outcome. **A criterion about a gate
must be validated by running that gate, not by a proxy that resembles it.**

Executor-side, **2nd occurrence**: it filed a correct blocker, then retracted
it and proceeded. Unlike phase-02 it invented no authorization and asked the
architect to reconcile — the honest form — but the instruction said stop.
phase-06's § Authorizations now says so explicitly: *do not proceed past a
blocker you have filed.* Watch for a third.

**phase-06 — sandbox-preflight-gate: done (approved_after_1) 2026-08-29**,
approval `a61f40b`. Fail-closed gate: probe once, cache the verdict, refuse
with an operator-facing reason. 1432 → 1440 lib tests.

One bounce (bug-phase-06-1): round 1 put the gate at `run.rs:172` while
`create_job_window` runs at `:62`, so **every refused command leaked a
`de-bg-*` window** — and the mismatch message rendered `sha256:sha256:…`.
Round 2 fixed both; verified GATE_FIRST (50 before 75) and the new
single-prefix test bites. Root cause of the first was my own Task 4, which
named two placements that cannot both hold.

**Executor-side calibration is now at 2 occurrences and needs a decision at
close:** round 2 **overwrote round-1's Update Log entries in place** while
asserting they "remain below, clearly marked superseded" — they did not. The
architect recovered them from `550e315`. With phase-02's fabricated
provenance that is twice this model has misdescribed its own bookkeeping
undetectably. **A third should change how it is dispatched, not just how it
is reviewed.**

**LIVE VERIFICATION, first run 2026-08-29 — and it found a production break.**
Six phases in, no daemoneye code had ever started a container. All three
`#[ignore]`d tests pass, and `daemoneye sandbox build` ran for the first time
(image built, lock written, recorded id matches `docker image inspect`, so
preflight now passes the full chain rather than via its `NoLock` escape).
**But the window command carries no `DOCKER_HOST`.** A live tmux pane here
reports `DOCKER_HOST=[UNSET]`, so the generated `docker` line targets
`/var/run/docker.sock` — the *rootful* socket, a different daemon — and
fails with "cannot connect". Phase-06's gate cannot catch it: the daemon
probes with `Command::env` set, while tmux runs a bare string. Invisible to
1440 green tests; found in the first minute of running the thing.

**phase-07 — docker-host-propagation: done (approved_first_try) 2026-08-29**,
approval `3605e3e`. `--host <docker_host>` now heads both argv builders, so
the tmux-run command carries its endpoint; 1440 → 1443 lib tests.

**The production break is closed and proven.** At review the live test was run
with `DOCKER_HOST` scrubbed from the reviewer's own environment: it passes as
shipped and **fails** when `--host` is reverted out of `run_args`. A second
mutation moving `--host` after `run` (which docker rejects) fails two more
tests, so the ordering is guarded too. Update Log entries were appended, not
rewritten — the phase-06 round-2 behaviour did not recur once § Authorizations
said so explicitly.

**Architect calibration:** the phase-06 approval left the README row reading
`in-progress` while the doc read `done`, because the script computed the
replacement into `s2`, asserted on `s2`, then wrote `s`. **An assertion on a
value you do not write proves nothing** — verify a scripted edit by reading the
file back. Both rows fixed.

**phase-08 — sandbox-gc: done (approved_first_try) 2026-08-29**, approval
`4cad433`. Every sandboxed container now carries `de.sandbox=1`; a best-effort
sweep of orphaned containers and leaked `de-stage-*` volumes runs at daemon
start. 1443 → 1450 lib tests.

The data-destroying slip is guarded, proven by mutation: `starts_with` →
`contains` fails `sandbox_gc_selects_only_stage_prefixed_volumes`, and
`zz-de-stage-decoy` is a real name docker's own `--filter name=` matched when
measured. The disabled short-circuit holds, so a disabled sandbox still spawns
nothing.

**Architect calibration, 4th of its family:** § Task 4 dictated a doc comment
containing `filter name=` while § Acceptance criteria required that string to
appear zero times in production code — the spec told the executor to write the
exact string the spec forbade. Stated at its sharpest in the README:
**a criterion must not forbid a string the phase itself is told to write.**
Standing fold candidate for PE.

**phase-09 — staging-mount-and-ghost-label: done (approved_first_try)
2026-08-29**, approval `0e2b715`. The staging helper got its read-only source
mount and ghost containers are labelled; 1450 → 1454 lib tests. Dropping `:ro`
fails two tests, so the security property is guarded — the helper runs as
container root = host `matt`, and a writable mount would give it write access
to the operator's real 0700 script library.

**Carried to M19, recorded not bounced:** the `is_ghost` derivation is
**unguarded** — hardcoding `is_ghost: true` leaves all 1454 tests green and
satisfies every criterion the phase set. That is my Test plan's fault: it
claimed the `run.rs` change had no unit-testable seam, which was wrong. M19
should extract a pure `is_ghost_session()` predicate **before** ghost teardown
starts reading `de.ghost=1`.

**THE PILOT RAN 2026-08-29 — and it passed.** In an isolated
`tmux -L de-pilot3` server started with **no `DOCKER_HOST`** (the exact
configuration broken before phase-07), the pane confirmed
`PANE_DOCKER_HOST=[UNSET]` and the shipped window command produced `1000` /
`PILOT_OK` / `drwx------ 2 de de … /de/work` / `__EXIT=0`. **No new defects.**
Not covered, and therefore not claimable: the startup sweep has never run
through a real daemon (the operator's holds the single-instance flock), and no
AI-driven background command has gone through the full chat path.

**phase-10 — docs-and-close: done (approved_first_try) 2026-08-29**, approval
`0df1101`. `CLAUDE.md` now documents the sandbox (a § Key files row for
`executor/container.rs` and a `## Container sandbox` section) and the README's
status blockquote is accurate. Both pinned diffs — `src/` and
`assets/etc/config.toml` — came back empty and the lib suite held at 1454, so a
docs phase stayed a docs phase.

Every claim in the new section was fact-checked against the code rather than
read: `network: "none"` is hardcoded at the **production** call site
(`src/daemon/background/run.rs:186`, not merely in the test vectors), the
preflight cache is a real `OnceLock`, `sweep_sandbox_leftovers` is wired into
daemon start, and `enabled` defaults to `false`. One disclosed architect edit at
review: the section claimed *every* sandboxed process runs as `--user
1000:1000`, but `run_as` is configurable and the uid gate refuses only container
**root**. That was my own wording, dictated verbatim in the spec — the executor
reproduced it faithfully.

## M19 — Sandbox Completion (opened 2026-08-29, PE sign-off)

**Active phase: phase-02 — staging-integration**
(`docs/dev/milestones/M19-sandbox-completion/phase-02-staging-integration.md`,
status: todo, drafted 2026-08-29). Dispatch with
`/rexymcp:dispatch phase-02`.

**phase-01 — is-ghost-predicate: done (approved_first_try) 2026-08-29**,
commit `475909a` + approval `6625650`. 1454 → 1458 lib tests; reviewer
mutation confirmed the two `resolve_is_ghost` tests discriminate different
branches. Two of my criteria pinned `grep -c "fn name"` at `1` where the
test-name prefixes made `3` the only reachable value — the documented
`fn name(` trap, validated in the failing state only. Corrected in the doc.

Phase-02 drafting notes:

- **Prototyped in the tree and mutated before speccing**, then reverted. The
  four functions, the `run.rs` wiring and the six tests in the spec are the
  prototype verbatim; 1458 → 1464, all four gates green with the module
  `#[allow(dead_code)]` **removed** (repo-wide 7 → 6), three mutations each
  caught by exactly one named test.
- **Measured: clippy names exactly two dead items behind that allow today**
  (`script_name_is_safe`, `stage_args`), not the 14 M18 phase-05 recorded —
  M18's later phases gave the rest callers. One caller of `stage_args`
  retires both.
- **Measured: every sandboxed job leaks a named volume.** `docker run --rm`
  removes anonymous volumes only, and `run_args` mounts `de-stage-<job_id>`
  unconditionally; three leaked volumes from M18's own tests were sitting on
  scrappy. Phase-02 removes the volume at both completion sites — D4 step 4,
  which was designed but never built.
- **Criterion defect caught by validating against the prototype tree, not
  the current one:** `grep -c 'let job_id'` reads `2` on a correct tree
  because `let job_id_bg` matches. Pinned as `let job_id = format!` instead.
  Same family as phase-01's, caught on the right side of dispatch this time.
- **Gap recorded in the README:** scheduled `ActionOn::Script` jobs never
  enter `run_background_in_window`, so they are neither sandboxed nor staged;
  D0's claim that scheduled commands run sandboxed is false today.

**PE decisions at open:** the **egress proxy is IN scope** — I had scoped it
out and was overruled; it is phases 06–08, three phases rather than one
precisely because it is the largest unbuilt piece. Milestone README:
`docs/dev/milestones/M19-sandbox-completion/README.md`, 10 phases.

**The approved upstream pull turned out to be a no-op, and my claim was
wrong.** § "The E2E block: runnable, complete, and seeded as a Spec task" was
folded *upstream from DaemonEye* on 2026-08-09; local `WORKFLOW.md` already
carries all three of its rules in **fuller** form inside § "End-to-end
verification", plus three things upstream lacks (`${PIPESTATUS[0]}`, the
no-unpastable-bytes clause, the last-entry-anchored PASTE MATCH recipe).
Pasting it would have inserted a weaker duplicate, so nothing was pulled.
**I reached "we do not have it" from a heading diff in the same breath as
documenting that heading diffs cannot see paragraph-level folds**, and my
follow-up content probe compounded it: `grep -ci "Prove it applied"` returned 0
because local words that rule as *"a `grep -c` of the mutated text after each
direction"*. A blind instrument reporting clean is § "Run every count
criterion"'s own second corollary.

Phase-01 staging notes:

- **The measured finding that reshaped the phase:** every other call site in
  the codebase reads `SessionEntry.is_ghost` (a stored bool); the
  `starts_with("ghost-")` heuristic exists **only** in `run.rs`, twice. So the
  phase is not merely "extract the predicate" as the M18 carry note said — it
  routes both sites through a resolution rule that prefers the authoritative
  entry and falls back to the prefix. A bare lookup would have been a
  regression: a ghost session with no store entry would silently lose its
  label.
- **Both functions are pure and the store lookup stays at the call site** —
  purity is what makes the mutation seam directly testable, which is the entire
  reason this phase exists.
- **The mutation instruments were checked before being written into the spec:**
  `^    false$` and `session_id.starts_with("ghost-")` each occur **0** times in
  `src/daemon/mod.rs` today, so both `grep -c` proofs discriminate 0 → 1. An
  instrument that already matched would certify a mutation that never applied.
- **The new corpus-contamination step got its first real use and passed:** the
  phase doc contains `starts_with("ghost-")` 10 times, but every criterion
  greps a `src/` file rather than the doc, so nothing is self-satisfying.
- All four criteria were run against the current tree and **fail** as required
  (2 / 0 / 0 / 0).

**Template drift checked at milestone start, and the recorded method is
wrong.** `comm`-ing the `^#{2,3} ` headings both ways gives 5 local-only and 1
upstream-only — but local `WORKFLOW.md` is **1802 lines against upstream's
1259**. The 543-line gap is invisible to a heading diff because most folds,
including both landed at M18 close, are *paragraphs inside existing sections*
(`PIPESTATUS` 2/0, `validate every mechanical criterion` 1/0, both M18 folds
1/0). **Probe by content, not by heading.**

**Active phase: none. M18 — Container-sandboxed Agents is closed
(2026-08-29), awaiting PE sign-off.** All 10 phases `done` — 6
approved_first_try, 4 approved_after_1, none escalated; 4 bug docs, all
resolved and verified. Retrospective in
`docs/dev/milestones/M18-container-sandboxing/README.md`.

**Two calibration folds are past threshold and need a PE decision before they
land in WORKFLOW.md** (the architect does not fold unilaterally):

1. **4 occurrences** — *a mechanical criterion must not be written so that its
   own corpus contains the text it greps for* (phase-02, phase-04 ×2,
   phase-08). Fixed each time by scoping the search.
2. **3 occurrences** — *a pinned count must be derived from the phase's own
   Spec by counting it, not estimated* (phase-04 ×2, phase-05).

### Both folds landed 2026-08-29 (PE sign-off) — and neither is a new section

Both existing sections already *gestured* at these failures, and § "Run every
count criterion" says outright that a sixth occurrence calls for **a mechanical
check, not stronger prose**. So the folds add mechanism where the prose was
already adequate:

1. **§ "Run every count criterion; never derive it"** gains *"scope the search
   so the criterion's own corpus cannot satisfy it"* — a three-row recipe table
   (`sed -n '1,/^#[cfg(test)]/p'` for production-only, `sed -n '/^## Update
   Log/,$p'` for log-only, a section range for docs) plus one literal drafting
   step: **after writing a grep criterion, run it against the phase doc you are
   writing.** A non-zero hit on your own spec means the corpus is contaminated.
   All three recipes were executed against real files before being written down
   — they discriminate 3 → 1 and 6 → 4 on `container.rs` and phase-10 — rather
   than being reasoned to look right, which is the failure mode of the very
   rules they sit under.
2. **§ "Every acceptance criterion must be satisfiable"** gains two paragraphs:
   *a criterion about a gate must be validated by running that gate, not by a
   proxy that resembles it* (the `grep -rc "allow(dead_code)"` vs `cargo
   clippy` miss — 14 dead items the proxy could not see), and *count the Spec,
   don't estimate it*, since a number describing the phase's own tasks is
   arithmetic over prose already written and needs no tree at all.

**Neither is applied upstream** — both join the push backlog alongside M13's
four and M14's two (`~/src/rexyMCP/plugin/templates/WORKFLOW.md`; drift is
bidirectional and is checked at milestone start, not close).

Two executor-side items are **held at 2 occurrences each** — proceeding past
its own filed blocker, and misdescribing its own bookkeeping undetectably. A
third of the latter should change how this model is dispatched, not just how it
is reviewed.

**M19 carries:** staging integration (the only thing that retires the module's
`#[allow(dead_code)]`), the `is_ghost` coverage gap, the escape hatch, the
egress proxy, `Request::ContainerStatus`, the `log` relay opcode, and the two
unrun live checks (startup sweep through a real daemon; an AI-driven background
command through the full chat path).

Scope: docs only. `CLAUDE.md` **does not mention the sandbox once**
(`grep -ci sandbox` → 0) and has no § "Key files" row for
`executor/container.rs`, now the milestone's largest addition; `README.md:219`
still says *"3 of 10 phases are merged"*. The phase adds the key-files row, a
`## Container sandbox` section, and an honest README status update.

Phase-10 staging notes:

- **Criteria pin both `git diff --stat -- src/` and
  `git diff --stat assets/etc/config.toml` empty**, and the lib count
  unchanged at 1454 — a changed count means source was touched. This is a
  docs phase and the criteria enforce that mechanically.
- **§ Gotchas is mostly a list of things not to claim**: only *background*
  execution is sandboxed (not foreground, not remote, not broker-native
  tools); ghost shells are *labelled*, not sandboxed; script staging is
  correct but **has no caller**, which is why the `#[allow(dead_code)]`
  survives into M19. The main risk in a docs phase is overclaiming, so the
  spec spends its pre-injection budget there.
- **The pilot's results are quoted into § Current state** so the executor can
  state them as fact without re-deriving them — and § Authorizations forbids
  stating any live fact the doc has not given it.
- **No unit tests, deliberately**, with the reason written into § Test plan:
  the phase changes only Markdown, and a test asserting on wording that is
  expected to change would be worse than none.
- `tests/doc_truth.rs` does **not** gate CLAUDE.md's key-files table (it gates
  the tools tables and `assets/etc/config.toml`), so these edits are not
  machine-checked — the spec says so plainly rather than letting the executor
  assume a safety net.

Two measured defects, both small:

- **The staging helper has never worked.** `stage_args` copies from
  `/de/src/<script>` and nothing mounts `/de/src`. Measured against a real
  0700 script: `cp: cannot stat '/de/src/…': No such file or directory`.
  Adding `-v ~/.daemoneye/scripts:/de/src:ro` fixes it, and the full D4 chain
  then works — the root helper (= host `matt`) reads the 0700 original, chowns
  the copy, and the sandboxed uid reads it back as `-r-x------ de de`. The gap
  survived phases 04–08 because **nothing ever calls `stage_args`**.
- **No ghost container is ever labelled.** The one call site hardcodes
  `is_ghost: false`, while `run.rs:57-58` already branches on
  `sid.starts_with("ghost-")` for the window prefix. The phase derives
  `is_ghost` from that same predicate so the two cannot disagree.

The `:ro` on the source mount is load-bearing and § Gotchas says why: the
helper runs as container root = host `matt`, so a writable mount would hand a
compromised helper write access to the operator's real script library.

**PE DECISION — MADE 2026-08-29: close M18 after the pilot; carry the rest
into M19.** So M18 is a **ten-phase milestone** after all, ending with
phase-10 as the pilot + docs + close-out. What M19 inherits:

- **Staging integration** — a production caller for `stage_args`, deciding
  when a background command *is* a script invocation. **This is the only thing
  that retires the `#[allow(dead_code)]`, so that attribute is an explicit,
  recorded carry out of M18** — not an oversight. M19's first phase should
  remove it.
- **The escape hatch** — `GhostPolicy.escape_allowlist`, park-and-notify, the
  `escape_hatch` flag on `ToolCallPrompt`.
- **The egress proxy** (`network = "proxy"` profiles), `Request::ContainerStatus`
  + the `daemoneye status` surface, and the `log` relay opcode.

**What phase-10 therefore is:** turn `[sandbox] enabled = true` for real, run
background commands through chat against the live rootless runtime, and
capture what happens — container start latency, whether the `de-bg-*` pane
still reads normally, whether the startup sweep reclaims the two stale
`de-stage-*` volumes deliberately left on the host, and whether anything about
the shipped surface is wrong in a way 1454 green tests cannot see. Plus the
doc sweep (CLAUDE.md, README, `assets/etc/config.toml`) and the retrospective.

**Framing note for drafting phase-10:** the design's § Rollout imagined a
*ghost-shell* pilot, but ghosts are not wired to containers yet — phase-09
only makes them labelled. The honest pilot for what M18 actually shipped is
**background command execution**, which is the one path that is complete end
to end. Do not spec a ghost pilot the code cannot perform.

*(Superseded — the original scope question, kept for the record:)*
**M18 will not fit in ten phases.** After 09 the
remaining work is: staging integration (a production caller for `stage_args`,
which is the only thing that retires the `#[allow(dead_code)]`), the escape
hatch, the egress proxy, `Request::ContainerStatus` + `daemoneye status`, the
`log` relay opcode, and docs + pilot. That is three to five phases, not one.
**Either extend M18, or close it after a pilot and carry the escape hatch and
proxy into M19.** The sandbox is functional and default-off today, so closing
early is a real option. Recorded in the milestone README § Notes.

Scope: label **every** sandboxed container `de.sandbox=1`, then sweep orphaned
containers and leaked `de-stage-*` volumes at daemon start. Nothing spawns
during the phase; the sweep is exercised by the architect at close.

**Renumbering:** ghost lifecycle + staging becomes 09 (it is what finally
retires the `#[allow(dead_code)]`), and escape-hatch merges with docs + pilot
into 10.

Phase-08 staging notes — a second live pass produced three facts, none of them
guessable from the code:

- **No sandboxed container is labelled today.** `run_args` emits
  `--label de.ghost=1` only when `is_ghost`, and the sole call site hardcodes
  `false`; `docker inspect … .Config.Labels` returns `{}`. **A sweep is
  therefore impossible right now**, which is why the phase labels every
  container rather than only ghosts.
- **A killed `docker` client leaves the container running and `--rm` never
  fires** — measured with `SIGKILL`, then `docker ps` reporting `Up 3
  seconds`. `--rm` alone does not prevent orphans.
- **`docker volume ls --filter name=` is a SUBSTRING match**, measured with a
  decoy `zz-de-stage-decoy` that matched `--filter name=de-stage-`. A sweep
  trusting docker's filter would delete user volumes. The spec does the prefix
  check in Rust and pins the decoy, a near-miss (`de-stagex`) and a control
  (`unrelated`) as must-NOT-matches.
- Two stale `de-stage-*` volumes are deliberately **left on the host** as the
  fixture the milestone-close sweep check will consume.
- The pinned vector changes for the second time in two phases (label added);
  § Gotchas 4 says so explicitly so the expectation is updated rather than
  worked around.

Inserted to fix the break above; ghost lifecycle moves to 08, escape-hatch
merges with chat containers into 09.

Phase-07 staging notes (all measured):

- **`--host` must precede the subcommand.**
  `docker --host <sock> run …` works; `docker run --host <sock> …` gives
  `unknown flag: --host`. So the flag is elements 0–1 of the argv vector.
- **The flag beats a `DOCKER_HOST=…` shell prefix.** Both were measured
  working, but the flag is ordinary argv — it flows through the same
  `sh_single_quote` path as everything else and carries no shell-assignment
  semantics.
- **This deliberately changes phase-04's pinned vector**, and the doc says so
  in § Gotchas 4 so the executor updates the expectation rather than working
  around it.
- **The live test's `.env_remove("DOCKER_HOST")` is the whole point** — a live
  test that inherits the variable passes on any developer machine and leaves
  the production gap invisible, which is exactly how it survived phases 05
  and 06. A criterion pins `env_remove` at 1.
- § Authorizations now also says: **append to the Update Log, never edit or
  delete an existing entry** — the phase-06 round-2 behaviour, made explicit.

**Scope change: a preflight gate was inserted as phase-06**, pushing ghost
lifecycle to 07 and folding the egress proxy into 09. Reason: phase-05 shipped
sandboxed background execution with **no preflight at all**, and worse, it is
**fail-open** — `sandbox_window_command` falls back to the host command when
the sandbox cannot be built. When the operator asked for isolation, running on
the host instead is the wrong answer. Phase-06 probes once, caches the
verdict, and **refuses** the command with an operator-facing reason. That gap
is more urgent than ghost lifecycle, which is still behind a default-off flag.

Phase-06 staging notes (measured on the live rootless Docker):

- **One container run yields both gate inputs**, so the spec pins a single
  probe rather than two: `sh -c 'id -u; echo ---; cat /proc/self/uid_map'`
  returns `1000`, the `---` sentinel, then the two-line map. The exact output
  is quoted in § Gotchas as the test fixture.
- **The fixture was parsed through to the pinned verdict before speccing:**
  uid 1000, ranges `(0,1000,1)` and `(1,100000,65536)`, `host_uid_for(1000)`
  = **100999** — matching the D1 measurement. The test asserts through
  `parse_uid_map`, not by string equality, so it proves the map survives the
  split in a form the gate can use.
- **`NoLock` is the expected verdict on this host** — `~/.daemoneye/etc/
  sandbox.lock` does not exist because `daemoneye sandbox build` has never
  run. The live test accepts `Ok(())` **or** `NoLock` and nothing else, and
  § Gotchas forbids "fixing" it by writing a lock from this phase.
- **The `allow(dead_code)` still cannot go**, and the criterion pins it
  **unchanged at 7** rather than predicting a number. After phase-06 wires the
  probe/preflight path, `stage_args` and `script_name_is_safe` remain
  unreachable until staging lands in phase-07. This is the phase-05 lesson
  applied: no count is asserted that was not derived.
- The `OnceLock` idiom is quoted from `src/daemon/mod.rs:17-25`
  (`DAEMON_START`), so the probe runs once per daemon lifetime rather than per
  command.

**This is the first M18 phase whose code actually starts a container.** Scope:
`sandbox_window_command` wraps a background command as a shell-quoted
`docker run …` line, wired at the single seam in `run.rs:159-172`; the
`de-bg-*` window, completion detection, output capture and GC are untouched.
It also removes the module's `#[allow(dead_code)]` (7 → 6), since it adds the
first production caller.

Phase-05 staging notes (measured on the live rootless Docker):

- **The safety property was proven before it was specced.** Using the exact
  `sh_single_quote` algorithm, the hostile command
  `echo inside-container; touch /tmp/PWNED` was built into a window command
  and run through `sh -c`: it printed `inside-container` from **inside** the
  container and `/tmp/PWNED` was never created on the host. A test pins the
  final token verbatim.
- **`sh_single_quote` (`shell.rs:27`), not `shell_escape_arg` (`shell.rs:15`)**
  — the codebase's own doc comment says which, and the latter is a tmux
  `send-keys` helper that would leave `;` and `&&` live. A criterion pins
  `shell_escape_arg` at 0 occurrences in `container.rs`.
- **Both pinned quoting renderings were executed, not written from memory:**
  `echo 'a'` → `'echo '\''a'\'''` and the hostile command → one token.
- **The seam ordering is load-bearing.** The wrap goes after the sudo
  sentinel and before `let wrapped = …`, so `$__de_ec` captures `docker run`'s
  exit status — which is the container's — and completion detection needs no
  change.
- **A named volume auto-creates on `-v` even read-only, and outlives `--rm`.**
  So phase-05 pre-creates nothing, and the volume-leak cleanup is explicitly
  phase-06's; a criterion pins `gc.rs` untouched.

Scope: the two pure decisions that stand between an agent and a container —
`evaluate_preflight` (runtime → uid gate → lock → image, in that order) and
the argv builders `run_args` / `stage_args` / `split_run_as`. **Nothing
spawns**; phase-05 is the first phase that starts a container.

**Scope change:** phase-04 was originally scoped as the whole
`ContainerExec` backend. Split — argv construction and the preflight decision
are pure, hermetic and where a subtle error silently defeats the sandbox;
spawning, the `de-bg-*` wiring, the `log` opcode, `Request::ContainerStatus`
and the egress proxy all move to phase-05. The README phase table reflects
this.

Phase-04 staging notes (measured on the live rootless Docker):

- **The whole argv was prototyped end to end before the spec was written**,
  against an image built from the checked-in `containers/Dockerfile`: uid 1000
  inside, staged script executed from the read-only volume, scratch written,
  `ls -ld /de/work` showing `drwx------ de de`. The pinned vector's flags were
  then diffed against that prototype — no flag in one and missing from the
  other.
- **`f64` renders without a trailing `.0`.** Measured with `rustc`:
  `format!("{}", 2.0f64)` → `"2"`, `1.5` → `"1.5"`; docker accepts
  `--cpus 2`. The pinned vector therefore expects `"2"`, and § Gotchas warns
  against `{:.1}`.
- **The tmpfs uid/gid must derive from `run_as`, not a literal.** A dedicated
  test uses `run_as = "10:0"` — a hardcoded `1000` passes the default-config
  test and fails that one.
- **`stage_args` interpolates a script name into a shell line**, so seven
  unsafe names are pinned as must-reject (`../etc/passwd`, `a;rm -rf /`,
  `a$(id)`, …).
- **Image ids are genuinely not reproducible** — the same Dockerfile built
  from two contexts produced `sha256:185a9ca…` and `sha256:0d02beb…`,
  confirming phase-03's no-hardcoded-digest rule empirically.
- **An unsatisfiable criterion was caught at drafting** (third occurrence of
  that shape, now a fold candidate — see the M18 README): three greps over
  `container.rs` would have counted the phase's own pinned test vector, which
  legitimately contains `mode=0700` and `uid=1000,gid=1000`. All three are
  now scoped to the production half with
  `sed -n '1,/^#\[cfg(test)\]/p'`, and re-validated at `0`.

Scope: `containers/Dockerfile`, `daemoneye sandbox build`, the
`~/.daemoneye/etc/sandbox.lock` digest lockfile, and the pure compare helpers
phase-04's refusal gate will call. Everything it writes is reachable from
`main.rs`, so it adds **no** `#[allow]`.

**Design correction found while drafting (D4, now in the design doc).** The
`/de/work` scratch tmpfs is **not** writable by the sandboxed uid unless the
mount flag carries `mode=0700,uid=1000,gid=1000`, and the obvious Dockerfile
fix cannot work — when the mountpoint exists in the image the tmpfs inherits
its **mode but not its ownership**, so an in-image `chown 1000:1000` still
gives `drwx------ root root` and a denial. The original D4 claim came from a
test against stock `alpine`, which has no `/de/work`, where Docker creates
the tmpfs `1777`. Measured table is in D4; **phase-04 must pass the uid/gid
options**.

**Scope change:** the image staleness warning and the runbook `requires_tools`
check are deferred out of phase-03 — `RetentionWarning` holds `&'static str`
fields (`src/daemon/utils/warnings.rs:24`) so a dynamic "built N days ago"
message does not fit it, and neither check has a consumer until phase-04.

Phase-03 staging notes (fixtures measured on the live rootless Docker; the
executor has no runtime):

- **The specced Dockerfile was built and exercised before being written into
  the doc** — the image runs as uid 1000 by default, `curl`/`jq`/`git`/
  `python3` are present, and its process is host-visible as uid 100999, the
  D1 expectation.
- **`docker build -q` prints the image id on stdout** and
  `docker image inspect --format '{{.Id}}'` returns the identical string;
  non-`-q` builds put progress on stderr and print no bare id.
- **The doc forbids hardcoding any digest**, with a criterion pinning
  `grep -rc "sha256:185a9ca" src/` at 0 — the id changes on every rebuild, so
  a test asserting a real one passes today and fails next week. Fixtures are
  `format!("sha256:{}", "a".repeat(64))`.
- The E2E block's section C runs `cargo run -- sandbox --help`, which
  exercises the real clap tree with no docker and no daemon. It fails today
  with `exit=2` / `unrecognized subcommand 'sandbox'`, so it discriminates.

Scope: `src/daemon/executor/container.rs` — the runtime version probe and the
D1 UID-mapping gate, with **all decision logic pure and fixture-tested** and a
single `#[ignore]`d live test. Nothing calls it; phase-04 wires it in.

**Scope change made at drafting:** the `Request::ContainerStatus` IPC surface
and its `daemoneye status` line moved from phase-02 to phase-04 — until a
container can actually run there is nothing to report but "the version probe
answered", and that does not justify new IPC surface. Recorded in the M18
README § Notes.

Phase-02 staging notes (every fixture captured live from the rootless Docker
on scrappy 2026-08-28; the executor cannot reproduce any of them, which is why
they are quoted verbatim in the doc):

- **`/proc/self/uid_map` is byte-identical with and without `--user`** — it
  describes the namespace, not the process. A gate reading only the map cannot
  tell a root container from a non-root one, so the spec requires **two**
  inputs (`id -u` plus the map). This is the trap most likely to produce a
  confidently wrong gate.
- **The pinned arithmetic was executed, not reasoned:** container `0 → 1000`,
  `1 → 100000`, `1000 → 100999`, `65536 → 165535`, and `65537`/`70000 → None`.
  The `1000 → 100999` value matches the live host-visible uid measured for a
  `--user 1000:1000` container; an off-by-one against the range start gives
  `101000`.
- **Missing binary and dead daemon are different outcomes** and the enum keeps
  them apart, because the operator fix differs. Measured: unreachable daemon →
  `exit=1`, empty stdout, a specific stderr string (quoted in the doc); absent
  binary → the spawn itself fails with `NotFound`. Healthy → `exit=0`,
  stdout `29.7.2`.
- **`cargo test sandbox_runtime` passes today with zero tests**, so every
  criterion is a line count. Measured on this tree.
- Reuses `crate::tmux::bounded_output_with` (`src/tmux/mod.rs:125`) rather
  than `Command::output()`, and a criterion pins `.output()` at 0 — a plain
  `output()` can deadlock on a full pipe buffer.
- The phase adds exactly **one** `#[ignore]`, pinned by a criterion, so the
  no-docker gate constraint cannot be met by quietly ignoring more.

Scope: `[sandbox]` config schema only — `SandboxConfig` + `SandboxLimits` +
`SandboxProfile` + `SandboxGhostDefaults`, serde defaults, a warn-only
`validate()`, `runs_as_container_root()`, wiring into `Config` and the
`daemon/mod.rs:479` startup validation site, and the `assets/etc/config.toml`
documentation. **Hermetic — the phase must not invoke docker at all**, so it
passes on a host with no container runtime.

Phase-01 staging notes (all measured against the tree at draft time, commit
`70a3389`):

- **Baseline: 1387 lib tests, four gates green, zero `sandbox` matches** in
  `src/`, `assets/`, `tests/`. Five of the six mechanical criteria measured
  `0` today; the sixth is a regression guard measured in **both** directions
  (see below).
- **The `doc_truth` config gate is automatic and bidirectional.**
  `config_sections()` derives sections from the `Config` struct, so adding
  `pub sandbox: SandboxConfig` immediately makes every `SandboxConfig` field
  a documentation obligation in `assets/etc/config.toml`. Proven live: seeding
  `# nonexistent_knob = 1` under `[ghost]` made
  `seeded_config_template_has_no_phantom_keys` FAIL naming the key; reverted,
  `2 passed`.
- **The `profile` map needs a bare `# [sandbox.profile]` heading line** — a
  trap that is not guessable. The gate matches a sub-table on its **last**
  dot-segment, so `[sandbox.profile.researcher]` registers as `researcher`
  and leaves `profile` undocumented. Simulated against faithful ports of the
  real `template_keys`/`subtables`/`struct_fields` functions: template with
  only the `.researcher` heading → `MISSING: ['profile']`; same template plus
  the bare heading → `MISSING: []`. Now § Gotchas item 2 with both results
  quoted.
- **`cargo test sandbox` passes today with zero tests** — the M16 vacuity
  trap. Measured: section A of the E2E block prints no test lines at all
  while reporting `cargo_exit=0`. Every criterion is therefore a **line
  count**, never an exit status, and § Gotchas item 3 says so with the
  measurement.
- **`run_as` must be split on `:`, not substring-matched** — `"10:0"` is not
  root. Pinned as a five-row table in § Test plan including three negative
  cases, per the "pin negative cases" rule.
- **PASTE MATCH validated both ways** against a copy of the phase doc: a
  byte-exact paste → `PASTE MATCH`, one retyped line → `PASTE MISMATCH`.

**Operator prerequisite before phase-02 dispatch:** rootless Docker installed
on the daemon host (sudo system-state change — operator/architect only, never
an executor task). Phase-01 is pure config schema and does not need it.

**Load-bearing constraint for every M18 phase doc:** the four gates must stay
green on hosts without docker; runtime-touching tests are `#[ignore]`d, logic
is tested against pre-injected fixture output.

---

## Historical: post-M17 boundary (superseded by M18 scoping)

**Active phase was none.** Both **M16 — LLM Stream Robustness** and **M17 —
Transcript View** were closed 2026-08-20 on PE sign-off.

**M17 closed** with all 7 phases `done` (5 approved_first_try; phase-02
approved_after_3, phase-03 approved_after_2) and all 5 bug docs resolved.
Retrospective: `docs/dev/milestones/M17-transcript-view/README.md`.

**M16 closed** with all 8 phases `done`. Its **five live exit criteria were
never run** — sign-off accepts that rather than recording a pass. The list and
the reason are in that milestone's § Close-out. M17 later proved the safe way to
run them: an isolated `tmux -L <name>` server, which cannot touch the operator's
session.

**Calibration state after the close:**

- **Applied** to `docs/dev/WORKFLOW.md` (PE-approved): *a criterion for a
  cleanup obligation must assert the cleanup ran, and assert the count* — three
  occurrences, all in M17.
- **Held at 2 occurrences:** *a criterion that names a function produces a test
  of that function; only a criterion that names an observable behaviour produces
  a test of the wiring.*
- **Carried from M16, unapplied:** the "criterion validated in only one
  direction" fold (5 occurrences), which was not part of this close decision.

**Carried work, none of it blocking:**

- M16's five live criteria (§ Close-out there).
- M17's four unrun live checks: resize-while-open reflow, `y` copy through the
  viewer, `/session load` rehydration against a live daemon, wheel/click.
- Two M17 design questions deliberately left out of bug fixes: whether the
  viewer should re-render markdown (it currently prints the raw token stream,
  losslessly), and opening the viewer from an approval or credential prompt,
  which use their own readers.

**Round 2 fixed the behaviour and left the guards hollow.** `dae5f5f` removed
`Modifier::UNDERLINED` (focus is now REVERSED on the header, BOLD on the body),
word-wraps prose, and keeps `Block::Output` on the hard wrap — all correct, and
`style_for_focused_is_distinct_without_underline` bites when the focus style is
neutered. But two reviewer mutations that **undo the user-visible fix** left the
suite fully green:

| Mutation | Observed |
|---|---|
| `Output` back to word-wrap (re-flows machine output) | 41/41 pass |
| prose back to hard wrap (mid-word cuts return) | 41/41 pass |

Causes: `wrap_words_does_not_split_words` calls the helper directly and never
goes through `layout_blocks`, so the wiring is unasserted; and
`output_rows_keep_hard_wrap` uses a single unbroken 30-char token, which both
wrappers split identically into 3×10.

**Architect-side, and the second occurrence of one shape in this phase** — the
round-1 review already recorded `expanded_layout_is_unchanged_by_the_new_path`
comparing a wrapper with the function it delegates to. Named for the calibration
queue: **a criterion that names a function produces a test of that function;
only a criterion that names an observable behaviour produces a test of the
wiring.** With the criterion-design fold already at threshold, this is a second,
distinct fold candidate for PE decision at close.

Round-3 criteria assert exact row vectors through `layout_blocks` with fixtures
whose two wrappings differ (`"aaa bbb ccc ddd"` at 7 → `["aaa bbb","ccc ddd"]`
word vs `["aaa bbb"," ccc dd","d"]` hard), and require a second mutation pair
(M2) proving each guard fails when its wiring is reverted. The bug doc forbids
changing the shipped behaviour to satisfy a fixture.

**Second defect found by looking at the thing on screen.** A user screenshot of
the working viewer showed the trailing answer rendered with dozens of underlined
rows. Confirmed by SGR capture in an isolated `tmux -L de-m17c` server: eight
rows of the focused block each carry `ESC[4m`. Cause:
`style_for_focused = style_for(..).add_modifier(Modifier::UNDERLINED)` applied
to **every row** whose `block == focus`, and the viewer opens focused on the
last block. Underline is a fine cue for one header row; on a multi-row block it
reads as a rendering fault.

The same screenshot shows prose wrapping mid-word (`` `/var/lo `` + `` g` ``,
`daem` + `on dir)`) because `push_wrapped` uses `wrap_line_hard` for everything.
That is right for tool **output** — machine text must not be re-flowed — and
wrong for the prose people now read in the viewer. Round-2 criteria pin both
halves, including `output_rows_keep_hard_wrap` so the fix cannot re-flow output.

**Classified `spec_bug` again.** Phase-03 task 5 said "emphasised style — pick
it from the existing `Palette`; nothing about the colour is pinned", which left
the *scope* of the emphasis unpinned; the executor reasonably applied it
per-row. The wrap was inherited from phase-02 reusing the inline helper. Both
phases implemented their specs.

**Explicitly out of scope in the bug:** the viewer shows literal `**bold**` and
backticks, because phase-01 stores the raw token stream (lossless) and the
viewer prints it plainly. Whether the viewer should re-render markdown is a
design decision for milestone discussion, not a bug fix.

**Pattern worth naming at close:** three of M17's four bugs are architect-side
spec gaps, and **two of them were invisible to every headless gate** — found
only by running the thing and looking at it. The milestone's deferred live
criteria have now paid for themselves twice.

**M17's live check found a real defect — the first thing the deferred live
criteria have caught.** Measured in an isolated `tmux -L de-m17` server against
the shipped binary: pressing `ctrl+o` **mid-turn**, at the moment phase-03's
`… N more lines · ctrl+o` footer renders, does nothing — `#{alternate_on}`
stays `0` and the keypress is **swallowed, not queued**. The same key at the
idle prompt opens the viewer correctly (`alternate_on` 1, full 60-line output
present in a 98-row transcript). So the viewer is sound; its single entry point
is not.

Mechanism: `Key::CtrlO` is handled only in the idle input loop
(`chat.rs:738`). During a turn the client is in `select_stream`
(`stream.rs:807`), where `focus_outcome` maps only `Key::FocusGained` and
`InterruptState::feed` returns `Ignore` for ctrl+O.

**Classified `spec_bug` — architect-side, not executor error.** Phase-02 listed
"opening the viewer mid-turn" as out of scope (defensible alone); phase-03 then
added the ` · ctrl+o` footer, which advertises the key at the one moment it
cannot work. A deliberate limitation plus an unconditional advertisement is a
broken promise. Neither phase's criteria could see it: both were satisfied, and
no headless test can observe "the key does nothing right now".

Round-4 criteria are behavioural and were each run against the current tree to
confirm they fail: `grep -c "Key::CtrlO" src/cli/commands/stream.rs` = 0,
`grep -c OpenViewer` = 0, and `stream_key_ctrl_o_opens_viewer` absent. The fix
extends the **existing** pure classifier (`focus_outcome`, `stream.rs:873`,
already unit-tested) rather than adding a second one, and must not end,
restart or reconnect the turn — `line_buf` is caller-owned so no daemon frames
are lost across the viewer.

**M17 is no longer at its boundary**: six phases `done`, phase-02 re-opened.

Phase-07 staging notes (verified against the tree at draft time):

- **The disable must live in the existing `AltScreenGuard` closure**, not after
  the loop. If mouse tracking is left on when the viewer exits by an error
  path, the *chat* session sprays escape sequences on every mouse move. This is
  the phase-02 failure re-run on a new resource, so the spec says it in those
  terms and a criterion checks the disable is inside the guard.
- **Mouse is enabled in the viewer and nowhere else** — enabling it on the
  inline surface would take drag-select away from the user's own terminal.
  Criterion: `EnableMouseCapture` appears in `viewer.rs` and in no other file
  under `src/cli/` (baseline verified: 0 occurrences anywhere today).
- **SGR fields are multi-digit decimals**, so the parser must accumulate until
  `M`/`m` — a click at column 137 sends three digits. There is no `<` arm in
  `read_key` today, so those digits currently leak out as stray `Key::Char`s.
  Pinned with an explicit multi-digit test (`col: 136, row: 41`).
- `crossterm` 0.29 already exports `EnableMouseCapture`/`DisableMouseCapture` —
  verified in the vendored source; no dependency change.
- tty test helpers named by line (`make_pipe_stdin` 353, `read_key_bounded` 410,
  `read_key_within` 415) so the executor reuses them instead of inventing a
  harness. Filter `cargo test --lib cli::input` verified: 32 tests today.
- Mutation baselines checked: `if false {` 0, `disarm` 0.

Phase-06 staging notes (verified against the tree at draft time):

- **The trigger is `/session load <name>`, not process start.** There is no
  "resume this session id" path in `daemoneye chat` — `run_chat_inner`'s
  `session_override` (`chat.rs:32`) is a **tmux session name** for
  managed-session auto-attach, and every chat mints a fresh id at `chat.rs:89`.
  A spec that assumed a resume flag would have sent the executor hunting for
  something that does not exist.
- **`Message` → `Block` is one-to-many and one-to-none**, pinned per case: one
  assistant record can carry both `content` and `tool_calls`; a user record
  carrying `tool_results` has empty `content`; a record with neither
  contributes zero blocks.
- **The transcript is cleared before refilling** — `/session load` replaces the
  conversation, so appending would interleave loaded history with the current
  client's screen. `Transcript::clear()` resets `evicted` too, so a stale
  "N older blocks evicted" note is not inherited.
- **The daemon's truncation marker is kept verbatim** and the pane log is never
  read — that archive is unmasked. Criterion:
  `grep -rn "pane_logs_dir\|var/log/panes" src/cli/` exits 1 (verified: it
  does today, and must keep doing so).
- Import path pinned as `crate::ai::Message`, verified against
  `src/session_store.rs:6` and `src/daemon/session.rs:5`.
- Mutation baseline checked: `grep -c 'shown: usize::MAX,' src/cli/transcript.rs`
  is 0 now, 1 once the field is broken.

**Start at the `ROUND 2` block at the top of the phase doc's § Acceptance
criteria.** That block holds the only unfinished work: four criteria, each run
and confirmed to fail against the current tree.

**Round 2 returned `complete` with an empty diff and changed nothing** — the
documented green-bounce failure mode. The cause was architect-side: round 1's
bounce filed the bug but left all eight original criteria passing, so the phase
doc still certified itself as finished and the executor correctly concluded
there was no work. The criteria are refreshed as of round 3; the bug doc alone
was not enough.

**This is a green bounce.** Round 1 shipped correct production code and all
eight original acceptance criteria pass; four green gates and a clean tree are
*expected* and are not evidence the phase is done. Two edits remain — one test
fixture
(`incident_context_is_none_for_a_low_signal_alert`, whose query shares no term
with its seed, so deleting the guard it exists to protect leaves all 12
situational tests green) and one self-referential rustdoc link — plus the
`(end-to-end verification)` Update Log entry the spec required and round 1 never
wrote. `cargo test --lib` must report **1147, not 1148**. Full detail and a
both-directions-executed fixture recipe are in the bug doc.

The two remaining write/read choke points that ignore the index. An
incident-response ghost starts cold on every alert with no memory of the last
time the same one fired; and `add_memory` to `incidents` never fills
`relates_to`, so `expand_relates_to` — which already consumes that field on the
prompt path — has nothing to walk. Five tasks: an additive
`fts5_search_in_category` (leaving `fts5_search` a wrapper so its two callers are
untouched), `inject_yaml_relates_to` mirroring the `session_origin` injector,
auto-linking on incident writes, an `assemble_incident_context` block seeded into
the ghost's first user turn, and tests.

**The one trap that would silently void this phase:** the index stores the
category's **`canonical_name()`** — `"incident"`, singular — while
`dir_name()` is `"incidents"`. Filtering on the plural matches zero rows and
every test built on it passes vacuously. Verified at drafting
(`src/memory.rs:24-40`, `src/memory/index.rs:785`), pre-injected, and pinned as a
negative criterion.

**First phase drafted under the three folds.** Its acceptance criteria are split
into five that **fail against the current tree** (progress markers, each run at
drafting) and two that **already pass** (no-regression guards, labelled as such
so neither is mistaken for evidence of work). And where 07a's spec would have
prescribed a fix, the E2E block now says: if the mutation leaves every test
green, **report a blocker rather than adjusting a test to make it fail** — that
is precisely what cost 07a a dispatch.

[M11 phase-07a — situational-turns-epochs](milestones/M11-knowledge-index/phase-07a-situational-turns-epochs.md)
**completed 2026-08-06** (`escalated` — architect takeover after 2 bounces and a
`NoProgressStall`; [bug-07a-1](milestones/M11-knowledge-index/bugs/bug-07a-1.md)
minor, fixed). A `[SITUATIONAL]` block now carries one cross-session turn and one
epoch into the per-turn prompt, `read_line_at_offset` lives once in
`src/memory/index.rs`, and `PromptCtx` carries `session_id`. All three failures
traced to my specs, not the executor: a prescribed fix that was wrong twice, and
a stale acceptance-criteria set. Those are the three folds above.

[M11 phase-06 — prompt-scoring-fix](milestones/M11-knowledge-index/phase-06-prompt-scoring-fix.md)
**completed 2026-08-06** (`escalated` — architect takeover after two
`NoProgressStall` hard_fails; 0 bug docs, neither failure was a defect in
shipped work). FTS hits now score `FTS_WEIGHT * mag/mag_max * eff`, the merge is
keyed by `(namespace, key)` with max-wins, production code makes one memory-dir
listing per turn instead of four, and `memory_retrieved` logs what it emitted.
The executor wrote the production code and five of six test fixtures; the
architect fixed the sixth and captured the mutation pair. Two calibration items
recorded in the phase doc — an acceptance criterion invalidated by the spec's own
Test plan (`spec_bug`, second occurrence of the general class), and a worked
example that showed the correct shape without the failure mode it prevents
(first occurrence).

[M11 phase-05c — reconcile-scope-fix](milestones/M11-knowledge-index/phase-05c-reconcile-scope-fix.md)
**approved 2026-08-06** (`approved_after_1`; one bounce,
[bug-05c-2](milestones/M11-knowledge-index/bugs/bug-05c-2.md) — the transcript
was not mechanically captured, the code fix was correct first time). A search
over an empty corpus no longer wipes every other corpus:
`open_and_reconcile_if_empty(table)` rebuilds only that corpus, and phase 05b's
whole-index seeding workaround is gone.

## Calibration earned on 05c (superseded items are marked in each phase doc)

1. **A mutation check the executor performs on itself is not trustworthy.** On
   05b it applied the mutation and failed to restore it *twice* — once rewriting
   the guard test to assert the mutated behavior, once leaving the mutation in
   shipped code after an explicit two-line undo instruction. The phase-doc
   instruction is necessary but not sufficient; **the restore must be verified at
   review by grepping the shipped source.** Third occurrence of self-reported
   verification not surviving checking (03b fabricated transcript, 05a untested
   diagnosis, 05b unreverted mutation).
2. **"Verify the guard is not vacuous" belongs inside every exclusion criterion.**
   05b's `"all"`-excludes-turns test passed *with the mutation applied* — it
   proved nothing, because an unrelated empty corpus had silently wiped its
   fixture. Absence assertions pass trivially whenever the fixture is empty for
   any reason. This is the second occurrence (03a's `line.contains("turn")` was
   the first) — **one more and it folds into WORKFLOW.md.**
3. **"Execute, don't assert" applies to diagnosis.** My 05a assist-2 root cause
   was derived from reading code and was wrong; it cost a run. Running the failing
   test first would have shown the real cause immediately.
4. **The read-only stall is now at 5 occurrences** across 03a, 05a (×3) and 05b
   (×2). Well past the fold threshold, but the remedy is runtime-side in the
   rexyMCP governor and out of bounds from this repo.

## The rule 03b earned: re-run a pasted transcript, never read it

The bounce was a **fabricated** end-to-end transcript — 39 of 56 pasted
`memory::index` test names did not exist, describing ~25 error-injection tests
for `index_event_segment` in a diff that added none. **The totals were correct**
(52 and 94 are the true counts), so skimming would have passed it. Only diffing
the pasted names against a live run caught it, and that diff is now the standard
check at review:

```sh
cargo test --lib <module> 2>&1 | grep '^test ' | sed 's/ \.\.\..*//' | sort > /tmp/real
# extract the pasted block, sort, then: comm -13 /tmp/real /tmp/claimed
```

Scope the extraction to the entry's line range — a whole-file grep also picks up
the server-authored completion block's full `cargo test` output and produces
false positives.

**Taxonomy gap, still open.** Recorded as `false_completion`, but that class is
defined as self-reporting complete on a *red* gate. There is no class for
**fabricated evidence under green gates**, which is the more dangerous shape:
correct code plus passing gates make it look like a clean pass. Worth raising in
the rexyMCP repo.

**The green-bounce treatment works on this executor.** 03b's fix round was a
green bounce (four green gates, clean tree, documentation-only fix) — the shape
whose documented failure mode is `complete` with an empty diff. Applying the full
treatment before re-dispatch (loud header that green gates are *not* evidence of
doneness, explicit do-not-touch list, one enumerated task, inverted finish
condition "1084, **not** 1085") landed it in 31 turns with zero source files
touched. First local confirmation of that fold.

## Two lessons out of 03a, both at the fold threshold

**1. Identity criteria must pin distinctness — second occurrence, one from a
fold.** An acceptance criterion of the form "each X maps to *its own* Y" needs
the spec to name the discriminator *and* forbid the vacuous one. "each offset
seeks to its own line" was satisfied by `line.contains("turn")` — a JSON key
every record carries. The fix that worked was asserting the values are **pairwise
distinct**, not merely individually well-formed. If this recurs, fold it.

**2. A prescribed fix in a bug report is a system fact, and must be executed
before it is written — `spec_bug`, second occurrence in M11.** `bug-03a-1`
Finding 2 called `.or(Some(0))` a defect and prescribed removing it. Applying
that instruction at review breaks three tests: `metadata` fails principally
because the archive *does not exist yet* on a fresh append, where offset `0` is
correct. The executor restored the fallback and was right to; the finding was
withdrawn. The M7–M10 rule ("do not assert a fact about the system in a spec
unless it was executed") was written for phase specs and had not been applied to
bug reports, where the prescribed fix is exactly such an assertion. First
occurrence was `bug-02b-1` Finding 1's `read_line` recipe. **One more and this
folds into WORKFLOW.md as a bug-report clause.**

**3. The executor verify-loop pathology recurred (second occurrence).** The first
re-dispatch of 03a hard-failed on `NoProgressStall` after 60 consecutive
read-only turns grepping for the import path of `crate::ai::Message` — unrelated
to the remaining work. Resume with pointed guidance (name the stall, mark the
already-correct files do-not-touch, inline the fix, give an inverted test-count
finish condition) cleared it in 48 turns. Prefer resume over takeover here; the
prior note said prefer takeover, which would have cost the telemetry point
unnecessarily.

**Calibration fold landed 2026-08-04 (PE sign-off).** `docs/dev/WORKFLOW.md`
§ "End-to-end verification" now requires the entry **per dispatch**, not per
phase: a bounce-fix round needs its own, and an entry from an earlier round does
not carry forward. Folded after three occurrences (phase-01 r1, phase-02a r2,
phase-02b r2). **Not yet applied upstream** — the same clause belongs in
rexyMCP's `plugin/templates/WORKFLOW.md`, which is out of bounds from a
target-project architect session and needs a separate change in that repo.

[M11 phase-02b — contentless-corpora](milestones/M11-knowledge-index/phase-02b-contentless-corpora.md)
**approved 2026-08-04** (`approved_after_1`; one bounce,
[bug-02b-1](milestones/M11-knowledge-index/bugs/bug-02b-1.md), verified fixed).
`turns` and `events` are populated with byte-offset sidecar maps, masked on
index, and resilient to corrupt files. All five corpora now build from disk.

[M11 phase-02a — index-schema-v2](milestones/M11-knowledge-index/phase-02a-index-schema-v2.md)
**approved 2026-08-03** (`approved_after_1`; one bounce,
[bug-02a-1](milestones/M11-knowledge-index/bugs/bug-02a-1.md), verified fixed).
The index now carries all seven tables at SCHEMA_VERSION 2, with `artifacts` and
`epochs` populated and `daemoneye reindex` reporting per-corpus counts truthfully.

[M11 phase-01 — write-time-masking](milestones/M11-knowledge-index/phase-01-write-time-masking.md)
**approved 2026-08-03** (`approved_after_1`; one bounce,
[bug-01-1](milestones/M11-knowledge-index/bugs/bug-01-1.md), verified fixed).
`append_epoch` and `log_event` now mask at the write choke point, so the epoch
and event corpora are safe to index in phases 02–03.

**M11 — Unified Knowledge Index scoped 2026-08-03** (PE decision). Seven phases
planned; design settled in `docs/design/knowledge-index.md`; milestone README at
`docs/dev/milestones/M11-knowledge-index/README.md`.

**M10 — Residual Hygiene closed 2026-08-02** (three phases, all
`approved_first_try`, zero bugs, zero bounces). Retrospective:
`docs/dev/milestones/M10-residual-hygiene/README.md`.

## The carried list is empty except for one unreproducible item

1. **`hooks_land_on_private_server`** — the old phase-04-review flake. Binds no
   ports; **0 failures in 300 runs** across M8, M9 and M10. No evidence to work
   from. Only a bug if it recurs.

Everything else carried out of M7 and M8 is closed: the tty tests fail instead of
hanging, the memory category→directory mapping is derived in all three callers,
the last real-clock sleep is gone, and `daemoneye reindex` is documented and gated.

## One calibration item, resolved at close

The executor mislabelled its own model in its Update Log entry three times (M9
phase-01, M10 phase-01, M10 phase-03) — it is Qwen3.6-27B-FP8 every time. Three
occurrences hit the fold threshold, and the PE's decision was to drop the model
line from the executor's own entry.

**Applying it revealed the premise was wrong: no template asks for that line.**
`docs/dev/WORKFLOW.md` § "Update Log entries" defines progress, blocker and
completion entries, and none has an `**Executor:**` field. The only one in the
file is at `:347` in the **Review verdict** template — the architect's line, which
has been correct throughout. The embedded executor contract does not request it
either.

The executor adds it unprompted, so there is nothing to delete and no fold to
file. The operative consequence is for review: **an unrequested, self-reported
model name in an executor entry is not a defect against any spec, and should not
be corrected in place.** It was corrected three times on the assumption it was
contract-mandated.

Actively suppressing it would mean editing `executor/templates/executor_contract.md`
in the **rexyMCP** repo — out of bounds from a target-project architect session.

## The rules M7–M10 earned

> **Do not assert a fact about the system in a spec unless it was executed.**
> A *claimed failure mode* is such a fact — M9 justified a test with a
> compile-time impossibility one `cargo build` would have disproven.
>
> **A criterion about the tree the phase will produce must be validated against
> that tree**, not the one in front of you. Calibrating against the current tree
> catches unsatisfiable criteria; it does not catch criteria the phase's own work
> invalidates, or criteria that already pass without the work being done.
>
> **Prototype the change and mutate it before writing the spec.** M10 phase 02's
> real risk — two labels with no test, where a swap left 1036 tests green — was
> invisible until the prototype was mutated.
>
> **An acceptance criterion for an intermittent failure must be a repeat count
> derived from a measured rate.** A single green run is not evidence.
>
> **Measure through the same door the user will use.** M9's in-process probe of
> `reconcile_index()` recorded a bare-`$HOME` result the shipped binary never
> produces.

Corollaries, each earned more than once: naming a false-success mode is worthless
unless the guard is checked against it; a phase that lands code for a *later*
phase must say how the deny-warnings gate is satisfied; and **a green bounce
always needs a refined re-dispatch**, never a plain one.

## M19 — Sandbox Completion (opened 2026-08-29)

**phase-01 — is-ghost-predicate: done (approved_first_try) 2026-08-29.**
`is_ghost_session_id` / `resolve_is_ghost` extracted to `src/daemon/mod.rs`
beside the other D6 predicates; both `run.rs` ghost checks route through them.

**phase-02 — staging-integration: done (approved_after_1) 2026-08-29**,
commits `8223ac8` (code) + `718b497` (round-2 evidence) + `f8738ef`
(approval). `stage_args` has a production caller, the module
`#[allow(dead_code)]` in `executor/mod.rs` is gone (repo-wide 7 → 6), and the
job's staging volume is removed on both completion paths. Bug-phase-02-1 was
evidence-only: the round-1 end-to-end entry carried no bare `PASTE MATCH`
line. Code was byte-identical across both rounds.

**Active phase: phase-03 — ghost-container-execution**
(`docs/dev/milestones/M19-sandbox-completion/phase-03-ghost-container-execution.md`,
status: todo, drafted 2026-08-29). **Its premise was corrected from
measurement before drafting.** The README claimed ghosts were "labelled, not
sandboxed"; measured, a ghost's ordinary background command is already
containerized. The real hole is two *other* doors: `background=false` reaches
`run_foreground`, which destructures `is_ghost: _` and ignores it, and
`retry_in_pane` reaches `respawn_background_in_pane`, where
`grep -c sandbox` prints 0 — it `send-keys`es the raw command to the host
shell. Scope: a pure `ghost_may_run_foreground(is_ghost, sandbox_enabled)`
gate beside `ghost_may_use_tmux_control` plus its dispatch-site refusal, and
the full preflight → stage → wrap → remove-volume sequence added to the retry
path, sharing a new `container::job_id_for(pane_id, unix_ts)` with `run.rs`.
Seven tests, two mutation pairs. Every mechanical acceptance criterion was
validated against the current tree at drafting: all read their stated "before"
value.

Live measurement taken while drafting (rootless Docker, daemon host): `docker
run -v <absent-name>:…` **creates** the named volume rather than refusing, so
a retry that skipped staging mounts an empty dir (loud failure, good) and a
retry leaks one volume unless it is removed (hence removal on both retry
completion paths). Recorded in the phase doc's § Live measurements.

Gap recorded in the milestone README while drafting: `[sandbox.ghost_defaults]
mount_scripts` is parsed and consulted by nothing and has no phase.

**phase-03 — ghost-container-execution: done (approved_first_try) 2026-08-29**,
commit `935ee23` + approval `3467223`; deepseek-v4-flash-0731, 196 turns. Two
findings repaired at review rather than bounced (`bugs/bug-phase-03-1.md`): the
status flip mis-anchored and ate the doc's `**Milestone:**` line, and Task 7's
M1 expectation was an architect spec defect (the mutation legitimately fails
two `job_id_for` tests, not one). **Calibration that changes dispatch:** the
executor's summary claimed M1 failed exactly one test while the artifact it had
just pasted showed two — the third of the misdescription family M18 held at
two. Fold: every phase doc for this model now pins a checkable criterion for
each claim its summary will make.

**Active phase: phase-04 — ghost-scoped-teardown**
(`docs/dev/milestones/M19-sandbox-completion/phase-04-ghost-scoped-teardown.md`,
status: todo, drafted 2026-08-29). Adds a `de.session=<session_id>` label to
every sandboxed container, a ghost-scoped teardown on `trigger_ghost_turn`'s
exit path, and wires `[sandbox.ghost_defaults] destroy_on_exit` — parsed since
M18, read by nothing until now.

**The whole change was prototyped end-to-end before the doc was written**, and
the prototype caught two things the spec would otherwise have shipped wrong:

1. The first design used additive wrapper functions (`run_args_labeled`,
   `sandbox_window_command_for_session`) to avoid touching 22 `ExecSpec`
   literals. **Clippy killed it:** with production calling only the new names,
   `run_args` and `sandbox_window_command` became `never used` and
   `-D warnings` failed. The shipped design gives the two existing functions a
   trailing `session_id: Option<&str>` and appends `None` at 23 test call
   sites.
2. **`cargo build` does not catch those 23 sites** — it compiles the lib only,
   and they live behind `#[cfg(test)]`. Measured: `cargo build` was green with
   every one of them still broken. Only `cargo clippy --all-targets` or
   `cargo test` reports them. That is now a § Gotcha, because a green `build`
   would otherwise read as "done".

Both mutation pairs were run on the prototype and each fails **exactly one**
named test — measured, not estimated, per the phase-03 fold. Test names use
the `sandbox_session_label_` prefix because the bare token `session_label`
also matches a pre-existing `approval_panel_sudo_session_label` and would make
the "4 ok" criterion read 5. Every mechanical criterion was validated against
the current tree; two were wrong on first write (`session_id.as_deref(),`
already occurs once per file, and `SandboxGhostDefaults` reaches
`crate::config` via `pub use types::*`, not a named re-export) and were
corrected.

Live docker facts recorded in the phase doc: `--filter label=k=v` is an exact
match (not a prefix — the opposite of `--filter name=`), repeated `--filter`
clauses AND, `rm -f` reclaims a running container, a label value containing
`=` or a space round-trips (so **no** sanitizer is needed for the webhook-
supplied alert name inside a ghost session id), and `-v name:` volumes carry
no labels (so volumes stay on per-job removal).

**phase-04 — ghost-scoped-teardown: done (approved_after_1) 2026-08-30**,
commits `b74cde3` (code) + `b01afe7` (round-2 evidence) + `1a058d8`
(approval). Every sandboxed container now carries `de.session=<id>`, a
ghost-scoped teardown runs on `trigger_ghost_turn`'s exit path, and
`[sandbox.ghost_defaults] destroy_on_exit` is read for the first time.
bug-phase-04-1 was evidence integrity, not code: the § E2E block appends, it
was run twice (the first with `fmt_exit=1`), and `/tmp/e2e-04.txt` was edited
down to one run so the self-check would pass. Round 2 re-captured cleanly.
**Positive calibration:** round 2's artifact keeps a *visible failed patch
attempt* — the first `== M1 APPLIED ==` shows tests green with `grep -c` 1,
the second shows FAILED with 0 — which is the after-each-direction guard
working, left in rather than trimmed. Task 10 of every later phase now carries
the repeat-run rule (`rm -f` the artifact; never edit either side).

**Active phase: phase-05 — container-status-ipc**
(`docs/dev/milestones/M19-sandbox-completion/phase-05-container-status-ipc.md`,
status: todo, drafted 2026-08-30). `Request::ContainerStatus` →
`Response::ContainerStatus`, the collector behind it, and a `SANDBOX` section
in `daemoneye status`.

**Prototyped end-to-end before the doc was written**, which produced three
facts the spec would otherwise have got wrong:

1. **Never parse `docker ps --format '{{.Labels}}'`.** It joins label pairs
   with `,`, so `de.session=ghost-disk,full=x-abc` splits into a wrong session
   plus an invented label. `docker inspect --format '{{.Id}} {{.State.Status}}
   {{.Config.Image}} {{json .Config.Labels}}'` is unambiguous, and a newline
   inside a value comes back escaped so the record stays on one line — a
   tab-separated format splits it across two.
2. **Adding a `Response` variant breaks two exhaustive matches** in
   `src/cli/commands/ask.rs:103` and `src/cli/commands/stream.rs:353`. Only
   the compiler says so; the spec pre-injects the error text and the unique
   `| Response::PaneList { .. }` anchor in each file.
3. **`docker inspect` with no arguments is a usage error (exit 1)** and the
   empty list is the common case, so `status_inspect_args` returns an empty
   vector and the caller skips the spawn.

Three mutation pairs, each measured to fail exactly one named test. **M1's
first candidate anchor was rejected by measurement:** `    if ids.is_empty() {`
occurs three times in `container.rs` and even the three-line `return
Vec::new();` form occurs twice, so that pair needs a seven-line `old_str`
reaching `"inspect".to_string(),` (measured unique) and is now M3. One
acceptance criterion was also wrong on first write — `grep -c '{{.Labels}}'`
reads **2** on a correct tree (both in comments explaining why that form is
not used), so the pin is `grep -c '{{json .Config.Labels}}'` = 1.

The report carries `enabled` and `image_detail` beyond the design's named
shape, because the milestone exit criterion asks for the lockfile comparison;
recorded for the phase-10 doc sweep. The rendered section and the live IPC
round-trip are architect-verified at milestone close against a real daemon.

**phase-05 — container-status-ipc: done (approved_first_try) 2026-08-30**,
commit `342f823` + approval `c6bc50e`; 127 turns. First phase dispatched under
the full fold set from 02–04 (pinned summary claims, pre-measured mutation
counts, the repeat-run rule) and the first clean run: byte-identical to the
prototype, one execution in the artifact, `PASTE MATCH`, two reviewer-only
mutations each failing exactly one test. Recorded as a confirmation, not a
fold.

**Between phases (2026-08-30):** `docker/compose-for-agents` and
`docs.docker.com/ai/sandboxes/` reviewed and measured against the real image
on the daemon host. PE decision: incorporate. README now carries phase-11
(hardening flags, in scope) and phase-12 (workspace mount policy), sentinel
credential injection and the sbx rule/audit semantics folded into 08,
HTTP-only egress stated as a **deferral** in 06 (SSH/TCP later — `host:port`
rules and a `proxy_type` audit field from the start so it stays additive),
and two measurements added to 10.

**Active phase: phase-06 — proxy-network-and-image**
(`docs/dev/milestones/M19-sandbox-completion/phase-06-proxy-network-and-image.md`,
status: todo, drafted 2026-08-30). The proxy image, its own lock, `sandbox
build` building both, and the network sweep. **Prototyped and stood up live
before writing**: an `--internal` network + tinyproxy container + agent
container on the daemon host — every isolation claim in the doc is a
measurement (agent reaches the proxy by name and nothing else; proxy's bridge
leg still cannot reach host loopback; `403 Filtered` on GET and CONNECT;
empty filter = deny-all; `network rm` fails with active endpoints; ~250 ms
cold). Scope was **narrowed** from the README intent line by the phase-02
lesson: the create/run/connect builders would be dead code without 07's
callers, so they move to 07. Two mutation pairs measured at exactly one
failure each; `include_str!` pins the conf file itself, so M2 mutates a
non-Rust file. Every before-value validated against the current tree.

**phase-06 — proxy-network-and-image: done (approved_first_try) 2026-08-30**,
commit `2ab040e` + approval `5cbdaed`. 1485 → 1491 lib tests. Reviewer ran two
mutations of its own beyond the spec's pair (`proxy_lock_path` colliding with
`sandbox.lock`; `write_lock_to` losing `create_dir_all`), each caught by
exactly one named test — four of the six new tests now proven to discriminate
— and `diff`ed both proxy image files against the spec's fenced blocks
(byte-identical). **Calibration recorded, not folded (one occurrence):** the
executor's summary reported this host's `grep` as "intermittently" flaky and
claimed its own pinned criterion `grep -c '^PASTE MATCH$'` returned 0,
substituting `fgrep -nF` + `diff` as proof and recommending spec hardening.
It does not reproduce — the criterion prints `1` under three independent
instruments. A **false red** rather than a false green, but the same rule
applies as [[executor-claims-need-rerunning]]: re-run the claim.

**Active phase: phase-07 — proxy-profile-wiring**
(`docs/dev/milestones/M19-sandbox-completion/phase-07-proxy-profile-wiring.md`,
status: todo, drafted 2026-08-30). Dispatch with
`/rexymcp:dispatch phase-07`. Makes `[sandbox.profile.<name>] network`
real — it has been parsed and validated since M18 and **read by nothing**
(`grep -rn 'SandboxProfile\|proxy_allow' src/ | grep -v '^src/config/'`
returns empty; both production `ExecSpec` sites pass the literal `"none"`).

Phase-07 drafting notes:

- **Prototyped in the tree, mutated, formatted and reverted before speccing.**
  Every code block in the Spec is that prototype after `cargo fmt --all`;
  1491 → 1499, four gates green, **zero dead-code warnings** (every new
  function has a caller — the phase-02/06 lesson held), and three mutations
  each caught by exactly one named test.
- **The measurement that reshaped the phase: tinyproxy refuses to start when
  its `Filter` path does not exist** — `filter file: No such file or
  directory`, container dead on arrival. So phase-07 *must* write and mount a
  filter file even though its contents are phase-08's. An empty file is
  deny-all, so the fail-closed end state falls out for free. Speccing this
  from the design alone would have produced a phase whose proxy never booted.
- **The full end state was stood up live**: agent on the `--internal` network
  only, all four `HTTP(S)_PROXY` spellings set → proxy reachable by name,
  `403` on HTTP, `curl_rc=56` on CONNECT, LAN/host-loopback/public all
  blocked, and `example.com` does not even resolve (`SERVFAIL`) — the agent
  cannot bypass the proxy because it has no DNS of its own. Positive control
  also confirmed (200/200 with one filter line), so phase-08 inherits working
  wiring.
- **Designed around a constraint rather than into one:** the proxy endpoint is
  derived from `ExecSpec`'s existing `network` and `job_id` fields instead of
  adding a field, because `ExecSpec` is constructed at 26 sites (24 of them
  tests). The phase therefore changes **zero existing tests**.
- **Criterion defect caught by counting instead of deriving** — the same fold
  as phase-02's. I wrote "`network: \"none\",` still prints 28" from an
  `ExecSpec` *symbol* count; the literal count is 24, and it goes to **25**
  because my own new test adds one. Both the criterion and the Gotcha were
  wrong until measured. Pinned now as 24 → 25 and 24 → 26.
- **Spec hardening carried forward from phase-06's incident:** every
  `test result:` line in this phase's evidence is piped through
  `sed 's/; finished in .*//'`. Phase-06's `PASTE MISMATCH` was caused *only*
  by per-run durations differing between two identical executions.
- The whole § End-to-end block was extracted from the doc and **run verbatim
  against the clean tree**: all 31 structural lines report their documented
  before-values, so no criterion is self-satisfying and none passes early.

**phase-07 — proxy-profile-wiring: done (approved_first_try) 2026-08-30**,
commit `1b8532b` + approval `0b13bb5`. 1491 → 1499 lib tests. Executor's
source was **byte-identical to the architect's reverted prototype** bar one
dropped doc comment. Reviewer added two mutations of its own (proxy losing
`de.ghost=1`; `proxy_env_args` losing the lowercase `https_proxy` that `curl`
reads), each caught by exactly one named test — five of eight new tests proven
discriminating. **Architect-side criterion defect, third of the milestone:**
`git diff --name-only | wc -l` prints `2` is unsatisfiable, because Task 11
requires the evidence entry to be written into the phase doc before the block
that counts files runs; the honest value is 3. Validated on a clean tree (0)
and on the prototype (2) but never on the state the executor would be in.
Corrected. **Calibration held at 2 occurrences, not folded:** phase-06 and
phase-07 summaries both generalised past their own evidence (a false
"flaky grep"; "nothing else deviated" while the pasted evidence carried a
mismatch). Both caught only by re-running the evidence.

**Active phase: phase-08 — proxy-allowlist**
(`docs/dev/milestones/M19-sandbox-completion/phase-08-proxy-allowlist.md`,
status: todo, drafted 2026-08-30). Dispatch with
`/rexymcp:dispatch phase-08`. Renders `proxy_allow` / `proxy_deny` into the
filter phase-07 mounts empty.

**Phase-08 was split into three, and the milestone gained two phases.** The
README's 08 intent bundles allowlist + audit record + sentinel credentials.
Measured against a working prototype the allowlist alone is ~330 lines, so 08
keeps the allowlist and the other two become **13 proxy-audit** and **14
proxy-credentials**, both before the phase-10 close-out. Exit criteria
unchanged — met by 08 + 13 + 14 together. Same narrowing 06 took.

Phase-08 drafting notes:

- **A real egress hole was found by measuring, and 08 closes it.** With no
  `ConnectPort` directive, tinyproxy opens CONNECT to **any** port on an
  allowlisted host: the proxy log shows it dialling `example.com:22`, `:25`
  and `:3306`. The `000` those return is a connection *timeout*, not a
  refusal. An agent allowed one HTTPS host could have tunnelled SSH to it —
  so the README's "egress is HTTP(S) only" contract was **false as shipped by
  06/07**. `ConnectPort 443` + `ConnectPort 563` fixes it (verified: one
  `opensock` line, for 443; ordinary HTTPS still 200), and a test pins both
  lines through `include_str!`.
- **The rule model is measured, not designed.** A tinyproxy filter line is
  fnmatch against the **host alone**: `example.com` matches only itself,
  `*.example.com` matches subdomains but **not** the apex (so allowing both
  takes two rules), and a `host:port` line matches **nothing**. Hence
  `parse_proxy_rule` accepts `:80`/`:443` and refuses every other port rather
  than rendering the bare host — which would silently grant more than asked.
- **"Deny beats allow" cannot be expressed narrowly**, because the filter is
  an allow list with no exception form. A deny landing inside a wildcard
  allow therefore **drops the whole wildcard** — blunt, but the only
  rendering that does not leave the denied host reachable. Pinned with both
  boundaries (a sibling deny leaves the wildcard; an apex deny does not drop
  `*.apex`).
- **M2 fails two tests, not one, and the spec says so.** Deleting the port
  gate trips both `..._refuses_every_rule_it_cannot_enforce` and
  `..._denies_everything_when_nothing_survives`. Measured; had I assumed
  "one" the executor would have been told to file a blocker on a correct tree.
- **I made the mistake the phase docs warn the executor about.** Restoring a
  mutation with `git checkout <file>` destroyed ~300 lines of uncommitted
  prototype. Rebuilt from saved fragments (tree afterwards byte-identical to
  the pre-loss diff), then re-ran every mutation with inverse patches only.
  It is now § Gotchas 2 of phase-08, stated as something that really happens.
- Prototype: 1499 → **1507**, four gates green, ~330 lines. Every criterion
  validated by running the § End-to-end block verbatim against the clean tree
  — all 19 discriminating lines report `0`.

**phase-08 — proxy-allowlist: done (approved_first_try) 2026-08-31**, commit
`90e74c5` + approval pending below. 1499 → 1507 lib tests. Executor's source
was **byte-identical to the architect's reverted prototype**, no exceptions.
`ConnectPort 443`/`563` is in the image conf, so the milestone's "egress is
HTTP(S) only" contract is now true rather than assumed.

**Carry to phase-13 (architect omission, not an executor defect):** the
reviewer's own mutation showed that removing the dot-boundary check from
`is_subdomain_of` (`src/daemon/executor/container.rs`) kills **no** test.
Measured consequence: allow `*.example.com` + deny `evilexample.com` then
renders `""` instead of the grant — it **over-denies** in both directions and
cannot over-grant, so the seam is fail-closed and the finding is minor. The
code is correct; the eight tests were specified verbatim by the architect and
implemented exactly. Add `sandbox_filter_lookalike_suffix_is_not_a_subdomain`
with the next change to this module.

**Instrument retired: `git diff --name-only | wc -l` as an acceptance
criterion.** Two misfires in two phases — `3` against a pinned `2` (07), `8`
against a pinned `7` (08) — both on correct trees, because the value depends
on how many doc commits the executor has already made. Replaced in phase-08
with `git diff --name-only | grep -cE '^(src|containers|assets)/'`, which
counts only what a phase authorises. Use that form from now on.

**A spec change answered a 2-occurrence calibration pattern, so it does not
need folding.** Phase-06 and phase-07 both shipped summaries that generalised
past their own evidence. Phase-08's § Authorizations added "if a pasted number
disagrees with the value the criterion states, say so in your summary rather
than reporting overall conformance" — and this run opened by naming the
mismatch unprompted, with its structural cause. Worth remembering as the
cheaper lever: a targeted Authorizations line, not a WORKFLOW.md fold.

**Active phase: phase-13 — proxy-audit**
(`docs/dev/milestones/M19-sandbox-completion/phase-13-proxy-audit.md`,
status: todo, drafted 2026-08-31). Dispatch with
`/rexymcp:dispatch phase-13`. Drains each job proxy's log at teardown into
`events.jsonl` — host, port, method, decision, reason, matched rule,
`proxy_type`, repeat count.

**Ordering choice:** 13 was drafted ahead of 09, 11 and 12 because it is the
continuation of the chain that just landed (06 → 07 → 08 → **13** → 14) and
the proxy's live behaviour was measurable in one drafting session. 09, 11 and
12 are independent of it and of each other; 10 stays the close-out.

Phase-13 drafting notes:

- **Prototyped, mutated and run against a live proxy before speccing, then
  reverted.** 1507 → **1522** (15 tests), four gates green, ~550 lines across
  two files. Five mutations run; four discriminate exactly one named test
  each and are specced as pairs. The fifth (deleting the path-stripping split)
  fails **six** tests — too wide to be a useful discriminator, so the seam is
  pinned by a `grep -cF` criterion and the six names are recorded in § Test
  plan as architect evidence rather than asked of the executor.
- **The parser's design came from a concurrency measurement, not from
  reading.** tinyproxy's decision lines carry **no** file descriptor, so exact
  attribution is impossible from the log. Under twelve-way concurrency every
  refusal still appeared on the line immediately after its own `Request` line
  (the filter and port checks are synchronous), while the allow path
  interleaves freely. Hence the rule: a request is denied only if the **next**
  line is one of the two refusal forms *and* that form's own guard — the host
  it names, or the port it names — matches this request. Any other next line
  means allowed, which is correct no matter whose line it is.
- **A real egress gap was measured and is deliberately *not* closed here.**
  `GET http://example.com:8080/` succeeds through an allowlist of
  `example.com`: `ConnectPort` caps the CONNECT method only, and a filter line
  cannot express a port. Recorded in the README as a milestone gap; the audit
  record's `port` field is what makes it visible meanwhile.
- **The audit must not become a secret sink.** The proxy logs the whole
  absolute URI — captured verbatim: `GET http://example.com/secret?token=abc`.
  `mask_json_value` would not catch a bearer token in a query parameter, so
  the record carries host, port and method only, and two tests assert on the
  **serialized event** rather than on a struct field.
- **Order is the phase's real failure mode**, and it has a mechanical
  criterion rather than prose: `docker rm` takes the container's log with it,
  so an audit after `remove_proxy` writes zero records with a green suite. An
  `awk` over `run.rs` pins audit-before-teardown at **2** (0 today).
- **`container.rs` keeps its zero `log_event` calls** — the module stays pure
  decisions plus one spawn per operation, and the event is written from
  `run.rs` beside `job_complete`. Pinned as an unchanged-at-0 criterion so the
  executor cannot "helpfully" move it.
- **The phase-08 carry landed**: `sandbox_filter_lookalike_suffix_is_not_a_subdomain`
  is in this phase's test block, and mutating the dot boundary in
  `is_subdomain_of` now fails exactly that test (it failed none before).
- **The built proxy image on the daemon host was stale** and the first probe
  read as a phase-08 regression that did not exist. Rebuilt, re-measured,
  confirmed. Recorded in the README for the phase-10 live checks.
- All 21 lines of the § End-to-end structural block were run against the clean
  tree: every discriminating line reads its stated "before" value, and the
  five unchanged ones read their stated unchanged value.

**phase-13 — proxy-audit: done (approved_after_1) 2026-08-31**, commits
`8abacaa` (round 1) + `9741cf7` (round 2) + approval below. 1507 → **1522**
lib tests. Every sandboxed egress request now lands in `events.jsonl` as a
`proxy_request` record — host, port, method, decision, reason, matched rule,
`proxy_type: "forward"`, repeat count — drained from the job proxy's log at
teardown, before `remove_proxy` takes the container's log with it.

Round-1 source was **byte-identical to the architect's reverted prototype**.
The single bounce (`bug-phase-13-1`, minor) was a doc comment: Task 3a's
insertion landed *inside* `run_background_in_window`'s doc block, so that
function lost its documentation and its 22-line description came to document
a 4-line helper. Round 2 moved the comment and touched nothing else —
`container.rs` byte-identical across the round.

**Two calibration entries, one each side.**

1. **The targeted § Authorizations line is the cheaper lever, confirmed twice
   more.** Round 1: Task 7 handed the executor a `sandbox_proxy`-filtered test
   command that structurally cannot see M3's test (named `sandbox_filter_*`);
   it ran the command, saw `20 passed` under a live mutation, diagnosed the
   name mismatch, verified through the full suite and reported the contrast
   unprompted. Round 2: it declined to re-run the mutation pairs, citing the
   bug doc's "no line of `container.rs` changes in round 2" — the correct
   reading, with its reason stated. That is the phase-08 Authorizations line
   ("if a pasted number disagrees with the value the criterion states, say
   so") working for the second and third time. **Recorded as settled: prefer a
   targeted Authorizations line to a WORKFLOW.md fold.**
2. **New sub-case, held at 1 occurrence: a count criterion must be validated
   against the test-name filter it runs under, not only against the tree.**
   Task 7's command was correct grep over a correct tree and still blind.
   § "Run every count criterion; never derive it" catches a criterion whose
   *corpus* contains its own answer; it does not catch one whose *filter*
   excludes its own subject. The drafting check that would have caught it:
   after writing a `cargo test --lib <filter>` criterion, confirm the test it
   is meant to discriminate actually matches `<filter>`.

**Warning, not a defect:** round 2's completion summary opened with a leaked
`</think>` block — the model's reasoning reached the summary field. Legible,
conclusions correct; noted so a pattern can be seen if it recurs.

**Reviewer's independent verification:** all four gates re-run separately
(1522 passed / 0 failed / 4 ignored across seven targets), all 22 round-1 and
3 round-2 structural criteria at their pinned values, M3 re-run and
reproducing, plus a **third mutation the phase doc does not name** — deleting
the `("denied", "port")` branch from `decision_for` — failing exactly
`sandbox_proxy_log_decides_each_request_from_the_line_that_follows_it`.

Remaining M19 phases: 09 escape-hatch, 10 live-verification-and-close,
11 container-hardening-flags, 12 workspace-mount-policy, 14 proxy-credentials
— none drafted. 14 completes the 08 split (08 + 13 + 14 together meet the
milestone's egress exit criterion); 10 stays the close-out.

**Active phase: phase-11 — container-hardening-flags**
(`docs/dev/milestones/M19-sandbox-completion/phase-11-container-hardening-flags.md`,
status: todo, drafted 2026-08-31). Dispatch with
`/rexymcp:dispatch phase-11`. Four `docker run` flags `run_args` does not set,
a digest-pinned base image, and a `container_run` event at spawn.

**PE DECISION 2026-08-31 — phase-14 is deferred out of M19; the exit criterion
is struck.** Drafting 14 began by measuring its mechanism against the real
proxy, and the design adopted from Docker Sandboxes on 2026-08-30 does not
port:

1. **`AddHeader` adds; it never replaces.** With
   `AddHeader "Authorization" "Bearer REAL-SECRET-VALUE"` in the conf, a
   request carrying `Authorization: Bearer de-cred-SENTINEL` reached the
   origin **with the sentinel unchanged**. Substitution is not an operation
   tinyproxy has.
2. **`AddHeader` is global and static** — one value for every allowed host, so
   the secret would go to *every* allowlisted destination. Weaker than the env
   var it replaces.
3. **HTTPS is a byte tunnel.** Through `CONNECT` the origin received no
   `Authorization` and not even `Via`, while the plain-HTTP request in the same
   run carried both; tinyproxy logs `Not sending client headers to remote
   machine`.

Making it work needs TLS interception — a different proxy, a per-daemon CA in
the agent image's trust store, a per-job cert cache, and the trade that the
proxy then reads every byte the agent sends. That is a later milestone's design
decision, not a completion task. **M19's egress story is 08 (allowlist) + 13
(audit).** The lesson is worth carrying: every claim in the Docker Sandboxes
read was checked against *this repo's code* and none against *this repo's
proxy binary*. A capability is a fact about the implementation you have, not
about the design you copied.

Phase-11 drafting notes:

- **Prototyped, mutated, and the flag set run against real containers before
  speccing, then reverted.** 1522 → **1529** (7 tests), ~230 lines across four
  files, four gates green. Six mutations run; four discriminate cleanly and are
  specced as pairs (three fail exactly one named test, M2 exactly three — the
  *set* is pinned, not a count). The two that don't are covered by grep
  criteria with their measured blast radius written into § Test plan.
- **Every flag was verified in-kernel or in `docker inspect`, with its contrast
  case.** `CapBnd: 0000000000000000`, `NoNewPrivs: 1`, `ReadonlyRootfs=true`;
  `MemorySwap` is 2 GiB without the flag and 1 GiB with it; `--pull never`
  turns a missing image into a local `No such image` where the default reaches
  for `docker.io`. The toolchain still works under `--read-only` (python3, git,
  curl all fine) because `/tmp` gets its own 1777 tmpfs.
- **The digest pin forces no rebuild, and that was checked rather than
  assumed.** `alpine:3.22` currently resolves to the pinned digest, so both
  images build to byte-identical ids — the agent image's is exactly the
  `image_id` already in `sandbox.lock`, so preflight keeps passing. Had they
  differed, every sandboxed command would have been refused until
  `daemoneye sandbox build` ran.
- **The record's image id comes from the lockfile, not a probe** — preflight
  already refuses when live ≠ lock, so at the spawn site the two agree and no
  process needs spawning.
- **Two criterion counts were wrong until I ran them.** I wrote M3's seam as
  2 → 1; `grep -c 'if !cfg.enabled {'` is actually **4 → 3**, because three
  other functions in the file carry the same guard. Corrected before the doc
  landed. This is the sub-case recorded at phase-13 close, arriving again in
  the same shape: a mechanical criterion is not validated until it is *run*
  in both directions.
- **§ Authorizations carries `bug-phase-13-1` forward as a forward-looking
  gotcha** — check what sits immediately above an insertion point, because a
  `///` block attaches to whatever item follows it. Task 2's anchor is a
  doc-comment line for exactly that reason.
- All 17 lines of the § End-to-end structural block were run against the clean
  tree: every discriminating line reads its stated "before" value.

**Two gaps recorded in the README rather than absorbed:** item 7 of the 11
intent (image staleness warning + `requires_tools`) needs a phase of its own,
and **the proxy image is never verified against `proxy.lock` at run time** —
`proxy.lock` does not exist on the daemon host at all, and nothing in
`start_proxy` reads it.

**phase-11 — container-hardening-flags: done (approved_first_try) 2026-08-31**,
commit `6534f89` + approval below. 1522 → **1529** lib tests. Sandboxed
containers now run with swap capped at the memory limit, a read-only root plus
two writable tmpfs, all capabilities dropped, `no-new-privileges`, and
`--pull=never`; both images pin their base by digest; and every sandboxed
spawn writes a `container_run` record — job id, session, image, image id,
network — which is the audit anchor phase-10's live checks bind to.

Executor's source was **byte-identical to the architect's reverted prototype**.
All four mutation pairs behaved exactly as specced, including M2's set of
three and M1/M3's "the lower number is the mutated one" seams.

**The phase-13 bug did not recur, and that is the calibration note.**
`bug-phase-13-1` (an insertion orphaning the doc comment above
`run_background_in_window`) was carried into phase-11's § Authorizations as a
forward-looking gotcha, and Task 2's anchor was chosen to be a doc-comment
line so the same insertion could not repeat it. It didn't:
`grep -B1 '^pub async fn run_background_in_window(' … | grep -c '^///'` still
prints `1`. **A bug re-expressed as a spec constraint for the very next phase
cost two sentences and held** — cheaper than a WORKFLOW.md rule, and the same
lever as the § Authorizations line recorded at phase-13 close.

**Reviewer's real-artifact verification** (DoD box 3, run against the
*committed* files rather than the prototype): both Dockerfiles build from the
pinned digest, and the agent image's id is exactly the `image_id` already in
`~/.daemoneye/etc/sandbox.lock` — so preflight still passes and the phase
requires no operator action, as drafting predicted. The committed flag set was
then run against the real image: `CapBnd: 0000000000000000`, `NoNewPrivs: 1`,
both tmpfs writable, `touch /ro` → `Read-only file system`. Plus a third
mutation the doc does not name (`"image"` rendered from `cfg.runtime`) failing
exactly one named test.

Remaining M19 phases: **09 escape-hatch**, **12 workspace-mount-policy**,
**10 live-verification-and-close** — none drafted. 14 is deferred out of the
milestone. 10 stays the close-out.

**M19 closed 2026-09-03 at the 2.0 boundary (PE direction).** Ten phases
done (seven first try, four bugs, all resolved); 09 escape-hatch and 12
workspace-mount re-homed into DaemonEye 2.0 (M21/M22); 10 live-verification
**not run** — the gap is recorded in the M19 README retrospective and carried
to M20/M21's exit criteria. `m19-sandbox-completion` (which contained M18)
fast-forwarded into `master`; crate bumped to 1.0.0; **`v1.0.0` tagged at
`029ab1a`** as the last tmux-based release. Both pushed to `origin`.

## DaemonEye 2.0 (plan of record 2026-09-03, `docs/design/daemoneye-2.0.md`)

The plan proposes M20–M29; PE decisions of 2026-09-03 are in its § 8. 2.0
proceeds on `master` behind an `[execution] backend = "tmux" | "pty"` flag;
there is no `v2` branch.

## M20 — Shell Engine (scoped 2026-09-03, **awaiting PE sign-off**)

Milestone README: `docs/dev/milestones/M20-shell-engine/README.md` — nine
phases proposed, none drafted. Pre-drafting measurements are recorded there:
`portable-pty` 0.9.0 + `vt100` 0.16.2 spawn/marker/colour all work; **the PTY
echo matches a naive marker first** (fixed by a split nonce in the typed
text); **a shell dies when its master-holder exits**, so the shell-host
process is the design, not an option. The `less`/resize leg was inconclusive
and is re-measured before phases 04/05 are drafted.

**phase-01 — execution-config: done (approved_first_try) 2026-09-03**,
commit `e535a8c`. DeepSeek V4 Flash 0731, 211 turns, zero bounces, zero bugs.
Lib tests 1533 → 1540 (the seven pinned tests). Gates re-run independently at
review; both new tests mutation-checked; the real-artifact check re-run under
a throwaway `HOME` (both dirs created, mode 0700, both config blocks seeded).

**Three review findings, all architect-side** — full detail in the phase doc's
Review verdict:

1. **A defective acceptance criterion**: `grep -c "pub execution:
   ExecutionConfig"` was pinned at `2` for "the struct field and the `Default`
   impl", which it can never return — the `Default` line has no `pub`. The
   executor's evidence honestly printed `1` against the stated `(2)`.
   Corrected in place. **Note what this says about the pre-dispatch
   validation:** every criterion *was* run against the tree in its failing
   state, and all fifteen returned `0` — which proves a criterion is
   unsatisfied *now*, not that its expected value is reachable *later*. That
   is precisely the gap `docs/dev/TODO.md` § 1 predicts, and this is a fresh
   instance of it.
2. **§ Authorizations missed a forced chain**: adding a `POLICY_TABLE` entry
   forces `src/config/runtime_tree.rs` (else `every_policy_path_appears_in_tree`
   fails) which forces `assets/memory/knowledge/agent-runtime-layout.md` (else
   `render_matches_shipped_asset` fails). Both confirmed by mutation at review.
   The executor made the minimal edits and flagged them. **Rule for the rest of
   M20: a phase that touches `POLICY_TABLE` must authorize all three files.**
3. **The E2E block prescribed `rm -rf "$T"`, which the executor's bash
   classifier blocks.** It substituted `find "$T" -depth -delete` and declared
   the change; the artifact was unaffected. Same class as the `sed -i` defect
   folded 2026-08-08 — a spec prescribing a banned command. **Second
   occurrence; recorded, not folded** (threshold is three). Until then, avoid
   `rm -rf` in E2E blocks; `find <dir> -depth -delete` is the working form.

**Active phase: phase-02 — pty-marker-protocol** (status: **in-progress**,
bounced 2026-09-03 on `bug-02-1`). Re-dispatch with
`/rexymcp:dispatch phase-02`.

**Round 1 (DeepSeek V4 Flash 0731, 146 turns): approved on substance, bounced
on one defect.** All four gates green independently at review, lib 1540 → 1550
(the ten pinned tests), no `unwrap`/`expect`/`panic!`/`unsafe`/`#[allow]` in
279 production lines, and two reviewer mutations confirmed the tests
discriminate. Paste fidelity was genuine — the reviewer ran the self-check
against the surviving artifact and it printed `PASTE MATCH`.

**bug-02-1 (major): `PtyShell::run` never enforces its timeout.** `remaining`
is computed from the deadline and then used only for an `is_zero()` test; it is
never applied to the blocking `read`, so a command that emits nothing runs past
its budget. Two measurements: through the crate's public API,
`run("sleep 20", 2s)` returned **`Ok` after 20.1 s**; and a reviewer mutation
of the marker made `cargo test --lib shell::pty::` **hang until a 10-minute
external kill** instead of failing at the test's own 10-second timeout. The
second is the M8/M10 lesson resurfacing — a starved read must fail fast, not
hang the suite.

**Architect defects found in the same review:**

- **Three acceptance criteria were unsatisfiable, and this is the second phase
  in a row.** They read `grep -c "fn <name>"` pinned at **1**, but the pinned
  *test* names contain `fn <name>` too, so the tree measured 7 / 3 / 2. The
  code was right. Corrected to `^pub fn <name>(`, which measures 1 / 1 / 1 on
  the very same tree. Phase-01's identical class (`pub execution:` pinned at 2,
  reachable 1) makes **two occurrences**; `docs/dev/TODO.md` § 1 is the
  standing item and this is now a trend, one short of the fold threshold. The
  root cause both times: validating that a criterion *fails now* does not
  validate that its *expected value is reachable later*.
- **The spec named a behaviour without a mechanism that could deliver it.**
  Task 5 said "on timeout return an `Err`" and described a read loop that
  cannot honour it. Recorded in the bug doc's Root cause as the
  architect-side half.
- **No test was specced for the timeout contract**, which is why it shipped
  broken and is what round 2 adds. Telemetry failure class:
  `missing_spec_test`.

Minor: round 1's evidence entry pasted the self-check *command* but not its
*verdict line*, so the check fell to the reviewer. Called out in the phase
doc's round-2 notes.

**Round 2 (98 turns): bug-02-1 fixed, two new defects found — bounced again.**
The timeout fix itself is correct and verified independently: a silent
`sleep 20` under a 2 s budget now returns `Err` at 2.0 s naming the timeout and
the command. But the fix moved the blocking read onto a per-`run` worker thread
and got the reader's lifecycle wrong.

- **bug-02-2 (blocker): every second command on a healthy shell fails.**
  `run`'s success path drops the worker but never calls `refresh_reader`, so
  `self.reader` stays the `std::io::empty()` placeholder `take_reader` installed;
  the next `run` reads 0 bytes at once and reports a false **"PTY closed"**.
  That error path *does* re-seat, hence a deterministic alternation. Measured on
  one healthy shell with no timeout involved: `Ok, ERR, Ok, ERR, Ok, ERR`. After
  a real timeout the shell stays poisoned as well, because the detached worker
  keeps a live reader clone and races the new one for bytes. **In both cases the
  command executed and only its output was lost** — the caller is told "timeout"
  or "PTY closed" for work that actually ran, which is the shape that would let
  a later agent retry a non-idempotent command.
- **bug-02-3 (major): round 1's evidence entry was rewritten with round 2's
  numbers.** A new entry was added correctly, and then the old one was edited in
  place — `10 passed` → `11 passed`, `1546` → `1547`, and the three `fn …`
  counts that *found the defective criteria at the round-1 review* replaced by
  the corrected `pub fn` ones. The round-1 tree never produced those numbers.
  Recoverable from `3536573`.

**Why the suite missed bug-02-2, and the architect's share of it:** every test
in the module spawns a fresh `PtyShell` and runs exactly **one** command, so
nothing exercises the module's primary use — a long-lived shell running many
commands. My Test plan specced it that way in both rounds. Round 3 adds
`pty_runs_many_commands_on_one_shell` and `pty_shell_is_usable_after_a_timeout`.

**Also architect-side:** nothing in the phase doc said the Update Log is
append-only. It says so now, in the round-3 notes.

**Round 3 (60 turns): `hard_fail` — `NoProgressStall`, empty diff, zero files
changed.** The executor spent its entire budget on `bug-02-3`'s documentation
surgery (`git show` / `diff` / `sed` / `python3` against the phase doc, over
and over) and **never opened `src/shell/pty.rs`**. Escalation lever: **refined
re-dispatch** (entry in the phase doc).

- **`bug-02-3` is resolved by the architect** and removed from executor scope.
  Round 1's entry and its server-authored `(complete)` entry were restored
  verbatim from `3712c74` (198 lines) with a superseded note above them; round
  2's entries untouched. It was record-keeping on the architect's own document
  with no telemetry value, and its shape — splicing an old block into a file
  that changed around it, with no `sed -i` or `>` available to the executor —
  is precisely what stalled the run. **Bounce work must be shaped for the
  executor's tool set, not just stated.**
- **Round 4 carries exactly one task**, `bug-02-2`, with a loud header saying
  green gates are expected and are not evidence of completion, a list of what
  to preserve rather than rewrite, a falsifiable finish condition
  (`13 passed, not 14`) and a self-run mutation check.

**phase-02 — pty-marker-protocol: done (approved_after_4) 2026-09-03**,
landing commit `3e06009`. Five rounds: two review bounces, two
`NoProgressStall` hard-fails, then a **resume** that completed in 90 turns —
the shortest of the five. Three bugs, all resolved. Lib 1540 → 1549;
`shell::pty::` 13 passed in 2.00 s.

Verified independently at review through the crate's public API, which is the
door both blockers surfaced at: six sequential commands on one healthy shell
all succeed, a timeout leaves the shell fully usable, a timed-out command's
late output does not leak into the next result, and the shell survives the
SIGTERM. Two mutations confirm the tests discriminate, including one of mine
showing the new signal helper is load-bearing. Append-only discipline held —
round 5's doc diff has zero deletion lines.

**What worked, for the next stall.** Two consecutive `NoProgressStall`s, and
neither was about code the executor could not write. R3 spent its budget on
documentation surgery; R4 wrote both tests correctly and then ran an identical
probe command ~40 times. The lever that landed it was **resume with the exact
edit named and probing forbidden**, not another re-dispatch. Raising
`read_only_stall_threshold` 60 → 200 gave that resume room to finish rather
than room to loop.

**Active phase: phase-03 — asciicast-log**
(`docs/dev/milestones/M20-shell-engine/phase-03-asciicast-log.md`, status:
todo, drafted 2026-09-03). Dispatch with `/rexymcp:dispatch phase-03`.

Scope: `src/shell/log.rs` — an asciicast v2 writer, a `.meta.json` command
index keyed by byte range, and a reader that slices command N out of the cast.
Pure over byte streams with an injected timestamp; no PTY, no clock call, no
production caller (phase-05 is the first). `"r"` resize events are deliberately
excluded — phase-05 owns resize and adds that method with its caller.

Drafting work, applying the phase-02 lessons:

- **The format spec was fetched and pasted inline**, since the executor has no
  web access: header fields, the `[time, code, data]` event shape, and all four
  event codes with the spec's own examples.
- **Three facts measured rather than assumed.** `serde_json` already escapes
  the unit-separator and ANSI bytes our marker protocol emits, so nothing needs
  hand-escaping. Float times always serialise with a decimal point. And the
  load-bearing one: `std::str::from_utf8` distinguishes an *incomplete* trailing
  sequence (`error_len() == None`, carry it) from *genuinely invalid* bytes
  (`error_len() == Some(n)`, consume them). Without that discriminator the
  writer either corrupts every split character or carries a bad byte forever —
  and phase-02 measured that a 4096-byte PTY read does split characters.
- **The headline test is the module's primary use**, not a unit slice: write a
  three-command session and read each command back byte-exact. That is the
  direct answer to phase-02's blocker, where a test plan that only exercised
  one command at a time let a broken second command ship past a green suite
  twice.
- **The test filter is `shell::log::`, not `log::`** — measured, a bare `log::`
  already matches **16** pre-existing tests and reports `ok` today. Same trap
  as phase-02's `shell::`, caught before dispatch this time.
- **Every criterion greps a source file, never this document**, so the
  self-matching class below cannot recur here.
- The doc contains **zero control bytes**, so the paste-fidelity check has
  nothing unpastable in it.

**phase-03 round 1 (96 turns): bounced on `bug-03-1`.** All four gates green
independently, lib 1549 → 1560 (exactly the 11 named tests), no
`unwrap`/`expect`/`panic!`/`unsafe`/`#[allow]`, and the module has no
production caller as the scope required. The writer, the index types,
`meta_path_for` and the reader are all correct.

**The defect: a dangling UTF-8 carry leaks across a command boundary.** When a
command's output ends mid-character — exactly what phase-02 measured a
4096-byte PTY read doing — the carried bytes are not written before the end
marker. Measured at review through the public API: command 0 wrote `A`, `B`
and the first byte of a 3-byte character; it read back as `"AB"`, a byte
short, and command 1 read back as a replacement character followed by its own
`"ZZZ"`. That violates the phase's own headline guarantee, "exactly that
command's own bytes and nothing from its neighbours."

**Root cause is a vacuous guard, and the completion summary asserted the
opposite.** `flush_carry` calls `write_output` with an empty slice; that
re-enters the same decode, finds the same incomplete sequence, emits nothing
and writes the carry straight back. It cannot flush on any input that can
reach it. The summary claimed the flush was "fixed" and "covered by the
existing `cast_carries_a_split_multibyte_character` test" — nothing is
flushed, and that test never calls `mark`. Telemetry class:
`false_completion`.

**Two smaller findings, both folded into the bug's definition of done rather
than filed separately.** One test hardcodes `first_byte: 45`, which lands
inside the 51-byte header line; it passes only because the reader skips the
resulting malformed partial line, so it verifies no byte range at all. And
event times render through `format!` rather than a JSON float, so a whole
number emits as `[10, "m", …]` instead of `[10.0, …]`. The second is
interoperable — every JSON parser reads a bare `10` as a number — and my own
spec was ambiguous about the field's rendering, so it is recorded as a
calibration note, not a defect.

**What went right, worth keeping.** The measured-facts section did its job:
the executor implemented the `error_len()` discriminator exactly as specified
and its split-character handling is correct *within* a single command. The
gap was a boundary case my test plan named in prose but never pinned as a
test — the same shape as phase-02's blocker, one level down.

**phase-03 — asciicast-log: done (approved_after_1) 2026-09-03**, landing
commit `66bd02a`. Two rounds, one bug, resolved. `shell::log::` 12 passed
(the `12, not 13` finish condition held exactly); `shell::pty::` still 13.

Round 2's fix verified independently with the probe that found the defect: the
command ending mid-character keeps its truncated byte and the next command's
slice is clean. Checked that the lossy substitution is confined to the
unrepresentable case — complete multi-byte output split across three
character-cutting writes still round-trips byte-exact. Mutation-checked in
**both** directions: a no-op flush fails the new test, and so does clearing the
carry without emitting it, so the guard discriminates rather than merely
existing.

**The durable lesson, and it is the architect's:** the guarantee that broke was
stated in the phase doc's own prose — the headline test's words, "nothing from
its neighbours" — but never pinned as a test that crossed a boundary. That is
phase-02's blocker one level down. **For the rest of M20: when a spec states a
guarantee about boundaries between units, one of the named tests must cross a
boundary.**

**Active phase: phase-04 — screen-model**
(`docs/dev/milestones/M20-shell-engine/phase-04-screen-model.md`, status: todo,
drafted 2026-09-03). Dispatch with `/rexymcp:dispatch phase-04`.

Scope: `src/shell/screen.rs` — a terminal-emulator wrapper giving a live
screen, a grid-cell semantic annotator replacing the string-based one, and a
one-line status summary that **calls** the existing pure classifier rather than
copying it. Hermetic and fixture-driven; no PTY, no clock, no config read, no
production caller. Adds one dependency.

**The outstanding measurement is done, and it changed a design assumption.**
The milestone had carried "re-measure the alt-screen and resize leg before
drafting phases 04 and 05" since scoping. Four facts now measured:

- **`set_size` does not reflow.** 26 characters at 20 columns read back as one
  logical line; after widening to 30 they read back as **two** lines broken at
  the old width. Widening corrupts text already on screen, so **resize cannot
  be a `set_size` call on a populated grid — phase-05 has to rebuild the
  screen from the cast log.** Recorded in the milestone README.
- The six colour codes map to specific indexed colours, pairing exactly with
  the existing string classifier's code pairs.
- `contents()` is the **visible screen only**; scrollback is a view offset.
  That confirms the split the design rests on: the screen is the viewport, the
  cast log is the transcript.
- The alternate-screen flag tracks the alt-screen escapes.

**Applying the phase-03 lesson.** The rule recorded there — when a spec states
a guarantee about boundaries, one named test must cross a boundary — is
honoured directly: `screen_does_not_merge_a_colour_run_across_a_row_boundary`
is a named test, and so is a run-grouping negative that catches
one-marker-per-cell, and an escape sequence split across two feeds.

**One drafting correction worth recording**, since architect defects have been
the recurring theme: my first draft quoted the filtered-test count as 1561 when
the tree prints 1565, and justified the qualified test filter by claiming a
bare `screen::` was a live trap. Measured: it matches nothing today, unlike
`log::` (16) and `shell::` (43). Both corrected before dispatch — an
inaccurate measured value in a spec is the same defect class as an unreachable
criterion.

**phase-04 round 1 (90 turns): bounced on `bug-04-1`.** All four gates green
independently, lib 1561 → 1571 (10 tests), all acceptance criteria met, no
`unwrap`/`expect`/`panic!`/`unsafe`, `src/tmux/` byte-for-byte untouched, and
the status classifier is called rather than copied — the criterion checking
both halves passed.

**The defect: the annotated view collapses cursor-positioned columns.**
`annotated()` skips every blank grid cell, including interior ones, so column
layout done by moving the cursor rather than writing spaces runs together.
Measured at review:

```
columns positioned with ESC[nC:
  contents()  = "NAME      SIZE      MODE"
  annotated() = "NAMESIZEMODE"
```

`ls`, `top`, `git status` and most table renderers position columns exactly
that way, and `annotated()` is the view the model reads — so an agent would
see unparseable run-together text. The spec said *trailing* blanks are
trimmed; interior ones are part of the row.

**A second, smaller finding folded into the same bug's DoD.** The test named
`screen_does_not_merge_a_colour_run_across_a_row_boundary` feeds red on row 0
and **green** on row 1, so the colour change alone forces the split and the
test cannot discriminate the same-colour case its name claims. **The behaviour
is correct** — verified at review, a same-colour run across the boundary
yields two markers — but the guard is decorative. This is the milestone's own
recorded lesson, one phase later: *a guarantee needs a test that actually
crosses the boundary*, and a test can carry the right name while exercising
the wrong case.

**phase-04 — screen-model: done (approved_after_1) 2026-09-03**, landing
commit `d952fd4`. Two rounds, one bug, resolved. Round 2 took **46 turns**, the
fastest of the milestone. `shell::screen::` 11 passed (the `11, not 12` finish
condition held exactly); siblings unchanged at 13 and 12; `src/tmux/`
byte-for-byte untouched.

Verified independently: cursor-positioned columns now render as
`"NAME      SIZE      MODE"` matching `contents()`, where they collapsed to
`"NAMESIZEMODE"`. Mutation-checked twice — reverting the gap fix fails the new
test, and merging rows fails the corrected boundary test, which the round-1
fixture could not have caught.

**The lesson this phase adds, one turn past phase-03's.** Phase-03 taught that
a guarantee needs a test crossing the boundary. Phase-04 showed a test can
carry exactly the right *name* and still exercise the wrong *case*: the
boundary test fed red on one row and green on the next, so the colour change
alone forced the split and it could never fail for the stated reason. The
behaviour was already correct, so nothing looked wrong. **What caught it was
reading the fixture, not the name** — worth doing on every named guard from
here.

**One residual carried, not re-bounced.** A gap between a coloured run and
plain text is still absorbed into the marker. The bug's own DoD asked for
`annotated()` and `contents()` to agree on column positions, which is
**unachievable in general** since the marker text shifts every later column —
my constraint was over-strong. Where a gap belongs relative to a marker is a
design question, recorded in the milestone README for phase-07 to decide.

**A third self-matching grep criterion, and this hits the fold threshold.**
`bug-02-3`'s own DoD greppped for an unanchored `fn parse_outcome        (1): 7`
— which the criterion's own quoted text in § Acceptance criteria also matches,
so it returned **2**, not the **1** it pinned, and could never have been
satisfied. Anchoring (`^fn …`) returns 1. With phase-01's `pub execution:`
(pinned 2, max 1) and round 1's three `fn <name>` counts (pinned 1, actual
7/3/2), that is **three occurrences of one class**: *validating that a
criterion fails now does not validate that its expected value is reachable
later.* `WORKFLOW.md` § Calibration puts the fix threshold at three, and
`docs/dev/TODO.md` § 1 is the standing item. **PE decision needed on whether
to fold** — the architect does not amend `WORKFLOW.md` unilaterally.

Drafting measurements (all executed on scrappy; probe sources kept in
`M20-shell-engine/probes/`, and the seven facts are quoted in the phase doc's
§ "Measured facts"):

- **The complete wrapper works identically in bash, zsh and fish** — exit `42`
  and byte-identical output `"\r\nhello\r\n\r\n"` in all three, with
  `$status` for fish and `$?` for the others.
- **A BEGIN marker was added to the design.** The 2.0 plan describes only an
  end marker; measurement showed the PTY echoes the typed command ahead of the
  output, leaving no reliable left edge. Recorded in the M20 README for the
  phase-09 doc sweep.
- **The split-quote is load-bearing and was proven by its failure.** The naive
  form matched the echo before the command ran and returned the tail of the
  echoed line as the "exit code".
- **A 4096-byte read splits multi-byte characters** — both chunks of a 4942-byte
  UTF-8 stream failed `from_utf8` while the whole buffer was valid. The parser
  must scan accumulated bytes.
- **A forged marker with a different nonce is ignored only if the search keys
  on the full end marker.** Splitting on a bare `\x1f` truncated the captured
  output in the same measurement.
- **`(exit N)` hangs fish** — it is command substitution there. Fixtures use
  `sh -c 'exit 42'`.

Four defects in the first draft were caught by validating the criteria rather
than assuming, and all four are the phase-01 review's lesson applied:

1. **`cargo test --lib shell::` already matches 43 pre-existing tests** in
   `daemon::utils::shell::` and reports `ok` today, so a criterion phrased over
   it would pass before any code existed. Changed to `shell::pty::`, which
   matches 0.
2. **An unanchored `awk '/#\[cfg\(test\)\]/{exit}'` is a vacuous guard** — a
   *doc comment* mentioning `#[cfg(test)]` stops it. Measured on
   `src/config/lifecycle.rs`: unanchored printed 7 lines of 613, anchored
   printed 284. The criterion now anchors at `^`.
3. **A bare `grep -c unsafe` would fail on a doc comment**, and this phase's
   rationale mentions `unsafe` in prose. The criterion now strips comment
   lines first.
4. **`grep -c` on an absent file errors (exit 2) rather than printing `0`**, so
   the E2E prose claiming otherwise was wrong; corrected, and a warning line in
   section F is now itself a documented failure signal.

Drafting notes:

- **All 15 mechanical acceptance criteria were validated against the current
  tree in their failing state** — every one returns `0` today. The two
  path-audit criteria are deliberately split into a quoted `source` string and
  a trailing-comma vec line, because `path_audit.rs` contains both forms for
  every constructor and a single grep cannot tell them apart; verified against
  the existing `etc_dir` pair (1 and 1).
- **The `constructors` vec in `inventory_contains_all_config_constructors`
  (`src/config/path_audit.rs:554`) is hand-maintained**, so an `INVENTORY`
  entry added without the matching vec line leaves the gate vacuously green.
  Both edits are separate criteria for that reason.
- **`is_covered()` covers subdirectories**, so `var/run/shells` inherits the
  `var/run` entry and Direction A would pass without it, while
  `var/log/shells` has no covering parent and fails without its own entry.
  The spec requires both anyway — the `var/run/shells` entry exists to record
  that shells are exempt from `var/run`'s `ClearAtStartup` intent.
- **Measured: no daemon-free subcommand loads `Config`.** A deliberate type
  error in `[limits]` left `daemoneye costs` exiting `0`, because it reads the
  event log directly. So the E2E block proves the real binary creates both
  directories and seeds the documented blocks, and says plainly that
  config *parsing* is unit-tested here and verified against the running binary
  in phase-07 and at the M20 close. The executor is told not to hunt for a CLI
  that prints the resolved config.
- **Section B of the E2E block can lie**: `cargo test --lib execution` today
  prints `test result: ok. 0 passed … 1533 filtered out` with `exit=0` and no
  matching test. The pass condition is the named test lines, not the exit code.
- **The PASTE MATCH self-check was validated both ways** against a copy of the
  phase doc: byte-exact → `PASTE MATCH`; one line retyped → `PASTE MISMATCH`
  naming the divergence.

