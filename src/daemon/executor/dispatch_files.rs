//! `execute_tool_call`'s arms for the file tools: `ReadFile`, `EditFile`.
//! A child module of `src/daemon/executor/mod.rs`.

use super::*;

pub(super) async fn dispatch<W, R>(
    call: &PendingCall,
    tx: &mut W,
    rx: &mut R,
    session_id: Option<&str>,
    ghost_policy: &Option<GhostPolicy>,
    is_ghost: bool,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    match call {
        PendingCall::ReadFile {
            path,
            offset,
            limit,
            pattern,
            host,
            ..
        } => 'arm: {
            // A ghost reaching a host its runbook does not name is refused
            // before the run — the same boundary the foreground arm enforces.
            let host_id = match resolve_host(host.as_deref()) {
                Ok(parsed) => parsed,
                Err(msg) => break 'arm Ok(ToolCallOutcome::Result(format!("Error: {msg}"))),
            };
            if let Some(policy) = ghost_policy.as_ref()
                && let Some(msg) = refuse_ghost_host(policy, &host_id, "read_file", session_id)
            {
                break 'arm Ok(ToolCallOutcome::Result(msg));
            }
            if let Err(msg) = connect_named_host(host.as_deref(), is_ghost, call.id(), tx, rx).await
            {
                log_connect_failure("read_file", session_id, host_id.as_str(), &msg);
                let outcome = format!("Error: {msg}");
                break 'arm Ok(ToolCallOutcome::Result(outcome));
            }
            file_ops::run_read_file(path, *offset, *limit, pattern.as_deref(), host.as_deref())
                .await
        }

        PendingCall::EditFile {
            id,
            path,
            operation,
            old_string,
            new_string,
            content,
            dest_path,
            host,
            sudo,
            ..
        } => {
            // The ghost refusal lives inside `run_edit_file`, so the connect
            // step must not run ahead of it: a ghost's `edit_file` would
            // otherwise open a BatchMode ControlMaster to any host, listed
            // or not, before being told file operations are refused (08d's
            // fence must not be tunnelled through a tool the ghost cannot
            // even use). Only a chat session connects first.
            if edit_file_connects_first(is_ghost, host.as_deref())
                && let Err(msg) =
                    connect_named_host(host.as_deref(), false, id.as_str(), tx, rx).await
            {
                log_connect_failure(
                    "edit_file",
                    session_id,
                    host.as_deref().unwrap_or("local"),
                    &msg,
                );
                let outcome = format!("Error: {msg}");
                send_response_split(
                    tx,
                    Response::ToolResult {
                        tool_call_id: id.to_string(),
                        output: outcome.clone(),
                    },
                )
                .await?;
                return Ok(ToolCallOutcome::Result(outcome));
            }
            file_ops::run_edit_file(
                file_ops::EditArgs {
                    id,
                    path,
                    operation,
                    old_string: old_string.as_deref(),
                    new_string: new_string.as_deref(),
                    content: content.as_deref(),
                    dest_path: dest_path.as_deref(),
                    host: host.as_deref(),
                    sudo: sudo.unwrap_or(false),
                },
                session_id,
                GhostCtx {
                    policy: ghost_policy.as_ref(),
                    is_ghost,
                },
                tx,
                rx,
            )
            .await
        }

        other => anyhow::bail!("{} is not one of the file tools", other.tool_name()),
    }
}

/// Record a failed connect to a named host, for a file tool's call.
fn log_connect_failure(tool: &str, session_id: Option<&str>, requested_host: &str, msg: &str) {
    crate::daemon::utils::log_event(
        "ssh_connect_failed",
        serde_json::json!({
            "session": session_id.unwrap_or("-"),
            "tool": tool,
            "requested_host": requested_host,
            "error": msg,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::log_connect_failure;
    use crate::ai::PendingCall;
    use crate::daemon::executor::ToolCallOutcome;
    use crate::daemon::session::SessionStore;
    use crate::ipc::Response;
    use crate::tmux::cache::SessionCache;
    use std::sync::Arc;
    use tokio::io::AsyncReadExt as _;

    #[test]
    fn log_connect_failure_records_the_tool_and_host() {
        let _home = crate::daemon::run::cancel::test_support::HomeGuard::new();
        log_connect_failure("edit_file", Some("sess-1"), "web01", "connection refused");

        let seg = crate::config::current_event_segment_path();
        let text = std::fs::read_to_string(&seg).unwrap_or_default();
        let events: Vec<serde_json::Value> = text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .filter(|v: &serde_json::Value| v["event"] == "ssh_connect_failed")
            .collect();
        assert_eq!(events.len(), 1, "one call, one event: {events:?}");
        assert_eq!(events[0]["tool"], "edit_file");
        assert_eq!(events[0]["requested_host"], "web01");
        assert_eq!(events[0]["session"], "sess-1");
        assert_eq!(events[0]["error"], "connection refused");
    }

    /// M31 #79: `read_file` is a silent tool — it reports through
    /// `ToolStarted`/`ToolFinished`, exactly as `open_shell`'s identical
    /// cold-host connect failure does — so its connect-failure branch must
    /// not also write an explicit `Response::ToolResult` frame; the client
    /// would see an extra frame no other silent-tool path sends. The
    /// returned outcome text (`Error: …`) and the `ssh_connect_failed`
    /// event are unchanged.
    #[tokio::test]
    async fn read_file_connect_failure_writes_no_tool_result_frame() {
        let _home = crate::test_home_guard();
        let call = PendingCall::ReadFile {
            id: "rf-79".to_string(),
            thought_signature: None,
            path: "/var/log/syslog".to_string(),
            offset: None,
            limit: None,
            pattern: None,
            host: Some("de-test-unreachable.invalid".to_string()),
        };
        let store = SessionStore::new();
        let (mut daemon_tx, mut client_rx) = tokio::io::duplex(64 * 1024);
        let mut daemon_rx = tokio::io::BufReader::new(std::io::Cursor::new(Vec::new()));
        let outcome = super::super::execute_tool_call(
            &call,
            &mut daemon_tx,
            &mut daemon_rx,
            crate::daemon::executor::SessionCtx {
                session_id: Some("sess-79"),
                session_name: "sess-79",
                chat_pane: None,
                sessions: &store,
            },
            &Arc::new(SessionCache::new("home")),
            &Arc::new(crate::scheduler::ScheduleStore::new_empty()),
        )
        .await
        .expect("the connect failure is a tool result, not a transport error");
        drop(daemon_tx);
        let mut raw = Vec::new();
        client_rx
            .read_to_end(&mut raw)
            .await
            .expect("reading the client half of the pipe must not error");
        let text = String::from_utf8(raw).expect("the daemon writes UTF-8 frames");
        let frames: Vec<Response> = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("every frame is a Response"))
            .collect();

        let msg = match &outcome {
            ToolCallOutcome::Result(msg) => msg.clone(),
            ToolCallOutcome::UserMessage(text) => {
                panic!("read_file's connect failure is not a redirect, got {text:?}")
            }
            ToolCallOutcome::SpawnGhostSession { .. } => {
                panic!("read_file's connect failure never spawns a ghost")
            }
        };
        assert!(
            msg.starts_with("Error:"),
            "the outcome text is the connect failure's Error sentence: {msg}"
        );
        let tool_results = frames
            .iter()
            .filter(|f| matches!(f, Response::ToolResult { .. }))
            .count();
        assert_eq!(
            tool_results, 0,
            "a silent tool's connect failure sends no ToolResult frame; the client saw: {frames:?}"
        );
    }
}
