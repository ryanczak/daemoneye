//! The ratatui approval primitives and prompt functions: reading an
//! approval, and the tool-call, credential, pane, yes/no, write and
//! edit-file prompts.

use super::*;

// ── Ratatui interactive approval primitives ──────────────────────────────────

/// The sentinel `read_approval_input_with` returns on a Ctrl+O first key:
/// not an answer, a request to open the viewer.
pub(super) const VIEW_SIGNAL: &str = "\u{0f}";

/// What one key does to the approval line or the credential value. Pure
/// step functions the tests drive with `Key` sequences; the async readers
/// only loop `read_key` → step → redraw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ApprovalStep {
    /// Keep editing: nothing to submit yet.
    Pending,
    /// Submit this text (empty for a deny).
    Done(String),
    /// Open the transcript viewer (Ctrl+O where viewing is offered).
    View,
}

/// One key against the approval line (PE decision 2026-11-01: y/n/a take
/// effect on Enter). Every key edits the line and only Enter submits it, so
/// a redirect typed as `abort that` never approves. The submitted text goes
/// to `parse_approval_response` unchanged. A paste is one value whose CR/LF
/// become spaces; it never submits. Ctrl+C and a bare Escape submit the
/// empty line. `offered` is whether Ctrl+O opens the diff viewer (the write
/// and edit prompts).
pub(super) fn approval_step(
    line: &mut crate::cli::input::InputLine,
    key: &Key,
    offered: bool,
) -> ApprovalStep {
    match key {
        Key::Enter => ApprovalStep::Done(line.as_str()),
        Key::Char('\x1b') | Key::CtrlC => {
            line.clear();
            ApprovalStep::Done(String::new())
        }
        Key::CtrlO if offered => ApprovalStep::View,
        Key::Char(c) if !c.is_control() => {
            line.insert(*c);
            ApprovalStep::Pending
        }
        Key::Backspace => {
            line.backspace();
            ApprovalStep::Pending
        }
        Key::Paste(text) => {
            let flat: String = text
                .chars()
                .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
                .collect();
            line.insert_str(&flat);
            ApprovalStep::Pending
        }
        _ => ApprovalStep::Pending,
    }
}

/// One key against the credential value: a printable `Char` or a paste (its
/// CR/LF removed) appends, Backspace removes one char, Enter submits,
/// Ctrl+C and a bare Escape clear the value and submit it empty. Everything
/// else is ignored. No deadline: the prompt has none today and gets none.
pub(super) fn credential_step(value: &mut String, key: &Key) -> ApprovalStep {
    match key {
        Key::Enter => ApprovalStep::Done(std::mem::take(value)),
        Key::Char('\x1b') | Key::CtrlC => {
            value.clear();
            ApprovalStep::Done(String::new())
        }
        Key::Char(c) if !c.is_control() => {
            value.push(*c);
            ApprovalStep::Pending
        }
        Key::Backspace => {
            value.pop();
            ApprovalStep::Pending
        }
        Key::Paste(text) => {
            value.extend(text.chars().filter(|&c| c != '\n' && c != '\r'));
            ApprovalStep::Pending
        }
        _ => ApprovalStep::Pending,
    }
}

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

/// One bounded read: `None` at the deadline, `None` at EOF, `Some(v)` when a
/// value arrives in time. The deadline seam behind the approval reads
/// (F7); named for its original byte reader. Generic over the read's
/// `Option` payload so both the byte-level and key-level readers share it.
pub(crate) async fn read_byte_by<F, T>(deadline: tokio::time::Instant, fut: F) -> Option<T>
where
    F: std::future::Future<Output = Option<T>>,
{
    tokio::time::timeout_at(deadline, fut).await.ok().flatten()
}

/// One bounded key read: `None` at the deadline, `None` at EOF, `Some(k)`
/// when a key arrives in time. Wraps the generic [`read_byte_by`] seam.
async fn read_key_by(deadline: tokio::time::Instant, stdin: &AsyncStdin) -> Option<Key> {
    read_byte_by(deadline, read_key(stdin)).await
}

