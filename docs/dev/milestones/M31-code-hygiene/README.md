# M31 — Code hygiene

**Goal:** no source file or function is too long to read in one sitting, and
a test keeps it that way: production code is at most 800 lines per file,
tests live in sibling files, no function is over 150 lines, and the two
1400-line tool dispatch matches are decomposed into per-family modules.

**Status:** done (scoped 2026-10-25, closed 2026-10-28)

**Release:** 2.4.0

**Depends on:** M30 (the tree as released in 2.3.0).

**Sources:** the audit of 2026-10-25 (below). The PE's decision of the same
day: accept the audit's recommendations; exempt `src/ai/types/pending.rs`
and `src/ai/tools/args.rs` as flat data.

**What exists today (audit 2026-10-25).** 193 source files, 139k lines
under `src/`. 42 files are over 1000 lines. They fall into three groups:

1. **Long because of their test module** (20 files). Production code is
   under 1000 lines; the `#[cfg(test)]` module pushes the file over. The
   convention for this exists: `ipc.rs` and `tmux/cache.rs` keep tests in a
   sibling file via `#[cfg(test)] #[path = "<name>_tests.rs"] mod tests;`.
   Examples: `memory/index/mod.rs` (618 prod / 3122 test), `shell/client.rs`
   (13 / 1804), `search.rs` (19 / 1540), `daemon/session.rs` (260 / 1350),
   `server/shells.rs` (732 / 1328), `shell/session.rs` (570 / 1318),
   `ai/tools/dispatch.rs` (231 / 1299), `shell/backend.rs` (312 / 1276),
   `shell/pty.rs` (163 / 1230), `config/mod.rs` (17 / 1169),
   `context/epochs.rs` (840 / 1047), plus `sbx.rs`, `registry.rs`, `log.rs`,
   `tail.rs`, `viewer.rs`, `file_ops/read.rs`, `utils/mod.rs`,
   `event_log.rs`, `scheduled.rs`.
2. **Production code itself over 800 lines** (about 20 files), each with
   visible seams: `executor/knowledge/shell.rs` (2286: seven tool families
   in sequence), `executor/mod.rs` (2115), `daemon/ghost.rs` (1677),
   `daemon/stream.rs` (1613), `cli/commands/stream.rs` (1516),
   `config/types.rs` (1466: twenty config structs), `ai/tools/defs.rs`
   (1398: one static of 43 `ToolDef`s), `cli/commands/top.rs` (1394),
   `cli/render_ratatui.rs` (1225), `cli/commands/chat.rs` (1147),
   `executor/shell_exec.rs` (1128), `cli/commands/top_tui.rs` (1050),
   `file_ops/ops.rs` (1018), then `cli/tail.rs` (988), `ipc.rs` (966),
   `cli/commands/slash.rs` (958), `cli/viewer.rs` (907), `daemon/mod.rs`
   (900), `server/handlers.rs` (877), `context/epochs.rs` (840).
3. **Giant functions**, the real readability failure: `run_conversation_loop`
   (`daemon/stream.rs`, 1456 lines), `execute_tool_call` (`executor/mod.rs`,
   1440), the `RatatuiRenderer` impl block (700), `ask_with_session_ratatui`
   (685), `run_daemon` (600), `run_chat_ratatui` (500), the `TopModel` key
   impl (310), `run_edit_with_spawn` (276), `run_in_shell_core` (260). The
   two 1400-line functions are each one `match` with 43 inlined arms.

**PE decisions (2026-10-25):**

1. **Caps.** 800 lines of production code per file, where production code
   is every line not inside an item that carries `#[cfg(test)]` (drafting
   01 found that "everything before the first `#[cfg(test)]`" undercounts:
   `search.rs` and `header.rs` have production code after a test-only
   helper). 150 lines per function, signature to closing brace. Test files (`*_tests.rs`, `tests/*.rs`, a `tests.rs`
   under a module directory) are exempt from the production cap and
   capped at 2000 lines each.
