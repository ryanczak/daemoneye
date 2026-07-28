use crate::ai::{AiEvent, Message};
use crate::config::Config;
use crate::daemon::background::{OwnedJobInfo, notify_job_completion};
use crate::daemon::ghost::{GhostManager, check_ghost_capacity, trigger_ghost_turn};
use crate::daemon::session::*;
use crate::daemon::utils::*;
use crate::ipc::Response;
use crate::runbook;
use crate::scheduler::{ActionOn, ScheduleStore, ScheduledJob};
use crate::scripts;
use crate::tmux;
use crate::tmux::cache::SessionCache;
use crate::webhook::inject_ghost_event;
use std::sync::Arc;
use std::time::Duration;

/// Run a single scheduled job in a dedicated tmux window.
///
/// - `ActionOn::Ghost`: spawns a full Ghost Shell session using the named runbook.
/// - `ActionOn::Alert`: emits a SystemMsg notification only.
/// - `ActionOn::Script`: runs the script in a `de-sj-*` window; captures output;
///   optionally runs watchdog analysis against a runbook.
/// - `ActionOn::Command` (deprecated): same as Script but with a raw command string.
///
/// On success the window is killed and the job marked `Succeeded` (or rescheduled
/// for `Every` jobs).  On failure the window is left open for inspection.
pub async fn run_scheduled_job(
    job: ScheduledJob,
    store: Arc<ScheduleStore>,
    session: String,
    sessions: SessionStore,
    config: Config,
    cache: Arc<SessionCache>,
    notify_tx: Option<tokio::sync::mpsc::UnboundedSender<Response>>,
) {
    crate::daemon::stats::inc_schedules_executed();

    // Ghost-mode: hand off entirely to the ghost shell infrastructure.
    if let ActionOn::Ghost { runbook: rb_name } = &job.action {
        if !check_ghost_capacity(&config) {
            log::warn!(
                "Scheduled ghost job '{}': skipped — concurrency limit ({}) reached",
                job.name,
                config.ghost.max_concurrent_ghosts
            );
            inject_ghost_event(
                &sessions,
                &format!(
                    "[Ghost Shell Skipped] Scheduled job '{}' skipped — concurrency limit reached",
                    job.name
                ),
            )
            .await;
            store.mark_done(
                &job.id,
                false,
                Some("ghost concurrency limit reached".to_string()),
            );
            return;
        }

        let alert_msg = format!(
            "Scheduled job '{}' fired ({})",
            job.name,
            job.kind.describe()
        );
        match runbook::load_runbook(rb_name) {
            Err(e) => {
                let msg = format!(
                    "Scheduled ghost job '{}': failed to load runbook '{}': {}",
                    job.name, rb_name, e
                );
                log::error!("{}", msg);
                store.mark_done(&job.id, false, Some(msg));
            }
            Ok(rb) => {
                let merged_config = crate::agents::merge_runbook_ghost_config(&rb);
                match GhostManager::start_session_with_config(
                    sessions.clone(),
                    &rb,
                    &merged_config,
                    &alert_msg,
                    crate::daemon::GS_SCHED_WINDOW_PREFIX,
                    config.approvals.ghost_commands,
                )
                .await
                {
                    Err(e) => {
                        let msg = format!(
                            "Scheduled ghost job '{}': failed to start session: {}",
                            job.name, e
                        );
                        log::error!("{}", msg);
                        inject_ghost_event(
                            &sessions,
                            &format!(
                                "[Ghost Shell Failed] Scheduled job '{}' could not start: {}",
                                job.name, e
                            ),
                        )
                        .await;
                        store.mark_done(&job.id, false, Some(msg));
                    }
                    Ok(sid) => {
                        let session_log = crate::daemon::session::session_file(&sid)
                            .display()
                            .to_string();
                        inject_ghost_event(
                            &sessions,
                            &format!(
                                "[Ghost Shell Started] Scheduled job '{}' started autonomous session — session log: {}",
                                job.name, session_log
                            ),
                        )
                        .await;
                        let result = trigger_ghost_turn(
                            &sid,
                            &sessions,
                            &config,
                            &cache,
                            &Arc::clone(&store),
                        )
                        .await;
                        match result {
                            Ok(()) => {
                                inject_ghost_event(
                                    &sessions,
                                    &format!(
                                        "[Ghost Shell Completed] Scheduled job '{}' finished — session log: {}",
                                        job.name, session_log
                                    ),
                                )
                                .await;
                                store.mark_done(&job.id, true, None);
                            }
                            Err(e) => {
                                log::error!("Scheduled ghost job '{}' failed: {}", job.name, e);
                                inject_ghost_event(
                                    &sessions,
                                    &format!(
                                        "[Ghost Shell Failed] Scheduled job '{}' error: {} — session log: {}",
                                        job.name, e, session_log
                                    ),
                                )
                                .await;
                                store.mark_done(
                                    &job.id,
                                    false,
                                    Some(format!("ghost error: {}", e)),
                                );
                            }
                        }
                    }
                }
            }
        }
        return;
    }

    if matches!(job.action, ActionOn::Script(_)) {
        crate::daemon::stats::inc_scripts_executed();
    }

    let unix_ts = chrono::Utc::now().timestamp();
    let temp_win_name = format!("{}tmp-{}", crate::daemon::SCHED_WINDOW_PREFIX, unix_ts);
    let cmd = match &job.action {
        ActionOn::Alert => {
            // Pure alert: no command to run.
            store.mark_done(&job.id, true, None);
            let msg = format!("Watchdog alert: {}", job.name);
            if let Some(ref tx) = notify_tx
                && let Err(e) = tx.send(Response::SystemMsg(msg.clone()))
            {
                log::debug!(
                    "scheduled job '{}': dropped notification (no receiver): {}",
                    job.name,
                    e
                );
            }
            fire_notification(&job.name, &msg, &config);
            return;
        }
        ActionOn::Command(c) => {
            if c.is_empty() {
                let msg = format!(
                    "Scheduled job '{}' has an empty command and no ghost runbook; marking failed",
                    job.name
                );
                log::error!("{}", msg);
                store.mark_done(&job.id, false, Some(msg));
                return;
            }
            log::warn!(
                "Scheduled job '{}' uses deprecated ActionOn::Command; migrate to ActionOn::Script",
                job.name
            );
            c.clone()
        }
        ActionOn::Script(s) => match scripts::resolve_script(s) {
            Ok(path) => path.to_string_lossy().to_string(),
            Err(e) => {
                let msg = format!("Scheduled job '{}' failed: {}", job.name, e);
                store.mark_done(&job.id, false, Some(msg.clone()));
                if let Some(ref tx) = notify_tx
                    && let Err(e) = tx.send(Response::SystemMsg(msg))
                {
                    log::debug!(
                        "scheduled job '{}': dropped notification (no receiver): {}",
                        job.name,
                        e
                    );
                }
                return;
            }
        },
        ActionOn::Ghost { .. } => unreachable!("handled above"),
    };

    let wrapped = format!("{}; exit $?", cmd);

    let s = session.to_string();
    let t = temp_win_name.to_string();
    let created = tmux::off_runtime("create-job-window", move || tmux::create_job_window(&s, &t))
        .await
        .unwrap_or_else(|| Err(anyhow::anyhow!("timed out creating window")));

    let pane_id = match created {
        Ok(p) => p,
        Err(e) => {
            let msg = format!(
                "Scheduled job '{}': failed to create window: {}",
                job.name, e
            );
            store.mark_done(&job.id, false, Some(e.to_string()));
            if let Some(ref tx) = notify_tx
                && let Err(e) = tx.send(Response::SystemMsg(msg))
            {
                log::debug!(
                    "scheduled job '{}': dropped notification (no receiver): {}",
                    job.name,
                    e
                );
            }
            return;
        }
    };

    // Build final name: prefix + pane-number + unix-ts + command-slug.
    let pane_num = pane_id.trim_start_matches('%');
    let cmd_slug = crate::daemon::utils::sanitize_cmd_for_window(&cmd, 30);
    let final_win_name = format!(
        "{}{}-{}-{}",
        crate::daemon::SCHED_WINDOW_PREFIX,
        pane_num,
        unix_ts,
        cmd_slug
    );
    let s = session.to_string();
    let t = temp_win_name.to_string();
    let r = final_win_name.clone();
    let renamed = tmux::off_runtime("rename-window", move || tmux::rename_window(&s, &t, &r))
        .await
        .unwrap_or_else(|| Err(anyhow::anyhow!("timed out renaming window")));

    let win_name = match renamed {
        Ok(()) => final_win_name,
        Err(e) => {
            log::warn!(
                "Failed to rename sched window {} -> {}: {}",
                temp_win_name,
                final_win_name,
                e
            );
            temp_win_name
        }
    };

    // P7: keep the pane alive in a '<dead>' state so we can query pane_dead_status.
    let p = pane_id.clone();
    let set = tmux::off_runtime("set-remain-on-exit", move || {
        tmux::set_remain_on_exit(&p, true)
    })
    .await
    .unwrap_or_else(|| Err(anyhow::anyhow!("timed out setting remain-on-exit")));
    if let Err(e) = set {
        log::warn!("Failed to set remain-on-exit for {}: {}", win_name, e);
    }

    let p = pane_id.clone();
    let sent = tmux::off_runtime("send-keys", move || tmux::send_keys(&p, &wrapped))
        .await
        .unwrap_or_else(|| Err(anyhow::anyhow!("timed out sending keys")));
    if let Err(e) = sent {
        let msg = format!("Scheduled job '{}': failed to send keys: {}", job.name, e);
        store.mark_done(&job.id, false, Some(e.to_string()));
        if let Some(ref tx) = notify_tx
            && let Err(e) = tx.send(Response::SystemMsg(msg))
        {
            log::debug!(
                "scheduled job '{}': dropped notification (no receiver): {}",
                job.name,
                e
            );
        }
        return;
    }

    let cmd_id = crate::daemon::stats::start_command(&cmd, "scheduled");

    let mut rx = bg_done_subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);

    let exit_code = loop {
        let p = pane_id.clone();
        let dead = tmux::off_runtime("pane-dead-status", move || tmux::pane_dead_status(&p))
            .await
            .flatten();
        if let Some(code) = dead {
            break code;
        }
        if tokio::time::Instant::now() >= deadline {
            break 124;
        }
        tokio::select! {
            result = rx.recv() => {
                if let Ok(notified_pane) = result
                    && notified_pane == pane_id
                {
                    let p = pane_id.clone();
                    let dead = tmux::off_runtime("pane-dead-status", move || {
                        tmux::pane_dead_status(&p)
                    })
                    .await
                    .flatten();
                    if let Some(code) = dead {
                        break code;
                    }
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                break 124;
            }
        }
    };

    crate::daemon::stats::finish_command(cmd_id, exit_code);

    let p = pane_id.clone();
    let raw = tmux::off_runtime("capture-pane", move || tmux::capture_pane(&p, 5000))
        .await
        .and_then(|r| r.ok())
        .unwrap_or_default();
    let output = normalize_output(&raw);
    let success = exit_code == 0;

    // Runbook / watchdog AI analysis (scheduled-job specific; runs before GC so the pane is still alive)
    if let Some(ref rb_name) = job.runbook
        && let Ok(rb) = runbook::load_runbook(rb_name)
    {
        let model_entry = config.resolve_model(None);
        let client = crate::ai::make_client(
            &model_entry.provider,
            model_entry.resolve_api_key(),
            model_entry.model.clone(),
            model_entry.effective_base_url(),
        );
        let system = runbook::watchdog_system_prompt(&rb);
        let msgs = vec![Message {
            role: "user".to_string(),
            content: format!("Command output:\n```\n{}\n```", output),
            tool_calls: None,
            tool_results: None,
            turn: None,
        }];
        let (ai_tx, mut ai_rx) = tokio::sync::mpsc::unbounded_channel::<AiEvent>();
        let api_err = client
            .chat(&system, msgs, ai_tx, false, Vec::new())
            .await
            .is_err();
        let mut ai_response = String::new();
        while let Some(ev) = ai_rx.recv().await {
            if let AiEvent::Token(t) = ev {
                ai_response.push_str(&t);
            }
        }
        let (should_act, trigger_reason) =
            crate::webhook::evaluate_watchdog_response(&ai_response, api_err);
        log::info!(
            "Scheduler watchdog for '{}': should_act={} reason='{}'",
            job.name,
            should_act,
            trigger_reason
        );
        if should_act {
            let msg = format!("[Watchdog] {}: {}", job.name, ai_response.trim());
            if let Some(ref tx) = notify_tx
                && let Err(e) = tx.send(Response::SystemMsg(msg.clone()))
            {
                log::debug!(
                    "scheduled job '{}': dropped notification (no receiver): {}",
                    job.name,
                    e
                );
            }
            fire_notification(&job.name, &msg, &config);
        }
    }

    store.mark_done(
        &job.id,
        success,
        if success {
            None
        } else {
            Some(format!("exit code {}", exit_code))
        },
    );

    // Hand off to the shared notification + GC handler (non-blocking)
    let cmd_str = cmd.to_string();
    let started_at = tokio::time::Instant::now() - Duration::from_secs(60);
    tokio::spawn(notify_job_completion(
        OwnedJobInfo {
            pane_id,
            cmd: cmd_str,
            win_name,
        },
        session,
        exit_code,
        None,
        notify_tx,
        started_at,
    ));
}
