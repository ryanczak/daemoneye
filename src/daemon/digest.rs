//! Session digest: structured compaction of conversation history.
//!
//! When the message count reaches [`DIGEST_THRESHOLD`], the digest builder scans
//! `events.jsonl` and the filesystem to produce a compact `[Session Digest]` block
//! that replaces the oldest messages.  This preserves the thread of reasoning
//! across long sessions without an extra LLM call.

use crate::ai::Message;
use crate::daemon::utils::log_event;
use chrono::{DateTime, Utc};
use std::path::Path;

/// When the in-memory message count reaches this threshold the next turn
/// triggers a digest-and-compact pass instead of plain tail trimming.
pub const DIGEST_THRESHOLD: usize = 30;

/// How many recent messages to keep after compaction.
/// Result layout: [first_message, digest_message, ...tail].
const TAIL_KEEP: usize = 16;

// ── Event tallies ────────────────────────────────────────────────────

#[derive(Default)]
struct EventTally {
    commands_ok: u32,
    commands_fail: u32,
    failed_cmds: Vec<(String, i32)>, // (cmd snippet, exit_code)
    files_edited: Vec<String>,
    prompt_tokens: u64,
    completion_tokens: u64,
    bg_windows_created: u32,
    bg_windows_closed: u32,
    alerts_received: Vec<String>,
    ghost_starts: u32,
    ghost_completions: u32,
}

/// Scan `events.jsonl` for events belonging to this session (or global events
/// like webhook alerts) that occurred after `since`.
fn tally_events(session_id: &str, since: DateTime<Utc>) -> EventTally {
    let mut t = EventTally::default();
    let path = crate::config::events_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            log::debug!("digest: could not read events.jsonl: {}", e);
            return t;
        }
    };

    let since_str = since.to_rfc3339();

    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // Skip events older than session start.
        let ts = v.get("ts").and_then(|t| t.as_str()).unwrap_or("");
        if ts < since_str.as_str() {
            continue;
        }

        let event = v.get("event").and_then(|e| e.as_str()).unwrap_or("");
        let ev_session = v.get("session").and_then(|s| s.as_str()).unwrap_or("");
        // Also check session_id field (used by ghost events).
        let ev_session_id = v.get("session_id").and_then(|s| s.as_str()).unwrap_or("");

        let belongs = ev_session == session_id
            || ev_session_id == session_id
            || ev_session == "-"
            || ev_session.is_empty();

        match event {
            "ai_turn" if belongs => {
                t.prompt_tokens += v.get("prompt_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
                t.completion_tokens += v
                    .get("completion_tokens")
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0);
            }
            "job_complete" if belongs => {
                let code = v.get("exit_code").and_then(|n| n.as_i64()).unwrap_or(-1) as i32;
                if code == 0 {
                    t.commands_ok += 1;
                } else {
                    t.commands_fail += 1;
                    let name = v
                        .get("job_name")
                        .and_then(|s| s.as_str())
                        .unwrap_or("?")
                        .to_string();
                    t.failed_cmds.push((name, code));
                }
            }
            "job_start" if belongs => {
                t.bg_windows_created += 1;
            }
            "gc_window" if belongs => {
                t.bg_windows_closed += 1;
            }
            "file_edit" if belongs => {
                if let Some(p) = v.get("path").and_then(|s| s.as_str()) {
                    t.files_edited.push(p.to_string());
                }
            }
            "webhook_alert" => {
                // Global events — always relevant.
                if let Some(name) = v.get("alert_name").and_then(|s| s.as_str()) {
                    t.alerts_received.push(name.to_string());
                }
            }
            "ghost_start" if belongs => {
                t.ghost_starts += 1;
            }
            "ghost_complete" if belongs => {
                t.ghost_completions += 1;
            }
            _ => {}
        }
    }

    t
}

// ── Artifact scanning ────────────────────────────────────────────────

struct ArtifactChanges {
    runbooks: Vec<String>,
    scripts: Vec<String>,
    memories: Vec<(String, String)>,  // (key, category)
    schedules: Vec<(String, String)>, // (name, kind description)
}

