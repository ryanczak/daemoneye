use anyhow::{Context, Result, bail};
use std::path::PathBuf;

pub enum MemoryCategory {
    Session,
    Knowledge,
    Incident,
}

impl MemoryCategory {
    /// Filesystem directory name under ~/.daemoneye/memory/.
    pub fn dir_name(&self) -> &'static str {
        match self {
            MemoryCategory::Session => "session",
            MemoryCategory::Knowledge => "knowledge",
            MemoryCategory::Incident => "incidents",
        }
    }

    /// The canonical name used in tool arguments and displayed to the AI.
    /// Always singular to match the tool description ('incident', not 'incidents').
    pub fn canonical_name(&self) -> &'static str {
        match self {
            MemoryCategory::Session => "session",
            MemoryCategory::Knowledge => "knowledge",
            MemoryCategory::Incident => "incident",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "session" => Some(MemoryCategory::Session),
            "knowledge" => Some(MemoryCategory::Knowledge),
            "incident" | "incidents" => Some(MemoryCategory::Incident),
            _ => None,
        }
    }
}

/// Memory entry with optional metadata parsed from frontmatter.
pub struct MemoryInfo {
    pub key: String,
    pub category: String,
    pub tags: Vec<String>,
    pub summary: Option<String>,
    pub relates_to: Vec<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub expires: Option<String>,
}

impl MemoryInfo {
    /// Returns true if `expires` is set and the date (YYYY-MM-DD) is strictly before today.
    pub fn is_expired(&self) -> bool {
        let Some(ref exp) = self.expires else {
            return false;
        };
        // Compare lexicographically — ISO date strings sort correctly as strings.
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        exp.trim() < today.as_str()
    }
}

/// Parsed frontmatter fields from a memory file.
struct ParsedFrontmatter {
    tags: Vec<String>,
    summary: Option<String>,
    relates_to: Vec<String>,
    created: Option<String>,
    updated: Option<String>,
    expires: Option<String>,
}

/// Parse optional YAML frontmatter from a memory file.
/// Returns `(ParsedFrontmatter, body)`. If no frontmatter, body is the full content and all
/// fields are empty/None.
fn parse_memory_frontmatter(raw: &str) -> (ParsedFrontmatter, String) {
    let empty = ParsedFrontmatter {
        tags: Vec::new(),
        summary: None,
        relates_to: Vec::new(),
        created: None,
        updated: None,
        expires: None,
    };
    if !raw.starts_with("---\n") {
        return (empty, raw.to_string());
    }
    let search_from = 4;
    let end_marker = "\n---\n";
    if let Some(rel_pos) = raw[search_from..].find(end_marker) {
        let fm_end = search_from + rel_pos;
        let frontmatter = &raw[search_from..fm_end];
        let body = raw[fm_end + end_marker.len()..].to_string();
        let parsed = parse_frontmatter_fields(frontmatter);
        (parsed, body)
    } else {
        (empty, raw.to_string())
    }
}

fn parse_frontmatter_fields(frontmatter: &str) -> ParsedFrontmatter {
    let mut tags = Vec::new();
    let mut summary = None;
    let mut relates_to = Vec::new();
    let mut created = None;
    let mut updated = None;
    let mut expires = None;

    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("tags:") {
            tags = parse_bracket_list(trimmed.strip_prefix("tags:").unwrap_or("").trim());
        } else if trimmed.starts_with("relates_to:") {
            relates_to =
                parse_bracket_list(trimmed.strip_prefix("relates_to:").unwrap_or("").trim());
        } else if trimmed.starts_with("summary:") {
            let val = trimmed.strip_prefix("summary:").unwrap_or("").trim();
            summary = Some(val.trim_matches('"').trim_matches('\'').to_string());
        } else if trimmed.starts_with("created:") {
            let val = trimmed.strip_prefix("created:").unwrap_or("").trim();
            created = Some(val.trim_matches('"').to_string());
        } else if trimmed.starts_with("updated:") {
            let val = trimmed.strip_prefix("updated:").unwrap_or("").trim();
            updated = Some(val.trim_matches('"').to_string());
        } else if trimmed.starts_with("expires:") {
            let val = trimmed.strip_prefix("expires:").unwrap_or("").trim();
            expires = Some(val.trim_matches('"').to_string());
        }
    }

    ParsedFrontmatter {
        tags,
        summary,
        relates_to,
        created,
        updated,
        expires,
    }
}

