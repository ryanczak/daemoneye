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
    let mut results: Vec<SearchResult> = Vec::new();

    match kind {
        "runbooks" | "scripts" => {
            let (dir, kind_label, index_kind) = match kind {
                "runbooks" => (base.join("runbooks"), "runbook", Some("runbook")),
                "scripts" => (base.join("scripts"), "script", Some("script")),
                _ => unreachable!(),
            };
            search_artifact_dir_fts(
                &dir,
                kind_label,
                query,
                &query_lower,
                context_lines,
                index_kind,
                &mut results,
            );
        }
        "memory" => {
            search_memory_fts(query, &query_lower, context_lines, namespaces, &mut results);
        }
        "events" => {
            search_events_fts(query, &query_lower, context_lines, &mut results);
        }
        "turns" => {
            search_turns_fts(query, &mut results);
        }
        "epochs" => {
            search_epochs_fts(query, &mut results);
        }
        "all" => {
            // Memory
            search_memory_fts(query, &query_lower, context_lines, namespaces, &mut results);
            // Runbooks
            let runbooks_dir = base.join("runbooks");
            search_artifact_dir_fts(
                &runbooks_dir,
                "runbook",
                query,
                &query_lower,
                context_lines,
                Some("runbook"),
                &mut results,
            );
            // Scripts
            let scripts_dir = base.join("scripts");
            search_artifact_dir_fts(
                &scripts_dir,
                "script",
                query,
                &query_lower,
                context_lines,
                Some("script"),
                &mut results,
            );
            // Events
            search_events_fts(query, &query_lower, context_lines, &mut results);
        }
        _ => {
            // Default to runbooks (existing behavior)
            let runbooks_dir = base.join("runbooks");
            search_artifact_dir_fts(
                &runbooks_dir,
                "runbook",
                query,
                &query_lower,
                context_lines,
                Some("runbook"),
                &mut results,
            );
        }
    }

    results
}

/// Search an artifact directory (runbooks or scripts) using FTS, falling back
/// to filename matching. Index hits are emitted first (rank-ordered), then
/// filename-only hits. De-duplication ensures a file hit by both paths appears
/// only once.
fn search_artifact_dir_fts(
    dir: &std::path::Path,
    kind_label: &str,
    query: &str,
    query_lower: &str,
    context_lines: usize,
    index_kind: Option<&str>,
    results: &mut Vec<SearchResult>,
) {
    // 1. FTS index search — ranked hits
    let index_hits = crate::memory::index::search_artifacts(query, MAX_RESULTS, index_kind);

    // Collect names of files matched by index (for de-dup)
    let mut index_hit_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    for hit in &index_hits {
        if results.len() >= MAX_RESULTS {
            break;
        }
        // Runbooks are stored as `<name>.md`; scripts have no extension.
        let path = if kind_label == "runbook" {
            dir.join(format!("{}.md", hit.name))
        } else {
            dir.join(&hit.name)
        };
        if let Ok(content) = std::fs::read_to_string(&path) {
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let lines: Vec<&str> = content.lines().collect();

            // Scan for literal matches
            let has_literal = lines.iter().any(|l| l.to_lowercase().contains(query_lower));

            if has_literal {
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
            } else {
                // THE TRAP: stemmed hit has no literal substring — still emit
                // the first non-empty line so the document is not silently dropped.
                if let Some((first_idx, first_line)) =
                    lines.iter().enumerate().find(|(_, l)| !l.trim().is_empty())
                {
                    let before_start = first_idx.saturating_sub(context_lines);
                    let after_end = (first_idx + context_lines + 1).min(lines.len());
                    results.push(SearchResult {
                        kind: kind_label.to_string(),
                        name: stem.clone(),
                        line_number: first_idx + 1,
                        matched_line: first_line.to_string(),
                        context_before: lines[before_start..first_idx]
                            .iter()
                            .map(|s| s.to_string())
                            .collect(),
                        context_after: lines[first_idx + 1..after_end]
                            .iter()
                            .map(|s| s.to_string())
                            .collect(),
                    });
                }
            }
            index_hit_names.insert(hit.name.clone());
        }
    }

    // 2. Filename matching — independent of the index
    if !dir.exists() {
        return;
    }
    let files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect(),
        Err(_) => return,
    };

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

        let name_matches =
            stem.to_lowercase().contains(query_lower) || name.to_lowercase().contains(query_lower);

        // Skip if already emitted as an index hit (de-dup). The index stores the
        // bare artifact name, so compare against the stem, not the filename.
        if !name_matches || index_hit_names.contains(&stem) {
            continue;
        }

        // Filename match but not an index hit — emit it
        results.push(SearchResult {
            kind: kind_label.to_string(),
            name: stem,
            line_number: 0,
            matched_line: format!("(filename matches: {})", name),
            context_before: Vec::new(),
            context_after: Vec::new(),
        });
    }
}

