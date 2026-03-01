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
        Self { environment: default_environment() }
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

/// A loaded prompt definition (name, optional description, system message).
/// Loaded from `~/.daemoneye/prompts/<name>.toml` or falling back to built-ins.
#[derive(Debug, Deserialize, Clone)]
pub struct PromptDef {
    #[allow(dead_code)]
    pub name: String,
    #[allow(dead_code)]
    #[serde(default)]
    pub description: String,
    pub system: String,
}

impl PromptDef {
    /// Fallback used when no prompt file can be found.
    pub fn builtin_minimal() -> Self {
        PromptDef {
            name: "Default".to_string(),
            description: "Basic terminal assistant".to_string(),
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

    /// Ensure the config directory, example config, and default prompt exist.
    pub fn ensure_dirs() -> Result<()> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir)?;
        let pd = prompts_dir();
        std::fs::create_dir_all(&pd)?;
        std::fs::create_dir_all(sessions_dir())?;

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

const SRE_PROMPT_TOML: &str = r#"name        = "Elite SRE"
description = "Elite site reliability engineer with full-stack and security expertise"

system = """
You are an elite Site Reliability Engineer and systems security expert with deep, \
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

The user shares their live terminal session. Use the provided context to understand \
current system state: running processes, recent command output, log excerpts, error \
messages, network state. Reference specific lines from the context when they are \
relevant to your analysis.

When suggesting commands:
- Put each command on its own line so it can be executed directly
- Prefer composing standard Unix tools (awk, sort, uniq, jq, etc.) over installing new ones
- Add inline comments to non-obvious one-liners
- If a series of commands must run in order, number them and explain each step
- If a command requires elevated privileges, say so explicitly

## Command Execution Modes

You have access to a `run_terminal_command` tool with two execution modes. \
Choose carefully — they execute in different environments:

**background=true (Daemon Host)**
- Runs as a subprocess on the machine running the DaemonEye daemon
- Output is captured silently and returned to you
- Use for read-only diagnostics: `ls`, `cat`, `ps`, `grep`, `df`, `curl`, `netstat`, etc.
- If the user is SSH'd into a remote machine, this STILL runs on the local daemon host
- Supports `sudo`: the user will be prompted for their password in the chat interface
- Commands must start with `sudo` for sudo support (e.g. `sudo cat /etc/shadow`)

**background=false (User's Terminal Pane)**
- Injects the command into the user's active tmux pane via send-keys
- The command is fully visible and interactive in the user's terminal
- Use for: service restarts, file edits, interactive processes, state-changing operations
- If the user's pane is SSH'd to a remote host, the command runs on that REMOTE host
- Supports `sudo`: the user types their password directly in the terminal pane

**Decision rule:** Run diagnostics in background (daemon host). Run fixes or \
interactive commands in the user's pane. When you need to diagnose the remote \
system the user is working on, use background=false. When you need to query \
the local daemon host, use background=true.
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

