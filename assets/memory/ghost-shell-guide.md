# Ghost Shell Guide

Ghost Shells are autonomous AI sessions that run in the background without a human present. Use them to delegate investigations and remediations that would otherwise block an interactive session.

## When to Use a Ghost Shell

- Long-running investigations (log triage, multi-step diagnostics)
- Automated remediation following a webhook alert
- Recurring maintenance via a scheduled job
- Any task where you want the AI to work in the background while you continue talking to the user

## Triggering a Ghost Shell

**From an interactive session** — use `spawn_ghost_shell`:
```
spawn_ghost_shell(
  runbook="high-disk-usage",
  message="Disk usage is at 94% on /dev/sda1. Investigate and remediate per runbook."
)
```
Returns the ghost session ID. The ghost injects `[Ghost Shell Started]` / `[Ghost Shell Completed]` / `[Ghost Shell Failed]` lifecycle events into active sessions.

**From a scheduled job** — use `schedule_command` with `ghost_runbook`:
```
schedule_command(
  name="nightly-cert-check",
  ghost_runbook="cert-expiry-check",
  cron="0 6 * * *"
)
```

**From a webhook** — configure the runbook with `enabled: true` in `ghost_config` frontmatter. When the watchdog analysis emits `GHOST_TRIGGER: YES`, DaemonEye spawns the ghost automatically.

## Runbook Frontmatter for Ghost Shells

```yaml
---
tags: [disk, ops]
memories: [disk-thresholds]
enabled: true
auto_approve_scripts: [check-disk.sh, cleanup-logs.sh]
auto_approve_read_only: true
run_with_sudo: false
max_ghost_turns: 20
ssh_target: ""
---
```

| Field | Description |
|---|---|
| `enabled` | Must be `true` for webhook-triggered ghosts |
| `auto_approve_scripts` | Script names in `~/.daemoneye/scripts/` the ghost may run without approval |
| `auto_approve_read_only` | Auto-approve safe informational commands (`df`, `ps`, `ls`, `cat`, `journalctl`, etc.) |
| `run_with_sudo` | Prepend `sudo` to approved scripts. Requires a NOPASSWD sudoers rule — see `scripts-and-sudoers` memory |
| `max_ghost_turns` | Hard turn limit (0 = use daemon default of 20) |
| `ssh_target` | If set (e.g. `user@host`), all approved scripts are wrapped in `ssh <target> <cmd>` |

## Ghost Policy — What Gets Approved

The ghost operates under `GhostPolicy` derived from the runbook frontmatter:
- **Auto-approved**: scripts listed in `auto_approve_scripts`; read-only commands if `auto_approve_read_only: true`
- **Auto-denied**: any command not in the above categories; sudo prompts not covered by NOPASSWD

The ghost AI is instructed to only use pre-approved scripts for mutating actions. If it needs something not on the whitelist it should document the gap and stop rather than attempt workarounds.

## tmux Window Naming

Ghost shells create windows with `de-gs-*` prefixes, distinct from interactive background windows (`de-bg-*`):

| Prefix | When created |
|---|---|
| `de-gs-bg-*` | Background command within a webhook- or interactively-triggered ghost |
| `de-gs-sj-*` | Background command within a scheduler-triggered ghost (`ActionOn::Ghost`) |
| `de-gs-ir-*` | Reserved for incident-response main session windows |

When you see `[BACKGROUND PANE de-gs-bg-*]` or `[BACKGROUND PANE de-gs-sj-*]` in terminal context, it is a ghost shell command window. The `[ghost]` annotation is always present.

## Concurrency Cap

The daemon limits concurrent ghosts via `max_concurrent_ghosts` in `config.toml` (default 3; set to 0 to disable). When the cap is reached, `spawn_ghost_shell` and scheduled ghost jobs return a `[Ghost Shell Skipped]` event rather than failing silently.

## Lifecycle Events in Catch-up Briefs

All ghost lifecycle events are injected into active sessions and surfaced in catch-up briefs after a detach:
- `[Ghost Shell Started]` — session created and first AI turn started
- `[Ghost Shell Completed]` — ghost finished within its turn budget
- `[Ghost Shell Failed]` — an error stopped the ghost (session ID included)
- `[Ghost Shell Skipped]` — concurrency cap prevented the ghost from starting

## Operational Checklist Before Spawning a Ghost

1. Verify the runbook exists: `read_runbook("name")`
2. Verify required scripts exist: `list_scripts()`
3. If scripts need sudo: verify sudoers rule exists or run `daemoneye install-sudoers <script>`
4. Check current ghost count in `daemoneye status` if nearing the cap
5. For SSH targets: confirm `ssh_target` is set in runbook frontmatter
