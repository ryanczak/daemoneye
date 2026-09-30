# NEXT

**Active phase:** **M21 phase-09b — remote-sudo-edit**, drafted 2026-09-30,
status `todo`. Dispatch with `/rexymcp:dispatch phase-09b`.

**What it does.** `edit_file` gains `sudo?: boolean`. With `sudo: true`, an
`operation: "edit"` on a named host runs as root. The line typed into the
host's shell starts sudo (`… set -- python3 …; sudo "$@"`), and sudo's
password prompt is answered through the chat's `CredentialPrompt`. The
file's mode and owner are kept (09a). A sudo edit is refused off-host and
for any other operation, is never auto-approved or session-approved, and
reports an `Error:` when it did not run.

**Measured at drafting.** The `set --` line is detected as sudo. Through the
daemon's run path, a fake privilege tool gave exactly one prompt and the edit
landed with its `0640` mode kept. With nobody attached, the run gave up
cleanly and left the file unedited, both locally (305 ms) and on pinky's real
sudo-rs (323 ms). A refused password leaves no sentinel line, so the result
needs an explicit error.

**Shell-gate reminder.** The executor cannot type the privilege tool's name.
Every test is named `…root_edit…` / `…root_flag…`, and the criteria spell it
`su[d]o`.

**After 09b:** `/rexymcp:architect next` drafts 09c, the live close: a sudo
edit from chat on pinky, with the password absent from every cast-log `"i"`
event. It is architect-run.
