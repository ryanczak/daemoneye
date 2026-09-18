# NEXT

**Active phase:** none — `phase-08b-shell-context-tool` was approved 2026-09-18
(`approved_first_try`). `get_terminal_context` is now `get_shell_context` with
the `"shells"` / `"session"` / `"all"` vocabulary, and the retired `"window"`
and the design's `"run"` are refused rather than aliased. Tool counts unchanged
at 42 / 28.

**One M22 row remains: 10** (prompt-assets) — rewrite `sre.toml`,
`assets/prompts/ghost-shell.txt`, the seeded memories (retiring the two tmux
knowledge ones) and the seeded agent configs, and correct
`docs/REQUIREMENTS.md`. It also owns the four stale `get_terminal_context`
references 08b deliberately left in `docs/REQUIREMENTS.md` (3) and
`assets/memory/knowledge/tmux-pane-toolkit.md` (1).

Run `/rexymcp:architect next` to draft it.
