//! Slash-command handling for the interactive chat loop.
//!
//! Each command here maps to a single daemon request/response round-trip (via
//! [`request_once`]) or to a purely client-side state change, and renders its
//! result into scrollback through the ratatui renderer. The chat loop redraws
//! the live region after `handle_slash` returns.
//!
//! Commands handled directly in `chat.rs` (and therefore *not* here): `/exit`,
//! `/quit`, `/help`, `/clear`, `/new` — they only touch chat-loop state.

use crate::cli::render_ratatui::RatatuiRendererStdout;
use crate::config::{Config, prompts_dir};
use crate::ipc::{Request, Response};

use super::approval::SessionApproval;
use super::ipc_client::request_once;

/// Outcome of attempting to handle a slash command.
pub(super) enum SlashOutcome {
    /// The command was recognized and handled; redraw and continue.
    Handled,
    /// The input was not a recognized slash command.
    NotACommand,
}

/// Mutable chat-loop state a slash command may read or update.
pub(super) struct SlashCtx<'a> {
    pub(super) renderer: &'a mut RatatuiRendererStdout,
    pub(super) session_id: &'a str,
    pub(super) approval: &'a mut SessionApproval,
    pub(super) model: &'a mut String,
    pub(super) context_window: &'a mut u32,
    pub(super) current_prompt: &'a mut Option<String>,
    pub(super) target_pane: &'a mut Option<String>,
    pub(super) transcript: &'a mut crate::cli::transcript::Transcript,
}

