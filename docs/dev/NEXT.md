# NEXT

**Active phase:** none drafted yet — **M21 (Host model & SSH) is in progress,
PE signed off on the breakdown 2026-09-18.** Run `/rexymcp:architect next` to
draft **01a — credential-prompt-on-pty**.

Scoping README: `docs/dev/milestones/M21-host-model/README.md` — ten rows
(01a, 01b, 02–08, 08b, 09). Both sign-off decisions are recorded there:

1. **Live-verification host: `pinky`** (`pinky.home.planetfoo.org:22`, user
   `matt`, `controlmaster false` in the user's config — so ControlMaster
   behaviour is measured purely from the daemon's own flags). Rows 04, 05,
   08b, 09 target it.
2. **`host?` on `spawn_ghost_shell` is in M21** as row 08b, after ghost-host
   mapping and gated by the runbook `hosts:` enforcement row 06 lands.

**01a drafts against a live probe first.** The scoping finding: the PTY path
has no credential-prompt handling — `shell_exec.rs`, `bg_shell.rs`,
`shell/client.rs` carry no sudo/password/`CredentialPrompt` code, while the
tmux path has a complete sentinel mechanism. If a chat `sudo` on the default
backend hangs to its 45 s timeout today, 01a is fixing a shipped defect, not
only laying groundwork; the drafting probe settles which before the spec is
written.

**Closed this session:** M22 (2026-09-18, PE signed off, one calibration
trigger folded) and M20 (2026-09-18, PE signed off).

**Upstream push backlog:** the `WORKFLOW.md` § Acceptance criteria pre-dispatch
check is local-only — rexyMCP's `plugin/templates/WORKFLOW.md` lacks it.
