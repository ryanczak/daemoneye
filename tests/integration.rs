//! Integration tests for DaemonEye.
//!
//! Exercises the persistence layer and IPC protocol without a running daemon
//! or tmux session.  These verify that the data paths (schedules, sessions,
//! event log, IPC messages) survive serialization round-trips and are
//! consistent across the boundary between daemon and CLI.

use daemoneye::ipc::{Request, Response};
use std::fs;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a temp dir scoped to one test run.
fn temp_daemoneye_home() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("daemoneye-integ-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

// ---------------------------------------------------------------------------
// IPC protocol round-trip
// ---------------------------------------------------------------------------

/// Verify that an Ask request survives JSON serialization/deserialization
/// using the production `Request` type from `daemoneye::ipc`.
#[test]
fn ipc_ask_round_trip() {
    let req = Request::Ask {
        query: "check disk usage".to_string(),
        tmux_pane: Some("%3".to_string()),
        session_id: Some("abc123".to_string()),
        chat_pane: Some("%2".to_string()),
        prompt: None,
        chat_width: Some(120),
        tmux_session: Some("de-chat".to_string()),
        target_pane: Some("%4".to_string()),
        model: None,
    };

    let json = serde_json::to_string(&req).expect("serialize Ask");
    let back: Request = serde_json::from_str(&json).expect("deserialize Ask");

    match back {
        Request::Ask { query, tmux_pane, session_id, chat_pane, chat_width, tmux_session, target_pane, .. } => {
            assert_eq!(query, "check disk usage");
            assert_eq!(tmux_pane.as_deref(), Some("%3"));
            assert_eq!(session_id.as_deref(), Some("abc123"));
            assert_eq!(chat_pane.as_deref(), Some("%2"));
            assert_eq!(chat_width, Some(120));
            assert_eq!(tmux_session.as_deref(), Some("de-chat"));
            assert_eq!(target_pane.as_deref(), Some("%4"));
        }
        _ => panic!("expected Ask variant"),
    }
}

/// Verify that a ToolCallResponse survives round-trip using the production `Request` type.
#[test]
fn ipc_tool_call_response_round_trip() {
    let req = Request::ToolCallResponse {
        id: "tool-1".to_string(),
        approved: true,
        user_message: None,
    };

    let json = serde_json::to_string(&req).expect("serialize");
    let back: Request = serde_json::from_str(&json).expect("deserialize");

    match back {
        Request::ToolCallResponse { id, approved, user_message } => {
            assert_eq!(id, "tool-1");
            assert!(approved);
            assert!(user_message.is_none());
        }
        _ => panic!("expected ToolCallResponse variant"),
    }
}

/// Verify that a SessionInfo response survives round-trip using the production `Response` type.
#[test]
fn ipc_session_info_round_trip() {
    let resp = Response::SessionInfo {
        message_count: 10,
        turn_count: 5,
    };

    let json = serde_json::to_string(&resp).expect("serialize");
    let back: Response = serde_json::from_str(&json).expect("deserialize");

    match back {
        Response::SessionInfo { message_count, turn_count } => {
            assert_eq!(message_count, 10);
            assert_eq!(turn_count, 5);
        }
        _ => panic!("expected SessionInfo variant"),
    }
}

// ---------------------------------------------------------------------------
// Schedule persistence via ScheduleStore
// ---------------------------------------------------------------------------

/// Verify that ScheduleStore::add() persists atomically and
/// ScheduleStore::load_or_create() reloads the jobs with correct fields.
/// Exercises the production save/load path.
#[test]
fn schedule_store_persistence() {
    use daemoneye::scheduler::{ActionOn, ScheduleKind, ScheduledJob, ScheduleStore};

    let home = temp_daemoneye_home();
    let schedule_path = home.join("var").join("schedules.json");
    fs::create_dir_all(schedule_path.parent().unwrap()).unwrap();

    let store = ScheduleStore::load_or_create(schedule_path.clone()).unwrap();
    let job = ScheduledJob::new(
        "disk check".into(),
        ScheduleKind::Every {
            interval_secs: 300,
            next_run: chrono::Utc::now(),
        },
        ActionOn::Script("check-disk.sh".into()),
        None,
    );
    store.add(job).unwrap();

    // Reload from disk and assert the persisted fields.
    let store2 = ScheduleStore::load_or_create(schedule_path).unwrap();
    let jobs = store2.list();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].name, "disk check");
    assert!(jobs[0].id.len() >= 8);
}

// ---------------------------------------------------------------------------
// Session persistence via session_store
// ---------------------------------------------------------------------------

