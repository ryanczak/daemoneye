# Pane Referencing Convention

- **User Context:** The user uses `CTRL+a q` to view tmux pane indices.
- **Protocol:** Always refer to panes by their **window-relative index** (e.g., "pane index 0 in 'bash'") when communicating with the user.
- **Daemon Handling:** When performing tool actions, I will map these human-friendly indices back to the internal tmux pane ID (`%N`) automatically.
- **Clarity:** Ensure every instruction or diagnostic step explicitly includes the index to prevent confusion.