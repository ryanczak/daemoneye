# Scripts and Sudoers

Scripts are pre-vetted programs stored in `~/.daemoneye/scripts/` that Ghost Shells and scheduled jobs can run without per-command human approval. Scripts may be **shell** (`.sh`) or **Python** (`.py`) — Python is preferred for anything with data processing, JSON handling, or multi-step logic.

## Managing Scripts

```
write_script(name="cleanup-logs.sh", content="#!/bin/bash\n...")
write_script(name="check-disk.py", content="#!/usr/bin/env python3\n...")
read_script("check-disk.py")
list_scripts()
delete_script("check-disk.py")
```

Scripts are stored with `chmod 700`. Path traversal (e.g., `../../etc/passwd`) is blocked.

## Authoring Scripts for Ghost Shell Use

1. **One responsibility per script** — ghost shells compose scripts; keep each focused.
2. **Exit codes matter** — exit non-zero on failure so the ghost knows to escalate.
3. **Stdout is the report** — ghost shells read stdout to decide next steps; be descriptive.
4. **Idempotent where possible** — the ghost may re-run a script if it is unsure of the result.
5. **Python preferred for complexity** — use Python for JSON parsing, REST calls, arithmetic on metrics, or any logic that would require 3+ piped shell commands.

### Python script example

```python
#!/usr/bin/env python3
# check-disk.py — report disk usage; exit 1 if above threshold
import subprocess, sys

THRESHOLD = 90
result = subprocess.run(["df", "-h", "/"], capture_output=True, text=True)
for line in result.stdout.splitlines()[1:]:
    parts = line.split()
    if not parts:
        continue
    pct = int(parts[4].rstrip("%"))
    mount = parts[5]
    print(f"Disk usage on {mount}: {pct}%")
    if pct > THRESHOLD:
        print(f"WARNING: {mount} usage above {THRESHOLD}%")
        sys.exit(1)
print("OK")
```

### Shell script example

```bash
#!/bin/bash
# restart-nginx.sh — reload nginx config and restart if needed
set -euo pipefail
nginx -t 2>&1 && echo "Config OK" || { echo "Config invalid"; exit 1; }
systemctl restart nginx
echo "nginx restarted"
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

### Enable automatic sudo in a runbook

Set `run_with_sudo: true` in the runbook frontmatter. DaemonEye then automatically prepends `sudo` when executing scripts listed in `auto_approve_scripts` — the ghost AI just writes `script.sh` and it runs as root. The sudoers rule must already be in place; the daemon will not prompt for a password.

With `run_with_sudo: false` (default), scripts in `auto_approve_scripts` run as the current user unless the ghost explicitly writes `sudo script.sh`.

Either way, only scripts in `auto_approve_scripts` may use sudo — arbitrary sudo commands (e.g. `sudo apt install`) are always denied by the ghost policy.

### Checklist before enabling `run_with_sudo`

1. `write_script("my-script.sh", ...)` — create and review the script
2. `daemoneye install-sudoers my-script.sh` — install the NOPASSWD rule
3. Add `my-script.sh` to `auto_approve_scripts` in the runbook frontmatter
4. Set `run_with_sudo: true` in the runbook frontmatter so it runs automatically with sudo

### Security notes

- Only scripts in `~/.daemoneye/scripts/` are eligible for sudoers rules.
- The NOPASSWD rule is scoped to the exact absolute path of the script — changing the script file after installation requires reinstalling the rule.
- Prefer narrow NOPASSWD rules (one script per rule) over broad `ALL` grants.
