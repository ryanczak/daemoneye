# NEXT

**Active phase:** **M23 phase-01a — pty-spawn-transient**, drafted
2026-09-30, status `todo`. Dispatch with `/rexymcp:dispatch phase-01a`.

**What it does.** A PTY shell starts in `$HOME` if it exists and in `/`
otherwise, so it never fails to spawn because `HOME` is gone. The six
connect tests in `shell_exec.rs` take `test_home_guard()` so no
HOME-mutating test can delete `HOME` under a spawn. The test-only retry M21
08e added is deleted, along with its wrong `TIOCSCTTY` diagnosis. Two new
tests: 2090 / 14.

**Re-diagnosed at drafting.** The exact 08e error
(`spawn_command failed: No such file or directory (os error 2)`) reproduces
deterministically by pointing `HOME` at a missing directory: portable-pty
chdirs the child into `$HOME` when no cwd is set. The fix alone still failed
the suite 2 runs in 3 (a check-then-use race with tests deleting `HOME`);
with the guard added it was 5 of 5 green.

**After 01a:** `/rexymcp:architect next` drafts 01b
(prompt-wait-outside-deadline).
