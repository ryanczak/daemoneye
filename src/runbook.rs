use anyhow::{Context, Result, bail};
use std::path::PathBuf;

/// Configuration for autonomous Ghost Sessions triggered by a runbook.
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct GhostConfig {
    /// Whether the AI can operate autonomously in a Ghost Session.
    pub enabled: bool,
    /// List of script names (in `~/.daemoneye/scripts/`) pre-approved for execution.
    pub auto_approve_scripts: Vec<String>,
    /// Whether to auto-approve known read-only informational commands.
    pub auto_approve_read_only: bool,
}

/// A runbook loaded from `~/.daemoneye/runbooks/<name>.md`.
///
/// Runbooks provide context for watchdog AI analysis and are managed
/// by AI tools (`write_runbook`, `read_runbook`, `list_runbooks`, `delete_runbook`).
#[derive(Debug, Clone)]
pub struct Runbook {
    pub name: String,
    /// Full markdown body (everything after the YAML frontmatter block).
    pub content: String,
    /// Tags parsed from frontmatter `tags: [a, b]`.
    /// Exposed for future use (e.g. filtering by tag); currently carried through
    /// `RunbookInfo` from `list_runbooks()`.
    #[allow(dead_code)]
    pub tags: Vec<String>,
    /// Knowledge memory keys to load when this runbook runs as a watchdog.
    pub memories: Vec<String>,
    /// Settings for autonomous response (Ghost Session).
    pub ghost_config: GhostConfig,
}

/// Metadata returned by `list_runbooks()`.
#[derive(Debug, Clone)]
pub struct RunbookInfo {
    pub name: String,
    pub tags: Vec<String>,
    pub ghost_config: GhostConfig,
}

/// Return the path to the runbooks directory: `~/.daemoneye/runbooks/`.
pub fn runbooks_dir() -> PathBuf {
    crate::config::config_dir().join("runbooks")
}

fn ensure_runbooks_dir() -> Result<()> {
    let dir = runbooks_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating runbooks dir {}", dir.display()))
}

fn validate_runbook_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("Runbook name cannot be empty");
    }
    if name.contains('/') || name.contains('\0') || name == "." || name == ".." {
        bail!("Invalid runbook name: '{}'", name);
    }
    Ok(())
}

fn validate_runbook_content(content: &str) -> Result<()> {
    if !content.contains("# Runbook:") {
        bail!("Missing '# Runbook:' heading in runbook content");
    }
    if !content.contains("## Alert Criteria") {
        bail!("Missing '## Alert Criteria' section in runbook content");
    }
    Ok(())
}

/// Parse YAML-like frontmatter from markdown.
///
/// If the content starts with `---\n`, finds the closing `\n---\n` and
/// extracts `tags:`, `memories:`, and `ghost_mode:` fields.  Returns
/// `(tags, memories, ghost_config, body_after_frontmatter)`.
fn parse_frontmatter(raw: &str) -> (Vec<String>, Vec<String>, GhostConfig, String) {
    if !raw.starts_with("---\n") {
        return (Vec::new(), Vec::new(), GhostConfig::default(), raw.to_string());
    }
    // Find closing delimiter
    let search_from = 4; // after "---\n"
    let end_marker = "\n---\n";
    if let Some(rel_pos) = raw[search_from..].find(end_marker) {
        let fm_end = search_from + rel_pos;
        let frontmatter = &raw[search_from..fm_end];
        let body = raw[fm_end + end_marker.len()..].to_string();

        let tags = parse_list_field(frontmatter, "tags");
        let memories = parse_list_field(frontmatter, "memories");
        
        // Manual parsing for ghost_mode fields. Supports flat keys for now.
        let enabled = parse_bool_field(frontmatter, "enabled");
        let auto_approve_scripts = parse_list_field(frontmatter, "auto_approve_scripts");
        let auto_approve_read_only = parse_bool_field(frontmatter, "auto_approve_read_only");

        let ghost_config = GhostConfig {
            enabled,
            auto_approve_scripts,
            auto_approve_read_only,
        };

        (tags, memories, ghost_config, body)
    } else {
        (Vec::new(), Vec::new(), GhostConfig::default(), raw.to_string())
    }
}

/// Parse a field of the form `key: true` or `key: false` from frontmatter text.
fn parse_bool_field(frontmatter: &str, key: &str) -> bool {
    let prefix = format!("{}:", key);
    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(&prefix) {
            let rest = trimmed[prefix.len()..].trim().to_lowercase();
            return rest == "true";
        }
    }
    false
}

/// Parse a field of the form `key: [item1, item2, item3]` from frontmatter text.
fn parse_list_field(frontmatter: &str, key: &str) -> Vec<String> {
    let prefix = format!("{}:", key);
    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(&prefix) {
            let rest = trimmed[prefix.len()..].trim();
            // Expect "[item1, item2]"
            if let Some(inner) = rest.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                return inner
                    .split(',')
                    .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
        }
    }
    Vec::new()
}

/// Load a named runbook from `~/.daemoneye/runbooks/<name>.md`.
pub fn load_runbook(name: &str) -> Result<Runbook> {
    validate_runbook_name(name)?;
    let path = runbooks_dir().join(format!("{}.md", name));
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading runbook at {}", path.display()))?;
    let (tags, memories, ghost_config, content) = parse_frontmatter(&raw);
    crate::daemon::stats::inc_runbooks_executed();
    Ok(Runbook {
        name: name.to_string(),
        content,
        tags,
        memories,
        ghost_config,
    })
}

