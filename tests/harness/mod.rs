//! Test-isolation harness.
//!
//! Runs a real `daemoneye` daemon against a throwaway `$HOME` and a private
//! tmux server, touching neither the operator's `~/.daemoneye/` nor their
//! default tmux server.

use std::process::Command;
use tempfile::TempDir;

/// Minimal config.toml written after `daemoneye setup` (which overwrites it).
const TEST_CONFIG_TOML: &str = r#"[models.default]
provider = "anthropic"
api_key  = "test-key-not-a-real-credential"
model    = "claude-sonnet-4-6"
input_cost_per_mtok       = 3.00
output_cost_per_mtok      = 15.00
cache_read_cost_per_mtok  = 0.30
cache_write_cost_per_mtok = 3.75
"#;

/// An isolated test environment with a throwaway `$HOME` and private tmux server.
pub struct IsolatedEnv {
    root: TempDir,
}

impl IsolatedEnv {
    /// Create a new isolated environment.
    ///
    /// The temp directory is rooted at `/tmp` (not `std::env::temp_dir()`)
    /// because Unix socket paths are capped at ~108 bytes and `$TMPDIR` can
    /// be arbitrarily long.
    pub fn new() -> Self {
        let root = TempDir::new_in("/tmp").expect("create temp dir under /tmp");

        // Assert the socket path is short enough for a Unix socket.
        let socket_path = root.path().join(".daemoneye/var/run/daemoneye.sock");
        let len = socket_path.as_os_str().len();
        assert!(
            len < 100,
            "socket path too long for Unix socket ({} bytes): {}",
            len,
            socket_path.display()
        );

        Self { root }
    }

    /// Return the throwaway root path (used as `$HOME`).
    pub fn root(&self) -> &std::path::Path {
        self.root.path()
    }

    /// Return the explicit socket path of the private tmux server.
    ///
    /// Used by `Drop` to pin `kill-server` to a specific socket rather than
    /// relying on `TMUX_TMPDIR` — teardown must be safe independently of
    /// whether `apply_env` is correct.
    ///
    /// Discovers the socket by scanning for the `tmux-*` directory tmux
    /// creates under `TMUX_TMPDIR`. If none exists (server never started),
    /// returns a path that will not exist, making teardown a no-op.
    fn private_tmux_socket(&self) -> std::path::PathBuf {
        let root = self.root.path();
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with("tmux-") {
                    return entry.path().join("default");
                }
            }
        }
        root.join("tmux-0/default")
    }

    // -----------------------------------------------------------------------
    // Environment helper — the single source of isolation
    // -----------------------------------------------------------------------

    /// Apply the isolation environment to a command.
    ///
    /// This is the **only** place where the isolation environment is set.
    /// Both `daemoneye()` and `tmux()` delegate here so they cannot drift.
    fn apply_env(&self, cmd: &mut Command) {
        let root = self.root.path().as_os_str();
        cmd.env("HOME", root)
            .env("TMUX_TMPDIR", root)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE");
    }

    // -----------------------------------------------------------------------
    // Command builders
    // -----------------------------------------------------------------------

    /// Build a `daemoneye` command inside the isolated environment.
    pub fn daemoneye(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_daemoneye"));
        cmd.args(args);
        self.apply_env(&mut cmd);
        cmd
    }

    /// Build a `tmux` command inside the isolated (private) server.
    pub fn tmux(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new("tmux");
        cmd.args(args);
        self.apply_env(&mut cmd);
        cmd
    }

    /// Build a `tmux` command on the **default** (operator's) server.
    ///
    /// This does **not** get the throwaway `HOME` or `TMUX_TMPDIR` — it
    /// observes the real environment for snapshotting the default server.
    pub fn default_tmux(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new("tmux");
        cmd.args(args)
            .env_remove("TMUX_TMPDIR")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE");
        cmd
    }

    // -----------------------------------------------------------------------
    // Daemon lifecycle
    // -----------------------------------------------------------------------

    /// Start the daemon inside the isolated environment.
    ///
    /// Runs `daemoneye setup` first (HOME-confined), then
    /// `daemoneye daemon --session <session>` without `--console`, waiting
    /// on the parent's exit status per the readiness handshake.
    pub fn start_daemon(&self, session: &str) -> std::process::Output {
        // Run setup to create the directory tree.
        let setup_out = self
            .daemoneye(&["setup"])
            .output()
            .expect("run daemoneye setup");
        assert!(
            setup_out.status.success(),
            "daemoneye setup failed: {}\nstdout: {}\nstderr: {}",
            setup_out.status,
            String::from_utf8_lossy(&setup_out.stdout),
            String::from_utf8_lossy(&setup_out.stderr)
        );

        // Write the test config after setup — setup's ensure_dirs() overwrites
        // any pre-existing config.toml with the bundled default (empty api_key).
        self.write_test_config();

        // Start the daemon (non-console, forks). The parent exits once the
        // child has bound its socket (readiness handshake).
        let daemon_out = self
            .daemoneye(&["daemon", "--session", session])
            .output()
            .expect("run daemoneye daemon");
        assert!(
            daemon_out.status.success(),
            "daemoneye daemon failed to start (exit {})\n\
             stderr: {}\n\
             daemon.log: {}",
            daemon_out.status,
            String::from_utf8_lossy(&daemon_out.stderr),
            self.daemon_log()
        );
        daemon_out
    }

    /// Write a minimal test config with a dummy API key.
    ///
    /// Must be called after `daemoneye setup` because setup's `ensure_dirs()`
    /// overwrites any pre-existing config.toml.
    fn write_test_config(&self) {
        let etc_dir = self.root.path().join(".daemoneye/etc");
        std::fs::create_dir_all(&etc_dir).expect("create etc dir");
        std::fs::write(etc_dir.join("config.toml"), TEST_CONFIG_TOML).expect("write config.toml");
    }

    /// Stop the daemon (best-effort).
    pub fn stop_daemon(&self) {
        let _ = self.daemoneye(&["stop"]).output();
    }

    /// Read the daemon log, returning an empty string if absent.
    pub fn daemon_log(&self) -> String {
        let log_path = self.root.path().join(".daemoneye/var/log/daemon.log");
        std::fs::read_to_string(&log_path).unwrap_or_default()
    }
}

impl Drop for IsolatedEnv {
    fn drop(&mut self) {
        // Best-effort cleanup: stop the daemon, then kill the private tmux server.
        let _ = self.daemoneye(&["stop"]).output();

        // Pin kill-server to the explicit socket path rather than going through
        // self.tmux() (which depends on TMUX_TMPDIR). Teardown must be safe
        // independently of whether apply_env is correct — a broken apply_env
        // should make teardown a no-op, not a catastrophe.
        let socket = self.private_tmux_socket();
        if socket.exists() {
            let _ = Command::new("tmux")
                .args(["-S", socket.to_str().unwrap(), "kill-server"])
                .output();
        }
        // TempDir's own drop removes the root.
    }
}
