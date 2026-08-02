//! End-to-end isolation tests.
//!
//! Verifies that the test harness runs a real `daemoneye` daemon against a
//! throwaway `$HOME` and a private tmux server, touching neither the
//! operator's `~/.daemoneye/` nor their default tmux server.

mod harness;

use harness::IsolatedEnv;

/// Check whether tmux is available on this host.
fn tmux_available() -> bool {
    std::process::Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Snapshot the default tmux server state.
///
/// Returns the combined stdout, stderr, and exit status of `list-sessions`
/// and `show-hooks -g`. Captures stderr and status, not just stdout: when no
/// default server is running, tmux writes to stderr and exits non-zero, and
/// that is a perfectly valid snapshot to compare.
fn snapshot_default_server(env: &IsolatedEnv) -> String {
    let ls_out = env
        .default_tmux(&["list-sessions", "-F", "#S"])
        .output()
        .expect("run list-sessions");
    let hooks_out = env
        .default_tmux(&["show-hooks", "-g"])
        .output()
        .expect("run show-hooks -g");

    format!(
        "{}:{}:{}:{}:{}",
        String::from_utf8_lossy(&ls_out.stdout),
        String::from_utf8_lossy(&ls_out.stderr),
        ls_out.status,
        String::from_utf8_lossy(&hooks_out.stdout),
        String::from_utf8_lossy(&hooks_out.stderr)
    )
}

/// The daemon boots entirely inside the throwaway root.
///
/// After `start_daemon`, the socket and PID file exist under
/// `<root>/.daemoneye/var/run/`, and `daemoneye ping` through the harness succeeds.
#[test]
fn daemon_boots_in_throwaway_root() {
    if !tmux_available() {
        eprintln!("SKIP: tmux not available");
        return;
    }

    let mut env = IsolatedEnv::new();
    env.start_daemon("de-test-boot");

    // Verify socket and PID file exist under the throwaway root.
    let run_dir = env.root().join(".daemoneye/var/run");
    assert!(
        run_dir.join("daemoneye.sock").exists(),
        "socket not found at {}",
        run_dir.join("daemoneye.sock").display()
    );
    assert!(
        run_dir.join("daemoneye.pid").exists(),
        "PID file not found at {}",
        run_dir.join("daemoneye.pid").display()
    );

    // Verify ping succeeds through the isolated socket.
    let ping_out = env
        .daemoneye(&["ping"])
        .output()
        .expect("run daemoneye ping");
    assert!(
        ping_out.status.success(),
        "daemoneye ping failed: {}",
        String::from_utf8_lossy(&ping_out.stderr)
    );
}

/// The daemon's global hooks land on the private server.
///
/// After `start_daemon`, the private server carries hooks whose **values**
/// invoke `daemoneye notify`. This is the positive half — proving the daemon
/// really did reach a tmux server, so that the negative half below is not
/// passing vacuously.
///
/// We assert on the hook's value (not just its name) because `show-hooks -g
/// <name>` echoes the name even when the hook is unset. A set hook renders as
/// `pane-died[0] run-shell -b '.../daemoneye notify activity ...'`.
///
/// We also check `client-attached`, which appears in the general `show-hooks -g`
/// listing, to verify via a second hook that does appear in the full listing.
#[test]
fn hooks_land_on_private_server() {
    if !tmux_available() {
        eprintln!("SKIP: tmux not available");
        return;
    }

    let mut env = IsolatedEnv::new();
    env.start_daemon("de-test-hooks");

    // Check pane-died hook value on the private server (while daemon is live).
    let pane_died_out = env
        .tmux(&["show-hooks", "-g", "pane-died"])
        .output()
        .expect("run show-hooks -g pane-died");
    let pane_died = String::from_utf8_lossy(&pane_died_out.stdout);
    assert!(
        pane_died.contains("pane-died[") && pane_died.contains("daemoneye notify"),
        "private server pane-died hook does not contain daemoneye notify:\n{}",
        pane_died
    );

    // Check client-attached hook value on the private server (while daemon is live).
    let client_attached_out = env
        .tmux(&["show-hooks", "-g", "client-attached"])
        .output()
        .expect("run show-hooks -g client-attached");
    let client_attached = String::from_utf8_lossy(&client_attached_out.stdout);
    assert!(
        client_attached.contains("client-attached[")
            && client_attached.contains("daemoneye notify"),
        "private server client-attached hook does not contain daemoneye notify:\n{}",
        client_attached
    );
}

/// The default tmux server is unchanged by the isolated daemon.
///
/// Snapshot the default server **before** and **after** the full scenario
/// and assert the two snapshots are byte-equal. The property is *unchanged*,
/// not *nonexistent* — the operator may have a server running.
///
/// Must-NOT: assert the default server is absent, or that its session list is
/// empty. Must-NOT call `kill-server`, `kill-session`, or any `set-hook`
/// through `default_tmux`.
#[test]
fn default_server_unchanged() {
    if !tmux_available() {
        eprintln!("SKIP: tmux not available");
        return;
    }

    let mut env = IsolatedEnv::new();

    // Snapshot the default server before starting the daemon.
    let before = snapshot_default_server(&env);

    // Start and stop the daemon in the isolated environment.
    env.start_daemon("de-test-default");
    env.stop_daemon();

    // Snapshot the default server after the daemon has been torn down.
    let after = snapshot_default_server(&env);

    assert_eq!(
        before, after,
        "default server changed after isolated daemon run.\n\
         before: {}\nafter: {}",
        before, after
    );
}

// ---------------------------------------------------------------------------
// Phase 06a tests: stub server, webhook plumbing, port allocation
// ---------------------------------------------------------------------------

/// Two environments constructed in the same process get different webhook ports.
#[test]
fn webhook_ports_differ_between_environments() {
    let env_a = IsolatedEnv::new();
    let env_b = IsolatedEnv::new();
    assert_ne!(
        env_a.webhook_port(),
        env_b.webhook_port(),
        "two environments should get different webhook ports"
    );
    assert_ne!(
        env_a.stub_port(),
        env_b.stub_port(),
        "two environments should get different stub ports"
    );
}

/// The written config.toml contains the webhook section with the allocated port
/// and a base_url pointing at the stub.
#[tokio::test]
async fn config_contains_webhook_and_stub_url() {
    let mut env = IsolatedEnv::new();
    env.start_stub().await;
    env.start_daemon("de-test-config");

    let config_path = env.root().join(".daemoneye/etc/config.toml");
    let config_text = std::fs::read_to_string(&config_path).expect("read config.toml");

    assert!(
        config_text.contains("[webhook]"),
        "config.toml missing [webhook] section"
    );
    assert!(
        config_text.contains("enabled = true"),
        "config.toml missing webhook enabled = true"
    );
    assert!(
        config_text.contains(&format!("port = {}", env.webhook_port())),
        "config.toml missing webhook port"
    );
    assert!(
        config_text.contains(&env.stub_base_url()),
        "config.toml missing stub base_url"
    );
}

/// Drive `make_client(...)` directly against the stub and assert the
/// concatenated `AiEvent::Token` text equals the supplied canned string.
#[tokio::test]
async fn stub_returns_canned_response_via_make_client() {
    let mut env = IsolatedEnv::new();
    let canned = "GHOST_TRIGGER: YES";
    env.set_stub_response(canned.to_string());
    env.start_stub().await;

    // Verify the stub is reachable before calling through make_client.
    let chat_url = format!("{}/chat/completions", env.stub_base_url());
    let resp = reqwest::get(&chat_url).await.expect("health check to stub");
    // 405 Method Not Allowed is expected for GET on POST-only route.
    assert!(
        resp.status() == 405 || resp.status() == 404,
        "stub should be reachable at {}",
        chat_url
    );

    let client = daemoneye::ai::make_client(
        "openai",
        "test-key".to_string(),
        "test-model".to_string(),
        env.stub_base_url(),
    );

    let system = "You are a watchdog.";
    let messages = vec![daemoneye::ai::Message {
        role: "user".to_string(),
        content: "test".to_string(),
        tool_calls: None,
        tool_results: None,
        turn: None,
    }];

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    client
        .chat(system, messages, tx, false, Vec::new())
        .await
        .expect("chat call to stub succeeded");

    let mut response = String::new();
    while let Some(ev) = rx.recv().await {
        if let daemoneye::ai::AiEvent::Token(t) = ev {
            response.push_str(&t);
        }
    }

    assert_eq!(
        response, canned,
        "stub response mismatch: got {:?}, expected {:?}",
        response, canned
    );
}

/// A started daemon accepts a POST to the allocated webhook port and returns 200.
#[tokio::test]
async fn daemon_webhook_returns_200() {
    if !tmux_available() {
        eprintln!("SKIP: tmux not available");
        return;
    }

    let mut env = IsolatedEnv::new();
    env.set_stub_response("ok".to_string());
    env.start_stub().await;
    env.start_daemon("de-test-webhook");

    let body = serde_json::json!({
        "alerts": [{
    "status": "firing",
            "alertname": "test-alert",
            "labels": {}
        }]
    });

    let (status, _body_text) = env.post_webhook(&body).await;
    assert_eq!(
        status, 200,
        "webhook POST returned status {} instead of 200",
        status
    );
}

// ---------------------------------------------------------------------------
// M6 Phase 06b: Webhook → Ghost End-to-End
// ---------------------------------------------------------------------------

/// A severity-less alert reaches a ghost shell through the full pipeline.
///
/// Drives `process_alert` in-process with a private tmux server, awaits the
/// returned `JoinHandle`, and asserts the event chain:
/// `webhook_alert` → `webhook_analysis{ghost_trigger:true}` → `ghost_start`.
///
/// This test is **deterministic** — it awaits the handle rather than
/// sleeping or polling, satisfying STANDARDS.md §3.3.
///
/// It sets both `HOME` and `TMUX_TMPDIR` under `test_home_guard()` to ensure
/// the ghost's `ensure_incident_session()` shells out to a private tmux server,
/// not the operator's. The guard is held through all environment-dependent work
/// and dropped at the end; both variables are restored afterwards.
#[test]
fn webhook_ghost_e2e_deterministic() {
    use daemoneye::webhook::{WebhookState, parse_payload, process_alert};

    daemoneye::ai::filter::init_masking(&[]);

    let mut env = IsolatedEnv::new();
    let _lock = daemoneye::test_home_guard();

    // Snapshot the default tmux server before any work.
    let before = snapshot_default_server(&env);

    let old_home = std::env::var("HOME").ok();
    let old_tmux_tmpdir = std::env::var("TMUX_TMPDIR").ok();
    unsafe {
        std::env::set_var("HOME", env.root().to_str().unwrap());
        std::env::set_var("TMUX_TMPDIR", env.root().to_str().unwrap());
        std::env::remove_var("TMUX");
        std::env::remove_var("TMUX_PANE");
    }

    daemoneye::config::Config::ensure_dirs().expect("ensure dirs");

    // Create a runbook that resolves for alert name "DiskFull" (kebab: disk-full).
    let runbooks_dir = daemoneye::config::config_dir().join("runbooks");
    std::fs::create_dir_all(&runbooks_dir).expect("create runbooks dir");
    let runbook_content = r#"---
enabled: true
max_ghost_turns: 1
---
# Runbook: disk-full

## Alert Criteria
- Disk usage above 90%

## Remediation Steps
1. Check large files
"#;
    std::fs::write(runbooks_dir.join("disk-full.md"), runbook_content).expect("write runbook");

    // Start the canned-AI stub server using the harness.
    env.set_stub_response("ALERT: Disk usage critical\n\nGHOST_TRIGGER: YES".to_string());

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");
    rt.block_on(env.start_stub());

    // Build WebhookState pointing at the harness stub.
    let mut config = daemoneye::config::Config::default();
    config.models.insert(
        "default".to_string(),
        daemoneye::config::ModelEntry {
            provider: "openai".to_string(),
            api_key: "test-key".to_string(),
            model: "test-model".to_string(),
            base_url: Some(env.stub_base_url()),
            input_cost_per_mtok: Some(3.0),
            output_cost_per_mtok: Some(15.0),
            cache_read_cost_per_mtok: Some(0.3),
            cache_write_cost_per_mtok: Some(3.75),
            context_window_tokens: None,
        },
    );

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

    // Severity-less payload — this is the defect-1 regression.
    let body = serde_json::json!({
        "status": "firing",
        "alerts": [{
            "status": "firing",
            "labels": {
                "alertname": "DiskFull",
                "instance": "web-01"
            },
            "annotations": {
                "summary": "Disk usage above 90%"
            },
            "fingerprint": "test-fp-diskfull"
        }]
    });
    let alerts = parse_payload(&body);
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].alert_name, "DiskFull");
    assert_eq!(alerts[0].severity, ""); // no severity label

    let handle = rt.block_on(process_alert(alerts[0].clone(), state));

    // A ghost should have been spawned — handle is Some.
    let ghost_handle = handle.expect("process_alert returned None — no ghost spawned");

    // Await the ghost handle — deterministic, no sleep or polling.
    rt.block_on(async {
        if let Err(e) = ghost_handle.await {
            panic!("Ghost task panicked: {:?}", e);
        }
    });

    // Read the event segment and assert the full chain.
    let path = daemoneye::config::current_event_segment_path();
    let content = std::fs::read_to_string(&path).expect("read event segment");

    // Assert webhook_alert exists.
    let webhook_alert = content.lines().find_map(|line| {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        (v.get("event").and_then(|e| e.as_str()) == Some("webhook_alert")).then_some(v)
    });
    assert!(
        webhook_alert.is_some(),
        "webhook_alert record not found in event segment:\n{}",
        content
    );

    // Assert webhook_analysis with ghost_trigger == true and ghost_enabled == true.
    let webhook_analysis = content.lines().find_map(|line| {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        (v.get("event").and_then(|e| e.as_str()) == Some("webhook_analysis")).then_some(v)
    });
    let webhook_analysis = webhook_analysis.unwrap_or_else(|| {
        panic!(
            "webhook_analysis record not found in event segment:\n{}",
            content
        )
    });
    assert_eq!(
        webhook_analysis["ghost_trigger"], true,
        "ghost_trigger should be true in webhook_analysis"
    );
    assert_eq!(
        webhook_analysis["ghost_enabled"], true,
        "ghost_enabled should be true in webhook_analysis"
    );

    // Assert ghost_start exists with matching alert_name.
    let ghost_start = content.lines().find_map(|line| {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        (v.get("event").and_then(|e| e.as_str()) == Some("ghost_start")).then_some(v)
    });
    let ghost_start = ghost_start.unwrap_or_else(|| {
        panic!(
            "ghost_start record not found in event segment:\n{}",
            content
        )
    });
    assert_eq!(
        ghost_start["alert_name"], "disk-full",
        "ghost_start alert_name should be 'disk-full'"
    );

    // Print the ghost_start record for the completion log.
    eprintln!(
        "ghost_start record: {}",
        serde_json::to_string_pretty(&ghost_start).unwrap_or_default()
    );

    // Assert the default tmux server is unchanged.
    let after = snapshot_default_server(&env);
    assert_eq!(
        before, after,
        "default tmux server changed after isolated ghost run.\nbefore: {}\nafter: {}",
        before, after
    );

    // Restore environment.
    match old_home {
        Some(v) => unsafe { std::env::set_var("HOME", v) },
        None => unsafe { std::env::remove_var("HOME") },
    }
    match old_tmux_tmpdir {
        Some(v) => unsafe { std::env::set_var("TMUX_TMPDIR", v) },
        None => unsafe { std::env::remove_var("TMUX_TMPDIR") },
    }

    drop(_lock);
}

