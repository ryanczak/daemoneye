//! G5: Tiered memory prompt assembly.
//!
//! Stable ambient block (pinned + high-relevance, cached with TTL) and
//! dynamic turn-relevant block (computed per turn from tag overlap).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crate::config::Config;
use crate::daemon::stats;
use crate::memory::index;
use crate::memory::tags::SessionTags;
use crate::memory::{MemoryCategory, MemoryInfo, list_memories_with_tags};
use crate::util::UnpoisonExt;

/// Monotonic dirty sequence counter for pinned-memory changes.
static PINNED_DIRTY_SEQ: AtomicU64 = AtomicU64::new(0);

/// Increment the pinned dirty sequence (called on pinned-memory CRUD).
pub fn invalidate_stable_block() {
    PINNED_DIRTY_SEQ.fetch_add(1, Ordering::Relaxed);
}

/// Current dirty sequence value.
fn current_dirty_seq() -> u64 {
    PINNED_DIRTY_SEQ.load(Ordering::Relaxed)
}

/// Cached stable block content.
struct StableBlockCache {
    content: String,
    computed_at: Instant,
    dirty_seq: u64,
}

static STABLE_BLOCK: Mutex<Option<StableBlockCache>> = Mutex::new(None);

/// Compute the volatility weight for composite scoring.
fn volatility_weight(vol: &str) -> f64 {
    match vol {
        "static" => 1.0,
        "slow" => 0.7,
        "moderate" => 0.4,
        "fast" => 0.0,
        "episodic" => 0.5,
        _ => 0.7,
    }
}

/// Compute the composite score for a memory entry.
/// `composite = effective_confidence × volatility_weight × (1.0 + usefulness_score * 0.5)`
fn composite_score(info: &MemoryInfo) -> f64 {
    let eff = crate::memory::review::effective_confidence(info);
    let vol = info.volatility.as_deref().unwrap_or("slow");
    let vw = volatility_weight(vol);
    let usefulness = info.usefulness_score.unwrap_or(0.0).clamp(-1.0, 1.0);
    eff * vw * (1.0 + usefulness * 0.5)
}

/// Format a single memory entry for a prompt block.
/// Uses summary if available, otherwise first 200 chars of body.
fn format_memory_entry(info: &MemoryInfo) -> String {
    let summary = info
        .summary
        .as_deref()
        .unwrap_or("");
    let text = if !summary.is_empty() {
        summary.to_string()
    } else {
        // Try to read body from file
        let cat = MemoryCategory::from_str(&info.category).unwrap_or(MemoryCategory::Knowledge);
        let path = memory_dir(&cat).join(format!("{}.md", info.key));
        if let Ok(content) = std::fs::read_to_string(&path) {
            let (_, body) = crate::memory::parse_memory_frontmatter(&content);
            let truncated: String = body.chars().take(200).collect();
            truncated
        } else {
            format!("[memory: {}]", info.key)
        }
    };
    format!("--- {} ---\n{}\n", info.key, text.trim())
}

/// Assemble the stable ambient memory block.
/// Queries pinned memories first, fills remaining budget with top-scored memories.
/// Cached with TTL; rebuilds if TTL expired or pinned dirty sequence changed.
pub fn assemble_ambient_memory(config: &Config) -> Option<String> {
    let budget = config.memory.stable_block_budget;
    let ttl_secs = config.memory.stable_block_ttl;

    // Check cache validity
    {
        let guard = STABLE_BLOCK.lock().unwrap_or_log();
        if let Some(cache) = guard.as_ref() {
            let expired = cache.computed_at.elapsed().as_secs() >= ttl_secs;
            let dirty = cache.dirty_seq != current_dirty_seq();
            if !expired && !dirty {
                return Some(cache.content.clone());
            }
        }
    }

    // Rebuild needed
    stats::inc_stable_block_rebuilds();
    assemble_ambient_memory_rebuild(config, budget)
}

