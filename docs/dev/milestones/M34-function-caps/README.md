# M34 — Function caps

**Goal:** no function in DaemonEye is longer than 150 lines. The 40 `fn`
entries in `tests/hygiene_allowlist.txt` are split until each is within the
cap and its line is deleted, and the six M32 findings deferred here are
fixed alongside the functions they live in.

**Status:** withdrawn (scoped 2026-10-31, withdrawn 2026-10-31 after phase 01)

**Release:** 2.7.0

**Depends on:** M33 (the tree as released in 2.6.0).

**Why now.** M31 put the caps in `tests/hygiene.rs` and allowlisted
today's offenders; M32 deferred the function work and six findings to
this milestone. Long functions are where the M32 defects hid (the copy
paths, the dispatch prologues). Splitting them also gives later milestones
(MCP surface, Knowledge) seams to change without re-reading 700 lines.

**PE decisions (2026-10-31):**

1. **All 40 functions**, in about 20 phases, ordered by risk: the small
   CLI functions first, the turn loops last.
2. **The six deferred findings ride with their functions** (#36, #37, #40
   with `run_copy_with_spawn`; #75, #78, #79 in their own phase). Each fix
   has a test that fails first. Every other phase is behaviour-preserving.
3. **Numbering:** this is M34; MCP surface becomes M35 and Knowledge M36.

**Rules every phase carries:**

- **A split preserves behaviour.** The suite count does not change in a
  split-only phase (state the exact number in the phase doc), and no test
  is rewritten to fit the new shape. A test that has to move with the code
  moves verbatim.
- **The allowlist line goes.** A phase deletes the line of each function it
  brings within 150; the gate fails with "remove it" until it does. No
  entry may grow, and none may be added, for functions or files.
- **File headroom.** Many host files are near the 800-line production cap
  (`executor/mod.rs` 788, `ghost.rs` 783, `ghost/turn.rs` 782,
  `tmux/cache.rs` 781, `shell_exec.rs` 777, `main.rs` 754). When a split
  would push a file past 800, the helpers go in a new child module, as
  M31 did (`foo.rs` plus `foo/bar.rs`, or a sibling `foo_bar.rs` declared
  from `foo.rs`). The phase doc names the module.
- **Helpers are named for what they do,** take the state they need as
  arguments or a small struct, and stay private (`fn` or `pub(super)`).
  No new trait, no new crate dependency.
- **Edits use `patch` with whole blocks,** never `patch_lines`, on these
  long files (M27 05b lost the accept loop to shifting line edits).
- **A new child module gets a row in `CLAUDE.md` § Key files.**

**Exit criteria:**

- `grep -c '^fn ' tests/hygiene_allowlist.txt` prints `0`; the allowlist
  file holds only its header comment.
- The four gates pass; `tests/hygiene.rs` and `tests/doc_truth.rs` pass.
- `cargo test` passes at 2618 tests plus the findings' new tests, and no
  test was deleted.
- The six findings (#36, #37, #40, #75, #78, #79) are marked fixed in
  `docs/dev/milestones/M32-findings-fixes/FINDINGS.md`, each naming its
  test.
- Live check on pinky, because the turn loops are split here: a chat turn
  with an approved command, an `ask`, a webhook-triggered ghost run, and a
  daemon restart with `chat --resume`.
- `2.7.0` is released per `REXYMCP.md` § Versioning.

## Architecture references

- `tests/hygiene.rs` (how a function's lines are counted: signature to
  closing brace) and `tests/hygiene_allowlist.txt`.
- `docs/dev/milestones/M31-code-hygiene/README.md` for the child-module
  pattern used in the file splits.
- `docs/dev/milestones/M32-findings-fixes/FINDINGS.md` rows #36, #37, #40,
  #75, #78, #79.

## Phases

Drafted one at a time with `/rexymcp:architect next`. Expected count: 20.
Line counts are the allowlist's.

| #  | Phase | Scope | Status |
|----|-------|-------|--------|
| 01 | cli-small-a ([phase-01-cli-small-a.md](phase-01-cli-small-a.md)) | `activity_row_in` 163, `aggregate_costs` 159, `cmd_session` 155, `search_memory_fts` 154 | done        |
| 02 | cli-small-b ([phase-02-cli-small-b.md](phase-02-cli-small-b.md)) | `run_setup` 256, `async_main` 173, `run_ask_raw` 169 | cancelled |
| 03 | backend-chat | `GeminiClient::chat` 263, `AnthropicClient::chat` 252, `OpenAiClient::chat` 238 | withdrawn |
| 04 | file-ops | `run_edit_with_spawn` 271, `run_read_file` 165, `run_create_with_spawn` 164 | withdrawn |
| 05 | copy-and-findings | `run_copy_with_spawn` 213; findings #36, #37, #40 | withdrawn |
| 06 | dispatch-findings | findings #75, #78, #79 (no allowlist line) | withdrawn |
| 07 | executor-core | `run_in_shell_core` 250, `prompt_and_await_approval` 210 | withdrawn |
| 08 | shell-readers | `run_schedule_command` 184, `read_to_marker` 163, `watch_shell_with` 159 | withdrawn |
| 09 | alert-and-schedule | `maybe_analyze_alert` 240, `run_scheduled_job_inner` 193, `trigger_ghost_turn` 172 | withdrawn |
| 10 | viewer-and-markdown | `viewer_loop` 234, `render_line_to_spans` 219, `lang_keywords` 216 | withdrawn |
| 11 | server-and-context | `handle_client` 214, `auto_search_context` 211, `run_compaction` 184 | withdrawn |
| 12 | tmux-cache | `get_labeled_context_scoped` 321, `SessionCache::refresh` 171 | withdrawn |
| 13 | status | `run_status` 435, `handle_status` 200 | withdrawn |
| 14 | run-background | `run_background` 440 | withdrawn |
| 15 | chat-loop | `run_chat_ratatui` 500, `read_input_line_inner_ratatui` 183 | withdrawn |
| 16 | handle-ask | `handle_ask` 573 | withdrawn |
| 17 | run-daemon | `run_daemon` 598 | withdrawn |
| 18 | ask-with-session | `ask_with_session_ratatui` 673 | withdrawn |
| 19 | ghost-turn | `do_ghost_turn` 775 | withdrawn |
| 20 | conversation-loop | `run_conversation_loop` 775 | withdrawn |

## Notes

- **Withdrawn 2026-10-31.** PE decision: "i don't think forcing functions
  to an arbitrary length makes sense. let's not do this." Phase 01 had
  landed (four functions split, approved, behaviour held at 2618 tests) and
  stays as committed; phase 02 was stopped at turn 16 with no edit on disk.
  The 36 remaining `fn` allowlist entries are not a backlog. The six M32
  findings folded into this milestone (#36, #37, #40, #75, #78, #79) return
  to the M32 FINDINGS deferred list with no owner. The `FN_CAP` gate in
  `tests/hygiene.rs` stays as a ratchet (PE, 2026-10-31): no listed
  function grows and no new function passes 150; a future phase that needs
  a longer one adds an allowlist line with its reason.

- **Scoped 2026-10-31**, after the M33 close. The phase order puts the
  cheapest splits first so the lean split spec is calibrated on 154-line
  functions before the 775-line turn loops.