/// Whether `cmd` looks like a command token (`/word`) rather than a path or
/// prose fragment that merely starts with a slash (e.g. `/etc/passwd`).
pub(super) fn is_command_shaped(cmd: &str) -> bool {
    cmd.len() > 1
        && cmd.starts_with('/')
        && cmd[1..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Dispatch a slash command. Returns [`SlashOutcome::NotACommand`] when the
/// first token is not one of the recognized commands so the caller can decide
/// whether to reject it (command-shaped) or send it to the LLM (prose).
pub(super) async fn handle_slash(ctx: SlashCtx<'_>, query: &str) -> SlashOutcome {
    let q = query.trim();
    let (cmd, rest) = match q.split_once(char::is_whitespace) {
        Some((c, r)) => (c, r.trim()),
        None => (q, ""),
    };

    match cmd {
        "/refresh" => cmd_refresh(ctx.renderer).await,
        "/model" | "/models" => cmd_model(ctx, rest).await,
        "/pane" | "/panes" => cmd_pane(ctx, rest).await,
        "/approvals" | "/approval" => cmd_approvals(ctx, rest),
        "/prompt" => cmd_prompt(ctx, rest),
        "/limits" => cmd_limits(ctx.renderer, ctx.session_id).await,
        "/session" | "/sessions" => cmd_session(ctx, rest).await,
        _ => return SlashOutcome::NotACommand,
    }
    SlashOutcome::Handled
}

/// Commit a single dim note line to scrollback.
fn note(r: &mut RatatuiRendererStdout, msg: &str) {
    let _ = r.commit(&format!("  {msg}\n"));
}

/// The scrollback line for a non-success daemon reply. `Error` carries a
/// human message; any other variant is a protocol mismatch we name but do
/// not debug-dump.
fn error_line(resp: &Response) -> String {
    match resp {
        Response::Error(e) => format!("✗ {e}"),
        other => format!("✗ unexpected reply from daemon ({})", other.kind()),
    }
}

fn render_error(r: &mut RatatuiRendererStdout, resp: Response) {
    note(r, &error_line(&resp));
}

// ── /refresh ─────────────────────────────────────────────────────────────────

async fn cmd_refresh(r: &mut RatatuiRendererStdout) {
    match request_once(Request::Refresh).await {
        Ok(Response::Ok) => note(r, "✓ system context refreshed"),
        Ok(other) => render_error(r, other),
        Err(e) => note(r, &format!("✗ {e}")),
    }
}

// ── /model ───────────────────────────────────────────────────────────────────

async fn cmd_model(ctx: SlashCtx<'_>, rest: &str) {
    if rest.is_empty() {
        // List configured models, marking the active one.
        match request_once(Request::ListModels {
            session_id: ctx.session_id.to_string(),
        })
        .await
        {
            Ok(Response::ModelList { models, active }) => {
                let mut body = Vec::new();
                for (key, model_id) in &models {
                    let marker = if key == &active { "●" } else { " " };
                    body.push(format!("{marker} {key:<12} {model_id}"));
                }
                body.push(String::new());
                body.push("switch with: /model <name>".to_string());
                let _ = ctx.renderer.commit_panel("models", &body, false);
            }
            Ok(other) => render_error(ctx.renderer, other),
            Err(e) => note(ctx.renderer, &format!("✗ {e}")),
        }
        return;
    }

    // Switch the session's active model on the daemon.
    match request_once(Request::SetModel {
        session_id: ctx.session_id.to_string(),
        model: rest.to_string(),
    })
    .await
    {
        Ok(Response::ModelChanged { model }) => {
            let cfg = Config::load().unwrap_or_default();
            let entry = cfg.resolve_model(Some(&model));
            *ctx.model = entry.model.clone();
            *ctx.context_window = entry.context_window();
            note(ctx.renderer, &format!("✓ model switched to {model}"));
        }
        Ok(other) => render_error(ctx.renderer, other),
        Err(e) => note(ctx.renderer, &format!("✗ {e}")),
    }
}

// ── /pane ────────────────────────────────────────────────────────────────────

/// Render the `/panes` inspector body: one section per window, rows numbered
/// globally so the number a user types into `/pane <n>` still indexes
/// `panes[n - 1]`.
///
/// Pure so the numbering and marker rules can be tested without a daemon.
fn render_pane_inspector(panes: &[crate::ipc::PaneInfo]) -> Vec<String> {
    if panes.is_empty() {
        return vec!["no targetable panes in this session".to_string()];
    }
    let mut body = Vec::new();
    let first_session = &panes[0].session;
    let mut prev_window: Option<&str> = None;
    for (i, p) in panes.iter().enumerate() {
        let number = i + 1; // global index — /pane <n> resolves panes[n - 1]
        // Emit a window header whenever the window changes.
        if prev_window.is_none_or(|prev| prev != p.window) {
            prev_window = Some(&p.window);
            // Count panes in this window.
            let count = panes[i..]
                .iter()
                .take_while(|q| q.window == p.window)
                .count();
            body.push(format!(
                "window '{}' ({} pane{})",
                p.window,
                count,
                if count == 1 { "" } else { "s" }
            ));
        }
        let marker = if p.is_target { "●" } else { " " }; // pinned target marker
        let mut row = format!(
            "{marker} [{}] {}  idx:{}  cmd:{}  status:{}",
            number, p.id, p.idx, p.cmd, p.status
        );
        if let Some(age) = p.activity_age_secs {
            if age < 30 {
                row.push_str(&format!("  [active {}s]", age));
            } else if age < 3600 {
                row.push_str(&format!("  [idle {}m]", age / 60));
            } else {
                row.push_str(&format!("  [idle {}h{}m]", age / 3600, (age % 3600) / 60));
            }
        }
        row.push_str(&format!("  cwd:{}", p.cwd));
        if p.session != *first_session {
            row.push_str(&format!("  session:{}", p.session));
        }
        body.push(row);
        if !p.preview.is_empty() {
            body.push(format!("    {}", p.preview));
        }
    }
    body.push(String::new());
    body.push("pin with: /pane <number|%id>".to_string());
    body
}

async fn cmd_pane(ctx: SlashCtx<'_>, rest: &str) {
    if rest.is_empty() {
        match request_once(Request::ListPanesForSession {
            session_id: ctx.session_id.to_string(),
        })
        .await
        {
            Ok(Response::PaneList { panes }) => {
                let body = render_pane_inspector(&panes);
                let _ = ctx.renderer.commit_panel("panes", &body, false);
            }
            Ok(other) => render_error(ctx.renderer, other),
            Err(e) => note(ctx.renderer, &format!("✗ {e}")),
        }
        return;
    }

    // Resolve the argument to a concrete pane id (`%N`).
    let pane_id = if rest.starts_with('%') {
        rest.to_string()
    } else if let Ok(n) = rest.parse::<usize>() {
        match request_once(Request::ListPanesForSession {
            session_id: ctx.session_id.to_string(),
        })
        .await
        {
            Ok(Response::PaneList { panes }) => match panes.get(n.saturating_sub(1)) {
                Some(p) => p.id.clone(),
                None => {
                    note(ctx.renderer, &format!("no pane #{n} (run /pane to list)"));
                    return;
                }
            },
            Ok(other) => {
                render_error(ctx.renderer, other);
                return;
            }
            Err(e) => {
                note(ctx.renderer, &format!("✗ {e}"));
                return;
            }
        }
    } else {
        note(ctx.renderer, "usage: /pane [<number>|%<id>]");
        return;
    };

    match request_once(Request::SetPane {
        session_id: ctx.session_id.to_string(),
        pane_id,
    })
    .await
    {
        Ok(Response::PaneChanged {
            pane_id,
            description,
        }) => {
            *ctx.target_pane = Some(pane_id);
            note(
                ctx.renderer,
                &format!("✓ foreground target pinned to {description}"),
            );
        }
        Ok(other) => render_error(ctx.renderer, other),
        Err(e) => note(ctx.renderer, &format!("✗ {e}")),
    }
}

// ── /approvals ───────────────────────────────────────────────────────────────

fn cmd_approvals(ctx: SlashCtx<'_>, rest: &str) {
    match rest {
        "" | "list" | "show" => {
            let body = vec![
                format!("commands : {}", on_off(ctx.approval.regular)),
                format!("sudo     : {}", on_off(ctx.approval.sudo)),
                format!(
                    "scripts  : {}",
                    scope(ctx.approval.scripts_all, ctx.approval.scripts.len())
                ),
                format!(
                    "runbooks : {}",
                    scope(ctx.approval.runbooks_all, ctx.approval.runbooks.len())
                ),
                format!(
                    "files    : {}",
                    scope(ctx.approval.file_edits_all, ctx.approval.file_edits.len())
                ),
                String::new(),
                "change with: /approvals on|off|revoke".to_string(),
            ];
            let _ = ctx.renderer.commit_panel("approvals", &body, false);
        }
        "on" | "auto" => {
            ctx.approval.regular = true;
            note(ctx.renderer, "✓ commands auto-approved for this session");
        }
        "off" | "manual" | "gated" => {
            ctx.approval.regular = false;
            note(ctx.renderer, "✓ commands now require approval");
        }
        "revoke" | "reset" => {
            *ctx.approval =
                SessionApproval::from_config(&crate::config::ApprovalsConfig::default());
            ctx.approval.regular = false;
            note(ctx.renderer, "✓ all session approvals revoked");
        }
        other => note(
            ctx.renderer,
            &format!("unknown: /approvals {other} (try on|off|revoke)"),
        ),
    }
}

fn on_off(b: bool) -> &'static str {
    if b { "auto" } else { "gated" }
}

fn scope(all: bool, count: usize) -> String {
    if all {
        "all".to_string()
    } else if count > 0 {
        format!("{count} approved")
    } else {
        "gated".to_string()
    }
}

// ── /prompt ──────────────────────────────────────────────────────────────────

fn cmd_prompt(ctx: SlashCtx<'_>, rest: &str) {
    if rest.is_empty() {
        let mut body = Vec::new();
        let active = ctx.current_prompt.as_deref().unwrap_or("(daemon default)");
        body.push(format!("active: {active}"));
        body.push(String::new());
        match std::fs::read_dir(prompts_dir()) {
            Ok(entries) => {
                let mut names: Vec<String> = entries
                    .filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let p = e.path();
                        if p.extension().and_then(|s| s.to_str()) == Some("toml") {
                            p.file_stem().and_then(|s| s.to_str()).map(String::from)
                        } else {
                            None
                        }
                    })
                    .collect();
                names.sort();
                if names.is_empty() {
                    body.push("no prompt files found".to_string());
                } else {
                    for n in names {
                        let marker = if ctx.current_prompt.as_deref() == Some(n.as_str()) {
                            "●"
                        } else {
                            " "
                        };
                        body.push(format!("{marker} {n}"));
                    }
                }
            }
            Err(e) => body.push(format!("(cannot read prompts dir: {e})")),
        }
        body.push(String::new());
        body.push("switch with: /prompt <name>  ·  reset: /prompt default".to_string());
        let _ = ctx.renderer.commit_panel("prompts", &body, false);
        return;
    }

    if rest == "default" || rest == "none" {
        *ctx.current_prompt = None;
        note(ctx.renderer, "✓ using the daemon default prompt");
        return;
    }

    let path = prompts_dir().join(format!("{rest}.toml"));
    if !path.exists() {
        note(
            ctx.renderer,
            &format!(
                "✗ no prompt '{rest}' in {} (run /prompt to list)",
                prompts_dir().display()
            ),
        );
        return;
    }
    *ctx.current_prompt = Some(rest.to_string());
    note(ctx.renderer, &format!("✓ system prompt set to '{rest}'"));
}

