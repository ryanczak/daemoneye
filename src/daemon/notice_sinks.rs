//! The notice bus's three consumers (M38 phase 02): the chat-history
//! sink (inline), the chat-notice sink (queued) and the `on_alert` command
//! sink (queued). [`register_default_sinks`] subscribes all three on the
//! global bus at daemon startup, with [`default_routes`] reproducing what
//! 2.9.0 delivers. Nothing publishes until phases 03–05 wire the producers.

use crate::ai::Message;
use crate::bus::{Bus, Filter, Notice, Severity, Source};
use crate::daemon::session::{SessionStore, append_session_message, with_sessions};

/// How long the command-sink test polls the file the `on_alert` command
/// writes before giving up.
#[cfg(test)]
const ON_ALERT_POLL: std::time::Duration = std::time::Duration::from_secs(5);

/// Per-sink route lists. A notice reaches a sink when **any** of the sink's
/// filters matches.
#[derive(Debug, Clone, Default)]
pub struct SinkRoutes {
    pub history: Vec<Filter>,
    pub chat_notice: Vec<Filter>,
    pub command: Vec<Filter>,
}

/// Whether `n` is routed to a sink whose routes are `routes`: any matching
/// filter routes it; an empty list matches nothing.
pub fn routes_match(routes: &[Filter], n: &Notice) -> bool {
    routes.iter().any(|f| f.matches(n))
}

/// The threshold a webhook notice must rank at or above to be routed.
/// `Severity::parse` falls back to [`Severity::Warning`] — the same
/// fail-closed default the webhook gate applies to an unrankable
/// `severity_threshold`.
pub fn threshold_from_config(threshold: &str) -> Severity {
    Severity::parse(threshold).unwrap_or(Severity::Warning)
}

/// The routes that reproduce what 2.9.0 delivers, given the webhook
/// severity threshold:
///
/// - history: webhook ≥ threshold; ghost (any); background (any).
/// - chat_notice: webhook ≥ threshold; ghost; scheduler; watchdog.
/// - command: webhook ≥ threshold; scheduler; watchdog.
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
        chat_notice: vec![
            webhook_at_or_above.clone(),
            ghost_any,
            scheduler_any.clone(),
            watchdog_any.clone(),
        ],
        command: vec![webhook_at_or_above, scheduler_any, watchdog_any],
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

/// The `on_alert` command's environment, in pair form so the test asserts
/// the names and values without spawning anything.
pub fn command_env(notice: &Notice) -> Vec<(&'static str, String)> {
    vec![
        ("DAEMONEYE_JOB", notice.name.clone()),
        ("DAEMONEYE_MSG", notice.history_text()),
        ("DAEMONEYE_SEVERITY", notice.severity.as_str().to_string()),
        ("DAEMONEYE_SOURCE", notice.source.as_str().to_string()),
    ]
}

/// The command sink's queued delivery: read `[notifications] on_alert`
/// through the config handle (so a reload applies), do nothing when it is
/// empty, and otherwise spawn `sh -c <cmd>` with the notice's environment —
/// the same fire-and-forget `fire_notification` does.
async fn deliver_command(notice: Notice) {
    let cmd = crate::config::current().notifications.on_alert.clone();
    if cmd.is_empty() {
        return;
    }
    let mut command = tokio::process::Command::new("sh");
    command.arg("-c").arg(cmd);
    for (key, value) in command_env(&notice) {
        command.env(key, value);
    }
    let _ = command.spawn();
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

/// Subscribe the three sinks on `bus` and spawn the two queued tasks.
///
/// Each queued sink gets capacity 256 and its subscriber name
/// (`history`, `chat_notice`, `command`). Called once by `run_daemon`, on
/// [`crate::bus::global`], right after the `SessionStore` is created.
pub fn register_default_sinks(bus: &Bus, sessions: SessionStore, routes: SinkRoutes) {
    let sessions_history = sessions.clone();
    bus.subscribe_inline("history", Filter::default(), move |n| {
        if routes_match(&routes.history, n) {
            deliver_history(&sessions_history, n);
        }
    });

    let chat_rx = bus.subscribe("chat_notice", Filter::default(), 256);
    let chat_sessions = sessions.clone();
    let chat_routes = routes.chat_notice.clone();
    tokio::spawn(async move {
        let mut rx = chat_rx;
        while let Some(notice) = rx.recv().await {
            if routes_match(&chat_routes, &notice) {
                let line = notice.line().to_string();
                deliver_chat_notice(chat_sessions.clone(), line).await;
            }
        }
    });
    let cmd_rx = bus.subscribe("command", Filter::default(), 256);
    let cmd_routes = routes.command.clone();
    tokio::spawn(async move {
        let mut rx = cmd_rx;
        while let Some(notice) = rx.recv().await {
            if routes_match(&cmd_routes, &notice) {
                deliver_command(notice.as_ref().clone()).await;
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
        Notice::new(severity, Source::Webhook, name, "title").status(NoticeStatus::Firing)
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
                && !routes_match(&routes.chat_notice, &web_info)
                && !routes_match(&routes.command, &web_info),
            "webhook info is below the warning threshold: no sink"
        );

        let web_warning = webhook(Severity::Warning, "HighCPU");
        assert!(
            routes_match(&routes.history, &web_warning)
                && routes_match(&routes.chat_notice, &web_warning)
                && routes_match(&routes.command, &web_warning),
            "webhook warning reaches all three sinks"
        );

        let ghost = Notice::new(Severity::Debug, Source::Ghost, "ghost-exit", "t");
        assert!(
            routes_match(&routes.history, &ghost)
                && routes_match(&routes.chat_notice, &ghost)
                && !routes_match(&routes.command, &ghost),
            "ghost (any severity) reaches history and chat_notice, not command"
        );

        let background = Notice::new(Severity::Info, Source::Background, "bg-done", "t");
        assert!(
            routes_match(&routes.history, &background)
                && !routes_match(&routes.chat_notice, &background)
                && !routes_match(&routes.command, &background),
            "background reaches history only"
        );

        for source in [Source::Scheduler, Source::Watchdog] {
            let n = Notice::new(Severity::Info, source, "job", "t");
            assert!(
                !routes_match(&routes.history, &n)
                    && routes_match(&routes.chat_notice, &n)
                    && routes_match(&routes.command, &n),
                "{source:?} reaches chat_notice and command, not history"
            );
        }
    }

    #[test]
    fn threshold_from_config_falls_back_to_warning() {
        assert_eq!(threshold_from_config("critical"), Severity::Critical);
        assert_eq!(threshold_from_config("bogus"), Severity::Warning);
        assert_eq!(threshold_from_config(""), Severity::Warning);
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
    async fn command_sink_runs_on_alert_with_notice_env() {
        let (_guard, home) = crate::test_home();
        let marker = home.path().join("on_alert_ran");
        let script = format!(
            "echo \"$DAEMONEYE_SEVERITY $DAEMONEYE_SOURCE\" > {}",
            marker.display()
        );
        std::fs::write(
            home.path().join(".daemoneye/etc/config.toml"),
            format!("[notifications]\non_alert = {script:?}\n"),
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
}
