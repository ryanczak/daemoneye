# NEXT

**Active phase:** `docs/dev/milestones/M22-tool-surface/phase-09b-pane-watch-removal.md`
— 09b, pane-watch-removal. Drafted 2026-09-16, status `todo`, not yet
dispatched.

Deletes `watch_pane` (replaced by `watch_shell`, 04b) and
`close_background_window` (`close_shell`, 04a). **45 → 43 tools: 31 → 29 core,
14 deferred unchanged.** `doc_truth` stays at 10.

09a's calibration was applied up front: the **orphan closure** was measured
before the authorization list was written, and it is empty outside the
authorized files — only `WatchHookGuard` is orphaned and it lives in
`knowledge/pane.rs`. Every shared helper (`FG_HOOK_COUNTER`,
`bg_done_subscribe`, `capture_pane`, `off_runtime`, …) keeps live callers in
`foreground.rs`. The doc also tells the executor what to do if the lint gate
still finds an orphan the measurement missed.

The sharpest must-survive item: `src/daemon/server/catchup.rs` scans for
`[Watch Pane` in session messages and must be left **completely** alone.
`knowledge/pane.rs` is its only producer, but session JSONL files already on
disk carry those lines and catch-up briefs still have to surface them. Deleting
the now-producer-less scanner would silently drop events from resumed old
sessions with no gate to catch it.

Remaining M22 order: **09b → 09c → 09d → 08b → 10**. 09c (`tmux_control`) is
what reaches the milestone's exit criterion of 42 tools.

Dispatch with `/rexymcp:dispatch phase-09b`.
