use super::ToolCallOutcome;
use super::USER_PROMPT_TIMEOUT;
use crate::ai::filter::mask_sensitive;
use crate::daemon::session::{
    FG_HOOK_COUNTER, SessionStore, append_session_message, bg_done_subscribe,
};
use crate::daemon::utils::send_response_split;
use crate::daemon::utils::{log_event, normalize_output};
use crate::ipc::{Request, Response, RunbookListItem, ScriptListItem};
use crate::scheduler::ScheduleStore;
use crate::scripts;
use crate::util::UnpoisonExt;
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Artifact context — passed to write operations for origin-stamping + tracking
// ---------------------------------------------------------------------------

pub(super) struct ArtifactCtx<'a> {
    pub session_id: Option<&'a str>,
    pub sessions: &'a SessionStore,
    pub saved_name: Option<&'a str>,
    pub turn_count: usize,
    pub is_ghost: bool,
}

fn track_artifact(ctx: &ArtifactCtx<'_>, kind: &str, name: &str) {
    if ctx.is_ghost {
        return;
    }
    let Some(sid) = ctx.session_id else { return };
    if let Ok(mut store) = ctx.sessions.lock() {
        if let Some(entry) = store.get_mut(sid) {
            entry
                .artifacts_created
                .push(crate::session_store::ArtifactRef {
                    kind: kind.to_string(),
                    name: name.to_string(),
                    at_turn: ctx.turn_count,
                });
        }
    }
}

// ---------------------------------------------------------------------------
// Scripts
// ---------------------------------------------------------------------------

pub(super) async fn write_script<W, R>(
    id: &str,
    script_name: &str,
    content: &str,
    artifact_ctx: &ArtifactCtx<'_>,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    if artifact_ctx.is_ghost {
        return Ok(ToolCallOutcome::Result(
            "Error: cannot write scripts in a Ghost Shell (requires user approval).".to_string(),
        ));
    }
    let existing_content = scripts::read_script(script_name).ok();
    send_response_split(
        tx,
        Response::ScriptWritePrompt {
            id: id.to_string(),
            script_name: script_name.to_string(),
            content: content.to_string(),
            existing_content,
        },
    )
    .await?;

    let mut line = String::new();
    let read_result = tokio::time::timeout(USER_PROMPT_TIMEOUT, rx.read_line(&mut line)).await;
    if matches!(read_result, Ok(Ok(0))) {
        return Err(anyhow::anyhow!("EOF"));
    }
    let approved = match read_result {
        Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
            Ok(Request::ScriptWriteResponse { approved, .. }) => approved,
            _ => false,
        },
        _ => false,
    };

    if approved {
        crate::daemon::stats::inc_scripts_approved();
        let stamped = match artifact_ctx.saved_name {
            Some(origin) => crate::header::inject_comment_session_origin(content, origin),
            None => content.to_string(),
        };
        match scripts::write_script(script_name, &stamped) {
            Ok(()) => {
                track_artifact(artifact_ctx, "script", script_name);
                Ok(ToolCallOutcome::Result(format!(
                    "Script '{}' written successfully",
                    script_name
                )))
            }
            Err(e) => Ok(ToolCallOutcome::Result(format!(
                "Failed to write script: {}",
                e
            ))),
        }
    } else {
        crate::daemon::stats::inc_scripts_denied();
        Ok(ToolCallOutcome::Result(
            "Script write denied by user".to_string(),
        ))
    }
}

pub(super) async fn list_scripts<W>(tx: &mut W) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let script_list = scripts::list_scripts().unwrap_or_default();
    let items: Vec<ScriptListItem> = script_list
        .iter()
        .map(|s| ScriptListItem {
            name: s.name.clone(),
            size: s.size,
        })
        .collect();
    let count = items.len();
    let _ = send_response_split(tx, Response::ScriptList { scripts: items }).await;
    Ok(ToolCallOutcome::Result(format!(
        "{} script(s) in ~/.daemoneye/scripts/",
        count
    )))
}

pub(super) fn read_script(script_name: &str) -> String {
    match scripts::read_script(script_name) {
        Ok(content) => content,
        Err(e) => format!("Error reading script '{}': {}", script_name, e),
    }
}

