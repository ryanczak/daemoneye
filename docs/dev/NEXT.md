# NEXT

**Active phase:** **M23 phase-06b — shell-log-read**, drafted 2026-10-03,
status `todo`. Dispatch with `/rexymcp:dispatch phase-06b`.

**What it does.** The daemon answers `ShellLogRead{id}` with one recording:
its size, every output event with its time, and where each indexed command
begins. A command begins at the first output after its typed line, so the
prompt and the command are in view. This is the data the `/log` viewer
(06c, `n`/`p`) and replay (06d) consume. Daemon side only, 5 files, 4 new
tests; lib 2157 → 2161.

**Measured at drafting.** Probed on three real recordings: every mark landed
on the typed command's echo. One mutant was unobservable and was dropped.
Every pinned mutant was checked clippy-clean (the 06a lesson). The spec's
per-file diffs were verified to reproduce the dry run.

**After 06b:** `/rexymcp:architect next` drafts 06c (log-viewer).
