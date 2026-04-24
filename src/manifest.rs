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
        let items: Vec<String> = e.knowledge.iter().map(memory_manifest_item).collect();
        let line = build_section_line(
            &format!("Knowledge memories ({}): ", e.knowledge.len()),
            &items,
            remaining,
        );
        remaining = remaining.saturating_sub(line.len() + 1);
        lines.push(line);
    }

    if !e.incidents.is_empty() {
        let items: Vec<String> = e.incidents.iter().map(memory_manifest_item).collect();
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

/// Format a memory entry for the knowledge manifest.
/// Format: `key — summary [tag1, tag2]` / `key — summary` / `key [tag1, tag2]` / `key`
fn memory_manifest_item(m: &crate::memory::MemoryInfo) -> String {
    match (&m.summary, m.tags.is_empty()) {
        (Some(s), false) => format!("{} — {} [{}]", m.key, s, m.tags.join(", ")),
        (Some(s), true) => format!("{} — {}", m.key, s),
        (None, false) => format!("{} [{}]", m.key, m.tags.join(", ")),
        (None, true) => m.key.clone(),
    }
}

// ---------------------------------------------------------------------------
// Phase 4: Contextual auto-search
// ---------------------------------------------------------------------------

/// Scan user query + pane content for terms matching stored knowledge.
/// Loads up to 3 matching items (runbook > tag > memory key/summary > script name),
/// then follows `relates_to` links on matched memories to pull in related entries.
/// Applies sensitive-data masking, caps at 4096 bytes.
/// Returns empty string when no matches.
pub fn auto_search_context(query: &str, pane_content: &str) -> String {
    const MAX_ITEMS: usize = 3;
    const CAP: usize = 4096;
    const TRUNC_NOTE: &str =
        "\n[truncated — use read_runbook/read_memory/read_script for full content]";

    let corpus = format!("{} {}", query, pane_content).to_lowercase();

    let e = load_all_entries();

    // Collect (priority, kind, key, optional_category)
    // Priority: 0=runbook name, 1=runbook tag, 2=memory key/summary/tag, 3=script name
    let mut matches: Vec<(u8, &'static str, String, Option<String>)> = Vec::new();

    for rb in &e.runbooks {
        let name_lc = rb.name.to_lowercase();
        if corpus.contains(&name_lc) {
            matches.push((0, "runbook", rb.name.clone(), None));
            continue;
        }
        // Also match on significant keywords from the hyphen/underscore-split name
        let name_keywords: Vec<&str> = name_lc.split(['-', '_']).filter(|w| w.len() >= 4).collect();
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
        let key_keywords: Vec<&str> = key_lc.split(['-', '_']).filter(|w| w.len() >= 4).collect();
        if key_keywords.iter().any(|kw| corpus.contains(*kw)) {
            matches.push((
                2,
                "knowledge",
                mem.key.clone(),
                Some("knowledge".to_string()),
            ));
            continue;
        }
        // Match on summary text
        if let Some(ref s) = mem.summary {
            let summary_lc = s.to_lowercase();
            if !summary_lc.is_empty() && corpus.contains(&summary_lc) {
                matches.push((
                    2,
                    "knowledge",
                    mem.key.clone(),
                    Some("knowledge".to_string()),
                ));
                continue;
            }
            // Also match on significant words from the summary
            let summary_words: Vec<&str> = summary_lc
                .split_whitespace()
                .filter(|w| w.len() >= 4)
                .collect();
            if summary_words.iter().any(|w| corpus.contains(*w)) {
                matches.push((
                    2,
                    "knowledge",
                    mem.key.clone(),
                    Some("knowledge".to_string()),
                ));
                continue;
            }
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

    let mut top: Vec<_> = matches.into_iter().take(MAX_ITEMS).collect();

    // Follow relates_to links: for each matched knowledge memory, check its relates_to
    // references and load them if we still have budget and they aren't already included.
    if top.len() < MAX_ITEMS {
        let loaded_keys: std::collections::HashSet<String> =
            top.iter().map(|(_, _, k, _)| k.clone()).collect();
        let knowledge_key_set: std::collections::HashSet<String> =
            e.knowledge.iter().map(|m| m.key.clone()).collect();
        let runbook_name_set: std::collections::HashSet<String> =
            e.runbooks.iter().map(|r| r.name.clone()).collect();

        let mut related: Vec<(u8, &'static str, String, Option<String>)> = Vec::new();
        for (_, kind, key, _) in &top {
            if *kind != "knowledge" {
                continue;
            }
            if let Some(mem) = e.knowledge.iter().find(|m| &m.key == key) {
                for ref_key in &mem.relates_to {
                    if loaded_keys.contains(ref_key) {
                        continue;
                    }
                    if knowledge_key_set.contains(ref_key) {
                        related.push((
                            4,
                            "knowledge",
                            ref_key.clone(),
                            Some("knowledge".to_string()),
                        ));
                    } else if runbook_name_set.contains(ref_key) {
                        related.push((4, "runbook", ref_key.clone(), None));
                    }
                }
            }
        }
        related.dedup_by_key(|(_, kind, key, _)| format!("{}/{}", kind, key));
        let budget = MAX_ITEMS - top.len();
        top.extend(related.into_iter().take(budget));
    }
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
        // Match on summary text
        if let Some(ref s) = mem.summary {
            let summary_lc = s.to_lowercase();
            let summary_words: Vec<&str> = summary_lc
                .split_whitespace()
                .filter(|w| w.len() >= 4)
                .collect();
            if summary_words.iter().any(|w| corpus.contains(*w))
                && seen.insert(format!("memory:{}", mem.key))
            {
                hints.push(format!("memory \"{}\"", mem.key));
                continue;
            }
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
#[path = "manifest_tests.rs"]
mod tests;
