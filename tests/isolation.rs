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
    env.start_daemon("de-test");

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
/// After `start_daemon`, `tmux show-hooks -g` on the **private** server
/// mentions `pane-died`. This proves the daemon really did reach a tmux
/// server, so that the negative half (default server unchanged) is not
/// passing vacuously.
#[test]
fn hooks_land_on_private_server() {
    if !tmux_available() {
        eprintln!("SKIP: tmux not available");
        return;
    }

    let env = IsolatedEnv::new();
    env.start_daemon("de-test");

    // Check that the private server has the daemon's hooks.
    // pane-died is a built-in event hook that does not appear in the
    // general `show-hooks -g` listing — it must be queried by name.
    let hooks_out = env
        .tmux(&["show-hooks", "-g", "pane-died"])
        .output()
        .expect("run show-hooks -g pane-died");
    let hooks = String::from_utf8_lossy(&hooks_out.stdout);
    assert!(
        hooks.contains("pane-died"),
        "private server does not have pane-died hook:\n{}",
        hooks
    );
}

/// The default tmux server is unchanged by the isolated daemon.
///
/// Snapshot the default server **before** and **after** the full scenario
/// and assert the two snapshots are byte-equal. The property is *unchanged*,
/// not *nonexistent* — the operator may have a server running.
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
    env.start_daemon("de-test");
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
