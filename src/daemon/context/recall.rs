//! `recall_context` — retrieve archived turns from the session archive.
//!
//! Searches the append-only archive file by substring query or turn range.
//! Output is masked and truncated per the standard tool-output conventions.

use std::fs::File;
use std::io::{BufRead, BufReader};

use crate::ai::filter::mask_sensitive;
use crate::ai::types::Message;
use crate::config::LimitsConfig;

const MAX_MATCHES: usize = 8;
const EXCERPT_HALF: usize = 200;

/// Arguments for `recall_context`.
pub struct RecallArgs {
    pub query: Option<String>,
    pub turn_start: Option<u32>,
    pub turn_end: Option<u32>,
}

/// Search / slice the session archive.
///
/// Modes:
/// - query only: case-insensitive substring over `content` and
///   `tool_results[].content`; returns up to MAX_MATCHES match blocks,
///   each "turn {n} ({role}): …±200-char excerpt around the match…".
///   Multiple matches within one message collapse to one block.
/// - turn range only: the messages whose `turn` falls in
///   [turn_start, turn_end] verbatim (role-prefixed), oldest first.
/// - query + range: substring search restricted to the range.
/// - neither: Err("recall_context requires a query and/or a turn range").
///
/// Output is passed through mask_sensitive and truncated at a char
/// boundary to `limits.tool_result_chars` with a
/// "[…truncated — narrow the turn range or refine the query…]" suffix.
/// Messages with `turn: None` (legacy) are searchable by query but
/// unreachable by range; a range query notes
/// "(legacy messages without turn numbers were skipped)" when any were.
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

    let archive_path = crate::daemon::session::archive_file(session_id);
    let file = File::open(&archive_path)
        .map_err(|e| format!("Archive file not found: {} ({})", archive_path.display(), e))?;
    let reader = BufReader::new(file);

    if has_range {
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

    // Query-only mode
    let query = args.query.as_ref().unwrap();
    let results = query_search(reader, query, limits);
    Ok(results)
}

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

        results.push(format!("turn {} ({}): {}", turn, msg.role, msg.content));
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

/// Query-only search: find up to MAX_MATCHES messages containing the query.
fn query_search(reader: BufReader<File>, query: &str, limits: &LimitsConfig) -> String {
    let lower_q = query.to_lowercase();
    let mut results: Vec<String> = Vec::new();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let msg: Message = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(_) => continue,
        };

        if !matches_content(&msg, &lower_q) {
            continue;
        }

        let turn_label = msg
            .turn
            .map(|t| format!("turn {}", t))
            .unwrap_or_else(|| "turn ?".to_string());

        // Build an excerpt with ±200 chars around the first match
        let excerpt = build_excerpt(&msg.content, &lower_q, EXCERPT_HALF);
        results.push(format!("{} ({}): {}", turn_label, msg.role, excerpt));

        if results.len() >= MAX_MATCHES {
            break;
        }
    }

    let mut output = String::new();
    for r in &results {
        output.push_str(r);
        output.push('\n');
    }

    if results.is_empty() {
        output.push_str("No matching turns found.\n");
    }

    apply_mask_and_truncate(&output, limits)
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
        write_archive(id, &msgs);

        let args = RecallArgs {
            query: Some("disk pressure".to_string()),
            turn_start: None,
            turn_end: None,
        };
        let result = recall(id, &args, &default_limits()).unwrap();
        assert!(
            result.contains("disk pressure") || result.contains("Disk pressure"),
            "Result should contain the match: {}",
            result
        );
        assert!(
            result.contains("turn 10"),
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
        let content = "🦀".repeat(1000); // Each 🦀 is 4 bytes
        let msgs = vec![make_msg("assistant", &content, Some(1))];
        write_archive(id, &msgs);

        // Use a custom limits config with a small cap to trigger truncation
        let limits = LimitsConfig {
            tool_result_chars: 50,
            ..default_limits()
        };

        let args = RecallArgs {
            query: Some("🦀".to_string()),
            turn_start: None,
            turn_end: None,
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

        let args = RecallArgs {
            query: Some("MATCH".to_string()),
            turn_start: None,
            turn_end: None,
        };
        let result = recall(id, &args, &default_limits()).unwrap();
        // The excerpt for this message should be bounded (~400 chars + markers)
        // Find the excerpt line
        let excerpt_line = result.lines().find(|l| l.contains("MATCH")).unwrap();
        // The excerpt should be roughly 200 + 5 + 200 = ~405 chars plus markers
        assert!(
            excerpt_line.len() < 500,
            "Excerpt line should be bounded (got {} chars): {}",
            excerpt_line.len(),
            excerpt_line
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
}
