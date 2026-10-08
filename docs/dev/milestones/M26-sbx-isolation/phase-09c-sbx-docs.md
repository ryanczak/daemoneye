# Phase 09c: sbx-docs — the docs describe Docker Sandboxes, not docker containers

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** todo
**Depends on:** phase-09b
**Estimated diff:** ~250 lines, docs and assets only
**Tags:** language=markdown, kind=docs, size=s

## Goal

This is the exit criterion "Docs current". Every doc that describes the
sandbox now describes sbx:
- `README.md`: the sandbox section, the CLI table, and the ghost prose;
- `CLAUDE.md`: § Container sandbox and the Key files table;
- `docs/security.md`, which today says nothing about the sandbox;
- the seeded `ghost-shell-guide` memory, which still describes `de-gs-*`
  windows as where ghost commands run;
- the seeded `assets/etc/config.toml` `[sandbox]` block;
- `docs/architecture.md`.

The docker runtime's code is gone (phase 09b). **No Rust source changes in
this phase.**

## Architecture references

- `docs/dev/milestones/M26-sbx-isolation/README.md` § Exit criteria, "Docs
  current", and § Decisions on the open questions, Q1–Q6.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any file.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

The facts below are what the code does today. They are the **only** facts
the new text may state. Do not add capabilities.

- **What `enabled` means.** `[sandbox] enabled = true` always means sbx.
  `runtime` is ignored, and any value other than `"sbx"` logs a warning.
- **Fails closed.** Every sandboxed command first runs the preflight
  (`sbx version`, then `sbx daemon status`). If it fails, the command is
  refused with a reason starting `sandbox unavailable:` and never runs on
  the host.
- **Ghost runs.** A local ghost gets **its own** sandbox, `de-ghost-<id>`,
  created on its first command:
  - every command it runs lands in a PTY shell whose child is
    `sbx exec -it <sandbox> …`;
  - the sandbox's profile is the ghost's agent name
    (`[sandbox.profile.<agent>]`), and it is mountless;
  - it is torn down when the ghost exits, on every path.
