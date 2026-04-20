# Product Definition: DaemonEye

## The Problem

Infrastructure operations are stuck between two extremes. On one side: human operators manually SSHing into servers, reading logs, running commands, and firefighting incidents at 3am. On the other: fully autonomous AI agents that terrify anyone who has ever seen `rm -rf` go wrong.

The gap between "I need help" and "I trust you to do it alone" is where most AI tooling fails. Chatbots suggest commands but can't run them. Autonomous agents run commands but can't be trusted. Neither approach lets an organization incrementally transfer operational knowledge and authority from humans to AI in a controlled, auditable way.

DaemonEye closes that gap.

## What DaemonEye Is

DaemonEye is a Linux daemon that embeds AI operators directly into your terminal workflow via tmux. It pairs human experts with AI agents across a trust spectrum -- from fully supervised pair-programming to fully autonomous incident response -- with every point in between available and every action logged.

The key insight: **autonomy is not binary**. A team that starts with "approve every command" can progressively grant the AI more authority -- per-command-class, per-script, per-runbook -- as trust is earned through observable, measurable behavior. DaemonEye makes this progression natural, safe, and reversible.

**Type**: Linux daemon + tmux integration
**Written in**: Rust (single static binary, no runtime dependencies beyond tmux)
**AI Providers**: Anthropic Claude, OpenAI, Google Gemini, Ollama, LM Studio -- switchable per-session or per-runbook

---

## Target Audience

- **SREs and Platform Engineers** managing production infrastructure, responding to alerts, and maintaining complex systems across multiple hosts and cloud accounts.
- **System Administrators** running fleets of servers, deploying applications, performing configuration management, and troubleshooting live production issues.
- **DevOps Teams** operating CI/CD pipelines, cloud infrastructure, and container orchestration from the terminal.
- **Security Engineers** auditing systems, analyzing scan output, and applying remediation at scale.

---

## The Trust Spectrum: From Supervised to Autonomous

DaemonEye's design principle is that organizations should be able to dial AI autonomy up or down based on risk, context, and earned trust. Every feature maps to a point on this spectrum.

### Level 1: Supervised Pair-Programming

The AI sees your terminal -- scrollback, environment variables, running processes, command history -- and proposes actions. Every command requires explicit approval before execution.

- **Three-option approval**: Approve once, approve the command class for the session, or deny. Sudo commands have a separate approval scope.
- **Visual confirmation**: The target pane highlights blue during the approval window so you always know where a command will land.
- **Pinned pane targeting**: Every AI turn includes a `[FOREGROUND TARGET]` line naming the exact pane where foreground commands will run. Use `/pane` to list options or `/pane %N` to change it. If the target shifts (pane closed, focus moved), the daemon announces the change before the AI responds.
- **Mid-stream redirect**: Instead of approving or denying, type a message to redirect the AI's approach entirely -- no synthetic errors, no wasted context.

This is where every team starts. The AI is useful from minute one: reading error logs, suggesting commands, explaining failures. But it cannot act without your say-so.

### Level 2: Session-Level Trust

Once you've seen the AI handle a class of operations reliably, grant session-level approval. Regular commands, sudo commands, specific scripts, specific runbooks, and specific file paths each have independent approval scopes.

- `/approvals` shows what's currently trusted across all five scopes (commands, sudo, scripts, runbooks, files), listing each approved item by name. `/approvals revoke` revokes everything instantly; `/approvals revoke <class>` revokes just that class (`commands`, `scripts`, `runbooks`, or `files`).
- Session approvals reset on `/clear`, `/prompt`, `/refresh`, and Ctrl+C — they never persist beyond the current interaction.
- The status bar shows active approvals at all times using compact count-based segments (e.g. `⚡approvals: all · scripts: 2 · files: 1 · Ctrl+C revokes`) so there is never any ambiguity about what the AI can do.
- Cumulative approval and denial counts for script writes, runbook writes, and file edits are tracked by the daemon and surfaced in `daemoneye status`, giving long-running sessions an audit trail.

### Level 3: Scheduled Operations

The AI can schedule commands, scripts, or Ghost Shell tasks to run once, on an interval, or on a cron expression. Watchdog monitors use AI-powered analysis to evaluate system state on a schedule and trigger remediation when something looks wrong.

- Each job runs in an isolated tmux window, left open on failure for inspection.
- Watchdog jobs reference runbooks and knowledge memories, giving the AI structured context for its analysis.
- Results appear in your next catch-up brief if you were away.

### Level 4: Autonomous Ghost Shells

When a critical alert fires, DaemonEye can spawn a Ghost Shell -- an unattended AI agent that investigates and remediates the problem in a dedicated tmux window while you sleep.

Ghost Shells are the highest trust level, and they are the most tightly controlled:

