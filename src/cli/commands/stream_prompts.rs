//! The ratatui approval primitives and prompt functions: reading an
//! approval, and the tool-call, credential, pane, yes/no, write and
//! edit-file prompts.

use super::*;

// ── Ratatui interactive approval primitives ──────────────────────────────────

/// Map a key event to a stream outcome the interrupt filter must not
/// swallow. FocusGained (ESC [ I) means the user switched back to this
/// pane and the viewport may need re-pinning; Ctrl+O opens the transcript
/// viewer mid-turn (the transcript holds everything up to this moment).
pub(super) fn key_outcome(key: &Key) -> Option<StreamOutcome> {
    match key {
        Key::FocusGained => Some(StreamOutcome::Reanchor),
        Key::CtrlO => Some(StreamOutcome::OpenViewer),
        _ => None,
    }
}

/// Bottom-border label for a finished tool: "✓ 1.2s" / "✗ 0.5s".
pub(super) fn tool_runtime_label(ok: bool, elapsed_ms: u64) -> String {
    let status = if ok { "✓" } else { "✗" };
    format!("{status} {:.1}s", elapsed_ms as f64 / 1000.0)
}

/// Build the canonical approval-prompt string shared by every approval flow.
/// Option order is fixed: [Y]es, [A]pprove for <label>, [N]o, then the
/// redirect affordance only where the flow supports it. Keeping all flows on
/// this one builder is what prevents the prompts from drifting apart again.
pub(super) fn build_approval_prompt(session_label: &str, supports_redirect: bool) -> String {
    let redirect = if supports_redirect {
        "or type a message "
    } else {
        ""
    };
    format!("  Approve? [Y]es  [A]pprove for {session_label}  [N]o  {redirect}› ")
}

/// Parse a user response to a Y/N/A prompt.
pub(super) fn parse_approval_response(input: &str) -> (bool, bool, Option<String>) {
    let trimmed = input.trim();
    let lower = trimmed.to_lowercase();
    match lower.as_str() {
        "y" | "yes" => (true, false, None),
        "n" | "no" | "" => (false, false, None),
        "a" => (true, true, None),
        _ => (false, false, Some(trimmed.to_string())),
    }
}

/// One bounded stdin read: `None` at the deadline, `None` at EOF, `Some(b)`
/// when a byte arrives in time. Named so the deadline behavior is testable
/// without a terminal.
pub(crate) async fn read_byte_by<F>(deadline: tokio::time::Instant, fut: F) -> Option<u8>
where
    F: std::future::Future<Output = Option<u8>>,
{
    tokio::time::timeout_at(deadline, fut).await.ok().flatten()
}

/// Read a single keypress or full line under crossterm raw mode, rendering
/// the prompt and input in the live region (not scrollback).
///
/// If the first character is Y, N, or A (case-insensitive), returns it
/// immediately.  Otherwise reads the full line using the input editor,
/// redrawing the live region on each keystroke so the user sees their
/// typed text in place.
///
/// The read is bounded by `deadline`: past it the function returns `None`
/// (F7) so the caller can commit the timeout, send nothing, and return to the
/// response loop before the daemon's own timeout lands. Without the bound the
/// panel outlives the daemon's wait and the next line typed fails with
/// `Broken pipe`.
async fn read_approval_input(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    prompt_text: &str,
    status: &crate::cli::render::StatusBarState<'_>,
    deadline: tokio::time::Instant,
) -> Option<String> {
    read_approval_input_with(renderer, stdin, deadline, |r, line| {
        let _ = r.draw_prompt(prompt_text, line, status);
    })
    .await
}

/// `read_approval_input`, panel edition: identical key semantics, but every
/// redraw renders the themed approval panel instead of the plain prompt.
async fn read_approval_input_panel(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    title: &str,
    session_label: &str,
    status: &crate::cli::render::StatusBarState<'_>,
    deadline: tokio::time::Instant,
) -> Option<String> {
    read_approval_input_with(renderer, stdin, deadline, |r, line| {
        let _ = r.draw_approval_panel(title, session_label, line, status);
    })
    .await
}

