# Pane Referencing Convention

- **User Context:** The user uses `CTRL+a q` to view tmux pane indices (0-based, window-relative).
- **Protocol:** Always refer to panes by their **window-relative index** (e.g., "pane index 0 in 'bash'") when communicating with the user.
- **`[PANE MAP]` line:** Every turn includes a `[PANE MAP]` line with the current `idx:N=<pane-id>` mapping. Use this to resolve user-spoken pane numbers to tmux pane IDs for tool calls.
- **Ask first:** If the target pane for a foreground command cannot be determined unambiguously from `[PANE MAP]` or context, ask the user which pane to use before calling `run_terminal_command`.
- **Clarity:** Refer to target panes as "pane index N in 'window' (%ID)" so the user can visually confirm before approving.