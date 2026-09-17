# NEXT

**Active phase:** none.

Phase-09a closed `approved_first_try` on 2026-09-16 — `list_panes`, `read_pane`
and `find_in_panes` are gone. **45 tools: 31 core + 14 deferred** (from 48 =
34 + 14); `doc_truth` 10; `cargo test --lib` 1886. `pane.rs` went 1417 → 415
lines, keeping exactly `close_bg_window`, `is_daemon_window`,
`kill_window_refusal` and `watch_pane`.

Next up is **09b — pane-watch-removal** (`watch_pane`, replaced by `watch_shell`
from 04b, plus `close_background_window`), **not yet drafted**. 45 → 43.

**Carry into the 09b draft** (calibration from 09a, in the milestone README's
Notes): a removal phase must authorize the **orphan closure** of what it
deletes, not just the call sites. 09a's authorization list forbade `src/tmux/`,
but removing `read_pane`/`find_in_panes` orphaned
`tmux::capture_pane_annotated`, and a `pub fn` with no callers fails
`cargo clippy -D warnings` — so the phase could not satisfy both its
authorizations and its lint gate. `watch_pane` and `close_background_window`
both reach into `crate::tmux`, so grep for their last-caller helpers before
writing 09b's authorization list.

Remaining M22 order: **09b → 09c → 09d → 08b → 10**.

Run `/rexymcp:architect next` to draft phase-09b.
