//! The ghost trigger: a queued bus consumer that runs the webhook watchdog
//! analysis — the model call that may start a ghost shell — for notices the
//! analyze route sends it.
//!
//! `process_alert` publishes the alert notice and stops there; this task
//! takes it off the bus, re-checks the gates the route cannot express (the
//! notice's `kind` label and `webhook.auto_analyze`) and runs the analysis.
//! Each analysis is awaited before the next notice is taken, so one consumer
//! never runs two at once; a ghost the analysis starts is its own spawned
//! task and is not awaited here.

use std::sync::Arc;

use crate::webhook::server::WebhookState;

/// Capacity of the `ghost_trigger` subscriber's queue.
const QUEUE_CAPACITY: usize = 256;

/// Subscribe a queued `ghost_trigger` consumer on `state.bus` and spawn its
/// task. Called once, right after the state is built — by `serve` in
/// production and by the tests that drive the consumer directly.
pub fn register_ghost_trigger(state: Arc<WebhookState>) {
    let rx = state.bus.subscribe(
        "ghost_trigger",
        crate::bus::Filter::default(),
        QUEUE_CAPACITY,
    );
    tokio::spawn(ghost_trigger_task(state, rx));
}

/// The consumer loop: take notices off the queue one at a time and run the
/// analysis for the ones every gate admits.
async fn ghost_trigger_task(
    state: Arc<WebhookState>,
    mut rx: tokio::sync::mpsc::Receiver<Arc<crate::bus::Notice>>,
) {
    while let Some(notice) = rx.recv().await {
        // The gate owns the `kind` rule; the config is read per notice so
        // `daemoneye reload` applies route changes. `state.config` is a
        // startup snapshot `reload` never updates — never route on it.
        if !crate::daemon::notice_sinks::analyze_routed(
            &crate::config::current().notifications,
            &notice,
        ) {
            continue;
        }
        if !state.config.webhook.auto_analyze {
            continue;
        }
        crate::webhook::process::maybe_analyze_alert(&notice, &state).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{Bus, Notice, NoticeStatus, Severity, Source};
    use std::sync::Mutex;

    use crate::config::Config;
    use crate::daemon::session::SessionStore;
    use crate::util::UnpoisonExt;

    /// One settle wait: long enough for the current-thread runtime to run
    /// the consumer to its next await point, bounded so a wedged task fails
    /// instead of hanging. Wall-clock, but only as a bound — the assertion
    /// is on a state change, never on the wait itself.
    const SETTLE: std::time::Duration = std::time::Duration::from_millis(100);

    fn alert(severity: Severity, name: &str) -> Notice {
        Notice::new(severity, Source::Webhook, name, "[Webhook Alert] title")
            .status(NoticeStatus::Firing)
            .label("severity", severity.as_str())
            .label("kind", "alert")
    }

    fn analysis(severity: Severity, name: &str) -> Notice {
        Notice::new(
            severity,
            Source::Webhook,
            name,
            "[Webhook Analysis] title: body",
        )
        .label("severity", severity.as_str())
        .label("kind", "analysis")
    }

    fn ghost(name: &str) -> Notice {
        Notice::new(
            Severity::Info,
            Source::Ghost,
            name,
            "[Ghost Shell Completed]",
        )
        .label("kind", "completed")
    }

    /// A state on its own private leaked bus: publishing on it reaches only
    /// this state's consumers. The caller holds the one `test_home_guard`
    /// for the test's whole body — the guard is not reentrant, so a test
    /// that builds two states must not take it twice.
    fn state(auto_analyze: bool) -> Arc<WebhookState> {
        let bus: &'static Bus = Box::leak(Box::new(Bus::with_journal(|_| {})));
        let mut config = Config::default();
        config.webhook.auto_analyze = auto_analyze;
        Arc::new(WebhookState {
            config,
            sessions: SessionStore::new(),
            cache: Arc::new(crate::tmux::cache::SessionCache::new("test")),
            schedule_store: Arc::new(crate::scheduler::ScheduleStore::new_empty()),
            bus,
            dedup: Mutex::new(std::collections::HashMap::new()),
            rate_limit: Mutex::new(std::collections::HashMap::new()),
            ip_limits: Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// Wait until `ready` turns true, polling at a coarse tick under a
    /// deadline. Used only to observe a side effect the analysis itself
    /// produces; a skipped notice has nothing to wait for, so the test
    /// yields once for queue delivery and asserts.
    async fn wait_for(ready: impl Fn() -> bool, what: &'static str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !ready() {
            assert!(
                std::time::Instant::now() < deadline,
                "the ghost trigger never {what}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn ghost_trigger_skips_what_it_must_not_analyse() {
        // auto_analyze off: a `Warning` alert on a bus whose consumer was
        // registered with `auto_analyze = false` must not be analysed.
        let _home = crate::test_home_guard();
        let state_aoff = state(false);
        register_ghost_trigger(state_aoff.clone());
        state_aoff
            .bus
            .publish(alert(Severity::Warning, "HighMemOff"));
        tokio::time::sleep(SETTLE).await;
        {
            let rl_off = UnpoisonExt::unwrap_or_log(state_aoff.rate_limit.lock());
            assert!(
                !rl_off.contains_key("HighMemOff"),
                "a Warning alert with auto_analyze off must not be analysed: {rl_off:?}"
            );
        }

        // A bus whose consumer has `auto_analyze` on: none of these leaves a
        // rate_limit entry.
        let state = state(true);
        register_ghost_trigger(state.clone());
        // An Info alert is below the analyze threshold.
        state.bus.publish(alert(Severity::Info, "LowCPU"));
        // An analysis notice must not trigger another analysis.
        state.bus.publish(analysis(Severity::Warning, "HighCPU"));
        // A ghost notice is not an alert.
        state.bus.publish(ghost("disk-full"));

        // Nothing here leaves a rate_limit entry: each notice is dropped at
        // a gate before the analysis runs.
        tokio::time::sleep(SETTLE).await;

        let rl = state.rate_limit.lock().unwrap_or_log();
        assert!(
            rl.is_empty(),
            "none of the skipped notices may leave a rate_limit entry: {rl:?}"
        );
    }

    #[tokio::test]
    async fn ghost_trigger_takes_a_warning_alert() {
        let _home = crate::test_home_guard();
        let state = state(true);
        register_ghost_trigger(state.clone());

        state.bus.publish(alert(Severity::Warning, "DiskFull"));

        // The rate-limit entry is the analysis's first side effect: it is
        // written before the runbook lookup, so its presence proves the
        // consumer ran the analysis for this alert's name.
        wait_for(
            || {
                state
                    .rate_limit
                    .lock()
                    .unwrap_or_log()
                    .contains_key("DiskFull")
            },
            "run the analysis",
        )
        .await;
    }
}
