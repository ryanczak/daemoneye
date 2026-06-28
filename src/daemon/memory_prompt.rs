//! G5: Tiered memory prompt assembly.
//!
//! Stable ambient block (pinned + high-relevance, cached with TTL) and
//! dynamic turn-relevant block (computed per turn from tag overlap).

use std::sync::atomic::{AtomicU64, Ordering};

use crate::config::Config;
use crate::daemon::stats;
use crate::memory::index;
use crate::memory::tags::SessionTags;
use crate::memory::{MemoryInfo, list_memories_with_tags};

/// Monotonic dirty sequence counter for pinned-memory changes.
static PINNED_DIRTY_SEQ: AtomicU64 = AtomicU64::new(0);

/// Increment the pinned dirty sequence (called on pinned-memory CRUD).
pub fn invalidate_stable_block() {
    PINNED_DIRTY_SEQ.fetch_add(1, Ordering::Relaxed);
}

fn format_memory_entry(info: &MemoryInfo) -> String {
    let summary = info.summary.as_deref().unwrap_or("");
    let text = if !summary.is_empty() {
        summary.to_string()
    } else {
        format!("[memory: {}]", info.key)
    };
    format!("--- {} ---\n{}\n", info.key, text.trim())
}

/// Assemble the dynamic turn-relevant memory block.
/// Computed fresh per turn from tag overlap + FTS5 search.
pub fn assemble_turn_relevant_memory(
    session_tags: &SessionTags,
    user_turn: &str,
    _config: &Config,
    session_id: Option<&str>,
    turn: usize,
    namespaces: &[&str],
) -> Option<String> {
    // G5 stub: use defaults until Config.memory section is added
    let budget = 4096;
    let threshold = 0.5;
    let all_tags = session_tags.all_tags();

    if all_tags.is_empty() && user_turn.is_empty() {
        stats::inc_dynamic_block_empty_turns();
        return None;
    }

    let all_memories = list_memories_with_tags(None, namespaces).unwrap_or_default();

    // Exclude archived memories
    let active: Vec<&MemoryInfo> = all_memories
        .iter()
        .filter(|m| {
            !m.pinned.unwrap_or(false) // exclude pinned (already in ambient)
                && !m.is_expired()
                && crate::memory::review::effective_confidence(m) >= threshold
        })
        .collect();

    // 1. Tag overlap candidates
    let tag_candidates = find_by_tag_overlap(&all_tags, 10, namespaces);

    // 2. One-hop relates_to expansion
    let relates_keys: Vec<String> = tag_candidates.iter().map(|k| k.key.clone()).collect();
    let relates_candidates = expand_relates_to(&relates_keys, namespaces);

    // 3. FTS5 search against user turn
    let fts_candidates = if !user_turn.is_empty() {
        ftsearch_memories(user_turn, 10, namespaces)
    } else {
        Vec::new()
    };

    // Merge candidates: build a scored set
    let mut candidate_keys: std::collections::HashMap<String, f64> =
        std::collections::HashMap::new();

    // Tag overlap score: overlap_count / tags.len()
    let tag_set: std::collections::HashSet<&String> = all_tags.iter().collect();
    for info in &tag_candidates {
        let overlap = info.tags.iter().filter(|t| tag_set.contains(t)).count();
        let score = overlap as f64 / all_tags.len().max(1) as f64;
        let eff = crate::memory::review::effective_confidence(info);
        let combined = score * eff;
        *candidate_keys.entry(info.key.clone()).or_insert(0.0) = combined;
    }

    // Relates_to candidates get base score
    for info in &relates_candidates {
        candidate_keys
            .entry(info.key.clone())
            .or_insert(0.3 * crate::memory::review::effective_confidence(info));
    }

    // FTS5 candidates get base score
    for info in &fts_candidates {
        candidate_keys
            .entry(info.key.clone())
            .or_insert(0.2 * crate::memory::review::effective_confidence(info));
    }

    // Filter by confidence threshold and build block
    let mut scored: Vec<(&MemoryInfo, f64)> = Vec::new();
    for info in &active {
        if let Some(&score) = candidate_keys.get(&info.key) {
            scored.push((info, score));
        }
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut parts: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut count = 0usize;

    for (info, _) in &scored {
        let entry = format_memory_entry(info);
        if total + entry.len() <= budget {
            total += entry.len();
            parts.push(entry);
            count += 1;
        }
    }

    if parts.is_empty() {
        stats::inc_dynamic_block_empty_turns();
        return None;
    }

    // Update rolling average for dynamic block size
    stats::set_memories_in_dynamic_block_avg(count);

    // Log memory_retrieved event with keys and scores
    let keys: Vec<String> = scored
        .iter()
        .map(|(info, _)| info.key.clone())
        .take(count)
        .collect();
    let scores: Vec<f64> = scored.iter().map(|(_, s)| *s).take(count).collect();
    crate::daemon::utils::log_event(
        "memory_retrieved",
        serde_json::json!({
            "session_id": session_id.unwrap_or("-"),
            "turn": turn,
            "keys": keys,
            "scores": scores,
        }),
    );

    let header = format!("[TURN MEMORY] {} memories, {} bytes\n", count, total);
    Some(format!("{}\n{}", header, parts.join("\n")))
}

/// Find memories whose tags intersect with the given tags.
/// Scored by overlap ratio, sorted by score × effective_confidence.
pub fn find_by_tag_overlap(tags: &[String], limit: usize, namespaces: &[&str]) -> Vec<MemoryInfo> {
    let all_memories = list_memories_with_tags(None, namespaces).unwrap_or_default();
    let tag_set: std::collections::HashSet<&str> = tags.iter().map(|s| s.as_str()).collect();

    let mut scored: Vec<(MemoryInfo, f64)> = Vec::new();
    for info in all_memories {
        let overlap = info
            .tags
            .iter()
            .filter(|t| tag_set.contains(t.as_str()))
            .count();
        if overlap > 0 {
            let score = overlap as f64 / tags.len().max(1) as f64;
            let eff = crate::memory::review::effective_confidence(&info);
            scored.push((info, score * eff));
        }
    }

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .take(limit)
        .map(|(info, _)| info)
        .collect()
}

/// One-hop relates_to expansion: find memories whose relates_to contains any of the given keys.
pub fn expand_relates_to(keys: &[String], namespaces: &[&str]) -> Vec<MemoryInfo> {
    let all_memories = list_memories_with_tags(None, namespaces).unwrap_or_default();
    let key_set: std::collections::HashSet<&str> = keys.iter().map(|s| s.as_str()).collect();

    let mut found: Vec<MemoryInfo> = Vec::new();
    for info in all_memories {
        if info.relates_to.iter().any(|r| key_set.contains(r.as_str())) {
            found.push(info);
        }
    }
    found
}

/// FTS5 search on memory index for matching memories.
/// Returns top `limit` by BM25 score.
pub fn ftsearch_memories(query: &str, limit: usize, namespaces: &[&str]) -> Vec<MemoryInfo> {
    let results = index::fts5_search(query, limit);
    let all_memories = list_memories_with_tags(None, namespaces).unwrap_or_default();

    let mut found = Vec::new();
    for (key, _) in results {
        if let Some(info) = all_memories.iter().find(|m| m.key == key) {
            found.push(info.clone());
        }
    }
    found
}

/// Pin a memory entry: set pinned=true, update index, invalidate cache.
/// G5 stub: not yet fully implemented.
pub fn pin_memory(_key: &str, _category: crate::memory::MemoryCategory) -> anyhow::Result<()> {
    stats::inc_memories_pinned();
    invalidate_stable_block();
    Ok(())
}

/// Unpin a memory entry: set pinned=false, update index, invalidate cache.
/// G5 stub: not yet fully implemented.
pub fn unpin_memory(_key: &str, _category: crate::memory::MemoryCategory) -> anyhow::Result<()> {
    stats::inc_memories_unpinned();
    invalidate_stable_block();
    Ok(())
}