// ── /limits ──────────────────────────────────────────────────────────────────

async fn cmd_limits(r: &mut RatatuiRendererStdout, session_id: &str) {
    match request_once(Request::QueryLimits {
        session_id: session_id.to_string(),
    })
    .await
    {
        Ok(Response::LimitsInfo {
            limits,
            turn_count,
            tool_calls_this_session,
            history_len,
        }) => {
            let cap = |n: usize| {
                if n == 0 {
                    "∞".to_string()
                } else {
                    n.to_string()
                }
            };
            let cap_u = |n: u32| {
                if n == 0 {
                    "∞".to_string()
                } else {
                    n.to_string()
                }
            };
            let mut body = vec![
                format!(
                    "turns this session     : {turn_count} / {}",
                    cap(limits.max_turns)
                ),
                format!(
                    "tool calls this session: {tool_calls_this_session} / {}",
                    cap(limits.max_tool_calls_per_session)
                ),
                format!("history messages       : {history_len}"),
                String::new(),
                format!("per-tool batch cap     : {}", cap_u(limits.per_tool_batch)),
                format!(
                    "tool calls per turn    : {}",
                    cap_u(limits.total_tool_calls_per_turn)
                ),
                format!("tool result chars      : {}", cap(limits.tool_result_chars)),
            ];
            if !limits.per_tool_overrides.is_empty() {
                body.push(String::new());
                body.push("per-tool overrides:".to_string());
                for (tool, n) in &limits.per_tool_overrides {
                    body.push(format!("  {tool:<22} {}", cap_u(*n)));
                }
            }
            let _ = r.commit_panel("limits", &body, false);
        }
        Ok(other) => render_error(r, other),
        Err(e) => note(r, &format!("✗ {e}")),
    }
}