fn parse_bracket_list(s: &str) -> Vec<String> {
    if let Some(inner) = s.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        inner
            .split(',')
            .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        Vec::new()
    }
}

/// Serialize frontmatter fields back to a `---\n...\n---\n` block.
/// Only includes fields that have values. Omits the block entirely if all are empty/None.
pub fn build_frontmatter(
    tags: &[String],
    summary: Option<&str>,
    relates_to: &[String],
    created: Option<&str>,
    updated: Option<&str>,
    expires: Option<&str>,
) -> String {
    let mut lines: Vec<String> = Vec::new();
    if !tags.is_empty() {
        let items = tags
            .iter()
            .map(|t| format!("\"{}\"", t))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("tags: [{}]", items));
    }
    if let Some(s) = summary
        && !s.is_empty()
    {
        lines.push(format!("summary: \"{}\"", s.replace('"', "'")));
    }
    if !relates_to.is_empty() {
        let items = relates_to
            .iter()
            .map(|r| format!("\"{}\"", r))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("relates_to: [{}]", items));
    }
    if let Some(s) = created
        && !s.is_empty()
    {
        lines.push(format!("created: \"{}\"", s));
    }
    if let Some(s) = updated
        && !s.is_empty()
    {
        lines.push(format!("updated: \"{}\"", s));
    }
    if let Some(s) = expires
        && !s.is_empty()
    {
        lines.push(format!("expires: \"{}\"", s));
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("---\n{}\n---\n", lines.join("\n"))
    }
}

fn memory_dir(category: &MemoryCategory) -> PathBuf {
    crate::config::config_dir()
        .join("memory")
        .join(category.dir_name())
}

fn ensure_memory_dir(category: &MemoryCategory) -> Result<()> {
    let dir = memory_dir(category);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating memory dir {}", dir.display()))
}

fn validate_memory_key(key: &str) -> Result<()> {
    if key.is_empty() {
        bail!("Memory key cannot be empty");
    }
    if key.contains('/') || key.contains('\0') || key == "." || key == ".." {
        bail!("Invalid memory key: '{}'", key);
    }
    Ok(())
}

/// Update specific fields of an existing memory entry without rewriting the whole file.
/// Only provided (Some) fields are changed; omitted fields are preserved.
/// If the entry does not exist, a new one is created.
/// `updated` timestamp is always set to the current UTC time.
pub fn update_memory(
    key: &str,
    category: MemoryCategory,
    body: Option<&str>,
    append: bool,
    tags: Option<&[String]>,
    summary: Option<&str>,
    relates_to: Option<&[String]>,
    expires: Option<&str>,
) -> Result<()> {
    validate_memory_key(key)?;
    ensure_memory_dir(&category)?;
    let path = memory_dir(&category).join(format!("{}.md", key));

    // Read existing content (if any).
    let (mut fm, mut existing_body) = if path.exists() {
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading memory key '{}' for update", key))?;
        parse_memory_frontmatter(&raw)
    } else {
        (
            ParsedFrontmatter {
                tags: Vec::new(),
                summary: None,
                relates_to: Vec::new(),
                created: None,
                updated: None,
                expires: None,
            },
            String::new(),
        )
    };

    // Merge provided fields.
    if let Some(t) = tags {
        fm.tags = t.to_vec();
    }
    if let Some(s) = summary {
        fm.summary = Some(s.to_string());
    }
    if let Some(r) = relates_to {
        fm.relates_to = r.to_vec();
    }
    if let Some(e) = expires {
        fm.expires = Some(e.to_string());
    }

    // Set/update the `updated` timestamp.
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    if fm.created.is_none() {
        fm.created = Some(now.clone());
    }
    fm.updated = Some(now);

    // Update body if provided.
    if let Some(b) = body {
        if append && !existing_body.is_empty() {
            if !existing_body.ends_with('\n') {
                existing_body.push('\n');
            }
            existing_body.push_str(b);
        } else {
            existing_body = b.to_string();
        }
    }

    let frontmatter = build_frontmatter(
        &fm.tags,
        fm.summary.as_deref(),
        &fm.relates_to,
        fm.created.as_deref(),
        fm.updated.as_deref(),
        fm.expires.as_deref(),
    );
    let content = format!("{}{}", frontmatter, existing_body);
    std::fs::write(&path, &content)
        .with_context(|| format!("writing updated memory key '{}'", key))?;
    crate::daemon::stats::inc_memories_created();
    Ok(())
}

