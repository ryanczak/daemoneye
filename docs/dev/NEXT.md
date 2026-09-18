# NEXT

**Active phase:** none — **M21 (Host model & SSH) is scoped and awaiting PE
sign-off on the breakdown.** No phase is drafted until that sign-off; then
`/rexymcp:architect next` drafts 01a.

Scoping README: `docs/dev/milestones/M21-host-model/README.md` — nine rows
(01a, 01b, 02–09), ordered credential mechanism → config → argv spawn → SSH
shell → prompts → approval classes → remote file ops → ghost mapping → live
close. Each row is tied to a fact measured from the tree on 2026-09-18, listed
under "Measured before scoping".

**Two things the PE decides at sign-off:**

1. **The live-verification host.** Rows 04, 05 and 09 need a reachable,
   configured SSH host; the daemon host's `~/.ssh/config` has one stanza.
   Name it (or a throwaway) before row 04 is drafted.
2. **`host?` on `spawn_ghost_shell`** — in M21 as its own row after 08, or
   left for M23. The README lists it under "Not in this milestone" pending
   this call.

**The finding worth reading before signing off:** the PTY path has **no
credential-prompt handling** — `shell_exec.rs`, `bg_shell.rs` and
`shell/client.rs` contain no sudo, password or `CredentialPrompt` code, while
the tmux path has a complete sentinel mechanism. The design assumes remote
prompts surface "the same way sudo prompts are today"; on the shipped default
backend they do not surface at all. Read from code, not yet run: 01a's drafting
probes it live first. If confirmed, a chat `sudo` on the default backend hangs
to its 45 s timeout **today**, independent of M21 — which is why 01a is row one.

**Closed this session:** M22 (2026-09-18, PE signed off, one calibration
trigger folded) and M20 (2026-09-18, PE signed off; its record had sat at
`in-progress` for nine days after completion).

**Upstream push backlog:** the `WORKFLOW.md` § Acceptance criteria pre-dispatch
check is local-only — rexyMCP's `plugin/templates/WORKFLOW.md` lacks it.
