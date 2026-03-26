//! Knowledge manifest builder, contextual auto-search, and related-knowledge hints.
//!
//! Three public functions:
//! - `build_knowledge_manifest()` — compact text index of all stored knowledge (Phase 1)
//! - `auto_search_context(query, pane)` — auto-load matching knowledge for first turn (Phase 4)
//! - `related_knowledge_hints(output)` — hint lines appended to tool results (Phase 5)

use crate::ai::filter::mask_sensitive;
use crate::memory::{MemoryCategory, MemoryInfo, list_memories_with_tags, read_memory};
use crate::runbook::{RunbookInfo, list_runbooks, load_runbook};
use crate::scripts::{list_scripts_with_tags, read_script};

// ---------------------------------------------------------------------------
// Internal entry cache
// ---------------------------------------------------------------------------

struct AllEntries {
    runbooks: Vec<RunbookInfo>,          // .name, .tags
    scripts: Vec<(String, Vec<String>)>, // (name, tags)
    knowledge: Vec<MemoryInfo>,          // .key, .tags
    incidents: Vec<MemoryInfo>,          // .key, .tags
}

fn load_all_entries() -> AllEntries {
    AllEntries {
        runbooks: list_runbooks().unwrap_or_default(),
        scripts: list_scripts_with_tags()
            .unwrap_or_default()
            .into_iter()
            .map(|(s, t)| (s.name, t))
            .collect(),
        knowledge: list_memories_with_tags(Some(MemoryCategory::Knowledge)).unwrap_or_default(),
        incidents: list_memories_with_tags(Some(MemoryCategory::Incident)).unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Phase 1: Knowledge manifest
// ---------------------------------------------------------------------------

/// Build a compact text index of all stored knowledge.
///
/// Returns an empty string when all stores are empty (new install).
/// Caps total output at 1024 bytes; individual sections truncated with `(+N more)`.
pub fn build_knowledge_manifest() -> String {
    const CAP: usize = 1024;

    let e = load_all_entries();

    if e.runbooks.is_empty()
        && e.scripts.is_empty()
        && e.knowledge.is_empty()
        && e.incidents.is_empty()
    {
        return String::new();
    }

    let header = "## Available Knowledge\n";
    let footer = "\n\n";
    let fixed = header.len() + footer.len();
    let mut remaining = CAP.saturating_sub(fixed);

    let mut lines: Vec<String> = Vec::new();

    if !e.runbooks.is_empty() {
        let items: Vec<String> = e
            .runbooks
            .iter()
            .map(|rb| {
                if rb.tags.is_empty() {
                    rb.name.clone()
                } else {
                    format!("{} [{}]", rb.name, rb.tags.join(", "))
                }
            })
            .collect();
        let line = build_section_line(
            &format!("Runbooks ({}): ", e.runbooks.len()),
            &items,
            remaining,
        );
        remaining = remaining.saturating_sub(line.len() + 1);
        lines.push(line);
    }

    if !e.scripts.is_empty() {
        let items: Vec<String> = e
            .scripts
            .iter()
            .map(|(name, tags)| {
                if tags.is_empty() {
                    name.clone()
                } else {
                    format!("{} [{}]", name, tags.join(", "))
                }
            })
            .collect();
        let line = build_section_line(
            &format!("Scripts ({}): ", e.scripts.len()),
            &items,
            remaining,
        );
        remaining = remaining.saturating_sub(line.len() + 1);
        lines.push(line);
    }

    if !e.knowledge.is_empty() {
        let items: Vec<String> = e
            .knowledge
            .iter()
            .map(|m| {
                if m.tags.is_empty() {
                    m.key.clone()
                } else {
                    format!("{} [{}]", m.key, m.tags.join(", "))
                }
            })
            .collect();
        let line = build_section_line(
            &format!("Knowledge memories ({}): ", e.knowledge.len()),
            &items,
            remaining,
        );
        remaining = remaining.saturating_sub(line.len() + 1);
        lines.push(line);
    }

    if !e.incidents.is_empty() {
        let items: Vec<String> = e
            .incidents
            .iter()
            .map(|m| {
                if m.tags.is_empty() {
                    m.key.clone()
                } else {
                    format!("{} [{}]", m.key, m.tags.join(", "))
                }
            })
            .collect();
        let line = build_section_line(
            &format!("Incidents ({}): ", e.incidents.len()),
            &items,
            remaining,
        );
        lines.push(line);
    }

    format!("{}{}{}", header, lines.join("\n"), footer)
}

/// Build one "SectionName (N): item1 [tag1, tag2], item2, (+M more)" line,
/// fitting within `budget` bytes total (prefix included).
fn build_section_line(prefix: &str, items: &[String], budget: usize) -> String {
    if budget < prefix.len() {
        return format!("{}(+{} more)", prefix, items.len());
    }

    let mut result = prefix.to_string();
    let mut count = 0usize;

    for (i, item) in items.iter().enumerate() {
        let sep = if i == 0 { "" } else { ", " };
        let candidate = format!("{}{}{}", result, sep, item);
        let remaining_after = items.len() - i - 1;
        let full_len = if remaining_after > 0 {
            candidate.len() + format!(", (+{} more)", remaining_after).len()
        } else {
            candidate.len()
        };

        if full_len <= budget {
            result = candidate;
            count = i + 1;
        } else {
            let omitted = items.len() - count;
            return if count > 0 {
                format!("{}, (+{} more)", result, omitted)
            } else {
                format!("{}(+{} more)", prefix, items.len())
            };
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Phase 4: Contextual auto-search
// ---------------------------------------------------------------------------

/// Scan user query + pane content for terms matching stored knowledge.
/// Loads up to 3 matching items (runbook > tag > memory key > script name),
/// applies sensitive-data masking, caps at 4096 bytes.
/// Returns empty string when no matches.
pub fn auto_search_context(query: &str, pane_content: &str) -> String {
    const MAX_ITEMS: usize = 3;
    const CAP: usize = 4096;
    const TRUNC_NOTE: &str =
        "\n[truncated — use read_runbook/read_memory/read_script for full content]";

    let corpus = format!("{} {}", query, pane_content).to_lowercase();

    let e = load_all_entries();

    // Collect (priority, kind, key, optional_category)
    // Priority: 0=runbook name, 1=runbook tag, 2=memory key, 3=script name
    let mut matches: Vec<(u8, &'static str, String, Option<String>)> = Vec::new();

    for rb in &e.runbooks {
        let name_lc = rb.name.to_lowercase();
        if corpus.contains(&name_lc) {
            matches.push((0, "runbook", rb.name.clone(), None));
            continue;
        }
        // Also match on significant keywords from the hyphen/underscore-split name
        let name_keywords: Vec<&str> = name_lc
            .split(['-', '_'])
            .filter(|w| w.len() >= 4)
            .collect();
        if name_keywords.iter().any(|kw| corpus.contains(*kw)) {
            matches.push((0, "runbook", rb.name.clone(), None));
            continue;
        }
        for tag in &rb.tags {
            let tag_lc = tag.to_lowercase();
            if !tag_lc.is_empty() && corpus.contains(&tag_lc) {
                matches.push((1, "runbook", rb.name.clone(), None));
                break;
            }
        }
    }

    for mem in &e.knowledge {
        let key_lc = mem.key.to_lowercase();
        if corpus.contains(&key_lc) {
            matches.push((
                2,
                "knowledge",
                mem.key.clone(),
                Some("knowledge".to_string()),
            ));
            continue;
        }
        // Also match on significant keywords from the key
        let key_keywords: Vec<&str> = key_lc
            .split(['-', '_'])
            .filter(|w| w.len() >= 4)
            .collect();
        if key_keywords.iter().any(|kw| corpus.contains(*kw)) {
            matches.push((
                2,
                "knowledge",
                mem.key.clone(),
                Some("knowledge".to_string()),
            ));
            continue;
        }
        for tag in &mem.tags {
            let tag_lc = tag.to_lowercase();
            if !tag_lc.is_empty() && corpus.contains(&tag_lc) {
                matches.push((
                    2,
                    "knowledge",
                    mem.key.clone(),
                    Some("knowledge".to_string()),
                ));
                break;
            }
        }
    }

    for (name, tags) in &e.scripts {
        let name_lc = name.to_lowercase();
        if corpus.contains(&name_lc) {
            matches.push((3, "script", name.clone(), None));
            continue;
        }
        for tag in tags {
            let tag_lc = tag.to_lowercase();
            if !tag_lc.is_empty() && corpus.contains(&tag_lc) {
                matches.push((3, "script", name.clone(), None));
                break;
            }
        }
    }

    // Sort by priority, deduplicate by (kind, key)
    matches.sort_by_key(|(p, k, n, _)| (*p, k.to_string(), n.clone()));
    matches.dedup_by_key(|(_, kind, key, _)| format!("{}/{}", kind, key));

    let top: Vec<_> = matches.into_iter().take(MAX_ITEMS).collect();
    if top.is_empty() {
        return String::new();
    }

    // Load content for each match
    let mut sections: Vec<String> = Vec::new();
    let mut total_bytes = 0usize;

    for (_, kind, key, _cat) in &top {
        let raw = match *kind {
            "runbook" => match load_runbook(key) {
                Ok(rb) => rb.content,
                Err(_) => continue,
            },
            "knowledge" => match read_memory(key, MemoryCategory::Knowledge) {
                Ok(v) => v,
                Err(_) => continue,
            },
            "script" => match read_script(key) {
                Ok(v) => v,
                Err(_) => continue,
            },
            _ => continue,
        };

        let masked = mask_sensitive(&raw);
        let heading = match *kind {
            "runbook" => format!("### Runbook: {}", key),
            "knowledge" => format!("### Knowledge: {}", key),
            "script" => format!("### Script: {}", key),
            _ => format!("### {}", key),
        };
        let section = format!("{}\n{}\n", heading, masked.trim());

        let new_total = total_bytes + section.len();
        if new_total > CAP {
            // Truncate this section to fit
            let available = CAP.saturating_sub(total_bytes + heading.len() + 2 + TRUNC_NOTE.len());
            if available > 0 {
                let truncated = &masked[..available.min(masked.len())];
                sections.push(format!("{}\n{}{}", heading, truncated, TRUNC_NOTE));
            }
            break;
        }

        total_bytes = new_total;
        sections.push(section);
    }

    if sections.is_empty() {
        return String::new();
    }

    format!("## Auto-loaded Knowledge\n{}\n\n", sections.join("\n"))
}

// ---------------------------------------------------------------------------
// Phase 5: Related knowledge hints
// ---------------------------------------------------------------------------

/// Scan command output for terms matching stored knowledge.
/// Returns a `[Related knowledge: ...]` hint line, or empty string on no matches.
/// Excludes scripts (operational, not informational).
pub fn related_knowledge_hints(output: &str) -> String {
    let corpus = output.to_lowercase();

    let e = load_all_entries();

    let mut hints: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for rb in &e.runbooks {
        if hints.len() >= 3 {
            break;
        }
        let name_lc = rb.name.to_lowercase();
        if corpus.contains(&name_lc) && seen.insert(format!("runbook:{}", rb.name)) {
            hints.push(format!("runbook \"{}\"", rb.name));
            continue;
        }
        for tag in &rb.tags {
            let tag_lc = tag.to_lowercase();
            if !tag_lc.is_empty()
                && corpus.contains(&tag_lc)
                && seen.insert(format!("runbook:{}", rb.name))
            {
                hints.push(format!("runbook \"{}\"", rb.name));
                break;
            }
        }
    }

    for mem in &e.knowledge {
        if hints.len() >= 3 {
            break;
        }
        let key_lc = mem.key.to_lowercase();
        if corpus.contains(&key_lc) && seen.insert(format!("memory:{}", mem.key)) {
            hints.push(format!("memory \"{}\"", mem.key));
            continue;
        }
        for tag in &mem.tags {
            let tag_lc = tag.to_lowercase();
            if !tag_lc.is_empty()
                && corpus.contains(&tag_lc)
                && seen.insert(format!("memory:{}", mem.key))
            {
                hints.push(format!("memory \"{}\"", mem.key));
                break;
            }
        }
    }

    if hints.is_empty() {
        return String::new();
    }

    format!("[Related knowledge: {}]", hints.join(", "))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
            let p = std::env::temp_dir().join(format!("de_mf_test_{}_{}", std::process::id(), n));
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

    const SAMPLE_RUNBOOK: &str = "# Runbook: disk-check\n\n## Alert Criteria\n- Usage > 90%\n";

    // --- Phase 1 tests ---

    #[test]
    fn manifest_empty_stores() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            let m = build_knowledge_manifest();
            assert!(m.is_empty(), "expected empty manifest, got: {m:?}");
        });
    }

    #[test]
    fn manifest_runbooks_with_tags() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            crate::runbook::write_runbook("high-disk-usage", "---\ntags: [disk, linux]\n---\n# Runbook: high-disk-usage\n\n## Alert Criteria\n- usage > 90%\n").unwrap();
            let m = build_knowledge_manifest();
            assert!(m.contains("high-disk-usage"), "runbook name missing: {m}");
            assert!(m.contains("disk"), "tag missing: {m}");
            assert!(m.contains("linux"), "tag missing: {m}");
            assert!(m.contains("## Available Knowledge"), "header missing: {m}");
        });
    }

    #[test]
    fn manifest_scripts_listed() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            crate::scripts::write_script("cleanup-logs.sh", "#!/bin/bash\necho done").unwrap();
            let m = build_knowledge_manifest();
            assert!(m.contains("cleanup-logs.sh"), "script name missing: {m}");
        });
    }

    #[test]
    fn manifest_knowledge_and_incidents() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            crate::memory::add_memory(
                "prod-db-hosts",
                "db1, db2",
                crate::memory::MemoryCategory::Knowledge,
            )
            .unwrap();
            crate::memory::add_memory(
                "2026-02-incident",
                "db failover",
                crate::memory::MemoryCategory::Incident,
            )
            .unwrap();
            let m = build_knowledge_manifest();
            assert!(m.contains("prod-db-hosts"), "knowledge key missing: {m}");
            assert!(m.contains("2026-02-incident"), "incident key missing: {m}");
            assert!(
                m.contains("Knowledge memories"),
                "knowledge section missing: {m}"
            );
            assert!(m.contains("Incidents"), "incidents section missing: {m}");
        });
    }

    #[test]
    fn manifest_excludes_session_memories() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            crate::memory::add_memory(
                "my-session-pref",
                "dark mode",
                crate::memory::MemoryCategory::Session,
            )
            .unwrap();
            let m = build_knowledge_manifest();
            assert!(
                !m.contains("my-session-pref"),
                "session memory should not appear: {m}"
            );
            // Should be empty since only session memory exists
            assert!(m.is_empty(), "expected empty manifest: {m:?}");
        });
    }

    #[test]
    fn manifest_caps_at_1kb() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            // Write many knowledge memories to exceed 1KB
            for i in 0..50 {
                crate::memory::add_memory(
                    &format!("very-long-key-name-for-testing-{:02}", i),
                    "value",
                    crate::memory::MemoryCategory::Knowledge,
                )
                .unwrap();
            }
            let m = build_knowledge_manifest();
            assert!(m.len() <= 1024, "manifest exceeds 1KB: {} bytes", m.len());
            assert!(m.contains("(+"), "should contain truncation marker: {m}");
            assert!(m.contains("more)"), "should contain '(+N more)': {m}");
        });
    }

    #[test]
    fn manifest_mixed_stores() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            crate::runbook::write_runbook(
                "deploy-rollback",
                "# Runbook: deploy-rollback\n\n## Alert Criteria\n- deploy failed\n",
            )
            .unwrap();
            crate::scripts::write_script("rotate-certs.sh", "#!/bin/bash\necho done").unwrap();
            crate::memory::add_memory(
                "monitoring-stack",
                "prometheus+grafana",
                crate::memory::MemoryCategory::Knowledge,
            )
            .unwrap();
            crate::memory::add_memory(
                "2026-01-outage",
                "details",
                crate::memory::MemoryCategory::Incident,
            )
            .unwrap();
            let m = build_knowledge_manifest();
            assert!(m.contains("Runbooks"), "missing runbooks section: {m}");
            assert!(m.contains("Scripts"), "missing scripts section: {m}");
            assert!(
                m.contains("Knowledge memories"),
                "missing knowledge section: {m}"
            );
            assert!(m.contains("Incidents"), "missing incidents section: {m}");
        });
    }

    // --- Phase 4 tests ---

    #[test]
    fn auto_search_matches_runbook_name() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            crate::runbook::write_runbook("high-disk-usage", SAMPLE_RUNBOOK).unwrap();
            let result = auto_search_context("disk is filling up", "");
            assert!(
                result.contains("high-disk-usage"),
                "runbook name missing: {result}"
            );
            assert!(
                result.contains("## Auto-loaded Knowledge"),
                "header missing: {result}"
            );
        });
    }

    #[test]
    fn auto_search_matches_runbook_tag() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            crate::runbook::write_runbook(
                "ssl-renewal",
                "---\ntags: [certs, ssl]\n---\n# Runbook: ssl-renewal\n\n## Alert Criteria\n- cert expires\n",
            ).unwrap();
            let result = auto_search_context("ssl certificate is expiring", "");
            assert!(
                result.contains("ssl-renewal"),
                "runbook name missing from tag match: {result}"
            );
        });
    }

    #[test]
    fn auto_search_matches_memory_key() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            crate::memory::add_memory(
                "prod-db-hosts",
                "db1.internal, db2.internal",
                crate::memory::MemoryCategory::Knowledge,
            )
            .unwrap();
            let result = auto_search_context("connect to prod-db-hosts", "");
            assert!(
                result.contains("prod-db-hosts"),
                "memory key missing: {result}"
            );
        });
    }

    #[test]
    fn auto_search_respects_4kb_cap() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            // Write a large runbook
            let big_content = format!(
                "# Runbook: big-runbook\n\n## Alert Criteria\n- x\n\n{}",
                "detailed content\n".repeat(500)
            );
            crate::runbook::write_runbook("big-runbook", &big_content).unwrap();
            let result = auto_search_context("big-runbook", "");
            assert!(
                result.len() <= 4096 + 200,
                "output exceeds 4KB cap: {} bytes",
                result.len()
            );
        });
    }

    #[test]
    fn auto_search_empty_on_no_match() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            crate::runbook::write_runbook("memory-leak", SAMPLE_RUNBOOK).unwrap();
            let result = auto_search_context("disk space is low", "");
            assert!(result.is_empty(), "expected no match: {result}");
        });
    }

    #[test]
    fn auto_search_deduplicates() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            crate::runbook::write_runbook(
                "disk-check",
                "---\ntags: [disk]\n---\n# Runbook: disk-check\n\n## Alert Criteria\n- usage > 90%\n",
            ).unwrap();
            // "disk" matches both name and tag — should appear only once
            let result = auto_search_context("disk is full", "");
            let count = result.matches("### Runbook: disk-check").count();
            assert_eq!(count, 1, "runbook should appear exactly once: {result}");
        });
    }

    #[test]
    fn auto_search_max_three_items() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            for i in 0..5 {
                crate::memory::add_memory(
                    &format!("key-{i}"),
                    &format!("content for key-{i}"),
                    crate::memory::MemoryCategory::Knowledge,
                )
                .unwrap();
            }
            let result = auto_search_context("key-0 key-1 key-2 key-3 key-4", "");
            let count = result.matches("### Knowledge:").count();
            assert!(
                count <= 3,
                "should load at most 3 items, got {count}: {result}"
            );
        });
    }

    // --- Phase 5 tests ---

    #[test]
    fn related_hints_matches_runbook_name() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            crate::runbook::write_runbook("high-disk-usage", SAMPLE_RUNBOOK).unwrap();
            let hints = related_knowledge_hints(
                "Disk usage is at 95% on /dev/sda1. high-disk-usage threshold exceeded.",
            );
            assert!(
                hints.contains("runbook \"high-disk-usage\""),
                "hint missing: {hints}"
            );
            assert!(
                hints.starts_with("[Related knowledge:"),
                "wrong format: {hints}"
            );
        });
    }

    #[test]
    fn related_hints_matches_tags() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            crate::runbook::write_runbook(
                "disk-check",
                "---\ntags: [disk, storage]\n---\n# Runbook: disk-check\n\n## Alert Criteria\n- usage > 90%\n",
            ).unwrap();
            let hints = related_knowledge_hints("disk usage is high on the storage volume");
            assert!(
                hints.contains("runbook \"disk-check\""),
                "tag-based hint missing: {hints}"
            );
        });
    }

    #[test]
    fn related_hints_caps_at_three() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            for i in 0..5 {
                crate::memory::add_memory(
                    &format!("key-{i}"),
                    "v",
                    crate::memory::MemoryCategory::Knowledge,
                )
                .unwrap();
            }
            let output = "key-0 key-1 key-2 key-3 key-4";
            let hints = related_knowledge_hints(output);
            // Count occurrences of "memory \""
            let count = hints.matches("memory \"").count();
            assert!(
                count <= 3,
                "hints should cap at 3 items, got {count}: {hints}"
            );
        });
    }

    #[test]
    fn related_hints_empty_on_no_match() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            crate::runbook::write_runbook("memory-leak-check", SAMPLE_RUNBOOK).unwrap();
            let hints = related_knowledge_hints("some unrelated command output");
            assert!(hints.is_empty(), "expected no hints: {hints}");
        });
    }

    #[test]
    fn related_hints_excludes_scripts() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            crate::scripts::write_script("cleanup-logs.sh", "#!/bin/bash\necho done").unwrap();
            let hints = related_knowledge_hints("cleanup-logs.sh ran successfully");
            assert!(
                !hints.contains("script"),
                "scripts should be excluded from hints: {hints}"
            );
        });
    }

    // --- Phase 6 tests (tag-aware) ---

    #[test]
    fn manifest_shows_memory_tags() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            // Write memory with frontmatter tags
            crate::memory::add_memory(
                "webhook-setup",
                "---\ntags: [webhook, alertmanager]\n---\nSetup instructions here",
                crate::memory::MemoryCategory::Knowledge,
            )
            .unwrap();
            let m = build_knowledge_manifest();
            assert!(m.contains("webhook-setup"), "key missing: {m}");
            assert!(m.contains("webhook"), "tag missing: {m}");
            assert!(m.contains("alertmanager"), "tag missing: {m}");
        });
    }

    #[test]
    fn manifest_shows_script_tags() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            crate::scripts::write_script("rotate-certs.sh", "#!/bin/bash\necho done").unwrap();
            let meta = crate::config::config_dir()
                .join("scripts")
                .join("rotate-certs.sh.meta.toml");
            std::fs::write(meta, "tags = [\"certs\", \"ssl\"]\n").unwrap();
            let m = build_knowledge_manifest();
            assert!(m.contains("rotate-certs.sh"), "script missing: {m}");
            assert!(m.contains("certs"), "tag missing: {m}");
        });
    }

    #[test]
    fn auto_search_matches_memory_tags() {
        let tmp = TmpHome::new();
        with_home(&tmp, || {
            crate::memory::add_memory(
                "postgres-config",
                "---\ntags: [postgres, database]\n---\nConnection string: ...",
                crate::memory::MemoryCategory::Knowledge,
            )
            .unwrap();
            let result = auto_search_context("postgres is down", "");
            assert!(
                result.contains("postgres-config"),
                "tag-based memory match missing: {result}"
            );
        });
    }
}
