use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub mod mailbox;
pub mod policy;
pub use policy::ToolPolicy;

/// Configuration for a named agent executor identity.
///
/// Agents are stored at `~/.daemoneye/agents/<name>/config.toml`.
/// An agent defines *who* executes a ghost shell — the role, model,
/// memory namespace, and trust boundaries — separate from *what* the
/// runbook asks it to do.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Unique slug identifying this agent (kebab-case, validated).
    pub name: String,
    /// Short human-readable description shown in listings.
    #[serde(default)]
    pub description: String,
    /// Role-defining system prompt addition layered on top of (or replacing)
    /// the default SRE prompt when this agent runs.
    #[serde(default)]
    pub prompt: String,
    /// Model key from `[models.*]` in config.toml. None = use daemon default.
    #[serde(default)]
    pub model: Option<String>,
    /// Memory namespace for this agent's scoped memories.
    /// Defaults to the agent name if empty.
    #[serde(default)]
    pub memory_namespace: String,
    /// Per-invocation turn budget. None = daemon default.
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// Allow the agent to run non-sudo commands without listing them in
    /// `auto_approve_scripts`. Non-sudo commands are already permitted by
    /// OS permissions; this flag makes that explicit in the ghost prompt.
    #[serde(default)]
    pub auto_approve_read_only: bool,
    /// Script names in `~/.daemoneye/scripts/` pre-approved for sudo execution
    /// when this agent runs. Each must also have a NOPASSWD sudoers rule.
    #[serde(default)]
    pub auto_approve_scripts: Vec<String>,
    /// Additional namespaces this agent can read from (in addition to its own
    /// namespace and "global"). Used for cross-agent knowledge sharing.
    #[serde(default)]
    pub read_namespaces: Vec<String>,
    /// Tool access policy. If `None`, all tools are permitted.
    /// `allow` and `deny` lists are mutually exclusive.
    #[serde(default)]
    pub tools: Option<ToolPolicy>,
}

/// Lightweight info returned by `list_agents()`.
#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub name: String,
    pub description: String,
    pub model: Option<String>,
}

/// `~/.daemoneye/agents/` — root directory for agent configs.
pub fn agents_dir() -> PathBuf {
    crate::config::config_dir().join("agents")
}

/// `~/.daemoneye/agents/<name>/` — directory for a specific agent.
pub fn agent_dir(name: &str) -> PathBuf {
    agents_dir().join(name)
}

/// Path to the config TOML for a specific agent.
pub fn config_path(name: &str) -> PathBuf {
    agent_dir(name).join("config.toml")
}

/// Path to the briefing markdown file for a specific agent.
///
/// Briefings are written after each clean ghost shell exit and injected into
/// the next invocation as prior context.
pub fn briefing_path(name: &str) -> PathBuf {
    agent_dir(name).join("briefing.md")
}

/// Validate an agent name slug.
///
/// Rules: `[a-z0-9-]+`, 1–48 chars, no leading/trailing dash.
/// Mirrors `validate_session_name()` but shorter max length.
pub fn validate_agent_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("agent name must not be empty");
    }
    if name.len() > 48 {
        bail!(
            "agent name '{}' is too long ({} chars, max 48)",
            name,
            name.len()
        );
    }
    let valid = name.chars().enumerate().all(|(i, c)| {
        if i == 0 || i == name.len() - 1 {
            c.is_ascii_lowercase() || c.is_ascii_digit()
        } else {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
        }
    });
    if !valid {
        let suggested: String = name
            .to_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .to_string();
        let suggestion = if !suggested.is_empty()
            && suggested
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric())
        {
            format!("; try '{}'", suggested)
        } else {
            String::new()
        };
        bail!(
            "invalid agent name '{}': use lowercase letters, digits, and hyphens (no leading/trailing dash){}",
            name,
            suggestion
        );
    }
    Ok(())
}

/// Ensure the agents directory exists.
pub fn ensure_agents_dir() -> Result<()> {
    let dir = agents_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating agents dir {}", dir.display()))
}

