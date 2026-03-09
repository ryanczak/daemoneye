use serde::{Deserialize, Serialize};

/// The default path for the DaemonEye IPC socket.
pub const DEFAULT_SOCKET_PATH: &str = "/tmp/daemoneye.sock";

/// A snapshot of a single tmux pane, sent in `PaneSelectPrompt` so the client
/// can display a numbered list for the user to choose from.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PaneInfo {
    pub id: String,
    pub current_cmd: String,
    pub summary: String,
}

/// Summary of a scheduled job for the `ScheduleList` response.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ScheduleListItem {
    pub id: String,
    pub name: String,
    /// Human-readable schedule kind (e.g. "every 5m", "once at 2026-03-01 12:00 UTC").
    pub kind: String,
    /// Human-readable action description.
    pub action: String,
    /// Human-readable status.
    pub status: String,
    pub last_run: Option<String>,
    pub next_run: Option<String>,
}

/// Summary of a script file for the `ScriptList` response.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ScriptListItem {
    pub name: String,
    pub size: u64,
}

/// Summary of a runbook for the `RunbookList` response.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RunbookListItem {
    pub name: String,
    pub tags: Vec<String>,
}

/// An entry in the `MemoryList` response.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MemoryListItem {
    pub category: String,
    pub key: String,
}

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
        /// The tmux session the client is running in, resolved by the client before
        /// connecting. Used by the daemon to adopt (or confirm) the correct session
        /// when started by systemd before any user session existed.
        #[serde(default)]
        tmux_session: Option<String>,
        /// The target pane for foreground commands, resolved client-side by sibling
        /// detection or user prompt. Eliminates mid-conversation pane picker prompts.
        #[serde(default)]
        target_pane: Option<String>,
    },
    /// Approve or deny a tool call.  When `approved` is false and `user_message`
    /// is `Some`, the daemon discards the pending tool chain and injects the
    /// message as a new user turn so the AI can course-correct.
    ToolCallResponse {
        id: String,
        approved: bool,
        /// Optional corrective message typed at the approval prompt.
        /// Present only when the user wants to redirect the agent.
        #[serde(default)]
        user_message: Option<String>,
    },
    /// User-supplied credential (password / passphrase) in response to
    /// `Response::CredentialPrompt`. The daemon injects it into the background tmux window.
    CredentialResponse { id: String, credential: String },
    /// User's pane selection in response to `Response::PaneSelectPrompt`.
    PaneSelectResponse { id: String, pane_id: String },
    /// Re-collect the system context (OS info, memory, processes, history).
    /// Daemon responds with Response::Ok when done.
    Refresh,
    /// Approve or deny a script write proposed by the AI.
    ScriptWriteResponse { id: String, approved: bool },
    /// Approve or deny a job schedule proposed by the AI.
    ScheduleWriteResponse { id: String, approved: bool },
    /// Approve or deny a runbook write proposed by the AI.
    RunbookWriteResponse { id: String, approved: bool },
    /// Approve or deny a runbook delete proposed by the AI.
    RunbookDeleteResponse { id: String, approved: bool },
    /// Notify the daemon of an event (e.g. background pane activity from a tmux hook).
    NotifyActivity {
        pane_id: String,
        hook_index: usize,
        session_name: String,
    },
    /// Notify the daemon that a background command finished.
    /// Carries the exit code directly so no scrollback scan is needed.
    NotifyComplete {
        pane_id: String,
        exit_code: i32,
        session_name: String,
    },
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
    ToolCallPrompt {
        id: String,
        command: String,
        background: bool,
    },
    /// The approved background command requires a credential (sudo password, etc.).
    /// The client MUST prompt the user with echo disabled and return a `CredentialResponse`.
    CredentialPrompt { id: String, prompt: String },
    /// The output captured after an approved tool call completes.
    /// Sent to the client so it can display a dimmed result block.
    ToolResult(String),
    /// Daemon cannot determine the target pane and needs the user to choose.
    /// Client displays the list and returns a `Request::PaneSelectResponse`.
    PaneSelectPrompt { id: String, panes: Vec<PaneInfo> },
    /// The AI wants to write a script; the client MUST show the content and
    /// prompt the user for approval, then return `Request::ScriptWriteResponse`.
    ScriptWritePrompt {
        id: String,
        script_name: String,
        content: String,
    },
    /// The AI wants to schedule a job; the client MUST show the details and
    /// prompt the user for approval, then return `Request::ScheduleWriteResponse`.
    ScheduleWritePrompt {
        id: String,
        name: String,
        kind: String,
        action: String,
    },
    /// The current list of scheduled jobs.
    ScheduleList { jobs: Vec<ScheduleListItem> },
    /// The current list of scripts in `~/.daemoneye/scripts/`.
    ScriptList { scripts: Vec<ScriptListItem> },
    /// The AI wants to write a runbook; the client MUST show the content and
    /// prompt the user for approval, then return `Request::RunbookWriteResponse`.
    RunbookWritePrompt {
        id: String,
        runbook_name: String,
        content: String,
    },
    /// The AI wants to delete a runbook; the client MUST show affected jobs and
    /// prompt the user for approval, then return `Request::RunbookDeleteResponse`.
    RunbookDeletePrompt {
        id: String,
        runbook_name: String,
        /// Names of scheduled jobs that reference this runbook.
        active_jobs: Vec<String>,
    },
    /// The current list of runbooks in `~/.daemoneye/runbooks/`.
    RunbookList { runbooks: Vec<RunbookListItem> },
    /// The current list of memory entries.
    MemoryList { entries: Vec<MemoryListItem> },
    /// Sent after each AI turn completes, carrying the prompt token count from
    /// that turn. The client uses this to update the context-budget display.
    UsageUpdate { prompt_tokens: u32 },
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
        assert!(matches!(
            roundtrip_req(&Request::Shutdown),
            Request::Shutdown
        ));
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
            tmux_session: Some("mysession".to_string()),
            target_pane: Some("%1".to_string()),
        };
        match roundtrip_req(&req) {
            Request::Ask {
                query,
                tmux_pane,
                session_id,
                chat_pane,
                prompt,
                chat_width,
                tmux_session,
                target_pane,
            } => {
                assert_eq!(query, "what is load avg?");
                assert_eq!(tmux_pane, Some("%3".to_string()));
                assert_eq!(session_id, Some("deadbeef".to_string()));
                assert_eq!(chat_pane, Some("%4".to_string()));
                assert_eq!(prompt, Some("sre".to_string()));
                assert_eq!(chat_width, Some(54));
                assert_eq!(tmux_session, Some("mysession".to_string()));
                assert_eq!(target_pane, Some("%1".to_string()));
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
            tmux_session: None,
            target_pane: None,
        };
        match roundtrip_req(&req) {
            Request::Ask {
                tmux_pane,
                session_id,
                chat_pane,
                prompt,
                chat_width,
                ..
            } => {
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
        let req = Request::ToolCallResponse {
            id: "tc_1".to_string(),
            approved: true,
            user_message: None,
        };
        match roundtrip_req(&req) {
            Request::ToolCallResponse { id, approved, user_message } => {
                assert_eq!(id, "tc_1");
                assert!(approved);
                assert!(user_message.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_tool_call_response_with_user_message_roundtrip() {
        let req = Request::ToolCallResponse {
            id: "tc_2".to_string(),
            approved: false,
            user_message: Some("don't do that, try a safer approach".to_string()),
        };
        match roundtrip_req(&req) {
            Request::ToolCallResponse { id, approved, user_message } => {
                assert_eq!(id, "tc_2");
                assert!(!approved);
                assert_eq!(user_message.as_deref(), Some("don't do that, try a safer approach"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_tool_call_response_backward_compat_no_user_message() {
        // Old clients omit user_message; default should be None.
        let json = r#"{"ToolCallResponse":{"id":"tc_3","approved":false}}"#;
        let parsed: Request = serde_json::from_str(json).expect("backward-compat deserialize");
        match parsed {
            Request::ToolCallResponse { user_message, .. } => assert!(user_message.is_none()),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_credential_response_roundtrip() {
        let req = Request::CredentialResponse {
            id: "tc_2".to_string(),
            credential: "hunter2".to_string(),
        };
        match roundtrip_req(&req) {
            Request::CredentialResponse { id, credential } => {
                assert_eq!(id, "tc_2");
                assert_eq!(credential, "hunter2");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_notify_activity_roundtrip() {
        let req = Request::NotifyActivity {
            pane_id: "%3".to_string(),
            hook_index: 42,
            session_name: "test_session".to_string(),
        };
        match roundtrip_req(&req) {
            Request::NotifyActivity { pane_id, .. } => {
                assert_eq!(pane_id, "%3");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_notify_complete_roundtrip() {
        let req = Request::NotifyComplete {
            pane_id: "%5".to_string(),
            exit_code: 42,
            session_name: "test_session".to_string(),
        };
        match roundtrip_req(&req) {
            Request::NotifyComplete { pane_id, exit_code, .. } => {
                assert_eq!(pane_id, "%5");
                assert_eq!(exit_code, 42);
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
            Response::ToolCallPrompt {
                id,
                command,
                background,
            } => {
                assert_eq!(id, "tc_3");
                assert_eq!(command, "ls -la");
                assert!(!background);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_credential_prompt_roundtrip() {
        let resp = Response::CredentialPrompt {
            id: "tc_4".to_string(),
            prompt: "[sudo] password for alice:".to_string(),
        };
        match roundtrip_resp(&resp) {
            Response::CredentialPrompt { id, prompt } => {
                assert_eq!(id, "tc_4");
                assert_eq!(prompt, "[sudo] password for alice:");
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
    fn request_pane_select_response_roundtrip() {
        let req = Request::PaneSelectResponse {
            id: "ps_1".to_string(),
            pane_id: "%3".to_string(),
        };
        match roundtrip_req(&req) {
            Request::PaneSelectResponse { id, pane_id } => {
                assert_eq!(id, "ps_1");
                assert_eq!(pane_id, "%3");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_pane_select_prompt_roundtrip() {
        let resp = Response::PaneSelectPrompt {
            id: "ps_2".to_string(),
            panes: vec![
                PaneInfo {
                    id: "%1".to_string(),
                    current_cmd: "bash".to_string(),
                    summary: "idle shell".to_string(),
                },
                PaneInfo {
                    id: "%3".to_string(),
                    current_cmd: "vim".to_string(),
                    summary: "editing file".to_string(),
                },
            ],
        };
        match roundtrip_resp(&resp) {
            Response::PaneSelectPrompt { id, panes } => {
                assert_eq!(id, "ps_2");
                assert_eq!(panes.len(), 2);
                assert_eq!(panes[0].id, "%1");
                assert_eq!(panes[0].current_cmd, "bash");
                assert_eq!(panes[1].id, "%3");
                assert_eq!(panes[1].current_cmd, "vim");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_script_write_response_roundtrip() {
        let req = Request::ScriptWriteResponse {
            id: "sw_1".to_string(),
            approved: true,
        };
        match roundtrip_req(&req) {
            Request::ScriptWriteResponse { id, approved } => {
                assert_eq!(id, "sw_1");
                assert!(approved);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_script_write_prompt_roundtrip() {
        let resp = Response::ScriptWritePrompt {
            id: "sw_2".to_string(),
            script_name: "check-disk.sh".to_string(),
            content: "#!/bin/bash\ndf -h".to_string(),
        };
        match roundtrip_resp(&resp) {
            Response::ScriptWritePrompt {
                id,
                script_name,
                content,
            } => {
                assert_eq!(id, "sw_2");
                assert_eq!(script_name, "check-disk.sh");
                assert!(content.contains("df -h"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_schedule_write_response_roundtrip() {
        let req = Request::ScheduleWriteResponse {
            id: "sch_1".to_string(),
            approved: true,
        };
        match roundtrip_req(&req) {
            Request::ScheduleWriteResponse { id, approved } => {
                assert_eq!(id, "sch_1");
                assert!(approved);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_schedule_write_prompt_roundtrip() {
        let resp = Response::ScheduleWritePrompt {
            id: "sch_2".to_string(),
            name: "MyJob".to_string(),
            kind: "every 5m".to_string(),
            action: "echo Hello".to_string(),
        };
        match roundtrip_resp(&resp) {
            Response::ScheduleWritePrompt {
                id,
                name,
                kind,
                action,
            } => {
                assert_eq!(id, "sch_2");
                assert_eq!(name, "MyJob");
                assert_eq!(kind, "every 5m");
                assert_eq!(action, "echo Hello");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_schedule_list_roundtrip() {
        let resp = Response::ScheduleList {
            jobs: vec![ScheduleListItem {
                id: "job-1".to_string(),
                name: "disk-check".to_string(),
                kind: "every 5m".to_string(),
                action: "cmd: df -h".to_string(),
                status: "pending".to_string(),
                last_run: None,
                next_run: Some("2026-03-01 12:00 UTC".to_string()),
            }],
        };
        match roundtrip_resp(&resp) {
            Response::ScheduleList { jobs } => {
                assert_eq!(jobs.len(), 1);
                assert_eq!(jobs[0].name, "disk-check");
                assert_eq!(jobs[0].next_run, Some("2026-03-01 12:00 UTC".to_string()));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_script_list_roundtrip() {
        let resp = Response::ScriptList {
            scripts: vec![
                ScriptListItem {
                    name: "check-disk.sh".to_string(),
                    size: 42,
                },
                ScriptListItem {
                    name: "monitor.sh".to_string(),
                    size: 128,
                },
            ],
        };
        match roundtrip_resp(&resp) {
            Response::ScriptList { scripts } => {
                assert_eq!(scripts.len(), 2);
                assert_eq!(scripts[0].name, "check-disk.sh");
                assert_eq!(scripts[0].size, 42);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn invalid_json_returns_error() {
        let result: Result<Request, _> = serde_json::from_str("not json at all");
        assert!(result.is_err());
    }
}
