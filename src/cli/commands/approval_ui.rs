//! Approval-prompt rendering and user-input capture.
//!
//! Each function in this module corresponds to one `Response::*Prompt` match
//! arm that used to live inline in `ask_with_session`. They render the prompt,
//! read the user's decision (Y/A/N, typed redirect message, password,
//! selection index), and return a decision value. The caller in
//! `stream.rs::ask_with_session` packages that value into the matching
//! `Request::*Response` and sends it back to the daemon.

use std::io::Write;

use crate::cli::diff::render_diff;
use crate::cli::input::{AsyncStdin, read_password_silent, restore_termios, set_raw_mode};
use crate::cli::render::{MarkdownRenderer, StatusBarState, draw_status_bar, print_tool_panel};
use crate::daemon::utils::command_has_sudo;
use crate::ipc::PaneInfo;

use super::approval::SessionApproval;
use super::stream::StreamResizeDims;

/// Shared rendering + input context threaded into every prompt.
///
/// Constructed fresh at each prompt call site inside `ask_with_session`, then
/// consumed by the prompt fn. Passing by value ends the mutable borrows on
/// `md`, `response_started`, and `approval` as soon as the prompt returns,
/// so the caller can resume using them for the outer loop.
pub(super) struct PromptCtx<'a, 'r> {
    pub(super) stdin: &'a AsyncStdin,
    pub(super) old_termios: libc::termios,
    pub(super) md: &'a mut MarkdownRenderer,
    pub(super) response_started: &'a mut bool,
    pub(super) approval: &'a mut SessionApproval,
    pub(super) resize: &'a Option<StreamResizeDims<'r>>,
    pub(super) session_id: Option<&'a str>,
    pub(super) prompt_tokens: u32,
    pub(super) context_window: u32,
}

/// Return value of `prompt_tool_call` and `prompt_edit_file`. A typed redirect
/// message carries through as `Some(text)` so the caller can route it as a
/// corrective user turn.
pub(super) type ToolDecision = (bool, Option<String>);

fn erase_spinner(response_started: &mut bool) {
    if !*response_started {
        print!("\r\x1b[K");
        *response_started = true;
    }
}

/// Redraws only the status bar after an in-flight approval state change.
///
/// `draw_status_bar` uses DEC save/restore cursor (`\x1b7`/`\x1b8`) so it is
/// safe to call mid-stream without disturbing the scroll region.  No-ops when
/// the frame has not been drawn yet (`has_frame = false`) or `resize` is `None`.
fn refresh_status_bar(
    resize: &Option<StreamResizeDims<'_>>,
    session_id: Option<&str>,
    approval: &SessionApproval,
    prompt_tokens: u32,
    context_window: u32,
) {
    let Some(d) = resize else { return };
    if !d.has_frame {
        return;
    }
    let hint = approval.hint();
    draw_status_bar(
        *d.height,
        *d.width,
        &StatusBarState {
            session_id: session_id.unwrap_or(""),
            approval_hint: &hint,
            model: &d.model,
            prompt_tokens,
            context_window,
            daemon_up: d.daemon_up,
        },
    );
}

// ── prompt_tool_call ─────────────────────────────────────────────────────────

