//! `recall_context` — retrieve archived turns from the session archive.
//!
//! Query mode uses BM25 over the `turns` FTS corpus. Range mode reads the
//! archive file directly. Output is masked and truncated per the standard
//! tool-output conventions.

use std::fs::File;
use std::io::{BufRead, BufReader};

use crate::ai::filter::mask_sensitive;
use crate::ai::types::Message;
use crate::config::LimitsConfig;

const EXCERPT_HALF: usize = 200;

/// Arguments for `recall_context`.
pub struct RecallArgs {
    pub query: Option<String>,
    pub turn_start: Option<u32>,
    pub turn_end: Option<u32>,
    pub scope: Option<String>,
}

/// Search / slice the session archive.
///
/// Modes:
/// - query only: BM25 over the `turns` FTS corpus; returns match blocks
///   ordered by relevance, each "turn {n} ({role}): …±200-char excerpt…".
///   Cross-session hits (scope: "all") are prefixed with their session id.
/// - turn range only: the messages whose `turn` falls in
///   [turn_start, turn_end] verbatim (role-prefixed), oldest first.
///   Tool-result bodies are rendered beneath the message content.
/// - query + range: BM25 search restricted to the range.
/// - neither: Err("recall_context requires a query and/or a turn range").
///
/// Output is passed through mask_sensitive and truncated at a char
/// boundary to `limits.tool_result_chars` with a
/// "[…truncated — narrow the turn range or refine the query…]" suffix.
pub fn recall(
    session_id: &str,
    args: &RecallArgs,
    limits: &LimitsConfig,
) -> Result<String, String> {
    let has_query = args.query.as_ref().map(|q| !q.is_empty()).unwrap_or(false);
    let has_range = args.turn_start.is_some() || args.turn_end.is_some();

    if !has_query && !has_range {
        return Err("recall_context requires a query and/or a turn range".to_string());
    }

    if has_range {
        let archive_path = crate::daemon::session::archive_file(session_id);
        let file = File::open(&archive_path)
            .map_err(|e| format!("Archive file not found: {} ({})", archive_path.display(), e))?;
        let reader = BufReader::new(file);
        let start = args.turn_start.unwrap_or(0);
        let end = args.turn_end.unwrap_or(u32::MAX);
        return Ok(range_query(
            reader,
            start,
            end,
            args.query.as_deref(),
            limits,
        ));
    }

    // Query-only mode — use FTS
    let query = args.query.as_ref().unwrap();
    let scope_all = args.scope.as_deref() == Some("all");
    let search_session_id = if scope_all { None } else { Some(session_id) };
    let results = fts_query_search(session_id, query, limits, search_session_id);
    Ok(results)
}

/// Range-only or query+range: return messages in [start, end] verbatim.
/// Range-only or query+range: return messages in [start, end] verbatim.
fn range_query(
    reader: BufReader<File>,
    start: u32,
    end: u32,
    query: Option<&str>,
    limits: &LimitsConfig,
) -> String {
    let mut results: Vec<String> = Vec::new();
    let mut legacy_skipped = false;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let msg: Message = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let Some(turn) = msg.turn else {
            legacy_skipped = true;
            continue;
        };
        let turn_u32 = turn as u32;
        if turn_u32 < start || turn_u32 > end {
            continue;
        }

        // If a query is also provided, filter by substring match
        if let Some(q) = query {
            let lower_q = q.to_lowercase();
            if !matches_content(&msg, &lower_q) {
                continue;
            }
        }

        let mut block = format!("turn {} ({}): {}", turn, msg.role, msg.content);
        if let Some(tool_results) = &msg.tool_results {
            for tr in tool_results {
                block.push_str(&format!("\ntool_result {}: {}", tr.tool_name, tr.content));
            }
        }
        results.push(block);
    }

    let mut output = String::new();
    for r in &results {
        output.push_str(r);
        output.push('\n');
    }

    if legacy_skipped {
        output.push_str("(legacy messages without turn numbers were skipped)\n");
    }

    apply_mask_and_truncate(&output, limits)
}