pub(super) async fn delete_script<W, R>(
    id: &str,
    script_name: &str,
    is_ghost: bool,
    session_id: Option<&str>,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    if is_ghost {
        return Ok(ToolCallOutcome::Result(
            "Error: cannot delete scripts in a Ghost Shell (requires user approval).".to_string(),
        ));
    }
    send_response_split(
        tx,
        Response::ScriptDeletePrompt {
            id: id.to_string(),
            script_name: script_name.to_string(),
        },
    )
    .await?;

    let mut line = String::new();
    let read_result = tokio::time::timeout(USER_PROMPT_TIMEOUT, rx.read_line(&mut line)).await;
    if matches!(read_result, Ok(Ok(0))) {
        return Err(anyhow::anyhow!("EOF"));
    }
    let approved = match read_result {
        Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
            Ok(Request::ScriptDeleteResponse { approved, .. }) => approved,
            _ => false,
        },
        _ => false,
    };

    if approved {
        match scripts::delete_script(script_name) {
            Ok(()) => {
                log::info!("Script '{}' deleted", script_name);
                log_event(
                    "script_delete",
                    serde_json::json!({ "session": session_id.unwrap_or("-"), "script": script_name }),
                );
                Ok(ToolCallOutcome::Result(format!(
                    "Script '{}' deleted",
                    script_name
                )))
            }
            Err(e) => Ok(ToolCallOutcome::Result(format!(
                "Failed to delete script '{}': {}",
                script_name, e
            ))),
        }
    } else {
        Ok(ToolCallOutcome::Result(
            "Script deletion denied by user".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Runbooks
// ---------------------------------------------------------------------------

pub(super) async fn write_runbook<W, R>(
    id: &str,
    name: &str,
    content: &str,
    artifact_ctx: &ArtifactCtx<'_>,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    if artifact_ctx.is_ghost {
        return Ok(ToolCallOutcome::Result(
            "Error: cannot write runbooks in a Ghost Shell (requires user approval).".to_string(),
        ));
    }
    let existing_content = crate::runbook::load_runbook(name).ok().map(|rb| rb.content);
    send_response_split(
        tx,
        Response::RunbookWritePrompt {
            id: id.to_string(),
            runbook_name: name.to_string(),
            content: content.to_string(),
            existing_content,
        },
    )
    .await?;

    let mut line = String::new();
    let read_result = tokio::time::timeout(USER_PROMPT_TIMEOUT, rx.read_line(&mut line)).await;
    if matches!(read_result, Ok(Ok(0))) {
        return Err(anyhow::anyhow!("EOF"));
    }
    let approved = match read_result {
        Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
            Ok(Request::RunbookWriteResponse { approved, .. }) => approved,
            _ => false,
        },
        _ => false,
    };

    if approved {
        crate::daemon::stats::inc_runbooks_approved();
        let stamped = match artifact_ctx.saved_name {
            Some(origin) => crate::header::inject_yaml_session_origin(content, origin),
            None => content.to_string(),
        };
        match crate::runbook::write_runbook(name, &stamped) {
            Ok(()) => {
                log::info!("Runbook '{}' written", name);
                log_event(
                    "runbook_write",
                    serde_json::json!({ "session": artifact_ctx.session_id.unwrap_or("-"), "runbook": name }),
                );
                track_artifact(artifact_ctx, "runbook", name);
                Ok(ToolCallOutcome::Result(format!(
                    "Runbook '{}' written to ~/.daemoneye/runbooks/{}.md",
                    name, name
                )))
            }
            Err(e) => Ok(ToolCallOutcome::Result(format!(
                "Failed to write runbook: {}",
                e
            ))),
        }
    } else {
        crate::daemon::stats::inc_runbooks_denied();
        Ok(ToolCallOutcome::Result(
            "Runbook write denied by user".to_string(),
        ))
    }
}

pub(super) async fn delete_runbook<W, R>(
    id: &str,
    name: &str,
    is_ghost: bool,
    session_id: Option<&str>,
    schedule_store: &Arc<ScheduleStore>,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let active_jobs: Vec<String> = schedule_store
        .list()
        .into_iter()
        .filter(|j| j.runbook.as_deref() == Some(name))
        .map(|j| j.name)
        .collect();

    if is_ghost {
        return Ok(ToolCallOutcome::Result(
            "Error: cannot delete runbooks in a Ghost Shell (requires user approval).".to_string(),
        ));
    }
    send_response_split(
        tx,
        Response::RunbookDeletePrompt {
            id: id.to_string(),
            runbook_name: name.to_string(),
            active_jobs,
        },
    )
    .await?;

    let mut line = String::new();
    let read_result = tokio::time::timeout(USER_PROMPT_TIMEOUT, rx.read_line(&mut line)).await;
    if matches!(read_result, Ok(Ok(0))) {
        return Err(anyhow::anyhow!("EOF"));
    }
    let approved = match read_result {
        Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
            Ok(Request::RunbookDeleteResponse { approved, .. }) => approved,
            _ => false,
        },
        _ => false,
    };

    if approved {
        match crate::runbook::delete_runbook(name) {
            Ok(()) => {
                log::info!("Runbook '{}' deleted", name);
                log_event(
                    "runbook_delete",
                    serde_json::json!({ "session": session_id.unwrap_or("-"), "runbook": name }),
                );
                Ok(ToolCallOutcome::Result(format!(
                    "Runbook '{}' deleted",
                    name
                )))
            }
            Err(e) => Ok(ToolCallOutcome::Result(format!(
                "Failed to delete runbook: {}",
                e
            ))),
        }
    } else {
        Ok(ToolCallOutcome::Result(
            "Runbook delete denied by user".to_string(),
        ))
    }
}

