//! Epoch records — append-only persistence, span-windowed tally/scan, and
//! epoch-chain compaction.
//!
//! This module owns the append-only epoch persistence layer, the span-windowed
//! tally/artifact-scan functions, and the epoch-chain compaction helpers
//! (`compact_with_epochs`, `render_context_block`).

use crate::ai::Message;
use crate::config;
use std::io::Write;
use std::path::PathBuf;

/// Cap on how many entries each list field of an EpochTally retains; the
/// paired `_count` field always carries the true total.
pub const TALLY_LIST_CAP: usize = 10;

/// Number of oldest uncovered epochs folded per rollup.
const ROLLUP_FOLD: usize = 5;

/// How many recent epochs to render in the context block.
pub const RENDER_EPOCHS: usize = 8;

/// Serializable per-span event tally. List fields are CAPPED at
/// `TALLY_LIST_CAP` entries; `_count` fields carry the true totals.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct EpochTally {
    pub commands_ok: u32,
    pub commands_fail: u32,
    pub failed_cmds: Vec<(String, i32)>, // capped
    pub files_edited_count: u32,
    pub files_edited: Vec<String>, // capped
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub alerts_count: u32,
    pub alerts: Vec<String>, // capped
    pub ghost_starts: u32,
    pub ghost_completions: u32,
}

