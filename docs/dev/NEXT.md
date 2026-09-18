# NEXT

**Active phase:** `docs/dev/milestones/M22-tool-surface/phase-10a-prompt-execution-prose.md`
— 10a, prompt-execution-prose. **Drafted 2026-09-18**, status `todo`.

Row 10 split into 10a/10b at drafting. Measuring found the seeded prompts carry
three **false** instructions rather than merely tmux-flavoured prose:

1. `sre.toml`'s two-mode section describes the tmux path as the only path,
   though both run modes route to the PTY substrate by default since phase 01.
2. `ghost-shell.txt` rule 7 tells every ghost to call `watch_pane`, deleted in
   phase 09b. `watch_shell` is not the fix — a ghost's background command has
   no shell id.
3. `background/run.rs` tells the model at two sites to use a `target=`
   parameter that no tool has ever had.

10a fixes those and adds a `doc_truth` gate pinning prompt-asset tool names.
Every acceptance criterion was run against HEAD `35fe59cc` and fails there.

Dispatch with `/rexymcp:dispatch phase-10a`.

**10b** (undrafted) then retires the two tmux memories and their `seeds.rs`
constants, cleans the four other memories and the one agent config, corrects
`docs/REQUIREMENTS.md`, and widens the gate to every asset. It is M22's last row.
