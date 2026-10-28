//! `execute_tool_call`'s arms for the terminal-context tool: `GetShellContext`.
//! A child module of `src/daemon/executor/mod.rs`.

use super::*;

pub(super) fn dispatch(
    call: &PendingCall,
    session_id: Option<&str>,
    chat_pane: Option<&str>,
    cache: &Arc<SessionCache>,
) -> anyhow::Result<ToolCallOutcome> {
    match call {
        PendingCall::GetShellContext { scope, .. } => 'arm: {
            let breadth = match knowledge::resolve_context_scope(scope.as_deref()) {
                Ok(b) => b,
                Err(msg) => break 'arm Ok(ToolCallOutcome::Result(msg)),
            };
            if breadth == knowledge::ContextBreadth::ShellsOnly {
                break 'arm Ok(ToolCallOutcome::Result(knowledge::shells_only_context(
                    &Owner::Chat(session_id.unwrap_or_default().to_string()),
                )));
            }
            // PE decision 1: a caller outside tmux gets the daemon's own
            // shells only — no pane blocks, no pane map.
            if chat_pane.is_none() {
                break 'arm Ok(ToolCallOutcome::Result(knowledge::shells_only_context(
                    &Owner::Chat(session_id.unwrap_or_default().to_string()),
                )));
            }
            let ctx_scope = match breadth {
                knowledge::ContextBreadth::All => crate::tmux::cache::ContextScope::All,
                _ => crate::tmux::cache::ContextScope::Session,
            };
            let ctx = cache.get_labeled_context_scoped(chat_pane, ctx_scope);
            let pane_map = cache.pane_map_summary(chat_pane);
            let shells_line = knowledge::shell_context_block(&Owner::Chat(
                session_id.unwrap_or_default().to_string(),
            ));
            Ok(ToolCallOutcome::Result(format!(
                "{shells_line}{ctx}\n{pane_map}"
            )))
        }

        other => anyhow::bail!(
            "{} is not one of the terminal-context tool",
            other.tool_name()
        ),
    }
}