impl EpochTally {
    /// Element-wise merge: sum scalar fields, re-cap list fields at
    /// `TALLY_LIST_CAP`. The `_count` fields always carry the exact total.
    pub fn merge(&mut self, other: &EpochTally) {
        self.commands_ok += other.commands_ok;
        self.commands_fail += other.commands_fail;
        self.files_edited_count += other.files_edited_count;
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.alerts_count += other.alerts_count;
        self.ghost_starts += other.ghost_starts;
        self.ghost_completions += other.ghost_completions;
        // Merge lists, re-capping at TALLY_LIST_CAP.
        self.failed_cmds.extend(other.failed_cmds.iter().cloned());
        self.failed_cmds.truncate(TALLY_LIST_CAP);
        self.files_edited.extend(other.files_edited.iter().cloned());
        self.files_edited.truncate(TALLY_LIST_CAP);
        self.alerts.extend(other.alerts.iter().cloned());
        self.alerts.truncate(TALLY_LIST_CAP);
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EpochRecord {
    pub seq: u32,
    /// "epoch" now; "chapter" arrives in phase 06.
    pub kind: String,
    pub turn_start: u32, // 0 when unknown (legacy messages)
    pub turn_end: u32,
    pub ts_start: chrono::DateTime<chrono::Utc>,
    pub ts_end: chrono::DateTime<chrono::Utc>,
    pub msg_count: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub narrative: Option<String>,
    pub tally: EpochTally,
    /// "runbook:name" / "script:name" / "memory:key [category]" /
    /// "schedule:name (kind)" strings.
    pub artifacts: Vec<String>,
    /// Phase 06: seq range this chapter covers. None for plain epochs.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub covers: Option<(u32, u32)>,
}

/// `sessions_dir()/<id>.epochs.jsonl`.
pub fn epochs_file(id: &str) -> PathBuf {
    config::sessions_dir().join(format!("{}.epochs.jsonl", id))
}

/// Read the whole epoch chain in order; empty Vec on absent/unreadable file
/// (never errors — a missing chain is a fresh session). Skip malformed lines.
pub fn read_epochs(id: &str) -> Vec<EpochRecord> {
    let path = epochs_file(id);
    let Ok(file) = std::fs::File::open(&path) else {
        return Vec::new();
    };
    let reader = std::io::BufReader::new(file);
    use std::io::BufRead;
    let mut records = Vec::new();
    for line_result in reader.lines() {
        let Ok(line) = line_result else {
            continue;
        };
        let Ok(record): Result<EpochRecord, _> = serde_json::from_str(&line) else {
            continue;
        };
        records.push(record);
    }
    records
}

/// Append one record as a single JSON line. Append-only: open with
/// OpenOptions::new().create(true).append(true). NEVER truncate/rewrite.
/// WARN + non-fatal on failure (mirror `append_session_message`).
pub fn append_epoch(id: &str, rec: &EpochRecord) {
    use std::fs::OpenOptions;
    let path = epochs_file(id);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        if let Ok(line) = serde_json::to_string(rec)
            && let Err(e) = writeln!(f, "{}", line)
        {
            log::warn!("Failed to append to epoch file {}: {}", path.display(), e);
        }
    } else {
        log::warn!("Failed to open epoch file {} for append", path.display());
    }
}

// ── Chapter rollup ─────────────────────────────────────────────────

/// An epoch is "uncovered" when kind == "epoch" and no chapter's `covers`
/// range contains its seq.
pub fn uncovered_epochs(all: &[EpochRecord]) -> Vec<&EpochRecord> {
    let covered_ranges: Vec<_> = all
        .iter()
        .filter_map(|r| if r.kind == "chapter" { r.covers } else { None })
        .collect();

    all.iter()
        .filter(|r| {
            r.kind == "epoch"
                && !covered_ranges
                    .iter()
                    .any(|(lo, hi)| r.seq >= *lo && r.seq <= *hi)
        })
        .collect()
}

/// Build a structured fallback narrative for a chapter from folded epochs.
/// Takes the first line of each epoch's narrative (or its tally one-liner),
/// joins with " · ", truncated to 500 chars at a char boundary.
fn build_chapter_fallback(folded: &[&EpochRecord]) -> String {
    let parts: Vec<String> = folded
        .iter()
        .map(|e| {
            if let Some(ref n) = e.narrative {
                n.lines().next().unwrap_or("").to_string()
            } else {
                format_tally_one_liner(&e.tally)
            }
        })
        .collect();
    let joined = parts.join(" · ");
    let truncated = joined
        .char_indices()
        .find(|&(i, _)| i >= 500)
        .map(|(i, _)| &joined[..i])
        .unwrap_or(&joined);
    truncated.to_string()
}

/// When uncovered count exceeds `rollup_after`, fold the ROLLUP_FOLD oldest
/// uncovered epochs into one chapter record and append it. Returns the
/// chapter record if one was created.
pub async fn maybe_rollup(id: &str, config: &crate::config::Config) -> Option<EpochRecord> {
    let all = read_epochs(id);
    let uncovered = uncovered_epochs(&all);
    let threshold = config.compaction.rollup_after;

    if uncovered.len() <= threshold as usize {
        return None;
    }

    let fold_count = ROLLUP_FOLD.min(uncovered.len());
    let folded: Vec<&EpochRecord> = uncovered[..fold_count].to_vec();

    let first = folded[0];
    let last = folded[fold_count - 1];

    // Compute union of spans and sum of tallies.
    let turn_start = first.turn_start;
    let turn_end = last.turn_end;
    let ts_start = folded
        .iter()
        .map(|e| e.ts_start)
        .min()
        .unwrap_or(first.ts_start);
    let ts_end = folded
        .iter()
        .map(|e| e.ts_end)
        .max()
        .unwrap_or(first.ts_end);
    let msg_count: u32 = folded.iter().map(|e| e.msg_count).sum();
    let mut tally = EpochTally::default();
    for e in &folded {
        tally.merge(&e.tally);
    }

    // Build chapter narrative.
    let narrative = if config.digest.narrative_enabled {
        let user_text = folded
            .iter()
            .map(|e| {
                let content = if let Some(ref n) = e.narrative {
                    n.lines()
                        .next()
                        .unwrap_or(&format_tally_one_liner(&e.tally))
                        .to_string()
                } else {
                    format_tally_one_liner(&e.tally)
                };
                format!(
                    "Epoch {} (turns {}–{}): {}",
                    e.seq, e.turn_start, e.turn_end, content
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        let model_entry =
            config.resolve_model(config.models.contains_key("digest").then_some("digest"));
        let system = "You are compacting an SRE assistant's session history. You will be shown 5 epoch summaries, each covering a span of conversation turns. Write ONE combined summary of at most 3 lines preserving: what was worked on, key outcomes/decisions, and anything still unresolved. Past tense, terse, no preamble.";
        crate::daemon::digest::summarize_once(system, &user_text, model_entry)
            .await
            .or(Some(build_chapter_fallback(&folded)))
    } else {
        Some(build_chapter_fallback(&folded))
    };

    // Next seq.
    let next_seq = all.last().map(|e| e.seq + 1).unwrap_or(1);

    let chapter = EpochRecord {
        seq: next_seq,
        kind: "chapter".to_string(),
        turn_start,
        turn_end,
        ts_start,
        ts_end,
        msg_count,
        narrative,
        tally,
        artifacts: Vec::new(),
        covers: Some((first.seq, last.seq)),
    };

    append_epoch(id, &chapter);
    Some(chapter)
}

// ── Context block rendering ────────────────────────────────────────

/// Format an EpochTally as a one-line summary (the tally one-liner).
fn format_tally_one_liner(t: &EpochTally) -> String {
    let total_cmds = t.commands_ok + t.commands_fail;
    let parts: Vec<String> = [
        if total_cmds > 0 {
            if t.commands_fail > 0 {
                format!("{} cmds ({} failed)", total_cmds, t.commands_fail)
            } else {
                format!("{} cmds", total_cmds)
            }
        } else {
            String::new()
        },
        if t.files_edited_count > 0 {
            format!("{} files edited", t.files_edited_count)
        } else {
            String::new()
        },
        if t.alerts_count > 0 {
            format!(
                "{} alert{}",
                t.alerts_count,
                if t.alerts_count == 1 { "" } else { "s" }
            )
        } else {
            String::new()
        },
        if t.ghost_starts > 0 {
            format!(
                "{} ghost start{}",
                t.ghost_starts,
                if t.ghost_starts == 1 { "" } else { "s" }
            )
        } else {
            String::new()
        },
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect();

    if parts.is_empty() {
        "no events".to_string()
    } else {
        parts.join(" · ")
    }
}

/// Render the epoch chain for the working-set head.
///
/// Layout:
/// - Ledger line: summed tallies across chapters + uncovered epochs (covered
///   epochs excluded to avoid double-counting).
/// - Chapters section: all chapters, oldest-first.
/// - Recent epochs: last 8 uncovered epochs, newest last.
///
/// The ledger is omitted when only one epoch exists (the epoch line already
/// says it all). The chapters section is omitted when no chapters exist.
pub fn render_context_block(epochs: &[EpochRecord]) -> String {
    let mut out = String::new();

    if epochs.is_empty() {
        return out;
    }

    // Collect chapters (all of them) and uncovered epochs.
    let chapters: Vec<&EpochRecord> = epochs.iter().filter(|r| r.kind == "chapter").collect();

    let uncovered = uncovered_epochs(epochs);

    // Compute ledger totals from chapters + uncovered epochs (covered epochs
    // are excluded to avoid double-counting).
    let ledger_records: Vec<&EpochRecord> = chapters
        .iter()
        .copied()
        .chain(uncovered.iter().copied())
        .collect();

    if !ledger_records.is_empty() {
        let (
            total_turns,
            total_cmds_ok,
            total_cmds_fail,
            total_files,
            total_alerts,
            total_ghosts,
            total_prompt,
            total_completion,
        ) = ledger_records.iter().fold(
            (0u32, 0u32, 0u32, 0u32, 0u32, 0u32, 0u64, 0u64),
            |(turns, cok, cfail, files, alerts, ghosts, pt, ct), e| {
                (
                    turns + e.turn_end - e.turn_start,
                    cok + e.tally.commands_ok,
                    cfail + e.tally.commands_fail,
                    files + e.tally.files_edited_count,
                    alerts + e.tally.alerts_count,
                    ghosts + e.tally.ghost_starts,
                    pt + e.tally.prompt_tokens,
                    ct + e.tally.completion_tokens,
                )
            },
        );

        out.push_str(&format!(
            "Session ledger: {} turns compacted across {} epochs — commands {} ok / {} failed \
             · files edited {} · alerts {} · ghosts {} · ~{}k prompt / ~{}k completion tokens\n",
            total_turns,
            epochs.len(),
            total_cmds_ok,
            total_cmds_fail,
            total_files,
            total_alerts,
            total_ghosts,
            (total_prompt as f64 / 1000.0).ceil() as u64,
            (total_completion as f64 / 1000.0).ceil() as u64,
        ));
    }

    // Render chapters oldest-first.
    if !chapters.is_empty() {
        out.push_str("Chapters:\n");
        for c in &chapters {
            let narrative = c
                .narrative
                .as_ref()
                .map(|n| {
                    n.split('\n')
                        .take_while(|l| !l.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ")
                        .trim()
                        .to_string()
                })
                .unwrap_or_else(|| format_tally_one_liner(&c.tally));
            if let Some((lo, hi)) = c.covers {
                out.push_str(&format!(
                    "  Chapter {} (turns {}–{}): {}\n",
                    c.seq, lo, hi, narrative
                ));
            }
        }
    }

    // Render recent uncovered epochs (last RENDER_EPOCHS).
    let recent_uncovered: Vec<&EpochRecord> = if uncovered.len() > RENDER_EPOCHS {
        uncovered[uncovered.len() - RENDER_EPOCHS..].to_vec()
    } else {
        uncovered.to_vec()
    };

    if !recent_uncovered.is_empty() {
        out.push_str("Recent epochs:\n");
        for e in &recent_uncovered {
            let line = if let Some(ref n) = e.narrative {
                n.split('\n')
                    .take_while(|l| !l.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
                    .trim()
                    .to_string()
            } else {
                format_tally_one_liner(&e.tally)
            };
            out.push_str(&format!(
                "  Epoch {} (turns {}–{}): {}\n",
                e.seq, e.turn_start, e.turn_end, line
            ));
        }
    }

    out
}

// ── Compaction helpers ─────────────────────────────────────────────

/// Return the minimum turn number in a message slice, or 0 if empty.
pub fn first_turn_of(msgs: &[Message]) -> u32 {
    msgs.iter()
        .filter_map(|m| m.turn)
        .min()
        .map(|v| v as u32)
        .unwrap_or(0)
}

/// Return the maximum turn number in a message slice, or 0 if empty.
pub fn last_turn_of(msgs: &[Message]) -> u32 {
    msgs.iter()
        .filter_map(|m| m.turn)
        .max()
        .map(|v| v as u32)
        .unwrap_or(0)
}

/// Layout: [synthetic user "[Session Context] …", synthetic assistant ack,
/// messages[tail_start..]]. `tail_start` MUST already be a clean/repaired
/// boundary (phase-03 planner). Returns `messages` unchanged when `tail_start`
/// is infeasible (`< 2` or `>= len`) — same guard as the legacy compactor.
pub fn compact_with_epochs(
    messages: Vec<Message>,
    rendered_context: &str,
    environment: &str,
    host: &str,
    turn_end: usize,
    tail_first_turn: usize,
    tail_start: usize,
) -> Vec<Message> {
    if tail_start < 2 || tail_start >= messages.len() {
        return messages;
    }

    let slot0_content = format!(
        "[Session Context — regenerated at compaction; turns 1..{} summarized]\n\
         Environment: {} · Daemon host: {}\n\n\
         {}\n\n\
         Older turns are preserved in the session archive — retrieve originals \
         with recall_context(query, turn_start, turn_end).",
        turn_end, environment, host, rendered_context
    );

    let slot0 = Message {
        role: "user".to_string(),
        content: slot0_content,
        tool_calls: None,
        tool_results: None,
        turn: None,
    };

    let slot1 = Message {
        role: "assistant".to_string(),
        content: format!(
            "Continuing session — the context above covers everything before turn {}.",
            tail_first_turn
        ),
        tool_calls: None,
        tool_results: None,
        turn: None,
    };

    let mut result = Vec::with_capacity(2 + messages.len() - tail_start);
    result.push(slot0);
    result.push(slot1);
    result.extend_from_slice(&messages[tail_start..]);
    result
}

/// Tally events for one session in the half-open window `[since, until)`.
pub fn tally_span(
    session_id: &str,
    since: chrono::DateTime<chrono::Utc>,
    until: chrono::DateTime<chrono::Utc>,
) -> EpochTally {
    let mut tally = EpochTally::default();

    crate::daemon::utils::for_each_event_between(Some(since), Some(until), &mut |v| {
        let event = v.get("event").and_then(|e| e.as_str()).unwrap_or("");
        let ev_session = v.get("session").and_then(|s| s.as_str()).unwrap_or("");
        // Also check session_id field (used by ghost events).
        let ev_session_id = v.get("session_id").and_then(|s| s.as_str()).unwrap_or("");

        let belongs = ev_session == session_id
            || ev_session_id == session_id
            || ev_session == "-"
            || ev_session.is_empty();

        match event {
            "ai_turn" if belongs => {
                tally.prompt_tokens += v.get("prompt_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
                tally.completion_tokens += v
                    .get("completion_tokens")
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0);
            }
            "job_complete" if belongs => {
                let code = v.get("exit_code").and_then(|n| n.as_i64()).unwrap_or(-1) as i32;
                if code == 0 {
                    tally.commands_ok += 1;
                } else {
                    tally.commands_fail += 1;
                    let name = v
                        .get("job_name")
                        .and_then(|s| s.as_str())
                        .unwrap_or("?")
                        .to_string();
                    if tally.failed_cmds.len() < TALLY_LIST_CAP {
                        tally.failed_cmds.push((name, code));
                    }
                }
            }
            "job_start" if belongs => {
                // bg_windows_created is not in the new EpochTally (05b removes it)
            }
            "gc_window" if belongs => {
                // bg_windows_closed is not in the new EpochTally (05b removes it)
            }
            "file_edit" if belongs => {
                if let Some(p) = v.get("path").and_then(|s| s.as_str()) {
                    tally.files_edited_count += 1;
                    if tally.files_edited.len() < TALLY_LIST_CAP {
                        tally.files_edited.push(p.to_string());
                    }
                }
            }
            "webhook_alert" => {
                // Global events — always relevant.
                if let Some(name) = v.get("alert_name").and_then(|s| s.as_str()) {
                    tally.alerts_count += 1;
                    if tally.alerts.len() < TALLY_LIST_CAP {
                        tally.alerts.push(name.to_string());
                    }
                }
            }
            "ghost_start" if belongs => {
                tally.ghost_starts += 1;
            }
            "ghost_complete" if belongs => {
                tally.ghost_completions += 1;
            }
            _ => {}
        }
    });

    tally
}

/// Artifacts whose mtime falls in `[since, until)`, as flat tag strings.
pub fn scan_artifacts_span(
    since: chrono::DateTime<chrono::Utc>,
    until: chrono::DateTime<chrono::Utc>,
) -> Vec<String> {
    let since_systime: std::time::SystemTime = since.into();
    let until_systime: std::time::SystemTime = until.into();
    let mut out = Vec::new();

    // Runbooks — format as "runbook:{name}"
    scan_dir_in_range(
        &crate::runbook::runbooks_dir(),
        since_systime,
        until_systime,
        &["md"],
        &mut out,
        |name| format!("runbook:{}", name),
    );

    // Scripts — format as "script:{name}"
    scan_dir_in_range(
        &crate::scripts::scripts_dir(),
        since_systime,
        until_systime,
        &[],
        &mut out,
        |name| format!("script:{}", name),
    );

    // Memories (three category subdirs) — format as "memory:{key} [{category}]"
    for (category, dir_name) in &[
        ("session", "session"),
        ("knowledge", "knowledge"),
        ("incident", "incidents"),
    ] {
        let dir = config::config_dir().join("memory").join(dir_name);
        scan_dir_in_range(
            &dir,
            since_systime,
            until_systime,
            &["md"],
            &mut out,
            |name| format!("memory:{} [{}]", name, category),
        );
    }

    // Schedules — check created_at field in schedules.json.
    if let Ok(text) = std::fs::read_to_string(config::Config::schedules_path())
        && let Ok(jobs) = serde_json::from_str::<Vec<serde_json::Value>>(&text)
    {
        for job in &jobs {
            let created = job
                .get("created_at")
                .and_then(|s| s.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&chrono::Utc));
            if let Some(created_at) = created
                && created_at >= since
                && created_at < until
            {
                let name = job
                    .get("name")
                    .and_then(|s| s.as_str())
                    .unwrap_or("?")
                    .to_string();
                let kind = job
                    .get("kind")
                    .and_then(|k| k.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("?")
                    .to_string();
                out.push(format!("schedule:{} ({})", name, kind));
            }
        }
    }

    out.sort();
    out
}

/// List files in `dir` whose mtime is in `[since, until)`, formatting names
/// via `format_fn`. If `extensions` is non-empty, only files with a matching
/// extension are included.
fn scan_dir_in_range(
    dir: &std::path::Path,
    since: std::time::SystemTime,
    until: std::time::SystemTime,
    extensions: &[&str],
    out: &mut Vec<String>,
    format_fn: impl Fn(&str) -> String,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let Ok(mtime) = meta.modified() else {
            continue;
        };
        if mtime < since || mtime >= until {
            continue;
        }
        if !extensions.is_empty() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !extensions.contains(&ext) {
                continue;
            }
        }
        let name = entry
            .path()
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        out.push(format_fn(&name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::io::Write;

    fn make_test_epoch(seq: u32) -> EpochRecord {
        let ts = chrono::Utc::now();
        EpochRecord {
            seq,
            kind: "epoch".to_string(),
            turn_start: (seq - 1) * 5,
            turn_end: seq * 5,
            ts_start: ts,
            ts_end: ts,
            msg_count: 5,
            narrative: Some(format!("narrative for epoch {}", seq)),
            tally: EpochTally::default(),
            artifacts: Vec::new(),
            covers: None,
        }
    }

    fn make_test_epoch_with_tally(seq: u32, tally: EpochTally) -> EpochRecord {
        let ts = chrono::Utc::now();
        EpochRecord {
            seq,
            kind: "epoch".to_string(),
            turn_start: (seq - 1) * 5,
            turn_end: seq * 5,
            ts_start: ts,
            ts_end: ts,
            msg_count: 5,
            narrative: Some(format!("narrative for epoch {}", seq)),
            tally,
            artifacts: Vec::new(),
            covers: None,
        }
    }

    fn make_msg(role: &str, content: &str) -> Message {
        Message {
            role: role.to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_results: None,
            turn: None,
        }
    }

    fn with_test_home<F: FnOnce()>(f: F) {
        let _lock = crate::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = tempfile::tempdir().unwrap();
        let saved_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", tmp.path());
        }
        f();
        if let Some(h) = saved_home {
            unsafe {
                std::env::set_var("HOME", h);
            }
        } else {
            unsafe {
                std::env::remove_var("HOME");
            }
        }
    }

    #[test]
    fn compact_with_epochs_head_shape() {
        // Build 32 messages: alternating user/assistant.
        let messages: Vec<crate::ai::Message> = (0..32)
            .map(|i| {
                let role = if i % 2 == 0 { "user" } else { "assistant" };
                make_msg(role, &format!("msg-{}", i))
            })
            .collect();

        let result = compact_with_epochs(messages, "context", "env", "host", 0, 0, 16);

        // First slot is synthetic user with [Session Context] header.
        assert_eq!(result[0].role, "user");
        assert!(result[0].content.contains("[Session Context"));
        // Second slot is synthetic assistant ack.
        assert_eq!(result[1].role, "assistant");
        assert!(result[1].content.contains("Continuing session"));
        // Tail starts at the clean boundary (index 16 = user).
        assert_eq!(result[2].role, "user");
        assert_eq!(result[2].content, "msg-16");
        // Original msg0 is absent from the working set (D7 fix).
        assert!(!result.iter().any(|m| m.content == "msg-0"));
    }

    #[test]
    fn render_context_block_caps_at_eight() {
        // Build 12 epoch records.
        let mut epochs = Vec::new();
        for i in 1..=12 {
            epochs.push(EpochRecord {
                seq: i,
                kind: "epoch".to_string(),
                turn_start: (i - 1) * 10,
                turn_end: i * 10,
                ts_start: chrono::Utc::now(),
                ts_end: chrono::Utc::now(),
                msg_count: 10,
                narrative: Some(format!("narrative for epoch {}", i)),
                tally: EpochTally::default(),
                artifacts: Vec::new(),
                covers: None,
            });
        }

        let rendered = render_context_block(&epochs);
        // Should have a ledger line.
        assert!(rendered.contains("Session ledger:"));
        // No chapters section since there are no chapters.
        assert!(rendered.contains("Recent epochs:"));
        // Should show only up to 8 recent epochs.
        // Epochs 5-12 should be present.
        for i in 5..=12 {
            assert!(rendered.contains(&format!("Epoch {} (", i)));
        }
        // Epochs 1-4 should be absent from the recent section.
        for i in 1..=4 {
            assert!(!rendered.contains(&format!("Epoch {} (", i)));
        }
    }

    #[test]
    fn render_context_block_uses_tally_one_liner_when_no_narrative() {
        let epochs = vec![EpochRecord {
            seq: 1,
            kind: "epoch".to_string(),
            turn_start: 0,
            turn_end: 10,
            ts_start: chrono::Utc::now(),
            ts_end: chrono::Utc::now(),
            msg_count: 10,
            narrative: None,
            tally: EpochTally {
                commands_ok: 5,
                commands_fail: 2,
                files_edited_count: 3,
                alerts_count: 1,
                ghost_starts: 0,
                ..Default::default()
            },
            artifacts: Vec::new(),
            covers: None,
        }];

        let rendered = render_context_block(&epochs);
        assert!(rendered.contains("7 cmds (2 failed)"));
        assert!(rendered.contains("3 files edited"));
        assert!(rendered.contains("1 alert"));
    }

    #[test]
    fn epoch_records_append_and_read_roundtrip() {
        with_test_home(|| {
            let id = "test-session";
            let rec = EpochRecord {
                seq: 1,
                kind: "epoch".to_string(),
                turn_start: 0,
                turn_end: 10,
                ts_start: chrono::Utc::now(),
                ts_end: chrono::Utc::now(),
                msg_count: 10,
                narrative: Some("test narrative".to_string()),
                tally: EpochTally::default(),
                artifacts: Vec::new(),
                covers: None,
            };
            append_epoch(id, &rec);
            let records = read_epochs(id);
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].seq, 1);
            assert_eq!(records[0].kind, "epoch");
            assert_eq!(records[0].narrative.as_ref().unwrap(), "test narrative");
        });
    }

    #[test]
    fn epoch_spans_are_disjoint_and_tallies_scoped() {
        with_test_home(|| {
            let events_dir = config::events_dir();
            let _ = std::fs::create_dir_all(&events_dir);

            let seg = events_dir.join("events-20240115.jsonl");

            // Window 1: 15:00-16:00 — write 2 job_complete (exit_code 0)
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&seg)
                .unwrap();
            writeln!(
                file,
                r#"{{"ts": "2024-01-15T15:30:00+00:00", "event": "job_complete", "session": "test-session", "exit_code": 0, "job_name": "cmd1"}}"#
            )
            .unwrap();
            writeln!(
                file,
                r#"{{"ts": "2024-01-15T15:45:00+00:00", "event": "job_complete", "session": "test-session", "exit_code": 0, "job_name": "cmd2"}}"#
            )
            .unwrap();

            // Window 2: 16:00-17:00 — write 1 job_complete (exit_code 1)
            writeln!(
                file,
                r#"{{"ts": "2024-01-15T16:30:00+00:00", "event": "job_complete", "session": "test-session", "exit_code": 1, "job_name": "cmd3"}}"#
            )
            .unwrap();
            drop(file);

            let since1 = chrono::NaiveDate::from_ymd_opt(2024, 1, 15)
                .unwrap()
                .and_hms_opt(15, 0, 0)
                .unwrap()
                .and_utc();
            let until1 = chrono::NaiveDate::from_ymd_opt(2024, 1, 15)
                .unwrap()
                .and_hms_opt(16, 0, 0)
                .unwrap()
                .and_utc();
            let since2 = until1;
            let until2 = chrono::NaiveDate::from_ymd_opt(2024, 1, 15)
                .unwrap()
                .and_hms_opt(17, 0, 0)
                .unwrap()
                .and_utc();

            let tally1 = tally_span("test-session", since1, until1);
            assert_eq!(tally1.commands_ok, 2);
            assert_eq!(tally1.commands_fail, 0);

            let tally2 = tally_span("test-session", since2, until2);
            assert_eq!(tally2.commands_ok, 0);
            assert_eq!(tally2.commands_fail, 1);
        });
    }

    #[test]
    fn tally_lists_capped_counts_exact() {
        with_test_home(|| {
            let events_dir = config::events_dir();
            let _ = std::fs::create_dir_all(&events_dir);

            let seg = events_dir.join("events-20240115.jsonl");
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&seg)
                .unwrap();

            // Write 15 job_complete failures
            for i in 0..15 {
                writeln!(
                    file,
                    r#"{{"ts": "2024-01-15T15:00:00+00:00", "event": "job_complete", "session": "test-session", "exit_code": 1, "job_name": "cmd{i}"}}"#
                )
                .unwrap();
            }
            drop(file);

            // Events are stamped at 15:00:00; the window must bracket that.
            let since = chrono::NaiveDate::from_ymd_opt(2024, 1, 15)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc();
            let until = since + chrono::Duration::hours(24);

            let tally = tally_span("test-session", since, until);
            assert_eq!(tally.commands_fail, 15);
            assert_eq!(tally.failed_cmds.len(), TALLY_LIST_CAP);
        });
    }

    #[test]
    fn scan_artifacts_span_until_bound_excludes_newer() {
        with_test_home(|| {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path().join("test_artifacts");
            std::fs::create_dir_all(&dir).unwrap();

            // Artifact with mtime inside the window
            let old = dir.join("old.md");
            std::fs::write(&old, "content").unwrap();
            let since = chrono::NaiveDate::from_ymd_opt(2024, 1, 15)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc();
            let until = since + chrono::Duration::hours(1);

            // Set mtime to inside the window
            let mtime = filetime::FileTime::from_system_time(since.into());
            filetime::set_file_mtime(&old, mtime).unwrap();

            // Artifact with mtime == until (should be excluded — half-open)
            let new = dir.join("new.md");
            std::fs::write(&new, "content").unwrap();
            let new_mtime = filetime::FileTime::from_system_time(until.into());
            filetime::set_file_mtime(&new, new_mtime).unwrap();

            // Test the helper directly
            let mut out = Vec::new();
            scan_dir_in_range(
                &dir,
                since.into(),
                until.into(),
                &["md"],
                &mut out,
                |name| name.to_string(),
            );

            // Only "old" should be included; "new" has mtime == until (excluded)
            assert_eq!(out.len(), 1);
            assert_eq!(out[0], "old");
        });
    }

    /// RAII test-home guard: holds the `TEST_HOME_LOCK`, points `HOME` at a
    /// fresh tempdir, and **restores the original `HOME` on drop** so the env is
    /// not leaked into subsequent tests.
    struct TestHome {
        tmp: tempfile::TempDir,
        _lock: std::sync::MutexGuard<'static, ()>,
        saved_home: Option<String>,
    }

    impl TestHome {
        fn path(&self) -> &std::path::Path {
            self.tmp.path()
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            unsafe {
                match &self.saved_home {
                    Some(h) => std::env::set_var("HOME", h),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }

    fn setup_test_env() -> TestHome {
        let lock = crate::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let saved_home = std::env::var("HOME").ok();
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("HOME", tmp.path());
        }
        TestHome {
            tmp,
            _lock: lock,
            saved_home,
        }
    }

    // ── Rollup and ledger tests ──────────────────────────────────

    #[test]
    fn tally_merge_sums_and_recaps() {
        let mut t1 = EpochTally {
            commands_ok: 5,
            commands_fail: 2,
            failed_cmds: vec![("cmd1".to_string(), 0), ("cmd2".to_string(), 1)],
            files_edited_count: 3,
            files_edited: vec!["f1".to_string(), "f2".to_string()],
            prompt_tokens: 100,
            completion_tokens: 200,
            alerts_count: 1,
            alerts: vec!["alert1".to_string()],
            ghost_starts: 1,
            ghost_completions: 1,
        };
        let t2 = EpochTally {
            commands_ok: 3,
            commands_fail: 1,
            failed_cmds: vec![("cmd3".to_string(), 0)],
            files_edited_count: 2,
            files_edited: vec!["f3".to_string()],
            prompt_tokens: 50,
            completion_tokens: 150,
            alerts_count: 2,
            alerts: vec!["alert2".to_string()],
            ghost_starts: 0,
            ghost_completions: 2,
        };
        t1.merge(&t2);
        assert_eq!(t1.commands_ok, 8);
        assert_eq!(t1.commands_fail, 3);
        assert_eq!(t1.files_edited_count, 5);
        assert_eq!(t1.prompt_tokens, 150);
        assert_eq!(t1.completion_tokens, 350);
        assert_eq!(t1.alerts_count, 3);
        assert_eq!(t1.ghost_starts, 1);
        assert_eq!(t1.ghost_completions, 3);
        // Lists merged and capped at TALLY_LIST_CAP.
        assert_eq!(t1.failed_cmds.len(), 3);
        assert_eq!(t1.files_edited.len(), 3);
        assert_eq!(t1.alerts.len(), 2);
    }

    #[test]
    fn rollup_triggers_only_above_threshold() {
        let tmp = setup_test_env();
        let id = "rollup_threshold_test";
        let var_dir = tmp.path().join(".daemoneye/var/log/sessions");
        std::fs::create_dir_all(&var_dir).unwrap();

        // 10 uncovered epochs — no rollup (threshold is "exceeds", not "reaches").
        for i in 1..=10 {
            let e = make_test_epoch(i);
            append_epoch(id, &e);
        }
        let config = Config::default();
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(maybe_rollup(id, &config));
        assert!(result.is_none(), "10 epochs should not trigger rollup");

        // 11 uncovered epochs — one rollup.
        let e11 = make_test_epoch(11);
        append_epoch(id, &e11);
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(maybe_rollup(id, &config));
        assert!(result.is_some(), "11 epochs should trigger rollup");
        let chapter = result.unwrap();
        assert_eq!(chapter.kind, "chapter");
        assert_eq!(chapter.covers, Some((1, 5)));
    }

    #[test]
    fn rollup_chapter_fields_union_and_sum() {
        let tmp = setup_test_env();
        let id = "rollup_fields_test";
        let var_dir = tmp.path().join(".daemoneye/var/log/sessions");
        std::fs::create_dir_all(&var_dir).unwrap();

        // 11 epochs with distinct tallies.
        for i in 1..=11 {
            let e = make_test_epoch_with_tally(
                i,
                EpochTally {
                    commands_ok: i,
                    commands_fail: i * 2,
                    files_edited_count: i * 3,
                    prompt_tokens: (i * 100) as u64,
                    completion_tokens: (i * 200) as u64,
                    ..Default::default()
                },
            );
            append_epoch(id, &e);
        }
        let config = Config::default();
        let chapter = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(maybe_rollup(id, &config))
            .unwrap();

        // Check union of spans. Helper sets turn_start=(seq-1)*5, turn_end=seq*5,
        // so the chapter spans epoch 1's start (0) to epoch 5's end (25).
        assert_eq!(chapter.turn_start, 0);
        assert_eq!(chapter.turn_end, 25);

        // Check sum of tallies (epochs 1-5).
        let expected_ok: u32 = (1..=5).sum();
        let expected_fail: u32 = (1..=5).map(|i| i * 2).sum();
        let expected_files: u32 = (1..=5).map(|i| i * 3).sum();
        assert_eq!(chapter.tally.commands_ok, expected_ok);
        assert_eq!(chapter.tally.commands_fail, expected_fail);
        assert_eq!(chapter.tally.files_edited_count, expected_files);
    }

    #[test]
    fn rollup_folds_once_per_call() {
        let tmp = setup_test_env();
        let id = "rollup_once_test";
        let var_dir = tmp.path().join(".daemoneye/var/log/sessions");
        std::fs::create_dir_all(&var_dir).unwrap();

        // 30 uncovered epochs — one call produces exactly one chapter.
        for i in 1..=30 {
            let e = make_test_epoch(i);
            append_epoch(id, &e);
        }
        let config = Config::default();
        let chapter = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(maybe_rollup(id, &config))
            .unwrap();

        // Only the first 5 are folded.
        assert_eq!(chapter.covers, Some((1, 5)));

        // 25 uncovered remain + 1 chapter.
        let all = read_epochs(id);
        let uncovered = uncovered_epochs(&all);
        assert_eq!(uncovered.len(), 25);
    }

    #[test]
    fn ledger_excludes_covered_epochs() {
        let tmp = setup_test_env();
        let id = "ledger_test";
        let var_dir = tmp.path().join(".daemoneye/var/log/sessions");
        std::fs::create_dir_all(&var_dir).unwrap();

        // 11 epochs with known tallies.
        for i in 1..=11 {
            let e = make_test_epoch_with_tally(
                i,
                EpochTally {
                    commands_ok: i,
                    ..Default::default()
                },
            );
            append_epoch(id, &e);
        }

        // Compute sum of all 11 epochs' tallies.
        let all = read_epochs(id);
        let total_ok: u32 = all.iter().map(|e| e.tally.commands_ok).sum();

        // Do the rollup.
        let config = Config::default();
        let _chapter = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(maybe_rollup(id, &config))
            .unwrap();

        // Read fresh (includes the chapter).
        let all_after = read_epochs(id);
        let rendered = render_context_block(&all_after);

        // The ledger's commands_ok should equal the sum of all original epochs.
        // (Chapter covers 1-5, uncovered are 6-11; ledger sums chapter+uncovered.)
        assert!(rendered.contains(&format!("commands {} ok", total_ok)));
    }

    #[test]
    fn render_with_chapters_and_recent() {
        let tmp = setup_test_env();
        let id = "render_test";
        let var_dir = tmp.path().join(".daemoneye/var/log/sessions");
        std::fs::create_dir_all(&var_dir).unwrap();

        // 11 epochs → rollup.
        for i in 1..=11 {
            let e = make_test_epoch(i);
            append_epoch(id, &e);
        }
        let config = Config::default();
        let _chapter = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(maybe_rollup(id, &config))
            .unwrap();

        let all = read_epochs(id);
        let rendered = render_context_block(&all);

        // Should contain ledger.
        assert!(rendered.contains("Session ledger:"));
        // Should contain chapters section.
        assert!(rendered.contains("Chapters:"));
        assert!(rendered.contains("Chapter"));
        // Should contain recent epochs section.
        assert!(rendered.contains("Recent epochs:"));
        // Covered epochs (1-5) should NOT appear in recent.
        for i in 1..=5 {
            assert!(
                !rendered.contains(&format!("Epoch {} (", i)),
                "Covered epoch {} should not appear in recent epochs",
                i
            );
        }
    }

    #[test]
    fn rollup_appends_never_rewrites() {
        let tmp = setup_test_env();
        let id = "append_test";
        let var_dir = tmp.path().join(".daemoneye/var/log/sessions");
        std::fs::create_dir_all(&var_dir).unwrap();

        // 11 epochs.
        for i in 1..=11 {
            let e = make_test_epoch(i);
            append_epoch(id, &e);
        }

        // Read file content before rollup.
        let file_path = var_dir.join(format!("{}.epochs.jsonl", id));
        let before = std::fs::read_to_string(&file_path).unwrap();

        // Do the rollup.
        let config = Config::default();
        let _chapter = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(maybe_rollup(id, &config))
            .unwrap();

        // Read file content after rollup.
        let after = std::fs::read_to_string(&file_path).unwrap();

        // Before should be a prefix of after (append-only).
        assert!(
            after.starts_with(&before),
            "File content was not append-only"
        );
    }

    #[test]
    fn rollup_with_narrative_disabled_uses_fallback() {
        let tmp = setup_test_env();
        let id = "fallback_test";
        let var_dir = tmp.path().join(".daemoneye/var/log/sessions");
        std::fs::create_dir_all(&var_dir).unwrap();

        // 11 epochs with narratives.
        for i in 1..=11 {
            let e = make_test_epoch(i);
            append_epoch(id, &e);
        }

        // Config with narrative disabled.
        let mut config = Config::default();
        config.digest.narrative_enabled = false;

        let chapter = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(maybe_rollup(id, &config))
            .unwrap();
        // Chapter narrative should be non-empty (structured fallback).
        assert!(
            chapter.narrative.is_some(),
            "Fallback narrative should be non-empty"
        );
        let narr = chapter.narrative.unwrap();
        assert!(!narr.is_empty());
    }

    #[test]
    fn uncovered_epochs_filters_correctly() {
        let epochs = vec![make_test_epoch(1), make_test_epoch(2), make_test_epoch(3)];
        let uncovered = uncovered_epochs(&epochs);
        assert_eq!(uncovered.len(), 3);

        // Add a chapter covering 1-2.
        let chapter = EpochRecord {
            seq: 4,
            kind: "chapter".to_string(),
            turn_start: 0,
            turn_end: 20,
            ts_start: chrono::Utc::now(),
            ts_end: chrono::Utc::now(),
            msg_count: 20,
            narrative: None,
            tally: EpochTally::default(),
            artifacts: Vec::new(),
            covers: Some((1, 2)),
        };
        let epochs = vec![
            make_test_epoch(1),
            make_test_epoch(2),
            make_test_epoch(3),
            chapter,
        ];
        let uncovered = uncovered_epochs(&epochs);
        assert_eq!(uncovered.len(), 1);
        assert_eq!(uncovered[0].seq, 3);
    }
}