pub(super) fn read_runbook(name: &str) -> String {
    match crate::runbook::load_runbook(name) {
        Ok(rb) => rb.content,
        Err(e) => format!("Error reading runbook '{}': {}", name, e),
    }
}

pub(super) async fn list_runbooks<W>(tx: &mut W) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let items = crate::runbook::list_runbooks().unwrap_or_default();
    let count = items.len();
    let runbook_items: Vec<RunbookListItem> = items
        .iter()
        .map(|r| RunbookListItem {
            name: r.name.clone(),
            tags: r.tags.clone(),
            ghost_config: r.ghost_config.clone(),
        })
        .collect();
    let _ = send_response_split(
        tx,
        Response::RunbookList {
            runbooks: runbook_items,
        },
    )
    .await;
    Ok(ToolCallOutcome::Result(format!(
        "{} runbook(s) in ~/.daemoneye/runbooks/",
        count
    )))
}

// ---------------------------------------------------------------------------
// Memory
// ---------------------------------------------------------------------------

pub(super) fn add_memory(
    key: &str,
    value: &str,
    category: &str,
    artifact_ctx: &ArtifactCtx<'_>,
) -> String {
    let Some(cat) = crate::memory::MemoryCategory::from_str(category) else {
        return format!(
            "Error: invalid category '{}'. Must be 'session', 'knowledge', or 'incident'.",
            category
        );
    };
    if value.trim().is_empty() {
        return "Error: memory value cannot be empty.".to_string();
    }
    let stamped = match artifact_ctx.saved_name {
        Some(origin) => crate::header::inject_yaml_session_origin(value, origin),
        None => value.to_string(),
    };
    match crate::memory::add_memory(key, &stamped, cat) {
        Ok(()) => {
            log_event(
                "memory_write",
                serde_json::json!({ "session": artifact_ctx.session_id, "op": "add", "category": category, "key": key }),
            );
            track_artifact(artifact_ctx, "memory", key);
            format!("Memory '{}' stored in {}", key, category)
        }
        Err(e) => format!("Error storing memory: {}", e),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn update_memory(
    key: &str,
    category: &str,
    body: Option<&str>,
    append: bool,
    tags: Option<&[String]>,
    summary: Option<&str>,
    relates_to: Option<&[String]>,
    expires: Option<&str>,
    session_id: Option<&str>,
) -> String {
    let Some(cat) = crate::memory::MemoryCategory::from_str(category) else {
        return format!(
            "Error: invalid category '{}'. Must be 'session', 'knowledge', or 'incident'.",
            category
        );
    };
    match crate::memory::update_memory(key, cat, body, append, tags, summary, relates_to, expires) {
        Ok(()) => {
            log_event(
                "memory_write",
                serde_json::json!({ "session": session_id, "op": "update", "category": category, "key": key }),
            );
            let mut updated_fields: Vec<&str> = Vec::new();
            if body.is_some() {
                updated_fields.push(if append { "body (appended)" } else { "body" });
            }
            if tags.is_some() {
                updated_fields.push("tags");
            }
            if summary.is_some() {
                updated_fields.push("summary");
            }
            if relates_to.is_some() {
                updated_fields.push("relates_to");
            }
            if expires.is_some() {
                updated_fields.push("expires");
            }
            if updated_fields.is_empty() {
                format!(
                    "Memory '{}' [{}] updated (timestamp refreshed).",
                    key, category
                )
            } else {
                format!(
                    "Memory '{}' [{}] updated: {}.",
                    key,
                    category,
                    updated_fields.join(", ")
                )
            }
        }
        Err(e) => format!("Error updating memory '{}': {}", key, e),
    }
}

pub(super) fn delete_memory(key: &str, category: &str, session_id: Option<&str>) -> String {
    let Some(cat) = crate::memory::MemoryCategory::from_str(category) else {
        return format!(
            "Error: invalid category '{}'. Must be 'session', 'knowledge', or 'incident'.",
            category
        );
    };
    match crate::memory::delete_memory(key, cat) {
        Ok(()) => {
            log_event(
                "memory_write",
                serde_json::json!({ "session": session_id, "op": "delete", "category": category, "key": key }),
            );
            format!("Memory '{}' deleted from {}", key, category)
        }
        Err(e) => format!("Error deleting memory: {}", e),
    }
}

pub(super) fn read_memory(key: &str, category: &str) -> String {
    let Some(cat) = crate::memory::MemoryCategory::from_str(category) else {
        return format!(
            "Error: invalid category '{}'. Must be 'session', 'knowledge', or 'incident'.",
            category
        );
    };
    match crate::memory::read_memory(key, cat) {
        Ok(content) => mask_sensitive(&content),
        Err(e) => format!("Error reading memory '{}': {}", key, e),
    }
}

pub(super) async fn list_memories<W>(
    category: Option<&str>,
    _tx: &mut W,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let cat = match category {
        None => None,
        Some(s) => match crate::memory::MemoryCategory::from_str(s) {
            Some(c) => Some(c),
            None => {
                return Ok(ToolCallOutcome::Result(format!(
                    "Error: invalid category '{}'. Must be 'session', 'knowledge', or 'incident'.",
                    s
                )));
            }
        },
    };
    let infos = crate::memory::list_memories_with_tags(cat).unwrap_or_default();
    let count = infos.len();
    if count == 0 {
        Ok(ToolCallOutcome::Result(
            "No memory entries found.".to_string(),
        ))
    } else {
        let lines: Vec<String> = infos
            .iter()
            .map(|info| {
                // Build: [category] key — summary (updated YYYY-MM-DD)
                let mut line = match &info.summary {
                    Some(s) => format!("[{}] {} — {}", info.category, info.key, s),
                    None => format!("[{}] {}", info.category, info.key),
                };
                // Append the date portion of updated (or created as fallback) when present.
                let ts_opt = info.updated.as_ref().or(info.created.as_ref());
                let label = if info.updated.is_some() {
                    "updated"
                } else {
                    "created"
                };
                if let Some(ts) = ts_opt {
                    let date = ts.split('T').next().unwrap_or(ts.as_str());
                    if !date.is_empty() {
                        line.push_str(&format!(" ({} {})", label, date));
                    }
                }
                line
            })
            .collect();
        Ok(ToolCallOutcome::Result(format!(
            "{} memory entries:\n{}",
            count,
            lines.join("\n")
        )))
    }
}

// ---------------------------------------------------------------------------
// Search / context
// ---------------------------------------------------------------------------

pub(super) fn search_repository(query: &str, kind: &str) -> String {
    let results = crate::search::search_repository(query, kind, 2);
    crate::search::format_results(&results)
}

// ---------------------------------------------------------------------------
// Background window management
// ---------------------------------------------------------------------------

pub(super) fn close_bg_window(
    pane_id: &str,
    session_id: Option<&str>,
    sessions: &SessionStore,
) -> String {
    let Some(sid) = session_id else {
        return "No active session — cannot close background window.".to_string();
    };
    let (win_name, tmux_session, still_running) = {
        let store = sessions.lock().unwrap_or_log();
        let Some(entry) = store.get(sid) else {
            return format!("Session '{}' not found.", sid);
        };
        let Some(win) = entry.bg_windows.iter().find(|w| w.pane_id == pane_id) else {
            return format!(
                "No background window with pane ID {} found in this session.",
                pane_id
            );
        };
        (
            win.window_name.clone(),
            win.tmux_session.clone(),
            win.exit_code.is_none(),
        )
    };

    if still_running {
        log::warn!(
            "Agent closing still-running bg window {} (pane {})",
            win_name,
            pane_id
        );
    }

    if let Err(e) = crate::tmux::kill_job_window(&tmux_session, &win_name) {
        log::warn!(
            "close_background_window: failed to kill {}: {}",
            win_name,
            e
        );
    }

    if let Ok(mut store) = sessions.lock()
        && let Some(entry) = store.get_mut(sid)
    {
        entry.bg_windows.retain(|w| w.pane_id != pane_id);
    }

    log_event(
        "close_bg_window",
        serde_json::json!({
            "session": sid, "pane_id": pane_id,
            "win_name": win_name, "was_running": still_running,
        }),
    );

    format!("Background window {} (pane {}) closed.", win_name, pane_id)
}

// ---------------------------------------------------------------------------
// List panes
// ---------------------------------------------------------------------------

pub(super) fn list_panes(
    cache: &crate::tmux::cache::SessionCache,
    chat_pane: Option<&str>,
) -> String {
    let panes = cache.panes.read().unwrap_or_log();
    let session = cache.session_name.read().unwrap_or_log().clone();

    let mut rows: Vec<_> = panes
        .iter()
        .filter(|(id, _)| chat_pane != Some(id.as_str()))
        .collect();
    rows.sort_by_key(|(id, _)| id.as_str());

    if rows.is_empty() {
        return format!("No targetable panes found in session '{}'.", session);
    }

    let mut out = format!(
        "{} pane{} in session '{}' (chat pane excluded):\n",
        rows.len(),
        if rows.len() == 1 { "" } else { "s" },
        session
    );
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    for (id, state) in &rows {
        let title_part = if !state.pane_title.is_empty() && state.pane_title != state.current_cmd {
            format!("  title:{}", mask_sensitive(&state.pane_title))
        } else {
            String::new()
        };
        let start_part = if !state.start_cmd.is_empty() && state.start_cmd != state.current_cmd {
            format!("  started:{}", state.start_cmd)
        } else {
            String::new()
        };
        let ghost_part = if state
            .window_name
            .starts_with(crate::daemon::INCIDENT_WINDOW_PREFIX)
            || state
                .window_name
                .starts_with(crate::daemon::GS_BG_WINDOW_PREFIX)
            || state
                .window_name
                .starts_with(crate::daemon::GS_SCHED_WINDOW_PREFIX)
        {
            "  [ghost]"
        } else {
            ""
        };
        let sync_part = if state.synchronized {
            "  [synchronized]"
        } else {
            ""
        };
        let dead_part = if state.dead {
            format!("  [dead: {}]", state.dead_status.unwrap_or(0))
        } else {
            String::new()
        };
        let activity_part = if state.last_activity > 0 && now_secs >= state.last_activity {
            let age = now_secs - state.last_activity;
            if age < 30 {
                format!("  [active {}s ago]", age)
            } else if age < 3600 {
                format!("  [idle {}m]", age / 60)
            } else {
                format!("  [idle {}h{}m]", age / 3600, (age % 3600) / 60)
            }
        } else {
            String::new()
        };
        out.push_str(&format!(
            "  {}  idx:{:<3}  window:{:<12}  cmd:{:<8}  cwd:{}{}{}{}{}{}{}\n",
            id,
            state.pane_index,
            state.window_name,
            state.current_cmd,
            state.current_path,
            start_part,
            title_part,
            ghost_part,
            sync_part,
            dead_part,
            activity_part,
        ));
    }
    out.push_str(
        "\nUse the pane ID as target_pane in run_terminal_command to execute a command there.",
    );
    out
}

// ---------------------------------------------------------------------------
// Watch pane
// ---------------------------------------------------------------------------

pub(super) fn watch_pane(
    pane_id: &str,
    timeout_secs: u64,
    pattern: Option<&str>,
    session_id: Option<&str>,
    session_name: &str,
    sessions: &SessionStore,
) -> String {
    let initial_cmd = crate::tmux::pane_current_command(pane_id).unwrap_or_default();

    let hook_idx = FG_HOOK_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let hook_name = format!("pane-title-changed[@de_wp_{}]", hook_idx);
    let current_exe =
        std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("daemoneye"));
    let notify_cmd = format!(
        "run-shell -b '{} notify activity {} 0 \"{}\"'",
        current_exe.display(),
        pane_id,
        crate::daemon::utils::shell_escape_arg(session_name)
    );
    let _ = std::process::Command::new("tmux")
        .args(["set-hook", "-t", pane_id, &hook_name, &notify_cmd])
        .output();

    let mut wp_rx = bg_done_subscribe();

    let pane_id_owned = pane_id.to_string();
    let session_id_owned = session_id.unwrap_or("-").to_string();
    let sessions_clone = Arc::clone(sessions);
    let timeout = Duration::from_secs(timeout_secs);
    let pattern_owned = pattern.map(|s| s.to_string());

    log::info!(
        "watch_pane: monitoring {} (initial_cmd={:?}) for session {}",
        pane_id,
        initial_cmd,
        session_id_owned
    );
    log_event(
        "watch_pane",
        serde_json::json!({
            "session": session_id_owned, "pane_id": pane_id,
            "pattern": pattern, "status": "active"
        }),
    );

    tokio::spawn(async move {
        let slow_poll = Duration::from_millis(500);
        let start_wait = Duration::from_secs(5);

        let pattern_re = pattern_owned
            .as_deref()
            .and_then(|p| regex::RegexBuilder::new(p).size_limit(1 << 20).build().ok());

        let completed = tokio::time::timeout(timeout, async {
            if let Some(ref re) = pattern_re {
                loop {
                    tokio::select! {
                        result = wp_rx.recv() => {
                            if let Ok(notified_pane) = result
                                && notified_pane == pane_id_owned {
                                    let snap = crate::tmux::capture_pane(&pane_id_owned, 200).unwrap_or_default();
                                    if re.is_match(&snap) { break; }
                                }
                        }
                        _ = tokio::time::sleep(slow_poll) => {
                            let snap = crate::tmux::capture_pane(&pane_id_owned, 200).unwrap_or_default();
                            if re.is_match(&snap) { break; }
                        }
                    }
                }
            } else {
                if super::foreground::is_shell_prompt(&initial_cmd) {
                    let _ = tokio::time::timeout(start_wait, async {
                        loop {
                            tokio::time::sleep(slow_poll).await;
                            let cur = crate::tmux::pane_current_command(&pane_id_owned).unwrap_or_default();
                            if !super::foreground::is_shell_prompt(&cur) { break; }
                        }
                    }).await;
                }

                loop {
                    tokio::select! {
                        result = wp_rx.recv() => {
                            if let Ok(notified_pane) = result
                                && notified_pane == pane_id_owned {
                                    let cur = crate::tmux::pane_current_command(&pane_id_owned).unwrap_or_default();
                                    if super::foreground::is_shell_prompt(&cur) { break; }
                                }
                        }
                        _ = tokio::time::sleep(slow_poll) => {
                            let cur = crate::tmux::pane_current_command(&pane_id_owned).unwrap_or_default();
                            if super::foreground::is_shell_prompt(&cur) { break; }
                        }
                    }
                }
            }
        }).await.is_ok();

        let _ = std::process::Command::new("tmux")
            .args(["set-hook", "-u", "-t", &pane_id_owned, &hook_name])
            .output();

        let raw = crate::tmux::capture_pane(&pane_id_owned, 200).unwrap_or_default();
        let mut body = mask_sensitive(&normalize_output(&raw));
        let hints = crate::manifest::related_knowledge_hints(&body);
        if !hints.is_empty() {
            body.push('\n');
            body.push_str(&hints);
        }

        let content = if completed {
            if let Some(ref pat) = pattern_owned {
                format!(
                    "[Watch Pane Match] Pattern `{}` matched in pane {}.\n<output>\n{}\n</output>",
                    pat, pane_id_owned, body
                )
            } else {
                format!(
                    "[Watch Pane Complete] Command finished in pane {}.\n<output>\n{}\n</output>",
                    pane_id_owned, body
                )
            }
        } else {
            format!(
                "[Watch Pane Timeout] Timed out waiting in pane {}.\n<output>\n{}\n</output>",
                pane_id_owned, body
            )
        };

        let watch_msg = crate::ai::Message {
            role: "user".to_string(),
            content,
            tool_calls: None,
            tool_results: None,
            turn: None,
        };

        if let Ok(mut store) = sessions_clone.lock()
            && let Some(entry) = store.get_mut(&session_id_owned)
        {
            append_session_message(&session_id_owned, &watch_msg);
            entry.messages.push(watch_msg);

            let alert = if completed {
                format!("Watched pane {} command completed", pane_id_owned)
            } else {
                format!("Watched pane {} timed out", pane_id_owned)
            };
            if let Some(ref cp) = entry.chat_pane {
                let _ = std::process::Command::new("tmux")
                    .args(["display-message", "-d", "5000", "-t", cp, &alert])
                    .output();
            }
        }
        log::info!(
            "watch_pane {}: {}",
            pane_id_owned,
            if completed { "completed" } else { "timed out" }
        );
    });

    if let Some(pat) = pattern {
        format!(
            "Now watching pane {} for pattern `{}`. \
             You will receive [Watch Pane Match] when the pattern appears, \
             or [Watch Pane Timeout] after {} seconds.",
            pane_id, pat, timeout_secs
        )
    } else {
        format!(
            "Now watching pane {} for command completion. \
             You will receive [Watch Pane Complete] when the command finishes, \
             or [Watch Pane Timeout] after {} seconds.",
            pane_id, timeout_secs
        )
    }
}

// ---------------------------------------------------------------------------
// Spawn ghost shell
// ---------------------------------------------------------------------------

pub(super) async fn spawn_ghost(
    runbook: &str,
    message: &str,
    sessions: &SessionStore,
) -> anyhow::Result<ToolCallOutcome> {
    use crate::daemon::ghost::{GhostManager, check_ghost_capacity};
    use crate::webhook::inject_ghost_event;

    let spawn_config = crate::config::Config::load().unwrap_or_default();
    if !check_ghost_capacity(&spawn_config) {
        return Ok(ToolCallOutcome::Result(format!(
            "Cannot spawn ghost shell: concurrency limit ({}) reached. \
             Wait for an active ghost to complete before spawning another.",
            spawn_config.ghost.max_concurrent_ghosts
        )));
    }

    let rb = match crate::runbook::load_runbook(runbook) {
        Ok(rb) => rb,
        Err(e) => {
            return Ok(ToolCallOutcome::Result(format!(
                "Failed to load runbook '{}': {}",
                runbook, e
            )));
        }
    };

    let rb_name = rb.name.clone();
    match GhostManager::start_session(
        sessions.clone(),
        &rb,
        message,
        crate::daemon::GS_BG_WINDOW_PREFIX,
        spawn_config.approvals.ghost_commands,
    )
    .await
    {
        Err(e) => Ok(ToolCallOutcome::Result(format!(
            "Failed to start ghost shell: {}",
            e
        ))),
        Ok(sid) => {
            inject_ghost_event(
                sessions,
                &format!(
                    "[Ghost Shell Started] AI-requested ghost shell started for runbook: {}",
                    rb_name
                ),
            );
            let tool_result = format!(
                "Ghost shell started (session: {}). It will run autonomously in the background \
                 and inject [Ghost Shell Completed] or [Ghost Shell Failed] events when done.",
                sid
            );
            Ok(ToolCallOutcome::SpawnGhostSession {
                session_id: sid,
                runbook_name: rb_name,
                tool_result,
            })
        }
    }
}
