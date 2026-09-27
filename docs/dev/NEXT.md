# NEXT

**Active phase:** **M21 phase-08a — ghost-target-resolution**, drafted
2026-09-27, status `todo`. Dispatch with `/rexymcp:dispatch phase-08a`.

**What it does.** At ghost start, a runbook's `ssh_target` is resolved
against `[hosts]` — by key, else by a unique match on a host's `ssh`
destination — and recorded as `ghost_ssh_target_resolved`. On today's
transport, two rules become real: `HostEntry.sudo_scripts` is **enforced**
(empty = unrestricted), and a non-empty runbook `hosts:` must grant the
resolved host. **The transport does not change** — a ghost command still runs
as `ssh <target> '<cmd>'` in a tmux window; 08b moves it onto a PTY shell.

**Row 08 split at drafting, on four PE decisions (2026-09-27).** Measured
first:
- Half the old row was already done (`sudo_scripts` / `sandbox_profile` have
  parsed since 02).
- `ssh_target`'s documented `user@host` cannot parse as a `HostId`.
- Ghosts never touch the PTY substrate — every ghost command is a tmux
  `Background` with `host` dropped on purpose.

Decisions: resolve, else keep 1.x; split resolution (08a) from the PTY move
(08b); enforce `sudo_scripts`; a remote ghost command is allowed under the
sandbox, with `hosts:` as the fence. Full record in the M21 README Notes.

**Probed before speccing.** The door test rides
`webhook_ghost_e2e_deterministic`: a copy writing `[hosts.e2e-host]` and an
`ssh_target: e2e-host` runbook passed in 0.09 s and loaded the host, with no
resolution event — exactly the gap 08a fills.

**Pre-dispatch check run.** Every grep criterion matches its stated current
value (resolver 0, predicates 0, gates 0, start call 0, stale code claim 1,
template claim 2 → 1, README row 0), both pinned-test filters run 0 passed,
and no command block contains the shell-gated `su`+`do` token. **Six
mutations**, one per gate and per rule — the 07b lesson that a test named for
a branch is not proof it exercises the branch.

**Next after 08a: 08b — ghost-remote-pty**, then 08c (spawn-ghost-host), 09.

Twenty rows in M21; live host `pinky` (sudo requires a password since
2026-09-22); `max_turns` 1000.
