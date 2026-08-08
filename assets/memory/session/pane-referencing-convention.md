# Pane Referencing Convention

- **User Context:** The user uses `CTRL+a q` to view tmux pane indices (0-based, window-relative).
- **Protocol:** Always refer to panes by their **window-relative index** (e.g., "pane index 0 in 'bash'") when communicating with the user.
- **`[PANE MAP]` line:** Every turn includes a `[PANE MAP]` line with the current `idx:N=<pane-id>` mapping. Use this to resolve user-spoken pane numbers to tmux pane IDs for tool calls.
- **Other sessions:** Panes outside the user's own tmux session are labelled with `session:<name>`. Name the session when you mention one — "pane index 1 in 'editor' (session `work`)" — because an index alone is ambiguous across sessions. **A foreign-session pane is not a valid foreground target**; `run_terminal_command` and the `/pane` pin only apply to the user's own session.
- **Look before asking.** If you need output the context block only summarises, read it: `read_pane` for a known pane, `find_in_panes` when you don't yet know which pane holds it. Ask the user to paste output only after those have failed.
- **Ask first on ambiguity:** If the target pane for a foreground command cannot be determined unambiguously from `[PANE MAP]` or context, ask the user which pane to use before calling `run_terminal_command`.
- **Use the status, don't infer it:** Each pane carries a live `status:` (`Running`, `Idle 4m`, `AwaitingInput`, `Bell`, `Dead(<code>)`). Trust it over guessing from the last output line — an idle shell is not a prompt waiting on input.
- **Moving the user is an action, not a courtesy.** `tmux_control` (focus/zoom/split/rename/kill) changes what the user is looking at and is approval-gated. Prefer telling them where to look; use the tool when being taken there is genuinely more useful.
- **Clarity:** Refer to target panes as "pane index N in 'window' (%ID)" so the user can visually confirm before approving.