/// The approval input state machine both prompt shapes share. `redraw`
/// paints the current input; it runs once on entry and after every edit.
/// `y`, `n` or `a` as the first key answers at once; Enter on an empty
/// line, Ctrl+C and Escape answer with an empty string; anything else
/// starts a typed message, ended by Enter. `None` when the deadline passes.
async fn read_approval_input_with(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    deadline: tokio::time::Instant,
    redraw: impl Fn(
        &mut crate::cli::render_ratatui::RatatuiRendererStdout,
        &crate::cli::input::InputLine,
    ),
) -> Option<String> {
    use crate::cli::input::InputLine;

    // Initial draw with empty input.
    let mut line = InputLine::new();
    redraw(renderer, &line);

    // Read the first byte to decide: single-key shortcut vs. full line edit.
    let first = read_byte_by(deadline, stdin.read_byte()).await?;
    let ch = first as char;
    match ch.to_ascii_lowercase() {
        'y' | 'n' | 'a' => {
            // Show the key the user pressed in the input box, then return.
            line.insert(ch);
            redraw(renderer, &line);
            Some(ch.to_string())
        }
        '\r' | '\n' => {
            // Empty input (Enter pressed immediately)
            Some(String::new())
        }
        _ => {
            // Start of a typed message — use the input editor.
            line.insert(ch);
            redraw(renderer, &line);

            loop {
                let b = read_byte_by(deadline, stdin.read_byte()).await?;
                match b {
                    b'\r' | b'\n' => return Some(line.as_str()),
                    b'\x7f' | b'\x08' => {
                        line.backspace();
                        redraw(renderer, &line);
                    }
                    b'\x03' => {
                        // Ctrl+C — cancel, return empty
                        return Some(String::new());
                    }
                    b'\x1b' => {
                        // Escape — cancel, return empty
                        return Some(String::new());
                    }
                    b => {
                        if b >= 0x20 {
                            line.insert(b as char);
                            redraw(renderer, &line);
                        }
                    }
                }
            }
        }
    }
}

/// The tail of a declined approval: a redirect line when the user typed a
/// message, a skip line otherwise. Returns the message, which goes back to
/// the daemon with the denial.
fn commit_declined(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    user_msg: Option<String>,
) -> Option<String> {
    match user_msg {
        Some(msg) => {
            let _ = renderer.commit("  ↩ redirecting agent with your message…\n");
            Some(msg)
        }
        None => {
            let _ = renderer.commit("  ✗ skipped\n");
            None
        }
    }
}

/// Display info text and prompt Y/N/A with typed-message support.
/// Returns `(approved, is_approve_session, user_message)`.
async fn prompt_with_session_approve(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    status: &crate::cli::render::StatusBarState<'_>,
    info_lines: &[&str],
    prompt_text: &str,
) -> (bool, bool, Option<String>) {
    // Commit info lines to scrollback (these are permanent).
    for line in info_lines {
        let _ = renderer.commit(&format!("{}\n", line));
    }
    // Read the decision in the live region.
    let deadline = tokio::time::Instant::now() + APPROVAL_TIMEOUT;
    let Some(input) = read_approval_input(renderer, stdin, prompt_text, status, deadline).await
    else {
        let _ = renderer.commit("  ⏱ approval timed out — the command was not run\n");
        return (false, false, None);
    };
    parse_approval_response(&input)
}

// ── Ratatui prompt functions (called from ask_with_session_ratatui) ──────────