/// Verify that save_session() writes meta.toml + messages.jsonl and that
/// load_session_messages() reads them back correctly.
#[test]
fn session_jsonl_round_trip() {
    use daemoneye::ai::Message;
    use daemoneye::session_store::{load_session_messages, save_session};

    let home = temp_daemoneye_home();
    let _lock = daemoneye::TEST_HOME_LOCK.lock().unwrap();
    unsafe { std::env::set_var("HOME", home.to_str().unwrap()); }
    daemoneye::config::Config::ensure_dirs().unwrap();

    let messages = vec![
        Message {
            role: "user".into(),
            content: "hello".into(),
            tool_calls: None,
            tool_results: None,
            turn: None,
        },
        Message {
            role: "assistant".into(),
            content: "hi there".into(),
            tool_calls: None,
            tool_results: None,
            turn: None,
        },
        Message {
            role: "user".into(),
            content: "bye".into(),
            tool_calls: None,
            tool_results: None,
            turn: None,
        },
    ];
    save_session("integ-test-sess", None, "integration test", &messages, 2, "default", &[], false).unwrap();

    let loaded = load_session_messages("integ-test-sess", 0).unwrap();
    assert_eq!(loaded.len(), 3);
    assert_eq!(loaded[0].role, "user");
    assert_eq!(loaded[0].content, "hello");
    assert_eq!(loaded[2].content, "bye");
}

/// Verify that save_session() creates an index entry visible to list_sessions().
#[test]
fn session_index_persistence() {
    use daemoneye::ai::Message;
    use daemoneye::session_store::{list_sessions, save_session};

    let home = temp_daemoneye_home();
    let _lock = daemoneye::TEST_HOME_LOCK.lock().unwrap();
    unsafe { std::env::set_var("HOME", home.to_str().unwrap()); }
    daemoneye::config::Config::ensure_dirs().unwrap();

    let messages = vec![Message {
        role: "user".into(),
        content: "hello".into(),
        tool_calls: None,
        tool_results: None,
        turn: None,
    }];
    save_session("integ-index-test", None, "index test", &messages, 1, "default", &[], false).unwrap();

    let sessions = list_sessions();
    assert!(sessions.iter().any(|(name, _)| name == "integ-index-test"));
}

// ---------------------------------------------------------------------------
// Event log via log_event
// ---------------------------------------------------------------------------

/// Verify that log_event() writes a valid JSONL entry with ts, event, and
/// caller-provided fields.
#[test]
fn event_log_entry_format() {
    use daemoneye::daemon::utils::log_event;

    let home = temp_daemoneye_home();
    let _lock = daemoneye::TEST_HOME_LOCK.lock().unwrap();
    unsafe { std::env::set_var("HOME", home.to_str().unwrap()); }
    daemoneye::config::Config::ensure_dirs().unwrap();

    let fields = serde_json::json!({
        "alert_name": "HighCPU",
        "severity": "critical"
    });
    log_event("webhook_alert", fields);

    let path = daemoneye::config::events_path();
    let content = fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    let last: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    assert_eq!(last["event"], "webhook_alert");
    assert_eq!(last["alert_name"], "HighCPU");
    assert!(last["ts"].is_string());
}

/// Verify that multiple log_event() calls append correctly and are readable
/// in order.
#[test]
fn event_log_append_read() {
    use daemoneye::daemon::utils::log_event;

    let home = temp_daemoneye_home();
    let _lock = daemoneye::TEST_HOME_LOCK.lock().unwrap();
    unsafe { std::env::set_var("HOME", home.to_str().unwrap()); }
    daemoneye::config::Config::ensure_dirs().unwrap();

    log_event("webhook_alert", serde_json::json!({ "alert_name": "HighCPU" }));
    log_event("ghost_started", serde_json::json!({ "session_id": "gs-1" }));
    log_event("ghost_completed", serde_json::json!({ "session_id": "gs-1" }));

    let path = daemoneye::config::events_path();
    let content = fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.lines().collect();

    // Filter to just our entries (file may have pre-existing entries from other tests).
    let ours: Vec<serde_json::Value> = lines
        .iter()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| {
            matches!(v["event"].as_str(),
                Some("webhook_alert") | Some("ghost_started") | Some("ghost_completed"))
        })
        .collect();
    assert_eq!(ours.len(), 3);
    assert_eq!(ours[0]["event"], "webhook_alert");
    assert_eq!(ours[2]["event"], "ghost_completed");
}

// ---------------------------------------------------------------------------
// Config validation
// ---------------------------------------------------------------------------

/// Verify that a minimal config.toml can be parsed.
#[test]
fn minimal_config_parsing() {
    let toml_str = r#"
[ai]
provider = "anthropic"
model = "claude-opus-4-6"
"#;

    let cfg: toml::Value = toml::from_str(toml_str).expect("parse minimal config");
    assert_eq!(cfg["ai"]["provider"].as_str().unwrap(), "anthropic");
    assert_eq!(cfg["ai"]["model"].as_str().unwrap(), "claude-opus-4-6");
}

/// Verify that ghost config section is parsed correctly.
#[test]
fn ghost_config_parsing() {
    let toml_str = r#"
[ghost]
enabled = true
max_concurrent_ghosts = 3
auto_approve_scripts = ["check-disk.sh"]
auto_approve_commands = true
"#;

    let cfg: toml::Value = toml::from_str(toml_str).expect("parse ghost config");
    assert_eq!(cfg["ghost"]["enabled"].as_bool().unwrap(), true);
    assert_eq!(cfg["ghost"]["max_concurrent_ghosts"].as_integer().unwrap(), 3);
    let scripts: Vec<&str> = cfg["ghost"]["auto_approve_scripts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(scripts, vec!["check-disk.sh"]);
}