- **Opt-in per runbook**: Only runbooks with `enabled: true` can trigger a Ghost Shell. No runbook, no autonomy.
- **Script whitelisting**: Sudo commands are restricted to scripts explicitly listed in `auto_approve_scripts` that also have a NOPASSWD sudoers rule installed via `daemoneye install-sudoers`. Both gates must pass. Everything else is denied.
- **Turn budget**: A hard ceiling on AI turns (default 20, configurable per-runbook and per-daemon) prevents runaway agents. The ghost is forcibly stopped when the limit is reached.
- **Concurrency cap**: A daemon-wide limit (default 3) prevents ghost shell storms during cascading failures.
- **Full audit trail**: Every command, approval, result, and lifecycle event is logged to `events.jsonl`. Ghost session transcripts are preserved for post-incident review.
- **Catch-up briefs**: When you re-attach after being away, DaemonEye delivers a summary of everything that happened -- ghost shells started, completed, or failed; alerts received; watchdog results.

---

## Core Capabilities

### Deep Terminal Awareness

DaemonEye doesn't just read your current pane. It understands the full topology of your tmux session:

- **Session topology**: Window names, pane counts, active/zoomed state, bell and activity flags.
- **Per-pane context**: Current working directory, running command, OSC terminal title, synchronized-input state, dead-pane exit codes, activity recency.
- **Environment capture**: Cloud account (AWS_PROFILE), Kubernetes cluster (KUBECONFIG), vault address, runtime environment, and active language runtimes -- via a curated allowlist from `tmux show-environment`.
- **Cross-session awareness**: When multiple tmux sessions exist, the AI sees all of them -- names, window counts, last activity, attachment state -- so it can reason across parallel workstreams.
- **Semantic ANSI parsing**: Red terminal output becomes `[ERROR: text]`, yellow becomes `[WARN: text]`, green becomes `[OK: text]` -- the AI identifies failures at a glance.
- **Viewport awareness**: The AI knows your terminal dimensions and adjusts output accordingly.
- **On-demand refresh**: After the first turn's full snapshot, the AI requests fresh context only when it needs it, keeping conversation lean.

### Dual Execution Modes

- **Background mode**: Commands run in dedicated daemon-host windows (`de-bg-*`), returning immediately with a pane ID. Output is archived. Windows are named `de-bg-<pane_num>-<unix_ts>-<cmd_slug>` (e.g. `de-bg-42-1712937600-cargo-build`) so `tmux list-windows` is immediately readable. A 60-second GC task automatically reclaims dead, idle-completed, and orphaned windows. The AI can chain follow-up commands in the same environment.
- **Foreground mode**: Commands are injected into your active terminal pane via `send-keys`. Completion is detected via PID tracking (local panes) or output stability (remote panes). Interactive commands like `ssh` and `mosh` are handled as a special case -- the daemon returns once the connection is established. The target pane is pinned at session start and injected as `[FOREGROUND TARGET]` on every AI turn so the agent never has to guess where a command will land. Use `/pane` to list targets or `/pane %N` to pin a different one; the AI is notified on the next turn automatically.

### Knowledge System

Three-tier persistence that lets the AI accumulate and apply operational knowledge:

- **Session memory**: Auto-injected into every AI turn. Facts the AI should always know during this engagement.
- **Knowledge memory**: Loaded on-demand by runbooks, watchdogs, and the AI itself. Organizational knowledge that outlives any single session.
- **Incident records**: Historical, searchable records of past incidents with root cause, symptoms, and resolution.

Memory entries support tags (with synonym matching), summaries, cross-references (`relates_to`), and TTLs (`expires`). Contextual auto-search follows relationship links to surface relevant knowledge automatically -- a match on `db-hosts` can pull in `db-quirks` and the `postgres-failover` runbook without anyone needing to know those keys exist.

Six built-in knowledge guides are seeded on first run covering webhooks, runbook format, ghost shells, scheduling, scripts, and sudoers setup.

### Scripts and Runbooks

- **Scripts** (`~/.daemoneye/scripts/`): Shell or Python, chmod 700, managed via AI tools with approval diffs. The AI defaults to Python for data processing and multi-step logic, shell for simple orchestration.
- **Runbooks** (`~/.daemoneye/runbooks/`): Markdown with structured frontmatter. Define alert criteria, remediation steps, ghost shell policy, model selection, SSH targets, and referenced knowledge memories. Runbooks can be prescriptive (fixed script) or open-ended (AI has latitude to investigate).

### Webhook Alert Ingestion

An optional HTTP endpoint (default port 9393) accepts alerts from Prometheus Alertmanager, Grafana, or any JSON source.

