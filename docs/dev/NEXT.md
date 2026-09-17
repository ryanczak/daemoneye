# NEXT

**Active phase:** `docs/dev/milestones/M22-tool-surface/phase-09c-tmux-control-removal.md`
— 09c, tmux-control-removal. Drafted 2026-09-17, status `todo`, not yet
dispatched.

Deletes `tmux_control`, the last of the six pane tools and the only one with no
shell counterpart — it *drives* tmux rather than reading it. This is the phase
that reaches the milestone's headline exit criterion: **42 tools, 28 core + 14
deferred**.

Three things move that 09a and 09b never touched:

- **`APPROVAL_GATED_TOOLS` shrinks 11 → 10** and the README's `⚠` marker goes
  with it — the add-a-tool checklist's step 12, guarded by
  `readme_approval_markers_match_the_gated_tools`. Task 8's mutation exercises
  exactly that pairing.
- **`src/daemon/executor/knowledge/pane.rs` is deleted outright.** After 09a and
  09b it holds only `is_daemon_window` and `kill_window_refusal`, and both exist
  solely for `tmux_control`'s `kill_window` action.
- **09b's own E2E test lists `tmux_control` as must-stay** and fails the moment
  the tool leaves `TOOLS`. The spec calls that out with the line number.

Both folds from 09a/09b were applied while drafting: the orphan closure was
measured over functions, wrappers **and** `use` statements (it found that
`explicitly_allows` *survives*, because `ghost_may_send_input` is a second
caller — the doc comment calling it "the ghost `tmux_control` gate" is
misleading), and the criteria are written against identifiers rather than
tool-name strings, with the one string criterion anchored away from the test
file.

Remaining M22 order: **09c → 09d → 08b → 10**.

Dispatch with `/rexymcp:dispatch phase-09c`.