#[allow(clippy::too_many_arguments)] // 9 params: the flow needs renderer, stdin, status, approval, command, background, pane, class, and the launch flag
pub(super) async fn prompt_tool_call_ratatui(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    status: &crate::cli::render::StatusBarState<'_>,
    approval: &mut super::approval::SessionApproval,
    command: &str,
    background: bool,
    target_pane: Option<&str>,
    classification: &crate::ipc::Classification,
    interactive: bool,
) -> anyhow::Result<(bool, Option<String>)> {
    use crate::daemon::utils::command_has_sudo;

    let where_label = tool_call_panel_label(background);
    let body = tool_call_panel_body(
        command,
        background,
        target_pane,
        classification,
        interactive,
    );
    let _ = renderer.commit_panel(where_label, &body, false);

    let is_sudo = command_has_sudo(command);
    let auto_approved = approval.auto_approves(classification, is_sudo);

    if auto_approved {
        let _ = renderer.commit("  ✓ auto-approved (session)\n");
        return Ok((true, None));
    }

    let session_label = approval_session_label(classification, is_sudo);
    let offer_session = matches!(
        classification,
        crate::ipc::Classification::Command | crate::ipc::Classification::RemoteShell { .. }
    );
    let deadline = tokio::time::Instant::now() + APPROVAL_TIMEOUT;
    let Some(input) = read_approval_input_panel(
        renderer,
        stdin,
        "approve command",
        &session_label,
        status,
        deadline,
    )
    .await
    else {
        let _ = renderer.commit("  ⏱ approval timed out — the command was not run\n");
        return Ok((false, None));
    };
    let (approved, is_session, user_msg) = parse_approval_response(&input);
    let is_session = is_session && offer_session;

    if approved {
        if is_session {
            match approval.record_session_approval(classification, is_sudo) {
                Some(kind) => {
                    let _ = renderer.commit(&format!(
                        "  ✓ approved — {kind} auto-approved for this session\n"
                    ));
                }
                None => {
                    let _ = renderer.commit("  ✓ approved\n");
                }
            }
        } else {
            let _ = renderer.commit("  ✓ approved\n");
        }
        Ok((true, None))
    } else {
        Ok((false, commit_declined(renderer, user_msg)))
    }
}

/// The session label the approval panel shows after `[A]pprove for`, naming
/// the host (or the always-asked class) the grant would widen.
pub(super) fn approval_session_label(
    classification: &crate::ipc::Classification,
    is_sudo: bool,
) -> String {
    let scope = if is_sudo { "sudo session" } else { "session" };
    match classification {
        crate::ipc::Classification::Command => scope.to_string(),
        crate::ipc::Classification::RemoteShell { host } => format!("{host} {scope}"),
        crate::ipc::Classification::NewHost { host } => {
            format!("{host} (unconfigured) — always asked")
        }
        crate::ipc::Classification::EscapeHatch => "escape hatch — always asked".to_string(),
        crate::ipc::Classification::UserShell { shell } => {
            format!("your shell {shell} — always asked")
        }
    }
}

/// The approval-panel body a `ToolCallPrompt` renders, classification included.
///
/// Separate from `prompt_tool_call_ratatui` because that function is the whole
/// interactive flow (panel + input + decision); the tests pin the body text
/// against a test backend without driving the input loop.
pub(super) fn tool_call_panel_body(
    command: &str,
    _background: bool,
    target_pane: Option<&str>,
    classification: &crate::ipc::Classification,
    interactive: bool,
) -> Vec<String> {
    let mut body = vec![format!("$ {}", command)];
    if interactive {
        body.push("→ interactive: the command keeps the shell; nothing waits for it".to_string());
    }
    if let Some(tp) = target_pane {
        body.push(format!("→ target: {}", tp));
    }
    match classification {
        crate::ipc::Classification::Command => {}
        crate::ipc::Classification::RemoteShell { host } => {
            body.push(format!("→ host: {} (configured)", host));
        }
        crate::ipc::Classification::NewHost { host } => {
            body.push(format!("→ host: {} (unconfigured)", host));
        }
        crate::ipc::Classification::EscapeHatch => {
            body.push("→ ghost shell reaching outside its sandbox".to_string());
        }
        crate::ipc::Classification::UserShell { shell } => {
            body.push(format!("→ your shell {shell} (opened with /shell)"));
        }
    }
    body
}

/// The label above the approval-panel body.
pub(super) fn tool_call_panel_label(background: bool) -> &'static str {
    if background {
        "daemon · runs silently"
    } else {
        "terminal · visible to you"
    }
}

