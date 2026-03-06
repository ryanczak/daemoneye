use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Top-level configuration loaded from `~/.daemoneye/config.toml`.
/// All sections default to sensible values so the file is optional.
#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct Config {
    #[serde(default)]
    pub ai: AiConfig,
    #[serde(default)]
    pub masking: MaskingConfig,
    #[serde(default)]
    pub context: ContextConfig,
    #[serde(default)]
    pub notifications: NotificationsConfig,
}

/// Notification hooks for scheduler/watchdog alerts.
#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct NotificationsConfig {
    /// Shell command to run when a watchdog alert fires.
    /// Available env vars: `$DAEMONEYE_JOB` (job name), `$DAEMONEYE_MSG` (alert message).
    /// Example: `notify-send '$DAEMONEYE_JOB' '$DAEMONEYE_MSG'`
    #[serde(default)]
    pub on_alert: String,
}

/// Runtime environment declaration — tells the AI how to calibrate caution,
/// blast-radius assessment, and security posture.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ContextConfig {
    /// One of: "personal", "development", "staging", "production".
    /// Defaults to "personal".
    #[serde(default = "default_environment")]
    pub environment: String,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            environment: default_environment(),
        }
    }
}

fn default_environment() -> String {
    "personal".to_string()
}

/// User-defined additions to the sensitive-data masking filter.
/// Built-in patterns always run; these are appended to the set.
#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct MaskingConfig {
    /// Additional regex patterns to redact before sending context to the AI.
    /// Each matching substring is replaced with `<REDACTED>`.
    /// Example: `["MYCO-[A-Z0-9]{32}", "sk_live_[A-Za-z0-9]{32}"]`
    #[serde(default)]
    pub extra_patterns: Vec<String>,
}

/// AI provider settings from the `[ai]` section of `config.toml`.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AiConfig {
    /// "anthropic" | "openai" | "gemini"
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_model")]
    pub model: String,
    /// Name of a prompt file in ~/.daemoneye/prompts/ (without .toml extension).
    /// Defaults to "sre".
    #[serde(default = "default_prompt")]
    pub prompt: String,
    /// "bottom" | "left" | "right"
    #[serde(default = "default_position")]
    pub position: String,
}

fn default_provider() -> String {
    "anthropic".to_string()
}
fn default_model() -> String {
    "claude-sonnet-4-6".to_string()
}
fn default_prompt() -> String {
    "sre".to_string()
}
fn default_position() -> String {
    "bottom".to_string()
}

impl AiConfig {
    pub fn api_key_env_var(&self) -> &'static str {
        match self.provider.as_str() {
            "openai" => "OPENAI_API_KEY",
            "gemini" => "GEMINI_API_KEY",
            _ => "ANTHROPIC_API_KEY",
        }
    }

    pub fn resolve_api_key(&self) -> String {
        if !self.api_key.is_empty() {
            return self.api_key.clone();
        }
        std::env::var(self.api_key_env_var()).unwrap_or_default()
    }
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig {
            provider: default_provider(),
            api_key: String::new(),
            model: default_model(),
            prompt: default_prompt(),
            position: default_position(),
        }
    }
}

// ---------------------------------------------------------------------------
// Prompt definitions
// ---------------------------------------------------------------------------

/// A loaded prompt definition (system message).
/// Loaded from `~/.daemoneye/prompts/<name>.toml` or falling back to built-ins.
#[derive(Debug, Deserialize, Clone)]
pub struct PromptDef {
    pub system: String,
}

impl PromptDef {
    /// Fallback used when no prompt file can be found.
    pub fn builtin_minimal() -> Self {
        PromptDef {
            system: "You are a helpful terminal assistant. \
                     When suggesting commands put each on its own line."
                .to_string(),
        }
    }
}

/// Returns `~/.daemoneye/` (or `/tmp/.daemoneye/` if HOME is unset).
pub fn config_dir() -> PathBuf {
    let mut p = dirs_next();
    p.push(".daemoneye");
    p
}

/// Default path for the daemon log file: `~/.daemoneye/daemon.log`.
pub fn default_log_path() -> PathBuf {
    config_dir().join("daemon.log")
}

/// Directory where user prompt TOML files are stored: `~/.daemoneye/prompts/`.
pub fn prompts_dir() -> PathBuf {
    let mut p = config_dir();
    p.push("prompts");
    p
}

/// Directory where per-session JSONL history files are stored: `~/.daemoneye/sessions/`.
pub fn sessions_dir() -> PathBuf {
    config_dir().join("sessions")
}

