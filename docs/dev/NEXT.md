# NEXT

**Active phase:** **M23 phase-10a — readme-slash-commands**, drafted
2026-10-04, status `todo`. Dispatch with `/rexymcp:dispatch phase-10a`.

**What it does.** README.md gains the eleven M23 slash commands (`/shells`,
`/hosts`, `/tail`, `/logs`, `/log`, `/attach`, `/agents`, `/shell`, `/runs`,
`/run`, `/stop`), the shell-viewer and attached-mode key tables, and a note
that tmux is optional. A new doc_truth test fails the build when any
command in `/help` or the slash dispatcher is missing from the README
section. No production code; doc_truth 14 → 15, lib unchanged at 2219.

**Measured at drafting.** The scan finds 28 commands. A naive regex picked up
argument lists (`/on`, `/p`), so a command must start a word. The
end-to-end block was run on the dry-run tree. Mutants A–C went red 3/3, all
clippy-clean. The doc reproduces the dry run.

**After 10a:** row 10b, the live close, is architect-run with the PE at the
keyboard. Then the milestone close (retrospective and folds, PE sign-off).
