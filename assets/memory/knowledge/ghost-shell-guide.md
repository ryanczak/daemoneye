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

**From a webhook** — configure the runbook with `enabled: true` in frontmatter. When the watchdog analysis emits `GHOST_TRIGGER: YES`, DaemonEye spawns the ghost automatically.

## Runbook Frontmatter for Ghost Shells

```yaml
---
tags: [disk, ops]
memories: [disk-thresholds]
enabled: true
auto_approve_scripts: [check-disk.sh, cleanup-logs.sh]
run_with_sudo: false
max_ghost_turns: 20
ssh_target: ""
model: opus
---
```

| Field | Description |
|---|---|
| `enabled` | Must be `true` for webhook-triggered ghosts |
| `auto_approve_scripts` | Script names in `~/.daemoneye/scripts/` the ghost may run. Only these scripts may use sudo (see `run_with_sudo`). Always required for scripts that need elevated privileges. |
| `run_with_sudo` | `true` = the daemon automatically prepends `sudo` when executing scripts in `auto_approve_scripts` — the ghost AI just writes `script.sh` and it runs as root. `false` (default) = scripts run as the current user unless the ghost explicitly writes `sudo script.sh` (still requires the script to be in `auto_approve_scripts`). Either way, only scripts in `auto_approve_scripts` may use sudo; arbitrary sudo commands (e.g. `sudo apt install`) are always denied. |
| `max_ghost_turns` | Hard turn limit (0 = use daemon default of 20) |
| `ssh_target` | If set (e.g. `user@host`), all commands are wrapped in `ssh <target> <cmd>` |
| `model` | Optional. Name of a `[models.<name>]` entry in `config.toml`. When set, this ghost uses that model instead of `[models.default]`. Omit to use the default. |

## Ghost Policy — What Gets Approved

The ghost operates under a simple OS-delegation model:

- **Non-sudo commands** — always allowed. The daemon runs as the same user as you; OS file permissions are the boundary.
- **Sudo commands** — only allowed when the script basename is listed in `auto_approve_scripts` AND has a NOPASSWD sudoers rule installed via `daemoneye install-sudoers <script>`. Any other sudo command is auto-denied and logged.
- **`run_with_sudo: true`** — the daemon auto-prepends `sudo` when executing approved scripts, so the ghost AI just writes `script.sh` instead of `sudo script.sh`. Does NOT grant permission to run arbitrary sudo commands.
- **`run_with_sudo: false` (default)** — approved scripts run as the current user unless the ghost explicitly writes `sudo script.sh`.

This means the ghost can freely run `ps`, `df`, `curl`, `journalctl`, `systemctl status`, etc. without any configuration. Sudo access always requires an explicit `auto_approve_scripts` entry and a NOPASSWD sudoers rule.

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

## Concurrency Cap

The daemon limits concurrent ghosts via `max_concurrent_ghosts` in `config.toml` (default 3; set to 0 to disable). When the cap is reached, `spawn_ghost_shell` and scheduled ghost jobs return a `[Ghost Shell Skipped]` event rather than failing silently.

## Lifecycle Events in Catch-up Briefs

All ghost lifecycle events are injected into active sessions and surfaced in catch-up briefs after a detach:
- `[Ghost Shell Started]` — session created and first AI turn started — includes `— session log: <path>`
- `[Ghost Shell Completed]` — ghost finished within its turn budget — includes `— session log: <path>`
- `[Ghost Shell Failed]` — an error stopped the ghost — includes `— session log: <path>`
- `[Ghost Shell Skipped]` — concurrency cap prevented the ghost from starting

When you see a `[Ghost Shell Completed]` or `[Ghost Shell Failed]` event, use `read_file(<path>)` on the session log path to review the full ghost conversation — what it investigated, which commands it ran, and the final outcome summary. Pane logs for individual background commands are in `~/.daemoneye/var/log/panes/` and are referenced in tool results when output was truncated.

## Troubleshooting Ghost Shells

Ghost shell activity is logged at multiple levels:

**`~/.daemoneye/var/log/daemon.log`** — human-readable trace:
- `INFO Ghost Shell started: <id> (alert: ..., tmux_session: ..., bg_prefix: ...)` — session created
- `INFO Ghost Shell <id>: starting turn N/M` — each turn start
- `INFO Ghost Shell <id>: turn N dispatching '<tool>'` — each tool call with command
- `INFO Ghost Shell auto-approved background: <cmd>` — policy allowed a command
- `INFO Ghost Shell auto-denied (sudo command not on whitelist): <cmd>` — policy denied (only when `run_with_sudo: false`)
- `INFO Ghost Shell <id>: completed in N turn(s)` — successful completion
- `WARN Ghost Shell <id>: reached max turns (N), stopping` — turn budget exhausted
- `ERROR Ghost Shell <id>: turn N timed out after 300s` — AI call hung

**`~/.daemoneye/var/log/events/events-<date>.jsonl`** — structured records (searchable via `search_repository`):
- `ghost_start` — session created: `session_id`, `alert_name`, `tmux_session`, `trigger`
- `ghost_lifecycle` — every lifecycle event message injected into sessions (started/completed/failed/skipped)
- `ghost_turn` — per-turn tool dispatch: `session_id`, `turn`, `tool_count`, `tools` (array of `{name, cmd}`)
- `ghost_complete` — successful finish: `session_id`, `turns_used`
- `ghost_error` — errors/timeouts/denials: `session_id`, `turn`, `error`
- `sandbox_egress` — a sandbox's allowed and blocked hosts, read before teardown: `sandbox`, `allowed`, `blocked`
- `ghost_sandbox_teardown` — the ghost sandbox's removal outcome: `session`, `outcome`

**`~/.daemoneye/var/log/sessions/ghost-<name>-<uuid>.jsonl`** — full message history including all tool calls and results. Created immediately when the session starts (even if the ghost fails before its first turn).

**Shell recordings** — every command's output, in the shell's cast log: `list_shell_logs` / `read_shell_log` (tmux-backend ghosts also keep `~/.daemoneye/var/log/panes/<win_name>.log`).

## Operational Checklist Before Spawning a Ghost

1. Verify the runbook exists: `read_runbook("name")`
2. Verify required scripts exist: `list_scripts()`
3. If using `run_with_sudo: false` and scripts need sudo: verify sudoers rule exists or run `daemoneye install-sudoers <script>`
4. Check current ghost count in `daemoneye status` if nearing the cap
5. For SSH targets: confirm `ssh_target` is set in runbook frontmatter
