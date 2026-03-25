# Scripts and Sudoers

Scripts are pre-vetted shell scripts stored in `~/.daemoneye/scripts/` that Ghost Shells and scheduled jobs can run without per-command human approval.

## Managing Scripts

```
write_script(name="cleanup-logs.sh", content="#!/bin/bash\n...")
read_script("cleanup-logs.sh")
list_scripts()
delete_script("cleanup-logs.sh")
```

Scripts are stored with `chmod 700`. Path traversal (e.g., `../../etc/passwd`) is blocked.

## Authoring Scripts for Ghost Shell Use

1. **One responsibility per script** — ghost shells compose scripts; keep each focused.
2. **Exit codes matter** — exit non-zero on failure so the ghost knows to escalate.
3. **Stdout is the report** — ghost shells read stdout to decide next steps; be descriptive.
4. **Idempotent where possible** — the ghost may re-run a script if it is unsure of the result.

Example:
```bash
#!/bin/bash
# check-disk.sh — report disk usage on /dev/sda1
set -euo pipefail
USAGE=$(df -h /dev/sda1 | awk 'NR==2 {print $5}' | tr -d '%')
echo "Disk usage: ${USAGE}%"
if [ "$USAGE" -gt 90 ]; then
  echo "WARNING: disk usage above 90%"
  exit 1
fi
echo "OK"
```

## Sudoers — Running Scripts with Elevated Privileges

Some remediation scripts (e.g., `systemctl restart`, `iptables`) require root. DaemonEye supports passwordless sudo for pre-vetted scripts via a sudoers drop-in.

### Install a NOPASSWD rule

```
daemoneye install-sudoers <script-name>
```

This writes `/etc/sudoers.d/daemoneye-<name>` with:
```
<user> ALL=(ALL) NOPASSWD: /home/<user>/.daemoneye/scripts/<name>
```
Then validates the file with `sudo visudo -c` (rolls back on failure).

### Enable sudo in a runbook

Set `run_with_sudo: true` in the runbook frontmatter. DaemonEye prepends `sudo` when executing the listed `auto_approve_scripts`. The sudoers rule must already be in place — the daemon will not prompt for a password.

### Checklist before enabling `run_with_sudo`

1. `write_script("my-script.sh", ...)` — create and review the script
2. `daemoneye install-sudoers my-script.sh` — install the NOPASSWD rule
3. Add `my-script.sh` to `auto_approve_scripts` in the runbook frontmatter
4. Set `run_with_sudo: true` in the runbook frontmatter

### Security notes

- Only scripts in `~/.daemoneye/scripts/` are eligible for sudoers rules.
- The NOPASSWD rule is scoped to the exact absolute path of the script — changing the script file after installation requires reinstalling the rule.
- Prefer narrow NOPASSWD rules (one script per rule) over broad `ALL` grants.