2. **Exemptions**, as flat data a split would not make more readable:
   `src/ai/types/pending.rs` (one 43-variant enum and its impl) and
   `src/ai/tools/args.rs` (forty argument structs), joined on 2026-10-28 by
   `src/ai/tools/defs.rs` (the `TOOLS` table). The exemption covers
   both caps: the file's size and its functions (PE decision 2026-10-25,
   after phase 01 measured `PendingCall::to_tool_call` at 617 lines). `src/ipc.rs` (the wire
   protocol enums) is a candidate for the same exemption; phase 01 says
   which way it goes.
3. **Enforced, or it regresses.** `tests/doc_truth.rs` exists because the
   README tables went stale three times before a gate caught them. A
   `tests/hygiene.rs` measures every `.rs` file against the caps and holds
   an allowlist of today's offenders that may only shrink. It lands first,
   so every later phase removes entries from it, and nothing new can join.
4. **Order by risk, lowest first.** Test-module moves, then pure splits by
   concern, then the two giant matches, once the gate is in place.
5. **Mechanical moves use the shell, not the edit tool.** `WORKFLOW.md`
   § "Executor self-sabotage on delete-heavy rewrites" records the hazard.
   A phase that moves a block says to cut it with `sed -n 'a,bp'` into the
   new file and delete the range in place, never to retype it. The gate for
   every mechanical phase is the same: the full suite passes with the same
   count, `git diff --stat` shows moved lines, and no symbol changes name
   or visibility except the `pub(super)`/`pub(crate)` the move requires.
6. **No behaviour change.** M31 adds no feature and fixes no bug. A defect
   found while splitting is reported, not fixed, so that every M31 diff is
   reviewable as a move.
7. **Every phase audits what it touches** (PE decision 2026-10-25). The
   executor reads every line it moves, which is the one time this code
   gets read end to end, so each phase carries a standing obligation: an
   `### Findings — <date>` entry in its Update Log listing every suspected
   bug, every optimisation opportunity and every smell seen in the code the
   phase handled, or the single line `none`. Each finding names the file and
   symbol, its kind (`bug` / `perf` / `smell`), what was seen, why it
   matters, and a severity (`blocker` / `major` / `minor` / `nit`). The
   executor does **not** act on a finding; the phase stays a move. At
   review the architect triages each one into
   [`FINDINGS.md`](FINDINGS.md) as `act` (a follow-on phase or bug doc,
   named) or `defer` (carried to a later milestone), with the PE's say on
   anything `major` or above. A phase with no Findings entry is bounced.

**Exit criteria:**

- `tests/hygiene.rs` passes with an **empty** allowlist: no production file
  over 800 lines except the exempted ones, no test file over 2000, no
  function over 150 lines.
- `execute_tool_call` and `run_conversation_loop` each dispatch to
  per-family (or per-event) modules; neither match arm holds inlined logic.
- `cargo test` passes with at least the 2570 tests of 2.3.0, and no test
  was deleted or weakened to make a split land.
- The CLAUDE.md "Key files" table names the new modules.
- Every phase's Update Log has a Findings entry, and every finding in
  `FINDINGS.md` carries a triage: `act` with the phase or bug doc that took
  it, or `defer` with the milestone it waits for. Nothing is untriaged at
  the close.
- Live close on pinky per the M14 convention: the L1–L8 blocks of M30's
  phase 10 pass unchanged on the refactored tree. `2.4.0` is released per
  `REXYMCP.md` § Versioning.

**Amended at close (PE decisions 2026-10-28).** Two criteria were met in
part and closed as they stood, so that the file work ships as 2.4.0 and
the function work gets a milestone of its own:

- **The allowlist is empty of `file` and `test` entries; its 40 `fn`
  entries carry over.** The gate forbids any of the 40 from growing, so
  the tree cannot regress while they wait. Decomposing them is the next
  milestone's scope, giant functions first (`run_conversation_loop` 775,
  `do_ghost_turn` 775, `ask_with_session_ratatui` 673, `run_daemon` 598,
  `handle_ask` 573, `run_chat_ratatui` 500, `run_background` 440,
  `run_status` 435; 32 more between 150 and 321).
