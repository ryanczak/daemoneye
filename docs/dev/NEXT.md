# NEXT

**Active phase:** `docs/dev/milestones/M21-host-model/phase-03-shell-spawn-argv.md`
— 03, shell-spawn-argv. **Drafted 2026-09-19**, status `todo`.

Threads argv through the whole spawn chain — the `shell-host` CLI, `run_host`,
`PtyBackend::spawn`, `PtyShell::spawn`, and out to `CommandBuilder` — and gives
`ShellRecord` a `host: HostId` across its nine construction sites. **Nothing
connects to anything:** no SSH, no ControlMaster. This is the carrier row 04
needs, not the payload.

**Local behaviour must stay byte-identical**, and that is the phase's sharpest
negative: the host renders only when it is *not* local, so a user with no hosts
configured sees exactly today's output. One mutation is built on that rule.

**The clap shape was probed, not assumed.** The real uncertainty was whether
clap's derive API can carry hyphenated argv across a process boundary. Probed
against this repo's clap 4.5 with a genuine ssh command line: the flags
round-trip intact, and `shell-host --id s0` with no `--` still parses to an
empty vec. The exact attribute shape is in the doc.

**One thing stated honestly rather than claimed.** Both mutations target code
this phase creates, so neither could be applied at drafting. The doc says so
plainly and tells the executor to run each and report what it observes,
including "stayed green" as a finding. That is the fourth-occurrence lesson
applied to my own drafting: after three phases damaged by mutations pinned
without being run, the fix is not to assert harder but to say which claims are
observations and which are predictions.

Every acceptance criterion was run against HEAD `f82402c7` and fails there.
Baselines measured and corrected during drafting: `shell::` 263, the knowledge
shell module 67, lib 1908 → 1915.

Dispatch with `/rexymcp:dispatch phase-03`.

**After 03:** 04 ssh-shell, which finally connects. Ten rows in M21; live host
`pinky`; `max_turns` 1000.
