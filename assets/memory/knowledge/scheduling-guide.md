# Scheduling Guide

DaemonEye has a built-in scheduler that runs jobs on a one-shot or recurring basis. Use `schedule_command` to create jobs from a chat session.

## Schedule Kinds

| Kind | When to use | `schedule_command` params |
|---|---|---|
| `Once` (run_at) | One-time future execution | `run_at: "2025-06-01T09:00:00Z"` |
| `Every` (interval) | Recurring on a fixed interval | `interval: "1h"`, `"30m"`, `"7d"` |
| `Cron` | Recurring on a calendar schedule | `cron: "0 6 * * *"` (5-field standard cron) |

Intervals use ISO 8601 duration notation: `1h` = 1 hour, `30m` = 30 minutes, `7d` = 7 days.

Cron expressions follow standard 5-field format: `minute hour day-of-month month day-of-week`. Common examples:
- `"0 6 * * *"` — 06:00 daily
- `"*/15 * * * *"` — every 15 minutes
- `"0 9 * * 1-5"` — 09:00 weekdays

## Action Types

### `ghost_runbook` — Ghost Shell job (preferred for AI-driven tasks)
```
schedule_command(
  name="nightly-cert-check",
  ghost_runbook="cert-expiry-check",
  cron="0 6 * * *"
)
```
Spawns an autonomous Ghost Shell that follows the named runbook. The ghost has its own turn budget, tool policy, and lifecycle events. Use this for any job that requires investigation, conditional logic, or remediation decisions.

### Script job (preferred for deterministic shell tasks)
```
schedule_command(
  name="nightly-cleanup",
  command="cleanup-logs.sh",
  is_script=true,
  cron="0 2 * * *"
)
```
Runs a pre-vetted script from `~/.daemoneye/scripts/` on a schedule. Output is captured and appended to the session context.

### Raw command (deprecated — refused)
Raw `command` strings without `is_script=true` still **load** from old
`schedules.json` files but are refused when they fire, with a logged reason.
Prefer named scripts for auditability and ghost-shell compatibility.

## Where scheduled work runs

Each job's script runs in a scheduler-owned PTY shell with a real exit code
(the shell's recording stays in `~/.daemoneye/var/log/shells/`). A scheduled
ghost job (`ghost_runbook`) follows the ghost-shell rules: the ghost's own
PTY shell — its sbx sandbox when enabled, otherwise the daemon host or the
configured `[hosts]` entry its runbook's `ssh_target` names. A configured host
overrides the sandbox only when the ghost is explicitly granted that host.

## Managing Schedules

```
list_schedules()          # see all scheduled jobs with next run times
cancel_schedule("name")   # pause a job (keeps it in the store)
delete_schedule("name")   # permanently remove a job
```

## Watchdog Analysis

Scheduled script/command jobs can run watchdog analysis on their output:
```
schedule_command(
  name="disk-check",
  command="check-disk.sh",
  is_script=true,
  interval="1h",
  watchdog_runbook="high-disk-usage"
)
```
The watchdog model analyzes output against the runbook and emits `GHOST_TRIGGER: YES` or `GHOST_TRIGGER: NO` on the last line. If `YES`, a Ghost Shell is automatically spawned using that runbook.

## Schedule Persistence

Schedules survive daemon restarts — they are persisted to `~/.daemoneye/var/run/schedules.json`. Jobs that were due while the daemon was down fire on next startup.
