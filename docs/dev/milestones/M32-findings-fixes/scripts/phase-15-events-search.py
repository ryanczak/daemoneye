"""M32 phase 15: the events search reads its index, returns each line once,
and masks what it returns (M32 FINDINGS #16, #17)."""

def edit(path, pairs):
    s = open(path).read()
    for old, new, n in pairs:
        assert s.count(old) == n, (path, old[:70], s.count(old))
        s = s.replace(old, new)
    open(path, 'w').write(s)

# ---- #16: a phrase expression for the events search ----
edit('src/memory/index/mod.rs', [
    ('''    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}
''', '''    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

/// The query's terms as one FTS5 phrase: the same tokens as
/// [`build_match_expr`], in order, quoted together so they must appear
/// adjacent. The events search wants the line holding `idx=0`, not every
/// line holding `idx` or `0`, which the OR form returns. Tokens are
/// alphanumeric, so none can close the quotes.
fn build_phrase_expr(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .take(32)
        .map(str::to_lowercase)
        .collect();
    (!terms.is_empty()).then(|| format!("\\"{}\\"", terms.join(" ")))
}
''', 1),
])
print("build_phrase_expr quotes the query as one phrase")

# ---- #16: search_events stops selecting the contentless `event` column ----
edit('src/memory/index/search.rs', [
    ('''pub fn search_events(query: &str, limit: usize) -> Vec<EventHit> {
    let Some(expr) = build_match_expr(query) else {
''', '''pub fn search_events(query: &str, limit: usize) -> Vec<EventHit> {
    let Some(expr) = build_phrase_expr(query) else {
''', 1),
    ('''pub struct EventHit {
    pub segment: String,
    pub offset: u64,
    #[allow(dead_code)]
    pub event: String,
    #[allow(dead_code)]
    pub score: f64,
}
''', '''pub struct EventHit {
    pub segment: String,
    pub offset: u64,
}
''', 1),
    ('''    let sql = "SELECT m.segment, m.offset, e.event, bm25(events)
               FROM events e JOIN events_map m ON m.id = e.rowid
''', '''    // Select only `events_map` columns: the `events` table is contentless
    // (`content=''`), so its own columns read back NULL, and decoding one
    // into a `String` failed on every row, which `filter_map` then dropped.
    let sql = "SELECT m.segment, m.offset
               FROM events e JOIN events_map m ON m.id = e.rowid
''', 1),
    ('''            Ok(EventHit {
                segment: r.get(0)?,
                offset: r.get::<_, i64>(1)? as u64,
                event: r.get(2)?,
                score: r.get(3)?,
            })
''', '''            Ok(EventHit {
                segment: r.get(0)?,
                offset: r.get::<_, i64>(1)? as u64,
            })
''', 1),
])
print("search_events reads only the map columns")

