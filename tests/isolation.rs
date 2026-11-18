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

/// The daemon installs no tmux hooks anywhere.
///
/// M27 phase 05b removed every hook install: the daemon is a read-only
/// observer. A private tmux server that existed before the daemon started
/// (started here by the fixture) must still carry no `daemoneye notify`
/// hooks after the daemon boots.
#[test]
fn daemon_installs_no_tmux_hooks() {
    if !tmux_available() {
        eprintln!("SKIP: tmux not available");
        return;
    }

    let mut env = IsolatedEnv::new();
    env.start_daemon("de-test-hooks");

    let out = env
        .tmux(&["show-hooks", "-g"])
        .output()
        .expect("run show-hooks -g");
    let hooks = String::from_utf8_lossy(&out.stdout);
    assert!(
        !hooks.contains("daemoneye notify"),
        "the daemon installed hooks: {}",
        hooks
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
        4096,
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
/// Drives `process_alert` in-process with a private tmux server, waits on the
/// bus for the ghost's lifecycle notice, and asserts the event chain:
/// `webhook_alert` → `webhook_analysis{ghost_trigger:true}` → `ghost_start`.
///
/// The analysis is run by the queued `ghost_trigger` bus consumer: the test
/// registers it, subscribes its own queued receiver for `Ghost` notices
/// **before** calling `process_alert`, and awaits the first notice whose
/// title starts `[Ghost Shell Completed]` / `[Ghost Shell Failed]` /
/// `[Ghost Shell Skipped]` under a timeout. Deterministic — the bus is the
/// completion signal, no sleep or polling.
///
/// It sets both `HOME` and `TMUX_TMPDIR` under `test_home_guard()` so any tmux
/// subprocess the daemon itself may shell out to lands in a private server,
/// not the operator's. Ghosts start no tmux session of their own; what this
/// guards is `TMUX_TMPDIR` isolation. The guard is held through all
/// environment-dependent work and dropped at the end; both variables are
/// restored afterwards.
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
    let _rt_guard = rt.enter();
    rt.block_on(env.start_stub());

    // Build WebhookState pointing at the harness stub.
    let mut config = daemoneye::config::Config::default();
    config.webhook.auto_analyze = true;
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
            max_tokens: None,
        },
    );

    // The gate is fixed at `warning`; the payload carries a rankable
    // `warning` severity so the alert is admitted.

    let sessions = daemoneye::daemon::session::SessionStore::default();
    let cache = std::sync::Arc::new(daemoneye::daemon::SessionCache::new("test"));
    let schedule_store = std::sync::Arc::new(daemoneye::scheduler::ScheduleStore::new_empty());
    let bus = daemoneye::daemon::notice_sinks::test_bus(
        sessions.clone(),
        daemoneye::bus::Severity::Warning,
    );
    let state = std::sync::Arc::new(WebhookState {
        config: config.clone(),
        sessions,
        cache,
        schedule_store,
        bus,
        dedup: std::sync::Mutex::new(std::collections::HashMap::new()),
        rate_limit: std::sync::Mutex::new(std::collections::HashMap::new()),
        ip_limits: std::sync::Mutex::new(std::collections::HashMap::new()),
    });

    let body = serde_json::json!({
        "status": "firing",
        "alerts": [{
            "status": "firing",
            "labels": {
                "alertname": "DiskFull",
                "severity": "warning",
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
    assert_eq!(alerts[0].severity, "warning");

    // The analysis is run by the queued `ghost_trigger` consumer, so the
    // ghost's lifecycle notices arrive on the bus. Subscribe a receiver for
    // them **before** publishing the alert.
    let mut ghost_rx = bus.subscribe(
        "isolation-ghost-watcher",
        daemoneye::bus::Filter {
            sources: vec![daemoneye::bus::Source::Ghost],
            ..Default::default()
        },
        64,
    );

    // The ghost trigger registers after `rt.enter()` (so its queued task
    // runs on this current-thread runtime) and before the alert is
    // published (a subscriber sees only what is published after it).
    daemoneye::webhook::register_ghost_trigger(state.clone());
    rt.block_on(process_alert(alerts[0].clone(), state.clone()));

    // Wait for the ghost's lifecycle notice — the bus is the completion
    // signal, deterministic, no sleep or polling.
    rt.block_on(async {
        let wait = async {
            while let Some(notice) = ghost_rx.recv().await {
                if notice.title.starts_with("[Ghost Shell Completed]")
                    || notice.title.starts_with("[Ghost Shell Failed]")
                    || notice.title.starts_with("[Ghost Shell Skipped]")
                {
                    return;
                }
            }
            panic!("the ghost watcher's queue closed before the ghost finished");
        };
        tokio::time::timeout(std::time::Duration::from_secs(120), wait)
            .await
            .expect("the ghost never finished — no lifecycle notice within 120s");
    });

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

/// The door test for row 08a: a real alert → `process_alert` →
/// `start_session_with_config` → the ghost's turn, with `[hosts.e2e-host]`
/// in the isolated `config.toml` and a runbook whose `ssh_target` names it.
/// The stub reply carries no tool call, so no command runs and the host is
/// never contacted — the assertion is only that ghost **start** recorded how
/// the value resolved: one `ghost_ssh_target_resolved` record with
/// `resolution == "configured"` and `host == "e2e-host"`. Copied from
/// `webhook_ghost_e2e_deterministic` on purpose, not refactored.
#[test]
fn webhook_ghost_records_its_ssh_target_resolution() {
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

    // Only the `[hosts]` table: the ghost turn takes its model from the
    // in-memory WebhookState, so the file needs nothing else.
    std::fs::write(
        daemoneye::config::etc_dir().join("config.toml"),
        "[hosts.e2e-host]\nssh = \"e2e-host.invalid\"\n",
    )
    .expect("write isolated config.toml");

    // Create a runbook that resolves for alert name "DiskFull" (kebab:
    // disk-full), carrying the ssh_target this row resolves.
    let runbooks_dir = daemoneye::config::config_dir().join("runbooks");
    std::fs::create_dir_all(&runbooks_dir).expect("create runbooks dir");
    let runbook_content = r#"---
enabled: true
max_ghost_turns: 1
ssh_target: e2e-host
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
    let _rt_guard = rt.enter();
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
            max_tokens: None,
        },
    );

    // The gate is fixed at `warning`; this test exercises the analysis
    // pipeline, so the payload carries a rankable `warning` severity.

    let sessions = daemoneye::daemon::session::SessionStore::default();
    let cache = std::sync::Arc::new(daemoneye::daemon::SessionCache::new("test"));
    let schedule_store = std::sync::Arc::new(daemoneye::scheduler::ScheduleStore::new_empty());
    let bus = daemoneye::daemon::notice_sinks::test_bus(
        sessions.clone(),
        daemoneye::bus::Severity::Warning,
    );
    let state = std::sync::Arc::new(WebhookState {
        config: config.clone(),
        sessions,
        cache,
        schedule_store,
        bus,
        dedup: std::sync::Mutex::new(std::collections::HashMap::new()),
        rate_limit: std::sync::Mutex::new(std::collections::HashMap::new()),
        ip_limits: std::sync::Mutex::new(std::collections::HashMap::new()),
    });

    let body = serde_json::json!({
        "status": "firing",
        "alerts": [{
            "status": "firing",
            "labels": {
                "alertname": "DiskFull",
                "severity": "warning",
                "instance": "web-01"
            },
            "annotations": {
                "summary": "Disk usage above 90%"
            },
            "fingerprint": "test-fp-ssh-target"
        }]
    });
    let alerts = parse_payload(&body);
    assert_eq!(alerts.len(), 1);

    // The analysis is run by the queued `ghost_trigger` consumer, so the
    // ghost's lifecycle notices arrive on the bus. Subscribe a receiver for
    // them **before** publishing the alert.
    let mut ghost_rx = bus.subscribe(
        "isolation-ghost-watcher-ssh",
        daemoneye::bus::Filter {
            sources: vec![daemoneye::bus::Source::Ghost],
            ..Default::default()
        },
        64,
    );

    // The alert carries severity "warning", which the fixed gate admits.
    // The ghost trigger registers after `rt.enter()` (so its queued task
    // runs on this current-thread runtime) and before the alert is
    // published (a subscriber sees only what is published after it).
    daemoneye::webhook::register_ghost_trigger(state.clone());
    rt.block_on(process_alert(alerts[0].clone(), state.clone()));

    // Wait for the ghost's lifecycle notice — the bus is the completion
    // signal, deterministic, no sleep or polling.
    rt.block_on(async {
        let wait = async {
            while let Some(notice) = ghost_rx.recv().await {
                if notice.title.starts_with("[Ghost Shell Completed]")
                    || notice.title.starts_with("[Ghost Shell Failed]")
                    || notice.title.starts_with("[Ghost Shell Skipped]")
                {
                    return;
                }
            }
            panic!("the ghost watcher's queue closed before the ghost finished");
        };
        tokio::time::timeout(std::time::Duration::from_secs(120), wait)
            .await
            .expect("the ghost never finished — no lifecycle notice within 120s");
    });

    // Read the event segment and assert the resolution record.
    let path = daemoneye::config::current_event_segment_path();
    let content = std::fs::read_to_string(&path).expect("read event segment");

    let resolved = content.lines().find_map(|line| {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        (v.get("event").and_then(|e| e.as_str()) == Some("ghost_ssh_target_resolved")).then_some(v)
    });
    let resolved = resolved.unwrap_or_else(|| {
        panic!(
            "ghost_ssh_target_resolved record not found in event segment:\n{}",
            content
        )
    });
    assert_eq!(
        resolved["resolution"], "configured",
        "the ssh_target must have resolved"
    );
    assert_eq!(
        resolved["host"], "e2e-host",
        "the ssh_target must have resolved to the [hosts] key it names"
    );
    assert_eq!(
        resolved["ssh_target"], "e2e-host",
        "the record carries the raw runbook value"
    );

    // The ghost must actually have started (the chain is intact).
    let ghost_start = content.lines().find_map(|line| {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        (v.get("event").and_then(|e| e.as_str()) == Some("ghost_start")).then_some(v)
    });
    assert!(
        ghost_start.is_some(),
        "ghost_start record not found in event segment:\n{}",
        content
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

    // Payload carrying a rankable `warning` severity, which the fixed gate
    // admits.
    let body = serde_json::json!({
        "status": "firing",
        "alerts": [{
            "status": "firing",
            "labels": {
                "alertname": "DiskFull",
                "severity": "warning",
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

/// An empty `--session` starts no tmux server at all.
///
/// `--session ""` means no tmux: the daemon adopts nothing and never runs
/// `tmux new-session`, so no tmux server is started under the private
/// `TMUX_TMPDIR` and no hooks are installed. The daemon itself still boots
/// and answers `ping`.
///
/// Deliberately has **no** `tmux_available()` guard: with no tmux installed
/// there is trivially no server, and the daemon must still boot and answer.
#[test]
fn an_empty_session_starts_no_tmux_server() {
    let mut env = IsolatedEnv::new();
    env.start_daemon("");

    // The daemon is up and answering through the isolated socket.
    let ping_out = env
        .daemoneye(&["ping"])
        .output()
        .expect("run daemoneye ping");
    assert!(
        ping_out.status.success(),
        "daemoneye ping failed with an empty session: {}",
        String::from_utf8_lossy(&ping_out.stderr)
    );

    // No tmux server was started: a server creates `tmux-<uid>/` directly
    // under the throwaway root (the env's TMUX_TMPDIR).
    let tmux_dirs: Vec<String> = std::fs::read_dir(env.root())
        .expect("read throwaway root")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("tmux-"))
        .collect();
    assert!(
        tmux_dirs.is_empty(),
        "an empty session name must not start a tmux server, found {tmux_dirs:?}"
    );

    // The startup path logged that it adopted nothing.
    let log = env.daemon_log();
    assert!(
        log.contains("No tmux session to adopt"),
        "daemon.log should say no tmux session was adopted:\n{log}"
    );
}

/// The daemon rebuilds the active-alert registry from the notice journal at
/// startup (M38 phase 08a).
///
/// Before `start_daemon`, journal one firing webhook notice an hour old with
/// a fingerprint no other test uses, into the throwaway root's current event
/// segment. After the daemon is up, `Request::AlertList` over its socket must
/// carry that fingerprint.
#[tokio::test]
async fn daemon_start_replays_active_alerts() {
    let env = IsolatedEnv::new();

    // The notice, journaled exactly as `Bus::new` journals it: the serialized
    // `Notice` (its `ts` is the record's `ts`) plus the `event` and `pid`
    // keys `log_event` stamps. An hour old — inside the registry's TTL
    // window, so the replay must pick it up.
    let notice = daemoneye::bus::Notice::new(
        daemoneye::bus::Severity::Warning,
        daemoneye::bus::Source::Webhook,
        "ReplayHighCPU",
        "CPU high on web-01",
    )
    .status(daemoneye::bus::NoticeStatus::Firing)
    .fingerprint("fp-08a-replay-e2e")
    .label("severity", "warning")
    .ts_refire(chrono::Utc::now().timestamp() - 3600);

    let mut record = serde_json::to_value(&notice).expect("serialize notice");
    let obj = record.as_object_mut().expect("notice is an object");
    let mut stamped = serde_json::Map::new();
    stamped.insert(
        "ts".to_string(),
        serde_json::Value::String(notice.ts.to_rfc3339()),
    );
    stamped.insert(
        "event".to_string(),
        serde_json::Value::String("notice".to_string()),
    );
    stamped.insert("pid".to_string(), serde_json::Value::from(1));
    for (k, v) in obj.clone() {
        stamped.insert(k, v);
    }

    // The harness's `start_daemon` runs `daemoneye setup` and writes the test
    // config, both HOME-confined — the directory tree exists after that, so
    // start the daemon first, stop it, then write the journal line and start
    // again. The stop is clean: the registry is rebuilt only from the journal
    // at startup, never persisted, so the second boot replays it.
    let mut env = env;
    env.start_daemon("de-test-replay");
    env.stop_daemon();

    let segment = env.root().join(".daemoneye/var/log/events").join(format!(
        "events-{}.jsonl",
        chrono::Utc::now().format("%Y%m%d")
    ));
    std::fs::create_dir_all(segment.parent().expect("segment has a parent"))
        .expect("create events dir");
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&segment)
        .expect("open the current event segment");
    writeln!(
        file,
        "{}",
        serde_json::to_string(&stamped).expect("serialize record")
    )
    .expect("append the journal record");

    env.start_daemon("de-test-replay-2");

    // Ask the daemon over its socket, serialized with the crate's own type.
    let socket = env.root().join(".daemoneye/var/run/daemoneye.sock");
    let stream = tokio::net::UnixStream::connect(&socket)
        .await
        .expect("connect to the daemon socket");
    let (rx, mut tx) = stream.into_split();
    let mut rx = tokio::io::BufReader::new(rx);
    daemoneye::cli::commands::send_request(&mut tx, daemoneye::ipc::Request::AlertList)
        .await
        .expect("send AlertList");
    let reply = daemoneye::cli::commands::recv(&mut rx)
        .await
        .expect("read the AlertList reply");

    let daemoneye::ipc::Response::AlertState { alerts } = reply else {
        panic!("expected Response::AlertState, got {reply:?}");
    };
    assert!(
        alerts.iter().any(|a| a.fingerprint == "fp-08a-replay-e2e"),
        "the journaled firing alert must be in the AlertList reply (root {}): {alerts:?}",
        env.root().display()
    );
}