/// Read the approval line through `read_key`, rendering the prompt and input
/// in the live region (not scrollback).
///
/// Every key edits the line via [`approval_step`] and only Enter submits it
/// (PE decision 2026-11-01): a redirect typed as `abort that` is a redirect,
/// never an approval, and `a` alone records nothing. `viewing` is whether
/// Ctrl+O opens the transcript viewer on this prompt — the write and edit
/// prompts — where the step returns [`ApprovalStep::View`] and the caller
/// re-enters the read against its original deadline.
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
    read_approval_input_with(renderer, stdin, deadline, false, |r, line| {
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
    read_approval_input_with(renderer, stdin, deadline, false, |r, line| {
        let _ = r.draw_approval_panel(title, session_label, line, status);
    })
    .await
}

/// The approval reader both prompt shapes share: loop `read_key` →
/// [`approval_step`] → redraw. `redraw` paints the current input; it runs
/// once on entry and after every edit. Returns the submitted text — the
/// empty string on Ctrl+C, a bare Escape or Enter on an empty line — and
/// `None` when the deadline passes. A `ApprovalStep::View` is not returned
/// here: `viewing` is false for both shapes that call this, so Ctrl+O is
/// ignored.
async fn read_approval_input_with(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    deadline: tokio::time::Instant,
    viewing: bool,
    redraw: impl Fn(
        &mut crate::cli::render_ratatui::RatatuiRendererStdout,
        &crate::cli::input::InputLine,
    ),
) -> Option<String> {
    use crate::cli::input::InputLine;

    // Initial draw with empty input.
    let mut line = InputLine::new();
    redraw(renderer, &line);

    loop {
        let key = read_key_by(deadline, stdin).await?;
        match approval_step(&mut line, &key, viewing) {
            ApprovalStep::Pending => redraw(renderer, &line),
            ApprovalStep::Done(text) => return Some(text),
            ApprovalStep::View => return Some(String::from(VIEW_SIGNAL)),
        }
    }
}

/// The approval reader the write and edit prompts use: like
/// [`read_approval_input_with`] but a Ctrl+O opens the diff viewer and the
/// read re-enters against the same deadline. Its single caller does not
/// need the `Clone` the generic loop shape suggested — dropped.
async fn read_approval_input_with_view(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    deadline: tokio::time::Instant,
    redraw: impl Fn(
        &mut crate::cli::render_ratatui::RatatuiRendererStdout,
        &crate::cli::input::InputLine,
    ),
) -> Option<String> {
    use crate::cli::input::InputLine;

    let mut line = InputLine::new();
    redraw(renderer, &line);

    loop {
        let key = read_key_by(deadline, stdin).await?;
        match approval_step(&mut line, &key, true) {
            ApprovalStep::Pending => redraw(renderer, &line),
            ApprovalStep::Done(text) => return Some(text),
            ApprovalStep::View => return Some(String::from(VIEW_SIGNAL)),
        }
    }
}

/// Open the transcript viewer on a transcript holding only the diff. The
/// viewer runs expanded (`Block::Output` never opens collapsed), is not
/// capped at 80 lines, and its teardown re-pins the inline viewport.
async fn open_diff_viewer(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    view: &crate::cli::diff::DiffView,
) {
    let mut transcript = crate::cli::transcript::Transcript::new();
    let full_text: String = view
        .full
        .iter()
        .map(|line| {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            text.trim_start_matches(INDENT_TRIM).to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    transcript.push(crate::cli::transcript::Block::Output {
        tool_call_id: String::new(),
        full: full_text,
        shown: 0,
    });
    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change()) {
        Ok(mut sigwinch) => {
            if let Err(e) = crate::cli::viewer::run_transcript_viewer(
                stdin,
                &mut sigwinch,
                renderer,
                &transcript,
            )
            .await
            {
                let _ = renderer.commit(&format!("  ✗ viewer: {e}\n"));
            }
        }
        Err(e) => {
            // No SIGWINCH stream: the viewer still works, resize handling
            // just cannot fire. Name the failure instead of opening a
            // viewer that would misbehave on resize.
            let _ = renderer.commit(&format!("  ✗ viewer unavailable (sigwinch: {e})\n"));
        }
    }
}

/// The indent the prompt puts before every diff row.
const INDENT_TRIM: &str = "  ";

/// Commit a diff view into scrollback: every row wrapped at the renderer's
/// width with `wrap_line` (styles kept, nothing clipped, no `…`), pushed
/// through `commit_styled` so styling becomes cell attributes rather than
/// literal SGR text.
fn commit_diff_view(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    view: &crate::cli::diff::DiffView,
) {
    let width = renderer.width();
    let rows: Vec<ratatui::text::Line<'static>> = view
        .rows
        .iter()
        .flat_map(|line| crate::cli::render::wrap_line(line.clone(), width))
        .collect();
    let _ = renderer.commit_styled(&rows);
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

/// `prompt_with_session_approve` for the write and edit prompts: the plain
/// prompt shape, Ctrl+O offered. The reader loops on Ctrl+O — each press
/// opens the transcript viewer on `view`'s uncapped diff — and returns to
/// the same prompt, still waiting against its original deadline. Ctrl+O is
/// not an answer; nothing is committed for it.
async fn prompt_with_session_approve_view(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    status: &crate::cli::render::StatusBarState<'_>,
    prompt_text: &str,
    view: &crate::cli::diff::DiffView,
) -> (bool, bool, Option<String>) {
    let deadline = tokio::time::Instant::now() + APPROVAL_TIMEOUT;
    loop {
        match read_approval_input_with_view(renderer, stdin, deadline, |r, line| {
            let _ = r.draw_prompt(prompt_text, line, status);
        })
        .await
        {
            None => {
                let _ = renderer.commit("  ⏱ approval timed out — the command was not run\n");
                return (false, false, None);
            }
            Some(input) if input == VIEW_SIGNAL => {
                open_diff_viewer(renderer, stdin, view).await;
                continue;
            }
            Some(input) => return parse_approval_response(&input),
        }
    }
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

    loop {
        let key = match read_key(stdin).await {
            Some(key) => key,
            None => break,
        };
        match credential_step(&mut cred_real, &key) {
            ApprovalStep::Pending => {
                cred_display.clear();
                for _ in 0..cred_real.chars().count() {
                    cred_display.insert('•');
                }
                let _ =
                    renderer.draw_credential_panel("sudo password", prompt, &cred_display, status);
            }
            ApprovalStep::Done(_) => break,
            ApprovalStep::View => {}
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
pub(super) async fn prompt_write_ratatui<FInsert>(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    status: &crate::cli::render::StatusBarState<'_>,
    approval: &mut super::approval::SessionApproval,
    name: &str,
    content: &str,
    existing_content: Option<&str>,
    kind: &str,
    prompt_kind: super::approval::PromptKind<'_>,
    insert_for_session: FInsert,
) -> bool
where
    FInsert: Fn(&mut super::approval::SessionApproval, &str),
{
    let _ = renderer.commit(&format!("\n  ⚙ AI wants to write {}: {}\n", kind, name));

    // Render diff
    let view = crate::cli::diff::render_diff(name, existing_content, content);
    commit_diff_view(renderer, &view);

    if approval.prompt_auto_approves(&prompt_kind) {
        let _ = renderer.commit("  ✓ auto-approved (session)\n");
        return true;
    }

    let prompt_text = build_approval_prompt("session", false);
    let (approved, is_session, _user_msg) =
        prompt_with_session_approve_view(renderer, stdin, status, &prompt_text, &view).await;

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

/// The delete prompts' shared flow: show what will be removed, auto-approve
/// when the session's `deletes` category covers it, else ask Y/N. A delete
/// has no per-name session grant: `deletes` is the only thing that answers
/// one without asking, so the prompt offers Y/N and records nothing.
#[allow(clippy::too_many_arguments)] // 8 params: renderer, stdin, status, approval, kind, name, label, path
pub(super) async fn prompt_delete_ratatui(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    status: &crate::cli::render::StatusBarState<'_>,
    approval: &mut super::approval::SessionApproval,
    kind: super::approval::PromptKind<'_>,
    name: &str,
    kind_label: &str,
    path_label: &str,
) -> bool {
    let _ = renderer.commit(&format!(
        "\n  ⚙ AI wants to delete {kind_label}: {name}\n  → {path_label}\n"
    ));

    if approval.prompt_auto_approves(&kind) {
        let _ = renderer.commit("  ✓ auto-approved (session)\n");
        return true;
    }

    let approved = prompt_yes_no_ratatui(renderer, stdin, status, "", "Approve?").await;
    if approved {
        let _ = renderer.commit("  ✓ approved\n");
    }
    approved
}

/// Strip the senders' prefixes from an agent frame's `script_name`
/// (`"agents/<name>"` for a write, `"agent: <name>"` for a delete) and
/// return the bare agent name. The delete arm passes the bare name today;
/// keeping the strip here costs nothing and keeps both callers safe.
pub(super) fn agent_name_of(script_name: &str) -> String {
    script_name
        .trim_start_matches("agents/")
        .trim_start_matches("agent: ")
        .to_string()
}

pub(super) async fn prompt_schedule_write_ratatui(
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    stdin: &AsyncStdin,
    status: &crate::cli::render::StatusBarState<'_>,
    approval: &mut super::approval::SessionApproval,
    name: &str,
    kind: &str,
    action: &str,
) -> bool {
    let _ = renderer.commit(&format!(
        "\n  ⚙ AI wants to schedule: {} ({})\n  Action: {}\n",
        name, kind, action
    ));
    if approval.prompt_auto_approves(&super::approval::PromptKind::ScheduleWrite) {
        let _ = renderer.commit("  ✓ auto-approved (session)\n");
        return true;
    }
    let approved = prompt_yes_no_ratatui(renderer, stdin, status, "", "Approve?").await;
    if approved {
        let _ = renderer.commit("  ✓ approved\n");
    }
    approved
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
    let view = crate::cli::diff::render_diff(diff_name, diff_existing, diff_new.unwrap_or(""));
    commit_diff_view(renderer, &view);

    let all_approved = approval.file_edits_all;

    if edit_file_auto_approved(approval, path, sudo) {
        let _ = renderer.commit("  ✓ auto-approved (session)\n");
        return (true, None);
    }

    let prompt_text = build_approval_prompt("session", true);
    let (approved, is_session, user_msg) =
        prompt_with_session_approve_view(renderer, stdin, status, &prompt_text, &view).await;

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
