use crate::ai::{AiEvent, Message, PendingCall, ToolResult, make_client};
use crate::config::{Config, PricingSource};
use crate::cost::CostAttribution;
use crate::daemon::auto_name;
use crate::daemon::executor::{self, SessionCtx};
use crate::daemon::session::{
    SessionStore, append_session_message, with_sessions, write_session_file,
};
use crate::daemon::utils::{log_event, send_response_split};
use crate::ipc::Response;
use crate::scheduler::ScheduleStore;
use crate::tmux::cache::SessionCache;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufRead, AsyncWrite};

mod conversation;
mod endings;

pub use conversation::run_conversation_loop;

// Bounded re-issue when the chat task dies without a terminal event
// (panic / backend bug). Without the bound this was an infinite silent
// retry loop — the user saw only KeepAlives while API calls burned.
const MAX_CHANNEL_CLOSED_RETRIES: u32 = 2;

/// Approval-gated tool names. Tools in this list bypass the per-turn,
/// per-session and per-tool budget caps because the user's per-call approval
/// prompt is the gate instead.
///
/// Re-exported from `crate::ai::tools::APPROVAL_GATED_TOOLS`, which is the
/// single source of truth for "this tool prompts the user". It used to be a
/// hand-maintained copy that had drifted in both directions — see that
/// constant's doc comment.
use crate::ai::tools::APPROVAL_GATED_TOOLS as APPROVAL_GATED;

/// The `stage` recorded in `RunState::Cancelled`: the door the cancellation
/// came through. Both doors fire `CancelReason::UserStop`, so the reason
/// cannot tell them apart — the mark each door leaves before firing can.
fn cancel_stage(run_id: &crate::daemon::run::RunId) -> String {
    crate::daemon::run::take_cancel_door(run_id)
        .map(crate::daemon::run::CancelDoor::stage)
        .unwrap_or("unmarked_cancel")
        .to_string()
}

/// Only an explicit `wait: false` skips the wait; an omitted `wait` waits.
fn routes_to_background(wait: Option<bool>, interactive: bool) -> bool {
    !interactive && wait == Some(false)
}

/// Whether this call skips the per-turn, per-session and per-tool caps: a
/// non-`InvalidArgs` call to an approval-gated tool. A call whose arguments
/// failed schema validation prompts for nothing and runs nothing, so the
/// model-visible mistake must still count against the caps — otherwise a
/// model stuck sending bad arguments to `edit_file` is uncapped (M29
/// phase-01).
fn is_budget_exempt(call: &PendingCall) -> bool {
    !matches!(call, PendingCall::InvalidArgs { .. }) && APPROVAL_GATED.contains(&call.tool_name())
}

/// Run the AI conversation loop: stream tokens, collect tool calls, execute,
/// and repeat until the AI produces a final answer with no pending tool calls.
///
/// This is the core inner loop of the daemon. It handles:
/// - Event streaming from the LLM (with keep-alive on timeout)
/// - Tool call collection and budget enforcement
/// - Tool execution (foreground, background, ghost spawn)
/// - Response persistence (in-memory and on-disk)
/// - Auto-name suggestion for unnamed sessions
pub struct ConversationLoopCtx<'a> {
    pub session_id: Option<String>,
    pub session_name: &'a str,
    pub chat_pane: Option<String>,
    pub messages: Vec<Message>,
    pub sys_prompt: String,
    pub session_active_model: Option<String>,
    pub is_ghost_session: bool,
    pub this_turn_count: usize,
    pub post_trim_len: usize,
    pub needs_compaction: bool,
    pub wants_background_compaction: bool,
    pub config: &'a Config,
    pub cache: Arc<SessionCache>,
    pub sessions: SessionStore,
    pub schedule_store: Arc<ScheduleStore>,
    pub cost_attribution: CostAttribution,
}

/// Truncate tool results for history storage when they exceed the character cap.
fn truncate_tool_results(
    tool_results: Vec<ToolResult>,
    char_cap: Option<usize>,
) -> Vec<ToolResult> {
    tool_results
        .into_iter()
        .map(|r| match char_cap {
            Some(cap) if r.content.len() > cap => {
                let mut end = cap;
                while !r.content.is_char_boundary(end) {
                    end -= 1;
                }
                ToolResult {
                    tool_call_id: r.tool_call_id,
                    tool_name: r.tool_name,
                    content: format!(
                        "{}\n[truncated — {} chars total; full output archived in pane log]",
                        &r.content[..end],
                        r.content.len()
                    ),
                }
            }
            _ => r,
        })
        .collect()
}

