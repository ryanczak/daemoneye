# NEXT

**Active phase:** **M23 phase-06d — log-replay**, drafted 2026-10-03,
status `todo`. Dispatch with `/rexymcp:dispatch phase-06d`.

**What it does.** `space` in a `/log` view replays the recording at its
recorded timing, with every pause capped at 2 s. While it runs, `space`
pauses and resumes, `Ctrl-C` closes, and any other key skips to the end.
When the replay finishes, the full log and its `n`/`p` marks are back.
Client only: 2 files, 7 new tests; lib 2170 → 2177.

**Measured at drafting.** Real recordings: 352 s raw replays in 10.1 s
capped. Two gotchas are pinned by tests and mutants: a clock that keys do not
reset counts the idle time before `space`, and focus or mouse reports would
skip the replay. The end-to-end block was run on the dry-run tree. Mutants
A–F went red 3/3, all clippy-clean. The doc's diffs reproduce the dry run.

**After 06d:** row 06 is complete. `/rexymcp:architect next` drafts row 07
(agents-and-attach).
