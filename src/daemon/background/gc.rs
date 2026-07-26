use crate::daemon::session::with_sessions;
use crate::daemon::utils::log_event;
use crate::ipc::Response;
use crate::tmux;

// ---------------------------------------------------------------------------
// Scheduled / watchdog job completion handler
// ---------------------------------------------------------------------------

pub struct OwnedJobInfo {
    pub pane_id: String,
    pub cmd: String,
    pub win_name: String,
}

/// Completion handler for scheduled and watchdog jobs (called from `server.rs`).
///
/// - Captures and archives pane output.
/// - Sends a `SystemMsg` notification to any listening chat client.
/// - **GC**: destroys the window on success (FR-1.2.10); leaves it open on
///   failure so the user can inspect it via `daemoneye schedule windows`.
pub async fn notify_job_completion(
    job: OwnedJobInfo,
    session: String,
    exit_code: i32,
    session_id: Option<String>,
    notify_tx: Option<tokio::sync::mpsc::UnboundedSender<Response>>,
    started_at: tokio::time::Instant,
) {
    let OwnedJobInfo {
        pane_id,
        cmd,
        win_name,
    } = job;
    let duration_ms = started_at.elapsed().as_millis() as u64;

    log_event(
        "job_complete",
        serde_json::json!({
            "session": session_id.as_deref().unwrap_or("-"),
            "job_id": win_name.split('-').next_back().unwrap_or(""),
            "job_name": win_name,
            "exit_code": exit_code,
            "duration_ms": duration_ms,
        }),
    );

    // Archive logs.
    let logs_dir = crate::config::pane_logs_dir();
    if let Err(e) = std::fs::create_dir_all(&logs_dir) {
        log::error!("Failed to create pane_logs directory: {}", e);
    } else if let Err(e) =
        tmux::pane::capture_pane_to_file(&pane_id, &logs_dir.join(format!("{}.log", win_name)))
    {
        log::error!("Failed to archive pane logs for {}: {}", win_name, e);
    }

    let status_word = if exit_code == 0 {
        "succeeded"
    } else {
        "failed"
    };
    let alert_msg = format!("`{}` {} in pane {}", cmd, status_word, pane_id);

    if let Some(ref tx) = notify_tx {
        let _ = tx.send(Response::SystemMsg(alert_msg));
    }

    // FR-1.2.10: destroy on success, leave open on failure for inspection.
    if exit_code == 0 {
        log_event(
            "gc_window",
            serde_json::json!({
                "session": session_id.as_deref().unwrap_or("-"),
                "win_name": win_name,
                "reason": "done",
            }),
        );
        if let Err(e) = tmux::kill_job_window(&session, &win_name) {
            log::error!("Failed to GC scheduled job window {}: {}", win_name, e);
        }
    }
    // On failure: leave open indefinitely. User closes manually or via
    // `daemoneye schedule windows`.
}

// ---------------------------------------------------------------------------
// Periodic background-window garbage collection
// ---------------------------------------------------------------------------

/// Pane state snapshot used by the GC decision logic.
pub(crate) struct PaneGcInfo {
    /// True when the pane's foreground process has exited (remain-on-exit mode).
    pub dead: bool,
    /// True when the pane is sitting at an idle shell prompt.
    pub idle_shell: bool,
    /// Unix timestamp of the last time the pane produced output.
    pub last_activity: u64,
}

/// How long a dead pane must have been idle before the GC kills its window.
const DEAD_THRESHOLD_SECS: u64 = 30;

/// How long a completed-command pane must sit idle at the shell prompt before
/// the GC reclaims its window.
const IDLE_THRESHOLD_SECS: u64 = 120;

const IDLE_SHELLS: &[&str] = &["bash", "sh", "zsh", "dash", "fish", "ksh", "tcsh", "csh"];

/// Pure decision function: given the current tracked windows and live pane
/// state, returns the pane IDs whose windows should be killed.
///
/// Separated from the tmux-calling executor so it can be unit-tested without
/// a running tmux server.
pub(crate) fn plan_gc_actions(
    windows: &[crate::daemon::session::BgWindowInfo],
    pane_info: &std::collections::HashMap<String, PaneGcInfo>,
    now_unix: u64,
) -> Vec<String> {
    let mut to_kill = Vec::new();
    for win in windows {
        match pane_info.get(&win.pane_id) {
            None => {
                // Pane no longer exists in tmux.
                to_kill.push(win.pane_id.clone());
            }
            Some(info) => {
                let elapsed = now_unix.saturating_sub(info.last_activity);
                let should_kill = (info.dead && elapsed > DEAD_THRESHOLD_SECS)
                    || (win.exit_code.is_some()
                        && info.idle_shell
                        && elapsed > IDLE_THRESHOLD_SECS);
                if should_kill {
                    to_kill.push(win.pane_id.clone());
                }
            }
        }
    }
    to_kill
}

