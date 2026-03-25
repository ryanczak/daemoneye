use crate::ai::{AiEvent, Message};
use crate::config::Config;
use crate::daemon::background::notify_job_completion;
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
    schedule_store: Arc<ScheduleStore>,
    notify_tx: Option<tokio::sync::mpsc::UnboundedSender<Response>>,
) {
    crate::daemon::stats::inc_schedules_executed();

    // Ghost-mode: hand off entirely to the ghost shell infrastructure.
    #[allow(deprecated)]
    if let ActionOn::Ghost { runbook: rb_name } = &job.action {
        if !check_ghost_capacity(&config) {
            log::warn!(
                "Scheduled ghost job '{}': skipped — concurrency limit ({}) reached",
                job.name,
                config.ghost.max_concurrent_ghosts
            );
            inject_ghost_event(
                &sessions,
                &format!("[Ghost Shell Skipped] Scheduled job '{}' skipped — concurrency limit reached", job.name),
            );
            store.mark_done(&job.id, false, Some("ghost concurrency limit reached".to_string()));
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
                match GhostManager::start_session(sessions.clone(), &rb, &alert_msg, crate::daemon::GS_SCHED_WINDOW_PREFIX).await {
                    Err(e) => {
                        let msg = format!(
                            "Scheduled ghost job '{}': failed to start session: {}",
                            job.name, e
                        );
                        log::error!("{}", msg);
                        inject_ghost_event(
                            &sessions,
                            &format!("[Ghost Shell Failed] Scheduled job '{}' could not start: {}", job.name, e),
                        );
                        store.mark_done(&job.id, false, Some(msg));
                    }
                    Ok(sid) => {
                        inject_ghost_event(
                            &sessions,
                            &format!("[Ghost Shell Started] Scheduled job '{}' started autonomous session", job.name),
                        );
                        let result =
                            trigger_ghost_turn(&sid, &sessions, &config, &cache, &schedule_store)
                                .await;
                        match result {
                            Ok(()) => {
                                inject_ghost_event(
                                    &sessions,
                                    &format!("[Ghost Shell Completed] Scheduled job '{}' finished", job.name),
                                );
                                store.mark_done(&job.id, true, None);
                            }
                            Err(e) => {
                                log::error!("Scheduled ghost job '{}' failed: {}", job.name, e);
                                inject_ghost_event(
                                    &sessions,
                                    &format!("[Ghost Shell Failed] Scheduled job '{}' error: {}", job.name, e),
                                );
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

    let id_short = &job.id[..job.id.len().min(8)];
    let now = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let win_name = format!("{}{}-{}", crate::daemon::SCHED_WINDOW_PREFIX, now, id_short);
    #[allow(deprecated)]
    let cmd = match &job.action {
        ActionOn::Alert => {
            // Pure alert: no command to run.
            store.mark_done(&job.id, true, None);
            let msg = format!("Watchdog alert: {}", job.name);
            if let Some(ref tx) = notify_tx {
                let _ = tx.send(Response::SystemMsg(msg.clone()));
            }
            fire_notification(&job.name, &msg, &config);
            return;
        }
        ActionOn::Command(c) => {
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
                if let Some(ref tx) = notify_tx {
                    let _ = tx.send(Response::SystemMsg(msg));
                }
                return;
            }
        },
        ActionOn::Ghost { .. } => unreachable!("handled above"),
    };

    let wrapped = format!("{}; exit $?", cmd);

    let pane_id = match tmux::create_job_window(&session, &win_name) {
        Ok(p) => p,
        Err(e) => {
            let msg = format!(
                "Scheduled job '{}': failed to create window: {}",
                job.name, e
            );
            store.mark_done(&job.id, false, Some(e.to_string()));
            if let Some(ref tx) = notify_tx {
                let _ = tx.send(Response::SystemMsg(msg));
            }
            return;
        }
    };

    // P7: keep the pane alive in a '<dead>' state so we can query pane_dead_status.
    if let Err(e) = tmux::set_remain_on_exit(&pane_id, true) {
        log::warn!("Failed to set remain-on-exit for {}: {}", win_name, e);
    }

    if let Err(e) = tmux::send_keys(&pane_id, &wrapped) {
        let msg = format!("Scheduled job '{}': failed to send keys: {}", job.name, e);
        store.mark_done(&job.id, false, Some(e.to_string()));
        if let Some(ref tx) = notify_tx {
            let _ = tx.send(Response::SystemMsg(msg));
        }
        return;
    }

    let cmd_id = crate::daemon::stats::start_command(&cmd, "scheduled");

    let mut rx = bg_done_subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);

    let exit_code = loop {
        if let Some(code) = tmux::pane_dead_status(&pane_id) {
            break code;
        }
        if tokio::time::Instant::now() >= deadline {
            break 124;
        }
        tokio::select! {
            result = rx.recv() => {
                if let Ok(notified_pane) = result {
                    if notified_pane == pane_id {
                        if let Some(code) = tmux::pane_dead_status(&pane_id) {
                            break code;
                        }
                    }
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                break 124;
            }
        }
    };

    crate::daemon::stats::finish_command(cmd_id, exit_code);

    let raw = tmux::capture_pane(&pane_id, 5000).unwrap_or_default();
    let output = normalize_output(&raw);
    let success = exit_code == 0;

    // Runbook / watchdog AI analysis (scheduled-job specific; runs before GC so the pane is still alive)
    if let Some(ref rb_name) = job.runbook {
        if let Ok(rb) = runbook::load_runbook(rb_name) {
            let api_key = config.ai.resolve_api_key();
            let client = crate::ai::make_client(
                &config.ai.provider,
                api_key,
                config.ai.model.clone(),
                config.ai.effective_base_url(),
            );
            let system = runbook::watchdog_system_prompt(&rb);
            let msgs = vec![Message {
                role: "user".to_string(),
                content: format!("Command output:\n```\n{}\n```", output),
                tool_calls: None,
                tool_results: None,
            }];
            let (ai_tx, mut ai_rx) = tokio::sync::mpsc::unbounded_channel::<AiEvent>();
            let api_err = client.chat(&system, msgs, ai_tx, false).await.is_err();
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
                job.name, should_act, trigger_reason
            );
            if should_act {
                let msg = format!("[Watchdog] {}: {}", job.name, ai_response.trim());
                if let Some(ref tx) = notify_tx {
                    let _ = tx.send(Response::SystemMsg(msg.clone()));
                }
                fire_notification(&job.name, &msg, &config);
            }
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
        pane_id, cmd_str, win_name, session, exit_code, None, sessions, notify_tx, started_at,
    ));
}