- **`execute_tool_call` dispatches to per-family modules** (93 lines, from
  1439). `run_conversation_loop`'s 43 tool arms dispatch through
  `PendingCall::from_tool_event`; the loop body that remains (the `Done`
  arm's cost, persistence and tool execution) is the carried 775 lines.
- **The live close is waived.** Every code commit since 2.3.0 is a move
  whose segment-identity check and unchanged suite count are its
  evidence, and the one `fix:` (#73, phase 27) carries its own test. The
  next milestone's live close runs L1–L8 on its tree.

## Architecture references

- `docs/dev/STANDARDS.md` — the Definition of Done the splits must keep.
- `docs/dev/WORKFLOW.md` § "Executor self-sabotage on delete-heavy
  rewrites" — why moves are shell range cuts.
- `tests/doc_truth.rs` — the shape of a documentation gate the hygiene
  gate follows.
- `CLAUDE.md` § "Key files" — the module map each split updates.

## Phases

Drafted one at a time with `/rexymcp:architect next`. The gate first, then
the test-module moves (group 1), then the splits by concern (group 2, one
phase per file or pair), then `execute_tool_call` and
`run_conversation_loop` (group 3), then the live close and `2.4.0`.
Expected count: 12–14. Each may split at drafting.

| #  | Phase | Status |
|----|-------|--------|
| 01 | hygiene-gate ([phase-01-hygiene-gate.md](phase-01-hygiene-gate.md)) — `tests/hygiene.rs` measures files and functions with `syn`, the allowlist of today's offenders that may only shrink, the two exemptions | done |
| 02 | test-siblings-1 ([phase-02-test-siblings-1.md](phase-02-test-siblings-1.md)) — eight files' inline `mod tests` move to sibling `*_tests.rs` files by shell range cut; same test inventory, same pass count | done |
| 03 | test-siblings-2 ([phase-03-test-siblings-2.md](phase-03-test-siblings-2.md)) — seven more inline test modules to sibling files (`registry`, `log`, `file_ops/read`, `utils/mod`, `search`, `event_log`, `scheduled`); same inventory, same pass count | done |
| 04 | config-split ([phase-04-config-split.md](phase-04-config-split.md)) — `config/types.rs` (1466) into `types`, `sandbox`, `shells` and `models` by whole section; the first group-2 split | done |
| 05 | handlers-split ([phase-05-handlers-split.md](phase-05-handlers-split.md)) — the five named-session handlers leave `server/handlers.rs` (877) for a new `server/sessions.rs` | done |
| 06 | daemon-startup ([phase-06-daemon-startup.md](phase-06-daemon-startup.md)) — `run_daemon` and its two startup helpers leave `daemon/mod.rs` (901) for a new `daemon/startup.rs`, re-exported | done |
| 07 | epoch-extract ([phase-07-epoch-extract.md](phase-07-epoch-extract.md)) — memory extraction leaves `context/epochs.rs` (840) for a child module `epoch_extract.rs`, re-exported | done |
| 08 | slash-session ([phase-08-slash-session.md](phase-08-slash-session.md)) — the `/session` and `/runs` commands leave `cli/commands/slash.rs` (958) for a child module `slash_session.rs` | done |
| 09 | viewer-layout ([phase-09-viewer-layout.md](phase-09-viewer-layout.md)) — the transcript layout section leaves `cli/viewer.rs` (907) for a child module `viewer_layout.rs`, re-exported | done |
| 10 | tail-view ([phase-10-tail-view.md](phase-10-tail-view.md)) — the tail and log viewers leave `cli/tail.rs` (988) for a child module `tail_view.rs`, re-exported | done |
| 11 | ipc-types ([phase-11-ipc-types.md](phase-11-ipc-types.md)) — the payload structs leave `ipc.rs` (966) for a child module `ipc_types.rs`, glob re-exported; `ipc.rs` needs no exemption | done |
| 12 | ops-delete-copy ([phase-12-ops-delete-copy.md](phase-12-ops-delete-copy.md)) — the `delete` and `copy` operations leave `file_ops/ops.rs` (1018) for child modules `ops_delete.rs` and `ops_copy.rs`; one script does the code, allowlist and CLAUDE.md edits | done |
| 13 | top-tui-draw ([phase-13-top-tui-draw.md](phase-13-top-tui-draw.md)) — `top`'s drawing section leaves `cli/commands/top_tui.rs` (1050) for a child module `top_tui_draw.rs` | done |
| 14 | chat-loop ([phase-14-chat-loop.md](phase-14-chat-loop.md)) — the ratatui chat loop and line reader leave `cli/commands/chat.rs` (1147) for a child module `chat_loop.rs`; both giant functions move unchanged | done |
| 15 | index-split ([phase-15-index-split.md](phase-15-index-split.md)) — the queries and the rebuild leave `memory/index/mod.rs` (1135) for sibling modules `search.rs` and `reconcile.rs`, glob re-exported | done |
| 16 | index-tests ([phase-16-index-tests.md](phase-16-index-tests.md)) — `memory/index/mod.rs`'s 2543-line inline tests move into sibling `tests.rs` and `incremental_tests.rs`, each under the test cap | done |
| 17 | render-tests ([phase-17-render-tests.md](phase-17-render-tests.md)) — `cli/render_ratatui.rs`'s 2055-line inline tests move into sibling `render_ratatui_tests.rs` and `render_ratatui_layout_tests.rs` | done |
| 18 | render-draw ([phase-18-render-draw.md](phase-18-render-draw.md)) — `RatatuiRenderer`'s generic impl block leaves `cli/render_ratatui.rs` (1225) for a child module `render_ratatui_draw.rs`; a byte-identical move | done |
| 19 | shell-exec-auth ([phase-19-shell-exec-auth.md](phase-19-shell-exec-auth.md)) — credentials and connecting leave `executor/shell_exec.rs` (1129) for a child module `shell_exec_auth.rs`; `ensure_connected` re-exported | done |
| 20 | shell-exec-tests ([phase-20-shell-exec-tests.md](phase-20-shell-exec-tests.md)) — `executor/shell_exec.rs`'s 3091 test lines move into `shell_exec_tests.rs`, its child `shell_exec_hosted_tests.rs`, and `shell_exec_connect_tests.rs` | done |
| 21 | top-split ([phase-21-top-split.md](phase-21-top-split.md)) — `cli/commands/top.rs` (1394) splits into child modules `top_load.rs`, `top_text.rs` and `top_detail.rs`, glob re-exported | done |
| 22 | stream-split ([phase-22-stream-split.md](phase-22-stream-split.md)) — `ask_with_session_ratatui` and the prompt functions leave `cli/commands/stream.rs` (1520) for child modules `stream_ask.rs` and `stream_prompts.rs` | done |
| 23 | shell-readers ([phase-23-shell-readers.md](phase-23-shell-readers.md)) — the read/log and find/watch families leave `knowledge/shell.rs` (2627 → 1499) for child modules `shell_read.rs` and `shell_watch.rs`; the first of two | done |
| 24 | shell-rest ([phase-24-shell-rest.md](phase-24-shell-rest.md)) — input, screen and lifecycle leave `knowledge/shell.rs` (1499) for child modules `shell_input.rs`, `shell_screen.rs` and `shell_life.rs`; its `file` entry goes | done |
| 25 | shell-tests ([phase-25-shell-tests.md](phase-25-shell-tests.md)) — `knowledge/shell.rs`'s 3940-line inline tests move into `shell_tests.rs` and its two children `shell_lifecycle_tests.rs` and `shell_observer_tests.rs` | done |
| 26 | dispatch-families ([phase-26-dispatch-families.md](phase-26-dispatch-families.md)) — 35 of `execute_tool_call`'s arms move into five per-family `dispatch_*.rs` modules (schedule, artifacts, memory, shells, runs); the function goes 1439 → 1161 | done |
| 27 | tool-finished ([phase-27-tool-finished.md](phase-27-tool-finished.md)) — `fix:` for #73, the one recorded exception to decision 6: the four silent-tool arms that `return` early become labeled blocks, so `ToolFinished` always follows `ToolStarted`; a test pins it | done |
| 28 | dispatch-rest ([phase-28-dispatch-rest.md](phase-28-dispatch-rest.md)) — nine more arms move into five family modules (files, context, open_shell, send_input, meta); only `run_terminal_command` stays; `execute_tool_call` goes 1161 → 797 | done |
| 29 | foreground-split ([phase-29-foreground-split.md](phase-29-foreground-split.md)) — `run_terminal_command`'s 392-line foreground arm leaves `execute_tool_call` for `dispatch_foreground.rs`, split into `dispatch`, `run_approved`, `run_on_named_host` and a shared `finish_run`; `execute_tool_call` goes 797 → 409 | done |
| 30 | background-split ([phase-30-background-split.md](phase-30-background-split.md)) — `run_terminal_command`'s 163-line background arm leaves `execute_tool_call` for `dispatch_background.rs`, split into `dispatch` and `start_approved`; `execute_tool_call` goes 409 → 250 | done |
| 31 | route-split ([phase-31-route-split.md](phase-31-route-split.md)) — `execute_tool_call` splits into itself (the policy checks and the feedback frames), `load_snapshot` and `route` (the family match), the last two in a new `route.rs`; `execute_tool_call` 250 → 93, `executor/mod.rs` 944 → 788, and both allowlist entries are deleted | done |
| 32 | ghost-turn ([phase-32-ghost-turn.md](phase-32-ghost-turn.md)) — `do_ghost_turn` leaves `ghost.rs` for `ghost/turn.rs`, its 126-line prompt assembly becoming `build_ghost_prompt` in `ghost/prompt.rs`; `ghost.rs` 1677 → 782, `do_ghost_turn` 896 → 775 (re-pathed), the file entry deleted | done |
| 33 | conversation-loop ([phase-33-conversation-loop.md](phase-33-conversation-loop.md)) — `run_conversation_loop` leaves `stream.rs` for `stream/conversation.rs`, its 43 field-for-field tool arms becoming `PendingCall::from_tool_event` in the exempt `pending.rs` and three terminal replies becoming `stream/endings.rs`; `stream.rs` 1613 → 168, the loop 1454 → 775 (re-pathed), the file entry deleted | done |
| 34 | defs-exempt ([phase-34-defs-exempt.md](phase-34-defs-exempt.md)) — `src/ai/tools/defs.rs` joins `EXEMPT` (PE decision 2026-10-28) and its allowlist line is deleted; the allowlist then holds no file entries | done |

## Retrospective (2026-10-28)

**Outcome.** Every source file is within the 800-line production cap or
exempt as flat data, every inline test module over the cap lives in a
sibling file, and `tests/hygiene.rs` keeps it so. The two 1400-line tool
matches are gone: `execute_tool_call` is 93 lines over ten family modules,
and `run_conversation_loop`'s tool arms are `PendingCall::from_tool_event`.
The function cap is not met: 40 functions over 150 lines remain on the
allowlist, carried to the next milestone (see the amendment above). The
live close was waived.

- `tests/hygiene_allowlist.txt`: 66 entries at the gate (21 `file`, 4
  `test`, 41 `fn`) → 40, all `fn`.
- `src/`: 187 files and 139,087 lines at 2.3.0 → 257 files and 139,807
  lines. The 720 added lines are the new files' module declarations, `use
  super::*` and doc headers; no production code was retyped.
- The suite went from 2570 to 2587 tests, and no test was deleted or
  weakened: every move's test count was a pinned criterion.
- 85 findings in `FINDINGS.md`, every one triaged: 18 `act`, 51 `defer`,
  16 `reject`. One was fixed in M31 (#73, as the recorded exception to
  decision 6); the rest wait for a `fix:` milestone.
- `src/ai/tools/defs.rs` joined the exemption on 2026-10-28; `src/ipc.rs`,
  the other candidate, was split instead (phase 11).

**Numbers** (from the telemetry store). 34 phases, 38 recorded runs, 3,136
executor turns, about 9.5 hours of executor wall clock, 4.4 M input
tokens, all on `nvidia/GLM-5.3-Flash-FP4-Local`.
- **30 `approved_first_try`:** 02, 03, 05–07, 09–27, 29–34.
- **2 `approved_after_1`:** 08 (bug-08-1, a missing End-to-end paste; 19
  turns to fix) and 28 (a stall during the Findings audit, resumed with
  dictated commands; 20 turns).
- **2 `escalated`:** 01 and 04, each after two `NoProgressStall` runs,
  finished by the architect.
- **5 hard-failed runs,** all stalls: 01 twice (writing a 400-line file
  against an unfamiliar crate API), 04 twice (finding its own cut
  boundaries), 28 once (an open-ended "read the arms" audit).
- **1 bug doc** (bug-08-1).
- Turns per phase fell as the spec shape settled: 120–131 for the
  described moves of 05, 10, 11 and 25; 29–66 for the dictated moves of
  29–34.

**What worked: the dictated cut.** From phase 12 on, a phase's Spec is one
script the architect wrote and dry-ran on a scratch worktree: the
anchors, the Python or sed that turns them into line ranges, the new
files, the `mod` lines, the allowlist and `CLAUDE.md` edits, then `cargo
fmt`. The End-to-end block compares every moved segment, whitespace
removed, with the original at the baseline commit, and is mutation-tested
before dispatch. The executor's work is the log, the Findings audit and
the commit. Twenty-three phases ran that way; all passed first try except
28, whose stall was in the audit, not the cut. The dry runs caught the
architect's own errors before dispatch: a `usize`/`u32` mismatch (29),
helpers left in a file that may not grow (31), an `#[allow]` attribute
not moved with its function (32), a segment check that missed because
rustfmt drops closure braces (29) or trailing commas (31), and a count of
43 arms written as 44 (33). Every one would have been a bounce.

**What stalled: discovery.** The five hard fails share one shape: the
Spec sent GLM to find something (an API, a boundary, a callee) with no
stopping point. 01's second run re-verified a compiled excerpt by reading
the crate's source; 04 could not fix its boundaries; 28 followed every
callee into other modules. In each case the same work, given as exact
commands with a stated budget, finished in 20–40 turns.

**Architect errors, recorded.**
- A criterion that contradicted its own Spec: 01's `exempt_listed`.
- A described move where a dictated one was needed: 04, twice.
- An open-ended audit step on arms that call into many modules: 28.
- Two telemetry records with `stall` as the failure class before the
  fixed vocabulary was checked (01, 04); the store is append-only, so they
  stand beside the corrected records.
- Spec steps the executor could satisfy with the wrong tool or heading:
  log entries appended with `cat >>` (34) or filed under the wrong heading
  (33). Content complete in each case, so none bounced.

**Process calibration.**
- **The governor is blind to bash writes** (rexyMCP issue #14, from 04):
  a run that edits through `python3 - <<'PY'` makes no edit-tool call,
  so its stall counter never resets. Every dictated-script phase since
  tells the executor to log with an edit tool at once. Carried to rexyMCP.
- **Completion summaries claim transcripts the log does not hold** (07
  accepted, 08 bounced, 33 and 34 filed under other headings). The server
  now authors the completion entry; it could append the End-to-end
  block's output the same way. Four occurrences; a rexyMCP product item,
  not a WORKFLOW rule, and filed there.
- **`cargo test` in a tree with a failing gate stops at the first failing
  binary,** so a `passed=` count from such a run is low, not wrong (31).
- **A dry run's `CARGO_TARGET_DIR` goes on disk,** not in `/tmp`: a 7 GB
  target directory on the 14 GB tmpfs filled it on 2026-10-26.

**Fold proposals.**
- **Proposal A — a mechanical move is dictated, not described.** Five
  stalls on described moves and audits (01 ×2, 04 ×2, 28) against
  twenty-two first-try dictated ones. Proposed for `WORKFLOW.md` beside
  § "Executor self-sabotage on delete-heavy rewrites": *"A phase that
  moves or splits code dictates the cut as one script the architect has
  dry-run on a scratch worktree: the anchors and the commands that turn
  them into ranges, every file the move touches (the allowlist, the docs
  table), then the formatter. The End-to-end block proves the moved text
  identical to the baseline, whitespace removed, and is mutation-tested
  before dispatch. The executor's tasks are the log entries, a Findings
  audit given as exact commands with a call budget, and the commit."*
  **Pending PE sign-off; not folded.**
- **Held as data:** the server-appended End-to-end output (rexyMCP), and
  a one-line "every log entry goes through an edit tool" reminder (two
  slips, 33 and 34, neither a bounce).

**Deferred.**
- The 40 function entries: the next milestone's scope.
- The 51 `defer` findings, #84 (the double `Config::load()` per foreground
  call) and #85 (the `ai_turn` event's model on tool-call turns) among
  them: a `fix:` milestone, or folded into the function work where a split
  exposes the seam.
- Carried from M30 and earlier, unchanged: the `chat` fresh-root
  observation, O5, O6, F8, and the M28 `wait: false` note.

## Notes

- **The gate is in, with 66 entries (phase 01, 2026-10-25):** 21 `file`,
  4 `test`, 41 `fn`. Every later phase deletes lines from
  `tests/hygiene_allowlist.txt`; the exit criterion is an empty list.
  Inline test modules count against the 2000-line test cap, so group 1's
  moves are enforced too.
- **The audit's giant-function list was short.** The gate also found
  `do_ghost_turn` (896), `handle_ask` (573), `run_background` (440),
  `run_status` (435) and `get_labeled_context_scoped` (321). Group 3 grows
  to cover them.
- **Exemption settled (2026-10-25):** it covers functions too, so
  `pending.rs`'s `to_tool_call` (617) and `summary` (188) left the
  allowlist, which now holds 66 entries.

- **`defs.rs` exempted (PE decision 2026-10-28):** `src/ai/tools/defs.rs` (1398 lines, the flat `TOOLS` table, one `ToolDef` per AI tool) joins `pending.rs` and `args.rs` under decision 2. The split would scatter one table across files that nothing else distinguishes, and `tests/doc_truth.rs` already holds it to the README. The `EXEMPT` entry and the allowlist deletion land in the phase after `stream.rs`.
- **#73 settled (PE decision 2026-10-28):** four silent-tool arms (`read_file`, `get_shell_context`, `open_shell`, `spawn_ghost_shell`) `return` early and skip `ToolFinished`, so the client's panel for the call is lost or unlabelled; `get_shell_context` hits it on every call from outside tmux. The arms cannot leave `execute_tool_call` without changing that, so the fix comes first as its own `fix:` phase with a test (the one recorded exception to decision 6), and the move follows as a pure move. Options weighed: fix inside the move (mixes a fix into a move diff) and preserving the skip with an early-exit result (plumbing to keep a bug alive into 2.4.0). See [`FINDINGS.md`](FINDINGS.md) #73.
- **M31 retrospective (2026-10-28):** one fold proposed (A, the dictated
  cut), pending PE sign-off; two items held as data; two rexyMCP product
  items carried (#14, the server-appended End-to-end output).
