# NEXT

**Active phase:** none — `phase-10a-prompt-execution-prose` was approved
2026-09-18 (`approved_first_try`). The seeded prompts no longer carry the three
false instructions, and `tests/doc_truth.rs` now gates prompt-asset tool names:
no retired tool name may appear in either prompt, and every tool heading in
`sre.toml` must name a live tool.

**One M22 row remains: 10b** (prompt-assets-seeded, undrafted) — retire
`assets/memory/knowledge/tmux-pane-toolkit.md` and
`assets/memory/session/pane-referencing-convention.md` with their `seeds.rs`
constants and seeding calls, clean the four other memories that mention panes
and the one agent config, correct `docs/REQUIREMENTS.md` (3 stale
`get_terminal_context` references and a pane-heavy FR-1.2.2), and widen 10a's
gate to every asset.

Run `/rexymcp:architect next` to draft it. It is M22's last row.
