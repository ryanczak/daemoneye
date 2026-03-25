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
- `auto_approve_scripts`: Script names in `~/.daemoneye/scripts/` that may use sudo when `run_with_sudo: false`. Not needed when `run_with_sudo: true`.
- `run_with_sudo`: `true` = the ghost may run **any** sudo command freely (broad root access). Pair with NOPASSWD sudoers rules for password-free execution. `false` (default) = only scripts listed in `auto_approve_scripts` may use sudo; all other sudo commands are denied.
- `max_ghost_turns`: Maximum number of AI turns before the session is forcibly stopped. Defaults to 20 if omitted or set to 0.
