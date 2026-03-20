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

/// Memory entry with optional tags parsed from frontmatter.
pub struct MemoryInfo {
    pub key: String,
    pub tags: Vec<String>,
}

/// Parse optional YAML frontmatter from a memory file.
/// Returns `(tags, body)`. If no frontmatter, body is the full content and tags are empty.
fn parse_memory_frontmatter(raw: &str) -> (Vec<String>, String) {
    if !raw.starts_with("---\n") {
        return (Vec::new(), raw.to_string());
    }
    let search_from = 4;
    let end_marker = "\n---\n";
    if let Some(rel_pos) = raw[search_from..].find(end_marker) {
        let fm_end = search_from + rel_pos;
        let frontmatter = &raw[search_from..fm_end];
        let body = raw[fm_end + end_marker.len()..].to_string();
        let tags = parse_memory_tag_field(frontmatter);
        (tags, body)
    } else {
        (Vec::new(), raw.to_string())
    }
}

fn parse_memory_tag_field(frontmatter: &str) -> Vec<String> {
    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("tags:") {
            let rest = trimmed["tags:".len()..].trim();
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

pub fn add_memory(key: &str, value: &str, category: MemoryCategory) -> Result<()> {
    validate_memory_key(key)?;
    ensure_memory_dir(&category)?;
    let path = memory_dir(&category).join(format!("{}.md", key));
    std::fs::write(&path, value).with_context(|| format!("writing memory key '{}'", key))
}

pub fn delete_memory(key: &str, category: MemoryCategory) -> Result<()> {
    let path = memory_dir(&category).join(format!("{}.md", key));
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

pub fn read_memory(key: &str, category: MemoryCategory) -> Result<String> {
    let path = memory_dir(&category).join(format!("{}.md", key));
    std::fs::read_to_string(&path)
        .with_context(|| format!("reading memory key '{}' from {}", key, path.display()))
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
            let tags = if let Ok(raw) = std::fs::read_to_string(&path) {
                let (t, _) = parse_memory_frontmatter(&raw);
                t
            } else {
                Vec::new()
            };
            results.push(MemoryInfo { key: name, tags });
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