pub(super) async fn prompt_tool_call(
    ctx: PromptCtx<'_, '_>,
    command: String,
    background: bool,
    target_pane: Option<String>,
) -> anyhow::Result<ToolDecision> {
    let PromptCtx {
        stdin,
        old_termios,
        md,
        response_started,
        approval,
        resize,
        session_id,
        prompt_tokens,
        context_window,
    } = ctx;

    erase_spinner(response_started);
    md.flush();
    println!(); // blank line before panel
    let where_label = if background {
        "daemon · runs silently"
    } else {
        "terminal · visible to you"
    };
    let cmd_line = format!("$ {}", command);

    // Resolve window-relative index for foreground target pane so the
    // user can visually map the pane ID to their tmux layout.
    let target_label = target_pane.as_deref().and_then(|tp| {
        let out = std::process::Command::new("tmux")
            .args([
                "display-message",
                "-t",
                tp,
                "-p",
                "#{pane_index}\t#{window_name}",
            ])
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        let mut parts = s.trim().splitn(2, '\t');
        let idx = parts.next()?;
        let win = parts.next().unwrap_or("");
        Some(format!("pane {} in '{}' ({})", idx, win, tp))
    });

    let mut panel_lines: Vec<String> = vec![cmd_line];
    if let Some(ref lbl) = target_label {
        panel_lines.push(format!("→ target: {}", lbl));
    }
    let panel_refs: Vec<&str> = panel_lines.iter().map(|s| s.as_str()).collect();
    print_tool_panel(where_label, &panel_refs, false);

    // Visually highlight the target pane while the user decides,
    // then immediately restore focus to the chat pane so the user
    // does not have to manually switch back.
    if let Some(ref tp) = target_pane
        && !background
    {
        let _ = std::process::Command::new("tmux")
            .args(["select-pane", "-t", tp, "-P", "bg=colour17"])
            .output();
        if let Ok(my_pane) = std::env::var("TMUX_PANE") {
            let _ = std::process::Command::new("tmux")
                .args(["select-pane", "-t", &my_pane])
                .output();
        }
    }

    let is_sudo = command_has_sudo(&command);
    let auto_approved = if is_sudo {
        approval.sudo
    } else {
        approval.regular
    };

    enum ApprovalDecision {
        Approved,
        ApprovedSession,
        Denied,
        UserMessage(String),
    }

    let decision = if auto_approved {
        println!("  \x1b[32m✓\x1b[0m \x1b[2mauto-approved (session)\x1b[0m");
        ApprovalDecision::Approved
    } else {
        let session_label = if is_sudo { "sudo session" } else { "session" };
        print!(
            "  \x1b[32mApprove?\x1b[0m \
             [\x1b[1;92mY\x1b[0m]es  \
             [\x1b[1;91mN\x1b[0m]o  \
             [\x1b[1;93mA\x1b[0m]pprove for {session_label}  \
             or type a message to redirect \
             \x1b[32m›\x1b[0m "
        );
        std::io::stdout().flush()?;

        // Temporarily revert to cooked mode for the tool approval prompt.
        restore_termios(old_termios);
        let input = stdin.read_line().await.unwrap_or_default();
        set_raw_mode()?; // back to raw mode for turn trap

        let trimmed = input.trim();
        if trimmed.eq_ignore_ascii_case("y") {
            println!("  \x1b[32m✓ approved\x1b[0m");
            ApprovalDecision::Approved
        } else if trimmed.eq_ignore_ascii_case("a") {
            if is_sudo {
                approval.sudo = true;
            } else {
                approval.regular = true;
            }
            println!(
                "  \x1b[32m✓ approved — all {} commands auto-approved for this session\x1b[0m",
                if is_sudo { "sudo" } else { "regular" }
            );
            refresh_status_bar(resize, session_id, approval, prompt_tokens, context_window);
            ApprovalDecision::ApprovedSession
        } else if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("n") {
            println!("  \x1b[2m✗ skipped\x1b[0m");
            ApprovalDecision::Denied
        } else {
            // Anything else is treated as a corrective message to the AI.
            println!("  \x1b[33m↩ redirecting agent with your message…\x1b[0m");
            ApprovalDecision::UserMessage(trimmed.to_string())
        }
    };

    let (approved, user_message) = match decision {
        ApprovalDecision::Approved | ApprovalDecision::ApprovedSession => (true, None),
        ApprovalDecision::Denied => (false, None),
        ApprovalDecision::UserMessage(msg) => (false, Some(msg)),
    };

    // Remove highlight when denied or redirected — daemon won't execute
    // so it won't clean up the highlight itself.
    if !approved
        && let Some(ref tp) = target_pane
        && !background
    {
        let _ = std::process::Command::new("tmux")
            .args(["select-pane", "-t", tp, "-P", "default"])
            .output();
    }

    md.reset();
    Ok((approved, user_message))
}

// ── prompt_credential ────────────────────────────────────────────────────────

pub(super) fn prompt_credential(md: &mut MarkdownRenderer, prompt: &str) -> String {
    md.flush();
    println!("\n\x1b[33m⚠\x1b[0m  \x1b[1m{}\x1b[0m", prompt);
    let credential = read_password_silent("   \x1b[33mPassword:\x1b[0m ").unwrap_or_default();
    md.reset();
    credential
}

// ── prompt_pane_select ───────────────────────────────────────────────────────