/// Full-daemon HTTP variant of the webhook→ghost pipeline.
///
/// This test exercises the real HTTP path: `start_daemon()`, `start_stub()`,
/// `post_webhook()` with a severity-less payload, then waits for `ghost_start`
/// to appear in the event segment.
///
/// This test is `#[ignore]` because it cannot be deterministic — the daemon
/// spawns per alert so the 200 response carries no completion signal, and
/// STANDARDS.md §3.3 forbids `sleep` and real wall-clock waits in tests.
/// It covers the HTTP handler path that the deterministic test cannot reach
/// (the `server.rs` spawn-and-return-200 path).
///
/// Run with: `cargo test --test isolation -- --ignored webhook_ghost_e2e_http`
#[tokio::test]
#[ignore]
async fn webhook_ghost_e2e_http() {
    let mut env = IsolatedEnv::new();

    // Create a runbook that resolves for alert name "DiskFull" (kebab: disk-full).
    let runbooks_dir = env.root().join(".daemoneye/runbooks");
    std::fs::create_dir_all(&runbooks_dir).expect("create runbooks dir");
    let runbook_content = r#"---
enabled: true
max_ghost_turns: 1
---
# Runbook: disk-full

## Alert Criteria
- Disk usage above 90%

## Remediation Steps
1. Check large files
"#;
    std::fs::write(runbooks_dir.join("disk-full.md"), runbook_content).expect("write runbook");

    env.set_stub_response("ALERT: Disk usage critical\n\nGHOST_TRIGGER: YES".to_string());
    env.start_stub().await;
    env.start_daemon("de-test-ghost-http");

    // Severity-less payload.
    let body = serde_json::json!({
        "status": "firing",
        "alerts": [{
            "status": "firing",
            "labels": {
                "alertname": "DiskFull",
                "instance": "web-01"
            },
            "annotations": {
                "summary": "Disk usage above 90%"
            },
            "fingerprint": "test-fp-diskfull-http"
        }]
    });

    let (status, _body_text) = env.post_webhook(&body).await;
    assert_eq!(status, 200, "webhook POST should return 200");

    // Wait for ghost_start to appear in the event segment.
    // Bound the wait so a failure fails loudly rather than hanging.
    let event_dir = env.root().join(".daemoneye/var/log/events");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut found = false;
    while std::time::Instant::now() < deadline {
        if let Ok(entries) = std::fs::read_dir(&event_dir) {
            for entry in entries.flatten() {
                if let Ok(content) = std::fs::read_to_string(entry.path())
                    && content.lines().any(|line| {
                        serde_json::from_str::<serde_json::Value>(line)
                            .ok()
                            .map(|v| v.get("event").and_then(|e| e.as_str()) == Some("ghost_start"))
                            .unwrap_or(false)
                    })
                {
                    found = true;
                    break;
                }
            }
        }
        if found {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(
        found,
        "ghost_start did not appear in the event segment within 30 seconds"
    );
}

/// Direct proof that the stub listener is genuinely held: attempting to bind
/// the same port must fail with AddrInUse. If the allocator ever reverts to
/// dropping the listener before returning, this fails immediately.
#[test]
fn held_port_cannot_be_rebound() {
    let env = IsolatedEnv::new();
    let err = std::net::TcpListener::bind(("127.0.0.1", env.stub_port()))
        .expect_err("stub port must still be held by the env");
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::AddrInUse,
        "expected AddrInUse, got {err:?}"
    );
}