pub fn add_memory(key: &str, value: &str, category: MemoryCategory) -> Result<()> {
    validate_memory_key(key)?;
    ensure_memory_dir(&category)?;
    let path = memory_dir(&category).join(format!("{}.md", key));
    std::fs::write(&path, value).with_context(|| format!("writing memory key '{}'", key))?;
    crate::daemon::stats::inc_memories_created();
    Ok(())
}

pub fn delete_memory(key: &str, category: MemoryCategory) -> Result<()> {
    let path = memory_dir(&category).join(format!("{}.md", key));
    if path.exists() {
        std::fs::remove_file(&path)?;
        crate::daemon::stats::inc_memories_deleted();
    }
    Ok(())
}

pub fn read_memory(key: &str, category: MemoryCategory) -> Result<String> {
    let path = memory_dir(&category).join(format!("{}.md", key));
    let val = std::fs::read_to_string(&path)
        .with_context(|| format!("reading memory key '{}' from {}", key, path.display()))?;
    crate::daemon::stats::inc_memories_recalled();
    Ok(val)
}

pub fn list_memories(category: Option<MemoryCategory>) -> Result<Vec<(String, String)>> {
    let categories: Vec<MemoryCategory> = match category {
        Some(c) => vec![c],
        None => vec![
            MemoryCategory::Session,
            MemoryCategory::Knowledge,
            MemoryCategory::Incident,
        ],
    };
    let mut results = Vec::new();
    for cat in &categories {
        let dir = memory_dir(cat);
        if !dir.exists() {
            continue;
        }
        let mut entries: Vec<String> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let path = e.path();
                if !path.is_file() {
                    return None;
                }
                path.file_stem().map(|s| s.to_string_lossy().to_string())
            })
            .collect();
        entries.sort();
        for name in entries {
            results.push((cat.canonical_name().to_string(), name));
        }
    }
    Ok(results)
}

/// List memories with optional tags parsed from frontmatter.
/// Session memories are included when `category` is None or Some(Session).
pub fn list_memories_with_tags(category: Option<MemoryCategory>) -> Result<Vec<MemoryInfo>> {
    let categories: Vec<MemoryCategory> = match category {
        Some(c) => vec![c],
        None => vec![
            MemoryCategory::Session,
            MemoryCategory::Knowledge,
            MemoryCategory::Incident,
        ],
    };
    let mut results = Vec::new();
    for cat in &categories {
        let dir = memory_dir(cat);
        if !dir.exists() {
            continue;
        }
        let mut entries: Vec<String> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let path = e.path();
                if !path.is_file() {
                    return None;
                }
                path.file_stem().map(|s| s.to_string_lossy().to_string())
            })
            .collect();
        entries.sort();
        for name in entries {
            let path = dir.join(format!("{}.md", name));
            let info = if let Ok(raw) = std::fs::read_to_string(&path) {
                let (fm, _) = parse_memory_frontmatter(&raw);
                MemoryInfo {
                    key: name,
                    category: cat.canonical_name().to_string(),
                    tags: fm.tags,
                    summary: fm.summary,
                    relates_to: fm.relates_to,
                    created: fm.created,
                    updated: fm.updated,
                    expires: fm.expires,
                }
            } else {
                MemoryInfo {
                    key: name,
                    category: cat.canonical_name().to_string(),
                    tags: Vec::new(),
                    summary: None,
                    relates_to: Vec::new(),
                    created: None,
                    updated: None,
                    expires: None,
                }
            };
            if !info.is_expired() {
                results.push(info);
            }
        }
    }
    Ok(results)
}

