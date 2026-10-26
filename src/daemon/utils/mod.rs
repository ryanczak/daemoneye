pub use crate::util::UnpoisonExt;

mod event_log;
mod host;
mod keepalive;
mod log_rotation;
mod output;
mod response;
mod shell;
mod sudo;
mod warnings;

pub use event_log::*;
pub use host::*;
pub use keepalive::*;
pub use log_rotation::*;
pub use output::*;
pub use response::*;
pub use shell::*;
pub use sudo::*;
pub use warnings::*;

/// Delete session archive files (`*.archive.jsonl`) whose mtime is older than
/// `retention_days` days. 0 = keep forever (no-op). Active sessions (present
/// in the in-memory store) are never swept.
pub fn sweep_session_archives(
    retention_days: u32,
    active_sessions: &std::collections::HashSet<String>,
) {
    if retention_days == 0 {
        return;
    }

    let sessions_dir = crate::config::sessions_dir();
    let Ok(entries) = std::fs::read_dir(&sessions_dir) else {
        return;
    };

    let now = std::time::SystemTime::now();
    let cutoff = now - std::time::Duration::from_secs(retention_days as u64 * 86_400);

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".archive.jsonl") {
            continue;
        }

        let Ok(meta) = path.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified >= cutoff {
            continue;
        }

        // Extract session id from `id.archive.jsonl`
        let session_id = name.trim_end_matches(".archive.jsonl");
        if active_sessions.contains(session_id) {
            continue;
        }

        log::info!("sessions: deleting expired archive {}", path.display());
        if let Err(e) = std::fs::remove_file(&path) {
            log::warn!(
                "sessions: failed to delete archive {}: {}",
                path.display(),
                e
            );
        } else {
            if let Err(e) = crate::memory::index::remove_session_turns(session_id) {
                log::warn!(
                    "sessions: failed to remove index rows for {}: {}",
                    session_id,
                    e
                );
            }
        }
    }
}

/// Delete an inactive session's chat log (`<id>.jsonl`) and its resume state
/// (`<id>.meta.json`) when the log's mtime is older than `retention_days`
/// days. 0 = keep forever (no-op). Archives (`<id>.archive.jsonl`) and epoch
/// files (`<id>.epochs.jsonl`) are never touched: they also end in `.jsonl`,
/// and they feed the memory index.
pub fn sweep_session_logs(
    retention_days: u32,
    active_sessions: &std::collections::HashSet<String>,
) {
    if retention_days == 0 {
        return;
    }

    let sessions_dir = crate::config::sessions_dir();
    let Ok(entries) = std::fs::read_dir(&sessions_dir) else {
        return;
    };

    let now = std::time::SystemTime::now();
    let cutoff = now - std::time::Duration::from_secs(retention_days as u64 * 86_400);

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".jsonl")
            || name.ends_with(".archive.jsonl")
            || name.ends_with(".epochs.jsonl")
        {
            continue;
        }

        let Ok(meta) = path.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified >= cutoff {
            continue;
        }

        // Extract session id from `<id>.jsonl`
        let session_id = name.trim_end_matches(".jsonl");
        if active_sessions.contains(session_id) {
            continue;
        }

        log::info!("sessions: deleting expired chat log {}", path.display());
        if let Err(e) = std::fs::remove_file(&path) {
            log::warn!("sessions: failed to delete log {}: {}", path.display(), e);
            continue;
        }
        let meta_path = sessions_dir.join(format!("{}.meta.json", session_id));
        if meta_path.exists()
            && let Err(e) = std::fs::remove_file(&meta_path)
        {
            log::warn!(
                "sessions: failed to delete resume state {}: {}",
                meta_path.display(),
                e
            );
        }
    }
}

