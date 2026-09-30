# NEXT

**Active phase:** **M21 phase-09c — live-close**, drafted 2026-09-30, status
`todo`. **Architect-run, not dispatched**: tell the architect to run it with
the PE at the keyboard (pinky's sudo password is typed twice: once to create
the root-owned fixture over `ssh -t`, once in the chat's credential prompt),
plus once more for the S5 cleanup.

**What it does.** The milestone's exit criterion for real: from a
`daemoneye chat` turn, `edit_file { host: "pinky", sudo: true }` edits a
root-owned file on pinky. Evidence is read where it lives — the shell's
cast log (the marker-delimited sudo edit, `DE_OK: Edited`, and `[redacted]`
`"i"` events), the session JSONL (the call with `sudo: true` and its
result), the file on pinky (`root:640`, new content), and `ssh -O check`
timing the reused ControlMaster. The PE types the password into a shell
variable for a grep that must find it in no log.

**Measured at drafting.** The installed `~/.cargo/bin/daemoneye` is from
Sep 18 and the daemon is not running; the live config has no
`[hosts.pinky]`. S0 adds the entry (kept) and S1 rebuilds, reinstalls and
restarts with M14's sha256 identity check. pinky's sudo-rs tickets are
per-tty, so the fixture `sudo` over `ssh -t` does not warm the daemon's
shell: the chat run will prompt, and `cast-redacted-i ≥ 1` proves it did.

**After 09c:** all M21 rows are `done`. `/rexymcp:architect` closes the
milestone (retrospective, folds, `NEXT.md` → none), gated on the PE.
