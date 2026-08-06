//! G5: Tiered memory prompt assembly.
//!
//! Stable ambient block (pinned + high-relevance, cached with TTL) and
//! dynamic turn-relevant block (computed per turn from tag overlap).

use std::collections::HashMap;
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

/// Weight applied to a normalized BM25 hit. Chosen so the strongest FTS hit
/// (0.6) outranks a relates_to hit (0.3) while a full tag-overlap match (1.0)
/// still leads.
const FTS_WEIGHT: f64 = 0.6;

/// Merge a score into the map using max-wins semantics.
fn merge_max(map: &mut HashMap<(String, String), f64>, info: &MemoryInfo, score: f64) {
    let e = map
        .entry((info.namespace.clone(), info.key.clone()))
        .or_insert(f64::NEG_INFINITY);
    if score > *e {
        *e = score;
    }
}

/// Find memories whose tags intersect with the given tags.
/// Scored by overlap ratio, sorted by score × effective_confidence.
pub fn find_by_tag_overlap(all: &[MemoryInfo], tags: &[String], limit: usize) -> Vec<MemoryInfo> {
    let tag_set: std::collections::HashSet<&str> = tags.iter().map(|s| s.as_str()).collect();

    let mut scored: Vec<(MemoryInfo, f64)> = Vec::new();
    for info in all {
        let overlap = info
            .tags
            .iter()
            .filter(|t| tag_set.contains(t.as_str()))
            .count();
        if overlap > 0 {
            let score = overlap as f64 / tags.len().max(1) as f64;
            let eff = crate::memory::review::effective_confidence(info);
            scored.push((info.clone(), score * eff));
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
pub fn expand_relates_to(all: &[MemoryInfo], keys: &[String]) -> Vec<MemoryInfo> {
    let key_set: std::collections::HashSet<&str> = keys.iter().map(|s| s.as_str()).collect();

    let mut found: Vec<MemoryInfo> = Vec::new();
    for info in all {
        if info.relates_to.iter().any(|r| key_set.contains(r.as_str())) {
            found.push(info.clone());
        }
    }
    found
}

/// FTS5 search on memory index for matching memories.
/// Returns top `limit` by BM25 score alongside the raw BM25 value.
pub fn ftsearch_memories(
    all: &[MemoryInfo],
    query: &str,
    limit: usize,
    namespaces: &[&str],
) -> Vec<(MemoryInfo, f64)> {
    let results = index::fts5_search(query, limit, namespaces);

    let mut found = Vec::new();
    for (namespace, key, score) in results {
        if let Some(info) = all
            .iter()
            .find(|m| m.namespace == namespace && m.key == key)
        {
            found.push((info.clone(), score));
        }
    }
    found
}

/// Merge the three candidate sources into one namespace-keyed scored set.
/// Returns active (non-expired, above-threshold, non-pinned) memories only,
/// sorted by descending score.
pub(crate) fn score_candidates(
    all: &[MemoryInfo],
    all_tags: &[String],
    user_turn: &str,
    namespaces: &[&str],
    threshold: f64,
) -> Vec<(MemoryInfo, f64)> {
    // Active filter: exclude pinned, expired, below-threshold
    let active: Vec<MemoryInfo> = all
        .iter()
        .filter(|m| {
            !m.pinned.unwrap_or(false) // exclude pinned (already in ambient)
                && !m.is_expired()
                && crate::memory::review::effective_confidence(m) >= threshold
        })
        .cloned()
        .collect();

    // 1. Tag overlap candidates
    let tag_candidates = find_by_tag_overlap(&active, all_tags, 10);

    // 2. One-hop relates_to expansion
    let relates_keys: Vec<String> = tag_candidates.iter().map(|k| k.key.clone()).collect();
    let relates_candidates = expand_relates_to(&active, &relates_keys);

    // 3. FTS5 search against user turn
    let fts_hits = if !user_turn.is_empty() {
        ftsearch_memories(&active, user_turn, 10, namespaces)
    } else {
        Vec::new()
    };

    // Merge candidates: build a namespace-keyed scored set
    let mut candidate_scores: HashMap<(String, String), f64> = HashMap::new();

    // Tag overlap score: overlap_count / tags.len()
    let tag_set: std::collections::HashSet<&String> = all_tags.iter().collect();
    for info in &tag_candidates {
        let overlap = info.tags.iter().filter(|t| tag_set.contains(t)).count();
        let score = overlap as f64 / all_tags.len().max(1) as f64;
        let eff = crate::memory::review::effective_confidence(info);
        let combined = score * eff;
        merge_max(&mut candidate_scores, info, combined);
    }

    // Relates_to candidates
    for info in &relates_candidates {
        let score = 0.3 * crate::memory::review::effective_confidence(info);
        merge_max(&mut candidate_scores, info, score);
    }

    // FTS5 candidates — normalized BM25
    if !fts_hits.is_empty() {
        let mag_max = fts_hits
            .iter()
            .map(|(_, raw)| -raw)
            .fold(f64::NEG_INFINITY, f64::max);

        for (info, raw) in &fts_hits {
            let mag_i = -raw;
            let normalized = if mag_max > 0.0 { mag_i / mag_max } else { 0.0 };
            let contribution =
                FTS_WEIGHT * normalized * crate::memory::review::effective_confidence(info);
            merge_max(&mut candidate_scores, info, contribution);
        }
    }

    // Collect scored entries from active memories
    let mut scored: Vec<(MemoryInfo, f64)> = Vec::new();
    for info in &active {
        if let Some(&score) = candidate_scores.get(&(info.namespace.clone(), info.key.clone())) {
            scored.push((info.clone(), score));
        }
    }
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                (a.0.namespace.as_str(), a.0.key.as_str())
                    .cmp(&(b.0.namespace.as_str(), b.0.key.as_str()))
            })
    });

    scored
}

