use serde::{Deserialize, Serialize};

/// The default path for the T1000 IPC socket.
pub const DEFAULT_SOCKET_PATH: &str = "/tmp/t1000.sock";

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
    /// `t1000 chat`) so the daemon can switch focus back to it after a foreground sudo command.
    Ask { query: String, tmux_pane: Option<String>, session_id: Option<String>, chat_pane: Option<String> },
    /// Approve or deny a tool call.
    ToolCallResponse { id: String, approved: bool },
    /// Respond to a sudo password prompt from the daemon.
    SudoPassword { id: String, password: String },
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