/// Prefixes that identify daemon-managed windows, used for orphan detection.
const DAEMON_BG_PREFIXES: &[&str] = &[
    crate::daemon::BG_WINDOW_PREFIX,
    crate::daemon::SCHED_WINDOW_PREFIX,
    crate::daemon::GS_BG_WINDOW_PREFIX,
    crate::daemon::GS_SCHED_WINDOW_PREFIX,
];

/// Periodic garbage collector for background windows.
///
/// Called every 60 seconds by the `bg-window-gc` supervised task.
///
/// For each session's tracked `bg_windows`:
/// - Kills windows whose pane is gone, dead, or has been idle since completing.
///
/// Also scans all tmux panes for daemon-prefixed windows not tracked by any
/// session (orphans from a daemon restart or missed completion signal) and
/// kills those too.
struct GcKill {
    session_id: String,
    window_name: String,
    tmux_session: String,
    pane_id: String,
    reason: &'static str,
}

pub fn gc_bg_windows(sessions: &crate::daemon::session::SessionStore) {
    use std::collections::{HashMap, HashSet};

    // One tmux call to get all live panes.
    let all_panes = match crate::tmux::pane::list_panes_detailed() {
        Ok(p) => p,
        Err(e) => {
            log::warn!("gc_bg_windows: failed to list panes: {}", e);
            return;
        }
    };

    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Build lookup map and collect daemon-prefixed window names.
    let mut pane_map: HashMap<String, PaneGcInfo> = HashMap::new();
    let mut daemon_windows: HashMap<String, (String, String)> = HashMap::new(); // pane_id -> (session, window_name)

    for p in &all_panes {
        let idle_shell = IDLE_SHELLS.contains(&p.current_cmd.as_str());
        pane_map.insert(
            p.pane_id.clone(),
            PaneGcInfo {
                dead: p.dead,
                idle_shell,
                last_activity: p.last_activity,
            },
        );
        let is_daemon_window = DAEMON_BG_PREFIXES
            .iter()
            .any(|prefix| p.window_name.starts_with(prefix));
        if is_daemon_window {
            daemon_windows.insert(
                p.pane_id.clone(),
                (p.session_name.clone(), p.window_name.clone()),
            );
        }
    }

    // Locked phase: decide what to kill and which panes stay tracked. No
    // subprocess spawn and no file write happens while the guard is alive.
    let (kills, tracked_pane_ids): (Vec<GcKill>, HashSet<String>) =
        with_sessions(sessions, |store| {
            let mut kills: Vec<GcKill> = Vec::new();
            let mut tracked: HashSet<String> = HashSet::new();

            for (session_id, entry) in store.iter_mut() {
                let to_kill = plan_gc_actions(&entry.bg_windows, &pane_map, now_unix);
                if to_kill.is_empty() {
                    for w in &entry.bg_windows {
                        tracked.insert(w.pane_id.clone());
                    }
                    continue;
                }

                for pane_id in &to_kill {
                    // Look up window info before removing.
                    if let Some(win) = entry.bg_windows.iter().find(|w| &w.pane_id == pane_id) {
                        let reason = if pane_map.contains_key(pane_id) {
                            if pane_map[pane_id].dead {
                                "pane_dead"
                            } else {
                                "idle_completed"
                            }
                        } else {
                            "pane_gone"
                        };
                        kills.push(GcKill {
                            session_id: session_id.clone(),
                            window_name: win.window_name.clone(),
                            tmux_session: win.tmux_session.clone(),
                            pane_id: pane_id.clone(),
                            reason,
                        });
                    }
                }

                entry.bg_windows.retain(|w| {
                    let keep = !to_kill.contains(&w.pane_id);
                    if keep {
                        tracked.insert(w.pane_id.clone());
                    }
                    keep
                });
            }

            (kills, tracked)
        });

    // Unlocked phase: everything blocking happens out here.
    for k in &kills {
        log_event(
            "gc_window",
            serde_json::json!({
                "session": k.session_id,
                "win_name": k.window_name,
                "pane_id": k.pane_id,
                "reason": k.reason,
            }),
        );
        if let Err(e) = tmux::kill_job_window(&k.tmux_session, &k.window_name) {
            log::warn!("gc_bg_windows: failed to kill {}: {}", k.window_name, e);
        }
    }

    // Orphan sweep: kill daemon-prefixed windows not tracked by any session.
    for (pane_id, (tmux_session, window_name)) in &daemon_windows {
        if !tracked_pane_ids.contains(pane_id) {
            log_event(
                "gc_window",
                serde_json::json!({
                    "session": "-",
                    "win_name": window_name,
                    "pane_id": pane_id,
                    "reason": "orphan",
                }),
            );
            if let Err(e) = tmux::kill_job_window(tmux_session, window_name) {
                log::warn!(
                    "gc_bg_windows: failed to kill orphan {}: {}",
                    window_name,
                    e
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── plan_gc_actions ───────────────────────────────────────────────────────

    fn make_win(pane_id: &str, exit_code: Option<i32>) -> crate::daemon::session::BgWindowInfo {
        crate::daemon::session::BgWindowInfo {
            pane_id: pane_id.to_string(),
            window_name: format!("de-bg-{}-0-cmd", pane_id.trim_start_matches('%')),
            tmux_session: "test".to_string(),
            exit_code,
        }
    }

    fn alive(idle: bool, last_activity: u64) -> PaneGcInfo {
        PaneGcInfo {
            dead: false,
            idle_shell: idle,
            last_activity,
        }
    }

    fn dead(last_activity: u64) -> PaneGcInfo {
        PaneGcInfo {
            dead: true,
            idle_shell: true,
            last_activity,
        }
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    #[test]
    fn gc_pane_gone_kills() {
        let wins = vec![make_win("%1", None)];
        let panes = std::collections::HashMap::new(); // pane not in map
        let result = plan_gc_actions(&wins, &panes, now());
        assert_eq!(result, vec!["%1".to_string()]);
    }

    #[test]
    fn gc_running_pane_no_kill() {
        let wins = vec![make_win("%2", None)];
        let mut panes = std::collections::HashMap::new();
        panes.insert("%2".to_string(), alive(false, now()));
        let result = plan_gc_actions(&wins, &panes, now());
        assert!(result.is_empty());
    }

    #[test]
    fn gc_dead_pane_fresh_no_kill() {
        let wins = vec![make_win("%3", Some(0))];
        let mut panes = std::collections::HashMap::new();
        // died just now — under threshold
        panes.insert("%3".to_string(), dead(now()));
        let result = plan_gc_actions(&wins, &panes, now());
        assert!(result.is_empty());
    }

    #[test]
    fn gc_dead_pane_stale_kills() {
        let wins = vec![make_win("%4", Some(0))];
        let mut panes = std::collections::HashMap::new();
        // died 60 seconds ago — over DEAD_THRESHOLD_SECS (30)
        panes.insert("%4".to_string(), dead(now().saturating_sub(60)));
        let result = plan_gc_actions(&wins, &panes, now());
        assert_eq!(result, vec!["%4".to_string()]);
    }

    #[test]
    fn gc_completed_idle_fresh_no_kill() {
        let wins = vec![make_win("%5", Some(0))];
        let mut panes = std::collections::HashMap::new();
        // completed but became idle just now — under IDLE_THRESHOLD_SECS (120)
        panes.insert("%5".to_string(), alive(true, now()));
        let result = plan_gc_actions(&wins, &panes, now());
        assert!(result.is_empty());
    }

    #[test]
    fn gc_completed_idle_stale_kills() {
        let wins = vec![make_win("%6", Some(0))];
        let mut panes = std::collections::HashMap::new();
        // idle for 180 seconds — over IDLE_THRESHOLD_SECS (120)
        panes.insert("%6".to_string(), alive(true, now().saturating_sub(180)));
        let result = plan_gc_actions(&wins, &panes, now());
        assert_eq!(result, vec!["%6".to_string()]);
    }

    #[test]
    fn gc_completed_not_idle_no_kill() {
        // exit_code is set but pane is still running another command
        let wins = vec![make_win("%7", Some(0))];
        let mut panes = std::collections::HashMap::new();
        panes.insert("%7".to_string(), alive(false, now().saturating_sub(180)));
        let result = plan_gc_actions(&wins, &panes, now());
        assert!(result.is_empty());
    }

    #[test]
    fn gc_no_exit_code_idle_no_kill() {
        // Still "running" (no exit_code recorded yet) — must not GC even if idle
        let wins = vec![make_win("%8", None)];
        let mut panes = std::collections::HashMap::new();
        panes.insert("%8".to_string(), alive(true, now().saturating_sub(180)));
        let result = plan_gc_actions(&wins, &panes, now());
        assert!(result.is_empty());
    }
}