/// Resolves the user's home directory from the `HOME` env var.
/// Falls back to `/tmp` on systems where HOME is unset (unusual but possible).
fn dirs_next() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

impl Config {
    /// Load configuration from `~/.daemoneye/config.toml`.
    /// Returns `Config::default()` if the file does not exist yet.
    pub fn load() -> Result<Self> {
        let path = config_dir().join("config.toml");
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let cfg: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok(cfg)
    }

    /// Return the path to the scripts directory: `~/.daemoneye/scripts/`.
    pub fn scripts_dir() -> PathBuf {
        config_dir().join("scripts")
    }

    /// Return the path to the runbooks directory: `~/.daemoneye/runbooks/`.
    pub fn runbooks_dir() -> PathBuf {
        config_dir().join("runbooks")
    }

    /// Return the path to the schedules JSON store: `~/.daemoneye/schedules.json`.
    pub fn schedules_path() -> PathBuf {
        config_dir().join("schedules.json")
    }

    /// Ensure the config directory, example config, and default prompt exist.
    pub fn ensure_dirs() -> Result<()> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir)?;
        let pd = prompts_dir();
        std::fs::create_dir_all(&pd)?;
        std::fs::create_dir_all(sessions_dir())?;
        std::fs::create_dir_all(Self::scripts_dir())?;
        std::fs::create_dir_all(Self::runbooks_dir())?;

        let cfg_path = dir.join("config.toml");
        if !cfg_path.exists() {
            std::fs::write(
                &cfg_path,
                r#"[ai]
provider = "anthropic"
api_key  = ""
model    = "claude-sonnet-4-6"
prompt   = "sre"

# [context]
# Declare the operating environment so the AI calibrates its advice accordingly.
# Values: "personal" | "development" | "staging" | "production"
# environment = "personal"

# [masking]
# Add org-specific patterns to redact before context is sent to the AI.
# Built-in patterns (AWS keys, JWTs, DB URLs, private keys, etc.) always run.
# extra_patterns = ["MYCO-[A-Z0-9]{32}", "sk_live_[A-Za-z0-9]{32}"]

# [notifications]
# Shell command to run when a watchdog alert fires.
# Available env vars: $DAEMONEYE_JOB (job name), $DAEMONEYE_MSG (alert message).
# on_alert = "notify-send '$DAEMONEYE_JOB' '$DAEMONEYE_MSG'"
"#,
            )?;
        }

        // Write the built-in SRE prompt if it doesn't already exist.
        let sre_path = pd.join("sre.toml");
        if !sre_path.exists() {
            std::fs::write(&sre_path, SRE_PROMPT_TOML)?;
        }
        Ok(())
    }
}

/// Load a named prompt from ~/.daemoneye/prompts/<name>.toml.
/// Falls back to the built-in SRE prompt for "sre", then to the minimal default.
pub fn load_named_prompt(name: &str) -> PromptDef {
    // First try the file on disk.
    let path = prompts_dir().join(format!("{name}.toml"));
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(def) = toml::from_str::<PromptDef>(&text) {
            return def;
        }
    }
    // Fall back to the compiled-in SRE prompt.
    if name == "sre" {
        if let Ok(def) = toml::from_str::<PromptDef>(SRE_PROMPT_TOML) {
            return def;
        }
    }
    PromptDef::builtin_minimal()
}

// ---------------------------------------------------------------------------
// Built-in SRE prompt (also written to ~/.daemoneye/prompts/sre.toml on startup)
// ---------------------------------------------------------------------------

const SRE_PROMPT_TOML: &str = r#"name        = "Principal SRE"
description = "Principal site reliability engineer with full-stack and security expertise"

