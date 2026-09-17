# NEXT

**Active phase:** `docs/dev/milestones/M22-tool-surface/phase-09a-pane-read-tool-removal.md`
— 09a, pane-read-tool-removal. Drafted 2026-09-16, status `todo`, not yet
dispatched.

Deletes `list_panes`, `read_pane` and `find_in_panes` — the three pane **read**
tools, whose replacements (`list_shells` 03a, `read_shell` 03b,
`find_in_shells` 03c) all shipped. **48 → 45 tools: 34 → 31 core, 14 deferred
unchanged.** `doc_truth` stays at 10.

Charter row 09 was split at drafting into 09a–09d; rationale and the measured
reference counts are in the milestone README's Notes. The ordering for the rest
of M22 is **09a → 09b → 09c → 09d → 08b → 10**: 08b renames
`get_terminal_context` and must land after the pane tools are gone.

Dispatch with `/rexymcp:dispatch phase-09a`.