- **Chat `wait: false`.** A chat session's `run_terminal_command(wait:
  false)` runs in a dedicated shell inside the session's sandbox,
  `de-chat-<id>`, created on first use with the default profile. Then:
  - the shell is closed once the completion message lands;
  - the hourly reaper removes the sandbox once no shell uses it and it has
    been idle 600 s.
- **Scripts.** An approved script a sandboxed command invokes is copied
  into the sandbox (`sbx cp` to `/de/scripts/<name>`) **on every
  invocation**, and the command runs that copy. The scripts directory is
  never mounted. A command under `sudo` is not staged.
- **Network.** A profile's `network = "none"` (the default) denies all
  egress (`--deny-network '**'`). `network = "proxy"` inherits the
  operator's global sbx network policy: `proxy_allow` **adds** hosts and
  does not restrict, `proxy_deny` denies, and a deny beats an allow.
- **Egress audit.** Before teardown, the daemon reads the sandbox's
  `sbx policy log --json`:
  - a ghost run gets a turn-log note
    `sandbox egress: allowed …; blocked …` and a `sandbox_egress` event;
  - a reaped chat sandbox gets the event.
- **Pause, cancel, close.** `Ctrl+P` (attached) stops and continues the
  job **inside the VM**. Cancel sends `^C`, and closing a shell hangs it up
  inside the VM.
- **Restarts and sweeps.** After a daemon restart, a sandbox shell is
  adopted with its sandbox name. The startup sweep removes only `de-*`
  sandboxes that no live shell record owns.
- **Status.** `daemoneye status` has a SANDBOX section, and
  `daemoneye sandbox status` prints just that section. Both show the sbx
  version (or why sbx is unusable) and the daemon's own sandboxes.
- **Not sandboxed:**
  - foreground commands, which run in your own pane;
  - commands on a configured `[hosts.<name>]` host;
  - the daemon's own tools (`read_file`, `edit_file`, memory, …).
- **Refused while enabled:** a command that would run in a tmux window,
  which means `[execution] backend = "tmux"` or a ghost `ssh_target` that
  names no configured host.
- **Docker-only keys.** `image`, `proxy_image`, `workdir`, `run_as`,
  `docker_host`, `limits.pids`, `limits.scratch` and
  `ghost_defaults.mount_scripts` still parse, but warn that they have no
  effect. `limits.memory` and `limits.cpus` apply.
- **No credentials in the sandbox.** sbx secret injection is not configured,
  so a sandbox holds no API key or credential.

## Spec

### Task 1 — `CLAUDE.md`

1. **Key files table:** replace the `src/daemon/executor/container.rs` row
   with these five rows:

   ```markdown
   | `src/daemon/executor/sbx.rs` | Docker Sandboxes (sbx), pure: sandbox naming (`de-ghost-*` / `de-chat-*`), `network_plan`, every `sbx` argv builder, the `ls --json` / `policy log --json` parsers, preflight classification, and the in-VM signal script (`VM_SIGNAL_SCRIPT`, `VmOp`) |
   | `src/daemon/executor/sbx_lifecycle.rs` | sbx side effects through an injected `SbxRunner`: `preflight`, `create_sandbox`, `teardown_sandbox` (rm, falling back to stop), `sweep_daemon_sandboxes`, `collect_sbx_status`, `read_egress` |
   | `src/daemon/executor/ghost_sandbox.rs` | A ghost run's sandbox: `ensure_sandbox` (fail-closed, created once per run), `run_ghost_in_sandbox`, `stage_script`, `forget_and_teardown`, `is_created` |
   | `src/daemon/executor/chat_sandbox.rs` | A chat session's sandbox for `wait: false`: `ensure_chat_sandbox`, the idle reaper (`reap_idle_chat_sandboxes`, 600 s grace) |
   | `src/shell/sbx_backend.rs` | `SbxBackend`: a shell host whose child is `sbx exec`, routing STOP/CONT/close into the VM and sending INT/TERM as a PTY `^C` |
   ```

2. **Replace the whole `## Container sandbox` section**, from its heading up
   to the next `## ` heading, with:

   ```markdown
   ## Sandbox (Docker Sandboxes / sbx)

   `[sandbox] enabled = true` (default off) runs agent work in **Docker
   Sandboxes** microVMs via the `sbx` CLI. `runtime` is ignored — an enabled
   sandbox is always sbx — and docker-only keys warn at load.

   - **Fails closed.** Preflight (`sbx version`, `sbx daemon status`) gates
     every sandboxed command; a failure refuses it with a `sandbox
     unavailable:` reason and it never runs on the host.
   - **Ghost runs** get their own sandbox `de-ghost-<id>` (profile = the
     ghost's agent, mountless), created on the first command, torn down on
     every exit path. Commands run in a PTY shell whose child is `sbx exec`.
   - **Chat `wait: false`** runs in a per-session sandbox `de-chat-<id>`; the
     dedicated shell closes after its completion message, and the hourly
     reaper removes a sandbox no shell uses once it has idled 600 s.
   - **Scripts** are staged with `sbx cp` into `/de/scripts/` on every
     invocation; the scripts directory is never mounted.
   - **Network:** `network = "none"` → `--deny-network '**'`; `"proxy"`
     inherits the global sbx policy, `proxy_allow` adds, `proxy_deny` denies.
   - **Egress audit:** `sbx policy log --json` is read before teardown — a
     turn-log note plus a `sandbox_egress` event.
   - **Signals:** pause/resume act inside the VM; cancel is a PTY `^C`.
   - **Startup** adopts sandbox shells (sidecar) and sweeps only `de-*`
     sandboxes no live record owns.
   - **Not sandboxed:** foreground commands, `[hosts.*]` commands, and
     broker-native tools. **Refused while enabled:** anything that would run
     in a tmux window (`backend = "tmux"`, an unresolved ghost `ssh_target`).
   ```

### Task 2 — `README.md`