pub(super) async fn prompt_pane_select(
    ctx: PromptCtx<'_, '_>,
    panes: Vec<PaneInfo>,
) -> anyhow::Result<String> {
    let PromptCtx {
        stdin,
        old_termios,
        md,
        response_started,
        ..
    } = ctx;

    erase_spinner(response_started);
    md.flush();
    println!();
    println!("  \x1b[33m⚙\x1b[0m \x1b[1mWhich pane should receive this command?\x1b[0m");
    println!();
    for (i, pane) in panes.iter().enumerate() {
        println!(
            "  \x1b[32m[{}]\x1b[0m  {} — {} — {}",
            i + 1,
            pane.id,
            pane.current_cmd,
            pane.summary
        );
    }
    println!();
    print!("  Select pane \x1b[32m›\x1b[0m ");
    std::io::stdout().flush()?;
    // Temporarily revert to cooked mode for user input
    restore_termios(old_termios);
    let input = stdin.read_line().await.unwrap_or_default();
    let _ = set_raw_mode(); // back to raw mode for turn trap
    let pane_id = input
        .trim()
        .parse::<usize>()
        .ok()
        .and_then(|n| panes.get(n.saturating_sub(1)))
        .map(|p| p.id.clone())
        .unwrap_or_else(|| panes.first().map(|p| p.id.clone()).unwrap_or_default());
    md.reset();
    Ok(pane_id)
}

// ── prompt_script_delete ─────────────────────────────────────────────────────

pub(super) async fn prompt_script_delete(
    ctx: PromptCtx<'_, '_>,
    script_name: &str,
) -> anyhow::Result<bool> {
    let PromptCtx {
        stdin,
        old_termios,
        md,
        response_started,
        ..
    } = ctx;

    erase_spinner(response_started);
    md.flush();
    println!();
    println!(
        "  \x1b[33m⚙\x1b[0m \x1b[1mAI wants to delete script:\x1b[0m \x1b[96m{}\x1b[0m",
        script_name
    );
    println!();
    print!(
        "  Approve deleting ~/.daemoneye/scripts/{}? \x1b[32m[y/N]\x1b[0m \x1b[32m›\x1b[0m ",
        script_name
    );
    std::io::stdout().flush()?;
    restore_termios(old_termios);
    let input = stdin.read_line().await.unwrap_or_default();
    let _ = set_raw_mode();
    let approved = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
    md.reset();
    Ok(approved)
}

// ── prompt_script_write ──────────────────────────────────────────────────────

pub(super) async fn prompt_script_write(
    ctx: PromptCtx<'_, '_>,
    script_name: &str,
    content: &str,
    existing_content: Option<&str>,
) -> anyhow::Result<bool> {
    let PromptCtx {
        stdin,
        old_termios,
        md,
        response_started,
        approval,
        resize,
        session_id,
        prompt_tokens,
        context_window,
    } = ctx;

    erase_spinner(response_started);
    md.flush();
    println!();
    println!(
        "  \x1b[33m⚙\x1b[0m \x1b[1mAI wants to write script:\x1b[0m \x1b[96m{}\x1b[0m",
        script_name
    );
    println!();
    let diff_lines = render_diff(script_name, existing_content, content);
    for line in &diff_lines {
        println!("  {}", line);
    }
    println!();
    let approved = if approval.scripts_all || approval.scripts.contains(script_name) {
        println!("  \x1b[32m✓\x1b[0m \x1b[2mauto-approved (session)\x1b[0m");
        true
    } else {
        let prompt = if approval.scripts_all {
            "  \x1b[32mApprove?\x1b[0m \
             [\x1b[1;92my\x1b[0m]es  \
             [\x1b[1;91mN\x1b[0m]o  \
             \x1b[32m›\x1b[0m "
        } else {
            "  \x1b[32mApprove?\x1b[0m \
             [\x1b[1;92my\x1b[0m]es  \
             [\x1b[1;93mA\x1b[0m]pprove for session  \
             [\x1b[1;91mN\x1b[0m]o  \
             \x1b[32m›\x1b[0m "
        };
        print!("{}", prompt);
        std::io::stdout().flush()?;
        restore_termios(old_termios);
        let input = stdin.read_line().await.unwrap_or_default();
        let _ = set_raw_mode();
        let trimmed = input.trim();
        if trimmed.eq_ignore_ascii_case("y") || trimmed.eq_ignore_ascii_case("yes") {
            println!("  \x1b[32m✓ approved\x1b[0m");
            true
        } else if trimmed.eq_ignore_ascii_case("a") && !approval.scripts_all {
            approval.scripts.insert(script_name.to_string());
            println!(
                "  \x1b[32m✓ approved — edits to '{}' auto-approved for this session\x1b[0m",
                script_name
            );
            refresh_status_bar(resize, session_id, approval, prompt_tokens, context_window);
            true
        } else {
            println!("  \x1b[2m✗ denied\x1b[0m");
            false
        }
    };
    md.reset();
    Ok(approved)
}

// ── prompt_schedule_write ────────────────────────────────────────────────────

