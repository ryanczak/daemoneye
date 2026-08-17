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

## Active milestone: M16 — LLM Stream Robustness (drafted 2026-08-16, opened 2026-08-16)

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

**Active phase: phase-03 — anthropic-gemini-two-phase**
(`docs/dev/milestones/M16-llm-stream-robustness/phase-03-anthropic-gemini-two-phase.md`,
status: todo — re-run its re-derive commands before dispatch). Advance via
`/rexymcp:architect next`.

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

**Active phase:
[M11 phase-07b — situational-knowledge-hooks](milestones/M11-knowledge-index/phase-07b-situational-knowledge-hooks.md)
(`in-progress` — **bounced ×2 on 2026-08-07**, see
[bug-07b-1](milestones/M11-knowledge-index/bugs/bug-07b-1.md)). **This is the
last phase of M11** — after it, the milestone hits its human gate.**

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
