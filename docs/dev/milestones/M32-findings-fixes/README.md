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
| 04b | session-rehydrate — `slash_session.rs` rehydrates the visible transcript from what the daemon loaded, not from the full session file on the client's own filesystem | #20 | todo |
| 05 | wrap-words — `viewer_layout.rs` chunks overlong tokens on char boundaries (a CJK and an emoji test), documents the blank-line rule, lifts `flush` out of the loop | #23 #24 #26 | todo |
| 06 | renderer-fixes — `render_ratatui_draw.rs`: `inserted_rows` advances only on `Ok`; borders fill by display width; one `render_status_bar` for the three panels; the picker gets the red border; the module doc's backtick | #55 #56 #57 #58 #60 | todo |
| 07 | remote-write-sentinels — `file_ops/ops_{delete,copy}.rs`: the sentinel scan reads a line, not a substring, and checks `__DE_DONE__`; a unique temp path; the dest-exists check and the shell string built in the same order on both hosts | #36 #37 #38 #40 | todo |
| 08 | host-refusal — one helper for the host-parse prologue and the `ghost_host_refused` event across `read_file`, `open_shell`, `edit_file` and the foreground arm; `edit_file` logs `ssh_connect_failed`; `read_file` drops its extra `ToolResult` frame; one `host_entry_or_bare` per call; a call with no session is refused, not run as the empty-string owner | #75 #78 #79 #80 #82 | todo |
| 09 | silent-drops — the `let _ =` sends and the dropped close log at debug; `lock().expect` sites become `.unwrap_or_log()` and a test holds the invariant; `apply_extraction` logs a dropped `add_memory` | #18 #61 #77 | todo |
| 10 | test-home — one `test_home()` fixture (and `RecordingWriter::new`, the web01 config, the archive transcript) replaces the ~130 copies across the shell, executor, memory-index and knowledge test files | #4 #8 #49 #50 #62 #63 | todo |
| 11 | test-reliability — `read_shell_screen_lines_tails_the_visible_rows` reads once; the PTY poll loops wait on a condition, not a sleep; a fake-PTY test drives `run_terminal_command`'s approved path and the named-host refusal through `execute_tool_call`; the 16 bare `assert_eq!`s get messages; the render tests share a status fixture and a parameterised renderer builder | #5 #6 #51 #53 #54 #65 #83 | todo |
| 12 | client-prompts — `stream_prompts.rs`'s two input state machines become one taking the redraw as a closure; the dead `_path_approved` goes; `stream_ask.rs`'s nine `StatusBarState` literals and the two commit ladders become helpers; one `enter_alt_screen` for the three viewers | #27 #28 #30 #66 #67 #68 | todo |
| 13 | tool-tidies — `compile_pattern` and one unknown-shell refusal for the shell tools; one `RunId::parse` helper in `dispatch_runs.rs`; `load_tools` collects members once; `run_started_at` calls `started_at_of`; `window_text` stops being quadratic and `activity_window_text` goes | #42 #43 #64 #70 #72 #76 #81 | todo |
| 14 | small-tidies — the dead `*Hit` fields; `ContainerInfo`'s docs and the hard-coded `image`; the config defaults named once; `startup.rs`'s explicit imports | #10 #17 #32 #33 #48 | todo |

## Notes

- **Scoped 2026-10-28** from the M31 ledger. Numbering: this milestone
  takes M32, so MCP surface becomes M33 and Knowledge M34.
- **Triage back-references.** When a phase lands, the review changes the
  source ledger's row from `defer: …` to `act: M32 phase-NN`, so the M31
  ledger stays the one record of where each finding went.