1. **The sandbox section.** Replace from the heading `### 🔒
   Container-Sandboxed Agents (opt-in)` through the line `Design of record:
   …agent-container-sandboxing.md…` (inclusive) with:

   ```markdown
   ### 🔒 Sandboxed Agents — Docker Sandboxes (opt-in)

   > With `[sandbox] enabled = true`, agent work runs inside **Docker
   > Sandboxes** (`sbx`) microVMs instead of on your host. The sandbox stays
   > **off by default**, so `sbx` is not needed to build, test or run
   > DaemonEye. Check it with `daemoneye sandbox status`.

   Every approval prompt above is a *behavioural* control: it depends on the
   operator reading the command correctly. None of it stops a
   degenerate-but-approved command, a script that writes outside its lane, or
   a prompt injection that yields a plausible-looking command. Without the
   sandbox a compromised agent has exactly the daemon's privileges.

   With it:

   - **Each ghost run gets its own microVM** (`de-ghost-<id>`), created on
     its first command and destroyed when the ghost exits — on every path,
     including failure and cancel. Its profile is the ghost's agent name.
   - **Chat background commands** (`wait: false`) run in your session's own
     sandbox (`de-chat-<id>`), removed once it has sat unused for ten minutes.
   - **No network by default.** A profile's `network = "none"` denies all
     egress. `network = "proxy"` uses your global `sbx` network policy;
     `proxy_allow` adds hosts to it and `proxy_deny` blocks them.
   - **Every request is audited.** Before a sandbox is removed, its `sbx`
     policy log — the hosts it reached and the hosts that were blocked — is
     written to the run's log and to `events.jsonl`.
   - **Only the script you approved goes in.** An approved script is copied
     into the sandbox each time it runs; the script library, your config,
     memory and `.ssh` are never mounted.
   - **No credentials in the sandbox.** The daemon talks to the model
     provider; the sandbox holds no API key.
   - **Fails closed.** If `sbx` is missing or its daemon is not running, a
     sandboxed command is refused with the reason — it never falls back to
     your host.
   - **Pause means pause.** `Ctrl+P` on an attached ghost stops the job
     inside the VM, not just the client.

   Foreground commands — the ones injected into your own pane — stay on the
   host by design: they are visible, approved, and run in your shell.
   Commands on a configured `[hosts.<name>]` host run there, not in a sandbox.
   While the sandbox is enabled, `[execution] backend = "tmux"` is refused,
   because a tmux window is outside it.

   Design of record: [`docs/design/daemoneye-2.0.md`](docs/design/daemoneye-2.0.md) § 2.9.
   ```

2. **CLI table.** Replace the `daemoneye sandbox build` row with:
   `| \`daemoneye sandbox status\` | Show the sandbox runtime (the \`sbx\` version, or why it is unusable) and the daemon's own sandboxes |`

3. **Ghost prose.** In the Ghost Shells paragraph, replace the sentence
   beginning `Three cases still use tmux windows on the daemon host` up to
   and including `(wrapped as \`ssh <target> <cmd>\`).` with:

   `With the sandbox enabled, a ghost's local commands run in its own Docker Sandboxes microVM instead (see the sandbox section above). Two cases still use tmux windows on the daemon host, prefixed \`de-gs-bg-*\` (webhook- and interactively-triggered) or \`de-gs-sj-*\` (scheduler-triggered): \`[execution] backend = "tmux"\`, and an \`ssh_target\` that names no configured host (wrapped as \`ssh <target> <cmd>\`) — and both are refused while the sandbox is enabled.`

4. **The two "With the container sandbox or `backend = "tmux"`" phrases.**
   Replace `With the container sandbox or \`backend = "tmux"\`` with
   `With \`backend = "tmux"\``, and leave the rest of each sentence alone.
   - The **first** wraps across lines: line ~1336 ends `… read-only. With the`
     and line ~1337 starts `container sandbox or \`backend = "tmux"\`, the
     commands run in tmux windows`. A one-line search will not find it.
     Rewrite both lines so the sentence reads `… read-only. With
     \`backend = "tmux"\`, the commands run in tmux windows`.
   - The **second** is on one line, about line 1376.
   - Afterwards, `grep -c 'container sandbox' README.md` must print `0`.

### Task 3 — `docs/security.md`

Insert this section **immediately before** `## What is deliberately NOT a
boundary`:

```markdown
## The agent sandbox (opt-in)

With `[sandbox] enabled = true`, agent work runs in Docker Sandboxes (`sbx`)
microVMs: a ghost run in its own sandbox, a chat session's `wait: false`
commands in the session's sandbox. Inside the VM the agent has full control;
the VM boundary, not anything inside it, is the guard. It holds:

- **No host filesystem.** Sandboxes are mountless. The one approved script a
  command invokes is copied in per invocation (`sbx cp`); config, memory,
  `.ssh`, the scripts directory and the daemon's sockets are never mounted.
- **No credentials.** sbx secret injection is not configured; the daemon
  keeps the model-provider key.
- **Egress is policy.** `network = "none"` (the default) denies all egress;
  `network = "proxy"` inherits the operator's global sbx policy plus
  `proxy_allow`, minus `proxy_deny`. Each sandbox's allowed and blocked hosts
  are recorded before teardown (`sandbox_egress` in `events.jsonl`).
- **Fails closed.** A failed preflight (`sbx` missing, daemon not running)
  refuses the command; there is no host fallback. So does any command that
  would run in a tmux window while the sandbox is enabled.

Not covered: foreground commands in your own pane, commands on configured
`[hosts.*]`, and the daemon's own tools — those run with the daemon's
privileges, under the guards above.
```

