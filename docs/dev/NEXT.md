# NEXT

**Active phase:** `docs/dev/milestones/M22-tool-surface/phase-10b-prompt-assets-seeded.md`
— 10b, prompt-assets-seeded. **Drafted 2026-09-18**, status `todo`.
**This is M22's last row.**

Widens 10a's retired-tool gate from the two prompt assets to every file under
`assets/` plus `docs/REQUIREMENTS.md`, then fixes the four files the census
found:

1. `assets/memory/knowledge/tmux-pane-toolkit.md` — all seven retired names,
   19 occurrences. Deleted, with its `seeds.rs` constant and all three other
   code sites.
2. `assets/memory/session/pane-referencing-convention.md` — two bullets about
   deleted tools removed; the rest is still true and stays.
3. `assets/agents/sysadmin/config.toml` — the seeded agent tells its ghosts to
   call `watch_pane`, deleted in 09b. Same live defect 10a fixed in the ghost
   prompt, in a file 10a could not touch.
4. `docs/REQUIREMENTS.md` — three stale tool names renamed, two capabilities
   marked superseded. No requirement is deleted; that is a PE decision.

Every acceptance criterion was run against HEAD `effd4e69` and fails there.

Dispatch with `/rexymcp:dispatch phase-10b`.

When 10b is approved, every M22 row is `done` and the milestone is ready to
close via `/rexymcp:architect`.
