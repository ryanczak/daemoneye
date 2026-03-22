# Ghost-Enabled Runbook Template

Use this template when creating a runbook that supports autonomous remediation via Ghost Sessions.

```markdown
---
tags: [service, alert-type]
memories: [relevant-knowledge-key]
enabled: true
auto_approve_scripts: [remediation-script.sh]
run_with_sudo: false
auto_approve_read_only: true
max_ghost_turns: 20
---
# Runbook: <name>

## Purpose
Brief description of the service and the specific issue this runbook addresses.

## Alert Criteria
- Thresholds (e.g., CPU > 90% for 5m)
- Error patterns (e.g., "502 Bad Gateway" in logs)

## Remediation Steps
1. **Investigation**: Informational commands to verify the state (auto-approved if `auto_approve_read_only: true`).
2. **Action**: Execute pre-approved scripts (auto-approved if listed in `auto_approve_scripts`).
3. **Escalation**: Steps to take if autonomous remediation fails.

## Notes
Lessons learned and manual overrides performed by humans.
```

## Frontmatter Fields for Ghost Mode

- `enabled`: Set to `true` to allow DaemonEye to spawn an autonomous Ghost Session for this alert.
- `auto_approve_scripts`: A list of script names in `~/.daemoneye/scripts/` that the AI is authorized to run without human approval.
- `run_with_sudo`: Set to `true` to prepend `sudo` when executing approved scripts. Pair with a `/etc/sudoers.d/` `NOPASSWD` entry so the script runs with elevated privileges without a password prompt. Defaults to `false`.
- `auto_approve_read_only`: Set to `true` to allow the AI to run safe informational commands (e.g., `ps`, `df`, `ls`) automatically.
- `max_ghost_turns`: Maximum number of AI turns before the session is forcibly stopped. Defaults to 20 if omitted or set to 0.