Also change the Trust model sentence `they do **not** turn it into a
sandbox.` to `they do **not** turn it into a sandbox — that is the opt-in
agent sandbox's job (below).`

### Task 4 — The seeded `ghost-shell-guide` (`assets/memory/knowledge/ghost-shell-guide.md`)

1. Replace the whole `## tmux Window Naming` section, up to the next `## `,
   with:

   ```markdown
   ## Where Ghost Commands Run

   A ghost's commands run in a **PTY shell the ghost owns**, reused for its
   later commands and closed when the ghost exits:

   - **Sandbox enabled** (`[sandbox] enabled = true`): the shell runs inside
     the ghost's own Docker Sandboxes microVM, `de-ghost-<id>`, created on
     the first command and destroyed on exit. Approved scripts are copied in
     per invocation (`/de/scripts/<name>`); the network follows the profile
     named after the ghost's agent (default: no egress). A refused command's
     result starts `sandbox unavailable:` — do not retry it on the host.
   - **Sandbox off:** on the daemon host, or on the configured `[hosts]`
     entry the runbook's `ssh_target` names.
   - **`backend = "tmux"`** (opt-out) and an `ssh_target` naming no
     configured host use tmux windows `de-gs-bg-*` (webhook/interactive) and
     `de-gs-sj-*` (scheduled) — both refused while the sandbox is enabled.

   `list_shells` / `read_shell` show a ghost's shells (owner `ghost:…`).
   ```

2. In `## Troubleshooting Ghost Shells`, add to the `events-<date>.jsonl`
   list:
   - `` - `sandbox_egress` — a sandbox's allowed and blocked hosts, read before teardown: `sandbox`, `allowed`, `blocked` ``;
   - `` - `ghost_sandbox_teardown` — the ghost sandbox's removal outcome: `session`, `outcome` ``.

3. Replace the `**\`~/.daemoneye/var/log/panes/<win_name>.log\`**` bullet
   with: `` **Shell recordings** — every command's output, in the shell's
   cast log: `list_shell_logs` / `read_shell_log` (tmux-backend ghosts also
   keep `~/.daemoneye/var/log/panes/<win_name>.log`). ``

### Task 5 — The seeded config template (`assets/etc/config.toml`)

Replace the `[sandbox]` comment block, from `# [sandbox]` through
`# mount_scripts   = "ro"   # mount mode for the staged script volume`,
with the block below. **Every key stays**:
`tests/doc_truth.rs::seeded_config_template_documents_every_config_field`
requires each `SandboxConfig` field to appear, and
`seeded_config_template_has_no_phantom_keys` forbids extras.

```toml
# [sandbox]
# Run agent work in Docker Sandboxes (`sbx`) microVMs. Off by default;
# needs `sbx` installed and its daemon running (`daemoneye sandbox status`).
#
# enabled     = false                    # master feature flag
# runtime     = "sbx"                    # ignored: an enabled sandbox is always sbx
# The keys below applied only to the removed docker runtime. They still parse,
# and warn at load if set.
# image       = "daemoneye-agent-base"
# proxy_image = "daemoneye-egress-proxy"
# workdir     = "/de/work"
# run_as      = "1000:1000"
# docker_host = "unix:///run/user/1000/docker.sock"
#
# [sandbox.limits]
# memory  = "1g"   # per-sandbox memory
# pids    = 256    # docker-only: no effect
# cpus    = 2.0    # per-sandbox CPUs
# scratch = "2g"   # docker-only: no effect
#
# [sandbox.profile]
# Per-profile policy, keyed by agent name (a ghost's sandbox uses its agent's
# profile; chat uses the default: network "none").
# [sandbox.profile.researcher]
# network      = "proxy"    # "none" (default: no egress) or "proxy" (the global
#                           # sbx network policy, plus proxy_allow, minus proxy_deny)
# proxy_allow  = ["crates.io", "*.crates.io"]  # hosts added to the policy
# proxy_deny   = []         # hosts this profile may never reach; deny beats allow
#
# [sandbox.ghost_defaults]
# destroy_on_exit = true   # destroy the ghost's sandbox on exit, every path
# mount_scripts   = "ro"   # docker-only: no effect
```

