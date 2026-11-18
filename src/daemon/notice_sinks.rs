//! The notice bus's three consumers (M38 phase 02): the chat-history
//! sink (inline), the chat-notice sink (queued) and the command sink
//! (queued, configured `command` routes only). [`register_default_sinks`]
//! subscribes all three on the global bus at daemon startup, with
//! [`default_routes`] reproducing what 2.9.0 delivers. Nothing publishes
//! until phases 03–05 wire the producers.

use crate::ai::Message;
use crate::bus::{Bus, Filter, Notice, Severity, Source};
use crate::daemon::session::{SessionStore, append_session_message, with_sessions};

/// How long the command-sink test polls the file the route's command
/// writes before giving up.
#[cfg(test)]
const ON_ALERT_POLL: std::time::Duration = std::time::Duration::from_secs(5);

/// Per-sink route lists. A notice reaches a sink when **any** of the sink's
/// filters matches. The command sink has no built-in routes: it runs only
/// configured `command` routes.
#[derive(Debug, Clone, Default)]
pub struct SinkRoutes {
    pub history: Vec<Filter>,
    pub chat_notice: Vec<Filter>,
    pub alert_registry: Vec<Filter>,
    pub analyze: Vec<Filter>,
}

/// Whether `n` is routed to a sink whose routes are `routes`: any matching
/// filter routes it; an empty list matches nothing.
pub fn routes_match(routes: &[Filter], n: &Notice) -> bool {
    routes.iter().any(|f| f.matches(n))
}

/// The analyze gate the alert producer and the ghost-trigger consumer
/// share: true when the notice is a webhook alert (`kind` = `alert`) and
/// [`sink_routed`] matches `default_routes(Severity::Warning).analyze`
/// (webhook, firing or resolved, at or above `warning`) against the
/// config the caller read per notice. The `kind` rule lives here because
/// a configured route with no `statuses` — `sink = "analyze"` filtered
/// only by `names` — matches an analysis notice too, and an analysis
/// triggering another analysis would loop for ever.
pub fn analyze_routed(cfg: &crate::config::NotificationsConfig, n: &Notice) -> bool {
    n.labels.get("kind").map(String::as_str) == Some("alert")
        && sink_routed(
            cfg,
            &default_routes(Severity::Warning).analyze,
            crate::config::RouteSink::Analyze,
            n,
        )
}

/// Whether `sink` receives `n`: the built-in defaults when
/// `[notifications] default_routes` is on, plus every configured
/// `[[notifications.route]]` for that sink whose filter matches. A
/// configured route **adds to** the defaults.
pub fn sink_routed(
    cfg: &crate::config::NotificationsConfig,
    defaults: &[Filter],
    sink: crate::config::RouteSink,
    n: &Notice,
) -> bool {
    let mut routed = cfg.default_routes && routes_match(defaults, n);
    if !routed {
        routed = cfg
            .route
            .iter()
            .any(|r| r.sink == sink && r.filter().matches(n));
    }
    routed
}

/// The commands the command sink runs for `n`, in order: each matching
/// configured `command` route's command, in config order. Each distinct
/// command appears once, first occurrence wins; a blank command never runs.
pub fn command_targets(cfg: &crate::config::NotificationsConfig, n: &Notice) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for route in &cfg.route {
        if route.sink == crate::config::RouteSink::Command
            && route.filter().matches(n)
            && let Some(cmd) = &route.command
        {
            let cmd = cmd.trim();
            if !cmd.is_empty() && !out.iter().any(|c| c == cmd) {
                out.push(cmd.to_string());
            }
        }
    }
    out
}

/// The routes that reproduce what 2.9.0 delivers, given the webhook
/// severity threshold:
/// - history: webhook ≥ threshold; ghost (any); background (any).
/// - chat_notice: webhook ≥ threshold; ghost; scheduler; watchdog.
/// - alert_registry: webhook firing/resolved at **any** severity — a
///   below-threshold alert is still recorded, as in 2.9.0.
pub fn default_routes(threshold: Severity) -> SinkRoutes {
    let webhook_at_or_above = Filter {
        min_severity: Some(threshold),
        sources: vec![Source::Webhook],
        ..Filter::default()
    };
    let ghost_any = Filter {
        sources: vec![Source::Ghost],
        ..Filter::default()
    };
    let background_any = Filter {
        sources: vec![Source::Background],
        ..Filter::default()
    };
    let scheduler_any = Filter {
        sources: vec![Source::Scheduler],
        ..Filter::default()
    };
    let watchdog_any = Filter {
        sources: vec![Source::Watchdog],
        ..Filter::default()
    };
    SinkRoutes {
        history: vec![
            webhook_at_or_above.clone(),
            ghost_any.clone(),
            background_any,
        ],
        chat_notice: vec![webhook_at_or_above, ghost_any, scheduler_any, watchdog_any],
        alert_registry: vec![Filter {
            sources: vec![Source::Webhook],
            statuses: vec![
                crate::bus::NoticeStatus::Firing,
                crate::bus::NoticeStatus::Resolved,
            ],
            ..Filter::default()
        }],
        analyze: vec![Filter {
            min_severity: Some(threshold),
            sources: vec![Source::Webhook],
            statuses: vec![
                crate::bus::NoticeStatus::Firing,
                crate::bus::NoticeStatus::Resolved,
            ],
            ..Filter::default()
        }],
    }
}