// ── /session ─────────────────────────────────────────────────────────────────

async fn cmd_session(ctx: SlashCtx<'_>, rest: &str) {
    let (sub, args) = match rest.split_once(char::is_whitespace) {
        Some((s, a)) => (s, a.trim()),
        None => (rest, ""),
    };
    let r = ctx.renderer;
    let sid = ctx.session_id.to_string();

    match sub {
        "" | "help" => {
            let body = vec![
                "/session list".to_string(),
                "/session save <name> [description] [--force]".to_string(),
                "/session load <name> [--force]".to_string(),
                "/session delete <name>".to_string(),
                "/session rename <old> <new>".to_string(),
            ];
            let _ = r.commit_panel("session", &body, false);
        }
        "list" | "ls" => match request_once(Request::ListSavedSessions).await {
            Ok(Response::SavedSessionList { sessions }) => {
                if sessions.is_empty() {
                    note(
                        r,
                        "no saved sessions yet — save one with /session save <name>",
                    );
                    return;
                }
                let mut body = Vec::new();
                for s in &sessions {
                    body.push(format!(
                        "{:<20} {} turns · {} msgs · updated {}",
                        s.name, s.turn_count, s.message_count, s.last_updated
                    ));
                    if !s.description.is_empty() {
                        body.push(format!("    {}", s.description));
                    }
                }
                let _ = r.commit_panel("saved sessions", &body, false);
            }
            Ok(other) => render_error(r, other),
            Err(e) => note(r, &format!("✗ {e}")),
        },
        "save" => {
            let force = args.split_whitespace().any(|t| t == "--force");
            let cleaned: Vec<&str> = args
                .split_whitespace()
                .filter(|t| *t != "--force")
                .collect();
            let Some((name, desc_parts)) = cleaned.split_first() else {
                note(r, "usage: /session save <name> [description] [--force]");
                return;
            };
            let description = desc_parts.join(" ");
            match request_once(Request::SaveSession {
                session_id: sid,
                name: name.to_string(),
                description,
                force,
            })
            .await
            {
                Ok(Response::SessionSaved { name }) => {
                    note(r, &format!("✓ session saved as '{name}'"))
                }
                Ok(other) => render_error(r, other),
                Err(e) => note(r, &format!("✗ {e}")),
            }
        }
        "load" | "resume" => {
            let force = args.split_whitespace().any(|t| t == "--force");
            let name = args
                .split_whitespace()
                .find(|t| *t != "--force")
                .unwrap_or("");
            if name.is_empty() {
                note(r, "usage: /session load <name> [--force]");
                return;
            }
            match request_once(Request::LoadSession {
                session_id: sid,
                name: name.to_string(),
                force,
            })
            .await
            {
                Ok(Response::SessionLoaded {
                    name,
                    message_count,
                    banner,
                    ..
                }) => {
                    note(r, &format!("✓ loaded '{name}' ({message_count} messages)"));
                    if !banner.is_empty() {
                        let _ = r.commit(&format!("{banner}\n"));
                    }
                    match crate::session_store::load_session_messages(&name, usize::MAX) {
                        Ok(msgs) => {
                            let blocks = crate::cli::transcript::blocks_from_messages(&msgs);
                            let n = blocks.len();
                            ctx.transcript.clear();
                            for block in blocks {
                                ctx.transcript.push(block);
                            }
                            ctx.transcript.push(crate::cli::transcript::Block::System {
                                text: format!("rehydrated {n} blocks from session '{name}'"),
                            });
                        }
                        Err(e) => {
                            note(r, &format!("✗ transcript rehydrate failed: {e}"));
                        }
                    }
                }
                Ok(other) => render_error(r, other),
                Err(e) => note(r, &format!("✗ {e}")),
            }
        }
        "delete" | "rm" => {
            if args.is_empty() {
                note(r, "usage: /session delete <name>");
                return;
            }
            match request_once(Request::DeleteSavedSession {
                name: args.to_string(),
            })
            .await
            {
                Ok(Response::Ok) => note(r, &format!("✓ deleted session '{args}'")),
                Ok(other) => render_error(r, other),
                Err(e) => note(r, &format!("✗ {e}")),
            }
        }
        "rename" | "mv" => {
            let parts: Vec<&str> = args.split_whitespace().collect();
            if parts.len() != 2 {
                note(r, "usage: /session rename <old> <new>");
                return;
            }
            match request_once(Request::RenameSavedSession {
                old_name: parts[0].to_string(),
                new_name: parts[1].to_string(),
            })
            .await
            {
                Ok(Response::Ok) => note(r, &format!("✓ renamed '{}' → '{}'", parts[0], parts[1])),
                Ok(other) => render_error(r, other),
                Err(e) => note(r, &format!("✗ {e}")),
            }
        }
        other => note(
            r,
            &format!("unknown: /session {other} (try list|save|load|delete|rename)"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_shaped_accepts_plain_commands() {
        assert!(is_command_shaped("/pane"));
        assert!(is_command_shaped("/session"));
        assert!(is_command_shaped("/model"));
        assert!(is_command_shaped("/a-b_c"));
    }

    #[test]
    fn command_shaped_rejects_paths_and_prose() {
        assert!(!is_command_shaped("/etc/passwd"));
        assert!(!is_command_shaped("/"));
        assert!(!is_command_shaped("/path/to/file"));
        assert!(!is_command_shaped("not a command"));
    }

    #[test]
    fn error_line_passes_through_daemon_error() {
        assert_eq!(error_line(&Response::Error("boom".into())), "✗ boom");
    }

    #[test]
    fn error_line_names_unexpected_variant_without_dump() {
        let resp = Response::ToolCallPrompt {
            id: "x".into(),
            command: "rm -rf /secret".into(),
            background: false,
            target_pane: None,
        };
        let line = error_line(&resp);
        assert!(line.contains("ToolCallPrompt"), "should name variant");
        assert!(
            !line.contains("rm -rf /secret"),
            "should not leak field value"
        );
        assert!(
            !line.contains('{'),
            "should not contain debug-struct braces"
        );
    }

    #[test]
    fn render_pane_inspector_numbers_panes_globally_not_per_window() {
        let panes = vec![
            crate::ipc::PaneInfo {
                id: "%1".into(),
                idx: 0,
                window: "main".into(),
                session: "de".into(),
                cmd: "bash".into(),
                cwd: "/home".into(),
                title: String::new(),
                status: "idle".into(),
                activity_age_secs: None,
                is_target: false,
                preview: String::new(),
            },
            crate::ipc::PaneInfo {
                id: "%2".into(),
                idx: 1,
                window: "main".into(),
                session: "de".into(),
                cmd: "vim".into(),
                cwd: "/home".into(),
                title: String::new(),
                status: "running".into(),
                activity_age_secs: None,
                is_target: false,
                preview: String::new(),
            },
            crate::ipc::PaneInfo {
                id: "%3".into(),
                idx: 0,
                window: "edit".into(),
                session: "de".into(),
                cmd: "nano".into(),
                cwd: "/tmp".into(),
                title: String::new(),
                status: "running".into(),
                activity_age_secs: None,
                is_target: false,
                preview: String::new(),
            },
        ];
        let body = render_pane_inspector(&panes);
        // Each pane gets a globally-unique number.
        assert!(body.iter().any(|l| l.contains("[1]")), "body: {body:?}");
        assert!(body.iter().any(|l| l.contains("[2]")), "body: {body:?}");
        assert!(body.iter().any(|l| l.contains("[3]")), "body: {body:?}");
        // The third pane (first in its window) is [3], not [1].
        let third_row = body
            .iter()
            .find(|l| l.contains("[3]") && l.contains("%3"))
            .expect("third pane row with [3]: {body:?}");
        assert!(
            third_row.contains("nano"),
            "row should be the edit-window pane: {third_row}"
        );
    }

    #[test]
    fn render_pane_inspector_marks_the_pinned_target() {
        let panes = vec![
            crate::ipc::PaneInfo {
                id: "%1".into(),
                idx: 0,
                window: "main".into(),
                session: "de".into(),
                cmd: "bash".into(),
                cwd: "/home".into(),
                title: String::new(),
                status: "idle".into(),
                activity_age_secs: None,
                is_target: false,
                preview: String::new(),
            },
            crate::ipc::PaneInfo {
                id: "%2".into(),
                idx: 1,
                window: "main".into(),
                session: "de".into(),
                cmd: "vim".into(),
                cwd: "/home".into(),
                title: String::new(),
                status: "running".into(),
                activity_age_secs: None,
                is_target: true,
                preview: String::new(),
            },
        ];
        let body = render_pane_inspector(&panes);
        // Find rows for each pane.
        let row1 = body
            .iter()
            .find(|l| l.contains("[1]") && l.contains("%1"))
            .expect("row for %1: {body:?}");
        let row2 = body
            .iter()
            .find(|l| l.contains("[2]") && l.contains("%2"))
            .expect("row for %2: {body:?}");
        assert!(
            !row1.starts_with("●"),
            "non-target pane should not have bullet: {row1}"
        );
        assert!(
            row2.starts_with("●"),
            "target pane should have bullet: {row2}"
        );
    }

    #[test]
    fn render_pane_inspector_groups_by_window() {
        let panes = vec![
            crate::ipc::PaneInfo {
                id: "%1".into(),
                idx: 0,
                window: "main".into(),
                session: "de".into(),
                cmd: "bash".into(),
                cwd: "/home".into(),
                title: String::new(),
                status: "idle".into(),
                activity_age_secs: None,
                is_target: false,
                preview: String::new(),
            },
            crate::ipc::PaneInfo {
                id: "%2".into(),
                idx: 1,
                window: "main".into(),
                session: "de".into(),
                cmd: "vim".into(),
                cwd: "/home".into(),
                title: String::new(),
                status: "running".into(),
                activity_age_secs: None,
                is_target: false,
                preview: String::new(),
            },
            crate::ipc::PaneInfo {
                id: "%3".into(),
                idx: 0,
                window: "edit".into(),
                session: "de".into(),
                cmd: "nano".into(),
                cwd: "/tmp".into(),
                title: String::new(),
                status: "running".into(),
                activity_age_secs: None,
                is_target: false,
                preview: String::new(),
            },
        ];
        let body = render_pane_inspector(&panes);
        // Exactly two window headers.
        let headers: Vec<&String> = body.iter().filter(|l| l.starts_with("window '")).collect();
        assert_eq!(headers.len(), 2, "expected 2 window headers: {body:?}");
        assert!(headers[0].contains("main"), "first header: {}", headers[0]);
        assert!(headers[1].contains("edit"), "second header: {}", headers[1]);
        // main rows sit between the main header and the edit header.
        let main_hdr_idx = body
            .iter()
            .position(|l| l.contains("window 'main'"))
            .unwrap();
        let edit_hdr_idx = body
            .iter()
            .position(|l| l.contains("window 'edit'"))
            .unwrap();
        assert!(
            main_hdr_idx < edit_hdr_idx,
            "main header must come before edit header"
        );
        // Two main rows between the headers.
        let main_rows = body[main_hdr_idx + 1..edit_hdr_idx]
            .iter()
            .filter(|l| l.contains("[1]") || l.contains("[2]"))
            .count();
        assert_eq!(main_rows, 2, "two main rows between headers: {body:?}");
    }

    #[test]
    fn render_pane_inspector_omits_empty_preview_and_unknown_age() {
        let panes = vec![
            crate::ipc::PaneInfo {
                id: "%1".into(),
                idx: 0,
                window: "main".into(),
                session: "de".into(),
                cmd: "bash".into(),
                cwd: "/home".into(),
                title: String::new(),
                status: "idle".into(),
                activity_age_secs: None,
                is_target: false,
                preview: String::new(),
            },
            crate::ipc::PaneInfo {
                id: "%2".into(),
                idx: 1,
                window: "main".into(),
                session: "de".into(),
                cmd: "cargo".into(),
                cwd: "/project".into(),
                title: String::new(),
                status: "running".into(),
                activity_age_secs: Some(12),
                is_target: false,
                preview: "running — Compiling daemoneye".into(),
            },
        ];
        let body = render_pane_inspector(&panes);
        // Pane %1: no age tag, no preview continuation.
        let row1 = body
            .iter()
            .find(|l| l.contains("[1]") && l.contains("%1"))
            .expect("row for %1: {body:?}");
        assert!(
            !row1.contains("[active") && !row1.contains("[idle"),
            "pane with unknown age should not show age tag: {row1}"
        );
        // Pane %2: both age tag and preview continuation present.
        let row2 = body
            .iter()
            .find(|l| l.contains("[2]") && l.contains("%2"))
            .expect("row for %2: {body:?}");
        assert!(
            row2.contains("[active 12s]"),
            "pane with known age should show age tag: {row2}"
        );
        // Preview is on a continuation line indented with spaces.
        let preview_line = body
            .iter()
            .find(|l| l.contains("Compiling daemoneye"))
            .expect("preview line should exist: {body:?}");
        assert!(
            preview_line.starts_with("    "),
            "preview should be indented: {preview_line}"
        );
    }
}
