//! Session digest: structured compaction of conversation history.
//!
//! Compaction is driven by prompt-token pressure in [`crate::daemon::server`].
//! When tokens cross the elision threshold, [`elide_old_tool_results`] condenses
//! oversized tool outputs in older turns.  When pressure crosses the digest
//! threshold, [`build_session_digest`] scans `events.jsonl` and the filesystem
//! to produce a compact `[Session Digest]` block that replaces the oldest
//! messages via [`compact_with_digest`].  [`DIGEST_THRESHOLD`] is the minimum
//! message count required before either pass may fire — a small floor so very
//! short sessions with token-heavy first turns don't compact prematurely.
//!
//! The digest is *hybrid*: [`build_narrative_summary`] calls a cheap model (the
//! optional `digest` config entry, falling back to `default`) to turn the
//! about-to-be-dropped turns into a short natural-language narrative capturing
//! causal threads.  That narrative is prepended to the deterministic structured
//! tally.  The narrative step is best-effort — if it times out or errors, the
//! structured digest still fires.

use crate::ai::Message;
use crate::daemon::utils::log_event;
use chrono::{DateTime, Utc};
use std::path::Path;
use std::time::Duration;

/// Minimum number of in-memory messages required before token-pressure-triggered
/// compaction may fire.  Must exceed `TAIL_KEEP + 2` so the digest has
/// something to compact.
pub const DIGEST_THRESHOLD: usize = 20;

/// How many recent messages to keep after compaction.
/// Result layout: [first_message, digest_message, ...tail].
const TAIL_KEEP: usize = 16;

/// Tool results larger than this many characters are replaced with a short
/// placeholder during elision.  Roughly ~750 tokens at 4 chars/token; short
/// results (file snippets, single-line outputs) stay intact.
const ELIDE_THRESHOLD_CHARS: usize = 3000;

/// Number of most-recent messages left untouched during elision — the model
/// still sees full tool output for the current investigation thread.
const ELISION_TAIL_KEEP: usize = 8;

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

// ── Narrative summary (hybrid digest) ────────────────────────────────

/// Hard cap on how long the narrative-summary call may run before we fall back
/// to a structured-only digest.  The model is supposed to be cheap; a long
/// delay almost certainly means the backend is degraded, and we'd rather ship
/// compaction on time than stall an interactive turn.
const NARRATIVE_TIMEOUT: Duration = Duration::from_secs(20);

/// Upper bound on how much raw text we hand to the summarizer.  The
/// narrative-generating model is typically small (Haiku / gpt-4o-mini /
/// gemini-flash) and doesn't need the full history — a representative slice
/// is enough.  Pick a size that fits comfortably inside a 32k-token window.
const NARRATIVE_INPUT_CHAR_BUDGET: usize = 60_000;

const NARRATIVE_SYSTEM_PROMPT: &str = "\
You are the context summarizer for an SRE assistant.  You will be shown a \
chunk of conversation — user turns, assistant replies, tool calls, and tool \
results — that is about to be dropped from active context to free tokens.  \
Write a short chronological narrative that preserves what the structured \
tally (command counts, file lists, token totals) cannot: the causal thread \
and any semantic state the assistant will need next turn.

Cover, in 8–15 lines total:
- What the user was investigating or trying to accomplish.
- Key findings, conclusions, or decisions.
- State changes that matter later (scripts written, runbooks created, \
  schedules added, knowledge learned).
- Anything left unresolved or still pending.

Rules:
- Use past tense.  Be terse.  Bullet points or short paragraphs are fine.
- Do NOT enumerate tool calls one-by-one — a tally block follows.
- Do NOT fabricate details that aren't in the transcript.
- If the chunk is too sparse to summarize, respond with exactly: \
  `No narrative summary — chunk too sparse.`
- Respond with ONLY the narrative.  No preamble, no closing remarks.";

