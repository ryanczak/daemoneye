# NEXT

**Active phase:** **M21 phase-07b — edit-file-host**, drafted 2026-09-26,
status `todo`. Dispatch with `/rexymcp:dispatch phase-07b`.

**What it closes.** The four `edit_file` write branches move onto 07a's
`run_in_host_shell`, and `target_pane` leaves the tool surface entirely
(1 → **0**), closing the milestone's *"`target_pane` is gone"* exit
criterion. With the last pane consumer gone, the pane-era helpers are
deleted.

**07a's false premise is corrected here, from a measured call graph.** 07a's
doc claimed the write ops needed the read-path helpers and lumped
`get_pane_remote_host` in with them. Grepped at this drafting:

| Helper | Real callers | 07b |
|---|---|---|
| `remote_run_and_capture` | 4, all in `ops.rs` — the branches being converted | delete |
| `local_read_via_buffer` | **0** (already dead, carries an allow) | delete |
| `build_local_buffer_read_cmd` | only the above, plus one test | delete with it |
| `get_pane_remote_host` | **`prompt.rs:116`, `foreground.rs:219`** | **KEEP** — nothing to do with file ops |

Both `#[allow(dead_code)]` in `read.rs` go away **by deletion**, and a
criterion pins `get_pane_remote_host`'s two callers so the correction cannot
be undone by enthusiasm.

**The security mutation is the important one.** Every write op prompts via
`EditFilePrompt` before touching anything. Mutation A removes that from the
host path — the tempting simplification when adding a transport — and must
turn `a_remote_write_still_prompts_for_approval` red.

**One criterion is deliberately a range, and the spec says why.** Task 3
deletes a test while Task 5 adds eleven, so the net lib count is 2024 or
2025 depending on the live-test choice. The architect cannot know it without
running the tasks, so the spec asks the executor to **run it and pin what it
gets, stating the arithmetic** — rather than guessing a number the executor
must then contort to hit.

**Pre-dispatch check run; five criteria confirmed failing**
(`target_pane_params=1`→0, `edit_file_host=0`→1, `pane_branches_gone=4`→0,
`dead_helpers_gone=2`→0, `dead_code_allows=2`→0) with
`pane_host_helper_kept=2` correctly passing as preservation.

**The post-phase simulation caught a real contamination before dispatch.**
The original criterion was `grep -c 'remote_run_and_capture' ops.rs → 0`.
Simulating the corpus — a doc comment saying *"the pane transport
`remote_run_and_capture` is gone"* — pushed it from 0 to **1**, i.e. the
criterion would have read as a failed deletion for a correct tree. Replaced
with `grep -c 'if let Some(pane) = target_pane' → 0`, which pins the same
property on a **branch line that prose cannot contain**, and the spec records
why. That is the 04d/05/06c/06d class caught at drafting rather than review.

**Next after 07b: 08 — ghost-host-mapping**, then 08b, 09.

Nineteen rows in M21; live host `pinky` (sudo requires a password since
2026-09-22); `max_turns` 1000.
