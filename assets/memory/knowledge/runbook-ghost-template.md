# Ghost-Enabled Runbook Template

Use this template when creating a runbook that supports autonomous remediation via Ghost Shells.

```markdown
---
tags: [service, alert-type]
memories: [relevant-knowledge-key]
enabled: true
auto_approve_scripts: [remediation-script.sh]
run_with_sudo: false
max_ghost_turns: 20
# model: opus   ← optional: name of a [models.<name>] entry; omit to use default
---
# Runbook: <name>

## Purpose
Brief description of the service and the specific issue this runbook addresses.

## Alert Criteria
- Thresholds (e.g., CPU > 90% for 5m)
- Error patterns (e.g., "502 Bad Gateway" in logs)

## Remediation Steps
1. **Investigation**: Use any non-sudo commands freely (ps, df, journalctl, curl, etc. — no special configuration needed).
2. **Action**: Execute pre-approved scripts for steps that require sudo (must be listed in `auto_approve_scripts` with a NOPASSWD sudoers rule).
3. **Escalation**: Steps to take if autonomous remediation fails.

## Notes
Lessons learned and manual overrides performed by humans.
```

## Frontmatter Fields for Ghost Mode

- `enabled`: Set to `true` to allow DaemonEye to spawn an autonomous Ghost Shell for this alert.
- `auto_approve_scripts`: Script names in `~/.daemoneye/scripts/` the ghost may run with sudo. Always required for scripts that need elevated privileges. Each script must have a NOPASSWD sudoers rule installed via `daemoneye install-sudoers <script>`.
- `run_with_sudo`: `true` = the daemon automatically prepends `sudo` when running scripts in `auto_approve_scripts` — the ghost just writes `script.sh` and it executes as `sudo /path/to/script.sh`. `false` (default) = scripts run as the current user; the ghost must explicitly write `sudo script.sh` to run with root. Either way, only scripts in `auto_approve_scripts` may use sudo — arbitrary sudo commands are always denied.
- `max_ghost_turns`: Maximum number of AI turns before the session is forcibly stopped. Defaults to 20 if omitted or set to 0.
- `model`: Optional. Name of a `[models.<name>]` entry in `config.toml` (e.g. `opus`). When set, the ghost uses that model instead of `[models.default]`. Omit to use the default model.
