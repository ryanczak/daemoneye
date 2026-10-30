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

/// Search events: the FTS index first, which covers the whole history ranked
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
        for line in r
            .context_before
            .iter_mut()
            .chain(r.context_after.iter_mut())
        {
            *line = crate::ai::mask_sensitive(line);
        }
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
    seen: &std::collections::HashSet<(String, String)>,
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
        if readable.to_lowercase().contains(query_lower)
            && !seen.contains(&(seg_name.clone(), readable.clone()))
        {
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
#[path = "search_tests.rs"]
mod tests;