/// Scan the filesystem for artifacts created or modified since `since`.
fn scan_artifacts(since: DateTime<Utc>) -> ArtifactChanges {
    let since_systime: std::time::SystemTime = since.into();
    let mut changes = ArtifactChanges {
        runbooks: Vec::new(),
        scripts: Vec::new(),
        memories: Vec::new(),
        schedules: Vec::new(),
    };

    // Runbooks
    scan_dir_newer(
        &crate::runbook::runbooks_dir(),
        since_systime,
        &["md"],
        &mut changes.runbooks,
    );

    // Scripts (any extension)
    scan_dir_newer(
        &crate::scripts::scripts_dir(),
        since_systime,
        &[],
        &mut changes.scripts,
    );
    // Remove .meta.toml sidecar files from the list.
    changes.scripts.retain(|n| !n.ends_with(".meta.toml"));

    // Memories (three category subdirs)
    for (category, dir_name) in &[
        ("session", "session"),
        ("knowledge", "knowledge"),
        ("incident", "incidents"),
    ] {
        let dir = crate::config::config_dir().join("memory").join(dir_name);
        let mut keys = Vec::new();
        scan_dir_newer(&dir, since_systime, &["md"], &mut keys);
        for key in keys {
            changes.memories.push((key, category.to_string()));
        }
    }

    // Schedules — check created_at field in schedules.json.
    if let Ok(text) = std::fs::read_to_string(crate::config::Config::schedules_path())
        && let Ok(jobs) = serde_json::from_str::<Vec<serde_json::Value>>(&text)
    {
        for job in &jobs {
            let created = job
                .get("created_at")
                .and_then(|s| s.as_str())
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc));
            if let Some(created_at) = created
                && created_at >= since
            {
                let name = job
                    .get("name")
                    .and_then(|s| s.as_str())
                    .unwrap_or("?")
                    .to_string();
                let kind = job
                    .get("kind")
                    .and_then(|k| k.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("?")
                    .to_string();
                changes.schedules.push((name, kind));
            }
        }
    }

    changes
}

/// List files in `dir` whose mtime is >= `since`, collecting stem names.
/// If `extensions` is non-empty, only files with a matching extension are included.
fn scan_dir_newer(
    dir: &Path,
    since: std::time::SystemTime,
    extensions: &[&str],
    out: &mut Vec<String>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let Ok(mtime) = meta.modified() else {
            continue;
        };
        if mtime < since {
            continue;
        }
        if !extensions.is_empty() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !extensions.contains(&ext) {
                continue;
            }
        }
        let name = entry
            .path()
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if !name.is_empty() {
            out.push(name);
        }
    }
    out.sort();
}

// ── Digest formatting ────────────────────────────────────────────────

/// Build the `[Session Digest]` text block from event tallies and artifact scans.
pub fn build_session_digest(
    session_id: &str,
    since: DateTime<Utc>,
    message_count: usize,
) -> String {
    log_event(
        "session_digest_start",
        serde_json::json!({
            "session": session_id,
            "message_count": message_count,
            "since": since.to_rfc3339(),
        }),
    );

    let tally = tally_events(session_id, since);

    log_event(
        "session_digest_events_scanned",
        serde_json::json!({
            "session": session_id,
            "commands_ok": tally.commands_ok,
            "commands_fail": tally.commands_fail,
            "files_edited": tally.files_edited.len(),
            "alerts": tally.alerts_received.len(),
            "ghosts": tally.ghost_starts,
        }),
    );

    let artifacts = scan_artifacts(since);

    let artifact_count = artifacts.runbooks.len()
        + artifacts.scripts.len()
        + artifacts.memories.len()
        + artifacts.schedules.len();

    log_event(
        "session_digest_artifacts_found",
        serde_json::json!({
            "session": session_id,
            "runbooks": artifacts.runbooks.len(),
            "scripts": artifacts.scripts.len(),
            "memories": artifacts.memories.len(),
            "schedules": artifacts.schedules.len(),
        }),
    );

    let mut out = format!(
        "[Session Digest — {} messages compacted]\n\n",
        message_count
    );

    // Commands summary
    let total_cmds = tally.commands_ok + tally.commands_fail;
    if total_cmds > 0 {
        out.push_str(&format!(
            "Commands executed: {} ({} succeeded, {} failed)\n",
            total_cmds, tally.commands_ok, tally.commands_fail
        ));
        for (name, code) in &tally.failed_cmds {
            out.push_str(&format!("  Failed: {} (exit {})\n", name, code));
        }
    }

    // Files edited
    if !tally.files_edited.is_empty() {
        out.push_str(&format!(
            "Files edited: {} ({})\n",
            tally.files_edited.len(),
            tally.files_edited.join(", ")
        ));
    }

    // Token usage
    if tally.prompt_tokens > 0 {
        out.push_str(&format!(
            "Token usage: ~{}k prompt / ~{}k completion\n",
            tally.prompt_tokens / 1000,
            tally.completion_tokens / 1000,
        ));
    }

    // Background windows
    if tally.bg_windows_created > 0 {
        let active = tally
            .bg_windows_created
            .saturating_sub(tally.bg_windows_closed);
        out.push_str(&format!(
            "Background windows: {} created, {} closed, {} may still be active\n",
            tally.bg_windows_created, tally.bg_windows_closed, active
        ));
    }

    // Alerts
    if !tally.alerts_received.is_empty() {
        out.push_str(&format!(
            "Alerts received: {} ({})\n",
            tally.alerts_received.len(),
            tally.alerts_received.join(", ")
        ));
    }

    // Ghost shells
    if tally.ghost_starts > 0 {
        out.push_str(&format!(
            "Ghost shells: {} started, {} completed\n",
            tally.ghost_starts, tally.ghost_completions
        ));
    }

    // Artifacts
    if artifact_count > 0 {
        out.push_str("\nArtifacts created/modified this session:\n");
        for name in &artifacts.runbooks {
            out.push_str(&format!("  Runbook: {}\n", name));
        }
        for name in &artifacts.scripts {
            out.push_str(&format!("  Script: {}\n", name));
        }
        for (key, cat) in &artifacts.memories {
            out.push_str(&format!("  Memory: {} [{}]\n", key, cat));
        }
    }

    // Schedule changes
    if !artifacts.schedules.is_empty() {
        out.push_str("\nSchedule changes:\n");
        for (name, kind) in &artifacts.schedules {
            out.push_str(&format!("  Added: \"{}\" ({})\n", name, kind));
        }
    }

    let digest_len = out.len();

    log_event(
        "session_digest_complete",
        serde_json::json!({
            "session": session_id,
            "digest_bytes": digest_len,
            "artifact_count": artifact_count,
        }),
    );

    out
}

