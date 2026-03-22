# Design Document: Autonomous Ghost Sessions

## 1. Objective
Enable DaemonEye to respond to webhook events autonomously when the user is offline or away from their terminal, without compromising the strict security model (FR-1.4.3).

## 2. Core Concept: Ghost Sessions
A "Ghost Session" is a headless tmux session orchestrated by DaemonEye in response to an automated trigger (like a webhook alert). It allows the AI to investigate and potentially remediate issues autonomously, within strict, pre-approved boundaries.

## 3. Architecture & Execution Flow

### 3.1. Trigger & Headless Initialization
1. A webhook fires, matches a runbook, and the AI watchdog determines an alert is valid.
2. The daemon checks for active user tmux sessions.
   - If one exists, it uses it.
   - If none exist (e.g., cold boot scenario), it creates a detached headless session: `tmux new-session -d -s daemoneye-incidents`.
3. The daemon creates a new window named `de-incident-<alertname>-<id>` in the selected session.
4. The daemon initializes a new `SessionEntry` in `SessionStore` marked `is_ghost = true`.
5. The AI is prompted with the alert context and a special system message indicating it is in an unattended Ghost Session and must use pre-approved scripts for remediation.

### 3.2. Runbook Boundaries
Runbooks (`runbook.rs`) are expanded to define the boundaries of the Ghost Session via YAML frontmatter:

```yaml
---
tags: [nginx, web]
memories: [nginx_quirks]
ghost_mode:
  enabled: true
  auto_approve_scripts: ["restart-nginx.sh", "clear-cache.sh"]
  auto_approve_read_only: true
---
```

### 3.3. The `--auto-approve-safe` Gate (`executor.rs`)
When the AI issues a `ToolCall` in a Ghost Session, the daemon routes it through a specialized offline approval gate:
*   **Read-Only Heuristic:** If `auto_approve_read_only` is true, commands starting with `cat`, `ls`, `grep`, `tail`, `journalctl`, `df`, `free`, etc., are automatically approved and run in the `de-incident-*` window.
*   **Script Whitelist:** If the command exactly matches the path of a script listed in `auto_approve_scripts`, it is automatically approved.
*   **Rejection:** If the command is mutating (e.g., `rm`, `kill`) and NOT a whitelisted script, or if a sudo password is required, it is immediately denied. The AI is informed so it can log a failure.

### 3.4. Handling Sudo & Credentials Offline
**DaemonEye will NEVER cache plaintext passwords.**
1. **Pre-Approved Sudo Script:** When creating a remediation script (e.g., `restart-nginx.sh`), the AI proposes dropping a NOPASSWD rule into `/etc/sudoers.d/daemoneye-restart-nginx` (e.g., `user ALL=(ALL) NOPASSWD: /home/user/.daemoneye/scripts/restart-nginx.sh`). The user approves this interactively.
2. **Offline Execution:** The Ghost Session runs `sudo /home/user/.daemoneye/scripts/restart-nginx.sh`. Because of the OS-level `sudoers.d` rule, the command executes without a password prompt.
3. **Fallback:** If the AI attempts a raw `sudo` command without a NOPASSWD rule, the OS will prompt for a password. DaemonEye detects this via `capture-pane`, realizes it is a Ghost Session, and automatically aborts the tool call.

### 3.5. The Handoff (User Re-attaches)
1. **Passive Monitoring:** When the user SSHs back in or re-attaches to tmux, the `client-attached` hook fires.
2. **Catch-up Brief:** The daemon injects a summary into their active chat pane:
   *`[Catch-up] Ghost session handled alert 'Nginx-502' in window 'de-incident-nginx-502-123'. AI successfully ran 'restart-nginx.sh'. Window left open for review.`*
3. **Taking Control:** The user can switch to the `daemoneye-incidents` session (or the specific `de-incident-*` window) to see the exact steps taken and optionally take over the session interactively.

## 4. Summary of Benefits
*   **High Availability:** Relies on tmux's native detached (`-d`) capabilities, ensuring Ghost Sessions work even if no human has logged in since boot.
*   **Zero Credential Caching:** Eliminates the risk of storing highly privileged credentials.
*   **Blast Radius Containment:** Scopes unattended AI actions to safe recon and explicitly whitelisted remediation scripts.
*   **Seamless UX:** Translates the offline autonomous actions into standard tmux windows for easy human review upon return.
