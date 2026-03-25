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
- `auto_approve_scripts`: A list of script names in `~/.daemoneye/scripts/` pre-approved for **sudo** execution. Non-sudo commands run freely without listing them here.
- `run_with_sudo`: Set to `true` to prepend `sudo` when executing approved scripts. Pair with a `/etc/sudoers.d/` `NOPASSWD` entry via `daemoneye install-sudoers`. Defaults to `false`.
- `max_ghost_turns`: Maximum number of AI turns before the session is forcibly stopped. Defaults to 20 if omitted or set to 0.
