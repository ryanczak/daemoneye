use anyhow::{Context, Result};

use super::load::*;
use super::types::Config;

impl Config {
    /// Ensure the config directory tree and default files exist.
    pub fn ensure_dirs() -> Result<()> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir)?;
        // FHS-inspired subtree
        std::fs::create_dir_all(etc_dir())?;
        std::fs::create_dir_all(var_run_dir())?;
        std::fs::create_dir_all(var_log_dir())?;
        std::fs::create_dir_all(pipe_log_dir())?;
        std::fs::create_dir_all(pane_logs_dir())?;
        std::fs::create_dir_all(var_index_dir())?;
        std::fs::create_dir_all(bin_dir())?;
        // Daemon-managed persistent data
        let pd = prompts_dir();
        std::fs::create_dir_all(&pd)?;
        std::fs::create_dir_all(sessions_dir())?;
        // User-managed top-level directories
        std::fs::create_dir_all(scripts_dir())?;
        std::fs::create_dir_all(runbooks_dir())?;

        let cfg_path = etc_dir().join("config.toml");
        if !cfg_path.exists() {
            std::fs::write(&cfg_path, include_str!("../../assets/etc/config.toml"))?;
        }

        // Write the built-in SRE prompt if it doesn't already exist.
        let sre_path = pd.join("sre.toml");
        if !sre_path.exists() {
            std::fs::write(&sre_path, SRE_PROMPT_TOML)?;
        }

        // Seed built-in knowledge memories if they don't already exist.
        // User edits are preserved — we only write on first run.
        seed_knowledge_memory("webhook-setup", WEBHOOK_SETUP_MEMORY)?;
        seed_knowledge_memory("runbook-format", RUNBOOK_FORMAT_MEMORY)?;
        seed_knowledge_memory("runbook-ghost-template", RUNBOOK_GHOST_TEMPLATE_MEMORY)?;
        seed_knowledge_memory("ghost-shell-guide", GHOST_SHELL_GUIDE_MEMORY)?;
        seed_knowledge_memory("scheduling-guide", SCHEDULING_GUIDE_MEMORY)?;
        seed_knowledge_memory("scripts-and-sudoers", SCRIPTS_AND_SUDOERS_MEMORY)?;
        seed_knowledge_memory("agent-runtime-layout", AGENT_RUNTIME_LAYOUT_MEMORY)?;
        seed_knowledge_memory("tmux-pane-toolkit", TMUX_PANE_TOOLKIT_MEMORY)?;

        // Seed built-in session memories if they don't already exist.
        seed_session_memory(
            "pane-referencing-convention",
            PANE_REFERENCING_CONVENTION_MEMORY,
        )?;
        seed_session_memory("unicode-decoration-pref", UNICODE_DECORATION_PREF_MEMORY)?;

        // Seed example named agents if they don't already exist.
        seed_agent("architect", AGENT_ARCHITECT)?;
        seed_agent("researcher", AGENT_RESEARCHER)?;
        seed_agent("sysadmin", AGENT_SYSADMIN)?;

        Ok(())
    }
}

/// Write a knowledge memory file only if it does not already exist.
fn seed_knowledge_memory(key: &str, content: &str) -> Result<()> {
    seed_memory_inner("knowledge", key, content, false)
}

/// Write a session memory file only if it does not already exist.
fn seed_session_memory(key: &str, content: &str) -> Result<()> {
    seed_memory_inner("session", key, content, false)
}

/// Write a memory file into the given subdirectory, optionally overwriting.
fn seed_memory_inner(subdir: &str, key: &str, content: &str, force: bool) -> Result<()> {
    let dir = config_dir().join("memory").join(subdir);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.md", key));
    if force || !path.exists() {
        std::fs::write(&path, content)
            .with_context(|| format!("seeding {} memory '{}'", subdir, key))?;
    }
    Ok(())
}

/// Write a named agent config only if it does not already exist.
///
/// The agent directory is created if absent. An existing config is never overwritten
/// so user edits are preserved across upgrades.
pub fn seed_agent(name: &str, content: &str) -> Result<()> {
    let dir = crate::agents::agent_dir(name);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating agent dir for '{}'", name))?;
    let path = dir.join("config.toml");
    if !path.exists() {
        std::fs::write(&path, content)
            .with_context(|| format!("seeding agent config '{}'", name))?;
    }
    Ok(())
}