/// The `Message` a history sink appends for `notice`.
fn history_message(notice: &Notice) -> Message {
    Message {
        role: "user".to_string(),
        content: notice.history_text(),
        tool_calls: None,
        tool_results: None,
        turn: None,
    }
}

/// The history sink's inline delivery.
///
/// `session: None` → the same set `inject_into_sessions` reaches.
/// `session: Some(id)` → that session only: the file append, then the
/// in-memory `push_event` when the entry is resident — the same sequence
/// the background-completion delivery uses.
///
/// **A producer must not publish while holding the `SessionStore` lock.**
/// This sink takes that lock, and the bus lock is held across inline
/// delivery — the two would deadlock.
fn deliver_history(sessions: &SessionStore, notice: &Notice) {
    let msg = history_message(notice);
    match &notice.session {
        None => crate::webhook::inject_into_sessions(sessions, &msg),
        Some(id) => {
            append_session_message(id, &msg);
            with_sessions(sessions, |store| {
                if let Some(entry) = store.get_mut(id) {
                    entry.push_event(msg);
                }
            });
        }
    }
}

/// The chat-notice sink's queued delivery: one overlay line to every chat
/// pane, the same tmux display-message + presence push
/// `notify_chat_panes` does.
async fn deliver_chat_notice(sessions: SessionStore, line: String) {
    let _ = crate::process::off_runtime("notify-chat-panes", move || {
        crate::webhook::notify_chat_panes(&sessions, &line)
    })
    .await;
}

/// A route command's environment, in pair form so the test asserts
/// the names and values without spawning anything.
pub fn command_env(notice: &Notice) -> Vec<(&'static str, String)> {
    vec![
        ("DAEMONEYE_JOB", notice.name.clone()),
        ("DAEMONEYE_MSG", notice.history_text()),
        ("DAEMONEYE_SEVERITY", notice.severity.as_str().to_string()),
        ("DAEMONEYE_SOURCE", notice.source.as_str().to_string()),
    ]
}

/// One command sink delivery: spawn `sh -c <cmd>` with the notice's
/// environment — the same fire-and-forget `fire_notification` does. The
/// command comes from `command_targets` (a configured `command` route).
async fn deliver_command(cmd: String, notice: Notice) {
    let mut command = tokio::process::Command::new("sh");
    command.arg("-c").arg(cmd);
    for (key, value) in command_env(&notice) {
        command.env(key, value);
    }
    let _ = command.spawn();
}

/// The registry update both paths share: a firing notice records the alert
/// at the notice's `ts`, a resolved notice removes it, an event notice does
/// neither. The severity recorded is the raw label (`labels["severity"]`),
/// falling back to the parsed severity's name, so an unrankable raw string
/// is preserved for the snapshot's ranking.
pub(crate) fn apply_notice_to_registry_in(
    map: &std::sync::Mutex<std::collections::HashMap<String, crate::webhook::ActiveAlert>>,
) -> impl Fn(&Notice) -> bool + '_ {
    move |notice: &Notice| {
        let severity = notice
            .labels
            .get("severity")
            .cloned()
            .unwrap_or_else(|| notice.severity.as_str().to_string());
        match notice.status {
            crate::bus::NoticeStatus::Firing => crate::webhook::active_alerts_firing_in(
                map,
                &notice.fingerprint,
                &notice.name,
                &severity,
                notice.ts.timestamp() as u64,
            ),
            crate::bus::NoticeStatus::Resolved => {
                crate::webhook::active_alerts_resolved_in(map, &notice.fingerprint)
            }
            crate::bus::NoticeStatus::Event => false,
        }
    }
}

/// The alert-registry sink's inline delivery: a firing notice records the
/// alert, a resolved notice removes it. When either changes the process-wide
/// registry, broadcast the new snapshot to every attached chat over the
/// presence connection — the same frame `process_alert` used to send.
///
/// The registry is process-global: tests need a fingerprint no other test
/// uses.
fn record_alert_registry(notice: &Notice) {
    let changed = apply_notice_to_registry_in(crate::webhook::registry())(notice);
    if changed {
        let frame = crate::ipc::Response::AlertState {
            alerts: crate::webhook::active_alerts_snapshot(),
        };
        crate::daemon::session::presence_broadcast_frame(&frame);
    }
}