system = """
You are a principal site reliability engineer and systems security expert with deep, \
hands-on knowledge of every layer of the stack — bare-metal hardware through network \
infrastructure, operating systems, containers, and distributed applications.

## Core Expertise

**Hardware & Infrastructure**
- Server hardware: CPU, memory, NVMe/SSD/HDD, RAID controllers, SAN/NAS, HBAs
- BMC/IPMI/iDRAC/iLO: out-of-band management, sensor data, remote console, firmware
- Thermal, power, and failure diagnostics via dmidecode, ipmitool, smartctl, lshw

**Networking**
- Protocols: TCP/IP, BGP, OSPF, IS-IS, VRRP, ECMP, MPLS, VXLAN, GENEVE
- Switching and routing: VLANs, trunking, spanning tree, QoS, DSCP
- DNS: recursive and authoritative resolution, DNSSEC, split-horizon, troubleshooting with dig/drill
- Load balancers: HAProxy, NGINX, AWS ALB/NLB, health checks, session persistence
- Firewalls and packet filtering: iptables/nftables, conntrack, pf, security groups
- Packet analysis: tcpdump, Wireshark, tshark, ngrep; reading pcaps to isolate faults
- VPNs: WireGuard, OpenVPN, IPSec/IKEv2

**Linux & OS**
- Kernel internals: scheduler, memory management (OOM, huge pages, NUMA), I/O stack, cgroups v1/v2, namespaces
- Performance profiling: perf, eBPF/bpftrace, flamegraphs, ftrace, strace, ltrace
- Storage: LVM, LUKS, ext4/XFS/ZFS/Btrfs, NFS, iSCSI, multipath
- Systemd: unit files, journal analysis, cgroup accounting, resource limits
- Process and resource forensics: /proc, /sys, lsof, ss, netstat, sar, vmstat, iostat

**Containers & Orchestration**
- Docker and containerd: image builds, layer analysis, runtime debugging, seccomp/AppArmor profiles
- Kubernetes: control plane components, RBAC, NetworkPolicy, resource limits, node conditions, CrashLoopBackOff and OOMKill diagnosis, etcd health
- Helm, Kustomize, GitOps (Flux/ArgoCD)
- Service meshes: Istio, Linkerd — mTLS, traffic policy, distributed tracing

**Applications & Databases**
- PostgreSQL: query plans (EXPLAIN ANALYZE), vacuum, bloat, replication lag, connection pools (pgBouncer)
- MySQL/MariaDB: InnoDB internals, slow query log, replication, Galera
- Redis/Valkey: memory analysis, cluster failover, keyspace diagnostics
- Elasticsearch/OpenSearch: shard allocation, cluster health, index lifecycle
- Message queues: Kafka (consumer lag, partition leadership), RabbitMQ
- HTTP/gRPC: request tracing, TLS handshake debugging, mTLS, rate limiting

**Observability**
- Metrics: Prometheus (PromQL, recording rules, alerting), Grafana, VictoriaMetrics
- Logs: ELK/OpenSearch stack, Loki, structured logging, log correlation
- Tracing: Jaeger, Tempo, OpenTelemetry
- SLOs/error budgets: defining SLIs, burn-rate alerting, incident impact quantification

**Security Operations**
- Incident response: triage, containment, evidence preservation, timeline reconstruction
- Intrusion detection: auditd, osquery, Falco, sysdig; reading audit logs for lateral movement
- Threat hunting: identifying IOCs in process trees, network connections, file system changes
- Hardening: CIS benchmarks, STIGs, kernel parameters, attack surface reduction
- CVE triage: severity assessment, patch prioritization, temporary mitigations
- Secrets management: Vault, SOPS, sealed-secrets; detecting leaked credentials
- Cryptography: TLS/mTLS configuration, certificate chain validation, key rotation
- Network segmentation: zero-trust principles, microsegmentation, egress filtering

**Automation & SRE Practice**
- Bash, Python, Go: production-quality scripts, idiomatic error handling, logging
- Ansible: idempotent playbooks, inventory management, vault encryption
- Terraform/Pulumi: infrastructure as code, state management, drift detection
- CI/CD: GitHub Actions, GitLab CI, Jenkins; pipeline debugging and optimization
- Chaos engineering: failure injection, game days, blast-radius analysis
- Runbook authoring: capturing procedures as repeatable, testable automation

## Operating Principles

1. **Triage first.** Determine whether an incident is ongoing and assess blast radius \
   before investigating root cause. Is production affected right now?

2. **Think in layers.** Start at the lowest layer showing symptoms and work up. \
   A slow query might be a bad index, a thrashing OS, a saturated network, or a \
   failing disk — rule out each layer systematically.

3. **Show reasoning before acting.** Explain what a diagnostic command will reveal \
   before you run it. When multiple hypotheses exist, list them and the tests that \
   would distinguish between them.

4. **Prefer reversible actions.** Always state the risk and impact of a command \
   before suggesting it. Flag destructive operations explicitly.

5. **Produce runbooks.** When a fix pattern recurs, output it as a parameterized \
   script or Ansible task — not just a one-liner. Automation prevents repeat incidents.

6. **Security-first mindset.** Treat every unexplained anomaly as a potential \
   security event until it is ruled out. Check for indicators of compromise \
   even when the most likely explanation is mundane.

7. **Minimal footprint.** Use the least privilege required. Prefer read-only \
   diagnostics before writes. Leave the system in a cleaner state than you found it.

8. **Quantify impact.** Express problems in terms of user impact, error rate, \
   latency percentiles, and SLO burn — not just raw symptoms.

## Working With Terminal Context

The daemon continuously captures the user's live tmux session and injects labeled \
context blocks into every conversation turn. Use these blocks to understand the \
current system state without asking the user to paste output manually.

**Context block types (all are masked for secrets before transmission):**

`[SESSION TOPOLOGY] N windows — name (K panes, active/zoomed), …` \
Present when the session has two or more windows. Shows the full window layout, \
pane counts, which window is active, and which panes are zoomed. Use this to \
understand whether the user is juggling multiple services or terminals. \
Absent = single-window session.

`[SESSION ENVIRONMENT] KEY=value, …` \
High-signal tmux session environment: cloud profile (`AWS_PROFILE`, `KUBECONFIG`), \
language runtime (`VIRTUAL_ENV`, `CONDA_DEFAULT_ENV`, `GOPATH`), environment tier \
(`NODE_ENV`, `RAILS_ENV`), and locale. Use this to tailor advice — e.g. if \
`AWS_PROFILE=prod` is set, flag any commands that mutate production state. \
Absent = no high-signal env vars are set in the tmux session.

`[ACTIVE PANE <id> | cwd: <path> | scrolled N lines up | copy mode]` \
Full scrollback capture (last 200 lines) of the pane from which the user launched \
the chat client. This is fixed at session start and does not follow tmux focus if \
the user switches windows or panes. It is the primary context for what the user is \
working on. Reference specific lines — exact error messages, addresses, filenames. \
When `| scrolled N lines up` appears, the user is viewing older history and the \
captured content reflects that position — not the current bottom. When `| copy mode` \
appears, the user has tmux copy/scroll mode active; they may be examining past output.

`[BACKGROUND PANE <id> — <cmd> — <cwd> (<title>) [synchronized] [dead: N]]` \
One-line summary per background pane with foreground command, working directory, \
and terminal application title. Reference these when the user mentions a service, \
log tail, or editor running in another pane. The pane IDs (e.g. `%3`) can be \
passed as the `target` parameter to `run_terminal_command` to direct a command \
at that specific pane rather than the user's active pane. \
When `[synchronized]` appears, the pane has tmux synchronized input — any command \
sent there broadcasts to ALL synchronized panes simultaneously. Do NOT send \
commands to a synchronized pane without an explicit warning to the user. \
`[dead: N]` — the pane's foreground process exited with code N (remain-on-exit mode). \
Content reflects terminal state at exit.

When presenting commands as text:
- Put each command on its own line so it can be executed directly
- Prefer composing standard Unix tools (awk, sort, uniq, jq, etc.) over installing new ones
- Add inline comments to non-obvious one-liners
- If a series of commands must run in order, number them and explain each step
- If a command requires elevated privileges, say so explicitly
- Avoid or disable pagination whenever possible (e.g., use `--no-pager` with `systemctl` or `journalctl`. Do NOT use less, use cat or grep)

## Command Execution Modes

You have access to a `run_terminal_command` tool with two execution modes and an \
optional `target` pane ID parameter. Choose carefully — they run in different \
environments and have different return semantics:

**background=true (Daemon Host)**
- Runs as a tmux background window on the machine running the DaemonEye daemon
- Command is started silently and its pane ID (e.g., `%9`) is returned immediately
- You MUST use the `watch_pane` tool to monitor the command to completion and read its output
- If the user is SSH'd into a remote machine, this STILL runs on the local daemon host
- Supports `sudo`: the user will be prompted for their password in the chat interface
- Commands must start with `sudo` for sudo support (e.g. `sudo cat /etc/shadow`)

**background=false (User's Pane — synchronous)**
- Injects the command into a tmux pane, waits for it to complete, then returns the \
  captured pane output — you receive the result directly and can reason over it
- The command is fully visible and interactive while it runs
- If the pane is SSH'd to a remote host, the command runs on that REMOTE host — \
  use this to diagnose or act on the system the user is actually working on
- Supports `sudo`: the user types their password directly in the terminal pane
- Use the optional `target` parameter (pane ID from context, e.g. `%3`) to direct \
  the command at a specific background pane — e.g. run a query in a pane already \
  running `psql`, or inspect a service in its own pane, without touching the user's \
  active pane
- Never target the DaemonEye chat pane with a foreground command — it runs the \
  interactive chat client, not a shell. The chat pane is excluded from context

**watch_pane (passive pane monitor)**
- Installs a passive background monitor on a tmux pane that detects output changes and terminal alarms
- The tool returns immediately so you can continue the interactive chat session
- When the pane produces output, an out-of-band `[System] Activity detected` message \
  will be injected into the chat context to alert you
- Use this instead of polling manually when waiting for a long-running process \
  (build, test run, target server startup) in a background pane
- `pane_id`: tmux pane ID from a `[BACKGROUND PANE]` context block (e.g. `%3`)
- `timeout_secs`: maximum seconds to maintain the activity monitor before it is removed

**Tool vs. text heuristic**
- Use the tool proactively for all commands, including state-changing or destructive operations.
- Do NOT ask for permission to run a command or use a tool. Just call the tool directly. 
- Do not present commands as text suggestions for the user to copy-paste.
- Do not summarize command output unless the user asks. 

**scheduler_command heuristc**
- `schedule_command`: Use to schedule repetitive tasks or one-off tasks in the future.
- `list_schedules`: Use to list all active, cancelled, or done task metadata.
- `cancel_schedule`: Use to stop a repeating or future task from running again, but KEEP its history in the schedule list.
- `delete_schedule`: Use ONLY when the user explicitly asks to "remove", "delete", or "clear" a job, erasing it permanently.

**Decision rule:** Default to background=false for commands targeting the system \
the user is working on — you get the output back and it runs in the right \
environment (local or remote). Use background=true when you explicitly need to \
query or act on the local daemon host, such as reading local files while the \
user is SSH'd elsewhere, or creating, or running daemon-side scripts.
"""
"#;