- **Deduplication** by fingerprint within a configurable window.
- **Sensitive data masking** before alerts enter the AI conversation.
- **Automatic runbook matching**: Alert names are matched to runbook filenames (kebab-case conversion).
- **Watchdog analysis**: Matching runbooks trigger AI-powered analysis that can escalate to a Ghost Shell.
- **Session injection**: Alerts appear in all active AI sessions and as tmux overlay messages.
- **Notification hooks**: Configurable external commands (`notify-send`, PagerDuty, Slack) fire on alert conditions.

### Multi-Model Support

Different tasks deserve different models. DaemonEye supports multiple named model configurations:

- Switch models mid-session with `/model <name>`.
- Pin a specific model to a runbook via frontmatter -- use a powerful model for complex incident response, a cheap local model for routine checks.
- Supports Anthropic, OpenAI, Gemini, Ollama, and LM Studio out of the box.

---

## Security

Giving an AI access to your terminal is a security decision. DaemonEye treats it as one. Every feature that increases the AI's capability has a corresponding control that limits its blast radius, and every action the AI takes is logged in a format designed for audit and incident review.

### Secrets Never Leave the Host

Before any terminal output, file content, or environment variable reaches an AI provider, DaemonEye's masking filter strips it of secrets. This is not optional and cannot be disabled.

- **Built-in patterns**: AWS access keys, PEM private keys, GCP service account credentials, JWTs, GitHub tokens, database connection strings, passwords in URLs, credit card numbers, and SSNs are all caught by default.
- **User-defined patterns**: Organizations can add custom regexes for internal secret formats. Built-in patterns cannot be removed.
- **Per-type counters**: `daemoneye status` shows how many redactions have occurred per category, so you can verify the filter is catching what it should.
- **Applied everywhere**: Redaction runs on terminal capture, file reads, environment snapshots, and webhook alert payloads -- every path into the AI conversation.

### Auditable Root Access

DaemonEye never asks for blanket sudo. Instead, it provides a controlled pipeline from script creation to privileged execution:

1. The AI writes a script to `~/.daemoneye/scripts/` (chmod 700, diff-reviewed and approved by the user).
2. An administrator runs `daemoneye install-sudoers <script-name>`, which writes a NOPASSWD rule to `/etc/sudoers.d/daemoneye-<name>` pinning the exact file path. No wildcards. No `ALL`.
3. Ghost shells and scheduled jobs can only sudo scripts that pass **both** gates: listed in the runbook's `auto_approve_scripts` **and** backed by an installed sudoers rule. If either gate fails, the command is denied.

Interactive sudo passwords are collected in the chat pane with terminal echo disabled. The password is never written to disk, never stored in memory beyond the immediate use, and never transmitted to the AI provider.

### Complete Audit Trail

Every action the AI takes is recorded to `events.jsonl` as structured JSON:

- **Command executions**: The command, target pane, approval decision (approve/session-approve/deny/redirect), exit code, and captured output.
- **AI interactions**: Tool calls, tool results, model responses, and token usage.
- **Ghost shell lifecycle**: Start, completion, failure, turn count, and the runbook that triggered it.
- **Webhook alerts**: Incoming alert payloads (post-redaction), dedup decisions, runbook matches, and watchdog analysis results.
- **Session transcripts**: Ghost shell transcripts are preserved in full for post-incident review.

This log is append-only and designed for grep, jq, and integration with centralized logging systems.

### Guardrails on Autonomy

The trust spectrum is also a security architecture. Each level of autonomy has hard limits:

- **Session approvals reset automatically** on `/clear`, `/prompt`, `/refresh`, and Ctrl+C. They never persist beyond the current interaction.
- **Ghost shells require opt-in per runbook** (`enabled: true`). No runbook, no autonomy.
- **Turn budgets** cap how many actions a ghost shell can take (default 20, configurable per-runbook). The ghost is forcibly stopped when the limit is reached.
- **Concurrency caps** prevent ghost shell storms during cascading failures (default 3 concurrent ghosts, daemon-wide).
- **Path-restricted file access**: `read_file` is blocked only from `etc/config.toml` and `etc/prompts/sre.toml` (API credential files); all other `~/.daemoneye/` paths are readable so the agent can self-introspect its audit trail, logs, and managed data. `edit_file` remains fully blocked from all of `~/.daemoneye/` — daemoneye-managed data is written only through dedicated tools (`write_script`, `write_runbook`, `add_memory`, etc.).

---

## Key Workflows

### Workflow 1: Interactive Troubleshooting

A database service fails with a cryptic 50-line error trace. You hit the AI keybinding. DaemonEye captures the scrollback, identifies the error, and proposes `sudo kill -9 <PID>` for the zombie process holding the port. You approve; the AI runs the command, verifies the port is free, and restarts the service. Total time: under a minute, zero context-switching.