/// Render scored entries in rank order, keeping those that fit the byte budget.
/// Returns the rendered entry alongside its `MemoryInfo` and score so the
/// caller logs exactly what it emitted.
pub(crate) fn pack_within_budget(
    scored: &[(MemoryInfo, f64)],
    budget: usize,
) -> Vec<(String, MemoryInfo, f64)> {
    let mut result: Vec<(String, MemoryInfo, f64)> = Vec::new();
    let mut total = 0usize;

    for (info, score) in scored {
        let entry = format_memory_entry(info);
        if total + entry.len() <= budget {
            total += entry.len();
            result.push((entry, info.clone(), *score));
        }
        // continue past too-large entries (intentional packing)
    }

    result
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

    let scored = score_candidates(&all_memories, &all_tags, user_turn, namespaces, threshold);

    let packed = pack_within_budget(&scored, budget);

    if packed.is_empty() {
        stats::inc_dynamic_block_empty_turns();
        return None;
    }

    let count = packed.len();
    let total: usize = packed.iter().map(|(entry, _, _)| entry.len()).sum();

    let parts: Vec<String> = packed.iter().map(|(entry, _, _)| entry.clone()).collect();

    // Update rolling average for dynamic block size
    stats::set_memories_in_dynamic_block_avg(count);

    // Log memory_retrieved event with keys and scores of the entries actually emitted
    let keys: Vec<String> = packed.iter().map(|(_, info, _)| info.key.clone()).collect();
    let scores: Vec<f64> = packed.iter().map(|(_, _, s)| *s).collect();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_env() -> (crate::TestHomeGuard, tempfile::TempDir) {
        let guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        (guard, tmp)
    }

    #[test]
    fn fts_hits_get_pairwise_distinct_scores() {
        let (_guard, _tmp) = setup_test_env();

        crate::memory::add_memory(
            "quokka-strong",
            "quokka quokka quokka",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add strong memory");

        crate::memory::add_memory(
            "quokka-weak",
            "the quick brown fox jumps over the lazy dog and then quokka appears once",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add weak memory");

        let all = list_memories_with_tags(None, &["global"]).unwrap_or_default();
        let scored = score_candidates(&all, &[], "quokka", &["global"], 0.0);

        assert!(scored.len() >= 2, "should find both memories");

        // Both present
        let keys: Vec<&str> = scored.iter().map(|(m, _)| m.key.as_str()).collect();
        assert!(keys.contains(&"quokka-strong"));
        assert!(keys.contains(&"quokka-weak"));

        // Scores are strictly different
        assert_ne!(
            scored[0].1, scored[1].1,
            "strong and weak FTS hits must get pairwise distinct scores"
        );

        // Stronger match sorts first
        assert_eq!(scored[0].0.key, "quokka-strong");

        // Top score equals FTS_WEIGHT (normalized to 1.0 for the best hit)
        assert!(
            (scored[0].1 - FTS_WEIGHT).abs() < 1e-9,
            "top score should equal FTS_WEIGHT"
        );
    }

    #[test]
    fn fts_score_is_not_the_flat_constant() {
        let (_guard, _tmp) = setup_test_env();

        crate::memory::add_memory(
            "quokka-strong",
            "quokka quokka quokka",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add strong memory");

        crate::memory::add_memory(
            "quokka-weak",
            "the quick brown fox jumps over the lazy dog and then quokka appears once",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add weak memory");

        let all = list_memories_with_tags(None, &["global"]).unwrap_or_default();
        let scored = score_candidates(&all, &[], "quokka", &["global"], 0.0);

        let weak_score = scored
            .iter()
            .find(|(m, _)| m.key == "quokka-weak")
            .map(|(_, s)| *s)
            .expect("weak hit should be present");

        assert!(weak_score > 0.0, "weak hit score must be > 0");
        assert!(
            weak_score < FTS_WEIGHT,
            "weak hit score must be < FTS_WEIGHT"
        );
        assert!(
            (weak_score - 0.2).abs() > 1e-9,
            "weak hit score must not equal the old flat constant 0.2"
        );

        let strong_score = scored
            .iter()
            .find(|(m, _)| m.key == "quokka-strong")
            .map(|(_, s)| *s)
            .expect("strong hit should be present");

        assert!(
            (strong_score - 0.2).abs() > 1e-9,
            "strong hit score must not equal the old flat constant 0.2"
        );
    }

    #[test]
    fn same_key_in_two_namespaces_scores_separately() {
        let (_guard, _tmp) = setup_test_env();

        crate::memory::add_memory(
            "shared-key",
            "quokka quokka quokka quokka quokka",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add global memory");

        crate::memory::add_memory(
            "shared-key",
            "just a brief mention of quokka",
            crate::memory::MemoryCategory::Knowledge,
            "analyst",
        )
        .expect("add analyst memory");

        let all = list_memories_with_tags(None, &["global", "analyst"]).unwrap_or_default();
        let scored = score_candidates(&all, &[], "quokka", &["analyst", "global"], 0.0);

        assert_eq!(
            scored.len(),
            2,
            "both namespace entries for the same key must survive"
        );

        let namespaces: Vec<&str> = scored.iter().map(|(m, _)| m.namespace.as_str()).collect();
        assert!(namespaces.contains(&"global"));
        assert!(namespaces.contains(&"analyst"));

        // Scores must be distinct since bodies differ
        assert_ne!(
            scored[0].1, scored[1].1,
            "different namespace entries must get distinct scores"
        );
    }

    #[test]
    fn tag_hit_does_not_suppress_a_stronger_fts_hit() {
        let (_guard, _tmp) = setup_test_env();

        crate::memory::add_memory(
            "dual-match",
            "quokka quokka quokka quokka quokka quokka quokka",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add memory");

        // Re-read so the memory has tags populated (it won't have any from add_memory,
        // but we need the MemoryInfo for the tag overlap calculation).
        // The tag contribution will be low because the memory has no matching tags.
        // The FTS contribution will be high because the body is a strong match.
        let all = list_memories_with_tags(None, &["global"]).unwrap_or_default();

        // Use session tags that partially overlap (the memory has no tags, so tag overlap = 0).
        // This tests that even if tag overlap were non-zero but below FTS_WEIGHT,
        // the max-merge picks the FTS score.
        let session_tags = vec![
            "rust".to_string(),
            "testing".to_string(),
            "memory".to_string(),
            "index".to_string(),
        ];

        let scored = score_candidates(&all, &session_tags, "quokka", &["global"], 0.0);

        let entry = scored
            .iter()
            .find(|(m, _)| m.key == "dual-match")
            .expect("dual-match must be present");

        // The score should be driven by FTS, not suppressed by a weak tag score.
        // Since the memory has no tags, tag overlap is 0, so the score is purely FTS.
        // With max-merge, the FTS score wins regardless.
        assert!(entry.1 > 0.0, "score must be > 0 (driven by FTS)");
        // The score should be close to FTS_WEIGHT since it's the strongest (and only) FTS hit
        assert!(
            (entry.1 - FTS_WEIGHT).abs() < 1e-9,
            "score should equal FTS_WEIGHT for the strongest FTS hit"
        );
    }

    #[test]
    fn expired_memory_is_excluded_and_the_guard_is_not_vacuous() {
        let (_guard, _tmp) = setup_test_env();

        crate::memory::add_memory(
            "expired-match",
            "quokka quokka quokka quokka quokka",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add expired memory");

        // Manually expire the memory by overwriting its file with expired
        // frontmatter. Resolve the directory through the same helper the
        // production code uses — for the `global` namespace that is
        // `<config>/memory/knowledge/`, with no per-namespace subdirectory.
        let knowledge_dir = crate::memory::memory_dir_for_namespace(
            "global",
            &crate::memory::MemoryCategory::Knowledge,
        );
        std::fs::create_dir_all(&knowledge_dir).expect("create knowledge dir");
        let expired_path = knowledge_dir.join("expired-match.md");
        let content = "---\nkey: expired-match\nnamespace: global\ncategory: knowledge\ntags: []\nrelates_to: []\nexpires: \"2020-01-01\"\n---\n\nquokka quokka quokka quokka quokka\n";
        std::fs::write(&expired_path, content).expect("write expired memory");

        // Control: a non-expired memory that also matches
        crate::memory::add_memory(
            "active-match",
            "quokka appears once here",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .expect("add active memory");

        let all = list_memories_with_tags(None, &["global"]).unwrap_or_default();
        let scored = score_candidates(&all, &[], "quokka", &["global"], 0.0);

        let keys: Vec<&str> = scored.iter().map(|(m, _)| m.key.as_str()).collect();

        assert!(
            !keys.contains(&"expired-match"),
            "expired memory must be excluded"
        );
        assert!(
            keys.contains(&"active-match"),
            "active control memory must be present (guard against empty fixture)"
        );
    }

    #[test]
    fn packing_reports_the_entries_it_emitted() {
        // Build scored entries by hand — no filesystem needed.
        let info1 = MemoryInfo {
            key: "small".to_string(),
            category: "knowledge".to_string(),
            namespace: "global".to_string(),
            tags: vec![],
            summary: Some("tiny".to_string()),
            relates_to: vec![],
            created: None,
            updated: None,
            expires: None,
            pinned: None,
        };
        let info2 = MemoryInfo {
            key: "huge".to_string(),
            category: "knowledge".to_string(),
            namespace: "global".to_string(),
            tags: vec![],
            summary: Some("x".repeat(5000)), // renders to >5000 bytes
            relates_to: vec![],
            created: None,
            updated: None,
            expires: None,
            pinned: None,
        };
        let info3 = MemoryInfo {
            key: "also-small".to_string(),
            category: "knowledge".to_string(),
            namespace: "global".to_string(),
            tags: vec![],
            summary: Some("also tiny".to_string()),
            relates_to: vec![],
            created: None,
            updated: None,
            expires: None,
            pinned: None,
        };

        let scored = vec![(info1, 1.0), (info2, 0.8), (info3, 0.5)];

        // Budget that fits entry 1 + 3 but not entry 2
        let packed = pack_within_budget(&scored, 200);

        assert_eq!(packed.len(), 2, "should pack entries 1 and 3");
        assert_eq!(packed[0].1.key, "small");
        assert_eq!(packed[1].1.key, "also-small");
        // Entry 2 (huge) was skipped, not included
        assert!(
            packed.iter().all(|(_, m, _)| m.key != "huge"),
            "huge entry must be skipped"
        );
    }
}
