use std::path::PathBuf;

/// A single search match.
pub struct SearchResult {
    pub kind: String,
    pub name: String,
    pub line_number: usize,
    pub matched_line: String,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
}

const MAX_RESULTS: usize = 50;
const EVENTS_TAIL_LINES: usize = 10_000;

/// Search across knowledge-base directories.
///
/// `kind`: `"runbooks"` | `"scripts"` | `"memory"` | `"events"` | `"all"`
/// `context_lines`: lines of surrounding context to include with each match.
#[cfg(test)]
pub fn search_repository(query: &str, kind: &str, context_lines: usize) -> Vec<SearchResult> {
    search_repository_with_namespaces(query, kind, context_lines, &["global"])
}

/// Search across knowledge-base directories with namespace-aware memory paths.
///
/// `namespaces`: list of namespaces to search for memory entries (e.g. `["analyst", "global"]`).
pub fn search_repository_with_namespaces(
    query: &str,
    kind: &str,
    context_lines: usize,
    namespaces: &[&str],
) -> Vec<SearchResult> {
    let query_lower = query.to_lowercase();
    let base = crate::config::config_dir();

    let mut dirs: Vec<(PathBuf, String)> = Vec::new();

    match kind {
        "runbooks" => {
            dirs.push((base.join("runbooks"), "runbook".to_string()));
        }
        "scripts" => {
            dirs.push((base.join("scripts"), "script".to_string()));
        }
        "memory" | "all" => {
            // Add memory directories for each namespace
            for ns in namespaces {
                let mem_base = if *ns == "global" {
                    base.join("memory")
                } else {
                    base.join("agents").join(ns).join("memory")
                };
                for category in crate::memory::MemoryCategory::ALL {
                    let dir = mem_base.join(category.dir_name());
                    if dir.exists() {
                        dirs.push((dir, format!("memory/{}", category.dir_name())));
                    }
                }
            }
            // For "all", also include runbooks and scripts
            if kind == "all" {
                dirs.push((base.join("runbooks"), "runbook".to_string()));
                dirs.push((base.join("scripts"), "script".to_string()));
            }
        }
        _ => {
            dirs.push((base.join("runbooks"), "runbook".to_string()));
        }
    }

    let mut results: Vec<SearchResult> = Vec::new();

    // Search all regular directories
    for (dir, kind_label) in &dirs {
        if results.len() >= MAX_RESULTS {
            break;
        }
        search_dir(dir, kind_label, &query_lower, context_lines, &mut results);
    }

    // Search event segments if requested
    if (kind == "events" || kind == "all") && results.len() < MAX_RESULTS {
        search_events_in_segments(&query_lower, context_lines, &mut results);
    }

    results
}

fn search_dir(
    dir: &PathBuf,
    kind_label: &str,
    query_lower: &str,
    context_lines: usize,
    results: &mut Vec<SearchResult>,
) {
    if !dir.exists() {
        return;
    }
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect(),
        Err(_) => return,
    };
    files.sort();

    for path in &files {
        if results.len() >= MAX_RESULTS {
            break;
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let lines: Vec<&str> = content.lines().collect();

        // Also match on filename
        let name_matches =
            stem.to_lowercase().contains(query_lower) || name.to_lowercase().contains(query_lower);

        if name_matches && results.len() < MAX_RESULTS {
            results.push(SearchResult {
                kind: kind_label.to_string(),
                name: stem.clone(),
                line_number: 0,
                matched_line: format!("(filename matches: {})", name),
                context_before: Vec::new(),
                context_after: Vec::new(),
            });
        }

        for (i, line) in lines.iter().enumerate() {
            if results.len() >= MAX_RESULTS {
                break;
            }
            if line.to_lowercase().contains(query_lower) {
                let before_start = i.saturating_sub(context_lines);
                let after_end = (i + context_lines + 1).min(lines.len());
                results.push(SearchResult {
                    kind: kind_label.to_string(),
                    name: stem.clone(),
                    line_number: i + 1,
                    matched_line: line.to_string(),
                    context_before: lines[before_start..i]
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                    context_after: lines[i + 1..after_end]
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                });
            }
        }
    }
}