# ---- #16 + #17: index first, scan adds what the index missed, all masked ----
S = 'src/search.rs'
edit(S, [
    ('''/// Search events using FTS index, falling back to the old segment scan.
fn search_events_fts(
    query: &str,
    query_lower: &str,
    context_lines: usize,
    results: &mut Vec<SearchResult>,
) {
    // 1. FTS index search
    let index_hits = crate::memory::index::search_events(query, MAX_RESULTS);

    for hit in &index_hits {
        if results.len() >= MAX_RESULTS {
            break;
        }
        // Resolve segment path and read the line at offset
        let events_dir = crate::config::events_dir();
        // `segment` is the file stem the index stores; the file is `<stem>.jsonl`.
        let seg_path = events_dir.join(format!("{}.jsonl", hit.segment));
        let matched_line = crate::memory::index::read_line_at_offset(&seg_path, hit.offset);

        if matched_line.is_empty() {
            log::warn!(
                "search_events: could not read line at offset {} in {}",
                hit.offset,
                hit.segment
            );
            continue;
        }

        results.push(SearchResult {
            kind: "events".to_string(),
            name: hit.segment.clone(),
            line_number: 1,
            matched_line,
            context_before: Vec::new(),
            context_after: Vec::new(),
        });
    }

    // 2. Fallback: old segment scan for any events not in the index
    // This preserves backward compatibility for unindexed events
    if results.len() < MAX_RESULTS {
        search_events_in_segments(query_lower, context_lines, results);
    }
}
''', '''/// Search events: the FTS index first, which covers the whole history ranked
/// by BM25 and finds the query's words as a phrase, then the tail scan's
/// substring matches the index did not already return. Both keep only lines
/// that contain the query text. Both paths name a hit by its segment file and render it with
/// `json_to_readable`, so a line both find is returned once. Every events
/// result is masked, as the index body is (M32 #16, #17).
fn search_events_fts(
    query: &str,
    query_lower: &str,
    context_lines: usize,
    results: &mut Vec<SearchResult>,
) {
    let start = results.len();
    let events_dir = crate::config::events_dir();
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();

    for hit in crate::memory::index::search_events(query, MAX_RESULTS) {
        if results.len() >= MAX_RESULTS {
            break;
        }
        // `segment` is the file stem the index stores; the file is `<stem>.jsonl`.
        let name = format!("{}.jsonl", hit.segment);
        let raw = crate::memory::index::read_line_at_offset(&events_dir.join(&name), hit.offset);
        if raw.is_empty() {
            log::warn!(
                "search_events: could not read line at offset {} in {}",
                hit.offset,
                hit.segment
            );
            continue;
        }
        let readable = json_to_readable(&raw);
        // The scan's rule, so both paths return the same kind of line: the
        // phrase can also match `idx-0` or `IDX: 0`, which `idx=0` does not.
        if !readable.to_lowercase().contains(query_lower) {
            continue;
        }
        if !seen.insert((name.clone(), readable.clone())) {
            continue;
        }
        results.push(SearchResult {
            kind: "events".to_string(),
            name,
            line_number: 1,
            matched_line: readable,
            context_before: Vec::new(),
            context_after: Vec::new(),
        });
    }

    if results.len() < MAX_RESULTS {
        search_events_in_segments(query_lower, context_lines, &seen, results);
    }

    for r in &mut results[start..] {
        r.matched_line = crate::ai::mask_sensitive(&r.matched_line);
        for line in r.context_before.iter_mut().chain(r.context_after.iter_mut()) {
            *line = crate::ai::mask_sensitive(line);
        }
    }
}
''', 1),
    ('''fn search_events_in_segments(
    query_lower: &str,
    context_lines: usize,
    results: &mut Vec<SearchResult>,
) {
''', '''fn search_events_in_segments(
    query_lower: &str,
    context_lines: usize,
    seen: &std::collections::HashSet<(String, String)>,
    results: &mut Vec<SearchResult>,
) {
''', 1),
    ('''        if readable.to_lowercase().contains(query_lower) {
            let before_start = i.saturating_sub(context_lines);
''', '''        if readable.to_lowercase().contains(query_lower)
            && !seen.contains(&(seg_name.clone(), readable.clone()))
        {
            let before_start = i.saturating_sub(context_lines);
''', 1),
])
print("the index hits come first, each line once, all masked")