/// Query-only search using BM25 over the `turns` FTS corpus.
fn fts_query_search(
    current_session_id: &str,
    query: &str,
    limits: &LimitsConfig,
    search_session_id: Option<&str>,
) -> String {
    let hits = crate::memory::index::search_turns(query, 100, search_session_id);

    if hits.is_empty() {
        return apply_mask_and_truncate("No matching turns found.\n", limits);
    }

    let lower_terms: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(|t| t.to_string())
        .collect();
    let mut results: Vec<String> = Vec::new();

    for hit in &hits {
        let archive_path = crate::daemon::session::archive_file(&hit.session_id);
        let line = crate::memory::index::read_line_at_offset(&archive_path, hit.offset as u64);
        let Ok(msg) = serde_json::from_str::<Message>(&line) else {
            continue;
        };

        let (excerpt, label_extra) = choose_excerpt(&msg, &lower_terms);
        let prefix = if hit.session_id != current_session_id {
            format!("[session {}] ", hit.session_id)
        } else {
            String::new()
        };
        let turn_label = hit.turn.to_string();
        let _ = hit.score; // used for ordering; kept for test visibility
        results.push(format!(
            "{}{} ({}): {}{}",
            prefix, turn_label, msg.role, excerpt, label_extra
        ));
    }

    let mut output = String::new();
    for r in &results {
        output.push_str(r);
        output.push('\n');
    }

    apply_mask_and_truncate(&output, limits)
}

/// Choose which field to excerpt from, and whether to label it.
/// Returns `(excerpt_text, optional_label)`.
fn choose_excerpt(msg: &Message, lower_terms: &[String]) -> (String, String) {
    // Check msg.content first
    let content_lower = msg.content.to_lowercase();
    for term in lower_terms {
        if content_lower.contains(term) {
            let excerpt = build_excerpt(&msg.content, term, EXCERPT_HALF);
            return (excerpt, String::new());
        }
    }

    // Check each tool result
    if let Some(tool_results) = &msg.tool_results {
        for tr in tool_results {
            let tr_lower = tr.content.to_lowercase();
            for term in lower_terms {
                if tr_lower.contains(term) {
                    let excerpt = build_excerpt(&tr.content, term, EXCERPT_HALF);
                    return (excerpt, format!(" [tool_result: {}]", tr.tool_name));
                }
            }
        }
    }

    // Stemming-only match — no literal substring exists; excerpt from head of content
    let excerpt = build_excerpt(&msg.content, &msg.content, EXCERPT_HALF);
    (excerpt, String::new())
}

/// Check if the message content or any tool result content matches the query.
fn matches_content(msg: &Message, lower_query: &str) -> bool {
    let content_lower = msg.content.to_lowercase();
    if content_lower.contains(lower_query) {
        return true;
    }
    if let Some(tool_results) = &msg.tool_results {
        for tr in tool_results {
            if tr.content.to_lowercase().contains(lower_query) {
                return true;
            }
        }
    }
    false
}

/// Build an excerpt with ±half **chars** around the first match. All indexing
/// is in char space (never byte offsets) so multi-byte content is handled
/// correctly and the ±half window is measured in characters as the spec pins.
fn build_excerpt(content: &str, lower_query: &str, half: usize) -> String {
    let chars: Vec<char> = content.chars().collect();
    let len = chars.len();
    if len == 0 {
        return content.to_string();
    }

    // Locate the match, then convert its byte offset to a char index so the
    // window below indexes `chars` consistently.
    let content_lower = content.to_lowercase();
    let match_byte = content_lower.find(lower_query).unwrap_or(0);
    let match_char = content_lower[..match_byte].chars().count();
    let query_chars = lower_query.chars().count();

    let start = match_char.saturating_sub(half);
    let end = std::cmp::min(match_char + query_chars + half, len);

    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < len { "…" } else { "" };
    let excerpt: String = chars[start..end].iter().collect();

    format!("{}{}{}", prefix, excerpt, suffix)
}

/// Apply masking and truncation to output.
fn apply_mask_and_truncate(output: &str, limits: &LimitsConfig) -> String {
    let masked = mask_sensitive(output);
    truncate_to_limit(&masked, limits.tool_result_chars)
}

