# NEXT

**Active phase:** **M23 phase-01b — prompt-wait-outside-deadline**, drafted
2026-09-30, status `todo`. Dispatch with `/rexymcp:dispatch phase-01b`.

**What it does.** `read_to_marker` adds the time spent inside the prompt
callback (where the password round trip blocks) back to the command's
deadline. A slow typist is no longer killed after their password was already
sent, and a command that hangs after being answered still times out on
schedule. It also deletes 09b's `SUDO_EDIT_TIMEOUT_SECS` workaround (now
false) and corrects 01a's residual comment. Two new tests: 2092 / 14.

**Measured at drafting.** Today, a 2 s command answered at 3 s fails "did not
complete within the 2s limit" after the answer was written. With the fix it
returns exit 0 (`GOT_15`). An instantly answered prompt followed by
`sleep 30` still times out at 2.00 s. The client and shell_exec suites pass
with the fix.

**After 01b:** `/rexymcp:architect next` drafts 01c (file-op-error-label).
