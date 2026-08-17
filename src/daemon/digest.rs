//! Session compaction helpers: narrative summarization and budget-based tail
//! planning.
//!
//! Compaction is driven by prompt-token pressure in [`crate::daemon::server`].
//! When tokens cross the elision threshold, [`elide_old_tool_results`] condenses
//! oversized tool outputs in older turns.  When pressure crosses the compaction
//! threshold, the server builds an epoch record and regenerates the working-set
//! head via [`crate::daemon::context::epochs`]; the narrative that feeds each
//! epoch comes from [`build_narrative_summary`] here, and the cut point from
//! [`planned_tail_start_by_budget`] / [`synthesized_tail_start`].
//! [`DIGEST_THRESHOLD`] is the minimum message count before compaction may fire.
//!
//! [`build_narrative_summary`] calls a cheap model (the optional `digest` config
//! entry, falling back to `default`) to turn the about-to-be-dropped turns into
//! a short natural-language narrative capturing causal threads.  Best-effort — if
//! it times out or errors, the structured epoch tally still fires.

use crate::ai::Message;
use crate::daemon::context::estimate::estimate_message_tokens;
use std::time::Duration;

/// Minimum number of in-memory messages required before token-pressure-triggered
/// compaction may fire.
pub const DIGEST_THRESHOLD: usize = 20;

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
pub(crate) fn format_messages_for_narrative(messages: &[Message]) -> String {
    fn format_one(m: &Message) -> String {
        let mut s = String::new();
        match m.role.as_str() {
            "user" if m.tool_results.is_some() => {
                if let Some(results) = &m.tool_results {
                    for r in results {
                        let preview = if r.content.len() > 400 {
                            format!("{}…", &r.content[..floor_char_boundary(&r.content, 400)])
                        } else {
                            r.content.clone()
                        };
                        s.push_str(&format!("[tool_result {}] {}\n", r.tool_name, preview));
                    }
                }
            }
            "user" if !m.content.is_empty() => {
                s.push_str("USER: ");
                s.push_str(&m.content);
                s.push('\n');
            }
            "assistant" => {
                if !m.content.is_empty() {
                    s.push_str("ASSISTANT: ");
                    s.push_str(&m.content);
                    s.push('\n');
                }
                if let Some(calls) = &m.tool_calls {
                    for c in calls {
                        let arg_preview = if c.arguments.len() > 200 {
                            format!(
                                "{}…",
                                &c.arguments[..floor_char_boundary(&c.arguments, 200)]
                            )
                        } else {
                            c.arguments.clone()
                        };
                        s.push_str(&format!("[tool_call {}] {}\n", c.name, arg_preview));
                    }
                }
            }
            _ => {}
        }
        s
    }

    // Keep the NEWEST messages that fit the budget: walk backward accumulating
    // formatted chunks, stop before exceeding the budget (but always keep at
    // least one), then emit in chronological order with a leading marker when
    // older turns were dropped from the summarizer input.
    let mut chunks: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut truncated = false;
    for m in messages.iter().rev() {
        let chunk = format_one(m);
        if chunk.is_empty() {
            continue;
        }
        if total + chunk.len() > NARRATIVE_INPUT_CHAR_BUDGET && !chunks.is_empty() {
            truncated = true;
            break;
        }
        total += chunk.len();
        chunks.push(chunk);
    }
    chunks.reverse();

    let mut out = String::new();
    if truncated {
        out.push_str("[…older dropped turns omitted from summarizer input…]\n");
    }
    for c in chunks {
        out.push_str(&c);
    }
    out
}

/// One-shot small-model call: system prompt + user text → trimmed response.
/// 20 s timeout; None on any failure. This is the reusable core extracted
/// from [`build_narrative_summary`].
pub async fn summarize_once(
    system: &str,
    user_text: &str,
    model_entry: &crate::config::ModelEntry,
) -> Option<String> {
    let client = crate::ai::make_client(
        &model_entry.provider,
        model_entry.resolve_api_key(),
        model_entry.model.clone(),
        model_entry.effective_base_url(),
        model_entry.effective_max_tokens(),
    );

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<crate::ai::AiEvent>();
    let system_owned = system.to_string();
    let user_msg = Message {
        role: "user".to_string(),
        content: user_text.to_string(),
        tool_calls: None,
        tool_results: None,
        turn: None,
    };
    let msgs = vec![user_msg];

    let chat_fut = client.chat(&system_owned, msgs, tx, false, Vec::new());
    let chat_result = tokio::time::timeout(NARRATIVE_TIMEOUT, chat_fut).await;

    match chat_result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            log::warn!("summarize_once: backend error ({}), skipping", e);
            return None;
        }
        Err(_) => {
            log::warn!(
                "summarize_once: timed out after {}s",
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

    summarize_once(NARRATIVE_SYSTEM_PROMPT, &transcript, model_entry).await
}

// ── Message compaction ───────────────────────────────────────────────

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
                        "[elided: tool `{0}` produced {1} chars at turn {2}; archived — \
                         retrieve the full output with recall_context (turn {2}).]",
                        r.tool_name,
                        orig_len,
                        msg.turn
                            .map(|t| t.to_string())
                            .unwrap_or_else(|| "unknown".to_string())
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
    fn digest_threshold_value() {
        // Sanity check: the compaction floor leaves room for a tail.
        const {
            assert!(DIGEST_THRESHOLD > MIN_TAIL_MESSAGES + 2);
        }
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
    fn narrative_input_keeps_newest() {
        // 5 messages each ~40% of the budget → only the newest ~2 fit. The
        // keep-newest policy must retain the LAST message and drop the FIRST.
        let chunk = "Y".repeat(NARRATIVE_INPUT_CHAR_BUDGET * 40 / 100);
        let msgs: Vec<Message> = (0..5)
            .map(|i| make_msg("user", &format!("MSG{i} {chunk}")))
            .collect();
        let out = format_messages_for_narrative(&msgs);
        assert!(out.contains("MSG4"), "newest message must be retained");
        assert!(!out.contains("MSG0"), "oldest message must be dropped");
        assert!(out.contains("[…older dropped turns omitted from summarizer input…]"));
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
        assert!(elided.contains("archived"), "placeholder mentions archive");
        assert!(elided.contains("turn"), "placeholder mentions turn");
        assert!(
            !elided.contains("events.jsonl"),
            "placeholder should no longer reference events.jsonl"
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
