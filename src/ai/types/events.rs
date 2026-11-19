use super::PendingCall;
use super::wire::TokenBreakdown;

#[derive(Debug)]
pub enum AiEvent {
    Token(String),
    /// `run_terminal_command`: carried separately because chat and ghost
    /// loops route `wait` and `interactive` into `PendingCall::Foreground`,
    /// `PendingCall::Background`, or `PendingCall::InvalidArgs`.
    ToolCall {
        id: String,
        cmd: String,
        wait: Option<bool>,
        interactive: bool,
        thought_signature: Option<String>,
        shell: Option<String>,
        host: Option<String>,
        timeout_secs: Option<u64>,
    },
    /// Every other parsed tool call (including `PendingCall::InvalidArgs` for
    /// a call to a known tool whose arguments failed schema validation).
    Tool(PendingCall),
    Done(TokenBreakdown),
    Error(String),
    /// Non-terminal advisory the user should see (truncation, refusal,
    /// dropped/unknown tool call, malformed provider frames). Forwarded to the
    /// chat client as a SystemMsg; log-and-ignore in non-interactive consumers.
    Notice(String),
}