/// Serialize a slice of messages into the compact transcript fed to the
/// narrative model.  Tool results are shortened aggressively — the narrative
/// step cares about the *arc* of the investigation, not raw bytes.
fn format_messages_for_narrative(messages: &[Message]) -> String {
    let mut out = String::new();
    for m in messages {
        match m.role.as_str() {
            "user" if m.tool_results.is_some() => {
                if let Some(results) = &m.tool_results {
                    for r in results {
                        let preview = if r.content.len() > 400 {
                            format!("{}…", &r.content[..400])
                        } else {
                            r.content.clone()
                        };
                        out.push_str(&format!("[tool_result {}] {}\n", r.tool_name, preview));
                    }
                }
            }
            "user" => {
                if !m.content.is_empty() {
                    out.push_str("USER: ");
                    out.push_str(&m.content);
                    out.push('\n');
                }
            }
            "assistant" => {
                if !m.content.is_empty() {
                    out.push_str("ASSISTANT: ");
                    out.push_str(&m.content);
                    out.push('\n');
                }
                if let Some(calls) = &m.tool_calls {
                    for c in calls {
                        let arg_preview = if c.arguments.len() > 200 {
                            format!("{}…", &c.arguments[..200])
                        } else {
                            c.arguments.clone()
                        };
                        out.push_str(&format!("[tool_call {}] {}\n", c.name, arg_preview));
                    }
                }
            }
            _ => {}
        }
        if out.len() >= NARRATIVE_INPUT_CHAR_BUDGET {
            out.push_str("\n[…truncated to fit summarizer budget…]\n");
            break;
        }
    }
    out
}

/// Generate a natural-language narrative of the messages about to be dropped.
/// Returns `None` on any failure — callers must tolerate an absent narrative
/// and fall back to the structured-only digest.
///
/// Uses the configured `digest` model entry if present, falling back to
/// `default`.  Runs with [`NARRATIVE_TIMEOUT`] so a degraded backend cannot
/// stall interactive turns.
pub async fn build_narrative_summary(
    messages: &[Message],
    model_entry: &crate::config::ModelEntry,
) -> Option<String> {
    if messages.is_empty() {
        return None;
    }

    let transcript = format_messages_for_narrative(messages);
    if transcript.trim().is_empty() {
        return None;
    }

    let user_msg = Message {
        role: "user".to_string(),
        content: format!(
            "Here is the conversation chunk to summarize:\n\n{}",
            transcript
        ),
        tool_calls: None,
        tool_results: None,
        turn: None,
    };

    let client = crate::ai::make_client(
        &model_entry.provider,
        model_entry.resolve_api_key(),
        model_entry.model.clone(),
        model_entry.effective_base_url(),
    );

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<crate::ai::AiEvent>();
    let system = NARRATIVE_SYSTEM_PROMPT.to_string();
    let msgs = vec![user_msg];

    // Race the chat call against a timeout.  On success or failure we still
    // drain the channel (via the receiver loop below) so no tokens are lost.
    let chat_fut = client.chat(&system, msgs, tx, false);
    let chat_result = tokio::time::timeout(NARRATIVE_TIMEOUT, chat_fut).await;

    match chat_result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            log::warn!("digest narrative: backend error ({}), skipping narrative", e);
            return None;
        }
        Err(_) => {
            log::warn!(
                "digest narrative: timed out after {}s, skipping narrative",
                NARRATIVE_TIMEOUT.as_secs()
            );
            return None;
        }
    }

    let mut text = String::new();
    while let Some(ev) = rx.recv().await {
        if let crate::ai::AiEvent::Token(t) = ev {
            text.push_str(&t);
        }
    }

    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

// ── Digest formatting ────────────────────────────────────────────────

