# NEXT

**Active phase:** **M21 phase-09a — edit-keeps-file-mode**, drafted
2026-09-29, status `todo`. Dispatch with `/rexymcp:dispatch phase-09a`.

**What it does.** `edit_file`'s `edit` writes a `.de_tmp` file and renames it
over the original, so the file takes the writer's umask instead of its own
mode. After this phase an edit keeps the file's permission bits on the local
path and on both remote interpreter branches (Python, Perl), and keeps its
owner and group wherever the writer is allowed to set them.

**Measured at drafting.** `0600` → `0644` on the local path and on both
remote branches run locally; the real built command on pinky (the daemon's
PTY shell has umask `0002`) turned `600:matt` into **`664:matt`**, so the
file became group-writable. The fix (stat → write → chmod → best-effort chown
→ rename) was run in both interpreters and on pinky, and kept the mode.

**Why it comes before the sudo edit.** Row 09 split three ways at drafting.
`edit_file` has no sudo path, so the milestone's exit ("a remote `sudo` edit
via `edit_file { host }`") could not be met. The PE chose to add one (09b)
before the live close (09c). A sudo edit through today's rename would also
reset the file's owner to root, so 09a goes first.

**After 09a:** `/rexymcp:architect next` drafts 09b (remote-sudo-edit). The
facts it needs, measured at 09 drafting, are in the milestone README's Notes
under "Measured at 09 drafting".