/// Write (create or update) a runbook at `~/.daemoneye/runbooks/<name>.md`.
pub fn write_runbook(name: &str, content: &str) -> Result<()> {
    validate_runbook_name(name)?;
    validate_runbook_content(content)?;
    ensure_runbooks_dir()?;
    let path = runbooks_dir().join(format!("{}.md", name));
    std::fs::write(&path, content)
        .with_context(|| format!("writing runbook at {}", path.display()))?;
    crate::daemon::stats::inc_runbooks_created();
    Ok(())
}

/// Delete a runbook. No-op if the file does not exist.
pub fn delete_runbook(name: &str) -> Result<()> {
    validate_runbook_name(name)?;
    let path = runbooks_dir().join(format!("{}.md", name));
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("deleting runbook at {}", path.display()))?;
        crate::daemon::stats::inc_runbooks_deleted();
    }
    Ok(())
}

/// List all runbooks in `~/.daemoneye/runbooks/`, sorted by name.
pub fn list_runbooks() -> Result<Vec<RunbookInfo>> {
    let dir = runbooks_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<RunbookInfo> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            if !path.is_file() {
                return None;
            }
            let ext = path.extension()?.to_str()?;
            if ext != "md" {
                return None;
            }
            let stem = path.file_stem()?.to_string_lossy().to_string();
            // Best-effort tag extraction — ignore errors
            let (tags, ghost_config) = if let Ok(raw) = std::fs::read_to_string(&path) {
                let (t, _, gc, _) = parse_frontmatter(&raw);
                (t, gc)
            } else {
                (Vec::new(), GhostConfig::default())
            };
            Some(RunbookInfo { name: stem, tags, ghost_config })
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// Build the watchdog system prompt, loading any referenced knowledge memories.
pub fn watchdog_system_prompt(runbook: &Runbook) -> String {
    let memory_context = if runbook.memories.is_empty() {
        String::new()
    } else {
        let mut parts = vec!["## Runbook Memory Context".to_string()];
        for key in &runbook.memories {
            if let Ok(val) =
                crate::memory::read_memory(key, crate::memory::MemoryCategory::Knowledge)
            {
                parts.push(format!("### {}\n{}", key, val));
            }
        }
        if parts.len() > 1 {
            format!("{}\n\n", parts.join("\n\n"))
        } else {
            String::new()
        }
    };

    format!(
        "You are an automated watchdog monitor. Analyze the command output below and \
         determine if it indicates an alert condition.\n\n\
         ## Runbook: {}\n\n{}\n\n\
         {}Respond with ALERT if any alert condition is met, followed by a brief explanation. \
         Respond with OK if everything looks normal.",
        runbook.name,
        runbook.content.trim(),
        memory_context,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::UnpoisonExt;
    use std::env;

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    struct TmpHome(std::path::PathBuf);
    impl TmpHome {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("de_rb_test_{}_{}", std::process::id(), n));
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
        let _guard = crate::TEST_HOME_LOCK.lock().unwrap_or_log();
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

    const SAMPLE_RUNBOOK: &str = r#"---
tags: [disk, storage]
memories: [disk_thresholds]
---
# Runbook: disk-check

## Purpose
Monitor disk usage.

## Alert Criteria
- Usage above 90%

## Remediation Steps
1. Check large files with `du -sh /*`
2. Remove old logs

## Notes
Last updated: 2026-03-01
"#;

    #[test]
    fn watchdog_prompt_contains_runbook_name() {
        let rb = Runbook {
            name: "disk-check".to_string(),
            content: "# Runbook: disk-check\n\n## Alert Criteria\n- usage > 90%\n\nNormal disk usage is below 80%".to_string(),
            tags: vec!["disk".to_string()],
            memories: vec![],
        };
        let prompt = watchdog_system_prompt(&rb);
        assert!(prompt.contains("disk-check"));
        assert!(prompt.contains("80%"));
        assert!(prompt.contains("ALERT"));
    }

    #[test]
    fn load_runbook_parses_frontmatter() {
        let tmp = temp_home();
        with_home(&tmp, || {
            ensure_runbooks_dir().unwrap();
            let path = runbooks_dir().join("disk-check.md");
            std::fs::write(&path, SAMPLE_RUNBOOK).unwrap();
            let rb = load_runbook("disk-check").unwrap();
            assert_eq!(rb.name, "disk-check");
            assert!(rb.tags.contains(&"disk".to_string()));
            assert!(rb.tags.contains(&"storage".to_string()));
            assert!(rb.memories.contains(&"disk_thresholds".to_string()));
            assert!(rb.content.contains("# Runbook: disk-check"));
        });
    }

    #[test]
    fn write_runbook_validates_format() {
        let tmp = temp_home();
        with_home(&tmp, || {
            // Missing # Runbook: heading
            let bad = "## Alert Criteria\nsome stuff\n";
            assert!(write_runbook("bad", bad).is_err());

            // Missing ## Alert Criteria
            let bad2 = "# Runbook: foo\nsome stuff\n";
            assert!(write_runbook("bad2", bad2).is_err());

            // Valid
            let good = "# Runbook: good\n\n## Alert Criteria\n- always\n";
            assert!(write_runbook("good", good).is_ok());
        });
    }

    #[test]
    fn list_runbooks_returns_sorted() {
        let tmp = temp_home();
        with_home(&tmp, || {
            let content = "# Runbook: x\n\n## Alert Criteria\n- x\n";
            write_runbook("charlie", content).unwrap();
            write_runbook("alpha", content).unwrap();
            write_runbook("bravo", content).unwrap();
            let list = list_runbooks().unwrap();
            let names: Vec<_> = list.iter().map(|r| r.name.as_str()).collect();
            assert_eq!(names, vec!["alpha", "bravo", "charlie"]);
        });
    }
}
