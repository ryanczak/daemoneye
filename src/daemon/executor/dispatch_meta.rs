//! `execute_tool_call`'s arms for the tool-set and delegation tools: `InvalidArgs`, `LoadTools`, `SpawnGhost`.
//! A child module of `src/daemon/executor/mod.rs`.

use super::*;

/// The chat-only approval round trip for a ghost spawn: send the prompt,
/// await the reply under `USER_PROMPT_TIMEOUT`, and accept only an explicit
/// `GhostSpawnResponse { approved: true }`. A ghost caller never reaches
/// this.
async fn ask_chat_approval<W, R>(
    call: &PendingCall,
    tx: &mut W,
    rx: &mut R,
    runbook: &str,
    agent: Option<&str>,
    host: Option<&str>,
    message: &str,
) -> anyhow::Result<bool>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    send_response_split(
        tx,
        Response::GhostSpawnPrompt {
            id: call.id().to_string(),
            runbook: runbook.to_string(),
            agent: agent.map(str::to_string),
            host: host.map(str::to_string),
            message: message.to_string(),
        },
    )
    .await?;
    let mut line = Vec::new();
    let read_result = tokio::time::timeout(
        USER_PROMPT_TIMEOUT,
        crate::ipc::read_bounded_line(rx, &mut line, crate::ipc::MAX_IPC_MESSAGE_BYTES),
    )
    .await;
    if matches!(read_result, Ok(Ok(0))) {
        return Err(anyhow::anyhow!("EOF"));
    }
    match read_result {
        Ok(Ok(_)) => Ok(reply_approval(call.id(), &line, "ghost spawn")),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::InvalidData => Err(anyhow::anyhow!(
            "ghost spawn reply line too large ({e}); refusing the prompt and ending the turn"
        )),
        _ => Ok(false),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn dispatch<W, R>(
    call: &PendingCall,
    tx: &mut W,
    rx: &mut R,
    session_id: Option<&str>,
    sessions: &SessionStore,
    ghost_policy: &Option<GhostPolicy>,
    tool_policy: &Option<crate::agents::ToolPolicy>,
    spawn_depth: u8,
    effective_parent_job_id: &Option<String>,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    match call {
        PendingCall::InvalidArgs {
            id, name, error, ..
        } => {
            let msg = format!(
                "Error: invalid arguments for {name}: {error}. Fix the arguments and call {name} again."
            );
            log::warn!("model sent invalid arguments for tool '{name}': {error}");
            send_response_split(
                tx,
                Response::ToolResult {
                    tool_call_id: id.to_string(),
                    output: msg.clone(),
                },
            )
            .await?;
            Ok(ToolCallOutcome::Result(msg))
        }
        PendingCall::LoadTools { groups, .. } => {
            let mut loaded: Vec<String> = Vec::new();
            let mut unknown: Vec<String> = Vec::new();
            let mut result_parts = Vec::new();
            for g in groups {
                let members = crate::ai::tools::tools_in_group(g);
                if members.is_empty() {
                    unknown.push(g.clone());
                } else {
                    result_parts.push(format!("Loaded group(s): {} → {}", g, members.join(", ")));
                    loaded.extend(members.iter().map(|m| m.to_string()));
                }
            }
            // Persist into session state
            if let Some(sid) = session_id {
                with_sessions(sessions, |store| {
                    if let Some(entry) = store.get_mut(sid) {
                        for name in &loaded {
                            entry.loaded_tools.insert(name.clone());
                        }
                        entry.persistence.dirty = true;
                    }
                });
            }
            if !unknown.is_empty() {
                result_parts.push(format!(
                    "Unknown group(s): {} (valid groups: {})",
                    unknown.join(", "),
                    crate::ai::tools::deferred_catalog_text()
                ));
            }
            result_parts.push("They are now available; call them directly.".to_string());
            Ok(ToolCallOutcome::Result(result_parts.join(". ")))
        }

        PendingCall::SpawnGhost {
            runbook,
            message,
            agent,
            host,
            ..
        } => 'arm: {
            if spawn_depth >= 2 {
                break 'arm Ok(ToolCallOutcome::Result(
                    "Delegation depth limit reached (max: coordinator + 1 level of specialists)."
                        .to_string(),
                ));
            }
            let approved = if ghost_policy.is_none() {
                let approved = ask_chat_approval(
                    call,
                    tx,
                    rx,
                    runbook,
                    agent.as_deref(),
                    host.as_deref(),
                    message,
                )
                .await?;
                crate::daemon::utils::log_event(
                    "ghost_spawn_approval",
                    serde_json::json!({
                        "session": session_id.unwrap_or("-"),
                        "runbook": runbook,
                        "decision": if approved { "approved" } else { "denied" },
                    }),
                );
                approved
            } else {
                true
            };
            if !approved {
                break 'arm Ok(ToolCallOutcome::Result(
                    "Ghost shell spawn denied by user".to_string(),
                ));
            }
            knowledge::spawn_ghost(
                runbook,
                message,
                agent.as_deref(),
                host.as_deref(),
                ghost_policy.as_ref(),
                tool_policy.as_ref(),
                sessions,
                spawn_depth,
                effective_parent_job_id.as_deref(),
            )
            .await
        }

        other => anyhow::bail!(
            "{} is not one of the tool-set and delegation tools",
            other.tool_name()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn_ghost_call() -> PendingCall {
        PendingCall::SpawnGhost {
            id: "gs-1".to_string(),
            thought_signature: None,
            runbook: "rb06b".to_string(),
            message: "do the thing".to_string(),
            agent: None,
            host: None,
        }
    }

    async fn run_spawn(approved: bool) -> ToolCallOutcome {
        let (_guard, _home) = crate::test_home();
        let (mut daemon_tx, client_rx) = tokio::io::duplex(64 * 1024);
        let (mut client_tx, daemon_rx) = tokio::io::duplex(64 * 1024);
        let mut daemon_rx = tokio::io::BufReader::new(daemon_rx);
        let mut client_rx = tokio::io::BufReader::new(client_rx);
        let sessions = SessionStore::new();
        let call = spawn_ghost_call();

        let daemon = async move {
            dispatch(
                &call,
                &mut daemon_tx,
                &mut daemon_rx,
                Some("chat-1"),
                &sessions,
                &None,
                &None,
                0,
                &None,
            )
            .await
        };
        let client = async {
            let mut line = Vec::new();
            crate::ipc::read_bounded_line(&mut client_rx, &mut line, 1024 * 1024)
                .await
                .expect("read prompt");
            let resp: Response = serde_json::from_slice(&line).expect("parse prompt");
            match resp {
                Response::GhostSpawnPrompt { runbook, .. } => {
                    assert_eq!(runbook, "rb06b");
                    let reply = Request::GhostSpawnResponse {
                        id: "gs-1".to_string(),
                        approved,
                    };
                    use tokio::io::AsyncWriteExt;
                    client_tx
                        .write_all(
                            format!("{}\n", serde_json::to_string(&reply).unwrap()).as_bytes(),
                        )
                        .await
                        .unwrap();
                    client_tx.flush().await.unwrap();
                    drop(client_tx);
                    client_rx
                }
                other => panic!("expected GhostSpawnPrompt, got {other:?}"),
            }
        };
        let (outcome, client_rx) = tokio::join!(daemon, client);
        drop(client_rx);
        outcome.expect("dispatch must not error")
    }

    #[tokio::test]
    async fn spawn_ghost_from_chat_asks_first() {
        let outcome = run_spawn(false).await;
        let ToolCallOutcome::Result(text) = outcome else {
            panic!("expected Result");
        };
        assert!(
            text.contains("denied"),
            "denied leg must name the denial, got: {text}"
        );
    }

    #[tokio::test]
    async fn approved_spawn_proceeds_to_load_the_runbook() {
        let outcome = run_spawn(true).await;
        let ToolCallOutcome::Result(text) = outcome else {
            panic!("expected Result");
        };
        assert!(
            text.contains("Failed to load runbook"),
            "an approved spawn proceeds to spawn_ghost, got: {text}"
        );
    }

    #[tokio::test]
    async fn spawn_ghost_from_a_ghost_sends_no_prompt() {
        let (_guard, _home) = crate::test_home();
        let (daemon_tx, client_rx) = tokio::io::duplex(64 * 1024);
        let (_client_tx, daemon_rx) = tokio::io::duplex(64 * 1024);
        let mut daemon_tx = daemon_tx;
        let mut daemon_rx = tokio::io::BufReader::new(daemon_rx);
        let mut client_rx = tokio::io::BufReader::new(client_rx);
        let sessions = SessionStore::new();
        let policy = crate::daemon::policy::GhostPolicy::from_config(&crate::ipc::GhostConfig {
            spawn_depth: 1,
            ..Default::default()
        });
        let ghost_policy = Some(policy);

        let outcome = dispatch(
            &spawn_ghost_call(),
            &mut daemon_tx,
            &mut daemon_rx,
            Some("gsid-1"),
            &sessions,
            &ghost_policy,
            &None,
            1,
            &None,
        )
        .await
        .expect("dispatch must not error");
        let ToolCallOutcome::Result(text) = outcome else {
            panic!("expected Result");
        };
        assert!(
            text.contains("Failed to load runbook"),
            "a ghost caller spawns without asking, got: {text}"
        );
        drop(daemon_tx);
        let mut line = Vec::new();
        let n = crate::ipc::read_bounded_line(&mut client_rx, &mut line, 1024 * 1024)
            .await
            .expect("read after drop");
        assert_eq!(n, 0, "no prompt frame may reach the client");
    }

    #[tokio::test]
    async fn spawn_ghost_from_a_ghost_logs_no_approval_event() {
        let (_guard, home) = crate::test_home();
        let (daemon_tx, client_rx) = tokio::io::duplex(64 * 1024);
        let (_client_tx, daemon_rx) = tokio::io::duplex(64 * 1024);
        let mut daemon_tx = daemon_tx;
        let mut daemon_rx = tokio::io::BufReader::new(daemon_rx);
        let mut client_rx = tokio::io::BufReader::new(client_rx);
        let sessions = SessionStore::new();
        let policy = crate::daemon::policy::GhostPolicy::from_config(&crate::ipc::GhostConfig {
            spawn_depth: 1,
            ..Default::default()
        });
        let ghost_policy = Some(policy);

        let outcome = dispatch(
            &spawn_ghost_call(),
            &mut daemon_tx,
            &mut daemon_rx,
            Some("gsid-1"),
            &sessions,
            &ghost_policy,
            &None,
            1,
            &None,
        )
        .await
        .expect("dispatch must not error");
        let ToolCallOutcome::Result(text) = outcome else {
            panic!("expected Result");
        };
        assert!(
            text.contains("Failed to load runbook"),
            "a ghost caller spawns without asking, got: {text}"
        );
        drop(daemon_tx);
        let mut line = Vec::new();
        let n = crate::ipc::read_bounded_line(&mut client_rx, &mut line, 1024 * 1024)
            .await
            .expect("read after drop");
        assert_eq!(n, 0, "no prompt frame may reach the client");

        let mut saw_approval = false;
        crate::daemon::utils::for_each_event_between(None, None, &mut |v| {
            if v.get("event").and_then(|e| e.as_str()) == Some("ghost_spawn_approval") {
                saw_approval = true;
            }
        });
        assert!(
            !saw_approval,
            "a ghost caller must not log a ghost_spawn_approval event (home: {})",
            home.path().display()
        );
    }
}
