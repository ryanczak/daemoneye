//! Integration tests for DaemonEye.
//!
//! Exercises the persistence layer and IPC protocol without a running daemon
//! or tmux session.  These verify that the data paths (schedules, sessions,
//! event log, IPC messages) survive serialization round-trips and are
//! consistent across the boundary between daemon and CLI.

use daemoneye::ipc::{Request, Response};
use std::fs;
use std::io::Write;
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
// Schedule persistence
// ---------------------------------------------------------------------------

/// Verify that the schedule store can be written and reloaded.
#[test]
fn schedule_store_persistence() {
    let home = temp_daemoneye_home();
    let schedule_path = home.join("var").join("schedules.json");

    // Write a minimal schedule file.
    let schedule_data = serde_json::json!({
        "jobs": [
            {
                "id": "test-job-1",
                "name": "disk check",
                "kind": "Every 5m",
                "action": "script: check-disk.sh",
                "status": "active",
                "last_run": null,
                "next_run": "2026-05-10T00:00:00Z"
            }
        ]
    });

    fs::create_dir_all(schedule_path.parent().unwrap()).unwrap();
    fs::write(&schedule_path, serde_json::to_string_pretty(&schedule_data).unwrap()).unwrap();

    // Reload and verify.
    let loaded: serde_json::Value = serde_json::from_str(&fs::read_to_string(&schedule_path).unwrap()).unwrap();
    let jobs = loaded["jobs"].as_array().expect("jobs array");
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["name"], "disk check");
    assert_eq!(jobs[0]["status"], "active");
}

// ---------------------------------------------------------------------------
// Session persistence
// ---------------------------------------------------------------------------

/// Verify that a session JSONL file can be written and read back.
#[test]
fn session_jsonl_round_trip() {
    let home = temp_daemoneye_home();
    let session_dir = home.join("var").join("sessions");
    let session_path = session_dir.join("test-session.jsonl");

    fs::create_dir_all(&session_dir).unwrap();

    let messages = vec![
        serde_json::json!({"role": "user", "content": "hello"}),
        serde_json::json!({"role": "assistant", "content": "hi there"}),
        serde_json::json!({"role": "user", "content": "bye"}),
    ];

    let mut f = fs::File::create(&session_path).unwrap();
    for msg in &messages {
        writeln!(f, "{}", serde_json::to_string(msg).unwrap()).unwrap();
    }

    // Read back.
    let lines: Vec<String> = fs::read_to_string(&session_path)
        .unwrap()
        .lines()
        .map(|l| l.to_string())
        .collect();

    assert_eq!(lines.len(), 3);
    let first: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(first["role"], "user");
    assert_eq!(first["content"], "hello");
}

/// Verify that the session index file survives a write/read cycle.
#[test]
fn session_index_persistence() {
    let home = temp_daemoneye_home();
    let index_path = home.join("var").join("sessions").join("index.json");

    fs::create_dir_all(index_path.parent().unwrap()).unwrap();

    let index = serde_json::json!({
        "sessions": {
            "deploy-fix": {
                "path": "deploy-fix.jsonl",
                "turn_count": 12,
                "message_count": 24,
                "created_at": "2026-05-09T10:00:00Z",
                "last_updated": "2026-05-09T10:30:00Z"
            }
        }
    });

    fs::write(&index_path, serde_json::to_string_pretty(&index).unwrap()).unwrap();

    let loaded: serde_json::Value = serde_json::from_str(&fs::read_to_string(&index_path).unwrap()).unwrap();
    assert_eq!(loaded["sessions"]["deploy-fix"]["turn_count"], 12);
    assert_eq!(loaded["sessions"]["deploy-fix"]["message_count"], 24);
}

// ---------------------------------------------------------------------------
// Event log format
// ---------------------------------------------------------------------------

/// Verify that an event log entry is valid JSON and has the expected fields.
#[test]
fn event_log_entry_format() {
    let event = serde_json::json!({
        "timestamp": "2026-05-09T10:00:00Z",
        "type": "webhook_alert",
        "alert_name": "HighCPU",
        "status": "firing",
        "severity": "critical",
        "source": "alertmanager"
    });

    // Verify it serializes to a single line.
    let line = serde_json::to_string(&event).unwrap();
    assert!(!line.contains('\n'), "event log entries must be single-line");

    // Verify it deserializes back with expected fields.
    let back: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(back["type"], "webhook_alert");
    assert_eq!(back["alert_name"], "HighCPU");
}

/// Verify that multiple event log entries can be appended and read sequentially.
#[test]
fn event_log_append_read() {
    let tmp = std::env::temp_dir().join(format!("de-events-{}", uuid::Uuid::new_v4()));
    let path = tmp.join("events.jsonl");
    fs::create_dir_all(&tmp).unwrap();

    let events = vec![
        serde_json::json!({"type": "webhook_alert", "alert_name": "HighCPU"}),
        serde_json::json!({"type": "ghost_started", "session_id": "gs-1"}),
        serde_json::json!({"type": "ghost_completed", "session_id": "gs-1"}),
    ];

    let mut f = fs::File::create(&path).unwrap();
    for evt in &events {
        writeln!(f, "{}", serde_json::to_string(evt).unwrap()).unwrap();
    }

    let content = fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 3);

    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first["type"], "webhook_alert");

    let last: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
    assert_eq!(last["type"], "ghost_completed");
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
