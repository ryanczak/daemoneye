//! The `/session`, `/runs`, `/run` and `/stop` slash commands.

use super::*;

// ── /session ─────────────────────────────────────────────────────────────────

pub(super) async fn cmd_session(ctx: SlashCtx<'_>, rest: &str) {
    let (sub, args) = match rest.split_once(char::is_whitespace) {
        Some((s, a)) => (s, a.trim()),
        None => (rest, ""),
    };
    match sub {
        "" | "help" => session_help(ctx.renderer),
        "list" | "ls" => session_list(ctx.renderer).await,
        "save" => session_save(ctx, args).await,
        "load" | "resume" => session_load(ctx, args).await,
        "delete" | "rm" => session_delete(ctx.renderer, args).await,
        "rename" | "mv" => session_rename(ctx.renderer, args).await,
        other => note(
            ctx.renderer,
            &format!("unknown: /session {other} (try list|save|load|delete|rename)"),
        ),
    }
}

/// The `/session` help panel.
fn session_help(r: &mut RatatuiRendererStdout) {
    let body = vec![
        "/session list".to_string(),
        "/session save <name> [description] [--force]".to_string(),
        "/session load <name> [--force]".to_string(),
        "/session delete <name>".to_string(),
        "/session rename <old> <new>".to_string(),
    ];
    let _ = r.commit_panel("session", &body, false);
}

/// The `/session list` panel: one row per saved session.
async fn session_list(r: &mut RatatuiRendererStdout) {
    match request_once(Request::ListSavedSessions).await {
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
    }
}

/// `/session save <name> [description] [--force]`.
async fn session_save(ctx: SlashCtx<'_>, args: &str) {
    let r = ctx.renderer;
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
        session_id: ctx.session_id.to_string(),
        name: name.to_string(),
        description,
        force,
    })
    .await
    {
        Ok(Response::SessionSaved { name }) => note(r, &format!("✓ session saved as '{name}'")),
        Ok(other) => render_error(r, other),
        Err(e) => note(r, &format!("✗ {e}")),
    }
}

/// `/session load <name> [--force]`: switch the live session, then rehydrate
/// the transcript from the saved one.
async fn session_load(ctx: SlashCtx<'_>, args: &str) {
    let r = ctx.renderer;
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
        session_id: ctx.session_id.to_string(),
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
            rehydrate_transcript(ctx.transcript, r, &name);
        }
        Ok(other) => render_error(r, other),
        Err(e) => note(r, &format!("✗ {e}")),
    }
}

/// Replace the transcript with the saved session's messages, then a System
/// block saying how many blocks came back.
fn rehydrate_transcript(
    transcript: &mut crate::cli::transcript::Transcript,
    r: &mut RatatuiRendererStdout,
    name: &str,
) {
    match crate::session_store::load_session_messages(name, usize::MAX) {
        Ok(msgs) => {
            let blocks = crate::cli::transcript::blocks_from_messages(&msgs);
            let n = blocks.len();
            transcript.clear();
            for block in blocks {
                transcript.push(block);
            }
            transcript.push(crate::cli::transcript::Block::System {
                text: format!("rehydrated {n} blocks from session '{name}'"),
            });
        }
        Err(e) => {
            note(r, &format!("✗ transcript rehydrate failed: {e}"));
        }
    }
}

/// `/session delete <name>`.
async fn session_delete(r: &mut RatatuiRendererStdout, name: &str) {
    if name.is_empty() {
        note(r, "usage: /session delete <name>");
        return;
    }
    match request_once(Request::DeleteSavedSession {
        name: name.to_string(),
    })
    .await
    {
        Ok(Response::Ok) => note(r, &format!("✓ deleted session '{name}'")),
        Ok(other) => render_error(r, other),
        Err(e) => note(r, &format!("✗ {e}")),
    }
}

/// `/session rename <old> <new>`.
async fn session_rename(r: &mut RatatuiRendererStdout, args: &str) {
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

// ── /runs, /run, /stop ───────────────────────────────────────────────────────

pub(super) fn cmd_runs(r: &mut RatatuiRendererStdout) {
    let _ = r.commit(&crate::daemon::run::render_runs_table(
        &crate::daemon::run::list_runs(),
    ));
}

pub(super) fn cmd_run(r: &mut RatatuiRendererStdout, rest: &str) {
    let id_text = rest.trim();
    if id_text.is_empty() {
        note(r, "usage: /run <id>");
        return;
    }
    let Ok(id) = crate::daemon::run::RunId::parse(id_text) else {
        note(r, &format!("not a run id: {id_text}"));
        return;
    };
    let _ = r.commit(&crate::daemon::run::run_log_tail(&id, None));
}

pub(super) fn cmd_stop(r: &mut RatatuiRendererStdout, rest: &str) {
    let id_text = rest.trim();
    if id_text.is_empty() {
        note(r, "usage: /stop <id>");
        return;
    }
    match crate::daemon::run::request_stop(id_text) {
        Ok(_path) => note(r, &format!("stop requested for {id_text}")),
        Err(e) => note(r, &format!("✗ {e}")),
    }
}