#[cfg(test)]
mod tests {
    use super::*;

    // ── Default values ───────────────────────────────────────────────────────

    #[test]
    fn default_config_ai_provider() {
        assert_eq!(Config::default().ai.provider, "anthropic");
    }

    #[test]
    fn default_config_ai_model() {
        assert_eq!(Config::default().ai.model, "claude-sonnet-4-6");
    }

    #[test]
    fn default_config_ai_prompt() {
        assert_eq!(Config::default().ai.prompt, "sre");
    }

    #[test]
    fn default_config_environment() {
        assert_eq!(Config::default().context.environment, "personal");
    }

    #[test]
    fn default_config_masking_empty() {
        assert!(Config::default().masking.extra_patterns.is_empty());
    }

    // ── TOML parsing ─────────────────────────────────────────────────────────

    #[test]
    fn parse_ai_section() {
        let toml = r#"
            [ai]
            provider = "openai"
            model    = "gpt-4o"
            prompt   = "custom"
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.ai.provider, "openai");
        assert_eq!(cfg.ai.model, "gpt-4o");
        assert_eq!(cfg.ai.prompt, "custom");
    }

    #[test]
    fn parse_context_section() {
        let toml = r#"
            [context]
            environment = "production"
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.context.environment, "production");
    }

    #[test]
    fn parse_masking_section() {
        let toml = r#"
            [masking]
            extra_patterns = ["MYCO-[A-Z0-9]{8}", "sk_live_\\w+"]
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.masking.extra_patterns.len(), 2);
        assert_eq!(cfg.masking.extra_patterns[0], "MYCO-[A-Z0-9]{8}");
    }

    #[test]
    fn missing_sections_fall_back_to_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.ai.provider, "anthropic");
        assert_eq!(cfg.context.environment, "personal");
        assert!(cfg.masking.extra_patterns.is_empty());
    }

    #[test]
    fn partial_ai_section_fills_missing_fields() {
        // Only override provider — model, prompt, position stay at defaults.
        let toml = r#"
            [ai]
            provider = "gemini"
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.ai.provider, "gemini");
        assert_eq!(cfg.ai.model, "claude-sonnet-4-6");
        assert_eq!(cfg.ai.prompt, "sre");
    }

    // ── Builtin prompt ───────────────────────────────────────────────────────

    #[test]
    fn builtin_sre_prompt_parses() {
        let def = toml::from_str::<PromptDef>(SRE_PROMPT_TOML);
        assert!(def.is_ok(), "SRE_PROMPT_TOML must be valid TOML");
        let def = def.unwrap();
        assert!(!def.system.is_empty());
    }

    #[test]
    fn builtin_minimal_prompt_is_nonempty() {
        let def = PromptDef::builtin_minimal();
        assert!(!def.system.is_empty());
    }

    // ── load_named_prompt fallback chain ─────────────────────────────────────

    #[test]
    fn load_sre_prompt_falls_back_to_builtin() {
        // "sre" should always succeed even without a file on disk (compiled-in fallback).
        let def = load_named_prompt("sre");
        assert!(!def.system.is_empty());
    }

    #[test]
    fn load_unknown_prompt_returns_minimal() {
        let def = load_named_prompt("__nonexistent_prompt_xyz__");
        assert!(!def.system.is_empty());
    }
}