pub(super) async fn prompt_credential_ratatui(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    status: &crate::cli::render::StatusBarState<'_>,
    prompt: &str,
) -> String {
    let _ = renderer.commit(&format!("\n⚠ {}\n", prompt));

    // Read the credential in the live region, showing • for each char.
    // Two buffers: cred_real holds the actual typed value; cred_display holds masked bullets.
    let prompt_text = "  Password: ";
    let mut cred_real = String::new();
    let mut cred_display = crate::cli::input::InputLine::new();
    let _ = renderer.draw_credential_panel("sudo password", prompt, &cred_display, status);

    while let Some(b) = stdin.read_byte().await {
        match b {
            b'\r' | b'\n' => break,
            b'\x7f' | b'\x08' => {
                cred_real.pop();
                cred_display.backspace();
                let _ =
                    renderer.draw_credential_panel("sudo password", prompt, &cred_display, status);
            }
            b'\x03' | b'\x1b' => {
                cred_real.clear();
                break;
            }
            c if c >= 0x20 => {
                cred_real.push(c as char);
                cred_display.insert('•');
                let _ =
                    renderer.draw_credential_panel("sudo password", prompt, &cred_display, status);
            }
            _ => {}
        }
    }

    // Commit the final masked line to scrollback.
    let _ = renderer.commit(&format!("{}\n", prompt_text));
    cred_real
}

pub(super) async fn prompt_pane_select_ratatui(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    status: &crate::cli::render::StatusBarState<'_>,
    panes: &[crate::ipc::PaneInfo],
) -> String {
    let _ = renderer.commit("\n  ⚙ Which pane should receive this command?\n");
    for (i, pane) in panes.iter().enumerate() {
        let _ = renderer.commit(&format!(
            "  [{}]  {} — {} — {}\n",
            i + 1,
            pane.id,
            pane.cmd,
            pane.preview
        ));
    }
    let prompt_text = "  Select pane › ";
    let deadline = tokio::time::Instant::now() + APPROVAL_TIMEOUT;
    let input = read_approval_input(renderer, stdin, prompt_text, status, deadline)
        .await
        .unwrap_or_default();
    input
        .trim()
        .parse::<usize>()
        .ok()
        .and_then(|n| panes.get(n.saturating_sub(1)).map(|p| p.id.clone()))
        .unwrap_or_else(|| panes.first().map(|p| p.id.clone()).unwrap_or_default())
}

