use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::ai::Message;

/// One artifact produced during a named chat session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub kind: String, // "memory", "runbook", or "script"
    pub name: String,
    pub at_turn: usize,
}

/// TOML-serialized metadata stored in `var/sessions/<name>/meta.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSessionMeta {
    pub schema_version: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// RFC 3339 timestamp when the session was first saved.
    pub created_at: String,
    /// RFC 3339 timestamp when the session was most recently loaded or saved.
    pub last_resumed_at: String,
    pub turn_count: usize,
    pub message_count: usize,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub artifacts_created: Vec<ArtifactRef>,
}

/// Lightweight entry in `var/sessions/index.json` — one per named session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub created_at: String,
    pub last_updated: String,
}

// ── Path helpers ──────────────────────────────────────────────────────────────

/// `~/.daemoneye/var/sessions/` — named saved sessions.
/// Distinct from `config::sessions_dir()` (`var/log/sessions/`), which holds ephemeral JSONL logs.
pub fn saved_sessions_dir() -> PathBuf {
    crate::config::config_dir().join("var/sessions")
}

fn session_dir(name: &str) -> PathBuf {
    saved_sessions_dir().join(name)
}

fn meta_path(name: &str) -> PathBuf {
    session_dir(name).join("meta.toml")
}

fn messages_path(name: &str) -> PathBuf {
    session_dir(name).join("messages.jsonl")
}

fn index_path() -> PathBuf {
    saved_sessions_dir().join("index.json")
}

// ── Name validation ───────────────────────────────────────────────────────────

const RESERVED_NAMES: &[&str] = &["default", "current", "new", "active", "none", "all"];