pub(super) async fn prompt_schedule_write(
    ctx: PromptCtx<'_, '_>,
    name: &str,
    kind: &str,
    action: &str,
) -> anyhow::Result<bool> {
    let PromptCtx {
        stdin,
        old_termios,
        md,
        response_started,
        ..
    } = ctx;

    erase_spinner(response_started);
    md.flush();
    println!();
    println!(
        "  \x1b[33m⚙\x1b[0m \x1b[1mAI wants to schedule a job:\x1b[0m \x1b[96m{}\x1b[0m",
        name
    );
    println!();
    println!("  \x1b[2mSchedule : {}\x1b[0m", kind);
    println!("  \x1b[2mAction   : {}\x1b[0m", action);
    println!();
    print!("  Approve scheduling this job? \x1b[32m[y/N]\x1b[0m \x1b[32m›\x1b[0m ");
    std::io::stdout().flush()?;
    // Temporarily revert to cooked mode for user input
    restore_termios(old_termios);
    let input = stdin.read_line().await.unwrap_or_default();
    let _ = set_raw_mode(); // back to raw mode for turn trap
    let approved = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
    md.reset();
    Ok(approved)
}

// ── prompt_runbook_write ─────────────────────────────────────────────────────

pub(super) async fn prompt_runbook_write(
    ctx: PromptCtx<'_, '_>,
    runbook_name: &str,
    content: &str,
    existing_content: Option<&str>,
) -> anyhow::Result<bool> {
    let PromptCtx {
        stdin,
        old_termios,
        md,
        response_started,
        approval,
        resize,
        session_id,
        prompt_tokens,
        context_window,
    } = ctx;

    erase_spinner(response_started);
    md.flush();
    println!();
    println!(
        "  \x1b[33m⚙\x1b[0m \x1b[1mAI wants to write runbook:\x1b[0m \x1b[96m{}\x1b[0m",
        runbook_name
    );
    println!();
    let diff_lines = render_diff(runbook_name, existing_content, content);
    for line in &diff_lines {
        println!("  {}", line);
    }
    println!();
    let approved = if approval.runbooks_all || approval.runbooks.contains(runbook_name) {
        println!("  \x1b[32m✓\x1b[0m \x1b[2mauto-approved (session)\x1b[0m");
        true
    } else {
        let prompt = if approval.runbooks_all {
            "  \x1b[32mApprove?\x1b[0m \
             [\x1b[1;92my\x1b[0m]es  \
             [\x1b[1;91mN\x1b[0m]o  \
             \x1b[32m›\x1b[0m "
        } else {
            "  \x1b[32mApprove?\x1b[0m \
             [\x1b[1;92my\x1b[0m]es  \
             [\x1b[1;93mA\x1b[0m]pprove for session  \
             [\x1b[1;91mN\x1b[0m]o  \
             \x1b[32m›\x1b[0m "
        };
        print!("{}", prompt);
        std::io::stdout().flush()?;
        restore_termios(old_termios);
        let input = stdin.read_line().await.unwrap_or_default();
        let _ = set_raw_mode();
        let trimmed = input.trim();
        if trimmed.eq_ignore_ascii_case("y") || trimmed.eq_ignore_ascii_case("yes") {
            println!("  \x1b[32m✓ approved\x1b[0m");
            true
        } else if trimmed.eq_ignore_ascii_case("a") && !approval.runbooks_all {
            approval.runbooks.insert(runbook_name.to_string());
            println!(
                "  \x1b[32m✓ approved — edits to '{}' auto-approved for this session\x1b[0m",
                runbook_name
            );
            refresh_status_bar(resize, session_id, approval, prompt_tokens, context_window);
            true
        } else {
            println!("  \x1b[2m✗ denied\x1b[0m");
            false
        }
    };
    md.reset();
    Ok(approved)
}

// ── prompt_edit_file ─────────────────────────────────────────────────────────