/// Build the `[Session Digest]` text block from event tallies and artifact scans.
///
/// If `narrative` is `Some`, it is prepended as the first section after the
/// header — the narrative is the human-readable story, the tally is the
/// authoritative numbers.  Pass `None` to emit the structured-only digest.
pub fn build_session_digest(
    session_id: &str,
    since: DateTime<Utc>,
    message_count: usize,
    narrative: Option<&str>,
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

    if let Some(narrative) = narrative {
        let trimmed = narrative.trim();
        if !trimmed.is_empty() {
            out.push_str("Narrative:\n");
            out.push_str(trimmed);
            out.push_str("\n\n");
        }
    }

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

/// Predict where [`compact_with_digest`] will cut the message vec.
///
/// Returns the index of the first message in the preserved tail (i.e. the
/// boundary between "dropped" and "kept") when compaction is feasible, or
/// `None` when the history is too short or lacks a clean turn boundary.
///
/// Useful for callers (e.g. the server's compaction block) that need to know
/// which messages are about to be dropped so they can feed that slice to
/// [`build_narrative_summary`] before the digest is built.
pub fn planned_tail_start(messages: &[Message]) -> Option<usize> {
    if messages.len() <= TAIL_KEEP + 2 {
        return None;
    }
    let raw_tail_start = messages.len().saturating_sub(TAIL_KEEP);
    crate::daemon::session::next_clean_turn_start(messages, raw_tail_start)
}

/// Replace old messages with a digest, keeping the first message (system context)
/// and a tail of recent messages.
///
/// Layout: `[first_message] [digest_as_assistant] [tail…]`
///
/// The tail starts at an even index (user turn) to preserve role alternation.
pub fn compact_with_digest(messages: Vec<Message>, digest: &str) -> Vec<Message> {
    let Some(tail_start) = planned_tail_start(&messages) else {
        if messages.len() > TAIL_KEEP + 2 {
            log::warn!(
                "compact_with_digest: no clean turn boundary found — skipping compaction"
            );
        }
        return messages;
    };

    let first = messages[0].clone();
    let digest_msg = Message {
        role: "assistant".to_string(),
        content: digest.to_string(),
        tool_calls: None,
        tool_results: None,
        turn: None,
    };

    let mut result = Vec::with_capacity(2 + messages.len() - tail_start);
    result.push(first);
    result.push(digest_msg);
    result.extend_from_slice(&messages[tail_start..]);
    result
}

/// Replace oversized tool_results in older messages with a short placeholder.
/// This preserves turn structure — the agent still sees which tool was called
/// and when — while freeing context occupied by stale, verbose output
/// (e.g. file dumps, directory listings, full command logs).
///
/// The most recent `ELISION_TAIL_KEEP` messages are left untouched so the
/// active investigation thread keeps its full fidelity.  Returns the number
/// of characters removed so callers can log the savings.
pub fn elide_old_tool_results(messages: &mut [Message]) -> usize {
    if messages.len() <= ELISION_TAIL_KEEP + 1 {
        return 0;
    }
    let elide_until = messages.len() - ELISION_TAIL_KEEP;
    let mut removed = 0usize;
    for msg in messages.iter_mut().take(elide_until) {
        let Some(results) = msg.tool_results.as_mut() else {
            continue;
        };
        for r in results.iter_mut() {
            if r.content.len() > ELIDE_THRESHOLD_CHARS {
                let orig_len = r.content.len();
                let replacement = format!(
                    "[elided: tool `{}` produced {} chars; outside live context window. \
                     See events.jsonl for full output.]",
                    r.tool_name, orig_len
                );
                removed += orig_len.saturating_sub(replacement.len());
                r.content = replacement;
            }
        }
    }
    removed
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
            turn: None,
        }
    }

    fn msg_with_tool_result(tool_name: &str, content: &str) -> Message {
        use crate::ai::ToolResult;
        Message {
            role: "user".to_string(),
            content: String::new(),
            tool_calls: None,
            tool_results: Some(vec![ToolResult {
                tool_call_id: "id".to_string(),
                tool_name: tool_name.to_string(),
                content: content.to_string(),
            }]),
            turn: None,
        }
    }

    #[test]
    fn elide_condenses_old_oversized_tool_results() {
        let big = "X".repeat(ELIDE_THRESHOLD_CHARS + 100);
        let mut messages = vec![make_msg("user", "first turn")];
        // 4 pairs of user/assistant with an oversized tool result on each user msg.
        // ELISION_TAIL_KEEP = 8, so we need more than 9 messages to get any elision.
        for i in 0..12 {
            if i % 2 == 0 {
                messages.push(msg_with_tool_result("read_file", &big));
            } else {
                messages.push(make_msg("assistant", "ack"));
            }
        }

        let removed = elide_old_tool_results(&mut messages);

        assert!(removed > 0, "expected some chars to be elided");
        // Tail is last 8 messages — their tool_results should still contain the big content.
        let tail_start = messages.len() - ELISION_TAIL_KEEP;
        for (i, msg) in messages.iter().enumerate() {
            if let Some(results) = &msg.tool_results {
                for r in results {
                    if i < tail_start {
                        assert!(
                            r.content.starts_with("[elided:"),
                            "msg {} should be elided",
                            i
                        );
                    } else {
                        assert_eq!(
                            r.content.len(),
                            ELIDE_THRESHOLD_CHARS + 100,
                            "tail msg {} should keep full content",
                            i
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn elide_leaves_small_results_intact() {
        let small = "ok".to_string();
        let mut messages: Vec<Message> = (0..16)
            .map(|i| {
                if i % 2 == 0 {
                    msg_with_tool_result("ls", &small)
                } else {
                    make_msg("assistant", "ack")
                }
            })
            .collect();

        let removed = elide_old_tool_results(&mut messages);
        assert_eq!(removed, 0);
        for msg in &messages {
            if let Some(results) = &msg.tool_results {
                for r in results {
                    assert_eq!(r.content, "ok");
                }
            }
        }
    }

    #[test]
    fn compact_skips_orphan_tool_result_at_boundary() {
        use crate::ai::ToolResult;
        use crate::ai::types::ToolCall;
        // Build 34 messages where the natural tail boundary would land on a
        // user message carrying a tool_result — i.e. orphaning it from the
        // assistant tool_call that produced it.
        let mut messages: Vec<Message> = Vec::new();
        messages.push(make_msg("user", "first")); // idx 0
        for i in 1..34 {
            if i % 2 == 1 {
                // assistant with tool_call
                messages.push(Message {
                    role: "assistant".to_string(),
                    content: String::new(),
                    tool_calls: Some(vec![ToolCall {
                        id: format!("tc-{}", i),
                        name: "read_file".to_string(),
                        arguments: "{}".to_string(),
                        thought_signature: None,
                    }]),
                    tool_results: None,
                    turn: None,
                });
            } else {
                // user with tool_result
                messages.push(Message {
                    role: "user".to_string(),
                    content: String::new(),
                    tool_calls: None,
                    tool_results: Some(vec![ToolResult {
                        tool_call_id: format!("tc-{}", i - 1),
                        tool_name: "read_file".to_string(),
                        content: "ok".to_string(),
                    }]),
                    turn: None,
                });
            }
        }

        let result = compact_with_digest(messages.clone(), "digest");
        // Every tool_result in the result must have its corresponding tool_call
        // present in the preceding assistant message.
        for (i, m) in result.iter().enumerate() {
            if let Some(results) = &m.tool_results {
                for r in results {
                    let mut found = false;
                    for prev in result[..i].iter().rev() {
                        if let Some(calls) = &prev.tool_calls {
                            if calls.iter().any(|c| c.id == r.tool_call_id) {
                                found = true;
                                break;
                            }
                        }
                    }
                    assert!(
                        found,
                        "orphan tool_result at idx {}: call_id={}",
                        i, r.tool_call_id
                    );
                }
            }
        }
    }

    #[test]
    fn compact_skipped_when_no_clean_boundary() {
        use crate::ai::ToolResult;
        // Construct a pathological history: every user message carries a
        // tool_result. There is no clean boundary past raw_tail_start so
        // compaction should return the history unchanged.
        let mut messages: Vec<Message> = vec![make_msg("user", "first")];
        for i in 1..30 {
            if i % 2 == 1 {
                messages.push(make_msg("assistant", "tool call turn"));
            } else {
                messages.push(Message {
                    role: "user".to_string(),
                    content: String::new(),
                    tool_calls: None,
                    tool_results: Some(vec![ToolResult {
                        tool_call_id: format!("tc-{}", i - 1),
                        tool_name: "read_file".to_string(),
                        content: "ok".to_string(),
                    }]),
                    turn: None,
                });
            }
        }
        let original_len = messages.len();
        let result = compact_with_digest(messages, "digest");
        assert_eq!(result.len(), original_len);
    }

    #[test]
    fn elide_noop_when_history_too_short() {
        let big = "Y".repeat(ELIDE_THRESHOLD_CHARS + 500);
        let mut messages = vec![
            make_msg("user", "q"),
            msg_with_tool_result("read_file", &big),
        ];
        let removed = elide_old_tool_results(&mut messages);
        assert_eq!(removed, 0);
        assert_eq!(
            messages[1].tool_results.as_ref().unwrap()[0].content.len(),
            ELIDE_THRESHOLD_CHARS + 500
        );
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

    // ── Narrative plumbing ───────────────────────────────────────────────

    #[test]
    fn format_messages_for_narrative_includes_roles_and_tool_calls() {
        use crate::ai::ToolResult;
        use crate::ai::types::ToolCall;

        let mut assistant = make_msg("assistant", "investigating disk pressure");
        assistant.tool_calls = Some(vec![ToolCall {
            id: "tc_1".into(),
            name: "run_terminal_command".into(),
            arguments: r#"{"command":"df -h"}"#.into(),
            thought_signature: None,
        }]);

        let tool_result = Message {
            role: "user".to_string(),
            content: String::new(),
            tool_calls: None,
            tool_results: Some(vec![ToolResult {
                tool_call_id: "tc_1".into(),
                tool_name: "run_terminal_command".into(),
                content: "/dev/sda1 95% used".into(),
            }]),
            turn: None,
        };

        let messages = vec![make_msg("user", "check disk"), assistant, tool_result];
        let out = format_messages_for_narrative(&messages);

        assert!(out.contains("USER: check disk"));
        assert!(out.contains("ASSISTANT: investigating disk pressure"));
        assert!(out.contains("[tool_call run_terminal_command]"));
        assert!(out.contains("[tool_result run_terminal_command]"));
        assert!(out.contains("95% used"));
    }

    #[test]
    fn format_messages_for_narrative_truncates_at_budget() {
        let big = "X".repeat(NARRATIVE_INPUT_CHAR_BUDGET);
        let msgs: Vec<Message> = (0..5).map(|_| make_msg("user", &big)).collect();
        let out = format_messages_for_narrative(&msgs);
        assert!(out.contains("[…truncated to fit summarizer budget…]"));
        // Should be roughly one full message + truncation marker, not all five.
        assert!(out.len() < 3 * NARRATIVE_INPUT_CHAR_BUDGET);
    }

    #[test]
    fn build_session_digest_includes_narrative_when_provided() {
        // No events.jsonl means only narrative + header are present.
        let digest = build_session_digest(
            "nonexistent-session",
            Utc::now() - chrono::Duration::hours(1),
            42,
            Some("The user was debugging a slow query.  We identified the index was missing."),
        );
        assert!(digest.starts_with("[Session Digest"));
        assert!(digest.contains("Narrative:"));
        assert!(digest.contains("debugging a slow query"));
    }

    #[test]
    fn build_session_digest_omits_narrative_section_when_none() {
        let digest = build_session_digest(
            "nonexistent-session",
            Utc::now() - chrono::Duration::hours(1),
            42,
            None,
        );
        assert!(!digest.contains("Narrative:"));
    }

    #[test]
    fn build_session_digest_omits_narrative_when_empty_string() {
        let digest = build_session_digest(
            "nonexistent-session",
            Utc::now() - chrono::Duration::hours(1),
            42,
            Some("   \n  \t"),
        );
        assert!(!digest.contains("Narrative:"));
    }

    #[test]
    fn planned_tail_start_returns_none_for_short_history() {
        let msgs: Vec<Message> = (0..5).map(|i| make_msg("user", &i.to_string())).collect();
        assert!(planned_tail_start(&msgs).is_none());
    }

    #[test]
    fn planned_tail_start_matches_compact_with_digest_boundary() {
        // 40 messages, clean alternation: user, assistant, user, ...
        let messages: Vec<Message> = (0..40)
            .map(|i| {
                let role = if i % 2 == 0 { "user" } else { "assistant" };
                make_msg(role, &format!("msg-{}", i))
            })
            .collect();

        let tail_start = planned_tail_start(&messages).expect("should plan a cut");
        let result = compact_with_digest(messages.clone(), "digest");

        // Tail length after compact should match: messages.len() - tail_start.
        assert_eq!(result.len(), 2 + (messages.len() - tail_start));
        // And the first tail message should be the same content.
        assert_eq!(result[2].content, messages[tail_start].content);
    }
}