Before you replace it, check the existing block's key lines and keep every
key you find there, even if this block omits one. If a doc_truth test fails,
the template lost or gained a key. Fix the template, never the test.

### Task 6 — `docs/architecture.md`

After the paragraph that ends `describe the \`tmux\` backend and remain
accurate.` (in `### DaemonEye 2.0 — M20 Shell Engine`), add:

```markdown
**Agent sandbox (M26).** The M18/M19 docker container sandbox is replaced by
Docker Sandboxes (`sbx`): with `[sandbox] enabled`, a ghost run gets its own
microVM and a chat session's `wait: false` commands run in the session's
sandbox, each reached through a shell host whose child is `sbx exec`. The
decisions are in `docs/design/daemoneye-2.0.md` § 2.9 and
`docs/dev/milestones/M26-sbx-isolation/README.md`.
```

### Task 7 — Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry.

## Acceptance criteria

- [ ] `grep -n -i -E 'docker --host|sandbox build|sandbox\.lock|rootless docker|container\.rs|Container-Sandboxed' README.md CLAUDE.md docs/security.md docs/architecture.md assets/memory/knowledge/ghost-shell-guide.md assets/etc/config.toml`
      prints nothing.
- [ ] `grep -c 'container sandbox' README.md` prints `0`. Today it prints `3`.
- [ ] `grep -c 'Docker Sandboxes' docs/security.md` prints at least `1`.
      Today it prints `0`.
- [ ] `grep -c '^## Where Ghost Commands Run' assets/memory/knowledge/ghost-shell-guide.md`
      prints `1`, and `grep -c '^## tmux Window Naming' …` prints `0`.
- [ ] `grep -c '^## Sandbox (Docker Sandboxes / sbx)' CLAUDE.md` prints `1`.
- [ ] `git diff --stat e7005955 -- src/` prints nothing: no Rust source
      changes.
- [ ] `cargo test --test doc_truth` reports `0 failed`.
- [ ] `cargo test` reports exactly `passed=2297 failed=0`, unchanged.
- [ ] The Update Log entry contains the § End-to-end verification output,
      every line as printed, and a line `PASTE MATCH: yes`.

## Test plan

No new tests. `tests/doc_truth.rs` is the gate, especially the config-template
key checks. The suite count must not move.

## End-to-end verification

```sh
grep -n -i -E 'docker --host|sandbox build|sandbox\.lock|rootless docker|container\.rs|Container-Sandboxed' README.md CLAUDE.md docs/security.md docs/architecture.md assets/memory/knowledge/ghost-shell-guide.md assets/etc/config.toml ; echo "stale_grep_done"
grep -c 'container sandbox' README.md
grep -c 'Docker Sandboxes' docs/security.md
grep -c '^## Where Ghost Commands Run' assets/memory/knowledge/ghost-shell-guide.md
grep -c '^## tmux Window Naming' assets/memory/knowledge/ghost-shell-guide.md
grep -c '^## Sandbox (Docker Sandboxes / sbx)' CLAUDE.md
git diff --stat e7005955 -- src/ ; echo "src_diff_done"
cargo test --test doc_truth 2>&1 | grep -E '^test result'
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff --stat
```

Each line runs **once**. Expect **only** `test result:` summary lines from
the `cargo test` lines. rexyMCP's output filter removes the individual
`test … ok` lines, so do not look for them and do not re-run to find them.

## Authorizations

- [x] May edit `README.md`, `CLAUDE.md`, `docs/security.md`,
      `docs/architecture.md`, `assets/memory/knowledge/ghost-shell-guide.md`
      and `assets/etc/config.toml`, **only** as Tasks 1–6 say.

## Out of scope

- Any Rust source change.
- `docs/design/agent-container-sandboxing.md`, the M18 design kept as
  history, and the M18/M19 milestone docs.
- `~/.daemoneye` (the installed copies of the seeded assets). Seeding is
  first-run; refreshing an installed memory is the operator's call, at the
  row-10 live close.

## Update Log

<!-- entries appended below this line -->