pub(super) async fn prompt_edit_file(
    ctx: PromptCtx<'_, '_>,
    path: &str,
    operation: &str,
    existing_content: Option<&str>,
    new_content: Option<&str>,
    dest_path: Option<&str>,
) -> anyhow::Result<ToolDecision> {
    let PromptCtx {
        stdin,
        old_termios,
        md,
        response_started,
        approval,
        resize,
        session_id,
        prompt_tokens,
        context_window,
    } = ctx;

    erase_spinner(response_started);
    md.flush();
    println!();

    let op_label = match operation {
        "create" => "create file",
        "delete" => "delete file",
        "copy" => "copy file",
        _ => "edit file",
    };
    println!(
        "  \x1b[33m⚙\x1b[0m \x1b[1mAI wants to {}:\x1b[0m \x1b[96m{}\x1b[0m",
        op_label, path
    );
    if operation == "copy"
        && let Some(dst) = dest_path
    {
        println!("  \x1b[2m→ destination: {}\x1b[0m", dst);
    }
    println!();

    // Render the diff using the same engine as script/runbook writes.
    let diff_name = if operation == "copy" {
        dest_path.unwrap_or(path)
    } else {
        path
    };
    let diff_lines = render_diff(diff_name, existing_content, new_content.unwrap_or(""));
    for line in &diff_lines {
        println!("  {}", line);
    }
    println!();

    // Session-level auto-approval is keyed by path for file edits.
    let auto_approved = approval.file_edits_all || approval.file_edits.contains(path);

    enum FileDecision {
        Approved,
        ApprovedSession,
        Denied,
        UserMessage(String),
    }

    let decision = if auto_approved {
        println!("  \x1b[32m✓\x1b[0m \x1b[2mauto-approved (session)\x1b[0m");
        FileDecision::Approved
    } else {
        let file_prompt = if approval.file_edits_all {
            "  \x1b[32mApprove?\x1b[0m \
             [\x1b[1;92my\x1b[0m]es  \
             [\x1b[1;91mN\x1b[0m]o  \
             or type a message to redirect \
             \x1b[32m›\x1b[0m "
        } else {
            "  \x1b[32mApprove?\x1b[0m \
             [\x1b[1;92my\x1b[0m]es  \
             [\x1b[1;93mA\x1b[0m]pprove for session  \
             [\x1b[1;91mN\x1b[0m]o  \
             or type a message to redirect \
             \x1b[32m›\x1b[0m "
        };
        print!("{}", file_prompt);
        std::io::stdout().flush()?;
        restore_termios(old_termios);
        let input = stdin.read_line().await.unwrap_or_default();
        let _ = set_raw_mode();
        let trimmed = input.trim();
        if trimmed.eq_ignore_ascii_case("y") || trimmed.eq_ignore_ascii_case("yes") {
            println!("  \x1b[32m✓ approved\x1b[0m");
            FileDecision::Approved
        } else if trimmed.eq_ignore_ascii_case("a") && !approval.file_edits_all {
            approval.file_edits.insert(path.to_string());
            println!(
                "  \x1b[32m✓ approved — edits to '{}' auto-approved for this session\x1b[0m",
                path
            );
            refresh_status_bar(resize, session_id, approval, prompt_tokens, context_window);
            FileDecision::ApprovedSession
        } else if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("n") {
            println!("  \x1b[2m✗ denied\x1b[0m");
            FileDecision::Denied
        } else {
            println!("  \x1b[33m↩ redirecting agent with your message…\x1b[0m");
            FileDecision::UserMessage(trimmed.to_string())
        }
    };

    let (approved, user_message) = match decision {
        FileDecision::Approved | FileDecision::ApprovedSession => (true, None),
        FileDecision::Denied => (false, None),
        FileDecision::UserMessage(msg) => (false, Some(msg)),
    };

    md.reset();
    Ok((approved, user_message))
}

// ── prompt_runbook_delete ────────────────────────────────────────────────────

pub(super) async fn prompt_runbook_delete(
    ctx: PromptCtx<'_, '_>,
    runbook_name: &str,
    active_jobs: &[String],
) -> anyhow::Result<bool> {
    let PromptCtx {
        stdin,
        old_termios,
        md,
        response_started,
        ..
    } = ctx;

    erase_spinner(response_started);
    md.flush();
    println!();
    println!(
        "  \x1b[33m⚙\x1b[0m \x1b[1mAI wants to delete runbook:\x1b[0m \x1b[96m{}\x1b[0m",
        runbook_name
    );
    if !active_jobs.is_empty() {
        println!();
        println!(
            "  \x1b[33mWarning:\x1b[0m the following scheduled jobs reference this runbook:"
        );
        for job in active_jobs {
            println!("    \x1b[2m- {}\x1b[0m", job);
        }
    }
    println!();
    print!(
        "  Approve deleting ~/.daemoneye/runbooks/{}.md? \x1b[32m[y/N]\x1b[0m \x1b[32m›\x1b[0m ",
        runbook_name
    );
    std::io::stdout().flush()?;
    restore_termios(old_termios);
    let input = stdin.read_line().await.unwrap_or_default();
    let _ = set_raw_mode();
    let approved = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
    md.reset();
    Ok(approved)
}
