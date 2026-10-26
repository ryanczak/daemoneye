//! The `/session`, `/runs`, `/run` and `/stop` slash commands.

use super::*;

// ── /session ─────────────────────────────────────────────────────────────────

pub(super) async fn cmd_session(ctx: SlashCtx<'_>, rest: &str) {
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
