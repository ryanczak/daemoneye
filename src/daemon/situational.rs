use crate::ai::filter::mask_sensitive;
use crate::ai::types::Message;
use crate::daemon::session::archive_file;
use crate::memory::index;

/// Minimum number of distinct terms of >= MIN_TERM_LEN characters before the
/// block is assembled at all. `build_match_expr` ORs every term, so a short or
/// filler turn ("yes", "run it") would otherwise match arbitrary history.
const MIN_QUERY_TERMS: usize = 3;
const MIN_TERM_LEN: usize = 4;
/// Per-line excerpt cap, in characters (not bytes — excerpts may be UTF-8).
const EXCERPT_CHARS: usize = 200;

/// Check whether `query` carries enough signal to justify a search.
fn has_sufficient_signal(query: &str) -> bool {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    terms.iter().filter(|t| t.len() >= MIN_TERM_LEN).count() >= MIN_QUERY_TERMS
}

/// Assemble the `[SITUATIONAL]` block: at most one past turn and one past
/// epoch from **other** sessions matching the current user turn.
///
/// Returns `None` when the query carries too little signal, when nothing
/// matches, or when every hit belongs to `current_session`.
pub fn assemble_situational_block(
    user_turn: &str,
    current_session: Option<&str>,
) -> Option<String> {
    if !has_sufficient_signal(user_turn) {
        return None;
    }

    let mut parts = Vec::new();

    // Turns lookup
    let turn_hits = index::search_turns(user_turn, 8, None);
    // Skip hits that cannot be resolved (missing archive line, undeserializable,
    // empty excerpt) and try the next candidate rather than dropping the line.
    let turn_result = turn_hits
        .iter()
        .filter(|hit| current_session.is_none_or(|cs| hit.session_id != cs))
        .find_map(resolve_turn_hit);

    if let Some((session_id, turn, excerpt)) = turn_result {
        parts.push(format!(
            "- past turn — session {}, turn {}: {}",
            session_id, turn, excerpt
        ));
    }

    // Epochs lookup
    let epoch_hits = index::search_epochs(user_turn, 8);
    let epoch_result = epoch_hits
        .iter()
        .find(|hit| current_session.is_none_or(|cs| hit.session_id != cs) && !hit.body.is_empty())
        .map(|hit| {
            let excerpt = render_excerpt(&hit.body);
            (hit.session_id.clone(), hit.seq, hit.kind.clone(), excerpt)
        });

    if let Some((session_id, seq, kind, excerpt)) = epoch_result {
        parts.push(format!(
            "- past epoch — session {}, epoch {} ({}): {}",
            session_id, seq, kind, excerpt
        ));
    }

    if parts.is_empty() {
        None
    } else {
        let mut block = String::from("[SITUATIONAL] Possibly-related history from other sessions");
        for part in parts {
            block.push('\n');
            block.push_str(&part);
        }
        Some(block)
    }
}

fn resolve_turn_hit(hit: &index::TurnHit) -> Option<(String, i64, String)> {
    let archive_path = archive_file(&hit.session_id);
    let line = index::read_line_at_offset(&archive_path, hit.offset as u64);
    if line.is_empty() {
        return None;
    }

    let msg: Message = match serde_json::from_str(line.trim_end()) {
        Ok(m) => m,
        Err(e) => {
            log::warn!("situational: failed to deserialize turn line: {}", e);
            return None;
        }
    };

    let mut matched_line = msg.content.clone();
    if let Some(tool_results) = &msg.tool_results {
        for tr in tool_results {
            if !matched_line.is_empty() {
                matched_line.push('\n');
            }
            matched_line.push_str(&tr.content);
        }
    }

    if matched_line.is_empty() {
        return None;
    }

    let excerpt = render_excerpt(&matched_line);
    if excerpt.is_empty() {
        return None;
    }

    Some((hit.session_id.clone(), hit.turn, excerpt))
}