### Workflow 2: Unattended Incident Response

At 2am, Prometheus fires a `HighDiskUsage` alert. The webhook hits DaemonEye. The daemon matches the alert to the `high-disk-usage` runbook, runs a watchdog analysis, and determines autonomous remediation is warranted. A Ghost Shell spawns, identifies stale log files consuming 40GB, runs the pre-approved cleanup script, verifies disk usage is back under threshold, and shuts down. When you check in the morning, the catch-up brief reads: *"1 event while you were away (4h): Ghost Shell completed -- HighDiskUsage resolved, /var/log cleaned, disk at 62%."*

### Workflow 3: Progressive Trust Building

Week 1: You approve every command. The AI helps you write a disk-cleanup script and a runbook. Week 2: You session-approve regular commands -- the AI handles routine checks without prompting. Week 3: You enable the webhook and let watchdog analysis run overnight, reviewing results each morning. Week 4: You set `enabled: true` on the runbook, install the sudoers rule, and the AI handles disk alerts end-to-end. Each step is a deliberate, reversible decision.

### Workflow 4: Knowledge Accumulation

The AI discovers a host runs Postgres on a non-standard port and stores the fact as a knowledge memory. Weeks later, during a connection failure, contextual auto-search surfaces the quirk automatically. After resolving a major outage, the AI writes an incident record with root cause, symptoms, and resolution -- searchable forever, available to every future session and ghost shell.

### Workflow 5: Fleet Operations

From a jump server, you ask the AI to update Nginx across 15 web servers listed in `fleet.txt`. The AI writes a Python script that parallelizes the rollout with health checks between batches. You review the diff, approve, and watch the progress in a background pane while continuing your work in the foreground.

### Workflow 6: Scheduled Monitoring

You ask the AI to set up a watchdog that checks disk usage every 10 minutes. It stores the alert threshold in a knowledge memory, writes a runbook with alert criteria, and creates a cron-scheduled watchdog job. Every 10 minutes, the daemon runs `df -h`, loads the threshold from memory, and has the AI evaluate whether action is needed. If usage exceeds the threshold, a notification fires. If the runbook is ghost-enabled, autonomous cleanup follows.

---

## Why DaemonEye

**It meets teams where they are.** Not every organization is ready for fully autonomous AI operations. DaemonEye doesn't force that choice. Start with pair-programming, graduate to autonomy as trust is earned.

**It works inside your existing workflow.** No new tools to learn, no dashboards to monitor, no agents to deploy. DaemonEye lives in tmux -- the same terminal environment operators already use. A keybinding summons the AI; detach and it keeps working.

**Every action is observable and auditable.** Structured event logs, session transcripts, redaction counters, ghost shell turn limits, catch-up briefs. There is no black box. When something goes wrong, you can trace exactly what happened and why.

**Security is structural, not bolted on.** Secrets are redacted before they can reach any AI provider -- always on, not configurable off. Sudo access requires two independent gates (script whitelist + sudoers rule) with no wildcards. Every command, approval, and AI interaction is logged as structured JSON. The AI cannot read or modify its own configuration. These are not features you enable; they are constraints you cannot remove.

**Autonomy is granular and reversible.** Per-command-class session approvals. Per-script sudo whitelisting. Per-runbook ghost enablement. Per-runbook model selection. Per-runbook turn budgets. Daemon-wide concurrency caps. Every dial can be turned independently, and every dial can be turned back.

**It gets smarter over time.** Knowledge memories, incident records, and runbooks create an organizational knowledge base that persists across sessions, operators, and incidents. The AI doesn't start from zero each time -- it builds on everything it has learned.

---

## Technical Requirements

| Requirement | Detail |
|---|---|
| Platform | Linux only (uses `fork(2)`, Unix domain sockets, Linux-specific tmux hooks) |
| Runtime dependency | tmux 2.6+ |
| Build dependency | Rust 1.79+ |
| AI provider | At least one configured: Anthropic, OpenAI, Gemini, Ollama, or LM Studio |
| Installation | Single binary; `daemoneye setup` initializes config, systemd service, and tmux keybinding |

---

## Success Metrics

- **Time to resolve**: Reduction in mean time to resolution for incidents handled with DaemonEye assistance vs. manual response.
- **Autonomy progression**: Percentage of runbooks that have graduated to `enabled: true` (ghost-capable) over time.
- **Knowledge coverage**: Number of knowledge memories and runbooks created, and their hit rate in contextual auto-search.
- **Redaction effectiveness**: Per-type redaction counts as a proxy for sensitive data exposure prevention.
- **Operator confidence**: Qualitative measure of willingness to grant higher trust levels, tracked through approval scope expansion over time.