# ---- tests ----
T = 'src/search_tests.rs'
edit(T, [
    ('''#[test]
fn search_events_returns_tail_not_head_when_segment_exceeds_cap() {
''', '''/// M32 #16: the index covers the whole history, so a head line outside the
/// scan's tail window is found; a line both paths find is returned once; and
/// the scan alone still reads only the tail.
#[test]
fn search_events_covers_the_whole_history_and_returns_each_line_once() {
''', 1),
    ('''    // Search for a high index that only exists in the tail. `search_events`
    // matches against the `json_to_readable` form, which renders fields as
    // `key=value` (e.g. `idx=10003`) — not raw JSON — so the query must use
    // that form.
    let results = search_repository(&format!("idx={}", EVENTS_TAIL_LINES + 3), "events", 0);
    assert!(
        !results.is_empty(),
        "idx={} should be found in the tail of the segment",
        EVENTS_TAIL_LINES + 3
    );

    // idx=0 is the very first line, outside the last-EVENTS_TAIL_LINES tail
    // window — it must NOT be surfaced (this is the tail-not-head guarantee).
    let results = search_repository("idx=0", "events", 0);
    assert!(
        results.is_empty(),
        "idx=0 should not appear — only the last EVENTS_TAIL_LINES lines are searched"
    );
}
''', '''    // Both paths match the `json_to_readable` form, which renders fields as
    // `key=value` (e.g. `idx=10003`), so the queries use that form.
    let tail = format!("idx={}", EVENTS_TAIL_LINES + 3);
    let results = search_repository(&tail, "events", 0);
    assert_eq!(
        results.len(),
        1,
        "a tail line found by the index and the scan is returned once: {:?}",
        results.iter().map(|r| &r.matched_line).collect::<Vec<_>>()
    );
    assert!(results[0].matched_line.contains(&tail));

    // idx=0 is the first line, outside the scan's tail window: only the
    // index reaches it.
    let results = search_repository("idx=0", "events", 0);
    assert_eq!(results.len(), 1, "the index finds a head line");
    assert!(results[0].matched_line.contains("idx=0 "));
    assert_eq!(results[0].name, "events-20260101.jsonl");

    // `dx=0` is no FTS token, so only the scan could match it, and the only
    // line holding it is outside the tail window.
    let results = search_repository("dx=0", "events", 0);
    assert!(
        results.is_empty(),
        "the scan still reads only the last EVENTS_TAIL_LINES lines"
    );
}

/// M32 #17: an events hit is masked whichever path found it.
#[test]
fn events_search_masks_what_it_returns() {
    let _lock = crate::test_home_guard();
    let dir = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", dir.path()) };

    let events_dir = crate::config::events_dir();
    std::fs::create_dir_all(&events_dir).unwrap();
    let seg = events_dir.join("events-20260101.jsonl");
    let line = r#"{"event":"command","ts":"2026-01-01T00:00:00Z","command":"aws configure set aws_access_key_id AKIAIOSFODNN7EXAMPLE"}"#;
    std::fs::write(&seg, format!("{line}\\n")).unwrap();
    crate::memory::index::index_event(
        "events-20260101",
        0,
        "command",
        &crate::search::json_to_readable(line),
    )
    .unwrap();

    let results = search_repository("configure", "events", 0);
    assert_eq!(results.len(), 1, "one line, found once");
    assert!(
        results[0].matched_line.contains("<AWS_KEY>")
            && !results[0].matched_line.contains("AKIAIOSFODNN7EXAMPLE"),
        "the key is masked: {}",
        results[0].matched_line
    );
}
''', 1),
    ('''    let results = search_repository("webhook_alert", "events", 0);
    assert!(
        !results.is_empty(),
        "events search should find webhook_alert by free text"
    );
}
''', '''    // A line the phrase `webhook alert` matches but the query text does
    // not: the index hit is dropped, as the scan would drop it.
    let other = r#"{"event":"note","ts":"2026-01-01T00:00:01Z","msg":"webhook-alert drill"}"#;
    std::fs::write(&seg, format!("{line}\\n{other}\\n")).unwrap();
    crate::memory::index::index_event(
        "events-20260101",
        (line.len() + 1) as u64,
        "note",
        &crate::search::json_to_readable(other),
    )
    .unwrap();

    let results = search_repository("webhook_alert", "events", 0);
    assert_eq!(
        results.len(),
        1,
        "events search finds webhook_alert by free text, once"
    );
    assert_eq!(results[0].name, "events-20260101.jsonl");
}
''', 1),
])
print("the events tests pin the whole history, one copy and masking")