/// Rebuild the active-alert registry from the notice journal at daemon
/// startup: replay every notice in `[now - ACTIVE_ALERT_TTL_SECS, now)` the
/// registry's route matches, in journal order, into `map` — `active.rs`'s
/// `_in` seam style, so tests fill their own map and production fills the
/// process-wide registry.
///
/// The replay never broadcasts: no chat is attached yet at startup, and the
/// attach flow reads the registry it just rebuilt.
pub(crate) fn replay_alert_registry_in(
    map: &std::sync::Mutex<std::collections::HashMap<String, crate::webhook::ActiveAlert>>,
) -> usize {
    let routes = default_routes(crate::bus::Severity::Warning).alert_registry;
    let now = chrono::Utc::now();
    let from = now - chrono::Duration::seconds(crate::webhook::ACTIVE_ALERT_TTL_SECS as i64);

    let apply = apply_notice_to_registry_in(map);
    crate::bus::for_each_notice_between(Some(from), Some(now), &mut |notice| {
        if routes_match(&routes, notice) {
            apply(notice);
        }
    });
    // What survived the replay — a fire later resolved restores nothing.
    let restored = {
        use crate::util::UnpoisonExt;
        map.lock().unwrap_or_log().len()
    };
    if restored > 0 {
        log::info!("Alert registry replay: {restored} alert(s) restored");
    }
    restored
}

/// A leaked bus with the default sinks registered on it, for tests.
///
/// `register_default_sinks` spawns tasks, so this must run inside a Tokio
/// runtime (inside `rt.block_on(async { .. })` or after `rt.enter()`).
/// Every test passes its **own** [`SessionStore`]: the sinks on a shared
/// bus deliver one test's notices into another test's sessions, so tests
/// never use the global bus.
pub fn test_bus(sessions: SessionStore, threshold: Severity) -> &'static Bus {
    let leaked: &'static Bus = Box::leak(Box::new(Bus::with_journal(|_| {})));
    register_default_sinks(leaked, sessions, default_routes(threshold));
    leaked
}