fn assemble_ambient_memory_rebuild(_config: &Config, budget: usize) -> Option<String> {
    let all_memories = list_memories_with_tags(None, &["global"]).unwrap_or_default();

    // Exclude archived memories
    let active: Vec<&MemoryInfo> = all_memories
        .iter()
        .filter(|m| {
            m.volatility.as_deref() != Some("_archive")
                && m.confidence.as_deref() != Some("_archive")
        })
        .collect();

    // Separate pinned and scored
    let pinned: Vec<&MemoryInfo> = active.iter().filter(|m| m.pinned.unwrap_or(false)).cloned().collect();
    let mut scored: Vec<&MemoryInfo> = active.iter().filter(|m| !m.pinned.unwrap_or(false)).cloned().collect();

    // Sort scored by composite score descending
    scored.sort_by(|a, b| {
        let sa = composite_score(a);
        let sb = composite_score(b);
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });

    // Build block: pinned first, then scored until budget exhausted
    let mut parts: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut count = 0usize;
    let mut dropped = 0usize;

    // Pinned memories (never truncated)
    for info in &pinned {
        let entry = format_memory_entry(info);
        total += entry.len();
        parts.push(entry);
        count += 1;
    }

    // Scored memories until budget
    for info in &scored {
        let entry = format_memory_entry(info);
        if total + entry.len() <= budget {
            total += entry.len();
            parts.push(entry);
            count += 1;
        } else {
            dropped += 1;
        }
    }

    if dropped > 0 {
        log::warn!(
            "Stable memory budget exceeded: dropped {} memories ({} bytes used, {} budget)",
            dropped,
            total,
            budget
        );
    }

    if parts.is_empty() {
        return None;
    }

    let header = format!("[AMBIENT MEMORY] {} memories, {} bytes\n", count, total);
    let block = format!("{}\n{}", header, parts.join("\n"));

    // Update cache
    {
        let mut guard = STABLE_BLOCK.lock().unwrap_or_log();
        *guard = Some(StableBlockCache {
            content: block.clone(),
            computed_at: Instant::now(),
            dirty_seq: current_dirty_seq(),
        });
    }

    stats::set_memories_in_stable_block(count);
    Some(block)
}

/// Assemble the dynamic turn-relevant memory block.
/// Computed fresh per turn from tag overlap + FTS5 search.
pub fn assemble_turn_relevant_memory(
    session_tags: &SessionTags,
    user_turn: &str,
    config: &Config,
    session_id: Option<&str>,
    turn: usize,
    namespaces: &[&str],
) -> Option<String> {
    let budget = config.memory.dynamic_block_budget;
    let threshold = config.memory.threshold_dynamic_block;
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
    let mut candidate_keys: std::collections::HashMap<String, f64> = std::collections::HashMap::new();

    // Tag overlap score: overlap_count / tags.len()
    let tag_set: std::collections::HashSet<&String> = all_tags.iter().collect();
    for info in &tag_candidates {
        let overlap = info.tags.iter().filter(|t| tag_set.contains(t)).count();
        let score = overlap as f64 / all_tags.len().max(1) as f64;
        let eff = crate::memory::review::effective_confidence(info);
        let combined = score * eff;
        candidate_keys.entry(info.key.clone()).or_insert(0.0);
        *candidate_keys.get_mut(&info.key).unwrap() = combined;
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
    let keys: Vec<String> = scored.iter().map(|(info, _)| info.key.clone()).take(count).collect();
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
        let overlap = info.tags.iter().filter(|t| tag_set.contains(t.as_str())).count();
        if overlap > 0 {
            let score = overlap as f64 / tags.len().max(1) as f64;
            let eff = crate::memory::review::effective_confidence(&info);
            scored.push((info, score * eff));
        }
    }

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(limit).map(|(info, _)| info).collect()
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
pub fn pin_memory(key: &str, category: crate::memory::MemoryCategory) -> anyhow::Result<()> {
    crate::memory::update_memory(
        key,
        category,
        None,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(true),
        None,
        None,
        None,
    )?;
    stats::inc_memories_pinned();
    invalidate_stable_block();
    Ok(())
}

/// Unpin a memory entry: set pinned=false, update index, invalidate cache.
pub fn unpin_memory(key: &str, category: crate::memory::MemoryCategory) -> anyhow::Result<()> {
    crate::memory::update_memory(
        key,
        category,
        None,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(false),
        None,
        None,
        None,
    )?;
    stats::inc_memories_unpinned();
    invalidate_stable_block();
    Ok(())
}
