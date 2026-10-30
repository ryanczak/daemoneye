# M32 — Findings fixes

**Goal:** everything the M31 audit found and deferred is fixed or
retired: the fourteen defects are fixed with a test each, the duplicated
code the audit named is one copy, and the test suite is fast, reliable
and built on shared fixtures.

**Status:** in-progress (scoped 2026-10-28)

**Release:** 2.5.0

**Depends on:** M31 (the tree as released in 2.4.0).

**Sources:** [`M31-code-hygiene/FINDINGS.md`](../M31-code-hygiene/FINDINGS.md):
the 51 `defer` rows, the seven `act` rows M31 did not take (#4, #8, #49,
#50, #62, #63 — the shared test fixture — and #60), and nothing else. The
16 `reject` rows stay rejected. The ten `act` rows that are "input to a
function-split phase" (#1, #2, #16, #21, #35, #45–#47, #69, #71) belong to
the function milestone, with the 40 allowlist entries, not here.

**What this milestone is not.** It is not the function decomposition. The
40 `fn` entries in `tests/hygiene_allowlist.txt` stay as they are, and the
gate keeps them from growing. Several phases here shrink one of them as a
side effect (the status-bar helper takes ~50 lines out of
`render_ratatui_draw.rs`'s panels, the input-state-machine merge takes 60
out of `stream_prompts.rs`), and that is welcome, but no phase's goal is a
line count.

**PE decisions (2026-10-28):**

1. **A defect is fixed with a test that fails first.** Every `bug` row
   gets a named test that fails on the 2.4.0 tree and passes after; the
   phase doc states the test's name and what it asserts, and the executor
   runs it red before the fix (M29's rule). A fix without a failing test
   is not a fix.
2. **A tidy changes no behaviour.** The `smell` rows are consolidations:
   one helper where there were three copies. The gate is M31's: the full
   suite passes at the same count, and the executor's Findings entry names
   what the merged copies differed in. Where copies differed (#37's two
   message formats, #56's missing red border), the phase doc says which
   one wins, and that is the one behaviour change the tidy may make.
3. **Specs are lean.** These changes are small and named: a symbol, a
   file, a test name, the constraint. No dictated scripts; the executor
   holds the compiler. The architect pins *where* the fix lives (the one
   function or file) and the finish condition (the test name, the count).
4. **Order by value.** The defects first (phases 01–09), the test suite
   next (10–11, because every later review pays for #65's flake and #5's
   poll loops), the tidies last (12–14).
5. **The audit continues.** Every phase writes `### Findings — <date>`
   (M31 decision 7, unchanged) into this milestone's
   [`FINDINGS.md`](FINDINGS.md), triaged at review the same way.
6. **No new allowlist entries.** `tests/hygiene.rs` is a gate here as in
   M31: a fix that pushes a function over 150 lines splits it or lands
   elsewhere.

**Exit criteria:**

- Every `bug` row in the source ledger (#11, #12, #20, #23, #38, #57, #58,
  #74, #77, #84, #85, with #13, #61 and #75 as their contract fixes) is
  closed by a named test in the tree, and the ledger row reads
  `act: M32 phase-NN`.
- Every `smell` and `perf` row in scope is closed by the phase named in
  its ledger row, with the suite count unchanged by that phase.
- `grep -rn 'lock().expect\|lock().unwrap()' src` finds nothing outside
  test code (#61, the CLAUDE.md invariant).
- One `test_home()` fixture serves the test files that today carry their
  own (#4 and its family); `grep -rn 'fn test_home\|fn shell_test_home\|fn lifecycle_home' src` finds one definition.
- `cargo test` passes 20 runs in a row with no failure (#65), and the
  suite's wall clock is reported before and after phases 10–11.
- `cargo test` passes with at least the 2587 tests of 2.4.0 plus one per
  fixed defect; `tests/hygiene.rs` and `tests/doc_truth.rs` pass; the
  allowlist has gained no entry.
- Live check on pinky of the two user-visible fixes: a `chat --resume` of
  an unknown session id is refused (#11/#20), and a 30-character CJK token
  wraps once in the viewer (#23). The rest are covered by their tests.
  `2.5.0` is released per `REXYMCP.md` § Versioning.

## Architecture references

- `docs/architecture.md` § 1.2 (orchestration: the executor, the stream
  loop, the session handlers), § 1.3 (the AI provider layer), § 2.1 (the
  request/response flow the fixes sit on).
- [`M31-code-hygiene/FINDINGS.md`](../M31-code-hygiene/FINDINGS.md): every
  row this milestone takes, with the executor's description and the
  review's triage note.

## Phases

Drafted one at a time with `/rexymcp:architect next`. Grouped by file so
each phase is one executor session; the finding numbers are the source
ledger's. Expected count: 14. Each may split at drafting.

| #  | Phase | Findings | Status |
|----|-------|----------|--------|
| 01 | ghost-gate ([phase-01-ghost-gate.md](phase-01-ghost-gate.md)) — `dispatch_foreground.rs` loads the config once per call; an unreadable config refuses the ghost (fail closed) instead of defaulting to "sandbox off" | #84 | done |
| 02 | ai-turn-model ([phase-02-ai-turn-model.md](phase-02-ai-turn-model.md)) — `stream/conversation.rs`'s two cost-and-event blocks become one helper, so the tool-call turn's `ai_turn` event records the model that answered | #85 | done |
| 03 | recall-limits ([phase-03-recall-limits.md](phase-03-recall-limits.md)) — `dispatch_memory.rs` threads the loaded `[limits]` into `recall_context` instead of `LimitsConfig::default()` | #74 | done |
| 04 | session-guard ([phase-04-session-guard.md](phase-04-session-guard.md)) — `server/sessions.rs`: save and load of an unknown session id reply `Response::Error` instead of saving an empty session or claiming a resume; #13 rejected (the banner labels both counts) | #11 #12 #13 | done |
| 04b | session-rehydrate — **dropped**: #20 rejected (PE decision 2026-10-29). The client reaches the daemon only over a local Unix socket, so it is never off the daemon's host, and showing the full saved history beside a banner naming the messages loaded is the intended behaviour | #20 | dropped |
| 05 | wrap-words ([phase-05-wrap-words.md](phase-05-wrap-words.md)) — `viewer_layout.rs`: `wrap_words` breaks overlong tokens on char boundaries (no doubled CJK token, no `space_remaining` underflow) and documents the whitespace-only-line rule; #24 rejected | #23 #24 #26 | done |
| 06 | renderer-fixes ([phase-06-renderer-fixes.md](phase-06-renderer-fixes.md)) — `render_ratatui_draw.rs`: panel borders fill by display width (#57) and the module doc's backtick (#60); #58 rejected (the failure path tears the renderer down); #55 and #56 move to phase 12 | #57 #58 #60 | done |
| 07 | remote-write-sentinels ([phase-07-remote-write-sentinels.md](phase-07-remote-write-sentinels.md)) — one `remote_write_outcome` parser for remote delete and copy: only lines above `__DE_DONE__` count, and a result line must start with its sentinel; #36 #37 #40 move to the function milestone | #36 #37 #38 #40 | done |
| 08 | host-refusal ([phase-08-host-refusal.md](phase-08-host-refusal.md)) — `edit_file` writes `ssh_connect_failed` through a shared `log_connect_failure` (#80); a named-host foreground call does its permissive lookup once (#82); #75 #78 #79 deferred to the function milestone | #75 #78 #79 #80 #82 | done |
| 09 | silent-drops ([phase-09-silent-drops.md](phase-09-silent-drops.md)) — the two `lock().expect` sites become `.unwrap_or_log()`, and a new `tests/lock_invariant.rs` gate holds the invariant (#61); the tool-feedback sends and the background close log at debug (#77); a failed extraction `add_memory` logs a warn (#18) | #18 #61 #77 | done |
| 10 | test-home ([phase-10-test-home.md](phase-10-test-home.md)) — one `crate::test_home()` in `src/lib.rs` replaces eleven copies of the temp-HOME helper (154 call sites); #8 #49 #50 #62 #63 rejected as single-file nits | #4 #8 #49 #50 #62 #63 | done |
| 11 | test-reliability ([phase-11-test-reliability.md](phase-11-test-reliability.md)) — `log_event` asserts the HOME lock under `cfg(test)` and the four unguarded writers take `crate::test_home()` (M32 #11); `read_shell_screen_lines_tails_the_visible_rows` polls one read (#65); two tests drive `run_terminal_command`'s approved path and the malformed-host refusal through `execute_tool_call` (#83) | #65 #83 M32 #11 | done |
| 11b | test-tidies ([phase-11b-test-tidies.md](phase-11b-test-tidies.md)) — one `test_renderer(cols, rows, viewport, depth)` builds every render test renderer (#54); `default_status()` and `approval_test_status()` are shared by both render test files (#53); two bare `assert_eq!`s get messages (#51); #5 #6 rejected (measured: the cost is HOME-lock serialisation, not the poll intervals) and #51's nine self-describing table checks kept | #5 #6 #51 #53 #54 | done |
| 12 | client-prompts ([phase-12-client-prompts.md](phase-12-client-prompts.md)) — `stream_prompts.rs`'s two approval-input state machines become one `read_approval_input_with` taking the redraw as a closure (#66); the dead `_path_approved` goes (#67); the shared redirect/skipped tail becomes `commit_declined` and `stream_ask.rs`'s nine prompt status literals become `prompt_status` (#68) | #66 #67 #68 | done |
| 12b | renderer-and-viewers ([phase-12b-renderer-and-viewers.md](phase-12b-renderer-and-viewers.md)) — one `render_status_bar` for six status-bar copies (#55); one `alert_panel` for the approval and credential shells, the picker's plain border kept on purpose (#56); `enter_alt_screen` and `resize_events` for the tail, log and attach viewers (#27); a test pins the log-before-note arm order in `render_tail` (#30); #28 rejected (the empty frame source is documented and matches `tail_loop`'s contract) | #27 #28 #30 #55 #56 | done |
| 13 | tool-tidies ([phase-13-tool-tidies.md](phase-13-tool-tidies.md)) — one `shell_tool_regex` holds the shell tools' pattern size limit (#70); `send_input` and `close_shell` use the shared `unknown_shell_error`, pinned byte for byte by their tests (#72); `window_text` is linear and its alias goes (#42 #43); `run_started_at` calls `started_at_of` (#64); one `with_run_id` for the run-log arms (#76); `load_tools` looks each group up once (#81) | #42 #43 #64 #70 #72 #76 #81 | done |
| 14 | small-tidies ([phase-14-small-tidies.md](phase-14-small-tidies.md)) — the ghost refusal prints the config error chain, with a test (M32 #1); the two config defaults call `default_true` (#10); `ContainerInfo`'s docs describe the sbx sandbox, `image` kept as wire format (#32 #33); the unread `ArtifactHit`/`EpochHit` fields go (#48, part); `startup.rs` names its imports (#17); `EventHit` stays, see M32 #16 | #10 #17 #32 #33 #48 M32 #1 | done |
| 15 | events-search ([phase-15-events-search.md](phase-15-events-search.md)) — the events search reads its FTS index, which never returned a hit (M32 #16): the whole history as a phrase query, then the tail scan, each line once; every events result is masked (M32 #17); `EventHit`'s dead fields go (#48, rest) | #48 M32 #16 M32 #17 | todo |

## Notes

- **Scoped 2026-10-28** from the M31 ledger. Numbering: this milestone
  takes M32, so MCP surface becomes M33 and Knowledge M34.
- **Triage back-references.** When a phase lands, the review changes the
  source ledger's row from `defer: …` to `act: M32 phase-NN`, so the M31
  ledger stays the one record of where each finding went.
