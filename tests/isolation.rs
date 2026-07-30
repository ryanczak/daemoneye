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

    let env = IsolatedEnv::new();
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

    let env = IsolatedEnv::new();
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

    let env = IsolatedEnv::new();

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
