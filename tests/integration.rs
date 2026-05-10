//! Integration tests for DaemonEye.
//!
//! Exercises the persistence layer, IPC protocol, daemon lifecycle, and
//! webhook pipeline.  These verify that the data paths (schedules, sessions,
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

/// Locate the daemoneye binary for process-spawn tests.
fn find_daemoneye_binary() -> PathBuf {
    // 1. Parent of the test executable (target/debug/daemoneye).
    let from_exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .map(|p| p.join("daemoneye"));
    if let Some(ref p) = from_exe {
        if p.exists() {
            return p.clone();
        }
    }
    // 2. Installed binary in ~/.daemoneye/bin/.
    let installed = std::env::var("HOME")
        .ok()
        .map(|h| format!("{h}/.daemoneye/bin/daemoneye"));
    if let Some(ref p) = installed {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return pb;
        }
    }
    // 3. On PATH.
    if let Ok(output) = std::process::Command::new("which")
        .arg("daemoneye")
        .output()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    panic!(
        "Cannot find daemoneye binary. Searched: {:?}, {:?}, and PATH.",
        from_exe, installed
    );
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

// ---------------------------------------------------------------------------
// A4 — Daemon process lifecycle
// ---------------------------------------------------------------------------

/// Spawn a real daemon process and verify Ping → Ok and Status → DaemonStatus
/// round-trips over the Unix domain socket.
///
/// This test exercises socket binding, IPC framing, and the daemon's request
/// dispatch loop — regressions that unit tests and persistence-only integration
/// tests cannot catch.
///
/// Marked `#[ignore]` because it requires tmux + a valid API key in the test
/// environment. Run with `cargo test --test integration -- --ignored` locally.
#[ignore]
#[tokio::test(flavor = "current_thread")]
async fn daemon_ping_status_loop() {
    // Precondition: tmux server must be running and binary discoverable.
    let tmux_ok = std::process::Command::new("tmux")
        .arg("list-sessions")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !tmux_ok {
        println!("Skipping daemon_ping_status_loop: tmux not available");
        return;
    }

    let binary = find_daemoneye_binary();
    let tmp = tempfile::tempdir().expect("create tempdir");
    // The daemon resolves the socket as ~/.daemoneye/var/run/daemoneye.sock from $HOME.
    let socket = tmp.path().join(".daemoneye/var/run/daemoneye.sock");
    let home_str = tmp.path().to_string_lossy().to_string();
    // The daemon resolves ~/.daemoneye/etc/config.toml from $HOME.
    let de_dir = tmp.path().join(".daemoneye");
    let etc_dir = de_dir.join("etc");
    fs::create_dir_all(&etc_dir).expect("create etc dir");
    let config_path = etc_dir.join("config.toml");
    fs::write(
        &config_path,
        r#"[models.default]
provider = "ollama"
model = "test"
"#,
    )
    .expect("write config");

    // Spawn daemon in --console mode (no fork, no background).
    let mut child = std::process::Command::new(&binary)
        .args(["daemon", "--console"])
        .env("HOME", &home_str)
        .env("DAEMONEYE_LOG", "error")
        .env("TMUX", "") // unset — daemon will create its own session
        .spawn()
        .expect("spawn daemon");

    // Wait for socket to appear.
    let mut waited = 0u64;
    while !socket.exists() {
        if waited >= 150 {
            let output = child.wait_with_output().ok();
            let stdout = output
                .as_ref()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_default();
            let stderr = output
                .as_ref()
                .map(|o| String::from_utf8_lossy(&o.stderr).to_string())
                .unwrap_or_default();
            panic!(
                "daemon socket did not appear in time. stdout: {} stderr: {}",
                stdout, stderr
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        waited += 1;
    }

    // Connect and send Ping.
    let mut stream = tokio::net::UnixStream::connect(&socket)
        .await
        .expect("connect to socket");

    let ping = serde_json::to_string(&Request::Ping).unwrap();
    use tokio::io::AsyncWriteExt;
    stream
        .write_all(format!("{}\n", ping).as_bytes())
        .await
        .unwrap();

    let mut buf = Vec::new();
    use tokio::io::AsyncReadExt;
    stream.read(&mut buf).await.unwrap();
    let resp: Response =
        serde_json::from_str(std::str::from_utf8(&buf).unwrap().trim())
            .unwrap();
    assert!(
        matches!(resp, Response::Ok),
        "expected Response::Ok for Ping, got {:?}",
        resp
    );

    // Send Status.
    buf.clear();
    let status_req = serde_json::to_string(&Request::Status).unwrap();
    stream
        .write_all(format!("{}\n", status_req).as_bytes())
        .await
        .unwrap();

    stream.read(&mut buf).await.unwrap();

    let resp: Response =
        serde_json::from_str(std::str::from_utf8(&buf).unwrap().trim())
            .unwrap();
    match resp {
        Response::DaemonStatus { uptime_secs, pid, .. } => {
            assert!(pid > 0);
            let _ = uptime_secs; // present and non-negative by type (u64)
        }
        _ => panic!("expected DaemonStatus, got {:?}", resp),
    }

    // Cleanup: kill daemon (it will clean up its own socket).
    let _ = child.kill();
    let _ = child.wait();
}

// ---------------------------------------------------------------------------
// A5 — Webhook alert → event-log pipeline
// ---------------------------------------------------------------------------

/// Parse a synthetic Alertmanager payload through the production webhook
/// pipeline (parse → dedup → mask → log) and assert that events.jsonl
/// receives a valid entry.
///
/// This exercises the same code path as the HTTP webhook handler without
/// needing to bind a TCP port or start the full daemon.
#[tokio::test(flavor = "current_thread")]
async fn webhook_alert_to_event_log() {
    use daemoneye::webhook::{parse_payload, process_alert, WebhookState};

    // Initialise masking filter (safe — OnceLock, idempotent).
    daemoneye::ai::filter::init_masking(&[]);

    // Isolate HOME so events_path resolves to a temp directory.
    let tmp = tempfile::tempdir().expect("create tempdir");
    let _lock = daemoneye::TEST_HOME_LOCK.lock().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path().to_str().unwrap()); }
    daemoneye::config::Config::ensure_dirs().expect("ensure dirs");

    // Parse a synthetic Alertmanager payload.
    let body = serde_json::json!({
        "status": "firing",
        "alerts": [{
            "status": "firing",
            "labels": {
                "alertname": "HighCPU",
                "severity": "critical",
                "instance": "web-01"
            },
            "annotations": {
                "summary": "CPU usage above 90%"
            },
            "fingerprint": "test-fp-001"
        }]
    });
    let alerts = parse_payload(&body);
    assert_eq!(alerts.len(), 1);
    let alert = &alerts[0];
    assert_eq!(alert.alert_name, "HighCPU");
    assert_eq!(alert.severity, "critical");
    assert_eq!(alert.source, "alertmanager");

    // Build minimal WebhookState — only the config and dedup/rate-limit
    // maps are exercised by this path; sessions/cache/schedule_store are
    // default since we only care about the log_event side effect.
    let config = daemoneye::config::Config::default();
    let sessions = daemoneye::daemon::session::SessionStore::default();
    let cache = std::sync::Arc::new(daemoneye::daemon::SessionCache::new("test"));
    let schedule_store =
        std::sync::Arc::new(daemoneye::scheduler::ScheduleStore::new_empty());
    let state = std::sync::Arc::new(WebhookState {
        config,
        sessions,
        cache,
        schedule_store,
        dedup: std::sync::Mutex::new(std::collections::HashMap::new()),
        rate_limit: std::sync::Mutex::new(std::collections::HashMap::new()),
    });

    // Process the alert through the pipeline.
    process_alert(alert.clone(), state).await;

    // Assert events.jsonl contains the webhook_alert entry.
    let path = daemoneye::config::events_path();
    let content = fs::read_to_string(&path).expect("read events.jsonl");
    let lines: Vec<&str> = content.lines().collect();
    let last: serde_json::Value =
        serde_json::from_str(lines.last().expect("at least one line"))
            .expect("parse last line");
    assert_eq!(last["event"], "webhook_alert");
    assert_eq!(last["alert_name"], "HighCPU");
    assert_eq!(last["severity"], "critical");
    assert!(last["ts"].is_string());
}