/// Load all files from memory/session/ into a formatted context block.
/// Applies the masking filter. Caps at SESSION_MEMORY_CAP bytes.
/// Returns empty string if no session memories exist.
pub fn load_session_memory_block() -> String {
    const SESSION_MEMORY_CAP: usize = 32_768;
    let dir = memory_dir(&MemoryCategory::Session);
    if !dir.exists() {
        return String::new();
    }
    // Collect entries with their modification times so we can load the most
    // recently updated ones first. When the cap is reached, older entries are
    // dropped — entries you've actively written/updated are more likely to be
    // relevant than ones that haven't been touched in a long time.
    let mut entries: Vec<(String, std::time::SystemTime)> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let path = e.path();
                if !path.is_file() {
                    return None;
                }
                let mtime = e.metadata().ok()?.modified().ok()?;
                let stem = path.file_stem()?.to_string_lossy().to_string();
                Some((stem, mtime))
            })
            .collect(),
        Err(_) => return String::new(),
    };
    // Newest first; ties broken alphabetically.
    entries.sort_by(|(a_key, a_mtime), (b_key, b_mtime)| {
        b_mtime.cmp(a_mtime).then_with(|| a_key.cmp(b_key))
    });
    let entries: Vec<String> = entries.into_iter().map(|(k, _)| k).collect();
    if entries.is_empty() {
        return String::new();
    }

    let mut parts = Vec::new();
    let mut total = 0usize;
    let mut omitted_keys: Vec<String> = Vec::new();

    for key in &entries {
        let path = dir.join(format!("{}.md", key));
        if let Ok(content) = std::fs::read_to_string(&path) {
            let masked = crate::ai::filter::mask_sensitive(&content);
            let chunk = format!("--- {} ---\n{}\n\n", key, masked.trim());
            if total + chunk.len() <= SESSION_MEMORY_CAP {
                total += chunk.len();
                parts.push(chunk);
            } else {
                omitted_keys.push(key.clone());
            }
        }
    }

    if parts.is_empty() {
        return String::new();
    }

    let mut body = parts.join("");
    if !omitted_keys.is_empty() {
        body.push_str(&format!(
            "[{} session {} omitted due to size cap: {} — use read_memory to load individually]\n",
            omitted_keys.len(),
            if omitted_keys.len() == 1 {
                "memory"
            } else {
                "memories"
            },
            omitted_keys.join(", ")
        ));
    }

    format!("## Persistent Memory\n```\n{}\n```\n\n", body)
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
            let p = std::env::temp_dir().join(format!("de_mem_test_{}_{}", std::process::id(), n));
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

    #[test]
    fn add_and_read_memory() {
        let tmp = temp_home();
        with_home(&tmp, || {
            add_memory(
                "user_prefs",
                "Prefers verbose output",
                MemoryCategory::Session,
            )
            .unwrap();
            let val = read_memory("user_prefs", MemoryCategory::Session).unwrap();
            assert_eq!(val, "Prefers verbose output");
        });
    }

    #[test]
    fn add_memory_upsert() {
        let tmp = temp_home();
        with_home(&tmp, || {
            add_memory("key1", "first", MemoryCategory::Knowledge).unwrap();
            add_memory("key1", "second", MemoryCategory::Knowledge).unwrap();
            let val = read_memory("key1", MemoryCategory::Knowledge).unwrap();
            assert_eq!(val, "second");
        });
    }

    #[test]
    fn delete_memory_removes_file() {
        let tmp = temp_home();
        with_home(&tmp, || {
            add_memory("to_delete", "some content", MemoryCategory::Session).unwrap();
            delete_memory("to_delete", MemoryCategory::Session).unwrap();
            assert!(read_memory("to_delete", MemoryCategory::Session).is_err());
        });
    }

    #[test]
    fn list_memories_returns_all_categories() {
        let tmp = temp_home();
        with_home(&tmp, || {
            add_memory("sess_key", "s", MemoryCategory::Session).unwrap();
            add_memory("know_key", "k", MemoryCategory::Knowledge).unwrap();
            add_memory("inc_key", "i", MemoryCategory::Incident).unwrap();
            let all = list_memories(None).unwrap();
            let cats: Vec<_> = all.iter().map(|(c, _)| c.as_str()).collect();
            assert!(cats.contains(&"session"));
            assert!(cats.contains(&"knowledge"));
            assert!(cats.contains(&"incident"));
        });
    }

    #[test]
    fn memory_frontmatter_tags_parsed() {
        let tmp = temp_home();
        with_home(&tmp, || {
            add_memory(
                "tagged-key",
                "---\ntags: [postgres, production]\n---\nActual content",
                MemoryCategory::Knowledge,
            )
            .unwrap();
            let infos = list_memories_with_tags(Some(MemoryCategory::Knowledge)).unwrap();
            let info = infos
                .iter()
                .find(|m| m.key == "tagged-key")
                .expect("key not found");
            assert!(
                info.tags.contains(&"postgres".to_string()),
                "tag missing: {:?}",
                info.tags
            );
            assert!(
                info.tags.contains(&"production".to_string()),
                "tag missing: {:?}",
                info.tags
            );
        });
    }

    #[test]
    fn memory_without_frontmatter_has_no_tags() {
        let tmp = temp_home();
        with_home(&tmp, || {
            add_memory("plain-key", "Just plain content", MemoryCategory::Knowledge).unwrap();
            let infos = list_memories_with_tags(Some(MemoryCategory::Knowledge)).unwrap();
            let info = infos
                .iter()
                .find(|m| m.key == "plain-key")
                .expect("key not found");
            assert!(info.tags.is_empty(), "expected no tags: {:?}", info.tags);
        });
    }

    #[test]
    fn list_memories_with_tags_returns_all() {
        let tmp = temp_home();
        with_home(&tmp, || {
            add_memory("k1", "v1", MemoryCategory::Knowledge).unwrap();
            add_memory("k2", "---\ntags: [foo]\n---\nv2", MemoryCategory::Knowledge).unwrap();
            let infos = list_memories_with_tags(Some(MemoryCategory::Knowledge)).unwrap();
            assert_eq!(infos.len(), 2);
            let k2 = infos.iter().find(|m| m.key == "k2").unwrap();
            assert_eq!(k2.tags, vec!["foo"]);
        });
    }

    #[test]
    fn memory_frontmatter_summary_parsed() {
        let tmp = temp_home();
        with_home(&tmp, || {
            add_memory(
                "meta-key",
                "---\ntags: [foo]\nsummary: \"A useful description\"\nrelates_to: [other-key, runbook-x]\ncreated: \"2026-01-01T00:00:00Z\"\nupdated: \"2026-03-31T12:00:00Z\"\nexpires: \"2026-12-31\"\n---\nBody content",
                MemoryCategory::Knowledge,
            )
            .unwrap();
            let infos = list_memories_with_tags(Some(MemoryCategory::Knowledge)).unwrap();
            let info = infos
                .iter()
                .find(|m| m.key == "meta-key")
                .expect("key not found");
            assert_eq!(info.summary.as_deref(), Some("A useful description"));
            assert_eq!(info.relates_to, vec!["other-key", "runbook-x"]);
            assert_eq!(info.created.as_deref(), Some("2026-01-01T00:00:00Z"));
            assert_eq!(info.updated.as_deref(), Some("2026-03-31T12:00:00Z"));
            assert_eq!(info.expires.as_deref(), Some("2026-12-31"));
        });
    }

    #[test]
    fn memory_without_frontmatter_has_empty_metadata() {
        let tmp = temp_home();
        with_home(&tmp, || {
            add_memory("bare", "Just content", MemoryCategory::Knowledge).unwrap();
            let infos = list_memories_with_tags(Some(MemoryCategory::Knowledge)).unwrap();
            let info = infos
                .iter()
                .find(|m| m.key == "bare")
                .expect("key not found");
            assert!(info.summary.is_none());
            assert!(info.relates_to.is_empty());
            assert!(info.expires.is_none());
        });
    }

    #[test]
    fn build_frontmatter_roundtrip() {
        let tags = vec!["postgres".to_string(), "database".to_string()];
        let relates_to = vec!["runbook-x".to_string()];
        let fm = build_frontmatter(
            &tags,
            Some("Primary DB hosts"),
            &relates_to,
            Some("2026-01-01T00:00:00Z"),
            Some("2026-03-31T00:00:00Z"),
            None,
        );
        assert!(fm.starts_with("---\n"));
        assert!(fm.contains("tags:"));
        assert!(fm.contains("postgres"));
        assert!(fm.contains("summary:"));
        assert!(fm.contains("Primary DB hosts"));
        assert!(fm.contains("relates_to:"));
        assert!(fm.contains("runbook-x"));
        assert!(fm.contains("created:"));
        assert!(
            !fm.contains("expires:"),
            "expires should be omitted when None"
        );
        // Round-trip: parse what we built
        let full = format!("{}Body text", fm);
        let (parsed, body) = parse_memory_frontmatter(&full);
        assert_eq!(parsed.tags, tags);
        assert_eq!(parsed.summary.as_deref(), Some("Primary DB hosts"));
        assert_eq!(parsed.relates_to, relates_to);
        assert_eq!(body, "Body text");
    }

    #[test]
    fn build_frontmatter_empty_returns_empty_string() {
        let fm = build_frontmatter(&[], None, &[], None, None, None);
        assert!(fm.is_empty());
    }

    #[test]
    fn update_memory_creates_new_entry() {
        let tmp = temp_home();
        with_home(&tmp, || {
            let tags = vec!["foo".to_string()];
            update_memory(
                "new-key",
                MemoryCategory::Knowledge,
                Some("initial body"),
                false,
                Some(&tags),
                Some("A summary"),
                None,
                None,
            )
            .unwrap();
            let raw = read_memory("new-key", MemoryCategory::Knowledge).unwrap();
            assert!(raw.contains("initial body"));
            assert!(raw.contains("foo"));
            assert!(raw.contains("A summary"));
            assert!(raw.contains("created:"));
            assert!(raw.contains("updated:"));
        });
    }

    #[test]
    fn update_memory_partial_update_preserves_other_fields() {
        let tmp = temp_home();
        with_home(&tmp, || {
            // Write initial entry with full frontmatter.
            add_memory(
                "existing",
                "---\ntags: [alpha, beta]\nsummary: \"original summary\"\nrelates_to: [\"other\"]\n---\nOriginal body",
                MemoryCategory::Knowledge,
            )
            .unwrap();
            // Update only summary.
            update_memory(
                "existing",
                MemoryCategory::Knowledge,
                None,
                false,
                None,
                Some("new summary"),
                None,
                None,
            )
            .unwrap();
            let raw = read_memory("existing", MemoryCategory::Knowledge).unwrap();
            // Tags and relates_to preserved.
            assert!(raw.contains("alpha"), "tags should be preserved: {raw}");
            assert!(
                raw.contains("other"),
                "relates_to should be preserved: {raw}"
            );
            // Summary updated.
            assert!(
                raw.contains("new summary"),
                "summary should be updated: {raw}"
            );
            // Body preserved.
            assert!(
                raw.contains("Original body"),
                "body should be preserved: {raw}"
            );
        });
    }

    #[test]
    fn update_memory_append_mode() {
        let tmp = temp_home();
        with_home(&tmp, || {
            add_memory("append-key", "First line", MemoryCategory::Session).unwrap();
            update_memory(
                "append-key",
                MemoryCategory::Session,
                Some("Second line"),
                true,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let raw = read_memory("append-key", MemoryCategory::Session).unwrap();
            assert!(raw.contains("First line"), "original body missing: {raw}");
            assert!(raw.contains("Second line"), "appended body missing: {raw}");
        });
    }

    #[test]
    fn update_memory_replace_body() {
        let tmp = temp_home();
        with_home(&tmp, || {
            add_memory("replace-key", "Old body", MemoryCategory::Session).unwrap();
            update_memory(
                "replace-key",
                MemoryCategory::Session,
                Some("New body"),
                false,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let raw = read_memory("replace-key", MemoryCategory::Session).unwrap();
            assert!(!raw.contains("Old body"), "old body should be gone: {raw}");
            assert!(raw.contains("New body"), "new body missing: {raw}");
        });
    }

    #[test]
    fn update_memory_sets_updated_timestamp() {
        let tmp = temp_home();
        with_home(&tmp, || {
            update_memory(
                "ts-key",
                MemoryCategory::Knowledge,
                Some("body"),
                false,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let infos = list_memories_with_tags(Some(MemoryCategory::Knowledge)).unwrap();
            let info = infos
                .iter()
                .find(|m| m.key == "ts-key")
                .expect("key not found");
            assert!(info.updated.is_some(), "updated timestamp should be set");
            assert!(info.created.is_some(), "created timestamp should be set");
        });
    }

    #[test]
    fn session_memory_block_respects_cap() {
        let tmp = temp_home();
        with_home(&tmp, || {
            // Write many large entries to exceed SESSION_MEMORY_CAP (32 768 bytes)
            for i in 0..50 {
                let content = "x".repeat(1000);
                add_memory(
                    &format!("entry_{:02}", i),
                    &content,
                    MemoryCategory::Session,
                )
                .unwrap();
            }
            let block = load_session_memory_block();
            assert!(
                block.len() <= 32_768 + 200,
                "block should be capped near 32 KB"
            );
            assert!(block.contains("omitted"), "should mention omitted entries");
            assert!(
                block.contains("entry_"),
                "should name at least one omitted key"
            );
        });
    }
}
