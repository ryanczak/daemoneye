//! `execute_tool_call`'s arms for the shell input tool: `SendInput`.
//! A child module of `src/daemon/executor/mod.rs`.

use super::*;

/// The id gate (S2c): an id the grammar rejects, or one the registry does
/// not hold, is refused before the probe, the screen read and the approval
/// prompt. Fails closed — nothing is typed, nothing touches a socket built
/// from the raw string, and no prompt is put to the user. The registry miss
/// also decides the host question: without a record its host cannot be
/// resolved.
async fn resolve_send_input_shell(
    shell: &str,
) -> Result<(crate::shell::ShellId, crate::host::HostId), ToolCallOutcome> {
    let Ok(sid) = crate::shell::ShellId::parse(shell) else {
        return Err(ToolCallOutcome::Result(format!(
            "Error: shell id {shell:?} is not a valid shell id, so nothing \
             was sent. Shell ids look like s3 — list_shells names the \
             live ones."
        )));
    };
    let record = crate::shell::with_shells(crate::shell::registry(), |store| {
        store
            .get(sid.as_str())
            .map(|r| (r.owner.key(), r.host.clone()))
    });
    let Some((_owner_key, shell_host)) = record else {
        return Err(ToolCallOutcome::Result(knowledge::unknown_shell_error(
            crate::shell::registry(),
            shell,
        )));
    };
    Ok((sid, shell_host))
}

