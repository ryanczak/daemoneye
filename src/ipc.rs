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
    },
    /// Approve or deny a tool call.
    ToolCallResponse { id: String, approved: bool },
    /// Respond to a sudo password prompt from the daemon.
    SudoPassword { id: String, password: String },
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
    /// The approved background command requires sudo — prompt the user for their password.
    SudoPrompt { id: String, command: String },
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
        };
        match roundtrip_req(&req) {
            Request::Ask { query, tmux_pane, session_id, chat_pane, prompt } => {
                assert_eq!(query, "what is load avg?");
                assert_eq!(tmux_pane, Some("%3".to_string()));
                assert_eq!(session_id, Some("deadbeef".to_string()));
                assert_eq!(chat_pane, Some("%4".to_string()));
                assert_eq!(prompt, Some("sre".to_string()));
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
        };
        match roundtrip_req(&req) {
            Request::Ask { tmux_pane, session_id, chat_pane, prompt, .. } => {
                assert!(tmux_pane.is_none());
                assert!(session_id.is_none());
                assert!(chat_pane.is_none());
                assert!(prompt.is_none());
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
    fn request_sudo_password_roundtrip() {
        let req = Request::SudoPassword { id: "tc_2".to_string(), password: "hunter2".to_string() };
        match roundtrip_req(&req) {
            Request::SudoPassword { id, password } => {
                assert_eq!(id, "tc_2");
                assert_eq!(password, "hunter2");
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
    fn response_sudo_prompt_roundtrip() {
        let resp = Response::SudoPrompt { id: "tc_4".to_string(), command: "sudo apt update".to_string() };
        match roundtrip_resp(&resp) {
            Response::SudoPrompt { id, command } => {
                assert_eq!(id, "tc_4");
                assert_eq!(command, "sudo apt update");
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