/// Abort the in-flight provider stream when the turn ends early for any
/// reason (error return, client gone, deadline). Without this the spawned
/// chat task keeps consuming (and billing) the provider stream with nobody
/// listening.
struct ChatTaskGuard(Option<tokio::task::JoinHandle<()>>);

impl ChatTaskGuard {
    fn new(handle: tokio::task::JoinHandle<()>) -> Self {
        ChatTaskGuard(Some(handle))
    }

    /// Reap the task and describe how it ended — used when the event channel
    /// closed without a terminal `Done`/`Error` event.
    async fn describe_end(&mut self) -> String {
        match self.0.take() {
            Some(handle) => match handle.await {
                Ok(()) => "backend returned without sending Done or Error \
                           (backend bug)"
                    .to_string(),
                Err(e) if e.is_panic() => {
                    format!("chat task panicked: {}", panic_message(e))
                }
                Err(e) => format!("chat task was cancelled: {e}"),
            },
            None => "chat task already reaped".to_string(),
        }
    }
}

impl Drop for ChatTaskGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

/// Extract the payload string from a panicked task's JoinError.
fn panic_message(e: tokio::task::JoinError) -> String {
    match e.try_into_panic() {
        Ok(payload) => payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic payload".to_string()),
        Err(e) => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_invalid_args_call_is_never_budget_exempt() {
        let invalid = PendingCall::InvalidArgs {
            id: "tc_1".into(),
            name: "edit_file",
            arguments: "{}".into(),
            error: "bad args".into(),
            thought_signature: None,
        };
        assert!(
            !is_budget_exempt(&invalid),
            "an InvalidArgs call naming a gated tool is never exempt"
        );

        let edit = PendingCall::EditFile {
            id: "tc_2".into(),
            thought_signature: None,
            path: "/tmp/f".into(),
            operation: "replace".into(),
            old_string: None,
            new_string: None,
            content: None,
            dest_path: None,
            host: None,
            sudo: None,
        };
        assert!(is_budget_exempt(&edit), "a gated tool call is exempt");

        let read = PendingCall::ReadFile {
            id: "tc_3".into(),
            thought_signature: None,
            path: "/tmp/f".into(),
            offset: None,
            limit: None,
            pattern: None,
            host: None,
        };
        assert!(!is_budget_exempt(&read), "a non-gated call is not exempt");
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_stage_distinguishes_the_two_doors() {
        use crate::daemon::run::cancel::test_support::{HomeGuard, fresh_run_id};
        use crate::daemon::run::{
            CancelReason, cancel_session, register_active, register_run, request_stop,
            watch_stop_sentinel,
        };
        let _home = HomeGuard::new();

        let chat_run = fresh_run_id();
        let (_chat_cancel, chat_signal) = register_run(&chat_run);
        let _chat_active = register_active("stream-cancel-stage-chat", &chat_run);
        assert!(cancel_session(
            "stream-cancel-stage-chat",
            CancelReason::UserStop
        ));
        let chat_stage = cancel_stage(&chat_run);

        let sentinel_run = fresh_run_id();
        let (_sentinel_cancel, sentinel_signal) = register_run(&sentinel_run);
        let watcher = tokio::spawn(watch_stop_sentinel(
            sentinel_run.clone(),
            std::time::Duration::from_millis(10),
        ));
        request_stop(sentinel_run.as_str()).expect("sentinel written before the watcher polls");
        tokio::task::yield_now().await;
        for _ in 0..3 {
            tokio::time::advance(std::time::Duration::from_millis(10)).await;
            tokio::task::yield_now().await;
        }
        assert!(
            watcher.is_finished(),
            "the watcher must fire on the sentinel"
        );
        let sentinel_stage = cancel_stage(&sentinel_run);

        assert_eq!(chat_signal.reason(), Some(CancelReason::UserStop));
        assert_eq!(
            sentinel_signal.reason(),
            Some(CancelReason::UserStop),
            "both doors fire the same reason — the stage is what must differ"
        );
        assert_ne!(
            chat_stage, sentinel_stage,
            "the chat door and the sentinel door must record different stages"
        );
        assert_eq!(
            cancel_stage(&chat_run),
            "unmarked_cancel",
            "a consumed mark must not be read twice"
        );
    }

    #[test]
    fn approval_gated_contains_expected_tools() {
        assert!(APPROVAL_GATED.contains(&"run_terminal_command"));
        assert!(APPROVAL_GATED.contains(&"edit_file"));
        assert!(APPROVAL_GATED.contains(&"write_script"));
        assert!(!APPROVAL_GATED.contains(&"read_file"));
        assert!(!APPROVAL_GATED.contains(&"get_shell_context"));

        // The 2026-08-08 reconciliation, pinned in both directions. These four
        // are the tools whose budget treatment changed when the exemption
        // stopped being a hand-maintained copy and became the prompting set.
        // `spawn_ghost_shell` and `delete_schedule` prompt for nothing, so a
        // cap can now reach them; `create_agent` and `delete_agent` do prompt,
        // so a cap no longer can.
        assert!(
            !APPROVAL_GATED.contains(&"spawn_ghost_shell"),
            "spawn_ghost_shell sends no approval prompt — it must be cappable"
        );
        assert!(
            !APPROVAL_GATED.contains(&"delete_schedule"),
            "delete_schedule sends no approval prompt — it must be cappable"
        );
        assert!(
            APPROVAL_GATED.contains(&"create_agent"),
            "create_agent prompts the user, so the caps must not also apply"
        );
        assert!(
            APPROVAL_GATED.contains(&"delete_agent"),
            "delete_agent prompts the user, so the caps must not also apply"
        );
    }

    #[test]
    fn omitting_wait_runs_in_the_foreground() {
        assert!(
            !routes_to_background(None, false),
            "an omitted `wait` waits — the PTY foreground path is the default"
        );
    }

    #[test]
    fn wait_false_runs_in_the_background() {
        assert!(
            routes_to_background(Some(false), false),
            "`wait: false` is the old `background: true`"
        );
    }

    #[test]
    fn wait_true_runs_in_the_foreground() {
        assert!(
            !routes_to_background(Some(true), false),
            "an explicit `wait: true` waits — a `wait.is_some()` routing would invert this"
        );
    }

    #[test]
    fn an_interactive_call_is_never_backgrounded() {
        assert!(
            !routes_to_background(Some(false), true),
            "interactive: true takes the waited foreground launch path even with \
             wait: false"
        );
        assert!(
            routes_to_background(Some(false), false),
            "without interactive, wait: false still backgrounds"
        );
    }

    #[test]
    fn truncate_tool_results_noop_when_under_cap() {
        let results = vec![ToolResult {
            tool_call_id: "1".to_string(),
            tool_name: "test".to_string(),
            content: "short".to_string(),
        }];
        let truncated = truncate_tool_results(results, Some(100));
        assert_eq!(truncated.len(), 1);
        assert_eq!(truncated[0].content, "short");
    }

    #[test]
    fn truncate_tool_results_truncates_over_cap() {
        let results = vec![ToolResult {
            tool_call_id: "1".to_string(),
            tool_name: "test".to_string(),
            content: "a".repeat(200),
        }];
        let truncated = truncate_tool_results(results, Some(50));
        assert_eq!(truncated.len(), 1);
        assert!(truncated[0].content.contains("[truncated"));
        assert!(truncated[0].content.contains("200 chars total"));
    }

    #[test]
    fn truncate_tool_results_noop_when_no_cap() {
        let results = vec![ToolResult {
            tool_call_id: "1".to_string(),
            tool_name: "test".to_string(),
            content: "a".repeat(200),
        }];
        let truncated = truncate_tool_results(results, None);
        assert_eq!(truncated.len(), 1);
        assert_eq!(truncated[0].content.len(), 200);
    }

    #[tokio::test]
    async fn panicking_chat_task_is_classified() {
        let handle = tokio::spawn(async { panic!("boom") });
        let mut guard = ChatTaskGuard::new(handle);
        let description = guard.describe_end().await;
        assert!(
            description.contains("panicked"),
            "expected panic classification, got: {description}"
        );
        assert!(
            description.contains("boom"),
            "expected panic payload, got: {description}"
        );
    }

    #[tokio::test]
    async fn clean_return_without_done_is_named_backend_bug() {
        let handle = tokio::spawn(async {});
        let mut guard = ChatTaskGuard::new(handle);
        let description = guard.describe_end().await;
        assert!(
            description.contains("without sending Done"),
            "expected backend-bug classification, got: {description}"
        );
    }

    #[tokio::test]
    async fn guard_drop_aborts_task() {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(async move {
            let _tx = tx;
            std::future::pending::<()>().await;
        });
        let guard = ChatTaskGuard::new(handle);
        drop(guard);
        tokio::task::yield_now().await;
        assert!(rx.await.is_err(), "oneshot should be dropped by the abort");
    }
}
