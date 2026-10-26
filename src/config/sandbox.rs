//! Config sections: [sandbox] and [execution].

use super::*;
use serde::{Deserialize, Serialize};

/// Per-container resource ceilings (`[sandbox.limits]`).
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SandboxLimits {
    /// Container memory ceiling, in Docker's `--memory` syntax. Default: "1g".
    #[serde(default = "default_sandbox_limits_memory")]
    pub memory: String,
    /// Maximum process IDs inside the container (`--pids-limit`), which is what
    /// bounds a fork bomb. Default: 256.
    #[serde(default = "default_sandbox_limits_pids")]
    pub pids: u32,
    /// CPU quota (`--cpus`). Default: 2.0.
    #[serde(default = "default_sandbox_limits_cpus")]
    pub cpus: f64,
    /// Size of the `/de/work` scratch tmpfs. Default: "2g".
    #[serde(default = "default_sandbox_limits_scratch")]
    pub scratch: String,
}

fn default_sandbox_limits_memory() -> String {
    "1g".to_string()
}

fn default_sandbox_limits_pids() -> u32 {
    256
}

fn default_sandbox_limits_cpus() -> f64 {
    2.0
}

fn default_sandbox_limits_scratch() -> String {
    "2g".to_string()
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            memory: default_sandbox_limits_memory(),
            pids: default_sandbox_limits_pids(),
            cpus: default_sandbox_limits_cpus(),
            scratch: default_sandbox_limits_scratch(),
        }
    }
}

/// Per-profile overrides (`[sandbox.profile.<name>]`), keyed by agent/profile name.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SandboxProfile {
    /// "none" (default, no route out) or "proxy" (egress via the containerized
    /// proxy on a shared user-defined network).
    #[serde(default = "default_sandbox_profile_network")]
    pub network: String,
    /// Hostnames this profile may reach when `network = "proxy"`. Ignored for
    /// "none". Default: empty.
    #[serde(default)]
    pub proxy_allow: Vec<String>,
    /// Hostnames this profile may never reach, even when an allow rule would
    /// cover them. Deny always beats allow. Default: empty.
    #[serde(default)]
    pub proxy_deny: Vec<String>,
}

fn default_sandbox_profile_network() -> String {
    "none".to_string()
}

impl Default for SandboxProfile {
    fn default() -> Self {
        Self {
            network: default_sandbox_profile_network(),
            proxy_allow: Vec::new(),
            proxy_deny: Vec::new(),
        }
    }
}

/// Defaults applied to ghost-shell containers (`[sandbox.ghost_defaults]`).
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SandboxGhostDefaults {
    /// Destroy the container on ghost exit, on every path including failure.
    /// Default: true.
    #[serde(default = "default_true")]
    pub destroy_on_exit: bool,
    /// Mount mode for the staged script volume. Default: "ro".
    #[serde(default = "default_sandbox_ghost_mount_scripts")]
    pub mount_scripts: String,
}

fn default_sandbox_ghost_mount_scripts() -> String {
    "ro".to_string()
}

impl Default for SandboxGhostDefaults {
    fn default() -> Self {
        Self {
            destroy_on_exit: default_true(),
            mount_scripts: default_sandbox_ghost_mount_scripts(),
        }
    }
}

/// Container sandboxing for agent command execution (`[sandbox]`).
/// Nothing here takes effect while `enabled = false`; later M18 phases consume
/// these values. The defaults describe the safe, disabled state.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SandboxConfig {
    /// Master feature flag. Default: false — behaviour is unchanged until set.
    #[serde(default)]
    pub enabled: bool,
    /// Ignored: an enabled sandbox always runs on sbx. Kept so an old config
    /// still parses; a value other than "sbx" warns.
    #[serde(default = "default_sandbox_runtime")]
    pub runtime: String,
    /// Image the agent containers run. Default: "daemoneye-agent-base".
    #[serde(default = "default_sandbox_image")]
    pub image: String,
    /// Image the egress proxy container runs. Default: "daemoneye-egress-proxy".
    #[serde(default = "default_sandbox_proxy_image")]
    pub proxy_image: String,
    /// Working directory / scratch mount point inside the container.
    /// Default: "/de/work".
    #[serde(default = "default_sandbox_workdir")]
    pub workdir: String,
    /// `--user` value for every sandboxed process, as "uid:gid".
    /// Default: "1000:1000". Under rootless Docker container root maps to the
    /// daemon's own host uid, so running as root would return execution to the
    /// exact identity the sandbox exists to contain.
    #[serde(default = "default_sandbox_run_as")]
    pub run_as: String,
    /// Docker API endpoint. Default: "unix:///run/user/1000/docker.sock"
    /// (the rootless per-user socket, not the rootful /var/run one).
    #[serde(default = "default_sandbox_docker_host")]
    pub docker_host: String,
    #[serde(default)]
    pub limits: SandboxLimits,
    #[serde(default)]
    pub profile: std::collections::HashMap<String, SandboxProfile>,
    #[serde(default)]
    pub ghost_defaults: SandboxGhostDefaults,
}

fn default_sandbox_runtime() -> String {
    "sbx".to_string()
}

