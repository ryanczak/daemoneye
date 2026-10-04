# NEXT

**Active phase:** **M23 phase-09a — user-shell-open**, drafted 2026-10-04,
status `todo`. Dispatch with `/rexymcp:dispatch phase-09a`.

**What it does.** `/shell` opens a new shell owned by the user on the daemon
host, or on a configured host with `/shell pinky`, and attaches to it. This
is the pre-authenticate use case. It adds a new `Owner::User`, a
`ShellOpen` → `ShellOpened` request, and the model's `open_shell` path split
into an id-returning core with the model's text unchanged. 9 files, 2 new
tests; lib 2212 → 2214.

**Row 09 was split at drafting:** 09a (this) opens a user shell, and 09b
lets agents see user shells and drive them only through approval.

**Measured at drafting.** The 9 existing `open_shell` tests guard the
refactor, and the end-to-end block runs them. One named-host mutant needs
real ssh and was dropped. The known flake hit 3 of 10 runs. The block was
run on the dry-run tree. Mutants A–D went red 3/3, all clippy-clean. The doc
reproduces the dry run.

**After 09a:** `/rexymcp:architect next` drafts 09b (user-shell-agents).
