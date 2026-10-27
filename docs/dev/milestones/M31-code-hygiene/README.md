# M31 — Code hygiene

**Goal:** no source file or function is too long to read in one sitting, and
a test keeps it that way: production code is at most 800 lines per file,
tests live in sibling files, no function is over 150 lines, and the two
1400-line tool dispatch matches are decomposed into per-family modules.

**Status:** in-progress (scoped 2026-10-25)

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
   `src/ai/tools/args.rs` (forty argument structs). The exemption covers
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
| 17 | render-tests ([phase-17-render-tests.md](phase-17-render-tests.md)) — `cli/render_ratatui.rs`'s 2055-line inline tests move into sibling `render_ratatui_tests.rs` and `render_ratatui_layout_tests.rs` | review      |

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
