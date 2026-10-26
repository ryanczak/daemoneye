//! Memory extraction from a finished epoch: `extract_memories_from_epoch`.

use super::*;

/// Best-effort: never blocks or fails the epoch build. Returns the number of
/// memories written (for the log).
pub async fn extract_memories_from_epoch(
    session_id: &str,
    record: &EpochRecord,
    dropped: &[Message],
    config: &config::Config,
) -> u32 {
    if !config.compaction.extract_memories {
        return 0;
    }

    let user_text = crate::daemon::digest::format_messages_for_narrative(dropped);
    let model_entry = config.resolve_model(Some("digest"));
    let system = "You extract durable operational knowledge from an SRE assistant's\
        conversation. From the transcript chunk, output 0 to 3 facts that will\
        still be true and useful weeks from now (host quirks, root causes,\
        fixed configurations, learned procedures). Output STRICT JSON only:\
        [{\"name\":\"kebab-case-slug\",\"content\":\"one to three sentences\"}] or [].\
        No prose. Do NOT record transient state (current disk %, running PIDs),\
        speculation, or anything already obvious from the runbooks.";

    let summary =
        crate::daemon::digest::summarize_once(system, &user_text, model_entry, config).await;
    let Some(summary) = summary else {
        return 0;
    };

    let count = apply_extraction(&summary, session_id, record.seq);
    if count > 0 {
        crate::daemon::utils::log_event(
            "memory_extracted",
            serde_json::json!({"session": session_id, "epoch_seq": record.seq, "count": count}),
        );
    }
    count
}

/// Parse strict JSON, write create-if-absent memories, return count written.
pub(super) fn apply_extraction(json: &str, _session_id: &str, _epoch_seq: u32) -> u32 {
    let facts: Vec<ExtractedFact> = match serde_json::from_str(json) {
        Ok(f) => f,
        Err(e) => {
            log::debug!("Memory extraction parse failure: {}", e);
            return 0;
        }
    };
    if facts.len() > 3 {
        log::debug!(
            "Memory extraction: {} facts (max 3), skipping all",
            facts.len()
        );
        return 0;
    }

    let mut count = 0u32;
    for fact in facts {
        if fact.content.is_empty() {
            continue;
        }
        if crate::memory::read_memory(
            &fact.name,
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .is_ok()
        {
            continue;
        }
        let body = format!(
            "---\nsource: \"compaction\"\ncreated: \"{}\"\n---\n{}\n",
            chrono::Utc::now().to_rfc3339(),
            fact.content.trim(),
        );
        if crate::memory::add_memory(
            &fact.name,
            &body,
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .is_ok()
        {
            count += 1;
        }
    }
    count
}

#[derive(Debug, serde::Deserialize)]
struct ExtractedFact {
    name: String,
    content: String,
}