/// Subscribe the four sinks on `bus` and spawn the two queued tasks.
///
/// The inline sinks (`history`, `alert_registry`) run inside `publish`;
/// each queued sink gets capacity 256 and its subscriber name
/// (`chat_notice`, `command`). Called once by `run_daemon`, on
/// [`crate::bus::global`], right after the `SessionStore` is created.
pub fn register_default_sinks(bus: &Bus, sessions: SessionStore, routes: SinkRoutes) {
    let sessions_history = sessions.clone();
    let history_routes = routes.history.clone();
    bus.subscribe_inline("history", Filter::default(), move |n| {
        let cfg = crate::config::current().notifications.clone();
        if sink_routed(&cfg, &history_routes, crate::config::RouteSink::History, n) {
            deliver_history(&sessions_history, n);
        }
    });

    let registry_routes = routes.alert_registry.clone();
    bus.subscribe_inline("alert_registry", Filter::default(), move |n| {
        if routes_match(&registry_routes, n) {
            record_alert_registry(n);
        }
    });

    let chat_rx = bus.subscribe("chat_notice", Filter::default(), 256);
    let chat_sessions = sessions.clone();
    let chat_routes = routes.chat_notice.clone();
    tokio::spawn(async move {
        let mut rx = chat_rx;
        while let Some(notice) = rx.recv().await {
            let cfg = crate::config::current().notifications.clone();
            if sink_routed(
                &cfg,
                &chat_routes,
                crate::config::RouteSink::ChatNotice,
                &notice,
            ) {
                let line = notice.line().to_string();
                deliver_chat_notice(chat_sessions.clone(), line).await;
            }
        }
    });
    let cmd_rx = bus.subscribe("command", Filter::default(), 256);
    tokio::spawn(async move {
        let mut rx = cmd_rx;
        while let Some(notice) = rx.recv().await {
            let cfg = crate::config::current().notifications.clone();
            for cmd in command_targets(&cfg, &notice) {
                deliver_command(cmd, notice.as_ref().clone()).await;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{NoticeStatus, Source};
    use crate::daemon::session::SessionEntry;

    fn webhook(severity: Severity, name: &str) -> Notice {
        Notice::new(severity, Source::Webhook, name, "title")
            .status(NoticeStatus::Firing)
            .label("kind", "alert")
    }

    fn fresh_registry_map()
    -> std::sync::Mutex<std::collections::HashMap<String, crate::webhook::ActiveAlert>> {
        std::sync::Mutex::new(std::collections::HashMap::new())
    }

    #[test]
    fn replay_rebuilds_what_is_still_firing() {
        let map = fresh_registry_map();
        let a = webhook(Severity::Warning, "AlertA").fingerprint("fp-replay-a");
        let b = webhook(Severity::Warning, "AlertB").fingerprint("fp-replay-b");
        let resolved_a = Notice {
            status: NoticeStatus::Resolved,
            ..a.clone()
        };

        let apply = apply_notice_to_registry_in(&map);
        apply(&a);
        apply(&b);
        apply(&resolved_a);

        let snapshot = crate::webhook::active_alerts_snapshot_in(&map, crate::webhook::now_secs());
        let fingerprints: Vec<&str> = snapshot
            .iter()
            .map(|alert| alert.fingerprint.as_str())
            .collect();
        assert_eq!(fingerprints, vec!["fp-replay-b"], "only B is still firing");
    }

    #[test]
    fn replay_keeps_first_seen_across_refires() {
        let map = fresh_registry_map();
        let b1 = webhook(Severity::Warning, "AlertB")
            .fingerprint("fp-replay-refire")
            .ts_refire(1_000);
        let b2 = webhook(Severity::Warning, "AlertB")
            .fingerprint("fp-replay-refire")
            .ts_refire(2_000);

        let apply = apply_notice_to_registry_in(&map);
        apply(&b1);
        apply(&b2);

        let snapshot = crate::webhook::active_alerts_snapshot_in(&map, 2_000);
        let entry = snapshot
            .iter()
            .find(|a| a.fingerprint == "fp-replay-refire")
            .expect("the refired alert is in the snapshot");
        assert_eq!(entry.first_seen_unix, 1_000, "first_seen is the first fire");
        assert_eq!(entry.last_seen_unix, 2_000, "last_seen is the latest fire");
    }

    /// Journal `notices` as `Bus::new` journals them, then run the real
    /// replay into a fresh map; return what it restored and the snapshot's
    /// fingerprints.
    fn journal_and_replay(notices: &[&Notice]) -> (usize, Vec<String>) {
        for notice in notices {
            crate::daemon::utils::log_event(
                "notice",
                serde_json::to_value(notice).expect("serialize notice"),
            );
        }
        let map = fresh_registry_map();
        let restored = replay_alert_registry_in(&map);
        let fingerprints =
            crate::webhook::active_alerts_snapshot_in(&map, crate::webhook::now_secs())
                .into_iter()
                .map(|alert| alert.fingerprint)
                .collect();
        (restored, fingerprints)
    }

    #[test]
    fn replay_ignores_what_the_registry_route_does_not_match() {
        let _guard = crate::test_home();
        let hour_ago = (crate::webhook::now_secs() - 3600) as i64;

        // A webhook `Event` notice (an analysis) and a scheduler notice marked
        // `Firing`: the registry route matches only firing/resolved webhook
        // notices, so the replay must record neither.
        let analysis = Notice::new(Severity::Info, Source::Webhook, "AnalysisDone", "analysis")
            .fingerprint("fp-replay-analysis")
            .ts_refire(hour_ago);
        let scheduler_firing = Notice::new(Severity::Warning, Source::Scheduler, "JobDone", "job")
            .fingerprint("fp-replay-scheduler")
            .status(NoticeStatus::Firing)
            .ts_refire(hour_ago);
        // The positive control: a firing webhook notice the route does match.
        let control = webhook(Severity::Warning, "Control")
            .fingerprint("fp-replay-control")
            .ts_refire(hour_ago);

        let (restored, fingerprints) =
            journal_and_replay(&[&analysis, &scheduler_firing, &control]);
        assert_eq!(
            fingerprints,
            vec!["fp-replay-control".to_string()],
            "only the firing webhook notice is replayed"
        );
        assert_eq!(restored, 1, "the replay reports what it restored");
    }

    #[test]
    fn replay_reads_only_the_ttl_window() {
        let _guard = crate::test_home();
        let now = crate::webhook::now_secs();
        let inside = webhook(Severity::Warning, "InsideTTL")
            .fingerprint("fp-replay-inside")
            .ts_refire((now - 3600) as i64);
        let outside = webhook(Severity::Warning, "OutsideTTL")
            .fingerprint("fp-replay-outside")
            .ts_refire((now - crate::webhook::ACTIVE_ALERT_TTL_SECS - 60) as i64);

        let (restored, fingerprints) = journal_and_replay(&[&inside, &outside]);
        assert_eq!(
            fingerprints,
            vec!["fp-replay-inside".to_string()],
            "the notice older than the TTL never enters the registry"
        );
        assert_eq!(restored, 1, "only the notice inside the window is restored");
    }

    #[test]
    fn replay_counts_survivors_not_changes() {
        let _guard = crate::test_home();
        let hour_ago = (crate::webhook::now_secs() - 3600) as i64;
        let fired = webhook(Severity::Warning, "Gone")
            .fingerprint("fp-replay-gone")
            .ts_refire(hour_ago);
        let resolved = Notice {
            status: NoticeStatus::Resolved,
            ..fired.clone()
        };
        let (restored, fingerprints) = journal_and_replay(&[&fired, &resolved]);
        assert!(fingerprints.is_empty(), "a resolved alert is not restored");
        assert_eq!(restored, 0, "a fire then resolve restores nothing");
    }

    fn resident_entry() -> SessionEntry {
        SessionEntry {
            messages: Vec::new(),
            last_accessed: std::time::Instant::now(),
            chat_pane: None,
            default_target_pane: None,
            bg_windows: Vec::new(),
            last_prompt_tokens: 0,
            tmux_session: String::new(),
            last_detach: None,
            detach_time_utc: None,
            messages_at_detach: 0,
            is_ghost: false,
            ghost_config: None,
            ghost_runbook_name: None,
            ghost_bg_prefix: "",
            started_at: chrono::Utc::now(),
            turn_count: 0,
            tool_calls_this_session: 0,
            active_model: None,
            last_snapshot_activity: 0,
            saved_name: None,
            dirty: false,
            artifacts_created: Vec::new(),
            auto_name_suggested: false,
            ghost_task_message: None,
            loaded_tools: std::collections::HashSet::new(),
            cost_usd: 0.0,
            cost_by_agent: std::collections::HashMap::new(),
            has_untracked_cost: false,
            token_scale: 1.5,
            compaction_in_flight: false,
            pending_compaction_notice: None,
            turn_inbox: None,
        }
    }

    fn insert_resident(sessions: &SessionStore, id: &str) {
        with_sessions(sessions, |store| {
            store.insert(id.to_string(), resident_entry());
        });
    }

    fn message_count(sessions: &SessionStore, id: &str) -> usize {
        with_sessions(sessions, |store| {
            store.get(id).map(|e| e.messages.len()).unwrap_or(0)
        })
    }

    #[test]
    fn routes_match_is_any_of_and_empty_matches_nothing() {
        let ghost = Filter {
            sources: vec![Source::Ghost],
            ..Filter::default()
        };
        let scheduler = Filter {
            sources: vec![Source::Scheduler],
            ..Filter::default()
        };
        let n = Notice::new(Severity::Info, Source::Ghost, "j", "t");
        assert!(
            routes_match(std::slice::from_ref(&ghost), &n),
            "ghost matches ghost"
        );
        assert!(
            routes_match(&[ghost.clone(), scheduler], &n),
            "any match routes the notice"
        );
        assert!(
            !routes_match(&[], &n),
            "an empty route list matches nothing"
        );
        let other = Notice::new(Severity::Info, Source::Webhook, "j", "t");
        assert!(!routes_match(&[ghost], &other), "no filter matches");
    }

    #[test]
    fn default_routes_reproduce_2_9_0() {
        let routes = default_routes(Severity::Warning);

        let web_info = webhook(Severity::Info, "HighCPU");
        assert!(
            !routes_match(&routes.history, &web_info)
                && !routes_match(&routes.chat_notice, &web_info),
            "webhook info is below the warning threshold: no sink"
        );

        let web_warning = webhook(Severity::Warning, "HighCPU");
        assert!(
            routes_match(&routes.history, &web_warning)
                && routes_match(&routes.chat_notice, &web_warning),
            "webhook warning reaches history and chat_notice"
        );

        let ghost = Notice::new(Severity::Debug, Source::Ghost, "ghost-exit", "t");
        assert!(
            routes_match(&routes.history, &ghost) && routes_match(&routes.chat_notice, &ghost),
            "ghost (any severity) reaches history and chat_notice"
        );

        let background = Notice::new(Severity::Info, Source::Background, "bg-done", "t");
        assert!(
            routes_match(&routes.history, &background)
                && !routes_match(&routes.chat_notice, &background),
            "background reaches history only"
        );

        for source in [Source::Scheduler, Source::Watchdog] {
            let n = Notice::new(Severity::Info, source, "job", "t");
            assert!(
                !routes_match(&routes.history, &n) && routes_match(&routes.chat_notice, &n),
                "{source:?} reaches chat_notice, not history"
            );
        }

        let resolved = webhook(Severity::Warning, "HighCPU").status(NoticeStatus::Resolved);
        assert!(
            routes_match(&routes.analyze, &web_warning),
            "a webhook warning firing alert matches the analyze route"
        );
        assert!(
            routes_match(&routes.analyze, &resolved),
            "a webhook warning resolved alert matches the analyze route"
        );
        assert!(
            !routes_match(&routes.analyze, &web_info),
            "a webhook info alert is below the analyze threshold"
        );
    }

    /// A webhook analysis notice (status `Event`, `kind` = `analysis`) must
    /// not match the analyze route: an analysis triggering another analysis
    /// would loop for ever.
    #[test]
    fn analyze_route_skips_analysis_notices() {
        let analysis = Notice::new(
            Severity::Warning,
            Source::Webhook,
            "HighCPU",
            "[Webhook Analysis] HighCPU: spike traced",
        )
        .label("kind", "analysis");
        assert_eq!(analysis.status, NoticeStatus::Event);
        assert!(
            !default_routes(Severity::Warning)
                .analyze
                .iter()
                .any(|f| f.matches(&analysis)),
            "an analysis notice does not match the analyze route"
        );
        assert!(
            !crate::daemon::notice_sinks::analyze_routed(
                &crate::config::NotificationsConfig::default(),
                &analysis
            ),
            "analyze_routed rejects the analysis notice"
        );
    }

    /// The analyze gate honours the config: the default routes keep the
    /// warning threshold; `default_routes = false` drops them; a configured
    /// `analyze` route adds to (or replaces) them.
    #[test]
    fn analyze_routed_honours_configured_routes() {
        let warning = webhook(Severity::Warning, "HighCPU");
        let info_other = webhook(Severity::Info, "DiskFull");

        let default_cfg = crate::config::NotificationsConfig::default();
        assert!(
            crate::daemon::notice_sinks::analyze_routed(&default_cfg, &warning),
            "default config routes a webhook warning alert"
        );
        assert!(
            !crate::daemon::notice_sinks::analyze_routed(&default_cfg, &info_other),
            "default config does not route a webhook info alert"
        );

        let cfg: crate::config::Config = toml::from_str(
            r#"
            [notifications]
            default_routes = false
        "#,
        )
        .expect("valid notifications config");
        assert!(
            !crate::daemon::notice_sinks::analyze_routed(&cfg.notifications, &warning),
            "with default_routes off and no routes, nothing is analyzed"
        );

        let cfg: crate::config::Config = toml::from_str(
            r#"
            [notifications]
            default_routes = false

            [[notifications.route]]
            sink = "analyze"
            names = ["LowCPU"]
        "#,
        )
        .expect("valid analyze route config");
        let info_low = webhook(Severity::Info, "LowCPU");
        assert!(
            crate::daemon::notice_sinks::analyze_routed(&cfg.notifications, &info_low),
            "the configured analyze route routes the info LowCPU alert"
        );
        assert!(
            !crate::daemon::notice_sinks::analyze_routed(&cfg.notifications, &info_other),
            "the configured analyze route does not route another name"
        );
    }

    /// A configured `analyze` route with no `statuses` matches an analysis
    /// notice too — the gate's `kind` rule is what keeps an analysis from
    /// triggering another.
    #[test]
    fn analyze_routed_never_routes_an_analysis() {
        let cfg: crate::config::Config = toml::from_str(
            r#"
            [[notifications.route]]
            sink = "analyze"
            names = ["HighCPU"]
        "#,
        )
        .expect("valid analyze route config");
        let analysis = Notice::new(
            Severity::Warning,
            Source::Webhook,
            "HighCPU",
            "[Webhook Analysis] HighCPU: spike traced",
        )
        .label("kind", "analysis");
        assert_eq!(analysis.status, NoticeStatus::Event);
        assert!(
            cfg.notifications.route[0].filter().matches(&analysis),
            "precondition: the configured route's filter alone matches the analysis notice"
        );
        assert!(
            !crate::daemon::notice_sinks::analyze_routed(&cfg.notifications, &analysis),
            "the gate never routes an analysis notice"
        );
    }

    /// A configured route reaches a notice the defaults ignore, while the
    /// defaults still deliver what they always did.
    #[test]
    fn configured_route_adds_to_the_defaults() {
        let cfg: crate::config::Config = toml::from_str(
            r#"
            [[notifications.route]]
            sink = "history"
            sources = ["shell"]
        "#,
        )
        .expect("valid route config");
        let defaults = default_routes(Severity::Warning);

        let shell = Notice::new(Severity::Info, Source::Shell, "sh-1", "t");
        assert!(
            !routes_match(&defaults.history, &shell),
            "precondition: the defaults do not route a shell notice"
        );
        assert!(
            sink_routed(
                &cfg.notifications,
                &defaults.history,
                crate::config::RouteSink::History,
                &shell
            ),
            "the configured shell route delivers it"
        );

        let ghost = Notice::new(Severity::Debug, Source::Ghost, "ghost-exit", "t");
        assert!(
            routes_match(&defaults.history, &ghost),
            "precondition: the defaults route a ghost notice"
        );
        assert!(
            sink_routed(
                &cfg.notifications,
                &defaults.history,
                crate::config::RouteSink::History,
                &ghost
            ),
            "the ghost notice still reaches history through the defaults"
        );
    }

    /// `default_routes = false` drops only the defaults; configured routes
    /// still deliver.
    #[test]
    fn default_routes_false_drops_only_the_defaults() {
        let cfg: crate::config::Config = toml::from_str(
            r#"
            [notifications]
            default_routes = false

            [[notifications.route]]
            sink = "history"
            sources = ["shell"]
        "#,
        )
        .expect("valid route config");
        let defaults = default_routes(Severity::Warning);

        let shell = Notice::new(Severity::Info, Source::Shell, "sh-1", "t");
        assert!(
            sink_routed(
                &cfg.notifications,
                &defaults.history,
                crate::config::RouteSink::History,
                &shell
            ),
            "the configured shell route survives the switch"
        );

        let ghost = Notice::new(Severity::Debug, Source::Ghost, "ghost-exit", "t");
        assert!(
            !sink_routed(
                &cfg.notifications,
                &defaults.history,
                crate::config::RouteSink::History,
                &ghost
            ),
            "with the switch off, the defaults route nothing"
        );
    }

    /// Two matching routes with the same command give it once; a route
    /// filtered to `watchdog` stays absent for a scheduler notice.
    #[test]
    fn command_targets_order_and_dedup() {
        let cfg: crate::config::Config = toml::from_str(
            r#"
            [[notifications.route]]
            sink = "command"
            command = "echo alert"

            [[notifications.route]]
            sink = "command"
            command = "echo alert"

            [[notifications.route]]
            sink = "command"
            command = "echo route-b"
            sources = ["watchdog"]
        "#,
        )
        .expect("valid route config");

        let n = Notice::new(Severity::Info, Source::Scheduler, "job", "t");
        let targets = command_targets(&cfg.notifications, &n);
        assert_eq!(
            targets,
            vec!["echo alert".to_string()],
            "the repeated command appears once; the watchdog-filtered third \
             route does not fire for a scheduler notice: {targets:?}"
        );
    }

    /// A configured `command` route contributes its command only when its
    /// filter matches the notice.
    #[test]
    fn command_targets_skip_a_route_whose_filter_does_not_match() {
        let cfg: crate::config::Config = toml::from_str(
            r#"
            [[notifications.route]]
            sink = "command"
            command = "echo watchdog-only"
            sources = ["watchdog"]
        "#,
        )
        .expect("valid route config");

        let watchdog = Notice::new(Severity::Info, Source::Watchdog, "disk", "t");
        assert_eq!(
            command_targets(&cfg.notifications, &watchdog),
            vec!["echo watchdog-only".to_string()],
            "the watchdog notice runs the route's command: {watchdog:?}"
        );

        let scheduler = Notice::new(Severity::Info, Source::Scheduler, "job", "t");
        assert!(
            command_targets(&cfg.notifications, &scheduler).is_empty(),
            "the scheduler notice matches no route: {scheduler:?}"
        );
    }

    #[tokio::test]
    async fn history_sink_broadcast_reaches_every_resident_session() {
        let _guard = crate::test_home();
        let sessions = SessionStore::new();
        insert_resident(&sessions, "a");
        insert_resident(&sessions, "b");

        let notice = Notice::new(Severity::Warning, Source::Webhook, "HighCPU", "CPU at 95%")
            .status(NoticeStatus::Firing);
        let bus = Bus::with_journal(|_| {});
        let routes = default_routes(Severity::Warning);
        register_default_sinks(&bus, sessions.clone(), routes);
        bus.publish(notice.clone());

        for id in ["a", "b"] {
            assert_eq!(
                message_count(&sessions, id),
                1,
                "session {id} gains exactly one message"
            );
        }
        let content = with_sessions(&sessions, |store| {
            store
                .get("a")
                .and_then(|e| e.messages.last().map(|m| m.content.clone()))
        });
        assert_eq!(
            content.as_deref(),
            Some(notice.history_text().as_str()),
            "the message content is the notice's history_text"
        );
    }

    #[tokio::test]
    async fn history_sink_targeted_reaches_only_its_session() {
        let _guard = crate::test_home();
        let sessions = SessionStore::new();
        insert_resident(&sessions, "a");
        insert_resident(&sessions, "b");

        let notice = Notice::new(Severity::Info, Source::Ghost, "note", "ghost note").session("a");
        let bus = Bus::with_journal(|_| {});
        let routes = default_routes(Severity::Warning);
        register_default_sinks(&bus, sessions.clone(), routes);
        bus.publish(notice);

        assert_eq!(
            message_count(&sessions, "a"),
            1,
            "the targeted session gains the message"
        );
        assert_eq!(
            message_count(&sessions, "b"),
            0,
            "the other session does not"
        );
        let file = crate::daemon::session::session_file("a");
        let text = std::fs::read_to_string(&file).expect("session a's JSONL exists");
        assert!(
            text.contains("ghost note"),
            "the JSONL file holds the injected message"
        );
    }

    #[tokio::test]
    async fn history_sink_skips_unrouted_notice() {
        let _guard = crate::test_home();
        let sessions = SessionStore::new();
        insert_resident(&sessions, "a");
        insert_resident(&sessions, "b");

        let notice = webhook(Severity::Info, "LowCPU");
        let bus = Bus::with_journal(|_| {});
        let routes = default_routes(Severity::Warning);
        register_default_sinks(&bus, sessions.clone(), routes);
        bus.publish(notice);

        assert_eq!(
            message_count(&sessions, "a"),
            0,
            "an unrouted webhook info notice leaves session a unchanged"
        );
        assert_eq!(
            message_count(&sessions, "b"),
            0,
            "an unrouted webhook info notice leaves session b unchanged"
        );
    }

    #[test]
    fn command_env_names_the_notice() {
        let n = Notice::new(
            Severity::Error,
            Source::Scheduler,
            "nightly-backup",
            "job failed",
        )
        .body("exit 1");
        let env = command_env(&n);
        assert_eq!(
            env,
            vec![
                ("DAEMONEYE_JOB", "nightly-backup".to_string()),
                ("DAEMONEYE_MSG", "job failed\nexit 1".to_string()),
                ("DAEMONEYE_SEVERITY", "error".to_string()),
                ("DAEMONEYE_SOURCE", "scheduler".to_string()),
            ],
            "the four env pairs carry the notice's identity"
        );
    }

    #[tokio::test]
    async fn command_sink_runs_a_route_command_with_notice_env() {
        let (_guard, home) = crate::test_home();
        let marker = home.path().join("route_cmd_ran");
        let script = format!(
            "echo \"$DAEMONEYE_SEVERITY $DAEMONEYE_SOURCE\" > {}",
            marker.display()
        );
        std::fs::write(
            home.path().join(".daemoneye/etc/config.toml"),
            format!("[[notifications.route]]\nsink = \"command\"\ncommand = {script:?}\n"),
        )
        .expect("the temp HOME's config.toml exists");

        let bus = Bus::with_journal(|_| {});
        let sessions = SessionStore::new();
        register_default_sinks(&bus, sessions, default_routes(Severity::Warning));
        bus.publish(
            Notice::new(Severity::Error, Source::Scheduler, "nightly", "job failed")
                .status(NoticeStatus::Firing),
        );

        let deadline = std::time::Instant::now() + ON_ALERT_POLL;
        let mut contents = String::new();
        while std::time::Instant::now() < deadline {
            if let Ok(text) = std::fs::read_to_string(&marker) {
                contents = text.trim().to_string();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(contents, "error scheduler");
    }

    #[tokio::test]
    async fn alert_registry_sink_records_firing_and_clears_on_resolve() {
        let _guard = crate::test_home();
        let fingerprint = "m38-p03-registry-sink";
        let bus = Bus::with_journal(|_| {});
        let sessions = SessionStore::new();
        register_default_sinks(&bus, sessions, default_routes(Severity::Warning));

        bus.publish(webhook(Severity::Warning, "RegistryHighCPU").fingerprint(fingerprint));
        let snapshot = crate::webhook::active_alerts_snapshot();
        let entry = snapshot
            .iter()
            .find(|a| a.fingerprint == fingerprint)
            .expect("the firing alert appears in the snapshot");
        assert_eq!(entry.name, "RegistryHighCPU");
        assert_eq!(
            entry.severity, "warning",
            "the recorded severity is the raw label"
        );

        bus.publish(
            webhook(Severity::Warning, "RegistryHighCPU")
                .fingerprint(fingerprint)
                .status(NoticeStatus::Resolved),
        );
        let snapshot = crate::webhook::active_alerts_snapshot();
        assert!(
            !snapshot.iter().any(|a| a.fingerprint == fingerprint),
            "the resolved notice removes the entry: {snapshot:?}"
        );
    }
}