fn search_events_in_segments(
    query_lower: &str,
    context_lines: usize,
    results: &mut Vec<SearchResult>,
) {
    // Collect the last EVENTS_TAIL_LINES event lines across all segments.
    // We iterate segments newest-first and within each segment we need the
    // true tail (newest lines), not the head. For the newest segment this
    // matters: if it alone exceeds the cap, we want its most recent lines.
    //
    // Strategy: read each segment's valid lines into a buffer, then take
    // only the tail portion needed to fill our remaining cap.
    let all_paths = crate::daemon::utils::event_segment_paths_between(None, None);

    // Collect lines newest-first, up to cap.
    let mut collected: Vec<(String, usize, String)> = Vec::new(); // (readable, line_num, segment_name)
    let mut global_line_num: usize = 0;

    let mut reversed_paths = all_paths;
    reversed_paths.reverse();

    for path in &reversed_paths {
        if collected.len() >= EVENTS_TAIL_LINES {
            break;
        }
        let Ok(file) = std::fs::File::open(path) else {
            continue;
        };
        let reader = std::io::BufReader::new(file);
        let segment_name = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());

        // Read all valid lines from this segment first.
        let mut segment_lines: Vec<(String, String)> = Vec::new(); // (readable, raw_json)
        use std::io::BufRead;
        for line_result in reader.lines() {
            let Ok(line) = line_result else { continue };
            let Ok(_value): Result<serde_json::Value, _> = serde_json::from_str(&line) else {
                continue;
            };
            let readable = json_to_readable(&line);
            segment_lines.push((readable, line));
        }

        // Take only the tail of this segment's lines that we still need.
        let remaining = EVENTS_TAIL_LINES - collected.len();
        let start = segment_lines.len().saturating_sub(remaining);
        for (readable, _raw) in &segment_lines[start..] {
            global_line_num += 1;
            collected.push((readable.clone(), global_line_num, segment_name.clone()));
        }
    }

    // Restore oldest-first order for display.
    collected.reverse();

    // Now search oldest-first.
    let all_lines: Vec<(String, usize, String)> = collected;
    for (i, (readable, line_num, seg_name)) in all_lines.iter().enumerate() {
        if results.len() >= MAX_RESULTS {
            break;
        }
        if readable.to_lowercase().contains(query_lower) {
            let before_start = i.saturating_sub(context_lines);
            let after_end = (i + context_lines + 1).min(all_lines.len());
            results.push(SearchResult {
                kind: "events".to_string(),
                name: seg_name.clone(),
                line_number: *line_num,
                matched_line: readable.clone(),
                context_before: all_lines[before_start..i]
                    .iter()
                    .map(|(r, _, _)| r.clone())
                    .collect(),
                context_after: all_lines[i + 1..after_end]
                    .iter()
                    .map(|(r, _, _)| r.clone())
                    .collect(),
            });
        }
    }
}

/// Convert a JSON event line to a human-readable key=value string.
pub(crate) fn json_to_readable(line: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(line)
        && let Some(obj) = v.as_object()
    {
        return obj
            .iter()
            .map(|(k, v)| {
                let val = match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                format!("{}={}", k, val)
            })
            .collect::<Vec<_>>()
            .join(" ");
    }
    line.to_string()
}