/// Re-seed all built-in memory files (knowledge + session), overwriting existing ones.
/// Called by `daemoneye setup --overwrite-memory`.
pub fn overwrite_knowledge_memories() -> Result<()> {
    seed_memory_inner("knowledge", "webhook-setup", WEBHOOK_SETUP_MEMORY, true)?;
    seed_memory_inner("knowledge", "runbook-format", RUNBOOK_FORMAT_MEMORY, true)?;
    seed_memory_inner(
        "knowledge",
        "runbook-ghost-template",
        RUNBOOK_GHOST_TEMPLATE_MEMORY,
        true,
    )?;
    seed_memory_inner(
        "knowledge",
        "ghost-shell-guide",
        GHOST_SHELL_GUIDE_MEMORY,
        true,
    )?;
    seed_memory_inner(
        "knowledge",
        "scheduling-guide",
        SCHEDULING_GUIDE_MEMORY,
        true,
    )?;
    seed_memory_inner(
        "knowledge",
        "scripts-and-sudoers",
        SCRIPTS_AND_SUDOERS_MEMORY,
        true,
    )?;
    seed_memory_inner(
        "knowledge",
        "agent-runtime-layout",
        AGENT_RUNTIME_LAYOUT_MEMORY,
        true,
    )?;
    seed_memory_inner(
        "knowledge",
        "tmux-pane-toolkit",
        TMUX_PANE_TOOLKIT_MEMORY,
        true,
    )?;
    seed_memory_inner(
        "session",
        "pane-referencing-convention",
        PANE_REFERENCING_CONVENTION_MEMORY,
        true,
    )?;
    seed_memory_inner(
        "session",
        "unicode-decoration-pref",
        UNICODE_DECORATION_PREF_MEMORY,
        true,
    )?;
    Ok(())
}

/// Overwrite the built-in SRE prompt regardless of whether it already exists.
/// Called by `daemoneye setup --overwrite-all`.
pub fn overwrite_sre_prompt() -> Result<()> {
    let sre_path = prompts_dir().join("sre.toml");
    std::fs::write(&sre_path, SRE_PROMPT_TOML)
        .with_context(|| format!("overwriting SRE prompt at {}", sre_path.display()))
}

// ---------------------------------------------------------------------------
// Built-in SRE prompt (also written to ~/.daemoneye/etc/prompts/sre.toml on startup)
// ---------------------------------------------------------------------------

pub(crate) const SRE_PROMPT_TOML: &str = include_str!("../../assets/prompts/sre.toml");

// ---------------------------------------------------------------------------
// Seeded knowledge memories (written to ~/.daemoneye/memory/knowledge/ on first run)
// ---------------------------------------------------------------------------

pub(crate) const WEBHOOK_SETUP_MEMORY: &str =
    include_str!("../../assets/memory/knowledge/webhook-setup.md");
pub(crate) const RUNBOOK_FORMAT_MEMORY: &str =
    include_str!("../../assets/memory/knowledge/runbook-format.md");
pub(crate) const RUNBOOK_GHOST_TEMPLATE_MEMORY: &str =
    include_str!("../../assets/memory/knowledge/runbook-ghost-template.md");
pub(crate) const GHOST_SHELL_GUIDE_MEMORY: &str =
    include_str!("../../assets/memory/knowledge/ghost-shell-guide.md");
pub(crate) const SCHEDULING_GUIDE_MEMORY: &str =
    include_str!("../../assets/memory/knowledge/scheduling-guide.md");
pub(crate) const SCRIPTS_AND_SUDOERS_MEMORY: &str =
    include_str!("../../assets/memory/knowledge/scripts-and-sudoers.md");
pub(crate) const AGENT_RUNTIME_LAYOUT_MEMORY: &str =
    include_str!("../../assets/memory/knowledge/agent-runtime-layout.md");
pub(crate) const TMUX_PANE_TOOLKIT_MEMORY: &str =
    include_str!("../../assets/memory/knowledge/tmux-pane-toolkit.md");

// ---------------------------------------------------------------------------
// Seeded session memories (written to ~/.daemoneye/memory/session/ on first run)
// ---------------------------------------------------------------------------

const PANE_REFERENCING_CONVENTION_MEMORY: &str =
    include_str!("../../assets/memory/session/pane-referencing-convention.md");
const UNICODE_DECORATION_PREF_MEMORY: &str =
    include_str!("../../assets/memory/session/unicode-decoration-pref.md");

// ---------------------------------------------------------------------------
// Seeded named agents (written to ~/.daemoneye/agents/<name>/config.toml on first run)
// ---------------------------------------------------------------------------

const AGENT_ARCHITECT: &str = include_str!("../../assets/agents/architect/config.toml");
const AGENT_RESEARCHER: &str = include_str!("../../assets/agents/researcher/config.toml");
const AGENT_SYSADMIN: &str = include_str!("../../assets/agents/sysadmin/config.toml");
