//! The tail and log viewers: opening a model from the first frame, the
//! viewer loops, and drawing the screen and its status line.

use super::*;

/// The model a tail starts from, given the stream's first frame: a
/// `ShellScreen` opens it; an `Error` (an unknown or exited shell) is the
/// message to show instead; anything else is a protocol mismatch.
pub fn open_tail(id: &str, first: Response) -> Result<TailModel, String> {
    match first {
        Response::ShellScreen { bytes, rows, cols } => Ok(TailModel::new(id, rows, cols, &bytes)),
        Response::Error(msg) => Err(msg),
        other => Err(format!("unexpected reply to ShellTail: {}", other.kind())),
    }
}

/// The model for `/log <id>` from the daemon's reply to `ShellLogRead`, or
/// the message to print when there is none.
pub fn open_log(id: &str, reply: Response) -> Result<TailModel, String> {
    match reply {
        Response::ShellLog {
            rows,
            cols,
            events,
            commands,
            truncated,
            ..
        } => Ok(TailModel::from_log(
            id, rows, cols, &events, &commands, truncated,
        )),
        Response::Error(msg) => Err(msg),
        other => Err(format!(
            "unexpected reply to ShellLogRead: {}",
            other.kind()
        )),
    }
}

/// Run `/log <id>`: read the recording from the daemon, and only if it is
/// there, show it in the alternate screen until the user closes the view.
/// Nothing streams into a log, so the viewer runs with no frame source.
pub async fn run_log_viewer(
    stdin: &crate::cli::input::AsyncStdin,
    sigwinch: &mut tokio::signal::unix::Signal,
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    id: &str,
) -> anyhow::Result<()> {
    use crossterm::cursor::Show;
    use crossterm::execute;
    use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
    use tokio::io::AsyncBufReadExt;

    let reply = crate::cli::commands::ipc_client::request_once(crate::ipc::Request::ShellLogRead {
        id: id.to_string(),
    })
    .await?;
    let mut model = open_log(id, reply).map_err(|msg| anyhow::anyhow!(msg))?;

    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let _guard = crate::cli::viewer::AltScreenGuard::new(|| {
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
        let _ = execute!(std::io::stdout(), Show);
        renderer.reanchor();
    });
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(std::io::stdout()))?;
    let mut keys = Box::pin(futures_util::stream::unfold(stdin, |stdin| async move {
        crate::cli::input::read_key(stdin)
            .await
            .map(|key| (key, stdin))
    }));
    let mut resizes = Box::pin(futures_util::stream::unfold(sigwinch, |sig| async move {
        sig.recv().await.map(|()| ((), sig))
    }));
    let mut frames = tokio::io::BufReader::new(tokio::io::empty()).lines();
    tail_loop(
        &mut terminal,
        &mut model,
        &mut frames,
        &mut keys,
        &mut resizes,
    )
    .await
}

/// Run `/tail <id>`: open a `ShellTail` on the daemon, and only if it answers
/// with a screen, take over the terminal's alternate screen until the user
/// closes the view. Output keeps arriving while the view is open; a closed
/// stream (the shell exited, or the daemon went away) marks the model exited
/// and the view stays until closed. Errors before the view opens are returned
/// for the caller to print.
pub async fn run_tail_viewer(
    stdin: &crate::cli::input::AsyncStdin,
    sigwinch: &mut tokio::signal::unix::Signal,
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    id: &str,
) -> anyhow::Result<()> {
    use crossterm::cursor::Show;
    use crossterm::execute;
    use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
    use tokio::io::AsyncBufReadExt;

    let stream = crate::cli::commands::ipc_client::connect().await?;
    let (rx, mut tx) = stream.into_split();
    crate::cli::commands::ipc_client::send_request(
        &mut tx,
        crate::ipc::Request::ShellTail {
            id: id.to_string(),
            follow: true,
        },
    )
    .await?;
    // `lines().next_line()` is cancel safe, so it can sit in `select!`;
    // `read_line` is not.
    let mut frames = tokio::io::BufReader::new(rx).lines();
    let first = match frames.next_line().await? {
        Some(line) => serde_json::from_str::<Response>(&line)?,
        None => anyhow::bail!("the daemon closed the connection"),
    };
    let mut model = open_tail(id, first).map_err(|msg| anyhow::anyhow!(msg))?;

    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let _guard = crate::cli::viewer::AltScreenGuard::new(|| {
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
        let _ = execute!(std::io::stdout(), Show);
        renderer.reanchor();
    });
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(std::io::stdout()))?;
    let mut keys = Box::pin(futures_util::stream::unfold(stdin, |stdin| async move {
        crate::cli::input::read_key(stdin)
            .await
            .map(|key| (key, stdin))
    }));
    let mut resizes = Box::pin(futures_util::stream::unfold(sigwinch, |sig| async move {
        sig.recv().await.map(|()| ((), sig))
    }));
    tail_loop(
        &mut terminal,
        &mut model,
        &mut frames,
        &mut keys,
        &mut resizes,
    )
    .await
}

