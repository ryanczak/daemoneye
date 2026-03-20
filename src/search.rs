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
pub fn search_repository(query: &str, kind: &str, context_lines: usize) -> Vec<SearchResult> {
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
        "memory" => {
            dirs.push((
                base.join("memory").join("session"),
                "memory/session".to_string(),
            ));
            dirs.push((
                base.join("memory").join("knowledge"),
                "memory/knowledge".to_string(),
            ));
            dirs.push((
                base.join("memory").join("incidents"),
                "memory/incidents".to_string(),
            ));
        }
        "all" => {
            dirs.push((base.join("runbooks"), "runbook".to_string()));
            dirs.push((base.join("scripts"), "script".to_string()));
            dirs.push((
                base.join("memory").join("session"),
                "memory/session".to_string(),
            ));
            dirs.push((
                base.join("memory").join("knowledge"),
                "memory/knowledge".to_string(),
            ));
            dirs.push((
                base.join("memory").join("incidents"),
                "memory/incidents".to_string(),
            ));
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

    // Search events.jsonl if requested
    if (kind == "events" || kind == "all") && results.len() < MAX_RESULTS {
        search_events(&base, &query_lower, context_lines, &mut results);
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

fn search_events(
    base: &PathBuf,
    query_lower: &str,
    context_lines: usize,
    results: &mut Vec<SearchResult>,
) {
    let events_path = base.join("events.jsonl");
    if !events_path.exists() {
        return;
    }
    let content = match std::fs::read_to_string(&events_path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let all_lines: Vec<&str> = content.lines().collect();
    let start = all_lines.len().saturating_sub(EVENTS_TAIL_LINES);
    let lines = &all_lines[start..];

    for (i, line) in lines.iter().enumerate() {
        if results.len() >= MAX_RESULTS {
            break;
        }
        // Convert JSON line to readable form for matching
        let readable = json_to_readable(line);
        if readable.to_lowercase().contains(query_lower) {
            let before_start = i.saturating_sub(context_lines);
            let after_end = (i + context_lines + 1).min(lines.len());
            results.push(SearchResult {
                kind: "events".to_string(),
                name: "events.jsonl".to_string(),
                line_number: start + i + 1,
                matched_line: readable,
                context_before: lines[before_start..i]
                    .iter()
                    .map(|l| json_to_readable(l))
                    .collect(),
                context_after: lines[i + 1..after_end]
                    .iter()
                    .map(|l| json_to_readable(l))
                    .collect(),
            });
        }
    }
}

/// Convert a JSON event line to a human-readable key=value string.
fn json_to_readable(line: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
        if let Some(obj) = v.as_object() {
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
    use crate::util::UnpoisonExt;
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
        let _guard = crate::TEST_HOME_LOCK.lock().unwrap_or_log();
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
}
