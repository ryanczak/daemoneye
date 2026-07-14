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
use crate::daemon::context::estimate::estimate_message_tokens;
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

/// Minimum number of messages the budget planner must keep in the tail, even
/// when a single message exceeds the whole budget. Guarantees the model always
/// retains recent context after a compaction pass.
const MIN_TAIL_MESSAGES: usize = 4;

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

/// Scan event segments for events belonging to this session (or global events
/// like webhook alerts) that occurred after `since`.
fn tally_events(session_id: &str, since: DateTime<Utc>) -> EventTally {
    let mut t = EventTally::default();

    crate::daemon::utils::for_each_event_between(Some(since), None, &mut |v| {
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
    });

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
            "user" if !m.content.is_empty() => {
                out.push_str("USER: ");
                out.push_str(&m.content);
                out.push('\n');
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
    let chat_fut = client.chat(&system, msgs, tx, false, Vec::new());
    let chat_result = tokio::time::timeout(NARRATIVE_TIMEOUT, chat_fut).await;

    match chat_result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            log::warn!(
                "digest narrative: backend error ({}), skipping narrative",
                e
            );
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

/// Plan the compaction cut so the *kept* tail fits within `budget_tokens`
/// estimated tokens (post-scale). Walks backward from the end accumulating
/// `estimate_message_tokens * token_scale`, stops before exceeding the budget,
/// then advances to the next clean turn boundary so no orphan tool_result is
/// created.
///
/// Guarantees: keeps at least `MIN_TAIL_MESSAGES`, leaves the two head slots
/// (`[first, digest]`), and drops at least one message. Returns `None` when the
/// history is too short or the only clean boundary would keep ≤ 1 message
/// (the caller then falls through to [`synthesized_tail_start`]).
pub fn planned_tail_start_by_budget(
    messages: &[Message],
    budget_tokens: u64,
    token_scale: f64,
) -> Option<usize> {
    let raw = raw_budget_cut(messages, budget_tokens, token_scale)?;
    let boundary = crate::daemon::session::next_clean_turn_start(messages, raw)?;
    // A boundary that keeps ≤ 1 message (or nothing) means we failed to drop a
    // meaningful span at a clean point — let the synthesized path handle it.
    if boundary >= messages.len().saturating_sub(1) {
        return None;
    }
    Some(boundary)
}

/// Last-resort cut when no clean turn boundary exists in the budget region:
/// return the raw budget index directly (which may orphan a leading
/// tool_result). The caller repairs the tail head via [`repair_tail_head`]
/// after the cut instead of skipping compaction.
pub fn synthesized_tail_start(
    messages: &[Message],
    budget_tokens: u64,
    token_scale: f64,
) -> Option<usize> {
    raw_budget_cut(messages, budget_tokens, token_scale)
}

/// Shared backward walk: the first-kept index whose tail fits `budget_tokens`,
/// clamped so the tail keeps at least `MIN_TAIL_MESSAGES`, the two head slots
/// are preserved (index ≥ 2), and at least one message is dropped.
fn raw_budget_cut(messages: &[Message], budget_tokens: u64, token_scale: f64) -> Option<usize> {
    let len = messages.len();
    if len <= MIN_TAIL_MESSAGES + 2 {
        return None;
    }
    let mut sum = 0u64;
    let mut cut = len;
    let mut i = len;
    while i > 2 {
        let idx = i - 1;
        let est = (estimate_message_tokens(&messages[idx]) as f64 * token_scale) as u64;
        let kept = len - idx;
        // Include this message if it still fits the budget, or if we have not
        // yet reached the minimum tail (the floor overrides the budget).
        if sum.saturating_add(est) > budget_tokens && kept > MIN_TAIL_MESSAGES {
            break;
        }
        sum = sum.saturating_add(est);
        cut = idx;
        i = idx;
    }
    if cut < 2 || cut >= len.saturating_sub(1) {
        return None;
    }
    Some(cut)
}

/// Repair the head of a compacted tail so no `tool_results` message is orphaned
/// from its (now-dropped) producing `tool_calls`. Strips `tool_results` from
/// each leading `user` message that carries them, substituting a placeholder
/// when the message would otherwise be empty. Stops at the first message that
/// is not a `user`-with-results — a leading `assistant` with `tool_calls` is
/// kept intact because its results follow inside the tail (pairing preserved).
pub fn repair_tail_head(tail: &mut [Message]) {
    for msg in tail.iter_mut() {
        let is_orphan_user =
            msg.role == "user" && msg.tool_results.as_ref().is_some_and(|v| !v.is_empty());
        if !is_orphan_user {
            break;
        }
        msg.tool_results = None;
        if msg.content.is_empty() {
            msg.content = "[tool results from a compacted turn were elided]".to_string();
        }
    }
}

/// Replace old messages with a digest, keeping the first message (system context)
/// and the tail from `tail_start` onward.
///
/// Layout: `[first_message] [digest_as_assistant] [messages[tail_start..]]`
///
/// `tail_start` is chosen by the caller via [`planned_tail_start_by_budget`]
/// (clean boundary) or [`synthesized_tail_start`] (raw, repaired afterward), so
/// the planner and the compactor can never disagree. Returns `messages`
/// unchanged when `tail_start` is not a feasible cut (`< 2` or `>= len`).
pub fn compact_with_digest(
    messages: Vec<Message>,
    digest: &str,
    tail_start: usize,
) -> Vec<Message> {
    if tail_start < 2 || tail_start >= messages.len() {
        return messages;
    }

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
///
/// `aggressive` selects the replacement strategy for an oversized result:
/// - `false` (the ≥ `elide_at_pct` path): soft head+tail truncation — keep the
///   first 1000 and last 500 chars around a truncation marker, so the model
///   still sees the shape of the output.
/// - `true` (the ≥ `compact_at_pct` path, run before digesting): full
///   placeholder, maximizing the freed budget.
pub fn elide_old_tool_results(messages: &mut [Message], aggressive: bool) -> usize {
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
                let replacement = if aggressive {
                    format!(
                        "[elided: tool `{}` produced {} chars; outside live context window. \
                         See events.jsonl for full output.]",
                        r.tool_name, orig_len
                    )
                } else {
                    soft_truncate(&r.content, 1000, 500)
                };
                removed += orig_len.saturating_sub(replacement.len());
                r.content = replacement;
            }
        }
    }
    removed
}

