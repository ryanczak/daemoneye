use serde::{Deserialize, Serialize};

/// The default path for the DaemonEye IPC socket.
pub const DEFAULT_SOCKET_PATH: &str = "/tmp/daemoneye.sock";

/// Messages sent from the CLI client to the daemon.
#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    /// A simple ping to check if the daemon is alive.
    Ping,
    /// Ask the daemon to shut down cleanly.
    Shutdown,
    /// Send an ask request with the invoking tmux pane (if in tmux).
    /// `session_id` is set by `run_chat` to maintain conversational memory across turns.
    /// `chat_pane` is the pane ID of the AI chat pane itself (i.e. `$TMUX_PANE` inside
    /// `daemoneye chat`) so the daemon can switch focus back to it after a foreground sudo command.
    Ask {
        query: String,
        tmux_pane: Option<String>,
        session_id: Option<String>,
        chat_pane: Option<String>,
        /// Optional prompt override — name of a prompt in ~/.daemoneye/prompts/.
        /// When set, the daemon uses this instead of the configured default.
        prompt: Option<String>,
        /// Width of the chat pane in columns (terminal_width() value from the client).
        /// Passed to the AI so it formats prose for the actual display width.
        #[serde(default)]
        chat_width: Option<usize>,
    },
    /// Approve or deny a tool call.
    ToolCallResponse { id: String, approved: bool },
    /// User-supplied credential (password / passphrase) in response to
    /// `Response::CredentialPrompt`. The daemon injects it into the PTY.
    CredentialResponse { id: String, credential: String },
    /// User's yes/no decision in response to `Response::ConfirmationPrompt`.
    /// `accepted = true` → "yes" injected; `false` → "no".
    ConfirmationResponse { id: String, accepted: bool },
    /// Re-collect the system context (OS info, memory, processes, history).
    /// Daemon responds with Response::Ok when done.
    Refresh,
}