// ── Message compaction ───────────────────────────────────────────────

/// Replace old messages with a digest, keeping the first message (system context)
/// and a tail of recent messages.
///
/// Layout: `[first_message] [digest_as_assistant] [tail…]`
///
/// The tail starts at an even index (user turn) to preserve role alternation.
pub fn compact_with_digest(messages: Vec<Message>, digest: &str) -> Vec<Message> {
    if messages.len() <= TAIL_KEEP + 2 {
        // Not enough messages to compact — return as-is.
        return messages;
    }

    let first = messages[0].clone();
    let digest_msg = Message {
        role: "assistant".to_string(),
        content: digest.to_string(),
        tool_calls: None,
        tool_results: None,
    };

    // How many tail messages to keep (at most TAIL_KEEP).
    let raw_tail_start = messages.len().saturating_sub(TAIL_KEEP);
    // Round up to even so the tail begins on a user message.
    let tail_start = if raw_tail_start.is_multiple_of(2) {
        raw_tail_start
    } else {
        (raw_tail_start + 1).min(messages.len())
    };

    let mut result = Vec::with_capacity(2 + messages.len() - tail_start);
    result.push(first);
    result.push(digest_msg);
    result.extend_from_slice(&messages[tail_start..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msg(role: &str, content: &str) -> Message {
        Message {
            role: role.to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_results: None,
        }
    }

    #[test]
    fn compact_preserves_first_and_tail() {
        // Build 32 messages: alternating user/assistant.
        let messages: Vec<Message> = (0..32)
            .map(|i| {
                let role = if i % 2 == 0 { "user" } else { "assistant" };
                make_msg(role, &format!("msg-{}", i))
            })
            .collect();

        let result = compact_with_digest(messages.clone(), "digest text");

        // First message preserved.
        assert_eq!(result[0].content, "msg-0");
        // Second is the digest.
        assert_eq!(result[1].content, "digest text");
        assert_eq!(result[1].role, "assistant");
        // Tail starts on a user message (even index in original).
        assert_eq!(result[2].role, "user");
        // Total should be 2 + TAIL_KEEP.
        assert_eq!(result.len(), 2 + TAIL_KEEP);
        // Last message is the original last.
        assert_eq!(result.last().unwrap().content, "msg-31");
    }

    #[test]
    fn compact_noop_when_too_few_messages() {
        let messages: Vec<Message> = (0..10)
            .map(|i| make_msg("user", &format!("msg-{}", i)))
            .collect();
        let result = compact_with_digest(messages.clone(), "digest");
        assert_eq!(result.len(), messages.len());
    }

    #[test]
    fn compact_tail_starts_on_user_turn() {
        // 34 messages: user at even indices, assistant at odd.
        let messages: Vec<Message> = (0..34)
            .map(|i| {
                let role = if i % 2 == 0 { "user" } else { "assistant" };
                make_msg(role, &format!("msg-{}", i))
            })
            .collect();

        let result = compact_with_digest(messages, "digest");
        // Index 2 in result should be the first tail message — must be a user turn.
        assert_eq!(result[2].role, "user");
    }

    #[test]
    fn digest_threshold_value() {
        // Sanity check: threshold is between TAIL_KEEP and MAX_HISTORY.
        assert!(DIGEST_THRESHOLD > TAIL_KEEP + 2);
        assert!(DIGEST_THRESHOLD < crate::daemon::session::MAX_HISTORY);
    }

    #[test]
    fn scan_dir_newer_filters_by_mtime() {
        let dir = tempfile::tempdir().unwrap();

        // Create a file with current time (should be included).
        let new_file = dir.path().join("new-item.md");
        std::fs::write(&new_file, "content").unwrap();

        // Create a file and backdate it (should be excluded).
        let old_file = dir.path().join("old-item.md");
        std::fs::write(&old_file, "content").unwrap();
        let old_time = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        filetime::set_file_mtime(&old_file, filetime::FileTime::from_system_time(old_time))
            .unwrap();

        let since = std::time::SystemTime::now() - std::time::Duration::from_secs(10);
        let mut names = Vec::new();
        scan_dir_newer(dir.path(), since, &["md"], &mut names);

        assert_eq!(names, vec!["new-item".to_string()]);
    }
}