/// Search memory entries using FTS, falling back to filename matching.
fn search_memory_fts(
    query: &str,
    query_lower: &str,
    context_lines: usize,
    namespaces: &[&str],
    results: &mut Vec<SearchResult>,
) {
    // 1. FTS index search
    let index_hits = crate::memory::index::fts5_search(query, MAX_RESULTS, namespaces);

    // Collect (namespace, key) pairs for de-dup
    let mut index_hit_keys: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();

    for (namespace, key, _score) in &index_hits {
        if results.len() >= MAX_RESULTS {
            break;
        }
        // Resolve to file path
        let base = crate::config::config_dir();
        let mem_base = if namespace == "global" {
            base.join("memory")
        } else {
            base.join("agents").join(namespace).join("memory")
        };

        // Try each category directory
        let mut found = false;
        for category in crate::memory::MemoryCategory::ALL {
            let dir = mem_base.join(category.dir_name());
            let path = dir.join(format!("{}.md", key));
            if path.exists() {
                let kind_label = format!("memory/{}", category.dir_name());
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let lines: Vec<&str> = content.lines().collect();
                    let has_literal = lines.iter().any(|l| l.to_lowercase().contains(query_lower));

                    if has_literal {
                        for (i, line) in lines.iter().enumerate() {
                            if results.len() >= MAX_RESULTS {
                                break;
                            }
                            if line.to_lowercase().contains(query_lower) {
                                let before_start = i.saturating_sub(context_lines);
                                let after_end = (i + context_lines + 1).min(lines.len());
                                results.push(SearchResult {
                                    kind: kind_label.clone(),
                                    name: key.clone(),
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
                    } else {
                        // THE TRAP: stemmed hit — emit first non-empty line
                        if let Some((first_idx, first_line)) =
                            lines.iter().enumerate().find(|(_, l)| !l.trim().is_empty())
                        {
                            let before_start = first_idx.saturating_sub(context_lines);
                            let after_end = (first_idx + context_lines + 1).min(lines.len());
                            results.push(SearchResult {
                                kind: kind_label.clone(),
                                name: key.clone(),
                                line_number: first_idx + 1,
                                matched_line: first_line.to_string(),
                                context_before: lines[before_start..first_idx]
                                    .iter()
                                    .map(|s| s.to_string())
                                    .collect(),
                                context_after: lines[first_idx + 1..after_end]
                                    .iter()
                                    .map(|s| s.to_string())
                                    .collect(),
                            });
                        }
                    }
                }
                index_hit_keys.insert((namespace.clone(), key.clone()));
                found = true;
                break;
            }
        }
        let _ = found; // suppress unused warning; we just need the de-dup tracking
    }

    // 2. Filename matching — independent of the index
    for ns in namespaces {
        if results.len() >= MAX_RESULTS {
            break;
        }
        let mem_base = if *ns == "global" {
            crate::config::config_dir().join("memory")
        } else {
            crate::config::config_dir()
                .join("agents")
                .join(ns)
                .join("memory")
        };
        for category in crate::memory::MemoryCategory::ALL {
            if results.len() >= MAX_RESULTS {
                break;
            }
            let dir = mem_base.join(category.dir_name());
            if !dir.exists() {
                continue;
            }
            let kind_label = format!("memory/{}", category.dir_name());

            let files: Vec<PathBuf> = match std::fs::read_dir(&dir) {
                Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
                Err(_) => continue,
            };

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

                let name_matches = stem.to_lowercase().contains(query_lower)
                    || name.to_lowercase().contains(query_lower);

                // De-dup: skip if already an index hit
                if !name_matches || index_hit_keys.contains(&(ns.to_string(), stem.clone())) {
                    continue;
                }

                results.push(SearchResult {
                    kind: kind_label.clone(),
                    name: stem,
                    line_number: 0,
                    matched_line: format!("(filename matches: {})", name),
                    context_before: Vec::new(),
                    context_after: Vec::new(),
                });
            }
        }
    }
}

/// Search events using FTS index, falling back to the old segment scan.
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

/// Search the `turns` FTS corpus and resolve hits through the archive files.
fn search_turns_fts(query: &str, results: &mut Vec<SearchResult>) {
    use crate::ai::types::Message;

    let hits = crate::memory::index::search_turns(query, MAX_RESULTS, None);
    for hit in hits {
        if results.len() >= MAX_RESULTS {
            break;
        }

        let archive_path = crate::daemon::session::archive_file(&hit.session_id);
        let line = crate::memory::index::read_line_at_offset(&archive_path, hit.offset as u64);
        if line.is_empty() {
            log::warn!(
                "search_turns_fts: could not read line at offset {} for session {}",
                hit.offset,
                hit.session_id
            );
            continue;
        }

        let msg: Message = match serde_json::from_str(line.trim_end()) {
            Ok(m) => m,
            Err(e) => {
                log::warn!(
                    "search_turns_fts: failed to deserialize message at offset {} for session {}: {e}",
                    hit.offset,
                    hit.session_id
                );
                continue;
            }
        };

        // Build matched_line from content + tool_results so a match that exists
        // only in a tool result is visible (same fix as phase 04 for recall_context).
        let mut matched_line = msg.content.clone();
        if let Some(tool_results) = &msg.tool_results {
            for tr in tool_results {
                if !matched_line.is_empty() {
                    matched_line.push('\n');
                }
                matched_line.push_str(&tr.content);
            }
        }

        results.push(SearchResult {
            kind: "turns".to_string(),
            name: format!("{} turn {}", hit.session_id, hit.turn),
            line_number: 1,
            matched_line,
            context_before: Vec::new(),
            context_after: Vec::new(),
        });
    }
}

/// Search the `epochs` FTS corpus. Epochs are stored-content, so no file
/// round-trip is needed — `body` is selected directly from the index.
fn search_epochs_fts(query: &str, results: &mut Vec<SearchResult>) {
    let hits = crate::memory::index::search_epochs(query, MAX_RESULTS);
    for hit in hits {
        if results.len() >= MAX_RESULTS {
            break;
        }

        results.push(SearchResult {
            kind: "epochs".to_string(),
            name: format!("{} epoch {}", hit.session_id, hit.seq),
            line_number: 1,
            matched_line: hit.body,
            context_before: Vec::new(),
            context_after: Vec::new(),
        });
    }
}

#[allow(dead_code)]
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

            // Index the runbook so FTS can find it
            crate::memory::index::index_artifact("runbook", "disk-check", "", content).unwrap();

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
            let rb_content = "# Runbook: needle\n\n## Alert Criteria\n- contains_needle\n";
            std::fs::write(rb_dir.join("needle.md"), rb_content).unwrap();

            // Index the runbook
            crate::memory::index::index_artifact("runbook", "needle", "", rb_content).unwrap();

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

    #[test]
    fn stemmed_query_finds_runbook_with_root_word() {
        let tmp = temp_home();
        with_home(&tmp, || {
            let dir = crate::config::config_dir().join("runbooks");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("service-recovery.md"),
                "# Service Recovery\n\nThe service must restart cleanly after a crash.\n",
            )
            .unwrap();

            crate::memory::index::index_artifact(
                "runbook",
                "service-recovery",
                "",
                "The service must restart cleanly after a crash.",
            )
            .unwrap();

            let results = search_repository("restarting", "runbooks", 0);
            assert!(
                !results.is_empty(),
                "stemmed query 'restarting' should find runbook with 'restart'. Results: {:?}",
                results.iter().map(|r| &r.name).collect::<Vec<_>>()
            );
            assert!(
                results.iter().any(|r| r.name == "service-recovery"),
                "should find service-recovery runbook"
            );
        });
    }

    #[test]
    fn stemmed_hit_renders_a_non_empty_matched_line() {
        let tmp = temp_home();
        with_home(&tmp, || {
            let dir = crate::config::config_dir().join("runbooks");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("recovery-plan.md"),
                "# Recovery Plan\n\nThe system restarts automatically.\n",
            )
            .unwrap();

            crate::memory::index::index_artifact(
                "runbook",
                "recovery-plan",
                "",
                "The system restarts automatically.",
            )
            .unwrap();

            let results = search_repository("restarting", "runbooks", 0);
            assert!(!results.is_empty(), "stemmed hit should produce results");
            for r in &results {
                assert!(
                    !r.matched_line.is_empty(),
                    "matched_line must be non-empty for a stemmed-only hit"
                );
            }
        });
    }

    #[test]
    fn stemmed_query_finds_memory_entry() {
        let tmp = temp_home();
        with_home(&tmp, || {
            crate::memory::add_memory(
                "daemon-behavior",
                "the daemon restarts on signal",
                crate::memory::MemoryCategory::Knowledge,
                "global",
            )
            .unwrap();

            let results = search_repository("restarting", "memory", 0);
            assert!(
                !results.is_empty(),
                "stemmed query 'restarting' should find memory with 'restarts'"
            );
            assert!(
                results.iter().any(|r| r.name == "daemon-behavior"),
                "should find daemon-behavior memory"
            );
        });
    }

    #[test]
    fn stemmed_query_finds_script() {
        let tmp = temp_home();
        with_home(&tmp, || {
            let dir = crate::config::config_dir().join("scripts");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("restart-service"),
                "#!/bin/bash\nsystemctl restart myservice\n",
            )
            .unwrap();

            crate::memory::index::index_artifact(
                "script",
                "restart-service",
                "",
                "#!/bin/bash\nsystemctl restart myservice",
            )
            .unwrap();

            let results = search_repository("restarting", "scripts", 0);
            assert!(
                !results.is_empty(),
                "stemmed query 'restarting' should find script with 'restart'"
            );
            assert!(
                results.iter().any(|r| r.name == "restart-service"),
                "should find restart-service script"
            );
        });
    }

    #[test]
    fn events_kind_finds_webhook_alert_by_free_text() {
        let _lock = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };

        let events_dir = crate::config::events_dir();
        std::fs::create_dir_all(&events_dir).unwrap();
        // Real segments are `events-YYYYMMDD.jsonl`; the indexed label is the stem.
        let seg = events_dir.join("events-20260101.jsonl");
        let line = r#"{"event":"webhook_alert","ts":"2026-01-01T00:00:00Z","msg":"disk space critical on /dev/sda1"}"#;
        std::fs::write(&seg, format!("{line}\n")).unwrap();

        // Index it through the production hook so the body, masking and the
        // map/FTS insert order all match what log_event actually writes.
        crate::memory::index::index_event(
            "events-20260101",
            0,
            "webhook_alert",
            &crate::search::json_to_readable(line),
        )
        .unwrap();

        let results = search_repository("webhook_alert", "events", 0);
        assert!(
            !results.is_empty(),
            "events search should find webhook_alert by free text"
        );
    }

    #[test]
    fn results_are_rank_ordered_not_alphabetical() {
        let tmp = temp_home();
        with_home(&tmp, || {
            let dir = crate::config::config_dir().join("runbooks");
            std::fs::create_dir_all(&dir).unwrap();

            std::fs::write(dir.join("alpha.md"), "# Alpha\n\nquokka quokka quokka\n").unwrap();
            crate::memory::index::index_artifact("runbook", "alpha", "", "quokka quokka quokka")
                .unwrap();

            std::fs::write(
                dir.join("zebra.md"),
                "# Zebra\n\nthe quick brown fox jumps over the lazy dog many times and then quokka appears once at the end\n",
            )
            .unwrap();
            crate::memory::index::index_artifact(
                "runbook",
                "zebra",
                "",
                "the quick brown fox jumps over the lazy dog many times and then quokka appears once at the end",
            )
            .unwrap();

            let results = search_repository("quokka", "runbooks", 0);
            assert!(results.len() >= 2, "should find both runbooks");

            let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
            assert!(
                names[0] == "alpha",
                "best-ranked document (alpha) should come first. Got: {:?}",
                names
            );
        });
    }

    #[test]
    fn filename_match_still_returned_without_body_match() {
        let tmp = temp_home();
        with_home(&tmp, || {
            let dir = crate::config::config_dir().join("runbooks");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("deploy-checklist.md"),
                "# Checklist\n\nReview all items before release.\n",
            )
            .unwrap();

            let results = search_repository("deploy", "runbooks", 0);
            assert!(
                !results.is_empty(),
                "filename match should still return even without index hit"
            );
            assert!(
                results.iter().any(|r| r.name == "deploy-checklist"),
                "should find deploy-checklist by filename"
            );
        });
    }

    #[test]
    fn file_matching_name_and_body_appears_once() {
        let tmp = temp_home();
        with_home(&tmp, || {
            let dir = crate::config::config_dir().join("runbooks");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                // Exactly ONE body line contains "deploy" so the count below is a
                // real de-dup assertion: results are one-per-matching-line, so a
                // second matching line would legitimately make this 2.
                dir.join("deploy-guide.md"),
                "# Service Runbook\n\nFollow these steps to deploy the service.\n",
            )
            .unwrap();

            crate::memory::index::index_artifact(
                "runbook",
                "deploy-guide",
                "",
                "Follow these steps to deploy the service.",
            )
            .unwrap();

            let results = search_repository("deploy", "runbooks", 0);
            let deploy_count = results.iter().filter(|r| r.name == "deploy-guide").count();
            assert_eq!(
                deploy_count, 1,
                "file matching both by name and body should appear exactly once"
            );
        });
    }

    #[test]
    fn non_matching_document_is_absent() {
        let tmp = temp_home();
        with_home(&tmp, || {
            let dir = crate::config::config_dir().join("runbooks");
            std::fs::create_dir_all(&dir).unwrap();

            std::fs::write(
                dir.join("target.md"),
                "# Target\n\nThe service must restart cleanly.\n",
            )
            .unwrap();
            crate::memory::index::index_artifact(
                "runbook",
                "target",
                "",
                "The service must restart cleanly.",
            )
            .unwrap();

            std::fs::write(
                dir.join("decoy.md"),
                "# Decoy\n\nThis is about cooking pasta.\n",
            )
            .unwrap();
            crate::memory::index::index_artifact(
                "runbook",
                "decoy",
                "",
                "This is about cooking pasta.",
            )
            .unwrap();

            let results = search_repository("restarting", "runbooks", 0);
            assert!(
                results.iter().any(|r| r.name == "target"),
                "should find target"
            );
            assert!(
                !results.iter().any(|r| r.name == "decoy"),
                "decoy must NOT appear in results"
            );
        });
    }

    #[test]
    fn search_survives_unwritable_index() {
        let tmp = temp_home();
        with_home(&tmp, || {
            let dir = crate::config::config_dir().join("runbooks");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("deploy-checklist.md"),
                "# Checklist\n\nReview all items before release.\n",
            )
            .unwrap();

            let _ = crate::memory::index::open_index();
            let index_path = crate::config::memory_index_path();
            let index_dir = index_path.parent().unwrap();
            let original_perms = std::fs::metadata(index_dir).unwrap().permissions();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(index_dir, std::fs::Permissions::from_mode(0o000))
                    .unwrap();
            }

            let results = search_repository("deploy", "runbooks", 0);
            assert!(
                results.iter().any(|r| r.name == "deploy-checklist"),
                "filename match should still work when index is unwritable"
            );

            std::fs::set_permissions(index_dir, original_perms).unwrap();
        });
    }

    #[test]
    fn turns_kind_finds_archived_turn() {
        let tmp = temp_home();
        with_home(&tmp, || {
            let session_id = format!(
                "test-sess-{}",
                COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            );
            let sessions_dir = crate::config::sessions_dir();
            std::fs::create_dir_all(&sessions_dir).unwrap();

            let archive_path = crate::daemon::session::archive_file(&session_id);
            let line = r#"{"role":"user","content":"tell me about quokka recovery"}"#;
            std::fs::write(&archive_path, format!("{line}\n")).unwrap();

            crate::memory::index::index_turn(&session_id, 1, 0, "tell me about quokka recovery")
                .unwrap();

            let results = search_repository("quokka recovery", "turns", 0);
            assert!(
                !results.is_empty(),
                "turns search should find archived turn"
            );
            assert!(
                results
                    .iter()
                    .any(|r| r.name.contains(&session_id) && r.name.contains("turn 1")),
                "result name should contain session id and turn number. Got: {:?}",
                results.iter().map(|r| &r.name).collect::<Vec<_>>()
            );
        });
    }

    #[test]
    fn turns_hit_shows_tool_result_text() {
        let tmp = temp_home();
        with_home(&tmp, || {
            let session_id = format!(
                "test-sess-tr-{}",
                COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            );
            let sessions_dir = crate::config::sessions_dir();
            std::fs::create_dir_all(&sessions_dir).unwrap();

            let archive_path = crate::daemon::session::archive_file(&session_id);
            // Message with empty content but a tool_result containing the search term
            let line = r#"{"role":"assistant","content":"","tool_results":[{"tool_call_id":"tc1","tool_name":"disk_check","content":"disk usage at 95 percent on sda1"}]}"#;
            std::fs::write(&archive_path, format!("{line}\n")).unwrap();

            crate::memory::index::index_turn(&session_id, 1, 0, "disk usage at 95 percent on sda1")
                .unwrap();

            let results = search_repository("disk usage", "turns", 0);
            assert!(
                !results.is_empty(),
                "turns search should find a turn matching only in tool_results"
            );
            // The matched_line must include the tool result text
            let matched = results.iter().find(|r| r.kind == "turns");
            assert!(matched.is_some(), "should have a turns result");
            assert!(
                matched
                    .unwrap()
                    .matched_line
                    .contains("disk usage at 95 percent"),
                "matched_line must include tool result text. Got: {:?}",
                results.iter().map(|r| &r.matched_line).collect::<Vec<_>>()
            );
        });
    }

    #[test]
    fn epochs_kind_finds_narrative() {
        let _lock = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().unwrap();

        let session_id = format!(
            "test-sess-ep-{}",
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        crate::memory::index::index_epoch(
            &session_id,
            1,
            "compaction",
            "The system experienced a cascading failure during the quokka migration",
        )
        .unwrap();

        let results = search_repository("cascading failure", "epochs", 0);
        assert!(
            !results.is_empty(),
            "epochs search should find narrative by free text"
        );
        assert!(
            results
                .iter()
                .any(|r| r.name.contains(&session_id) && r.name.contains("epoch 1")),
            "result name should contain session id and seq. Got: {:?}",
            results.iter().map(|r| &r.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn turns_results_are_rank_ordered() {
        let tmp = temp_home();
        with_home(&tmp, || {
            let session_id = format!(
                "test-sess-rank-t-{}",
                COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            );
            let sessions_dir = crate::config::sessions_dir();
            std::fs::create_dir_all(&sessions_dir).unwrap();

            let archive_path = crate::daemon::session::archive_file(&session_id);
            // Write two lines: the weaker match first, the stronger match second.
            // The stronger match (written last) should be returned first.
            let weak = r#"{"role":"user","content":"the quick brown fox quokka"}"#;
            let strong = r#"{"role":"user","content":"quokka quokka quokka quokka quokka"}"#;
            let content = format!("{weak}\n{strong}\n");
            std::fs::write(&archive_path, &content).unwrap();

            let weak_offset = 0u64;
            let strong_offset = (weak.len() + 1) as u64;

            crate::memory::index::index_turn(
                &session_id,
                1,
                weak_offset,
                "the quick brown fox quokka",
            )
            .unwrap();
            crate::memory::index::index_turn(
                &session_id,
                2,
                strong_offset,
                "quokka quokka quokka quokka quokka",
            )
            .unwrap();

            let results = search_repository("quokka", "turns", 0);
            assert!(results.len() >= 2, "should find both turns");
            // The stronger match (turn 2, more occurrences of "quokka") should be first
            assert!(
                results[0].name.contains("turn 2"),
                "best-ranked turn (turn 2) should come first. Got: {:?}",
                results.iter().map(|r| &r.name).collect::<Vec<_>>()
            );
        });
    }

    #[test]
    fn epochs_results_are_rank_ordered() {
        let _lock = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        crate::config::Config::ensure_dirs().unwrap();

        let session_id = format!(
            "test-sess-rank-e-{}",
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        // Write the weaker match first, the stronger match last.
        crate::memory::index::index_epoch(
            &session_id,
            1,
            "compaction",
            "the quick brown fox quokka",
        )
        .unwrap();
        crate::memory::index::index_epoch(
            &session_id,
            2,
            "compaction",
            "quokka quokka quokka quokka quokka",
        )
        .unwrap();

        let results = search_repository("quokka", "epochs", 0);
        assert!(results.len() >= 2, "should find both epochs");
        // The stronger match (seq 2) should be first
        assert!(
            results[0].name.contains("epoch 2"),
            "best-ranked epoch (epoch 2) should come first. Got: {:?}",
            results.iter().map(|r| &r.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn all_kind_excludes_turns_and_epochs() {
        let tmp = temp_home();
        with_home(&tmp, || {
            // Write a turn
            let session_id = format!(
                "test-sess-all-{}",
                COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            );
            let sessions_dir = crate::config::sessions_dir();
            std::fs::create_dir_all(&sessions_dir).unwrap();

            let archive_path = crate::daemon::session::archive_file(&session_id);
            let line = r#"{"role":"user","content":"needle in the haystack"}"#;
            std::fs::write(&archive_path, format!("{line}\n")).unwrap();
            crate::memory::index::index_turn(&session_id, 1, 0, "needle in the haystack").unwrap();

            // Write an epoch
            crate::memory::index::index_epoch(
                &session_id,
                1,
                "compaction",
                "needle in the haystack",
            )
            .unwrap();

            let results = search_repository("needle", "all", 0);
            for r in &results {
                assert!(
                    r.kind != "turns",
                    "kind='all' must NOT include turns. Found kind={}",
                    r.kind
                );
                assert!(
                    r.kind != "epochs",
                    "kind='all' must NOT include epochs. Found kind={}",
                    r.kind
                );
            }
        });
    }

    #[test]
    fn turns_hit_with_missing_archive_is_skipped() {
        let tmp = temp_home();
        with_home(&tmp, || {
            let session_id = format!(
                "test-sess-missing-{}",
                COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            );
            // Index a turn but do NOT create the archive file
            crate::memory::index::index_turn(&session_id, 1, 0, "some content here").unwrap();

            // Should not panic — the missing archive is silently skipped
            let results = search_repository("some content", "turns", 0);
            assert!(
                results.is_empty(),
                "missing archive file should produce empty results, not a panic"
            );
        });
    }

    #[test]
    fn new_kinds_survive_unwritable_index() {
        let tmp = temp_home();
        with_home(&tmp, || {
            let _ = crate::memory::index::open_index();
            let index_path = crate::config::memory_index_path();
            let index_dir = index_path.parent().unwrap();
            let original_perms = std::fs::metadata(index_dir).unwrap().permissions();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(index_dir, std::fs::Permissions::from_mode(0o000))
                    .unwrap();
            }

            // Both new kinds should return empty (not panic) when index is unwritable
            let results = search_repository("anything", "turns", 0);
            assert!(
                results.is_empty(),
                "turns should return empty on unwritable index"
            );

            let results = search_repository("anything", "epochs", 0);
            assert!(
                results.is_empty(),
                "epochs should return empty on unwritable index"
            );

            std::fs::set_permissions(index_dir, original_perms).unwrap();
        });
    }
}
