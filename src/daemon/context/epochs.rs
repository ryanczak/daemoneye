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

/// Render the epoch chain for the working-set head. The last RENDER_EPOCHS (8)
/// epochs, newest last — each as "Epoch {seq} (turns {a}–{b}): {line}" where
/// {line} is the narrative (trimmed to one paragraph) or, when absent, a tally
/// one-liner. Older epochs collapse to a single line:
/// "…{n} earlier epochs — chapter rollups arrive in a later phase."
/// (Phase 06 replaces that line with ledger + chapters.)
pub fn render_context_block(epochs: &[EpochRecord]) -> String {
    let mut out = String::new();

    if epochs.is_empty() {
        return out;
    }

    let total = epochs.len();
    let recent_start = total.saturating_sub(RENDER_EPOCHS);
    let recent = &epochs[recent_start..];

    if recent_start > 0 {
        out.push_str(&format!(
            "…{} earlier epochs — chapter rollups arrive in a later phase.\n",
            recent_start
        ));
    }

    for e in recent {
        let line = if let Some(ref n) = e.narrative {
            // Take only the first paragraph (split on blank line)
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
            "Epoch {} (turns {}–{}): {}\n",
            e.seq, e.turn_start, e.turn_end, line
        ));
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
         Older turns are preserved in the session archive.",
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
    use std::io::Write;

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
        let _lock = crate::TEST_HOME_LOCK.lock().unwrap();
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
        // Should show 8 recent epochs + 1 "…4 earlier epochs" line.
        assert!(rendered.contains("…4 earlier epochs"));
        // Epochs 5-12 should be present. Match the trailing " (" so "Epoch 1"
        // does not spuriously match "Epoch 12".
        for i in 5..=12 {
            assert!(rendered.contains(&format!("Epoch {} (", i)));
        }
        // Epochs 1-4 should be absent.
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
}