/// The shell's key modes and its screen, in one read. Read before the
/// approval prompt so a bad key name or an unbracketable paste is the tool
/// result, not a panel followed by a failure; an unreadable or unannotated
/// reply falls back to the defaults, which encode plain bytes for every key.
/// The screen text feeds the panel's TUI tail.
async fn read_modes_and_screen(shell: &str) -> (crate::shell::ShellModes, Option<String>) {
    match crate::shell::screen(
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
    }
}

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

            // 2. The id gate (S2c): an id the grammar rejects, or one the
            //    registry does not hold, is refused before the probe, the
            //    screen read and the approval prompt. The record's host is
            //    what the approval classification needs (S2b, host).
            let (sid, shell_host) = match resolve_send_input_shell(shell).await {
                Ok(pair) => pair,
                Err(outcome) => return Ok(outcome),
            };

            // 3. What actually holds the shell, probed live before any panel
            //    or send (D9).
            let fg = crate::shell::probe_foreground(
                crate::shell::registry(),
                &sid,
                crate::shell::PROBE_TIMEOUT,
            )
            .await;

            // 4. The ghost gate: a ghost may type only into a shell at its
            //    prompt, plus into a running program when its runbook opted
            //    in with `allow_tui_input`. Nothing is sent and no panel is
            //    shown.
            let allow_tui_input = ghost_policy.as_ref().is_some_and(|p| p.allow_tui_input);
            if is_ghost && !ghost_may_type_into(&fg, allow_tui_input) {
                return Ok(ToolCallOutcome::Result(ghost_input_refusal(shell, &fg)));
            }

            // 5. The shell's key modes and its screen, in one read. Read
            //    before the approval prompt so a bad key name or an
            //    unbracketable paste is the tool result, not a panel followed
            //    by a failure; an unreadable or unannotated reply falls back
            //    to the defaults, which encode plain bytes for every key.
            //    The screen text feeds the panel's TUI tail.
            let (modes, screen_text) = read_modes_and_screen(shell).await;
            let input = match knowledge::compose_input(text, *paste, keys, *enter, modes) {
                Ok(input) => input,
                Err(e) => {
                    return Ok(ToolCallOutcome::Result(format!(
                        "Error: {} — nothing was sent.",
                        knowledge::format_err_chain(&e)
                    )));
                }
            };

            // 6. Approval. `ghost_policy` is `None` on purpose: the gates
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
                    host: Some(shell_host.as_str()),
                    user_shell: crate::shell::is_user_shell(crate::shell::registry(), shell)
                        .then_some(shell.as_str()),
                    interactive: false,
                    shell_label: None,
                    sudo: Some(crate::daemon::utils::command_has_sudo(
                        &knowledge::sudo_decision_text(text, keys),
                    )),
                    session_id,
                },
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
                knowledge::send_input(shell, &input, &call_owner(is_ghost, session_id)).await,
            ))
        }

        other => anyhow::bail!("{} is not one of the shell input tool", other.tool_name()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S2c: an id `ShellId::parse` rejects is refused before the probe, the
    /// screen read and the approval prompt — nothing touches a socket built
    /// from the raw string, and no prompt is put to the user.
    #[tokio::test]
    async fn send_input_refuses_an_unparseable_shell_id_before_the_prompt() {
        use tokio::io::AsyncReadExt as _;
        let (_guard, _home) = crate::test_home();
        let store = SessionStore::new();
        let call = PendingCall::SendInput {
            id: "si-03".to_string(),
            thought_signature: None,
            shell: "../../x".to_string(),
            text: "echo hi".to_string(),
            keys: Vec::new(),
            paste: None,
            enter: false,
        };
        let (mut daemon_tx, mut client_rx) = tokio::io::duplex(64 * 1024);
        let mut daemon_rx = std::io::Cursor::new(Vec::<u8>::new());
        let outcome = execute_tool_call(
            &call,
            &mut daemon_tx,
            &mut daemon_rx,
            SessionCtx {
                session_id: Some("sess-03"),
                session_name: "sess-03",
                chat_pane: None,
                sessions: &store,
                shells: crate::shell::registry(),
            },
            &Arc::new(SessionCache::new("home")),
            &Arc::new(crate::scheduler::ScheduleStore::new_empty()),
        )
        .await;
        drop(daemon_tx);
        let mut raw = Vec::new();
        client_rx
            .read_to_end(&mut raw)
            .await
            .expect("reading the client half of the pipe");
        let frames: Vec<Response> = String::from_utf8(raw)
            .expect("the daemon writes UTF-8 frames")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str::<Response>(line).expect("every frame is a Response"))
            .collect();
        let text = match outcome.expect("the run must not error") {
            ToolCallOutcome::Result(t) => t,
            ToolCallOutcome::UserMessage(t) => panic!("no redirect is possible: {t}"),
            ToolCallOutcome::SpawnGhostSession { tool_result, .. } => {
                panic!("not a spawn: {tool_result}")
            }
        };
        assert!(
            text.contains("../../x") && text.contains("nothing was sent"),
            "the refusal names the id and says nothing was sent: {text}"
        );
        assert!(
            !frames
                .iter()
                .any(|f| matches!(f, Response::ToolCallPrompt { .. })),
            "no approval prompt is put to the user: {frames:?}"
        );
    }

    /// S2c: an id that parses but is not in the registry is refused the same
    /// way, before any prompt — its host cannot be resolved, so no approval
    /// question can be put to the user either.
    #[tokio::test]
    async fn send_input_refuses_an_id_the_registry_does_not_hold_before_the_prompt() {
        use tokio::io::AsyncReadExt as _;
        let (_guard, _home) = crate::test_home();
        let store = SessionStore::new();
        let call = PendingCall::SendInput {
            id: "si-03b".to_string(),
            thought_signature: None,
            shell: "s990".to_string(),
            text: "echo hi".to_string(),
            keys: Vec::new(),
            paste: None,
            enter: false,
        };
        let (mut daemon_tx, mut client_rx) = tokio::io::duplex(64 * 1024);
        let mut daemon_rx = std::io::Cursor::new(Vec::<u8>::new());
        let outcome = execute_tool_call(
            &call,
            &mut daemon_tx,
            &mut daemon_rx,
            SessionCtx {
                session_id: Some("sess-03"),
                session_name: "sess-03",
                chat_pane: None,
                sessions: &store,
                shells: crate::shell::registry(),
            },
            &Arc::new(SessionCache::new("home")),
            &Arc::new(crate::scheduler::ScheduleStore::new_empty()),
        )
        .await;
        drop(daemon_tx);
        let mut raw = Vec::new();
        client_rx
            .read_to_end(&mut raw)
            .await
            .expect("reading the client half of the pipe");
        let frames: Vec<Response> = String::from_utf8(raw)
            .expect("the daemon writes UTF-8 frames")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str::<Response>(line).expect("every frame is a Response"))
            .collect();
        let text = match outcome.expect("the run must not error") {
            ToolCallOutcome::Result(t) => t,
            ToolCallOutcome::UserMessage(t) => panic!("no redirect is possible: {t}"),
            ToolCallOutcome::SpawnGhostSession { tool_result, .. } => {
                panic!("not a spawn: {tool_result}")
            }
        };
        assert!(
            text.contains("s990") && text.starts_with("Error:"),
            "the refusal names the id: {text}"
        );
        assert!(
            !frames
                .iter()
                .any(|f| matches!(f, Response::ToolCallPrompt { .. })),
            "no approval prompt is put to the user: {frames:?}"
        );
    }

    /// Insert a shell record this session owns, field by field, with the
    /// given host; remove it at the end of the test. The record has no
    /// socket, so the probe and the screen read fail fast and fall back.
    fn insert_owned_shell(index: u32, host: crate::host::HostId) -> crate::shell::ShellId {
        let id = crate::shell::ShellId::from_index(index);
        crate::shell::with_shells(crate::shell::registry(), |s| {
            s.insert(
                id.as_str().to_string(),
                crate::shell::ShellRecord {
                    id: id.clone(),
                    owner: Owner::Chat("sess-03".to_string()),
                    host,
                    sandbox: None,
                    started_unix: 1_788_470_000,
                    dedicated: false,
                    exited_unix: None,
                    last_foreground: None,
                    in_flight: None,
                    last_programs: Vec::new(),
                },
            );
        });
        id
    }

    fn remove_shell(id: &crate::shell::ShellId) {
        crate::shell::with_shells(crate::shell::registry(), |s| {
            s.remove(id.as_str());
        });
    }

    /// Drive one `send_input` call for session `sess-03` with an empty
    /// client reply, and return the outcome text plus every frame the
    /// daemon wrote.
    async fn drive_send_input(
        call: &PendingCall,
    ) -> (Result<ToolCallOutcome, anyhow::Error>, Vec<Response>) {
        use tokio::io::AsyncReadExt as _;
        let store = SessionStore::new();
        let (mut daemon_tx, mut client_rx) = tokio::io::duplex(64 * 1024);
        let mut daemon_rx = std::io::Cursor::new(Vec::<u8>::new());
        let outcome = execute_tool_call(
            call,
            &mut daemon_tx,
            &mut daemon_rx,
            SessionCtx {
                session_id: Some("sess-03"),
                session_name: "sess-03",
                chat_pane: None,
                sessions: &store,
                shells: crate::shell::registry(),
            },
            &Arc::new(SessionCache::new("home")),
            &Arc::new(crate::scheduler::ScheduleStore::new_empty()),
        )
        .await;
        drop(daemon_tx);
        let mut raw = Vec::new();
        client_rx
            .read_to_end(&mut raw)
            .await
            .expect("reading the client half of the pipe");
        let frames: Vec<Response> = String::from_utf8(raw)
            .expect("the daemon writes UTF-8 frames")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str::<Response>(line).expect("every frame is a Response"))
            .collect();
        (outcome, frames)
    }

    fn send_input_call(id: &str, shell: &str, text: &str, keys: &[&str]) -> PendingCall {
        PendingCall::SendInput {
            id: id.to_string(),
            thought_signature: None,
            shell: shell.to_string(),
            text: text.to_string(),
            keys: keys.iter().map(|s| s.to_string()).collect(),
            paste: None,
            enter: false,
        }
    }

    fn prompt_of(frames: &[Response]) -> &Response {
        frames
            .iter()
            .find(|f| matches!(f, Response::ToolCallPrompt { .. }))
            .expect("a ToolCallPrompt frame is written")
    }

    /// S2b host: input into a shell on a configured remote host is approved
    /// as remote input on that host — the record's `HostId` reaches the
    /// prompt and the classification is `RemoteShell`.
    #[tokio::test]
    async fn send_input_into_a_remote_shell_is_classified_remote_shell() {
        let (_guard, home) = crate::test_home();
        std::fs::write(
            crate::config::etc_dir().join("config.toml"),
            "[hosts.pinky]\nssh = \"pinky\"\n",
        )
        .expect("writing the host table");
        let id = insert_owned_shell(903, crate::host::HostId::Named("pinky".into()));
        let call = send_input_call("si-903", id.as_str(), "echo hi", &[]);
        let (outcome, frames) = drive_send_input(&call).await;
        remove_shell(&id);
        drop(home);
        // The empty reply makes the prompt wait hit EOF; the outcome being
        // an Err is the expected shape, the prompt frame is the evidence.
        assert!(outcome.is_err(), "EOF at the prompt, not a silent run");
        match prompt_of(&frames) {
            Response::ToolCallPrompt { classification, .. } => assert_eq!(
                classification,
                &crate::ipc::Classification::RemoteShell {
                    host: "pinky".to_string()
                },
                "the prompt classifies as remote input on pinky"
            ),
            other => panic!("not a prompt: {other:?}"),
        }
    }

    /// S2b host (preservation): input into a local shell is still classified
    /// `Command`.
    #[tokio::test]
    async fn send_input_into_a_local_shell_is_still_classified_command() {
        let (_guard, _home) = crate::test_home();
        let id = insert_owned_shell(904, crate::host::HostId::Local);
        let call = send_input_call("si-904", id.as_str(), "echo hi", &[]);
        let (outcome, frames) = drive_send_input(&call).await;
        remove_shell(&id);
        assert!(outcome.is_err(), "EOF at the prompt, not a silent run");
        match prompt_of(&frames) {
            Response::ToolCallPrompt { classification, .. } => assert_eq!(
                classification,
                &crate::ipc::Classification::Command,
                "a local shell's prompt is an ordinary command"
            ),
            other => panic!("not a prompt: {other:?}"),
        }
    }

    /// The client's sudo decision for one `send_input` prompt: the frame's
    /// `sudo` fact joined with the shipped default approval state. `false`
    /// means the prompt is shown, `true` means it auto-approves. Built
    /// through the same `SessionApproval::from_config` the chat session
    /// builds at its start — the production constructor.
    fn auto_approves(classification: &crate::ipc::Classification, sudo: bool) -> bool {
        let approval = crate::cli::commands::approval_for_tests();
        approval.auto_approves(classification, sudo)
    }

    fn sudo_text(text: &str, keys: &[&str]) -> String {
        knowledge::sudo_decision_text(
            text,
            &keys.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
    }

    /// S2b sudo: a newline in the text puts `sudo` at a command position on
    /// its own line; the panel shows the text Debug-quoted (`\n` as two
    /// characters), so the decision must come from the call's own fields.
    #[tokio::test]
    async fn send_input_types_sudo_on_a_newline_and_the_client_is_asked() {
        let (_guard, _home) = crate::test_home();
        let id = insert_owned_shell(905, crate::host::HostId::Local);
        let call = send_input_call("si-905", id.as_str(), "echo hi\nsudo reboot", &[]);
        let (outcome, frames) = drive_send_input(&call).await;
        remove_shell(&id);
        assert!(outcome.is_err(), "EOF at the prompt, not a silent run");
        let sudo = match prompt_of(&frames) {
            Response::ToolCallPrompt { sudo, .. } => *sudo,
            other => panic!("not a prompt: {other:?}"),
        };
        assert!(sudo, "the daemon says the input types sudo");
        assert!(
            !auto_approves(&crate::ipc::Classification::Command, sudo),
            "under ApprovalsConfig::default() a sudo send_input is asked, not auto-approved"
        );
        assert!(
            crate::daemon::utils::command_has_sudo(&sudo_text("echo hi\nsudo reboot", &[])),
            "the decision text (real newlines) has sudo at a command position"
        );
    }

    /// S2b sudo: single-character keys spell `sudo` one character at a
    /// time; the panel shows `keys [s, u, d, o, …]`, so the decision must
    /// come from what the keys type.
    #[tokio::test]
    async fn send_input_types_sudo_through_single_character_keys_and_the_client_is_asked() {
        let (_guard, _home) = crate::test_home();
        let id = insert_owned_shell(906, crate::host::HostId::Local);
        let keys = [
            "s", "u", "d", "o", "Space", "r", "e", "b", "o", "o", "t", "Enter",
        ];
        let call = send_input_call("si-906", id.as_str(), "", &keys);
        let (outcome, frames) = drive_send_input(&call).await;
        remove_shell(&id);
        assert!(outcome.is_err(), "EOF at the prompt, not a silent run");
        let sudo = match prompt_of(&frames) {
            Response::ToolCallPrompt { sudo, .. } => *sudo,
            other => panic!("not a prompt: {other:?}"),
        };
        assert!(sudo, "the daemon says the keys type sudo");
        assert!(
            !auto_approves(&crate::ipc::Classification::Command, sudo),
            "under ApprovalsConfig::default() a sudo send_input is asked, not auto-approved"
        );
    }

    /// S2b sudo: the plain spelling still works, though the panel already
    /// caught this one.
    #[tokio::test]
    async fn send_input_with_a_plain_sudo_text_is_asked() {
        let (_guard, _home) = crate::test_home();
        let id = insert_owned_shell(907, crate::host::HostId::Local);
        let call = send_input_call("si-907", id.as_str(), "sudo reboot", &[]);
        let (outcome, frames) = drive_send_input(&call).await;
        remove_shell(&id);
        assert!(outcome.is_err(), "EOF at the prompt, not a silent run");
        let sudo = match prompt_of(&frames) {
            Response::ToolCallPrompt { sudo, .. } => *sudo,
            other => panic!("not a prompt: {other:?}"),
        };
        assert!(sudo, "the daemon says the input types sudo");
        assert!(
            !auto_approves(&crate::ipc::Classification::Command, sudo),
            "under ApprovalsConfig::default() a sudo send_input is asked, not auto-approved"
        );
    }

    /// S2b sudo (negative): `sudo` as a substring is not a command word,
    /// and a control key (`C-c`) types nothing readable.
    #[test]
    fn send_input_sudo_decision_text_rejects_sudo_substrings_and_control_keys() {
        assert!(
            !crate::daemon::utils::command_has_sudo(&sudo_text("echo pseudo", &[])),
            "pseudo is not sudo"
        );
        assert!(
            !crate::daemon::utils::command_has_sudo(&sudo_text("cat /etc/sudoers", &[])),
            "sudoers is not sudo"
        );
        assert!(
            !crate::daemon::utils::command_has_sudo(&sudo_text("", &["C-c"])),
            "C-c types no readable character, so it cannot spell sudo: {:?}",
            sudo_text("", &["C-c"])
        );
    }
}
