# Runbook Format & Conventions

## Standard Template

```markdown
---
tags: [tag1, tag2]
memories: [knowledge-key1, knowledge-key2]
---
# Runbook: <name>

## Purpose
One sentence describing what this runbook handles.

## Alert Criteria
- What conditions trigger this runbook
- Relevant thresholds or signals

## Remediation Steps
1. Step one
2. Step two

## Notes
Updated after each resolution with lessons learned.
```

## Naming Convention

Runbook filenames are kebab-case. For Prometheus alerts, convert the CamelCase
alertname to kebab-case:

  HighDiskUsage      → high-disk-usage
  PodCrashLoopBackOff → pod-crash-loop-back-off

DaemonEye auto-analysis looks up runbooks by this converted name when an alert fires.
**Always create the runbook BEFORE configuring the Prometheus alert rule.**

## Frontmatter Fields

- `tags`: free-form labels for search_repository
- `memories`: list of `knowledge` memory keys to auto-load during watchdog analysis
- `enabled`: (bool) set `true` to enable autonomous Ghost Shells for this runbook
- `auto_approve_scripts`: (list) script names in `~/.daemoneye/scripts/` pre-approved for **sudo** execution; non-sudo commands run freely without listing them
- `run_with_sudo`: (bool) prepend `sudo` when executing `auto_approve_scripts`; requires a NOPASSWD sudoers rule — see `scripts-and-sudoers` memory
- `max_ghost_turns`: (int) turn budget for the Ghost Shell; 0 or omitted = daemon default (20)
- `ssh_target`: (string) if set (e.g. `user@host`), approved scripts are wrapped in `ssh <target> <cmd>`

## Unified Header Format

All three DaemonEye artifact types share the same field names:

| Artifact | Header style | Sentinel |
|---|---|---|
| Runbook | YAML `---` frontmatter | `---` / `---` |
| Memory | YAML `---` frontmatter | `---` / `---` |
| Script | Comment block | `# --- daemoneye ---` / `# --- /daemoneye ---` |

Fields (`tags`, `summary`, `relates_to`, `expires`) mean the same thing in all three.
Scripts also support extra fields like `run_with_sudo: true` in their header.

## Key Rules

- `write_runbook` OR `search_repository` (not both) to check for duplicates before creating
- After resolving an alert, update the `## Notes` section with what you learned
- `delete_runbook` requires user approval