/// Format search results as a human-readable string for the AI tool result.
pub fn format_results(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return "No matches found.".to_string();
    }

    let mut out = String::new();
    let mut current_file = String::new();

    for r in results {
        let file_key = format!("{}/{}", r.kind, r.name);
        if file_key != current_file {
            if !current_file.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("=== {} ({}) ===\n", r.name, r.kind));
            current_file = file_key;
        }

        if r.line_number == 0 {
            // Filename match
            out.push_str(&format!("  {}\n", r.matched_line));
        } else {
            for (j, ctx) in r.context_before.iter().enumerate() {
                let ln = r.line_number - r.context_before.len() + j;
                out.push_str(&format!("  {:>4}  {}\n", ln, ctx));
            }
            out.push_str(&format!("  {:>4}> {}\n", r.line_number, r.matched_line));
            for (j, ctx) in r.context_after.iter().enumerate() {
                out.push_str(&format!("  {:>4}  {}\n", r.line_number + 1 + j, ctx));
            }
        }
    }

    if results.len() >= MAX_RESULTS {
        out.push_str(&format!(
            "\n[Results capped at {} — refine your query for more targeted matches]\n",
            MAX_RESULTS
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    struct TmpHome(std::path::PathBuf);
    impl TmpHome {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("de_srch_test_{}_{}", std::process::id(), n));
            std::fs::create_dir_all(&p).unwrap();
            TmpHome(p)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TmpHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_home() -> TmpHome {
        TmpHome::new()
    }

    fn with_home<F: FnOnce()>(tmp: &TmpHome, f: F) {
        let _guard = crate::test_home_guard();
        let old = env::var("HOME").ok();
        unsafe {
            env::set_var("HOME", tmp.path());
        }
        f();
        match old {
            Some(v) => unsafe {
                env::set_var("HOME", v);
            },
            None => unsafe {
                env::remove_var("HOME");
            },
        }
    }

    #[test]
    fn search_finds_match_in_runbooks() {
        let tmp = temp_home();
        with_home(&tmp, || {
            let dir = crate::config::config_dir().join("runbooks");
            std::fs::create_dir_all(&dir).unwrap();
            let content = "# Runbook: disk-check\n\n## Alert Criteria\n- disk usage above 90%\n";
            std::fs::write(dir.join("disk-check.md"), content).unwrap();

            let results = search_repository("disk usage", "runbooks", 1);
            assert!(!results.is_empty());
            assert!(
                results
                    .iter()
                    .any(|r| r.matched_line.contains("disk usage"))
            );
        });
    }

    #[test]
    fn search_returns_empty_for_no_match() {
        let tmp = temp_home();
        with_home(&tmp, || {
            let dir = crate::config::config_dir().join("runbooks");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("test.md"),
                "# Runbook: test\n\n## Alert Criteria\n- something\n",
            )
            .unwrap();

            let results = search_repository("xyzzy_not_found_12345", "runbooks", 1);
            assert!(results.is_empty());
        });
    }

    #[test]
    fn search_respects_kind_filter() {
        let tmp = temp_home();
        with_home(&tmp, || {
            // Write a runbook with the keyword
            let rb_dir = crate::config::config_dir().join("runbooks");
            std::fs::create_dir_all(&rb_dir).unwrap();
            std::fs::write(
                rb_dir.join("needle.md"),
                "# Runbook: needle\n\n## Alert Criteria\n- contains_needle\n",
            )
            .unwrap();

            // Write a script without the keyword
            let sc_dir = crate::config::config_dir().join("scripts");
            std::fs::create_dir_all(&sc_dir).unwrap();
            std::fs::write(sc_dir.join("nope.sh"), "#!/bin/bash\necho nope").unwrap();

            // Search only scripts — should not find the runbook match
            let results = search_repository("contains_needle", "scripts", 0);
            assert!(
                results.is_empty(),
                "script search should not return runbook matches"
            );

            // Search runbooks — should find it
            let results = search_repository("contains_needle", "runbooks", 0);
            assert!(!results.is_empty());
        });
    }

    #[test]
    fn search_events_returns_tail_not_head_when_segment_exceeds_cap() {
        let _lock = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().unwrap();

        let events_dir = crate::config::events_dir();
        std::fs::create_dir_all(&events_dir).unwrap();
        let seg = events_dir.join("events-20260101.jsonl");

        let mut lines = Vec::new();
        // Write 10,001 lines: the first 6 have a unique "EARLY_IDX" marker,
        // the rest are generic (no unique marker).
        for i in 0..EVENTS_TAIL_LINES {
            let ts = format!("2026-01-01T00:00:{:02}Z", i % 60);
            let line = format!(
                r#"{{"event":"test","ts":"{}","agent_name":"agent","provider":"anthropic","model":"claude","tokens":{{"input_tokens":1,"output_tokens":1,"cache_read_tokens":0,"cache_write_tokens":0}},"cost":{{"total_cost_usd":0.01}},"pricing_source":"BuiltinDefault","idx":{}}}"#,
                ts, i
            );
            lines.push(line);
        }
        // 5 tail lines — these are the most recent
        for i in 0..5 {
            let ts = format!("2026-01-01T01:00:{:02}Z", i);
            let line = format!(
                r#"{{"event":"test","ts":"{}","agent_name":"agent","provider":"anthropic","model":"claude","tokens":{{"input_tokens":1,"output_tokens":1,"cache_read_tokens":0,"cache_write_tokens":0}},"cost":{{"total_cost_usd":0.01}},"pricing_source":"BuiltinDefault","idx":{}}}"#,
                ts,
                EVENTS_TAIL_LINES + i
            );
            lines.push(line);
        }

        std::fs::write(&seg, lines.join("\n") + "\n").unwrap();

        // Search for a high index that only exists in the tail. `search_events`
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

    #[test]
    fn memory_search_dirs_label_incidents_plural() {
        let tmp = temp_home();
        with_home(&tmp, || {
            let incidents_dir = crate::config::config_dir().join("memory").join("incidents");
            std::fs::create_dir_all(&incidents_dir).unwrap();
            std::fs::write(
                incidents_dir.join("test-incident.md"),
                "# test incident\n\nThis is a test incident file.",
            )
            .unwrap();

            let results = search_repository("test incident", "memory", 0);
            assert!(!results.is_empty(), "should find the test incident file");

            // The label must be the plural directory name "memory/incidents"
            for r in &results {
                if r.kind.starts_with("memory/") {
                    assert_eq!(
                        r.kind, "memory/incidents",
                        "label must be memory/incidents (plural), not memory/incident"
                    );
                }
            }
        });
    }
}
