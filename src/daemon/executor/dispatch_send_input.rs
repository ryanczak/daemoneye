//! `execute_tool_call`'s arms for the shell input tool: `SendInput`.
//! A child module of `src/daemon/executor/mod.rs`.

use super::*;

pub(super) async fn dispatch<W, R>(
    call: &PendingCall,
    tx: &mut W,
    rx: &mut R,
    session_id: Option<&str>,
    ghost_policy: &Option<GhostPolicy>,
    tool_policy: &Option<crate::agents::ToolPolicy>,
    is_ghost: bool,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    match call {
        PendingCall::SendInput {
            id,
            shell,
            text,
            keys,
            paste,
            enter,
            ..
        } => {
            // 1. Policy gate — before anything else, so a ghost the policy
            //    denies gets this refusal, not the foreground one below.
            //    Default-deny: the agent `ToolPolicy` must *explicitly* allow
            //    `send_input`.
            if !ghost_may_send_input(is_ghost, tool_policy.as_ref()) {
                return Ok(ToolCallOutcome::Result(
                    "send_input is denied for ghost shells unless the agent's tool \
                     policy explicitly allows it."
                        .to_string(),
                ));
            }

            // 2. What actually holds the shell, probed live before any panel
            //    or send (D9). An id that cannot parse is not a shell at all,
            //    so it classifies `Unknown` — the same answer the probe gives
            //    an id the registry does not hold.
            let fg = match crate::shell::ShellId::parse(shell) {
                Ok(sid) => {
                    crate::shell::probe_foreground(
                        crate::shell::registry(),
                        &sid,
                        crate::shell::PROBE_TIMEOUT,
                    )
                    .await
                }
                Err(_) => crate::shell::Foreground::Unknown,
            };

            // 3. The ghost gate: a ghost may type only into a shell at its
            //    prompt, plus into a running program when its runbook opted
            //    in with `allow_tui_input`. Nothing is sent and no panel is
            //    shown.
            let allow_tui_input = ghost_policy.as_ref().is_some_and(|p| p.allow_tui_input);
            if is_ghost && !ghost_may_type_into(&fg, allow_tui_input) {
                return Ok(ToolCallOutcome::Result(ghost_input_refusal(shell, &fg)));
            }

            // 4. The shell's key modes and its screen, in one read. Read
            //    before the approval prompt so a bad key name or an
            //    unbracketable paste is the tool result, not a panel followed
            //    by a failure; an unreadable or unannotated reply falls back
            //    to the defaults, which encode plain bytes for every key.
            //    The screen text feeds the panel's TUI tail.
            let (modes, screen_text) = match crate::shell::screen(
                &crate::shell::socket_path(shell),
                false,
                0,
                std::time::Duration::from_secs(5),
            )
            .await
            {
                Ok(crate::shell::ShellResponse::ScreenText {
                    text: screen,
                    bracketed_paste,
                    application_cursor,
                    ..
                }) => (
                    crate::shell::ShellModes {
                        bracketed_paste,
                        application_cursor,
                    },
                    Some(screen),
                ),
                _ => (crate::shell::ShellModes::default(), None),
            };
            let input = match knowledge::compose_input(text, *paste, keys, *enter, modes) {
                Ok(input) => input,
                Err(e) => {
                    return Ok(ToolCallOutcome::Result(format!(
                        "Error: {} — nothing was sent.",
                        knowledge::format_err_chain(&e)
                    )));
                }
            };

            // 5. Approval. `ghost_policy` is `None` on purpose: the gates
            //    above already made the ghost decisions, and passing the live
            //    policy here would re-enter the helper's auto-approve branch.
            //    `target_pane_hint` is `None` because this is a PTY shell, not
            //    a tmux pane.
            let panel = knowledge::input_panel(shell, text, &input, &fg, screen_text.as_deref());
            match prompt_and_await_approval(
                ApprovalRequest {
                    id: id.as_str(),
                    cmd: &panel,
                    background: false,
                    target_pane_hint: None,
                    host: None,
                    user_shell: crate::shell::is_user_shell(crate::shell::registry(), shell)
                        .then_some(shell.as_str()),
                    interactive: false,
                },
                session_id,
                None,
                tx,
                rx,
            )
            .await?
            {
                Ok(_cmd_id) => {}
                Err(outcome) => return Ok(outcome),
            }

            // 6. Execute.
            Ok(ToolCallOutcome::Result(
                knowledge::send_input(
                    shell,
                    &input,
                    &Owner::Chat(session_id.unwrap_or_default().to_string()),
                )
                .await,
            ))
        }

        other => anyhow::bail!("{} is not one of the shell input tool", other.tool_name()),
    }
}