/// Delete pane log files (`*.log`) in `var/log/panes/` whose mtime is older
/// than `retention_days` days. 0 = keep forever (no-op).
pub fn sweep_pane_logs(retention_days: u32) {
    if retention_days == 0 {
        return;
    }

    let panes_dir = crate::config::pane_logs_dir();
    let Ok(entries) = std::fs::read_dir(&panes_dir) else {
        return;
    };

    let now = std::time::SystemTime::now();
    let cutoff = now - std::time::Duration::from_secs(retention_days as u64 * 86_400);

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".log") {
            continue;
        }

        let Ok(meta) = path.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified >= cutoff {
            continue;
        }

        log::info!("panes: deleting expired log {}", path.display());
        if let Err(e) = std::fs::remove_file(&path) {
            log::warn!("panes: failed to delete {}: {}", path.display(), e);
        }
    }
}

/// Delete asciicast logs (`*.cast`) and their `.meta.json` sidecars in
/// `var/log/shells/` whose mtime is older than `retention_days` days.
/// 0 = keep forever (no-op). A log whose shell is still registered is never
/// deleted, however old it is — a long-running shell that has been quiet past
/// the retention still owns its recording. A file whose stem does not start
/// with a valid shell id is kept too: if ownership cannot be read off the
/// name, the file cannot be proven not to belong to a live shell.
pub fn sweep_shell_logs(retention_days: u32, live_ids: &std::collections::BTreeSet<String>) {
    if retention_days == 0 {
        return;
    }

    let shells_dir = crate::config::shell_logs_dir();
    let Ok(entries) = std::fs::read_dir(&shells_dir) else {
        return;
    };

    let now = std::time::SystemTime::now();
    let cutoff = now - std::time::Duration::from_secs(retention_days as u64 * 86_400);

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // `meta_path_for` writes `<stem>.meta.json` (the producer's spelling);
        // a `.cast.meta.json` name is the legacy spelling the sweep once
        // recognised. Note `.cast` is checked after the two `.meta.json`
        // forms, so the arm order matches suffix specificity.
        let (extension, stem) = if let Some(stem) = name.strip_suffix(".cast.meta.json") {
            (".meta.json", stem)
        } else if let Some(stem) = name.strip_suffix(".meta.json") {
            (".meta.json", stem)
        } else if let Some(stem) = name.strip_suffix(".cast") {
            (".cast", stem)
        } else {
            continue;
        };

        // The id is the part before the first hyphen, mirroring the cast-name
        // grammar (`<id>-<started_unix>[.label].cast`).
        let Some((id_part, _)) = stem.split_once('-') else {
            continue;
        };
        if crate::shell::ShellId::parse(id_part).is_err() {
            continue;
        }

        let Ok(meta) = path.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified >= cutoff {
            continue;
        }

        if live_ids.contains(id_part) {
            continue;
        }

        log::info!(
            "shells: deleting expired {extension} log {}",
            path.display()
        );
        if let Err(e) = std::fs::remove_file(&path) {
            log::warn!("shells: failed to delete {}: {}", path.display(), e);
        }
    }
}

/// Delete mailbox files (`*.json`) in every agent's `agents/<name>/mailbox/`
/// directory whose mtime is older than `retention_days` days. 0 = keep forever
/// (no-op). An agent with no mailbox directory is silently skipped.
pub fn sweep_agent_mailboxes(retention_days: u32) {
    if retention_days == 0 {
        return;
    }

    let agents_dir = crate::agents::agents_dir();
    let Ok(agent_entries) = std::fs::read_dir(&agents_dir) else {
        return;
    };

    let now = std::time::SystemTime::now();
    let cutoff = now - std::time::Duration::from_secs(retention_days as u64 * 86_400);

    for agent_entry in agent_entries.filter_map(|e| e.ok()) {
        let mailbox = agent_entry.path().join("mailbox");
        let Ok(mailbox_entries) = std::fs::read_dir(&mailbox) else {
            continue;
        };

        for entry in mailbox_entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.ends_with(".json") {
                continue;
            }

            let Ok(meta) = path.metadata() else { continue };
            let Ok(modified) = meta.modified() else {
                continue;
            };
            if modified >= cutoff {
                continue;
            }

            log::info!("mailboxes: deleting expired entry {}", path.display());
            if let Err(e) = std::fs::remove_file(&path) {
                log::warn!("mailboxes: failed to delete {}: {}", path.display(), e);
            }
        }
    }
}

#[cfg(test)]
#[path = "sweep_tests.rs"]
mod sweep_tests;
