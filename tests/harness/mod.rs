//! Test-isolation harness.
//!
//! Runs a real `daemoneye` daemon against a throwaway `$HOME` and a private
//! tmux server, touching neither the operator's `~/.daemoneye/` nor their
//! default tmux server.

use std::process::Command;
use std::sync::Arc;
use tempfile::TempDir;

/// An isolated test environment with a throwaway `$HOME` and private tmux server.
pub struct IsolatedEnv {
    root: TempDir,
    webhook_port: u16,
    stub_handle: Option<tokio::task::JoinHandle<()>>,
    stub_port: u16,
    stub_response: Arc<std::sync::Mutex<String>>,
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

        let webhook_port = alloc_free_port();
        let stub_port = alloc_free_port();

        Self {
            root,
            webhook_port,
            stub_handle: None,
            stub_port,
            stub_response: Arc::new(std::sync::Mutex::new(String::new())),
        }
    }

    /// Return the throwaway root path (used as `$HOME`).
    pub fn root(&self) -> &std::path::Path {
        self.root.path()
    }

    /// Return the webhook port allocated for this environment.
    pub fn webhook_port(&self) -> u16 {
        self.webhook_port
    }

    /// Return the stub server port.
    pub fn stub_port(&self) -> u16 {
        self.stub_port
    }

    /// Set the canned response the stub server will return.
    pub fn set_stub_response(&mut self, response: String) {
        let mut guard = self.stub_response.lock().unwrap();
        *guard = response;
    }

    /// Start the canned-AI stub server.
    ///
    /// // Chose OpenAI-compatible wire format because it is simpler to emit
    /// // faithfully than Anthropic's: a single SSE stream with
    /// // `choices[0].delta.content` tokens and a `[DONE]` terminator.
    /// // The Anthropic format requires `message_start`, `content_block_start`,
    /// // `content_block_delta`, `content_block_stop`, `message_delta`,
    /// // `message_stop` — more moving parts for the same test goal.
    pub async fn start_stub(&mut self) {
        let response = Arc::clone(&self.stub_response);
        let port = self.stub_port;

        let handle = tokio::spawn(async move {
            let response = Arc::clone(&response);

            let app = axum::Router::new().route(
                "/chat/completions",
                axum::routing::post(
                    move |axum::Json(_body): axum::Json<serde_json::Value>| async move {
                        let canned: String = {
                            let guard = response.lock().unwrap();
                            guard.clone()
                        };
                        let events: Vec<
                            Result<axum::response::sse::Event, std::convert::Infallible>,
                        > = build_sse_events(canned);
                        axum::response::Sse::new(futures_util::stream::iter(events))
                    },
                ),
            );

            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
                .await
                .expect("bind stub server");
            axum::serve(listener, app).await.expect("serve stub");
        });

        self.stub_handle = Some(handle);
        // Give the stub a moment to bind.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    /// Return the stub's base URL (including `/v1` suffix).
    pub fn stub_base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.stub_port)
    }

    /// Build the test config TOML with webhook and stub settings.
    fn build_test_config(&self) -> String {
        format!(
            r#"[models.default]
provider = "openai"
api_key  = "test-key-not-a-real-credential"
model    = "claude-sonnet-4-6"
base_url = "{}"
input_cost_per_mtok       = 3.00
output_cost_per_mtok      = 15.00
cache_read_cost_per_mtok  = 0.30
cache_write_cost_per_mtok = 3.75

[webhook]
enabled = true
port = {}
bind_addr = "127.0.0.1"
"#,
            self.stub_base_url(),
            self.webhook_port
        )
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
        std::fs::write(etc_dir.join("config.toml"), self.build_test_config())
            .expect("write config.toml");
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

    /// POST a JSON body to the environment's webhook endpoint.
    ///
    /// Returns the HTTP status code and response body.
    pub async fn post_webhook(&self, body: &serde_json::Value) -> (u16, String) {
        let url = format!("http://127.0.0.1:{}/webhook", self.webhook_port);
        let resp = reqwest::Client::new()
            .post(&url)
            .json(body)
            .send()
            .await
            .expect("POST to webhook");
        (
            resp.status().as_u16(),
            resp.text().await.expect("read webhook response"),
        )
    }
}

impl Drop for IsolatedEnv {
    fn drop(&mut self) {
        // Shut down the stub server.
        if let Some(handle) = self.stub_handle.take() {
            handle.abort();
        }

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

/// Return the explicit socket path of the private tmux server.
///
/// Used by `Drop` to pin `kill-server` to a specific socket rather than
/// relying on `TMUX_TMPDIR` — teardown must be safe independently of
/// whether `apply_env` is correct.
///
/// Discovers the socket by scanning for the `tmux-*` directory tmux
/// creates under `TMUX_TMPDIR`. If none exists (server never started),
/// returns a path that will not exist, making teardown a no-op.
fn private_tmux_socket(root: &std::path::Path) -> std::path::PathBuf {
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

impl IsolatedEnv {
    fn private_tmux_socket(&self) -> std::path::PathBuf {
        private_tmux_socket(self.root.path())
    }
}

/// Allocate a free TCP port by binding to port 0 and reading the assigned port.
fn alloc_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind to port 0");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    port
}

/// Build SSE events for an OpenAI-compatible chat completion response.
///
/// Chose OpenAI-compatible wire format because it is simpler to emit faithfully
/// than Anthropic's: a single SSE stream with `choices[0].delta.content` tokens
/// and a `[DONE]` terminator. The Anthropic format requires `message_start`,
/// `content_block_start`, `content_block_delta`, `content_block_stop`,
/// `message_delta`, `message_stop` — more moving parts for the same test goal.
fn build_sse_events(
    text: String,
) -> Vec<Result<axum::response::sse::Event, std::convert::Infallible>> {
    let mut events = Vec::new();

    // Send the entire text as a single token. Splitting by word boundaries
    // loses spaces when the client trims empty deltas, so a single chunk
    // preserves the exact string.
    let payload = serde_json::json!({
        "id": "chatcmpl-test",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": "test-model",
        "choices": [
            {
                "index": 0,
                "delta": { "content": &text },
                "finish_reason": null
            }
        ]
    });
    let event = axum::response::sse::Event::default().data(format!("{}\n", payload));
    events.push(Ok(event));

    // Final chunk with finish_reason.
    let final_payload = serde_json::json!({
        "id": "chatcmpl-test",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": "test-model",
        "choices": [
            {
                "index": 0,
                "delta": {},
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": text.split_whitespace().count(),
            "total_tokens": 10 + text.split_whitespace().count()
        }
    });
    let final_event = axum::response::sse::Event::default().data(format!("{}\n", final_payload));
    events.push(Ok(final_event));

    // [DONE] terminator.
    events.push(Ok(axum::response::sse::Event::default().data("[DONE]")));

    events
}