fn default_sandbox_image() -> String {
    "daemoneye-agent-base".to_string()
}

fn default_sandbox_proxy_image() -> String {
    "daemoneye-egress-proxy".to_string()
}

fn default_sandbox_workdir() -> String {
    "/de/work".to_string()
}

fn default_sandbox_run_as() -> String {
    "1000:1000".to_string()
}

fn default_sandbox_docker_host() -> String {
    "unix:///run/user/1000/docker.sock".to_string()
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            runtime: default_sandbox_runtime(),
            image: default_sandbox_image(),
            proxy_image: default_sandbox_proxy_image(),
            workdir: default_sandbox_workdir(),
            run_as: default_sandbox_run_as(),
            docker_host: default_sandbox_docker_host(),
            limits: SandboxLimits::default(),
            profile: std::collections::HashMap::new(),
            ghost_defaults: SandboxGhostDefaults::default(),
        }
    }
}

impl SandboxConfig {
    /// True when `run_as` would put the process at container uid 0 — which
    /// under rootless Docker is the daemon's own host uid. An empty value
    /// counts as root, because Docker's own default is root when `--user` is
    /// omitted.
    pub fn runs_as_container_root(&self) -> bool {
        let uid_field = self.run_as.split(':').next().unwrap_or("").trim();
        uid_field.is_empty() || uid_field == "0" || uid_field == "root"
    }

    /// Emit warnings for sandbox configuration that would silently defeat the
    /// sandbox. Call once at daemon startup after the config is loaded.
    pub fn validate(&self) {
        for w in self.warnings() {
            log::warn!("{w}");
        }
    }

    /// Every warning about this `[sandbox]` section, in a stable order. Pure:
    /// `validate()` is the caller that logs them.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.runtime != "sbx" {
            out.push(format!(
                "[sandbox] runtime = {:?} is ignored — the docker runtime was removed, and \
                 an enabled sandbox always runs on sbx",
                self.runtime
            ));
        }
        let docker_only: Vec<(&str, String)> = vec![
            ("image", self.image.clone()),
            ("proxy_image", self.proxy_image.clone()),
            ("workdir", self.workdir.clone()),
            ("run_as", self.run_as.clone()),
            ("docker_host", self.docker_host.clone()),
            ("limits.pids", self.limits.pids.to_string()),
            ("limits.scratch", self.limits.scratch.clone()),
            (
                "ghost_defaults.mount_scripts",
                self.ghost_defaults.mount_scripts.clone(),
            ),
        ];
        let defaults = SandboxConfig::default();
        let default_values: Vec<(&str, String)> = vec![
            ("image", defaults.image),
            ("proxy_image", defaults.proxy_image),
            ("workdir", defaults.workdir),
            ("run_as", defaults.run_as),
            ("docker_host", defaults.docker_host),
            ("limits.pids", defaults.limits.pids.to_string()),
            ("limits.scratch", defaults.limits.scratch),
            (
                "ghost_defaults.mount_scripts",
                defaults.ghost_defaults.mount_scripts,
            ),
        ];
        for ((key, value), (_, default)) in docker_only.into_iter().zip(default_values) {
            if value != default {
                out.push(format!(
                    "[sandbox] {key} = {value:?} has no effect — it applied only to the \
                     removed docker runtime"
                ));
            }
        }
        let mut names: Vec<&String> = self.profile.keys().collect();
        names.sort();
        for name in names {
            let profile = &self.profile[name];
            let network = profile.network.as_str();
            if network != "none" && network != "proxy" {
                out.push(format!(
                    "[sandbox] profile.{name} network = {network:?} is invalid — only \
                     \"none\" or \"proxy\" are supported"
                ));
            } else if network == "proxy" {
                out.push(format!(
                    "[sandbox] profile.{name} network = \"proxy\" inherits the operator's \
                     global sbx network policy; proxy_allow adds hosts to it and does not \
                     restrict it — use network = \"none\" for no egress"
                ));
            }
        }
        out
    }
}

/// Execution backend selection (`[execution]`).
/// `backend` defaults to `"pty"`, the only substrate; `"tmux"` was removed in
/// 2.0 and is accepted only so old configs load (`validate` warns).
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ExecutionConfig {
    /// `"pty"` (the only supported value).
    #[serde(default = "default_execution_backend")]
    pub backend: String,
}

fn default_execution_backend() -> String {
    "pty".to_string()
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            backend: default_execution_backend(),
        }
    }
}

impl ExecutionConfig {
    /// Warn about the retired tmux backend and any unrecognised value. Never
    /// panics and never returns a value.
    pub fn validate(&self) {
        let normalised = self.backend.trim().to_ascii_lowercase();
        if normalised == "tmux" {
            log::warn!(
                "[execution] backend = \"tmux\" was removed — commands run in PTY shells. \
                 Delete the `backend` line or set it to \"pty\"."
            );
        } else if normalised != "pty" {
            log::warn!(
                "[execution] backend = {:?} is invalid — supported value is \"pty\". \
                 Falling back to the default, \"pty\".",
                self.backend
            );
        }
    }
}