/// Validate a session name.  Returns `Err(human_message)` with a suggestion on failure.
///
/// Rules: `^[a-z0-9][a-z0-9-]{0,63}$`, reserved names blocked, `auto-*` prefix reserved.
pub fn validate_session_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("session name must not be empty".to_string());
    }
    if name.len() > 64 {
        return Err(format!(
            "'{}' is too long ({} chars, max 64)",
            name,
            name.len()
        ));
    }
    if RESERVED_NAMES.contains(&name) {
        return Err(format!(
            "'{}' is a reserved name; choose a more specific name like 'my-investigation'",
            name
        ));
    }
    if name.starts_with("auto-") {
        let trimmed = name.trim_start_matches("auto-");
        return Err(format!(
            "names starting with 'auto-' are reserved; try '{}'",
            trimmed
        ));
    }
    let valid = name.chars().enumerate().all(|(i, c)| {
        if i == 0 {
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
        return Err(format!(
            "invalid name '{}': use lowercase letters, digits, and hyphens{}",
            name, suggestion
        ));
    }
    Ok(())
}

// ── Index helpers ─────────────────────────────────────────────────────────────

fn load_index() -> HashMap<String, IndexEntry> {
    let path = index_path();
    if !path.exists() {
        return HashMap::new();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_index(index: &HashMap<String, IndexEntry>) -> Result<()> {
    let path = index_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(index)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("renaming {} → {}", tmp.display(), path.display()))?;
    Ok(())
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Save a session to `var/sessions/<name>/`.
///
/// If `name` is already in the index AND `current_saved_name != Some(name)`, returns an error
/// unless `force` is true.  Use `force` only when the user explicitly passes `--force`.
pub fn save_session(
    name: &str,
    current_saved_name: Option<&str>,
    description: &str,
    messages: &[Message],
    turn_count: usize,
    model: &str,
    artifacts: &[ArtifactRef],
    force: bool,
) -> Result<()> {
    validate_session_name(name).map_err(|e| anyhow::anyhow!(e))?;

    let mut index = load_index();
    let is_update = current_saved_name == Some(name);
    if index.contains_key(name) && !is_update && !force {
        bail!(
            "a session named '{}' already exists; use --force to overwrite, \
             or choose a different name",
            name
        );
    }

    let dir = session_dir(name);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating session directory {}", dir.display()))?;

    let now = chrono::Utc::now().to_rfc3339();
    let created_at = index
        .get(name)
        .map(|e| e.created_at.clone())
        .unwrap_or_else(|| now.clone());

    let meta = SavedSessionMeta {
        schema_version: 1,
        name: name.to_string(),
        description: description.to_string(),
        tags: Vec::new(),
        parent: None,
        created_at: created_at.clone(),
        last_resumed_at: now.clone(),
        turn_count,
        message_count: messages.len(),
        model: model.to_string(),
        artifacts_created: artifacts.to_vec(),
    };

    let meta_str = toml::to_string_pretty(&meta).context("serializing session meta")?;
    let tmp_meta = meta_path(name).with_extension("toml.tmp");
    std::fs::write(&tmp_meta, &meta_str)
        .with_context(|| format!("writing {}", tmp_meta.display()))?;
    std::fs::rename(&tmp_meta, meta_path(name))?;

    // Write messages JSONL atomically.
    use std::io::Write;
    let msg_path = messages_path(name);
    let tmp_msg = msg_path.with_extension("jsonl.tmp");
    let mut f = std::fs::File::create(&tmp_msg)
        .with_context(|| format!("creating {}", tmp_msg.display()))?;
    for msg in messages {
        if let Ok(line) = serde_json::to_string(msg) {
            writeln!(f, "{}", line)?;
        }
    }
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp_msg, &msg_path)?;

    index.insert(
        name.to_string(),
        IndexEntry {
            created_at,
            last_updated: now,
        },
    );
    save_index(&index)?;
    Ok(())
}

/// Load the metadata for a named session.
pub fn load_session_meta(name: &str) -> Result<SavedSessionMeta> {
    let path = meta_path(name);
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Load the last `max_count` messages from a named session's JSONL file.
/// Pass `max_count = 0` to load all messages.
pub fn load_session_messages(name: &str, max_count: usize) -> Result<Vec<Message>> {
    let path = messages_path(name);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut messages: Vec<Message> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if max_count > 0 && messages.len() > max_count {
        messages.drain(..messages.len() - max_count);
    }
    Ok(messages)
}

/// Return all saved sessions sorted by `last_updated` descending (most recent first).
pub fn list_sessions() -> Vec<(String, IndexEntry)> {
    let mut entries: Vec<(String, IndexEntry)> = load_index().into_iter().collect();
    entries.sort_by(|a, b| b.1.last_updated.cmp(&a.1.last_updated));
    entries
}

/// Return true if a named session exists in the index.
pub fn session_exists(name: &str) -> bool {
    load_index().contains_key(name)
}

/// Delete a named session from disk and from the index.
pub fn delete_session(name: &str) -> Result<()> {
    let mut index = load_index();
    if !index.contains_key(name) {
        bail!("no saved session named '{}'", name);
    }
    let dir = session_dir(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
    }
    index.remove(name);
    save_index(&index)?;
    Ok(())
}

/// Rename a saved session.  Updates the directory and the index.
/// Artifact `session_origin` frontmatter rewrite is deferred to Phase 3.
pub fn rename_session(old_name: &str, new_name: &str) -> Result<()> {
    validate_session_name(new_name).map_err(|e| anyhow::anyhow!(e))?;
    let mut index = load_index();
    if !index.contains_key(old_name) {
        bail!("no saved session named '{}'", old_name);
    }
    if index.contains_key(new_name) {
        bail!("a session named '{}' already exists", new_name);
    }

    // Update the name field inside meta.toml first (artifact writes before index flip).
    if let Ok(mut meta) = load_session_meta(old_name) {
        meta.name = new_name.to_string();
        if let Ok(s) = toml::to_string_pretty(&meta) {
            let tmp = meta_path(old_name).with_extension("toml.tmp");
            let _ =
                std::fs::write(&tmp, &s).and_then(|_| std::fs::rename(&tmp, meta_path(old_name)));
        }
    }

    let old_dir = session_dir(old_name);
    let new_dir = session_dir(new_name);
    std::fs::rename(&old_dir, &new_dir)
        .with_context(|| format!("renaming {} → {}", old_dir.display(), new_dir.display()))?;

    // Atomic index flip — this is the commit point.
    let entry = index.remove(old_name).unwrap();
    index.insert(new_name.to_string(), entry);
    save_index(&index)?;
    Ok(())
}

/// Stamp `session_origin: "<name>"` on each artifact listed in `artifacts`.
///
/// Called on the first save of a previously-unnamed session.  Skips artifacts
/// that already have `session_origin`.  Logs warnings but never fails fatally —
/// returns names of artifacts that could not be patched.
pub fn backfill_session_origin(artifacts: &[ArtifactRef], name: &str) -> Vec<String> {
    let mut failed = Vec::new();
    for artifact in artifacts {
        if let Err(e) = stamp_artifact_origin(&artifact.kind, &artifact.name, name) {
            log::warn!(
                "backfill_session_origin: {}/{}: {}",
                artifact.kind,
                artifact.name,
                e
            );
            failed.push(format!("{}/{}", artifact.kind, artifact.name));
        }
    }
    failed
}

fn stamp_artifact_origin(kind: &str, artifact_name: &str, session_name: &str) -> Result<()> {
    let base = crate::config::config_dir();
    match kind {
        "memory" => {
            for dir_name in &["knowledge", "session", "incident"] {
                let path = base
                    .join("memory")
                    .join(dir_name)
                    .join(format!("{}.md", artifact_name));
                if path.exists() {
                    let content = std::fs::read_to_string(&path)?;
                    let stamped = crate::header::inject_yaml_session_origin(&content, session_name);
                    if stamped == content {
                        return Ok(()); // already stamped
                    }
                    let tmp = path.with_extension("md.tmp");
                    std::fs::write(&tmp, &stamped)?;
                    std::fs::rename(&tmp, &path)?;
                    return Ok(());
                }
            }
            bail!("memory '{}' not found in any category", artifact_name)
        }
        "runbook" => {
            let path = base.join("runbooks").join(format!("{}.md", artifact_name));
            if path.exists() {
                let content = std::fs::read_to_string(&path)?;
                let stamped = crate::header::inject_yaml_session_origin(&content, session_name);
                if stamped != content {
                    let tmp = path.with_extension("md.tmp");
                    std::fs::write(&tmp, &stamped)?;
                    std::fs::rename(&tmp, &path)?;
                }
            }
            Ok(())
        }
        "script" => {
            let path = base.join("scripts").join(artifact_name);
            if path.exists() {
                let content = std::fs::read_to_string(&path)?;
                let stamped = crate::header::inject_comment_session_origin(&content, session_name);
                if stamped != content {
                    let tmp = path.with_extension("tmp");
                    std::fs::write(&tmp, &stamped)?;
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o700))?;
                    std::fs::rename(&tmp, &path)?;
                }
            }
            Ok(())
        }
        other => bail!("unknown artifact kind '{}'", other),
    }
}

/// Build the banner text shown to the user (and injected as AI context) when a session is loaded.
pub fn build_resumed_banner(meta: &SavedSessionMeta, loaded_count: usize) -> String {
    let age_secs = chrono::DateTime::parse_from_rfc3339(&meta.last_resumed_at)
        .ok()
        .map(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds())
        .unwrap_or(0);
    let age_str = humanize_age(age_secs);

    let artifact_note = if meta.artifacts_created.is_empty() {
        String::new()
    } else {
        format!(" | {} artifact(s) created", meta.artifacts_created.len())
    };
    let desc_line = if meta.description.is_empty() {
        String::new()
    } else {
        format!("\n  {}", meta.description)
    };
    let stale_warning = if age_secs > 7 * 24 * 3600 {
        "\n  ⚠  Last active >7 days ago — verify file paths, hostnames, and PR numbers before acting."
    } else {
        ""
    };

    format!(
        "[Session Resumed — \"{}\" | {} | {} turns{} | {} message(s) loaded]{}{}",
        meta.name, age_str, meta.turn_count, artifact_note, loaded_count, desc_line, stale_warning,
    )
}

fn humanize_age(secs: i64) -> String {
    if secs < 0 {
        return "just now".to_string();
    }
    let secs = secs as u64;
    if secs < 120 {
        return "just now".to_string();
    }
    if secs < 3600 {
        return format!("{}m ago", secs / 60);
    }
    let hours = secs / 3600;
    if hours < 48 {
        return format!("{}h ago", hours);
    }
    format!("{}d ago", hours / 24)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "session_store_tests.rs"]
mod tests;
