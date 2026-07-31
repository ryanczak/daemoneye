//! Repo-hygiene gate: fail when any bug doc is `open` while its phase doc is `done`.
//!
//! This test reads the checked-in milestone doc tree, classifies each bug doc
//! against its phase doc's status, and asserts that no violations exist.
//! It is an integration test (not compiled into the shipped binary) because it
//! reads from the repo's own documentation tree.

use std::path::Path;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct BugRecord {
    /// Repo-relative path, e.g. "M2-tui-renderer/bugs/bug-phase-01-1.md".
    doc: String,
    /// First token of the bug doc's header Status line, lowercased.
    bug_status: String,
    /// Phase id parsed from the filename, e.g. "01", "02b".
    phase_id: String,
    /// First token of the phase doc's header Status line — None if no phase doc matched.
    phase_status: Option<String>,
}

#[derive(Debug, PartialEq)]
enum Finding {
    OpenBugOnDonePhase { doc: String, phase_id: String },
    UnknownBugStatus { doc: String, status: String },
    DanglingBug { doc: String, phase_id: String },
}

// ---------------------------------------------------------------------------
// Pure classifier
// ---------------------------------------------------------------------------

/// Classify bug records against their phase statuses.
///
/// Returns a vec of findings.  A record produces at most one finding.
fn classify(records: &[BugRecord]) -> Vec<Finding> {
    let terminal_bug_statuses = ["open", "closed", "fixed", "resolved", "verified"];

    records
        .iter()
        .filter_map(|r| match &r.phase_status {
            None => Some(Finding::DanglingBug {
                doc: r.doc.clone(),
                phase_id: r.phase_id.clone(),
            }),
            Some(_) if !terminal_bug_statuses.contains(&r.bug_status.as_str()) => {
                Some(Finding::UnknownBugStatus {
                    doc: r.doc.clone(),
                    status: r.bug_status.clone(),
                })
            }
            Some(ps) if r.bug_status == "open" && ps == "done" => {
                Some(Finding::OpenBugOnDonePhase {
                    doc: r.doc.clone(),
                    phase_id: r.phase_id.clone(),
                })
            }
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Header-status parser
// ---------------------------------------------------------------------------

/// Extract the first token of the first `**Status:**` line in a markdown file.
///
/// Returns `None` if no such line is found.  The returned token is lowercased
/// and has trailing `(`, `,`, `.`, `—` characters stripped.
fn header_status(text: &str) -> Option<String> {
    let marker = "**Status:**";
    for line in text.lines() {
        if let Some(after) = line.strip_prefix(marker) {
            let first_token = after
                .split_whitespace()
                .next()
                .map(|s| s.to_lowercase())
                .unwrap_or_default();
            let stripped = first_token.trim_matches(|c: char| matches!(c, '(' | ',' | '.' | '—'));
            if stripped.is_empty() {
                return None;
            }
            return Some(stripped.to_string());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Filename parser
// ---------------------------------------------------------------------------

/// Parse the phase id from a bug filename like `bug-phase-01-1.md` or `bug-09-1.md`.
///
/// Strips the leading `bug-`, then an optional `phase-`, then splits off the
/// trailing `-<n>`.  What remains is the phase id (e.g. "01", "02b", "09").
fn parse_phase_id(filename: &str) -> Option<String> {
    // Strip the .md extension
    let name = filename.strip_suffix(".md")?;
    // Strip the leading "bug-"
    let after_bug = name.strip_prefix("bug-")?;
    // Strip an optional "phase-"
    let after_phase = after_bug.strip_prefix("phase-").unwrap_or(after_bug);
    // Split off the trailing "-<n>" (the bug number)
    let phase_id = after_phase.rsplit_once('-')?.0;
    if phase_id.is_empty() {
        return None;
    }
    Some(phase_id.to_string())
}

// ---------------------------------------------------------------------------
// Scanner
// ---------------------------------------------------------------------------

/// Scan the milestone doc tree and return bug records.
///
/// Walks each milestone directory, finds `bugs/*.md` files, parses the phase
/// id from the filename, reads the bug's header status, locates the matching
/// phase doc, and reads its header status.
fn scan(milestones_dir: &Path) -> Vec<BugRecord> {
    let mut records = Vec::new();

    let entries = match std::fs::read_dir(milestones_dir) {
        Ok(e) => e,
        Err(_) => return records,
    };

    for mil_entry in entries.filter_map(|e| e.ok()) {
        if !mil_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }

        let mil_name = mil_entry.file_name().to_string_lossy().to_string();
        let bugs_dir = mil_entry.path().join("bugs");
        if !bugs_dir.is_dir() {
            continue;
        }

        let bug_entries = match std::fs::read_dir(&bugs_dir) {
            Ok(rd) => rd.filter_map(|e| e.ok()).collect::<Vec<_>>(),
            Err(_) => continue,
        };
        for bug_entry in bug_entries {
            let bug_path = bug_entry.path();
            let filename = bug_path.file_name().unwrap().to_string_lossy().to_string();
            if !filename.ends_with(".md") {
                continue;
            }

            let phase_id = match parse_phase_id(&filename) {
                Some(id) => id,
                None => continue,
            };

            let bug_text = match std::fs::read_to_string(&bug_path) {
                Ok(t) => t,
                Err(_) => continue,
            };

            let bug_status = match header_status(&bug_text) {
                Some(s) => s,
                None => continue,
            };

            // Find the phase doc by prefix match
            let phase_prefix = format!("phase-{}-", phase_id);
            let phase_doc = std::fs::read_dir(mil_entry.path())
                .ok()
                .into_iter()
                .flat_map(|rd| rd.filter_map(|e| e.ok()))
                .find_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.starts_with(&phase_prefix) && name.ends_with(".md") {
                        Some(e.path())
                    } else {
                        None
                    }
                });

            let phase_status = phase_doc.and_then(|p| {
                std::fs::read_to_string(&p)
                    .ok()
                    .and_then(|t| header_status(&t))
            });

            let rel_path = format!("{}/bugs/{}", mil_name, filename);

            records.push(BugRecord {
                doc: rel_path,
                bug_status,
                phase_id,
                phase_status,
            });
        }
    }

    records
}

// ---------------------------------------------------------------------------
// Tests — pure (no filesystem)
// ---------------------------------------------------------------------------

#[test]
fn header_status_reads_bare_word() {
    let text = "**Status:** open";
    assert_eq!(header_status(text), Some("open".to_string()));
}

#[test]
fn header_status_strips_trailing_prose() {
    assert_eq!(
        header_status("**Status:** closed 2026-07-30 — verified at review round 2"),
        Some("closed".to_string())
    );
    assert_eq!(
        header_status("**Status:** fixed (architect takeover, 2026-06-27)"),
        Some("fixed".to_string())
    );
}

#[test]
fn header_status_uses_first_occurrence_only() {
    let text = "**Status:** done
Some other content.
**Status:** All 5 functions converted. Build, clippy, fmt, and tests pass.";
    assert_eq!(header_status(text), Some("done".to_string()));
}

#[test]
fn open_bug_on_done_phase_is_a_finding() {
    let records = [BugRecord {
        doc: "M2-tui-renderer/bugs/bug-phase-01-1.md".to_string(),
        bug_status: "open".to_string(),
        phase_id: "01".to_string(),
        phase_status: Some("done".to_string()),
    }];
    let findings = classify(&records);
    assert_eq!(
        findings,
        vec![Finding::OpenBugOnDonePhase {
            doc: "M2-tui-renderer/bugs/bug-phase-01-1.md".to_string(),
            phase_id: "01".to_string(),
        }]
    );
}

#[test]
fn open_bug_on_in_progress_phase_is_clean() {
    let records = [BugRecord {
        doc: "M3-polish-maintenance/bugs/bug-phase-03-1.md".to_string(),
        bug_status: "open".to_string(),
        phase_id: "03".to_string(),
        phase_status: Some("in-progress".to_string()),
    }];
    let findings = classify(&records);
    assert!(findings.is_empty());
}

// ---------------------------------------------------------------------------
// Tests — real tree gate
// ---------------------------------------------------------------------------

#[test]
fn repository_bug_tracker_is_consistent() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let milestones = root.join("docs/dev/milestones");

    let records = scan(&milestones);
    let findings = classify(&records);

    assert!(
        findings.is_empty(),
        "Bug tracker violations found:\n{}",
        findings
            .iter()
            .map(|f| match f {
                Finding::OpenBugOnDonePhase { doc, phase_id } => {
                    format!("  - OpenBugOnDonePhase: {} (phase {})", doc, phase_id)
                }
                Finding::UnknownBugStatus { doc, status } => {
                    format!("  - UnknownBugStatus: {} (status {})", doc, status)
                }
                Finding::DanglingBug { doc, phase_id } => {
                    format!("  - DanglingBug: {} (phase {})", doc, phase_id)
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    );
}
