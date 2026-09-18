# NEXT

**Active phase:** `docs/dev/milestones/M22-tool-surface/phase-08b-shell-context-tool.md`
— 08b, shell-context-tool. **Drafted 2026-09-18**, status `todo`.

Renames `get_terminal_context` to `get_shell_context` and replaces its scope
vocabulary with `"shells"` / `"session"` / `"all"`, refusing the retired
`"window"` and the design's `"run"` rather than aliasing either. The `"shells"`
breadth returns 08a's `[SHELLS]` / `[DEFAULT SHELL]` block alone and makes no
tmux call. No tool count change: 42 tools, 28 core + 14 deferred.

Every acceptance criterion was run against HEAD `26804ecf` and fails there.

Dispatch with `/rexymcp:dispatch phase-08b`.

After 08b, one M22 row remains: **10** (prompt-assets), which also owns the
stale `get_terminal_context` references 08b deliberately leaves in
`docs/REQUIREMENTS.md` and `assets/memory/knowledge/tmux-pane-toolkit.md`.