/// Truncate `s` to its first `head` and last `tail` bytes joined by a marker,
/// snapping both split points to UTF-8 char boundaries so multi-byte content
/// never panics. Returns `s` unchanged when it is already within `head + tail`.
fn soft_truncate(s: &str, head: usize, tail: usize) -> String {
    let total = s.len();
    if total <= head + tail {
        return s.to_string();
    }
    let head_end = floor_char_boundary(s, head);
    let tail_start = ceil_char_boundary(s, total - tail);
    let removed = tail_start.saturating_sub(head_end);
    format!(
        "{}[… {} chars truncated …]{}",
        &s[..head_end],
        removed,
        &s[tail_start..]
    )
}

/// Largest char boundary `<= index` (stable-Rust equivalent of the unstable
/// `str::floor_char_boundary`).
fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Smallest char boundary `>= index` (stable-Rust equivalent of the unstable
/// `str::ceil_char_boundary`).
fn ceil_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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

        let removed = elide_old_tool_results(&mut messages, true);

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

        let removed = elide_old_tool_results(&mut messages, true);
        assert_eq!(removed, 0);
        for msg in &messages {
            if let Some(results) = &msg.tool_results {
                for r in results {
                    assert_eq!(r.content, "ok");
                }
            }
        }
    }

    /// Shared checker: assert that every `tool_results` message in `msgs` has
    /// its producing `tool_calls` present in a preceding message. Used to prove
    /// no compaction path (clean boundary, synthesized boundary, repair) leaves
    /// an orphan the provider backends would reject.
    fn assert_no_orphan_tool_results(msgs: &[Message]) {
        for (i, m) in msgs.iter().enumerate() {
            if let Some(results) = &m.tool_results {
                for r in results {
                    let found = msgs[..i].iter().rev().any(|prev| {
                        prev.tool_calls
                            .as_ref()
                            .is_some_and(|calls| calls.iter().any(|c| c.id == r.tool_call_id))
                    });
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
    fn compact_skips_orphan_tool_result_at_boundary() {
        use crate::ai::ToolResult;
        use crate::ai::types::ToolCall;
        // Repeating 3-unit history [assistant(call), user(result), user(clean)]
        // so clean user boundaries exist at indices 0, 3, 6, … The budget
        // planner must advance to a clean boundary and NOT orphan a result.
        let mut messages: Vec<Message> = vec![make_msg("user", "first")]; // idx 0 (clean)
        for j in 0..12 {
            messages.push(Message {
                role: "assistant".to_string(),
                content: String::new(),
                tool_calls: Some(vec![ToolCall {
                    id: format!("tc-{}", j),
                    name: "read_file".to_string(),
                    arguments: "{}".to_string(),
                    thought_signature: None,
                }]),
                tool_results: None,
                turn: None,
            });
            messages.push(Message {
                role: "user".to_string(),
                content: String::new(),
                tool_calls: None,
                tool_results: Some(vec![ToolResult {
                    tool_call_id: format!("tc-{}", j),
                    tool_name: "read_file".to_string(),
                    content: "ok".to_string(),
                }]),
                turn: None,
            });
            messages.push(make_msg("user", &format!("clean-{}", j)));
        }

        let original_len = messages.len();
        let ts =
            planned_tail_start_by_budget(&messages, 100, 1.0).expect("clean boundary should exist");
        // The clean planner must land on a user turn without tool_results.
        assert_eq!(messages[ts].role, "user");
        assert!(messages[ts].tool_results.is_none());
        let result = compact_with_digest(messages, "digest", ts);
        assert!(result.len() < original_len, "should have compacted");
        assert_no_orphan_tool_results(&result);
    }

    #[test]
    fn synthesized_boundary_repairs_orphans() {
        use crate::ai::ToolResult;
        use crate::ai::types::ToolCall;
        // Pathological history: every user message carries a tool_result (paired
        // with the preceding assistant's tool_call), so no clean boundary exists.
        // The old code SKIPPED compaction; now the synthesized boundary + repair
        // compacts it and strips whichever result is orphaned by the cut.
        let mut messages: Vec<Message> = vec![make_msg("user", "first")];
        for i in 1..30 {
            if i % 2 == 1 {
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

        // No clean boundary → planner returns None; synthesized returns a cut.
        assert!(planned_tail_start_by_budget(&messages, 100, 1.0).is_none());
        let ts =
            synthesized_tail_start(&messages, 100, 1.0).expect("synthesized boundary should exist");
        let mut result = compact_with_digest(messages, "digest", ts);
        assert!(result.len() < original_len, "should have compacted");

        // Repair the tail head (result[2..]) — any leading orphan user result
        // is stripped and given the placeholder.
        crate::daemon::digest::repair_tail_head(&mut result[2..]);
        assert_no_orphan_tool_results(&result);
        // The first tail message, if it was an orphan user, now carries the
        // placeholder content and no tool_results.
        let head = &result[2];
        if head.role == "user" {
            assert!(head.tool_results.is_none());
            assert_eq!(
                head.content,
                "[tool results from a compacted turn were elided]"
            );
        }
    }

    #[test]
    fn synthesized_boundary_keeps_paired_assistant_head() {
        use crate::ai::ToolResult;
        use crate::ai::types::ToolCall;
        // Negative case: a tail whose head is an assistant with tool_calls whose
        // results follow INSIDE the tail must NOT be stripped — pairing intact.
        let mut tail: Vec<Message> = vec![
            Message {
                role: "assistant".to_string(),
                content: "calling".to_string(),
                tool_calls: Some(vec![ToolCall {
                    id: "tc-keep".to_string(),
                    name: "read_file".to_string(),
                    arguments: "{}".to_string(),
                    thought_signature: None,
                }]),
                tool_results: None,
                turn: None,
            },
            Message {
                role: "user".to_string(),
                content: String::new(),
                tool_calls: None,
                tool_results: Some(vec![ToolResult {
                    tool_call_id: "tc-keep".to_string(),
                    tool_name: "read_file".to_string(),
                    content: "ok".to_string(),
                }]),
                turn: None,
            },
        ];
        crate::daemon::digest::repair_tail_head(&mut tail);
        // Assistant head untouched: still carries its tool_calls.
        assert!(tail[0].tool_calls.is_some());
        // Its paired result (inside the tail) is untouched.
        assert!(tail[1].tool_results.is_some());
        assert_no_orphan_tool_results(&tail);
    }

    #[test]
    fn elide_noop_when_history_too_short() {
        let big = "Y".repeat(ELIDE_THRESHOLD_CHARS + 500);
        let mut messages = vec![
            make_msg("user", "q"),
            msg_with_tool_result("read_file", &big),
        ];
        let removed = elide_old_tool_results(&mut messages, true);
        assert_eq!(removed, 0);
        assert_eq!(
            messages[1].tool_results.as_ref().unwrap()[0].content.len(),
            ELIDE_THRESHOLD_CHARS + 500
        );
    }

    #[test]
    fn tally_events_reads_dated_segments() {
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let saved_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let events_dir = crate::config::events_dir();
        let _ = std::fs::create_dir_all(&events_dir);

        let seg = events_dir.join("events-20240115.jsonl");
        let since = chrono::NaiveDate::from_ymd_opt(2024, 1, 15)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();

        // Write a job_complete event with exit_code 0
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&seg)
            .unwrap();
        let record = serde_json::json!({
            "ts": "2024-01-15T12:00:00+00:00",
            "event": "job_complete",
            "session": "test-session",
            "exit_code": 0,
            "job_name": "my-job"
        });
        writeln!(file, "{}", record).unwrap();

        let tally = tally_events("test-session", since);
        assert_eq!(tally.commands_ok, 1);

        if let Some(h) = saved_home {
            unsafe { std::env::set_var("HOME", h) };
        } else {
            unsafe { std::env::remove_var("HOME") };
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

        // Cut at index 16 (a user turn) — reproduces the legacy TAIL_KEEP cut.
        let result = compact_with_digest(messages.clone(), "digest text", 16);

        // First message preserved.
        assert_eq!(result[0].content, "msg-0");
        // Second is the digest.
        assert_eq!(result[1].content, "digest text");
        assert_eq!(result[1].role, "assistant");
        // Tail starts on a user message (even index in original).
        assert_eq!(result[2].role, "user");
        assert_eq!(result[2].content, "msg-16");
        // Total should be 2 (head + digest) + the kept tail (32 - 16).
        assert_eq!(result.len(), 2 + (32 - 16));
        // Last message is the original last.
        assert_eq!(result.last().unwrap().content, "msg-31");
    }

    #[test]
    fn compact_noop_when_tail_start_infeasible() {
        let messages: Vec<Message> = (0..10)
            .map(|i| make_msg("user", &format!("msg-{}", i)))
            .collect();
        // tail_start < 2 leaves no room for [first, digest] — unchanged.
        assert_eq!(
            compact_with_digest(messages.clone(), "digest", 0).len(),
            messages.len()
        );
        // tail_start >= len is out of range — unchanged.
        assert_eq!(
            compact_with_digest(messages.clone(), "digest", 99).len(),
            messages.len()
        );
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

        // The budget planner must land the cut on a clean user turn.
        let budget = 200; // small enough to force a real cut
        let ts =
            planned_tail_start_by_budget(&messages, budget, 1.0).expect("should plan a clean cut");
        assert_eq!(messages[ts].role, "user", "planner must cut on a user turn");
        let result = compact_with_digest(messages, "digest", ts);
        // Index 2 in result is the first tail message — must be a user turn.
        assert_eq!(result[2].role, "user");
    }

    #[test]
    fn digest_threshold_value() {
        // Sanity check: threshold is between TAIL_KEEP and MAX_HISTORY.
        const {
            assert!(DIGEST_THRESHOLD > TAIL_KEEP + 2);
            assert!(DIGEST_THRESHOLD < crate::daemon::session::MAX_HISTORY);
        }
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
        let result = compact_with_digest(messages.clone(), "digest", tail_start);

        // Tail length after compact should match: messages.len() - tail_start.
        assert_eq!(result.len(), 2 + (messages.len() - tail_start));
        // And the first tail message should be the same content.
        assert_eq!(result[2].content, messages[tail_start].content);
    }

    // ── Budget-cut + hysteresis (phase 03) ───────────────────────────────

    fn uniform_history(n: usize, content_chars: usize) -> Vec<Message> {
        (0..n)
            .map(|i| {
                let role = if i % 2 == 0 { "user" } else { "assistant" };
                make_msg(role, &"x".repeat(content_chars))
            })
            .collect()
    }

    #[test]
    fn budget_cut_respects_target() {
        // context_window 10_000, target 40% → budget 4000 tokens, scale 1.0.
        // ~258-token messages (1000 chars) at ~65% pressure.
        let messages = uniform_history(30, 1000);
        let budget: u64 = 10_000 * 40 / 100;
        let ts = planned_tail_start_by_budget(&messages, budget, 1.0).expect("should plan a cut");
        let kept_tokens =
            crate::daemon::context::estimate::estimate_history_tokens(&messages[ts..]);
        let max_msg = estimate_message_tokens(&messages[ts]);
        assert!(
            kept_tokens <= budget + max_msg,
            "kept tail {kept_tokens} exceeds budget {budget} + one message {max_msg}"
        );
    }

    #[test]
    fn budget_cut_keeps_min_tail() {
        // Enormous messages, tiny budget: the floor must still keep MIN_TAIL.
        let messages = uniform_history(10, 4000);
        let ts = planned_tail_start_by_budget(&messages, 1, 1.0).expect("floor guarantees a cut");
        assert!(
            messages.len() - ts >= MIN_TAIL_MESSAGES,
            "kept {} messages, expected >= {}",
            messages.len() - ts,
            MIN_TAIL_MESSAGES
        );
    }

    #[test]
    fn no_rethrash_after_compaction() {
        // After compacting to target, a second decision with the same window
        // must not want to compact again (token_pct below compact threshold).
        let context_window: u64 = 10_000;
        let messages = uniform_history(40, 1000);
        let budget = context_window * 40 / 100;
        let ts = planned_tail_start_by_budget(&messages, budget, 1.0).expect("should plan a cut");
        let result = compact_with_digest(messages, "short digest", ts);
        let result_tokens = crate::daemon::context::estimate::estimate_history_tokens(&result);
        let token_pct = (result_tokens as f64 / context_window as f64 * 100.0) as u32;
        assert!(
            token_pct < 60,
            "compacted working set at {token_pct}% still above the 60% compact threshold"
        );
    }

    #[test]
    fn elide_soft_truncates_head_tail() {
        let big = format!(
            "{}{}{}",
            "A".repeat(1000),
            "B".repeat(2000),
            "C".repeat(500)
        );
        let mut messages = vec![msg_with_tool_result("read_file", &big)];
        for _ in 0..11 {
            messages.push(make_msg("assistant", "ack"));
        }
        let removed = elide_old_tool_results(&mut messages, false);
        assert!(removed > 0);
        let elided = &messages[0].tool_results.as_ref().unwrap()[0].content;
        assert!(elided.starts_with(&"A".repeat(1000)), "head preserved");
        assert!(elided.contains("chars truncated"), "marker present");
        assert!(elided.ends_with(&"C".repeat(500)), "tail preserved");
        assert!(elided.len() < big.len(), "content shrank");
    }

    #[test]
    fn elide_aggressive_full_placeholder() {
        let big = "Z".repeat(ELIDE_THRESHOLD_CHARS + 100);
        let mut messages = vec![msg_with_tool_result("read_file", &big)];
        for _ in 0..11 {
            messages.push(make_msg("assistant", "ack"));
        }
        elide_old_tool_results(&mut messages, true);
        let elided = &messages[0].tool_results.as_ref().unwrap()[0].content;
        assert!(
            elided.starts_with("[elided:"),
            "aggressive uses full placeholder"
        );
    }

    #[test]
    fn elide_truncation_is_utf8_safe() {
        // Multi-byte content: 2000 × 'é' (2 bytes each) = 4000 bytes > threshold.
        // A byte-index slice at 1000/last-500 would land mid-char and panic if
        // not snapped to a char boundary.
        let big = "é".repeat(2000);
        let mut messages = vec![msg_with_tool_result("read_file", &big)];
        for _ in 0..11 {
            messages.push(make_msg("assistant", "ack"));
        }
        // Must not panic.
        let removed = elide_old_tool_results(&mut messages, false);
        assert!(removed > 0);
        // Result is a valid String by construction; assert the marker landed.
        let elided = &messages[0].tool_results.as_ref().unwrap()[0].content;
        assert!(elided.contains("chars truncated"));
    }
}