fn render_excerpt(text: &str) -> String {
    let masked = mask_sensitive(text);
    let flattened: String = masked
        .replace(['\r', '\n'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    let chars: Vec<char> = flattened.chars().collect();
    if chars.len() <= EXCERPT_CHARS {
        flattened
    } else {
        let excerpt: String = chars[..EXCERPT_CHARS].iter().collect();
        format!("{}…", excerpt)
    }
}

/// Assemble the `[PRIOR INCIDENTS]` block for an incident-response ghost's
/// first turn: past `incident` memories and past epochs matching the alert
/// text. Returns `None` when nothing matches.
pub fn assemble_incident_context(alert_msg: &str) -> Option<String> {
    if !has_sufficient_signal(alert_msg) {
        return None;
    }

    let mut lines = Vec::new();

    // Up to three prior incident memories
    let hits =
        crate::memory::index::fts5_search_in_category(alert_msg, 3, &["global"], Some("incident"));
    for (namespace, key, _score) in hits {
        let body = match crate::memory::read_memory(
            &key,
            crate::memory::MemoryCategory::Incident,
            &namespace,
        ) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let excerpt = render_excerpt(&body);
        lines.push(format!("- incident memory {key}: {excerpt}"));
    }

    // Up to two matching epochs
    for epoch in crate::memory::index::search_epochs(alert_msg, 2) {
        if epoch.body.is_empty() {
            continue;
        }
        let excerpt = render_excerpt(&epoch.body);
        lines.push(format!(
            "- past epoch — session {}, epoch {} ({}): {excerpt}",
            epoch.session_id, epoch.seq, epoch.kind
        ));
    }

    if lines.is_empty() {
        return None;
    }

    Some(format!(
        "[PRIOR INCIDENTS] Related history for this alert\n{}",
        lines.join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (crate::TestHomeGuard, tempfile::TempDir) {
        let guard = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("HOME", tmp.path());
        }
        (guard, tmp)
    }

    #[test]
    fn short_turn_injects_nothing() {
        let (_guard, _tmp) = setup();

        // Seed a turn that would match if the guard didn't fire
        let session_id = "seeded";
        let body = "this is a distinctive multi word phrase for testing";
        let msg = Message {
            role: "user".to_string(),
            content: body.to_string(),
            tool_calls: None,
            tool_results: None,
            turn: Some(5),
        };
        let line = serde_json::to_string(&msg).unwrap();
        let archive_path = archive_file(session_id);
        std::fs::create_dir_all(archive_path.parent().unwrap()).unwrap();
        std::fs::write(&archive_path, &line).unwrap();
        let _ = index::index_turn(session_id, 5, 0, body);

        // "run it" has two terms, both under MIN_TERM_LEN
        let result = assemble_situational_block("run it", None);
        assert!(
            result.is_none(),
            "short query should return None even with matching corpus"
        );
    }

    #[test]
    fn matching_turn_from_another_session_is_injected() {
        let (_guard, _tmp) = setup();

        let body = "database connection pool exhausted after timeout";
        let msg = Message {
            role: "assistant".to_string(),
            content: body.to_string(),
            tool_calls: None,
            tool_results: None,
            turn: Some(42),
        };
        let line = serde_json::to_string(&msg).unwrap();
        let archive_path = archive_file("other");
        std::fs::create_dir_all(archive_path.parent().unwrap()).unwrap();
        std::fs::write(&archive_path, &line).unwrap();
        let _ = index::index_turn("other", 42, 0, body);

        let result = assemble_situational_block(body, Some("current"));
        assert!(result.is_some(), "should return Some with matching turn");
        let block = result.unwrap();
        assert!(
            block.contains("session other"),
            "block should name the other session"
        );
        assert!(
            block.contains("turn 42"),
            "block should contain the turn number"
        );
        assert!(
            block.contains("database"),
            "block should contain text from the seeded body"
        );
    }

    #[test]
    fn current_session_turn_is_excluded_and_the_guard_is_not_vacuous() {
        let (_guard, _tmp) = setup();

        let body = "authentication failure on the remote endpoint";

        // Seed in current session
        let msg_current = Message {
            role: "user".to_string(),
            content: body.to_string(),
            tool_calls: None,
            tool_results: None,
            turn: Some(10),
        };
        let line_current = serde_json::to_string(&msg_current).unwrap();
        let archive_path_current = archive_file("current");
        std::fs::create_dir_all(archive_path_current.parent().unwrap()).unwrap();
        std::fs::write(&archive_path_current, &line_current).unwrap();
        let _ = index::index_turn("current", 10, 0, body);

        // Seed in other session
        let msg_other = Message {
            role: "user".to_string(),
            content: body.to_string(),
            tool_calls: None,
            tool_results: None,
            turn: Some(20),
        };
        let line_other = serde_json::to_string(&msg_other).unwrap();
        let archive_path_other = archive_file("other");
        std::fs::create_dir_all(archive_path_other.parent().unwrap()).unwrap();
        std::fs::write(&archive_path_other, &line_other).unwrap();
        let _ = index::index_turn("other", 20, 0, body);

        let result = assemble_situational_block(body, Some("current"));
        let block = result.expect("should return Some with matching turn from other session");

        assert!(
            !block.contains("session current"),
            "block must not name the current session"
        );
        assert!(
            block.contains("session other"),
            "block must name the other session — without this assertion the test \
             passes whenever nothing matched at all"
        );
    }

    #[test]
    fn only_current_session_matches_yields_none() {
        let (_guard, _tmp) = setup();

        let body = "unique phrase that only current session has";
        let msg = Message {
            role: "user".to_string(),
            content: body.to_string(),
            tool_calls: None,
            tool_results: None,
            turn: Some(1),
        };
        let line = serde_json::to_string(&msg).unwrap();
        let archive_path = archive_file("current");
        std::fs::create_dir_all(archive_path.parent().unwrap()).unwrap();
        std::fs::write(&archive_path, &line).unwrap();
        let _ = index::index_turn("current", 1, 0, body);

        let result = assemble_situational_block(body, Some("current"));
        assert!(
            result.is_none(),
            "should return None when only current session matches"
        );
    }

    #[test]
    fn epoch_hit_renders_with_its_kind() {
        let (_guard, _tmp) = setup();

        let body = "deployment pipeline failed at integration stage";
        let kind = "deployment";
        let _ = index::index_epoch("other", 7, kind, body);

        let result = assemble_situational_block(body, Some("current"));
        assert!(result.is_some(), "should return Some with matching epoch");
        let block = result.unwrap();
        assert!(
            block.contains("session other"),
            "block should name the session"
        );
        assert!(block.contains("epoch 7"), "block should contain the seq");
        assert!(block.contains(kind), "block should contain the kind string");
    }

    #[test]
    fn excerpt_is_single_line_and_char_truncated() {
        let (_guard, _tmp) = setup();

        // Body with embedded newlines and multi-byte characters, far longer than EXCERPT_CHARS
        let body = "日本語テスト\n\nThis is a very long body with multiple lines\nthat should be truncated properly\nincluding multi-byte characters: café résumé naïve\nand more text to ensure we exceed the character limit significantly beyond the excerpt cap of 200 characters which should trigger the truncation with the ellipsis suffix at the end of the rendered excerpt line in the situational block output";
        let msg = Message {
            role: "assistant".to_string(),
            content: body.to_string(),
            tool_calls: None,
            tool_results: None,
            turn: Some(99),
        };
        let line = serde_json::to_string(&msg).unwrap();
        let archive_path = archive_file("other");
        std::fs::create_dir_all(archive_path.parent().unwrap()).unwrap();
        std::fs::write(&archive_path, &line).unwrap();
        let _ = index::index_turn("other", 99, 0, body);

        let result = assemble_situational_block(body, Some("current"));
        let block = result.expect("should return Some with matching turn");

        // The turn line should contain no newline inside the excerpt
        let turn_line = block
            .lines()
            .find(|l| l.starts_with("- past turn"))
            .expect("should have a turn line");

        // Extract the excerpt part after the ": "
        let excerpt_part = turn_line
            .rsplit_once(": ")
            .map(|(_, e)| e)
            .expect("should have excerpt after colon-space");

        assert!(
            !excerpt_part.contains('\n'),
            "excerpt should be a single line with no embedded newlines"
        );
        assert!(
            excerpt_part.ends_with('…'),
            "excerpt should end with ellipsis when truncated"
        );
    }

    #[test]
    fn tool_result_only_match_still_renders() {
        let (_guard, _tmp) = setup();

        let phrase = "unexpected error in the processing pipeline";
        let msg = Message {
            role: "assistant".to_string(),
            content: String::new(), // empty content
            tool_calls: None,
            tool_results: Some(vec![crate::ai::types::ToolResult {
                tool_call_id: "tool-1".to_string(),
                tool_name: "test".to_string(),
                content: phrase.to_string(),
            }]),
            turn: Some(55),
        };
        let line = serde_json::to_string(&msg).unwrap();
        let archive_path = archive_file("other");
        std::fs::create_dir_all(archive_path.parent().unwrap()).unwrap();
        std::fs::write(&archive_path, &line).unwrap();
        let _ = index::index_turn("other", 55, 0, phrase);

        let result = assemble_situational_block(phrase, Some("current"));
        assert!(
            result.is_some(),
            "should return Some when match is in tool_results"
        );
        let block = result.unwrap();
        assert!(
            block.contains("unexpected"),
            "excerpt should contain the phrase from tool_results"
        );
    }

    #[test]
    fn unresolvable_turn_hit_falls_through_to_the_next() {
        let (_guard, _tmp) = setup();

        // The unresolvable hit must rank FIRST, or this test proves nothing —
        // `find` would pick the resolvable one and the fallback path would never
        // run. BM25 normalizes by document length, so the *shorter, exact* body
        // outranks the longer one; repeating a term in the longer body does not
        // overcome the length penalty. The rank precondition is asserted below
        // rather than assumed.
        let query = "unresolvable subsystem failure";

        // Turn 100 — shortest, exact match, so it ranks first. Indexed at an
        // offset past the end of the archive, so it can never resolve.
        let bad_body = query;
        let msg_bad = Message {
            role: "user".to_string(),
            content: bad_body.to_string(),
            tool_calls: None,
            tool_results: None,
            turn: Some(100),
        };
        let line_bad = serde_json::to_string(&msg_bad).unwrap();

        // Turn 200 — same terms but much longer, so it ranks second. Resolves.
        let good_body = "unresolvable subsystem failure plus a great deal of additional \
                         filler text that makes this document substantially longer than \
                         the other one so bm25 length normalization penalises it";
        let msg_good = Message {
            role: "user".to_string(),
            content: good_body.to_string(),
            tool_calls: None,
            tool_results: None,
            turn: Some(200),
        };
        let line_good = serde_json::to_string(&msg_good).unwrap();

        let archive_path = archive_file("other");
        std::fs::create_dir_all(archive_path.parent().unwrap()).unwrap();
        std::fs::write(&archive_path, format!("{line_bad}\n{line_good}\n")).unwrap();

        let _ = index::index_turn("other", 100, 9_999_999, bad_body);
        let good_offset = (line_bad.len() + 1) as u64; // +1 for the newline
        let _ = index::index_turn("other", 200, good_offset, good_body);

        // Precondition: the unresolvable hit really is first. If BM25 ordering
        // ever changes this, fail loudly here instead of passing vacuously.
        let hits = index::search_turns(query, 8, None);
        assert_eq!(
            hits.first().map(|h| h.turn),
            Some(100),
            "fixture precondition: the unresolvable turn must rank first, \
             otherwise the fallback path is never exercised"
        );

        let result = assemble_situational_block(query, Some("current"));
        let block = result.expect("should fall through to the resolvable turn");

        assert!(
            block.contains("turn 200"),
            "block should contain the second, resolvable turn"
        );
        assert!(
            !block.contains("turn 100"),
            "block must not contain the unresolvable first turn"
        );
    }

    // ── Incident context tests ──────────────────────────────────────────

    #[test]
    fn incident_context_includes_a_matching_prior_incident() {
        let (_guard, _tmp) = setup();

        crate::memory::add_memory(
            "prior-outage",
            "The database connection pool exhausted during peak load",
            crate::memory::MemoryCategory::Incident,
            "global",
        )
        .expect("add prior incident");

        let result =
            assemble_incident_context("database connection pool exhausted during peak load");
        let block = result.expect("should return Some for matching incident");

        assert!(
            block.contains("prior-outage"),
            "block must name the prior incident key: {block}"
        );
        assert!(
            block.contains("connection pool"),
            "block must carry body text from excerpt: {block}"
        );
    }

    #[test]
    fn incident_context_includes_a_matching_epoch() {
        let (_guard, _tmp) = setup();

        let _ = crate::memory::index::index_epoch(
            "epoch-session",
            42,
            "turn",
            "The quantum cascade failure was detected and resolved",
        );

        let result = assemble_incident_context("quantum cascade failure detected and resolved");
        let block = result.expect("should return Some for matching epoch");

        assert!(
            block.contains("epoch-session"),
            "block must name the session: {block}"
        );
        assert!(
            block.contains("42"),
            "block must carry the epoch seq: {block}"
        );
        assert!(block.contains("turn"), "block must carry the kind: {block}");
    }

    #[test]
    fn incident_context_is_none_for_a_low_signal_alert() {
        let (_guard, _tmp) = setup();

        // Seed a non-empty matching corpus so the test is about the guard, not
        // about an empty index. The seeded memory contains terms that share
        // characters with the query words but are not the same tokens.
        crate::memory::add_memory(
            "seeded-incident",
            "The highlight_by_service crashed during peak load",
            crate::memory::MemoryCategory::Incident,
            "global",
        )
        .expect("add seeded incident");

        let result = assemble_incident_context("hi by");
        assert!(
            result.is_none(),
            "low-signal alert must return None even with a non-empty index"
        );
    }

    #[test]
    fn incident_context_is_none_when_nothing_matches() {
        let (_guard, _tmp) = setup();

        // Seed something that won't match
        crate::memory::add_memory(
            "unrelated-incident",
            "The weather was fine today",
            crate::memory::MemoryCategory::Incident,
            "global",
        )
        .expect("add unrelated incident");

        let result = assemble_incident_context("quantum cascade failure meltdown sector seven");
        assert!(
            result.is_none(),
            "high-signal alert with no matches must return None"
        );
    }
}