/// Save an agent config to `~/.daemoneye/agents/<name>/config.toml`.
///
/// Creates the directory tree if it does not exist. Overwrites any existing
/// config for the same name.
pub fn save_agent(config: &AgentConfig) -> Result<()> {
    validate_agent_name(&config.name)?;
    ensure_agents_dir()?;
    let dir = agent_dir(&config.name);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating agent dir {}", dir.display()))?;
    let path = config_path(&config.name);
    let text = toml::to_string_pretty(config).context("serializing agent config")?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, &text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("renaming {} → {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Load an agent config by name.
///
/// Normalizes `memory_namespace` to the agent name when the field is empty
/// (i.e. was omitted from the TOML file), so callers never see an empty namespace.
pub fn load_agent(name: &str) -> Result<AgentConfig> {
    validate_agent_name(name)?;
    let path = config_path(name);
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut config: AgentConfig =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    if config.memory_namespace.is_empty() {
        config.memory_namespace = config.name.clone();
    }
    Ok(config)
}

/// Delete an agent config and its directory. No-op if the agent does not exist.
pub fn delete_agent(name: &str) -> Result<()> {
    validate_agent_name(name)?;
    let dir = agent_dir(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("removing agent dir {}", dir.display()))?;
    }
    Ok(())
}

/// Apply agent config defaults to a `GhostConfig`.
///
/// Runbook wins for every field that is already explicitly set. Agent values
/// are applied only when the runbook left a field at its zero/empty default.
/// Sets `ghost_config.agent` to the agent name unconditionally for audit purposes.
pub fn apply_agent_to_ghost_config(
    agent: &AgentConfig,
    ghost_config: &mut crate::ipc::GhostConfig,
) {
    // model: runbook wins (Some vs None)
    if ghost_config.model.is_none() {
        ghost_config.model = agent.model.clone();
    }
    // max_ghost_turns: 0 means "not set by runbook" — apply agent value
    if ghost_config.max_ghost_turns == 0
        && let Some(t) = agent.max_turns
    {
        ghost_config.max_ghost_turns = t as usize;
    }
    // auto_approve_commands: false is the runbook default; apply agent if it is more permissive
    if !ghost_config.auto_approve_commands {
        ghost_config.auto_approve_commands = agent.auto_approve_read_only;
    }
    // auto_approve_scripts: union — add agent scripts not already listed
    for s in &agent.auto_approve_scripts {
        if !ghost_config.auto_approve_scripts.contains(s) {
            ghost_config.auto_approve_scripts.push(s.clone());
        }
    }
    // tool_policy: applied only when the runbook left it unset
    if ghost_config.tool_policy.is_none() {
        ghost_config.tool_policy = agent.tools.clone();
    }
    ghost_config.agent = Some(agent.name.clone());
}

/// Build a merged `GhostConfig` for a runbook, applying the agent referenced in
/// `ghost_config.agent` (i.e. the runbook frontmatter `agent:` field) if present.
///
/// This is the single source of truth for agent merging. All ghost-shell spawn
/// paths (AI tool, webhook, scheduler) must go through this function.
pub fn merge_runbook_ghost_config(rb: &crate::runbook::Runbook) -> crate::ipc::GhostConfig {
    merge_runbook_ghost_config_from(rb, rb.ghost_config.clone())
}

/// Like `merge_runbook_ghost_config` but starts from a caller-supplied base config
/// instead of cloning the runbook's own config. Used by the AI `spawn_ghost_shell`
/// tool when the agent name comes from the tool parameter rather than the frontmatter.
pub fn merge_runbook_ghost_config_from(
    rb: &crate::runbook::Runbook,
    mut config: crate::ipc::GhostConfig,
) -> crate::ipc::GhostConfig {
    if let Some(ref agent_name) = config.agent.clone() {
        match load_agent(agent_name) {
            Ok(agent) => {
                apply_agent_to_ghost_config(&agent, &mut config);
                log::info!(
                    "Ghost shell for runbook '{}' merged agent config '{}'",
                    rb.name,
                    agent_name
                );
            }
            Err(e) => {
                log::warn!(
                    "Runbook '{}' references agent '{}' which could not be loaded ({}); \
                     proceeding with runbook defaults",
                    rb.name,
                    agent_name,
                    e
                );
            }
        }
    }
    config
}

/// List all agent configs, sorted by name.
pub fn list_agents() -> Result<Vec<AgentInfo>> {
    let dir = agents_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<AgentInfo> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            if !path.is_dir() {
                return None;
            }
            let name = path.file_name()?.to_str()?.to_string();
            // Best-effort load — skip dirs with no valid config
            load_agent(&name).ok().map(|c| AgentInfo {
                name: c.name,
                description: c.description,
                model: c.model,
            })
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    struct TmpHome(std::path::PathBuf);
    impl TmpHome {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p =
                std::env::temp_dir().join(format!("de_agent_test_{}_{}", std::process::id(), n));
            std::fs::create_dir_all(&p).unwrap();
            TmpHome(p)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TmpHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_home() -> TmpHome {
        TmpHome::new()
    }

    fn with_home<F: FnOnce()>(tmp: &TmpHome, f: F) {
        let _guard = crate::test_home_guard();
        let old = env::var("HOME").ok();
        unsafe {
            env::set_var("HOME", tmp.path());
        }
        f();
        match old {
            Some(v) => unsafe {
                env::set_var("HOME", v);
            },
            None => unsafe {
                env::remove_var("HOME");
            },
        }
    }

    fn sample_config(name: &str) -> AgentConfig {
        AgentConfig {
            name: name.to_string(),
            description: format!("Test agent: {}", name),
            prompt: "You are a test agent.".to_string(),
            model: Some("haiku".to_string()),
            memory_namespace: name.to_string(),
            max_turns: Some(10),
            auto_approve_read_only: true,
            auto_approve_scripts: vec!["check.sh".to_string()],
            read_namespaces: Vec::new(),
            tools: None,
        }
    }

    #[test]
    fn roundtrip() {
        let tmp = temp_home();
        with_home(&tmp, || {
            let cfg = sample_config("test-agent");
            save_agent(&cfg).unwrap();
            let loaded = load_agent("test-agent").unwrap();
            assert_eq!(loaded.name, "test-agent");
            assert_eq!(loaded.description, "Test agent: test-agent");
            assert_eq!(loaded.model, Some("haiku".to_string()));
            assert_eq!(loaded.memory_namespace, "test-agent");
            assert_eq!(loaded.max_turns, Some(10));
            assert!(loaded.auto_approve_read_only);
            assert_eq!(loaded.auto_approve_scripts, vec!["check.sh"]);
        });
    }

    #[test]
    fn name_validation_valid() {
        assert!(validate_agent_name("a").is_ok());
        assert!(validate_agent_name("disk-monitor").is_ok());
        assert!(validate_agent_name("postgres-dba-01").is_ok());
        assert!(validate_agent_name("a1b2c3").is_ok());
    }

    #[test]
    fn name_validation_invalid() {
        // Empty
        assert!(validate_agent_name("").is_err());
        // Too long
        assert!(validate_agent_name(&"a".repeat(49)).is_err());
        // Leading dash
        assert!(validate_agent_name("-foo").is_err());
        // Trailing dash
        assert!(validate_agent_name("foo-").is_err());
        // Uppercase
        assert!(validate_agent_name("Foo").is_err());
        // Underscore
        assert!(validate_agent_name("foo_bar").is_err());
        // Spaces
        assert!(validate_agent_name("foo bar").is_err());
    }

    #[test]
    fn list_empty() {
        let tmp = temp_home();
        with_home(&tmp, || {
            let list = list_agents().unwrap();
            assert!(list.is_empty());
        });
    }

    #[test]
    fn list_populated() {
        let tmp = temp_home();
        with_home(&tmp, || {
            save_agent(&sample_config("alpha")).unwrap();
            save_agent(&sample_config("bravo")).unwrap();
            let list = list_agents().unwrap();
            assert_eq!(list.len(), 2);
            assert_eq!(list[0].name, "alpha");
            assert_eq!(list[1].name, "bravo");
        });
    }

    #[test]
    fn delete_agent_removes_dir() {
        let tmp = temp_home();
        with_home(&tmp, || {
            save_agent(&sample_config("to-delete")).unwrap();
            assert!(load_agent("to-delete").is_ok());
            delete_agent("to-delete").unwrap();
            assert!(load_agent("to-delete").is_err());
        });
    }

    #[test]
    fn delete_nonexistent_is_noop() {
        let tmp = temp_home();
        with_home(&tmp, || {
            assert!(delete_agent("no-such-agent").is_ok());
        });
    }

    #[test]
    fn default_fields_deserialize() {
        // Minimal TOML should parse with defaults for optional fields.
        let toml = r#"
name = "minimal"
description = ""
prompt = ""
"#;
        let cfg: AgentConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.name, "minimal");
        assert_eq!(cfg.description, "");
        assert_eq!(cfg.prompt, "");
        assert!(cfg.model.is_none());
        // Raw deserialization yields empty string before load_agent normalization.
        assert_eq!(cfg.memory_namespace, "");
        assert!(cfg.max_turns.is_none());
        assert!(!cfg.auto_approve_read_only);
        assert!(cfg.auto_approve_scripts.is_empty());
    }

    #[test]
    fn load_agent_normalizes_empty_namespace() {
        let tmp = temp_home();
        with_home(&tmp, || {
            // Save with an explicit empty namespace to simulate omitting the field.
            let mut cfg = sample_config("ns-test");
            cfg.memory_namespace = String::new();
            save_agent(&cfg).unwrap();
            let loaded = load_agent("ns-test").unwrap();
            // load_agent must default memory_namespace to the agent name.
            assert_eq!(loaded.memory_namespace, "ns-test");
        });
    }

    #[test]
    fn apply_agent_merge_fills_defaults() {
        use crate::ipc::GhostConfig;
        let agent = AgentConfig {
            name: "analyst".to_string(),
            description: String::new(),
            prompt: String::new(),
            model: Some("haiku".to_string()),
            memory_namespace: "analyst".to_string(),
            max_turns: Some(5),
            auto_approve_read_only: true,
            auto_approve_scripts: vec!["check.sh".to_string()],
            read_namespaces: Vec::new(),
            tools: None,
        };
        let mut gc = GhostConfig::default();
        apply_agent_to_ghost_config(&agent, &mut gc);
        assert_eq!(gc.model, Some("haiku".to_string()));
        assert_eq!(gc.max_ghost_turns, 5);
        assert!(gc.auto_approve_commands);
        assert!(gc.auto_approve_scripts.contains(&"check.sh".to_string()));
        assert_eq!(gc.agent, Some("analyst".to_string()));
    }

    #[test]
    fn apply_agent_merge_respects_runbook_precedence() {
        use crate::ipc::GhostConfig;
        let agent = AgentConfig {
            name: "analyst".to_string(),
            description: String::new(),
            prompt: String::new(),
            model: Some("haiku".to_string()),
            memory_namespace: "analyst".to_string(),
            max_turns: Some(5),
            auto_approve_read_only: true,
            auto_approve_scripts: vec!["agent-script.sh".to_string()],
            read_namespaces: Vec::new(),
            tools: None,
        };
        // Runbook has already set these fields explicitly.
        let mut gc = GhostConfig {
            model: Some("opus".to_string()),
            max_ghost_turns: 30,
            auto_approve_commands: false, // runbook explicitly left this false
            auto_approve_scripts: vec!["runbook-script.sh".to_string()],
            ..Default::default()
        };
        apply_agent_to_ghost_config(&agent, &mut gc);
        // Runbook values must not be overwritten.
        assert_eq!(gc.model, Some("opus".to_string()));
        assert_eq!(gc.max_ghost_turns, 30);
        // auto_approve_commands: runbook false + agent true => agent wins
        // (the agent makes it MORE permissive; the guard is !ghost_config.auto_approve_commands)
        assert!(gc.auto_approve_commands);
        // Scripts from both runbook and agent are present (union).
        assert!(
            gc.auto_approve_scripts
                .contains(&"runbook-script.sh".to_string())
        );
        assert!(
            gc.auto_approve_scripts
                .contains(&"agent-script.sh".to_string())
        );
    }

    #[test]
    fn apply_agent_merge_no_script_duplicates() {
        use crate::ipc::GhostConfig;
        let agent = AgentConfig {
            name: "a".to_string(),
            description: String::new(),
            prompt: String::new(),
            model: None,
            memory_namespace: "a".to_string(),
            max_turns: None,
            auto_approve_read_only: false,
            auto_approve_scripts: vec!["shared.sh".to_string()],
            read_namespaces: Vec::new(),
            tools: None,
        };
        let mut gc = GhostConfig {
            auto_approve_scripts: vec!["shared.sh".to_string()],
            ..Default::default()
        };
        apply_agent_to_ghost_config(&agent, &mut gc);
        assert_eq!(gc.auto_approve_scripts, vec!["shared.sh"]);
    }
}
