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
    if let Some(ref p) = from_exe
        && p.exists()
    {
        return p.clone();
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
        Request::Ask {
            query,
            tmux_pane,
            session_id,
            chat_pane,
            chat_width,
            tmux_session,
            target_pane,
            ..
        } => {
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
        Request::ToolCallResponse {
            id,
            approved,
            user_message,
        } => {
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
        Response::SessionInfo {
            message_count,
            turn_count,
        } => {
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
    use daemoneye::scheduler::{ActionOn, ScheduleKind, ScheduleStore, ScheduledJob};

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
    unsafe {
        std::env::set_var("HOME", home.to_str().unwrap());
    }
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
    save_session(
        "integ-test-sess",
        None,
        "integration test",
        &messages,
        2,
        "default",
        &[],
        false,
    )
    .unwrap();

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
    unsafe {
        std::env::set_var("HOME", home.to_str().unwrap());
    }
    daemoneye::config::Config::ensure_dirs().unwrap();

    let messages = vec![Message {
        role: "user".into(),
        content: "hello".into(),
        tool_calls: None,
        tool_results: None,
        turn: None,
    }];
    save_session(
        "integ-index-test",
        None,
        "index test",
        &messages,
        1,
        "default",
        &[],
        false,
    )
    .unwrap();

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
    unsafe {
        std::env::set_var("HOME", home.to_str().unwrap());
    }
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
    unsafe {
        std::env::set_var("HOME", home.to_str().unwrap());
    }
    daemoneye::config::Config::ensure_dirs().unwrap();

    log_event(
        "webhook_alert",
        serde_json::json!({ "alert_name": "HighCPU" }),
    );
    log_event("ghost_started", serde_json::json!({ "session_id": "gs-1" }));
    log_event(
        "ghost_completed",
        serde_json::json!({ "session_id": "gs-1" }),
    );

    let path = daemoneye::config::events_path();
    let content = fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.lines().collect();

    // Filter to just our entries (file may have pre-existing entries from other tests).
    let ours: Vec<serde_json::Value> = lines
        .iter()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| {
            matches!(
                v["event"].as_str(),
                Some("webhook_alert") | Some("ghost_started") | Some("ghost_completed")
            )
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
    assert!(cfg["ghost"]["enabled"].as_bool().unwrap());
    assert_eq!(
        cfg["ghost"]["max_concurrent_ghosts"].as_integer().unwrap(),
        3
    );
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

    // Connect, split into reader/writer halves, wrap reader in BufReader
    // for newline-delimited JSON framing.
    let stream = tokio::net::UnixStream::connect(&socket)
        .await
        .expect("connect to socket");
    let (rd, mut wr) = stream.into_split();
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut rd = BufReader::new(rd);
    use tokio::io::AsyncWriteExt;

    // Send Ping.
    let ping = serde_json::to_string(&Request::Ping).unwrap();
    wr.write_all(format!("{}\n", ping).as_bytes())
        .await
        .unwrap();

    let mut line = String::new();
    rd.read_line(&mut line).await.unwrap();
    let resp: Response = serde_json::from_str(line.trim()).unwrap();
    assert!(
        matches!(resp, Response::Ok),
        "expected Response::Ok for Ping, got {:?}",
        resp
    );

    // Send Status.
    let status_req = serde_json::to_string(&Request::Status).unwrap();
    wr.write_all(format!("{}\n", status_req).as_bytes())
        .await
        .unwrap();

    line.clear();
    rd.read_line(&mut line).await.unwrap();
    let resp: Response = serde_json::from_str(line.trim()).unwrap();
    match resp {
        Response::DaemonStatus {
            uptime_secs, pid, ..
        } => {
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
    use daemoneye::webhook::{WebhookState, parse_payload, process_alert};

    // Initialise masking filter (safe — OnceLock, idempotent).
    daemoneye::ai::filter::init_masking(&[]);

    // Isolate HOME so events_path resolves to a temp directory.
    let tmp = tempfile::tempdir().expect("create tempdir");
    {
        let _lock = daemoneye::TEST_HOME_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("HOME", tmp.path().to_str().unwrap());
        }
    }
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
    let schedule_store = std::sync::Arc::new(daemoneye::scheduler::ScheduleStore::new_empty());
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
        serde_json::from_str(lines.last().expect("at least one line")).expect("parse last line");
    assert_eq!(last["event"], "webhook_alert");
    assert_eq!(last["alert_name"], "HighCPU");
    assert_eq!(last["severity"], "critical");
    assert!(last["ts"].is_string());
}

// ── G1 Named Agents ──────────────────────────────────────────────────────────

/// Verify that `merge_runbook_ghost_config` propagates all agent fields into the
/// ghost config and that runbook values take precedence over agent defaults.
#[test]
fn g1_spawn_ghost_shell_with_agent_merge() {
    use daemoneye::agents::{AgentConfig, apply_agent_to_ghost_config};
    use daemoneye::ipc::GhostConfig;

    // --- Case 1: empty GhostConfig gets all agent defaults applied ---
    let agent = AgentConfig {
        name: "analyst".to_string(),
        description: "Test analyst".to_string(),
        prompt: "You are an analyst.".to_string(),
        model: Some("haiku".to_string()),
        memory_namespace: "analyst".to_string(),
        max_turns: Some(8),
        auto_approve_read_only: true,
        auto_approve_scripts: vec!["scan.sh".to_string()],
        read_namespaces: Vec::new(),
        tools: None,
    };
    let mut gc = GhostConfig::default();
    apply_agent_to_ghost_config(&agent, &mut gc);

    assert_eq!(gc.model, Some("haiku".to_string()), "model from agent");
    assert_eq!(gc.max_ghost_turns, 8, "max_ghost_turns from agent");
    assert!(
        gc.auto_approve_commands,
        "auto_approve_commands from agent.auto_approve_read_only"
    );
    assert!(
        gc.auto_approve_scripts.contains(&"scan.sh".to_string()),
        "script from agent"
    );
    assert_eq!(
        gc.agent,
        Some("analyst".to_string()),
        "agent name stamped for audit"
    );

    // --- Case 2: runbook values win over agent defaults ---
    let mut gc2 = GhostConfig {
        model: Some("opus".to_string()),
        max_ghost_turns: 20,
        auto_approve_scripts: vec!["runbook-script.sh".to_string()],
        ..Default::default()
    };
    apply_agent_to_ghost_config(&agent, &mut gc2);

    assert_eq!(
        gc2.model,
        Some("opus".to_string()),
        "runbook model preserved"
    );
    assert_eq!(gc2.max_ghost_turns, 20, "runbook max_ghost_turns preserved");
    // Scripts are unioned, not replaced.
    assert!(
        gc2.auto_approve_scripts
            .contains(&"runbook-script.sh".to_string())
    );
    assert!(gc2.auto_approve_scripts.contains(&"scan.sh".to_string()));
    // No duplicates even if same script appears in both.
    assert_eq!(
        gc2.auto_approve_scripts
            .iter()
            .filter(|s| *s == "scan.sh")
            .count(),
        1,
        "no script duplicates"
    );
}

// ---------------------------------------------------------------------------
// G3 — Tool Policy Enforcement
// ---------------------------------------------------------------------------

/// Verify that an agent with a `deny` tool policy propagates it into the
/// merged `GhostConfig` and that the `permits()` method correctly blocks
/// denied tools while allowing others.
#[test]
fn g3_tool_policy_deny_merged_and_enforced() {
    use daemoneye::agents::{AgentConfig, ToolPolicy, apply_agent_to_ghost_config};
    use daemoneye::ipc::GhostConfig;

    let agent = AgentConfig {
        name: "restricted-agent".to_string(),
        description: "Agent with deny list".to_string(),
        prompt: String::new(),
        model: None,
        memory_namespace: "restricted-agent".to_string(),
        max_turns: None,
        auto_approve_read_only: false,
        auto_approve_scripts: Vec::new(),
        read_namespaces: Vec::new(),
        tools: Some(ToolPolicy {
            allow: None,
            deny: Some(vec!["edit_file".to_string(), "delete_script".to_string()]),
        }),
    };

    let mut gc = GhostConfig::default();
    apply_agent_to_ghost_config(&agent, &mut gc);

    // Tool policy must be present in the merged config.
    let policy = gc.tool_policy.as_ref().expect("tool_policy should be set");

    // Denied tools must be blocked.
    assert!(!policy.permits("edit_file"), "edit_file should be denied");
    assert!(
        !policy.permits("delete_script"),
        "delete_script should be denied"
    );

    // Other tools must be permitted.
    assert!(policy.permits("read_file"), "read_file should be permitted");
    assert!(
        policy.permits("search_repository"),
        "search_repository should be permitted"
    );
    assert!(
        policy.permits("run_terminal_command"),
        "run_terminal_command should be permitted"
    );
}

/// Verify that an agent with an `allow` tool policy only permits listed tools.
#[test]
fn g3_tool_policy_allow_merged_and_enforced() {
    use daemoneye::agents::{AgentConfig, ToolPolicy, apply_agent_to_ghost_config};
    use daemoneye::ipc::GhostConfig;

    let agent = AgentConfig {
        name: "allow-only-agent".to_string(),
        description: "Agent with allow list".to_string(),
        prompt: String::new(),
        model: None,
        memory_namespace: "allow-only-agent".to_string(),
        max_turns: None,
        auto_approve_read_only: false,
        auto_approve_scripts: Vec::new(),
        read_namespaces: Vec::new(),
        tools: Some(ToolPolicy {
            allow: Some(vec![
                "read_file".to_string(),
                "search_repository".to_string(),
            ]),
            deny: None,
        }),
    };

    let mut gc = GhostConfig::default();
    apply_agent_to_ghost_config(&agent, &mut gc);

    let policy = gc.tool_policy.as_ref().expect("tool_policy should be set");

    // Allowed tools must be permitted.
    assert!(policy.permits("read_file"), "read_file should be permitted");
    assert!(
        policy.permits("search_repository"),
        "search_repository should be permitted"
    );

    // Unlisted tools must be denied.
    assert!(!policy.permits("edit_file"), "edit_file should be denied");
    assert!(
        !policy.permits("run_terminal_command"),
        "run_terminal_command should be denied"
    );
}

/// Verify that runbook tool_policy takes precedence over agent tool_policy.
#[test]
fn g3_tool_policy_runbook_precedence_over_agent() {
    use daemoneye::agents::{AgentConfig, ToolPolicy, apply_agent_to_ghost_config};
    use daemoneye::ipc::GhostConfig;

    let agent = AgentConfig {
        name: "agent-with-policy".to_string(),
        description: String::new(),
        prompt: String::new(),
        model: None,
        memory_namespace: "agent-with-policy".to_string(),
        max_turns: None,
        auto_approve_read_only: false,
        auto_approve_scripts: Vec::new(),
        read_namespaces: Vec::new(),
        tools: Some(ToolPolicy {
            allow: Some(vec!["read_file".to_string()]),
            deny: None,
        }),
    };

    // Runbook sets its own tool policy.
    let mut gc = GhostConfig {
        tool_policy: Some(ToolPolicy {
            allow: Some(vec![
                "read_file".to_string(),
                "search_repository".to_string(),
            ]),
            deny: None,
        }),
        ..Default::default()
    };
    apply_agent_to_ghost_config(&agent, &mut gc);

    // Runbook policy must be preserved (not overwritten by agent).
    let policy = gc.tool_policy.as_ref().expect("tool_policy should be set");
    assert!(
        policy.permits("search_repository"),
        "runbook policy should allow search_repository"
    );
}

// ---------------------------------------------------------------------------
// G4 — Persistent Briefing State
// ---------------------------------------------------------------------------

/// Verify that when a briefing file exists for an agent, the briefing helpers
/// can read and clear it correctly.
#[test]
fn g4_briefing_read_and_clear() {
    use daemoneye::agents;
    use daemoneye::daemon::briefing;

    let tmp = temp_daemoneye_home();
    let old_home = std::env::var("HOME").ok();
    unsafe {
        std::env::set_var("HOME", &tmp);
    }

    // Write a briefing file manually.
    let path = agents::briefing_path("test-agent");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "Key findings: disk full on /dev/sda1\nActions: cleared logs",
    )
    .unwrap();

    // Read it back.
    let content = briefing::read_briefing("test-agent").expect("briefing should exist");
    assert!(content.contains("disk full"));
    assert!(content.contains("cleared logs"));

    // Clear it.
    briefing::clear_briefing("test-agent");
    assert!(
        briefing::read_briefing("test-agent").is_none(),
        "briefing should be gone after clear"
    );

    // Clearing again is a no-op.
    briefing::clear_briefing("test-agent");

    match old_home {
        Some(v) => unsafe { std::env::set_var("HOME", v) },
        None => unsafe { std::env::remove_var("HOME") },
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Verify that the `format_tool_restriction_block` function produces the
/// correct output for both allow and deny modes (G3, but tested here alongside G4).
#[test]
fn g4_briefing_injection_block_format() {
    use daemoneye::agents::ToolPolicy;
    use daemoneye::agents::policy::format_tool_restriction_block;

    // Allow mode
    let allow_policy = ToolPolicy {
        allow: Some(vec!["read_file".to_string()]),
        deny: None,
    };
    let block = format_tool_restriction_block(&allow_policy).unwrap();
    assert!(block.contains("## Tool Restrictions"));
    assert!(block.contains("available: read_file"));

    // Deny mode
    let deny_policy = ToolPolicy {
        allow: None,
        deny: Some(vec!["edit_file".to_string()]),
    };
    let block = format_tool_restriction_block(&deny_policy).unwrap();
    assert!(block.contains("## Tool Restrictions"));
    assert!(block.contains("NOT available"));
    assert!(block.contains("edit_file"));

    // Unrestricted
    assert!(format_tool_restriction_block(&ToolPolicy::default()).is_none());
}