/// The viewer's event loop, over any frame source, key source and resize
/// source, drawing into any ratatui backend, so a test can drive it with a
/// `TestBackend`, a duplex and a channel. Frames update the model; a closed
/// frame stream marks it exited and stops reading frames, but the view stays
/// until a key closes it or the keys end.
pub async fn tail_loop<B, R, K, W>(
    terminal: &mut ratatui::Terminal<B>,
    model: &mut TailModel,
    frames: &mut tokio::io::Lines<R>,
    keys: &mut K,
    resizes: &mut W,
) -> anyhow::Result<()>
where
    B: ratatui::backend::Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
    R: tokio::io::AsyncBufRead + Unpin,
    K: futures_util::Stream<Item = crate::cli::input::Key> + Unpin,
    W: futures_util::Stream<Item = ()> + Unpin,
{
    use futures_util::StreamExt;

    terminal.draw(|f| render_tail(f, model))?;
    let mut stream_open = true;
    let mut ticker = tokio::time::interval(REPLAY_TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // When the replay clock last moved; reset by every key, so time spent
    // paused, or before a replay starts, is never counted.
    let mut last = tokio::time::Instant::now();
    loop {
        tokio::select! {
            _ = ticker.tick(), if model.replay_running() => {
                let now = tokio::time::Instant::now();
                model.replay_tick(now.duration_since(last).as_secs_f64());
                last = now;
            }
            line = frames.next_line(), if stream_open => {
                match line {
                    Ok(Some(line)) => {
                        if let Ok(frame) = serde_json::from_str::<Response>(&line) {
                            model.apply(&frame);
                        }
                    }
                    Ok(None) | Err(_) => {
                        stream_open = false;
                        model.exited = true;
                    }
                }
            }
            _ = resizes.next() => {}
            key = keys.next() => {
                let Some(key) = key else { break };
                let page = terminal.size()?.height.saturating_sub(1) as usize;
                model.note = None;
                let action = if model.replaying() {
                    replay_key_action(&key)
                } else if model.log {
                    log_key_action(&key)
                } else {
                    tail_key_action(&key, model.typing())
                };
                if !model.act(action, page) {
                    break;
                }
                last = tokio::time::Instant::now();
            }
        }
        terminal.draw(|f| render_tail(f, model))?;
    }
    Ok(())
}

/// The status line while attached: how to leave, how to send EOF, and
/// whether a first `Ctrl-d` is waiting for a second.
fn attached_status(model: &TailModel, rows: u16, cols: u16) -> String {
    if model.exited {
        format!("attached {} · exited · any key returns to chat", model.id)
    } else if model.ctrl_d_pending {
        format!(
            "attached {} · ctrl-d again sends EOF to the shell, or wait to detach",
            model.id
        )
    } else if model.paused {
        format!(
            "attached {} · PAUSED · ctrl-p resume · ctrl-d detach (resumes)",
            model.id
        )
    } else {
        let note = match &model.note {
            Some(note) => format!(" · {note}"),
            None => String::new(),
        };
        format!(
            "attached {} · {rows}×{cols}{note} · ctrl-p pause · ctrl-c interrupt · ctrl-d detach · ctrl-d ctrl-d EOF",
            model.id
        )
    }
}

/// The status line while a replay is on screen: how far it has got of how
/// long, and whether it is paused.
fn replay_status(model: &TailModel, rows: u16, cols: u16) -> String {
    let Some(r) = &model.replay else {
        return String::new();
    };
    let total = r.due.last().copied().unwrap_or(0.0);
    let state = if r.paused {
        "paused · space to resume"
    } else {
        "space to pause"
    };
    format!(
        "log {} · {rows}×{cols} · replay {:.1}s/{total:.1}s · {state} · any key skips to the end · ctrl-c to close",
        model.id,
        r.clock.min(total)
    )
}

/// Paint `model`'s grid into the frame, cell by cell, with a one-line status
/// bar at the bottom. The grid keeps the shell's own size: when the terminal
/// is smaller, the top-left corner is shown; when it is larger, the rest is
/// left blank. Pure over `Frame`, so `TestBackend` tests cover it.
pub fn render_tail(f: &mut ratatui::Frame, model: &TailModel) {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};

    let area = f.area();
    let body_height = area.height.saturating_sub(1);
    let screen = model.screen();
    let (rows, cols) = screen.size();
    let buf = f.buffer_mut();
    for row in 0..rows.min(body_height) {
        for col in 0..cols.min(area.width) {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            let Some(out) = buf.cell_mut((area.x + col, area.y + row)) else {
                continue;
            };
            let text = cell.contents();
            if text.is_empty() {
                out.set_char(' ');
            } else {
                out.set_symbol(text);
            }
            let mut style = Style::default()
                .fg(color(cell.fgcolor()))
                .bg(color(cell.bgcolor()));
            if cell.bold() {
                style = style.add_modifier(Modifier::BOLD);
            }
            if cell.italic() {
                style = style.add_modifier(Modifier::ITALIC);
            }
            if cell.underline() {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            if cell.inverse() {
                style = style.add_modifier(Modifier::REVERSED);
            }
            out.set_style(style);
        }
    }

    let mode = if model.exited {
        "exited".to_string()
    } else if model.follow {
        "following".to_string()
    } else {
        format!("paused · {} lines back", model.offset())
    };
    if model.attached && !model.exited && !screen.hide_cursor() {
        let (row, col) = screen.cursor_position();
        if row < body_height && col < area.width {
            f.set_cursor_position((area.x + col, area.y + row));
        }
    }
    let status = match (&model.search, &model.note) {
        _ if model.attached => attached_status(model, rows, cols),
        _ if model.replay.is_some() => replay_status(model, rows, cols),
        _ if model.log => {
            let at = match &model.note {
                Some(note) => note.clone(),
                None => format!("{} commands", model.marks.len()),
            };
            let cut = if model.truncated {
                " · oldest output dropped"
            } else {
                ""
            };
            format!(
                "log {} · {rows}×{cols} · {at}{cut} · n/p commands · space replay · ↑↓ PgUp/PgDn · esc to close",
                model.id
            )
        }
        (Some(s), _) if s.typing => format!("/{}▏ · enter to search · esc to cancel", s.query),
        (_, Some(note)) => format!(
            "tail {} · {rows}×{cols} · {mode} · {note} · n/N older/newer · esc to close",
            model.id
        ),
        _ => format!(
            "tail {} · {rows}×{cols} · {mode} · f follow · / search · ↑↓ PgUp/PgDn · esc to close",
            model.id
        ),
    };
    f.render_widget(
        Line::from(Span::styled(
            status,
            Style::default().add_modifier(Modifier::DIM),
        )),
        ratatui::layout::Rect::new(area.x, area.y + body_height, area.width, 1),
    );
}

/// A vt100 colour as ratatui draws it. The 16 ANSI colours map by name so the
/// user's terminal theme applies; 256-colour and RGB pass through.
fn color(c: vt100::Color) -> ratatui::style::Color {
    use ratatui::style::Color as R;
    match c {
        vt100::Color::Default => R::Reset,
        vt100::Color::Idx(0) => R::Black,
        vt100::Color::Idx(1) => R::Red,
        vt100::Color::Idx(2) => R::Green,
        vt100::Color::Idx(3) => R::Yellow,
        vt100::Color::Idx(4) => R::Blue,
        vt100::Color::Idx(5) => R::Magenta,
        vt100::Color::Idx(6) => R::Cyan,
        vt100::Color::Idx(7) => R::Gray,
        vt100::Color::Idx(8) => R::DarkGray,
        vt100::Color::Idx(9) => R::LightRed,
        vt100::Color::Idx(10) => R::LightGreen,
        vt100::Color::Idx(11) => R::LightYellow,
        vt100::Color::Idx(12) => R::LightBlue,
        vt100::Color::Idx(13) => R::LightMagenta,
        vt100::Color::Idx(14) => R::LightCyan,
        vt100::Color::Idx(15) => R::White,
        vt100::Color::Idx(n) => R::Indexed(n),
        vt100::Color::Rgb(r, g, b) => R::Rgb(r, g, b),
    }
}
