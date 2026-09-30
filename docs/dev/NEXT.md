# NEXT

**Active phase:** **M23 phase-01c — file-op-error-label**, drafted
2026-09-30, status `todo`. Dispatch with `/rexymcp:dispatch phase-01c`.

**What it does.** A sudo edit that fails to run gets its own error wording,
naming the file and host and what to do next. It no longer says "in shell
edit_file" or advises a `shell` parameter `edit_file` doesn't take. The shared
`shell_run_error_text` stops doubling its full stop (`…chat session.. The`),
which affected `run_terminal_command` too. Three new tests: 2095 / 14.

**Measured at drafting.** All three error branches were rendered as the sudo
edit sees them. The spec's exact helper code and every test assertion were
compiled and run against the three error shapes before writing.

**New criterion this time:** the paste-check line must be in the doc on a line
of its own (`grep -cx 'PASTE MATCH'` → 1). The last two phases reported the
check without printing it into the doc.

**After 01c:** `/rexymcp:architect next` drafts 01d (session-log-retention).