/// Messages sent from the daemon back to the CLI client.
#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    /// Acknowledgment of a successful request.
    Ok,
    /// An error occurred on the daemon side.
    Error(String),
    /// Sent once before streaming begins; carries the number of stored messages
    /// from prior turns so the client can display a session indicator.
    SessionInfo { message_count: usize },
    /// A stream of tokens from the AI.
    Token(String),
    /// A system-level notification from the daemon (sudo alerts, pane-switch
    /// notices, etc.).  Displayed with a distinct amber prefix.
    SystemMsg(String),
    /// A prompt for the user to approve a tool call.
    ToolCallPrompt { id: String, command: String, background: bool },
    /// The background PTY command is waiting for a credential (password / passphrase).
    /// The client MUST prompt the user with echo disabled and return a
    /// `Request::CredentialResponse`.
    CredentialPrompt { id: String, prompt: String },
    /// The background PTY command is waiting for a yes/no confirmation (e.g. SSH host-key).
    /// The client MUST display `message` and return a `Request::ConfirmationResponse`.
    ConfirmationPrompt { id: String, message: String },
    /// The output captured after an approved tool call completes.
    /// Sent to the client so it can display a dimmed result block.
    ToolResult(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip_req(req: &Request) -> Request {
        let json = serde_json::to_string(req).expect("serialize");
        serde_json::from_str(&json).expect("deserialize")
    }

    fn roundtrip_resp(resp: &Response) -> Response {
        let json = serde_json::to_string(resp).expect("serialize");
        serde_json::from_str(&json).expect("deserialize")
    }

    // ── Request round-trips ──────────────────────────────────────────────────

    #[test]
    fn request_ping_roundtrip() {
        assert!(matches!(roundtrip_req(&Request::Ping), Request::Ping));
    }

    #[test]
    fn request_shutdown_roundtrip() {
        assert!(matches!(roundtrip_req(&Request::Shutdown), Request::Shutdown));
    }

    #[test]
    fn request_refresh_roundtrip() {
        assert!(matches!(roundtrip_req(&Request::Refresh), Request::Refresh));
    }

    #[test]
    fn request_ask_roundtrip() {
        let req = Request::Ask {
            query: "what is load avg?".to_string(),
            tmux_pane: Some("%3".to_string()),
            session_id: Some("deadbeef".to_string()),
            chat_pane: Some("%4".to_string()),
            prompt: Some("sre".to_string()),
            chat_width: Some(54),
        };
        match roundtrip_req(&req) {
            Request::Ask { query, tmux_pane, session_id, chat_pane, prompt, chat_width } => {
                assert_eq!(query, "what is load avg?");
                assert_eq!(tmux_pane, Some("%3".to_string()));
                assert_eq!(session_id, Some("deadbeef".to_string()));
                assert_eq!(chat_pane, Some("%4".to_string()));
                assert_eq!(prompt, Some("sre".to_string()));
                assert_eq!(chat_width, Some(54));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_ask_optional_fields_none() {
        let req = Request::Ask {
            query: "hi".to_string(),
            tmux_pane: None,
            session_id: None,
            chat_pane: None,
            prompt: None,
            chat_width: None,
        };
        match roundtrip_req(&req) {
            Request::Ask { tmux_pane, session_id, chat_pane, prompt, chat_width, .. } => {
                assert!(tmux_pane.is_none());
                assert!(session_id.is_none());
                assert!(chat_pane.is_none());
                assert!(prompt.is_none());
                assert!(chat_width.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_ask_backward_compat_no_prompt_field() {
        // Simulate a message from an old client that omits the `prompt` field.
        let json = r#"{"Ask":{"query":"hi","tmux_pane":null,"session_id":null,"chat_pane":null}}"#;
        let parsed: Request = serde_json::from_str(json).expect("backward-compat deserialize");
        match parsed {
            Request::Ask { prompt, .. } => assert!(prompt.is_none()),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_tool_call_response_roundtrip() {
        let req = Request::ToolCallResponse { id: "tc_1".to_string(), approved: true };
        match roundtrip_req(&req) {
            Request::ToolCallResponse { id, approved } => {
                assert_eq!(id, "tc_1");
                assert!(approved);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_credential_response_roundtrip() {
        let req = Request::CredentialResponse { id: "tc_2".to_string(), credential: "hunter2".to_string() };
        match roundtrip_req(&req) {
            Request::CredentialResponse { id, credential } => {
                assert_eq!(id, "tc_2");
                assert_eq!(credential, "hunter2");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_confirmation_response_roundtrip() {
        let req = Request::ConfirmationResponse { id: "tc_3".to_string(), accepted: true };
        match roundtrip_req(&req) {
            Request::ConfirmationResponse { id, accepted } => {
                assert_eq!(id, "tc_3");
                assert!(accepted);
            }
            _ => panic!("wrong variant"),
        }
    }

    // ── Response round-trips ─────────────────────────────────────────────────

    #[test]
    fn response_ok_roundtrip() {
        assert!(matches!(roundtrip_resp(&Response::Ok), Response::Ok));
    }

    #[test]
    fn response_error_roundtrip() {
        let resp = Response::Error("something broke".to_string());
        match roundtrip_resp(&resp) {
            Response::Error(msg) => assert_eq!(msg, "something broke"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_session_info_roundtrip() {
        let resp = Response::SessionInfo { message_count: 7 };
        match roundtrip_resp(&resp) {
            Response::SessionInfo { message_count } => assert_eq!(message_count, 7),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_token_roundtrip() {
        let resp = Response::Token("Hello".to_string());
        match roundtrip_resp(&resp) {
            Response::Token(t) => assert_eq!(t, "Hello"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_tool_call_prompt_roundtrip() {
        let resp = Response::ToolCallPrompt {
            id: "tc_3".to_string(),
            command: "ls -la".to_string(),
            background: false,
        };
        match roundtrip_resp(&resp) {
            Response::ToolCallPrompt { id, command, background } => {
                assert_eq!(id, "tc_3");
                assert_eq!(command, "ls -la");
                assert!(!background);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_credential_prompt_roundtrip() {
        let resp = Response::CredentialPrompt { id: "tc_4".to_string(), prompt: "[sudo] password for alice:".to_string() };
        match roundtrip_resp(&resp) {
            Response::CredentialPrompt { id, prompt } => {
                assert_eq!(id, "tc_4");
                assert_eq!(prompt, "[sudo] password for alice:");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_confirmation_prompt_roundtrip() {
        let resp = Response::ConfirmationPrompt { id: "tc_5".to_string(), message: "Are you sure? (yes/no)".to_string() };
        match roundtrip_resp(&resp) {
            Response::ConfirmationPrompt { id, message } => {
                assert_eq!(id, "tc_5");
                assert_eq!(message, "Are you sure? (yes/no)");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_tool_result_roundtrip() {
        let resp = Response::ToolResult("output here".to_string());
        match roundtrip_resp(&resp) {
            Response::ToolResult(s) => assert_eq!(s, "output here"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn invalid_json_returns_error() {
        let result: Result<Request, _> = serde_json::from_str("not json at all");
        assert!(result.is_err());
    }
}