pub(super) async fn prompt_yes_no_ratatui(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    status: &crate::cli::render::StatusBarState<'_>,
    info: &str,
    prompt_text: &str,
) -> bool {
    let _ = renderer.commit(&format!("\n  ⚙ {}\n", info));
    let full_prompt = format!("  {} [y/N] › ", prompt_text);
    let deadline = tokio::time::Instant::now() + APPROVAL_TIMEOUT;
    let input = read_approval_input(renderer, stdin, &full_prompt, status, deadline)
        .await
        .unwrap_or_default();
    input.trim().to_lowercase() == "y" || input.trim().to_lowercase() == "yes"
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn prompt_write_ratatui<FAll, FInsert, FContains>(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    status: &crate::cli::render::StatusBarState<'_>,
    approval: &mut super::approval::SessionApproval,
    name: &str,
    content: &str,
    existing_content: Option<&str>,
    kind: &str,
    insert_for_session: FInsert,
    is_all_approved: FAll,
    is_name_approved: FContains,
) -> bool
where
    FAll: Fn(&super::approval::SessionApproval) -> bool,
    FInsert: Fn(&mut super::approval::SessionApproval, &str),
    FContains: Fn(&super::approval::SessionApproval, &str) -> bool,
{
    let _ = renderer.commit(&format!("\n  ⚙ AI wants to write {}: {}\n", kind, name));

    // Render diff
    let diff_lines = crate::cli::diff::render_diff(name, existing_content, content);
    for line in &diff_lines {
        let _ = renderer.commit(&format!("  {}\n", line));
    }

    let all_approved = is_all_approved(approval);
    let name_approved = is_name_approved(approval, name);

    if all_approved || name_approved {
        let _ = renderer.commit("  ✓ auto-approved (session)\n");
        return true;
    }

    let prompt_text = build_approval_prompt("session", false);
    let (approved, is_session, _user_msg) =
        prompt_with_session_approve(renderer, stdin, status, &[], &prompt_text).await;

    if approved {
        if is_session {
            insert_for_session(approval, name);
            let _ = renderer.commit(&format!(
                "  ✓ approved — edits to '{}' auto-approved for this session\n",
                name
            ));
        } else {
            let _ = renderer.commit("  ✓ approved\n");
        }
        true
    } else {
        let _ = renderer.commit("  ✗ denied\n");
        false
    }
}

pub(super) async fn prompt_schedule_write_ratatui(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    status: &crate::cli::render::StatusBarState<'_>,
    name: &str,
    kind: &str,
    action: &str,
) -> bool {
    let _ = renderer.commit(&format!(
        "\n  ⚙ AI wants to schedule: {} ({})\n  Action: {}\n",
        name, kind, action
    ));
    prompt_yes_no_ratatui(renderer, stdin, status, "", "Approve?").await
}

/// A sudo edit is never auto-approved: neither the session-wide file-edit
/// approval nor a per-path one covers an edit that runs as root.
pub(super) fn edit_file_auto_approved(
    approval: &super::approval::SessionApproval,
    path: &str,
    sudo: bool,
) -> bool {
    !sudo && (approval.file_edits_all || approval.file_edits.contains(path))
}

/// `[A]pprove for session` on a sudo edit approves this edit only.
pub(super) fn edit_file_grants_session(is_session: bool, all_approved: bool, sudo: bool) -> bool {
    is_session && !all_approved && !sudo
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn prompt_edit_file_ratatui(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    status: &crate::cli::render::StatusBarState<'_>,
    approval: &mut super::approval::SessionApproval,
    path: &str,
    operation: &str,
    existing_content: Option<&str>,
    new_content: Option<&str>,
    dest_path: Option<&str>,
    sudo: bool,
) -> (bool, Option<String>) {
    let op_label = match operation {
        "create" => "create file",
        "delete" => "delete file",
        "copy" => "copy file",
        _ => "edit file",
    };
    let _ = renderer.commit(&format!("\n  ⚙ AI wants to {}: {}\n", op_label, path));
    if sudo {
        let _ = renderer.commit("  ⚠ runs as root (sudo) on the host\n");
    }
    if operation == "copy"
        && let Some(dst) = dest_path
    {
        let _ = renderer.commit(&format!("  → destination: {}\n", dst));
    }

    // Render diff
    let diff_name = if operation == "copy" {
        dest_path.unwrap_or(path)
    } else {
        path
    };
    let diff_existing = existing_content;
    let diff_new = new_content;
    let diff_lines =
        crate::cli::diff::render_diff(diff_name, diff_existing, diff_new.unwrap_or(""));
    for line in &diff_lines {
        let _ = renderer.commit(&format!("  {}\n", line));
    }

    let all_approved = approval.file_edits_all;

    if edit_file_auto_approved(approval, path, sudo) {
        let _ = renderer.commit("  ✓ auto-approved (session)\n");
        return (true, None);
    }

    let prompt_text = build_approval_prompt("session", true);
    let (approved, is_session, user_msg) =
        prompt_with_session_approve(renderer, stdin, status, &[], &prompt_text).await;

    if approved {
        if edit_file_grants_session(is_session, all_approved, sudo) {
            approval.file_edits.insert(path.to_string());
            let _ = renderer.commit(&format!(
                "  ✓ approved — edits to '{}' auto-approved for this session\n",
                path
            ));
        } else if sudo {
            let _ =
                renderer.commit("  ✓ approved — sudo edits are never approved for the session\n");
        } else {
            let _ = renderer.commit("  ✓ approved\n");
        }
        (true, None)
    } else {
        (false, commit_declined(renderer, user_msg))
    }
}
