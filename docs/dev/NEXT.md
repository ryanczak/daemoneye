# NEXT

**Active phase:** `docs/dev/milestones/M21-host-model/phase-01a-credential-prompt-on-pty.md`
— 01a, credential-prompt-on-pty. **Drafted 2026-09-18**, status `todo`.

**The defect is confirmed, not suspected.** The drafting probe drove a real
`daemoneye shell-host` over its Unix socket in an isolated `HOME`:

- `sudo -k; sudo true` → emits `[sudo] password for matt: `, **no end marker
  ever arrives**. The command runs to its 45 s deadline and the user is never
  asked for a password. That is the shipped default backend today.
- `sudo -k; sudo -S true </dev/null` → completes in 1.8 s with
  `sudo: a password is required`. The machinery is sound; what is missing is
  noticing the prompt and answering it.
- Second finding, recorded for 01b and row 04: a shell parked at a prompt
  **eats the next command's bytes**. 01a's give-up path must un-park the shell.

01a adds a pure prompt classifier that matches the **sentinel**, never sudo's
own locale-dependent text; a streaming variant of `run_command` that can answer
mid-command; and the wiring that turns a detected prompt into
`Response::CredentialPrompt`. Local sudo only — no SSH, no hosts, no config.

Every acceptance criterion was run against HEAD `bf9dd263` and fails there.
The three module test baselines are measured: 11 / 17 / 19.

Dispatch with `/rexymcp:dispatch phase-01a`.

**Next rows:** 01b (scrub the credential out of the cast log, in the shell-host
process), then 02 host-config. Ten rows total; live host is `pinky`.