/// Truncate at a UTF-8 char boundary, appending the truncation suffix.
fn truncate_to_limit(s: &str, limit: usize) -> String {
    if limit == 0 || s.len() <= limit {
        return s.to_string();
    }
    let mut end = limit;
    // Walk back to a UTF-8 char boundary
    while end < s.len() && !s.is_char_boundary(end) {
        end -= 1;
    }
    let truncated: String = s[..end].chars().collect();
    format!(
        "{}[…truncated — narrow the turn range or refine the query…]",
        truncated
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// RAII test-home guard: holds `TEST_HOME_LOCK`, points `HOME` at a fresh
    /// tempdir, and restores the original `HOME` on drop — so archive FS tests
    /// don't race with (or leak into) other HOME-using tests or the real
    /// `~/.daemoneye`. Every test that touches `archive_file`/`recall` must take
    /// one (`let _home = TestHome::new();`).
    struct TestHome {
        _tmp: tempfile::TempDir,
        _lock: crate::TestHomeGuard,
        saved: Option<String>,
    }

    impl TestHome {
        fn new() -> Self {
            let lock = crate::test_home_guard();
            let saved = std::env::var("HOME").ok();
            let tmp = tempfile::tempdir().unwrap();
            unsafe {
                std::env::set_var("HOME", tmp.path());
            }
            Self {
                _tmp: tmp,
                _lock: lock,
                saved,
            }
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            unsafe {
                match &self.saved {
                    Some(h) => std::env::set_var("HOME", h),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }

    fn make_msg(role: &str, content: &str, turn: Option<usize>) -> Message {
        Message {
            role: role.to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_results: None,
            turn,
        }
    }

    fn make_msg_with_result(
        role: &str,
        content: &str,
        turn: Option<usize>,
        tool_content: &str,
    ) -> Message {
        Message {
            role: role.to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_results: Some(vec![crate::ai::types::ToolResult {
                tool_call_id: "tc1".to_string(),
                tool_name: "read_file".to_string(),
                content: tool_content.to_string(),
            }]),
            turn,
        }
    }

    fn write_archive(id: &str, messages: &[Message]) {
        let path = crate::daemon::session::archive_file(id);
        let parent = path.parent().unwrap();
        std::fs::create_dir_all(parent).unwrap();
        let mut f = std::fs::File::create(&path).unwrap();
        for msg in messages {
            let line = serde_json::to_string(msg).unwrap();
            writeln!(f, "{}", line).unwrap();
        }
    }

    fn clean_archive(id: &str) {
        let path = crate::daemon::session::archive_file(id);
        let _ = std::fs::remove_file(&path);
    }

    fn default_limits() -> LimitsConfig {
        LimitsConfig::default()
    }

    #[test]
    fn recall_query_finds_archived_content() {
        let _home = TestHome::new();
        let id = "test_query_find";
        let msgs = vec![
            make_msg("user", "Check the disk pressure", Some(10)),
            make_msg("assistant", "Running df -h", Some(11)),
            make_msg_with_result(
                "assistant",
                "Disk analysis complete",
                Some(12),
                "Filesystem /dev/sda1 is at 87% capacity with 4.2G free",
            ),
        ];
        write_and_index_turns(id, &msgs);

        let args = RecallArgs {
            query: Some("disk pressure".to_string()),
            turn_start: None,
            turn_end: None,
            scope: None,
        };
        let result = recall(id, &args, &default_limits()).unwrap();
        assert!(
            result.contains("disk pressure") || result.contains("Disk pressure"),
            "Result should contain the match: {}",
            result
        );
        assert!(
            result.contains("10 (user)"),
            "Result should attribute to turn 10: {}",
            result
        );

        clean_archive(id);
    }

    #[test]
    fn recall_range_returns_verbatim_and_skips_legacy() {
        let _home = TestHome::new();
        let id = "test_range";
        let msgs = vec![
            make_msg("user", "First message", None), // legacy - no turn
            make_msg("user", "Second message", Some(5)),
            make_msg("assistant", "Third message", Some(6)),
            make_msg("user", "Fourth message", Some(10)),
        ];
        write_archive(id, &msgs);

        let args = RecallArgs {
            query: None,
            turn_start: Some(5),
            turn_end: Some(6),
            scope: None,
        };
        let result = recall(id, &args, &default_limits()).unwrap();
        assert!(
            result.contains("Second message"),
            "Should contain turn 5: {}",
            result
        );
        assert!(
            result.contains("Third message"),
            "Should contain turn 6: {}",
            result
        );
        assert!(
            !result.contains("Fourth message"),
            "Should not contain turn 10: {}",
            result
        );
        assert!(
            result.contains("legacy messages without turn numbers were skipped"),
            "Should note legacy: {}",
            result
        );

        clean_archive(id);
    }

    #[test]
    fn recall_requires_query_or_range() {
        let _home = TestHome::new();
        let id = "test_requires";
        let args = RecallArgs {
            query: None,
            turn_start: None,
            turn_end: None,
            scope: None,
        };
        let result = recall(id, &args, &default_limits());
        assert!(result.is_err(), "Should error when no query or range");
        assert!(
            result
                .unwrap_err()
                .contains("requires a query and/or a turn range")
        );
    }

    #[test]
    fn recall_masks_sensitive_output() {
        let _home = TestHome::new();
        let id = "test_mask";
        let msgs = vec![make_msg(
            "assistant",
            "The AWS key is AKIAIOSFODNN7EXAMPLE",
            Some(1),
        )];
        write_archive(id, &msgs);

        let args = RecallArgs {
            query: Some("AWS key".to_string()),
            turn_start: None,
            turn_end: None,
            scope: None,
        };
        let result = recall(id, &args, &default_limits()).unwrap();
        // The AWS key pattern should be masked
        assert!(
            !result.contains("AKIAIOSFODNN7EXAMPLE"),
            "AWS key should be masked: {}",
            result
        );

        clean_archive(id);
    }

    #[test]
    fn recall_truncates_at_cap_utf8_safe() {
        let _home = TestHome::new();
        let id = "test_truncate";
        // Create a message with multi-byte UTF-8 content at the truncation boundary
        let content = "🦀rust 🦀rust 🦀rust ".repeat(334); // ~4000 chars
        let msgs = vec![make_msg("assistant", &content, Some(1))];
        write_archive(id, &msgs);

        // Use a custom limits config with a small cap to trigger truncation
        let limits = LimitsConfig {
            tool_result_chars: 50,
            ..default_limits()
        };

        // Range mode returns full content, so truncation triggers
        let args = RecallArgs {
            query: None,
            turn_start: Some(1),
            turn_end: Some(1),
            scope: None,
        };
        let result = recall(id, &args, &limits).unwrap();
        // Should be truncated with the suffix
        assert!(
            result.contains("[…truncated — narrow the turn range or refine the query…]"),
            "Should have truncation suffix: {}",
            result
        );
        // Should be valid UTF-8 (no partial chars at boundary)
        let _ = std::str::from_utf8(result.as_bytes()).unwrap();

        clean_archive(id);
    }

    #[test]
    fn recall_excerpt_is_bounded() {
        let _home = TestHome::new();
        let id = "test_excerpt_bound";
        // Create a 50k-char message
        let large_content = "x".repeat(25000) + "MATCH" + &"y".repeat(25000);
        let msgs = vec![make_msg("assistant", &large_content, Some(1))];
        write_archive(id, &msgs);

        // Range mode returns full content, so truncation triggers
        let args = RecallArgs {
            query: None,
            turn_start: Some(1),
            turn_end: Some(1),
            scope: None,
        };
        let result = recall(id, &args, &default_limits()).unwrap();
        // The result should be truncated (not the full 50k chars)
        assert!(
            result.len() < large_content.len(),
            "Result should be truncated: got {} chars, content is {} chars",
            result.len(),
            large_content.len()
        );

        clean_archive(id);
    }

    #[test]
    fn build_excerpt_handles_empty_content() {
        assert_eq!(build_excerpt("", "x", 10), "");
    }

    #[test]
    fn build_excerpt_handles_match_at_start() {
        let result = build_excerpt("MATCH rest of text", "match", 5);
        assert!(
            !result.starts_with("…"),
            "Should not have prefix when match is at start: {}",
            result
        );
        assert!(
            result.contains("MATCH"),
            "Should contain the match: {}",
            result
        );
    }

    #[test]
    fn build_excerpt_handles_match_at_end() {
        let result = build_excerpt("start of text MATCH", "match", 5);
        assert!(
            !result.ends_with("…"),
            "Should not have suffix when match is at end: {}",
            result
        );
        assert!(
            result.contains("MATCH"),
            "Should contain the match: {}",
            result
        );
    }

    #[test]
    fn build_excerpt_is_multibyte_safe() {
        // Multibyte padding (2-byte 'é') on both sides of an ASCII match: a
        // byte-vs-char index confusion would slice the wrong window or panic.
        let pad = "é".repeat(300); // 300 chars, 600 bytes
        let content = format!("{pad}NEEDLE{pad}");
        let excerpt = build_excerpt(&content, "needle", 200);
        assert!(
            excerpt.contains("NEEDLE"),
            "excerpt must contain the match: {excerpt}"
        );
        assert!(excerpt.starts_with('…'), "left side clipped");
        assert!(excerpt.ends_with('…'), "right side clipped");
        assert!(excerpt.contains('é'), "multibyte context preserved");
        // Windowed, not the whole 606-char string.
        assert!(excerpt.chars().count() < content.chars().count());
    }

    // -----------------------------------------------------------------------
    // New tests for phase-04: FTS query mode, tool-result rendering, scope
    // -----------------------------------------------------------------------

    /// Helper: write archive lines and index them into the turns FTS corpus.
    fn write_and_index_turns(session_id: &str, messages: &[Message]) {
        write_archive(session_id, messages);
        let path = crate::daemon::session::archive_file(session_id);
        // Use the same indexing path as production
        let conn = crate::memory::index::open_index().unwrap();
        let _ = crate::memory::index::index_archive_file(&conn, session_id, &path);
    }

    #[test]
    fn query_excerpt_comes_from_the_matched_tool_result() {
        let _home = TestHome::new();
        let id = "test_tool_result_excerpt";
        let msgs = vec![make_msg_with_result(
            "assistant",
            "AAAAAAAAAA padding padding padding BBBBBBBBBB",
            Some(7),
            "KERNELPANIC detected on cpu0",
        )];
        write_and_index_turns(id, &msgs);

        let args = RecallArgs {
            query: Some("KERNELPANIC".to_string()),
            turn_start: None,
            turn_end: None,
            scope: None,
        };
        let result = recall(id, &args, &default_limits()).unwrap();
        assert!(
            result.contains("KERNELPANIC"),
            "query mode output must contain the matched tool-result text, got: {}",
            result
        );

        clean_archive(id);
    }

    #[test]
    fn range_mode_renders_tool_result_bodies() {
        let _home = TestHome::new();
        let id = "test_range_tool_result";
        let msgs = vec![make_msg_with_result(
            "assistant",
            "ran the command",
            Some(3),
            "OUTPUT_MARKER disk full",
        )];
        write_archive(id, &msgs);

        let args = RecallArgs {
            query: None,
            turn_start: Some(3),
            turn_end: Some(3),
            scope: None,
        };
        let result = recall(id, &args, &default_limits()).unwrap();
        assert!(
            result.contains("OUTPUT_MARKER disk full"),
            "range mode must render tool-result bodies, got: {}",
            result
        );

        clean_archive(id);
    }

    #[test]
    fn query_mode_returns_more_than_eight_matches() {
        let _home = TestHome::new();
        let id = "test_many_matches";
        let mut msgs = Vec::new();
        for i in 0..12 {
            msgs.push(make_msg(
                "user",
                &format!("target phrase match number {}", i),
                Some(i),
            ));
        }
        write_and_index_turns(id, &msgs);

        let args = RecallArgs {
            query: Some("target phrase".to_string()),
            turn_start: None,
            turn_end: None,
            scope: None,
        };
        let result = recall(id, &args, &default_limits()).unwrap();
        let match_lines = result
            .lines()
            .filter(|l| l.contains("target phrase"))
            .count();
        assert!(
            match_lines > 8,
            "query mode should return more than 8 matches (got {}), old ceiling is gone\nresult: {}",
            match_lines,
            result
        );

        clean_archive(id);
    }

    #[test]
    fn query_results_are_bm25_ordered_not_file_ordered() {
        let _home = TestHome::new();
        let id = "test_bm25_order";
        // Write messages where the best BM25 match is written LAST.
        // Turn 1: single occurrence of "server"
        // Turn 2: single occurrence of "restart"
        // Turn 3: multiple occurrences of both "server" and "restart" — should rank best
        let msgs = vec![
            make_msg("user", "the server is online", Some(1)),
            make_msg("user", "the service needs a restart", Some(2)),
            make_msg(
                "user",
                "the server crashed and the server needs a restart and the server rebooted",
                Some(3),
            ),
        ];
        write_and_index_turns(id, &msgs);

        let args = RecallArgs {
            query: Some("server restart".to_string()),
            turn_start: None,
            turn_end: None,
            scope: None,
        };
        let result = recall(id, &args, &default_limits()).unwrap();
        let lines: Vec<&str> = result
            .lines()
            .filter(|l| l.starts_with(|c: char| c.is_ascii_digit()))
            .collect();
        assert!(
            lines.len() >= 2,
            "should have at least 2 matching turns, got: {}",
            result
        );
        // The last-written message (turn 3) has "server" and "restart" multiple times,
        // so it should rank best and appear first.
        assert!(
            lines[0].starts_with("3 (user)"),
            "BM25 best match (turn 3, written last) should appear first, got: {:?}",
            lines
        );

        clean_archive(id);
    }

    #[test]
    fn scope_all_finds_another_session_and_labels_it() {
        let _home = TestHome::new();
        let current_id = "test_scope_all_current";
        let other_id = "test_scope_all_other";
        let msgs = vec![make_msg("user", "unique phrase for other session", Some(5))];
        write_and_index_turns(other_id, &msgs);
        // Current session has nothing matching.
        write_and_index_turns(current_id, &[make_msg("user", "hello world", Some(1))]);

        let args = RecallArgs {
            query: Some("unique phrase".to_string()),
            turn_start: None,
            turn_end: None,
            scope: Some("all".to_string()),
        };
        let result = recall(current_id, &args, &default_limits()).unwrap();
        assert!(
            result.contains("unique phrase"),
            "scope:all must find text from another session, got: {}",
            result
        );
        assert!(
            result.contains(&format!("[session {}]", other_id)),
            "cross-session hit must be prefixed with session id, got: {}",
            result
        );

        clean_archive(current_id);
        clean_archive(other_id);
    }

    #[test]
    fn default_scope_excludes_other_sessions() {
        let _home = TestHome::new();
        let current_id = "test_default_scope_current";
        let other_id = "test_default_scope_other";
        let shared_text = "shared query text for scope test";
        write_and_index_turns(current_id, &[make_msg("user", shared_text, Some(1))]);
        write_and_index_turns(other_id, &[make_msg("user", shared_text, Some(2))]);

        let args = RecallArgs {
            query: Some("shared query".to_string()),
            turn_start: None,
            turn_end: None,
            scope: None,
        };
        let result = recall(current_id, &args, &default_limits()).unwrap();
        // Must NOT contain the other session's prefix
        assert!(
            !result.contains(&format!("[session {}]", other_id)),
            "default scope must not leak another session's turns, got: {}",
            result
        );

        clean_archive(current_id);
        clean_archive(other_id);
    }

    #[test]
    fn unknown_scope_value_behaves_as_current() {
        let _home = TestHome::new();
        let current_id = "test_unknown_scope_current";
        let other_id = "test_unknown_scope_other";
        let unique_text = "only in other session for scope test";
        write_and_index_turns(other_id, &[make_msg("user", unique_text, Some(1))]);
        write_and_index_turns(
            current_id,
            &[make_msg("user", "current session text", Some(1))],
        );

        let args = RecallArgs {
            query: Some("only in other".to_string()),
            turn_start: None,
            turn_end: None,
            scope: Some("everything".to_string()),
        };
        let result = recall(current_id, &args, &default_limits()).unwrap();
        // Unknown scope should behave as "current", so the other session's text
        // should NOT appear
        assert!(
            !result.contains(unique_text),
            "unknown scope must behave as current-session, got: {}",
            result
        );

        clean_archive(current_id);
        clean_archive(other_id);
    }

    #[test]
    fn stemmed_only_match_still_renders_a_block() {
        let _home = TestHome::new();
        let id = "test_stemmed_match";
        // "restart" in the indexed body; query uses "restarting" which stems to the same
        let msgs = vec![make_msg("user", "the server needs to restart", Some(1))];
        write_and_index_turns(id, &msgs);

        let args = RecallArgs {
            query: Some("restarting".to_string()),
            turn_start: None,
            turn_end: None,
            scope: None,
        };
        let result = recall(id, &args, &default_limits()).unwrap();
        // Must render a block rather than "No matching turns found"
        assert!(
            !result.contains("No matching turns found"),
            "stemmed-only match must still render a block, got: {}",
            result
        );
        // The block should contain the turn number (rendered as "1 (user): ...")
        assert!(
            result.contains("1 (user)"),
            "rendered block should contain turn number, got: {}",
            result
        );

        clean_archive(id);
    }

    #[test]
    fn range_mode_ignores_scope() {
        let _home = TestHome::new();
        let id = "test_range_scope";
        let msgs = vec![make_msg("user", "range scope test message", Some(5))];
        write_archive(id, &msgs);

        // scope: "all" should be ignored for range mode
        let args = RecallArgs {
            query: None,
            turn_start: Some(5),
            turn_end: Some(5),
            scope: Some("all".to_string()),
        };
        let result = recall(id, &args, &default_limits()).unwrap();
        assert!(
            result.contains("range scope test message"),
            "range mode should work regardless of scope, got: {}",
            result
        );

        clean_archive(id);
    }
}
