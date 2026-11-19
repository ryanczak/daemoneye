//! Background supervisor tasks spawned by `run_daemon`: cache polling,
//! scheduler ticks, webhook server, session counters, OSC 7501 program
//! notices, and periodic session/artifact/sandbox cleanup.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::config::Config;
use crate::daemon::scheduled::run_scheduled_job;
use crate::daemon::utils::log_event;
use crate::daemon::{DaemonState, scheduled, supervise};
use crate::tmux::cache::SessionCache;

pub(super) fn spawn_cache_poller_task(shutdown: Arc<AtomicBool>, cache: Arc<SessionCache>) {
    log::info!("Cache poller started");
    tokio::spawn(supervise("cache-poller", shutdown, move || {
        let c = Arc::clone(&cache);
        async move {
            loop {
                if let Err(e) = c.refresh() {
                    log::warn!("Failed to refresh tmux cache: {}", e);
                    log_event(
                        "cache_refresh_error",
                        serde_json::json!({ "error": e.to_string() }),
                    );
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }));
}

pub(super) fn spawn_scheduler_task(
    shutdown: Arc<AtomicBool>,
    state: Arc<DaemonState>,
    initial_session: String,
) {
    tokio::spawn(supervise("scheduler", shutdown, move || {
        let state_sched = Arc::clone(&state);
        let sn = initial_session.clone();
        async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tick.tick().await;
                let due = scheduled::due_jobs_for_tick(&state_sched.schedules, &sn);
                for job in due {
                    let state2 = Arc::clone(&state_sched);
                    let sn2 = sn.clone();
                    tokio::spawn(async move {
                        let _ = run_scheduled_job(job, sn2, &state2).await;
                    });
                }
            }
        }
    }));
}

pub(super) async fn spawn_webhook_task(
    shutdown: Arc<AtomicBool>,
    state: Arc<DaemonState>,
    startup_config: &Config,
) -> Result<()> {
    if !startup_config.webhook.enabled {
        return Ok(());
    }
    let listener = crate::webhook::bind(startup_config).await?;
    if startup_config.webhook.secret.is_empty() {
        log::warn!(
            "Webhook listener enabled on port {} — no auth (set webhook.secret in config.toml to require a Bearer token)",
            startup_config.webhook.port
        );
    } else {
        log::info!(
            "Webhook listener enabled on port {} — Bearer token auth required",
            startup_config.webhook.port
        );
    }
    let listener = Arc::new(tokio::sync::Mutex::new(Some(listener)));
    tokio::spawn(supervise("webhook", shutdown, move || {
        let wh_state = Arc::clone(&state);
        let listener = Arc::clone(&listener);
        async move {
            let listener = {
                let mut guard = listener.lock().await;
                guard.take()
            };
            match listener {
                Some(l) => {
                    if let Err(e) = crate::webhook::serve(l, wh_state).await {
                        log::error!("Webhook server exited: {}", e);
                    }
                }
                None => {
                    log::error!("webhook listener was consumed; not restarting");
                }
            }
        }
    }));
    Ok(())
}

async fn session_counters_loop(state: Arc<DaemonState>) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tick.tick().await;
        let session_ids = state.presence.session_ids();
        crate::daemon::counters::counters_tick_with_global(
            &state.shells,
            state.metrics.ghosts.active() as u32,
            &session_ids,
            |sid, frame| {
                crate::daemon::counters::presence_push_to_session(&state.presence, sid, frame);
            },
        );
    }
}

pub(super) fn spawn_session_counters_task(shutdown: Arc<AtomicBool>, state: Arc<DaemonState>) {
    tokio::spawn(supervise("session-counters", shutdown, move || {
        session_counters_loop(Arc::clone(&state))
    }));
}

pub(super) fn spawn_program_notices_task(shutdown: Arc<AtomicBool>) {
    crate::daemon::program_notices::spawn_program_notices_task(shutdown);
}

async fn run_hourly_cleanup_sweep(
    state: &DaemonState,
    log_path: &Option<PathBuf>,
    active_ids: &std::collections::HashSet<String>,
) {
    let tick_config = state.config.current();
    crate::daemon::utils::sweep_event_segments(tick_config.events.retention_days);
    crate::daemon::utils::sweep_session_archives(
        tick_config.sessions.archive_retention_days,
        active_ids,
    );
    crate::daemon::utils::sweep_session_logs(tick_config.sessions.log_retention_days, active_ids);
    crate::daemon::utils::sweep_pane_logs(tick_config.retention.pane_log_retention_days);
    crate::daemon::utils::sweep_agent_mailboxes(tick_config.retention.mailbox_retention_days);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    crate::shell::reap_and_sweep(
        &state.shells,
        &crate::config::shells_dir(),
        tick_config.shells.exited_retention_secs,
        now,
    );
    crate::daemon::executor::knowledge::reap_idle_shells(
        &state.shells,
        tick_config.shells.idle_reap_secs,
        now,
    )
    .await;
    let program = crate::shell::which_sbx().unwrap_or_else(|_| "sbx".to_string());
    for reaped in crate::daemon::executor::chat_sandbox::reap_idle_chat_sandboxes(
        &crate::shell::real_runner(program),
        &state.shells,
        now,
    )
    .await
    {
        if let Some(audit) = reaped.egress {
            crate::daemon::utils::log_event(
                "sandbox_egress",
                crate::daemon::executor::sbx::egress_event(&reaped.name, &audit),
            );
        }
        crate::daemon::utils::log_event(
            "chat_sandbox_teardown",
            serde_json::json!({ "sandbox": reaped.name, "outcome": format!("{:?}", reaped.teardown) }),
        );
    }
    let mut live_shell_ids = std::collections::BTreeSet::new();
    crate::shell::with_shells(&state.shells, |store| {
        live_shell_ids.extend(store.keys().cloned());
    });
    crate::daemon::utils::sweep_shell_logs(tick_config.shells.log_retention_days, &live_shell_ids);
    if let Some(lp) = log_path
        && crate::daemon::utils::rotate_log_file(
            lp,
            tick_config.logging.log_max_bytes,
            tick_config.logging.log_keep_count,
        )
    {
        crate::daemon::utils::reattach_log_fds(lp);
    }
}

pub(super) fn spawn_session_cleanup_task(
    shutdown: Arc<AtomicBool>,
    state: Arc<DaemonState>,
    log_file: Option<PathBuf>,
) {
    tokio::spawn(supervise("session-cleanup", shutdown, move || {
        let state_cleanup = Arc::clone(&state);
        let log_path = log_file.clone();
        async move {
            let mut sweep_counter = 0u32;
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let (_evicted, active_ids) = crate::daemon::session::cleanup_pass(
                    &state_cleanup.sessions,
                    Instant::now(),
                    Duration::from_secs(1800),
                );
                sweep_counter = sweep_counter.wrapping_add(1);
                if sweep_counter.is_multiple_of(60) {
                    run_hourly_cleanup_sweep(&state_cleanup, &log_path, &active_ids).await;
                }
            }
        }
    }));
}
