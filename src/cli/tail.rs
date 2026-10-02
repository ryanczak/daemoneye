//! `/tail`'s model: one shell's terminal, rebuilt in the client from a
//! `ShellTail` stream (`ShellScreen`, then `ShellChunk`s, then `ShellExited`).
//! Pure state, no terminal I/O, so every rule here is tested without a
//! daemon or a screen.

use crate::ipc::Response;

/// Lines of history the client keeps for scrolling back. The shell's own
/// history before the tail started is not sent; this holds what arrives.
pub const TAIL_SCROLLBACK: usize = 5_000;

/// One tailed shell. `follow` keeps the view pinned to the newest output;
/// scrolling up pauses it, and the view then stays put while output arrives.
pub struct TailModel {
    pub id: String,
    parser: vt100::Parser,
    pub follow: bool,
    pub exited: bool,
    /// The search in progress, if any.
    pub search: Option<TailSearch>,
    /// A one-off message for the status line, cleared by the next key.
    pub note: Option<String>,
}

/// A `/` search in the tail view. `typing` is true while the query is being
/// entered; `at` is the line index of the current match, oldest line = 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailSearch {
    pub query: String,
    pub typing: bool,
    pub at: Option<usize>,
}

impl TailModel {
    /// The model for `id`, from the stream's first frame: a grid of the
    /// shell's own `rows` × `cols`, drawn from `screen`.
    pub fn new(id: &str, rows: u16, cols: u16, screen: &[u8]) -> Self {
        let mut parser = vt100::Parser::new(rows.max(1), cols.max(1), TAIL_SCROLLBACK);
        parser.process(screen);
        Self {
            id: id.to_string(),
            parser,
            follow: true,
            exited: false,
            search: None,
            note: None,
        }
    }

    /// Apply one later frame of the stream. Returns `true` when the view may
    /// have changed and should be redrawn.
    pub fn apply(&mut self, frame: &Response) -> bool {
        match frame {
            Response::ShellChunk { bytes } => {
                self.parser.process(bytes);
                if self.follow {
                    self.parser.screen_mut().set_scrollback(0);
                }
                true
            }
            Response::ShellExited => {
                self.exited = true;
                true
            }
            _ => false,
        }
    }

    /// Follow on: jump to the newest output. Follow off: hold the view where
    /// it is.
    pub fn toggle_follow(&mut self) {
        self.follow = !self.follow;
        if self.follow {
            self.parser.screen_mut().set_scrollback(0);
        }
    }

    /// Scroll back `n` lines into history. Scrolling back pauses follow.
    pub fn scroll_up(&mut self, n: usize) {
        let to = self.offset().saturating_add(n);
        self.parser.screen_mut().set_scrollback(to);
        if self.offset() > 0 {
            self.follow = false;
        }
    }

    /// Scroll `n` lines toward the newest output. Reaching it resumes follow.
    pub fn scroll_down(&mut self, n: usize) {
        let to = self.offset().saturating_sub(n);
        self.parser.screen_mut().set_scrollback(to);
        if to == 0 {
            self.follow = true;
        }
    }

    /// How many lines back from the newest output the view is.
    pub fn offset(&self) -> usize {
        self.parser.screen().scrollback()
    }

    /// The grid as it is currently in view.
    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    /// True while a search query is being typed: keys are then text.
    pub fn typing(&self) -> bool {
        self.search.as_ref().is_some_and(|s| s.typing)
    }

    /// Every line the model holds, oldest first: the scrollback, then the
    /// live screen's rows. Steps the view through the scrollback (vt100 has no
    /// line index) and puts it back where it was.
    fn all_lines(&mut self) -> Vec<String> {
        let keep = self.offset();
        let (_, cols) = self.parser.screen().size();
        self.parser.screen_mut().set_scrollback(usize::MAX);
        let max = self.offset();
        let mut out = Vec::with_capacity(max + 64);
        for k in (1..=max).rev() {
            self.parser.screen_mut().set_scrollback(k);
            out.push(
                self.parser
                    .screen()
                    .rows(0, cols)
                    .next()
                    .unwrap_or_default(),
            );
        }
        self.parser.screen_mut().set_scrollback(0);
        out.extend(self.parser.screen().rows(0, cols));
        self.parser.screen_mut().set_scrollback(keep);
        out
    }

    /// Scroll so line `idx` (oldest = 0) is in view: on the live screen it
    /// shows at offset 0, otherwise at the top. Pauses follow.
    fn show_line(&mut self, idx: usize, total: usize) {
        let rows = self.parser.screen().size().0 as usize;
        let max = total.saturating_sub(rows);
        let to = max.saturating_sub(idx);
        self.parser.screen_mut().set_scrollback(to);
        self.follow = false;
    }

    /// Find `query` (case-insensitive) starting at line `from` and moving
    /// toward older lines (`older`) or newer ones, bring the match into view
    /// and remember it. Returns `false` and leaves the view alone on no match.
    fn find(&mut self, from: usize, older: bool) -> bool {
        let Some(query) = self.search.as_ref().map(|s| s.query.to_lowercase()) else {
            return false;
        };
        let lines = self.all_lines();
        let total = lines.len();
        if total == 0 || query.is_empty() {
            return false;
        }
        let hit = if older {
            let from = from.min(total - 1);
            (0..=from)
                .rev()
                .find(|&i| lines[i].to_lowercase().contains(&query))
        } else {
            (from..total).find(|&i| lines[i].to_lowercase().contains(&query))
        };
        match hit {
            Some(i) => {
                self.show_line(i, total);
                if let Some(s) = self.search.as_mut() {
                    s.at = Some(i);
                }
                self.note = Some(format!("match at line {} of {total}", i + 1));
                true
            }
            None => {
                self.note = Some(format!("\"{query}\" not found"));
                false
            }
        }
    }

    /// Index of the bottom line in view, oldest = 0.
    fn bottom_in_view(&mut self) -> usize {
        let rows = self.parser.screen().size().0 as usize;
        let offset = self.offset();
        self.parser.screen_mut().set_scrollback(usize::MAX);
        let max = self.offset();
        self.parser.screen_mut().set_scrollback(offset);
        (max - offset.min(max)) + rows.saturating_sub(1)
    }
}

/// What a keypress means to the tail viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailAction {
    ScrollUp,
    ScrollDown,
    PageUp,
    PageDown,
    Top,
    Bottom,
    ToggleFollow,
    SearchOpen,
    SearchType(char),
    SearchBackspace,
    SearchCommit,
    SearchCancel,
    MatchOlder,
    MatchNewer,
    Quit,
    Ignore,
}

/// Decode one key for the tail viewer. `typing` is true while a search query
/// is being entered: printable keys are then text, `Enter` searches and `Esc`
/// cancels. Otherwise `Esc`, `q` and `Ctrl-O` close the view, as they close
/// the transcript viewer. `Ctrl-C` closes it in both modes.
pub fn tail_key_action(key: &crate::cli::input::Key, typing: bool) -> TailAction {
    use crate::cli::input::Key;
    if typing {
        return match key {
            Key::Char('\x1b') => TailAction::SearchCancel,
            Key::Enter => TailAction::SearchCommit,
            Key::Backspace => TailAction::SearchBackspace,
            Key::CtrlC => TailAction::Quit,
            Key::Char(c) if !c.is_control() => TailAction::SearchType(*c),
            _ => TailAction::Ignore,
        };
    }
    match key {
        Key::Char('/') => TailAction::SearchOpen,
        Key::Char('n') => TailAction::MatchOlder,
        Key::Char('N') => TailAction::MatchNewer,
        Key::CtrlC => TailAction::Quit,
        Key::Up => TailAction::ScrollUp,
        Key::Down => TailAction::ScrollDown,
        Key::PageUp => TailAction::PageUp,
        Key::PageDown => TailAction::PageDown,
        Key::Home => TailAction::Top,
        Key::End => TailAction::Bottom,
        Key::Char('f') => TailAction::ToggleFollow,
        Key::Char('\x1b') | Key::Char('q') | Key::CtrlO => TailAction::Quit,
        _ => TailAction::Ignore,
    }
}

impl TailModel {
    /// Apply one viewer action; `page` is how many lines a page scroll moves.
    /// Returns `false` when the viewer should close.
    pub fn act(&mut self, action: TailAction, page: usize) -> bool {
        match action {
            TailAction::ScrollUp => self.scroll_up(1),
            TailAction::ScrollDown => self.scroll_down(1),
            TailAction::PageUp => self.scroll_up(page.max(1)),
            TailAction::PageDown => self.scroll_down(page.max(1)),
            TailAction::Top => self.scroll_up(usize::MAX),
            TailAction::Bottom => self.scroll_down(usize::MAX),
            TailAction::ToggleFollow => self.toggle_follow(),
            TailAction::SearchOpen => {
                self.search = Some(TailSearch {
                    query: String::new(),
                    typing: true,
                    at: None,
                });
            }
            TailAction::SearchType(c) => {
                if let Some(s) = self.search.as_mut() {
                    s.query.push(c);
                }
            }
            TailAction::SearchBackspace => {
                if let Some(s) = self.search.as_mut() {
                    s.query.pop();
                }
            }
            TailAction::SearchCancel => self.search = None,
            TailAction::SearchCommit => {
                let empty = self.search.as_ref().is_none_or(|s| s.query.is_empty());
                if empty {
                    self.search = None;
                } else {
                    if let Some(s) = self.search.as_mut() {
                        s.typing = false;
                    }
                    let from = self.bottom_in_view();
                    self.find(from, true);
                }
            }
            TailAction::MatchOlder => match self.search.as_ref().and_then(|s| s.at) {
                Some(0) => self.note = Some("no older match".to_string()),
                Some(at) if !self.find(at - 1, true) => {
                    self.note = Some("no older match".to_string());
                }
                _ => {}
            },
            TailAction::MatchNewer => {
                if let Some(at) = self.search.as_ref().and_then(|s| s.at)
                    && !self.find(at + 1, false)
                {
                    self.note = Some("no newer match".to_string());
                }
            }
            TailAction::Quit => return false,
            TailAction::Ignore => {}
        }
        true
    }
}

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
    loop {
        tokio::select! {
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
                if !model.act(tail_key_action(&key, model.typing()), page) {
                    break;
                }
            }
        }
        terminal.draw(|f| render_tail(f, model))?;
    }
    Ok(())
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
    let status = match (&model.search, &model.note) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_key_action_maps_the_viewer_keys() {
        use crate::cli::input::Key;
        assert_eq!(tail_key_action(&Key::Up, false), TailAction::ScrollUp);
        assert_eq!(tail_key_action(&Key::Down, false), TailAction::ScrollDown);
        assert_eq!(tail_key_action(&Key::PageUp, false), TailAction::PageUp);
        assert_eq!(tail_key_action(&Key::PageDown, false), TailAction::PageDown);
        assert_eq!(tail_key_action(&Key::Home, false), TailAction::Top);
        assert_eq!(tail_key_action(&Key::End, false), TailAction::Bottom);
        assert_eq!(
            tail_key_action(&Key::Char('f'), false),
            TailAction::ToggleFollow
        );
        for quit in [Key::Char('\x1b'), Key::Char('q'), Key::CtrlO, Key::CtrlC] {
            assert_eq!(tail_key_action(&quit, false), TailAction::Quit, "{quit:?}");
        }
        assert_eq!(
            tail_key_action(&Key::Char('/'), false),
            TailAction::SearchOpen
        );
        assert_eq!(
            tail_key_action(&Key::Char('n'), false),
            TailAction::MatchOlder
        );
        assert_eq!(
            tail_key_action(&Key::Char('N'), false),
            TailAction::MatchNewer
        );
        for other in [Key::Char('x'), Key::Enter] {
            assert_eq!(
                tail_key_action(&other, false),
                TailAction::Ignore,
                "{other:?}"
            );
        }
    }

    #[test]
    fn tail_key_action_in_search_mode_types_text() {
        use crate::cli::input::Key;
        assert_eq!(
            tail_key_action(&Key::Char('q'), true),
            TailAction::SearchType('q')
        );
        assert_eq!(
            tail_key_action(&Key::Char('f'), true),
            TailAction::SearchType('f')
        );
        assert_eq!(tail_key_action(&Key::Enter, true), TailAction::SearchCommit);
        assert_eq!(
            tail_key_action(&Key::Char('\x1b'), true),
            TailAction::SearchCancel
        );
        assert_eq!(
            tail_key_action(&Key::Backspace, true),
            TailAction::SearchBackspace
        );
        assert_eq!(
            tail_key_action(&Key::CtrlC, true),
            TailAction::Quit,
            "Ctrl-C closes in both modes"
        );
    }

    fn search(m: &mut TailModel, query: &str) {
        m.act(TailAction::SearchOpen, 4);
        for c in query.chars() {
            m.act(TailAction::SearchType(c), 4);
        }
        m.act(TailAction::SearchCommit, 4);
    }

    #[test]
    fn tail_search_finds_the_newest_match_then_older_and_newer() {
        let mut m = TailModel::new("s3", 5, 20, b"");
        m.apply(&chunk(lines(30)));
        search(&mut m, "line 1");
        assert_eq!(
            top_line(&m),
            "line 19",
            "the newest match at or above the bottom"
        );
        assert!(!m.follow, "a search jump pauses follow");
        m.act(TailAction::MatchOlder, 4);
        assert_eq!(top_line(&m), "line 18", "n moves to the next older match");
        m.act(TailAction::MatchNewer, 4);
        assert_eq!(top_line(&m), "line 19", "N moves back to the newer one");
        m.act(TailAction::MatchNewer, 4);
        assert_eq!(m.note.as_deref(), Some("no newer match"));
    }

    #[test]
    fn tail_search_is_case_insensitive_and_reaches_the_live_screen() {
        let mut m = TailModel::new("s3", 5, 30, b"");
        m.apply(&chunk(lines(30)));
        m.apply(&chunk(b"ERROR disk full\r\n".to_vec()));
        search(&mut m, "Error");
        assert_eq!(
            m.offset(),
            0,
            "a match on the live screen shows at offset 0"
        );
        assert!(m.screen().contents().contains("ERROR disk full"));
        assert!(
            m.note
                .as_deref()
                .is_some_and(|n| n.starts_with("match at line")),
            "{:?}",
            m.note
        );
    }

    #[test]
    fn tail_search_without_a_match_keeps_the_view() {
        let mut m = TailModel::new("s3", 5, 20, b"");
        m.apply(&chunk(lines(30)));
        m.scroll_up(3);
        let before = top_line(&m);
        search(&mut m, "zzz");
        assert_eq!(
            top_line(&m),
            before,
            "no match leaves the view where it was"
        );
        assert!(
            m.note.as_deref().is_some_and(|n| n.contains("not found")),
            "{:?}",
            m.note
        );
        m.act(TailAction::SearchOpen, 4);
        m.act(TailAction::SearchCommit, 4);
        assert!(m.search.is_none(), "an empty query cancels the search");
    }

    #[test]
    fn render_tail_shows_the_search_prompt_while_typing() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut m = TailModel::new("s3", 5, 20, b"");
        m.act(TailAction::SearchOpen, 4);
        for c in "err".chars() {
            m.act(TailAction::SearchType(c), 4);
        }
        let mut terminal = Terminal::new(TestBackend::new(60, 8)).unwrap();
        terminal.draw(|f| render_tail(f, &m)).unwrap();
        let bottom: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .skip(7 * 60)
            .take(60)
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(bottom.starts_with("/err"), "{bottom:?}");
    }

    #[test]
    fn tail_model_act_pages_jumps_and_quits() {
        let mut m = TailModel::new("s3", 5, 20, b"");
        m.apply(&chunk(lines(30)));
        assert!(m.act(TailAction::PageUp, 4));
        assert_eq!(m.offset(), 4, "a page is the page size");
        assert!(!m.follow, "paging back pauses");
        assert!(m.act(TailAction::Top, 4));
        assert_eq!(top_line(&m), "line 0", "Home shows the oldest line");
        assert!(m.act(TailAction::Bottom, 4));
        assert!(m.follow, "End resumes follow");
        assert_eq!(m.offset(), 0);
        assert!(
            m.act(TailAction::Ignore, 4),
            "other keys keep the view open"
        );
        assert!(!m.act(TailAction::Quit, 4), "Quit closes the view");
    }

    #[test]
    fn open_tail_starts_from_a_screen_or_reports_the_error() {
        let m = open_tail(
            "s3",
            Response::ShellScreen {
                bytes: b"hi".to_vec(),
                rows: 30,
                cols: 100,
            },
        )
        .expect("a screen opens the tail");
        assert_eq!(m.screen().size(), (30, 100));
        assert!(m.screen().contents().starts_with("hi"));
        assert_eq!(
            open_tail("s9", Response::Error("no live shell s9".to_string())).err(),
            Some("no live shell s9".to_string())
        );
        let other = open_tail("s3", Response::Ok).err().unwrap_or_default();
        assert!(other.contains("unexpected reply"), "{other}");
    }

    #[tokio::test]
    async fn tail_loop_shows_frames_marks_exit_and_closes_on_esc() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

        let (mut daemon, client) = tokio::io::duplex(1 << 16);
        let frame = serde_json::to_string(&Response::ShellChunk {
            bytes: b"hello from s3\r\n".to_vec(),
        })
        .unwrap();
        daemon
            .write_all(format!("{frame}\n").as_bytes())
            .await
            .unwrap();
        drop(daemon);
        let mut frames = tokio::io::BufReader::new(client).lines();

        let (key_tx, key_rx) = tokio::sync::mpsc::channel(4);
        let mut keys = Box::pin(futures_util::stream::unfold(key_rx, |mut rx| async move {
            rx.recv().await.map(|key| (key, rx))
        }));
        let mut resizes = Box::pin(futures_util::stream::pending::<()>());
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        let mut model = TailModel::new("s3", 5, 20, b"");

        let press_esc = async {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            key_tx
                .send(crate::cli::input::Key::Char('\x1b'))
                .await
                .unwrap();
        };
        let run = tail_loop(
            &mut terminal,
            &mut model,
            &mut frames,
            &mut keys,
            &mut resizes,
        );
        let (outcome, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(run, press_esc)
        })
        .await
        .expect("Esc closes the loop within 5 s");
        outcome.expect("the loop ends cleanly");

        assert!(model.exited, "a closed stream marks the shell exited");
        assert!(model.screen().contents().contains("hello from s3"));
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(
            text.contains("hello from s3"),
            "the frame reached the screen: {text:?}"
        );
        assert!(
            text.contains("exited"),
            "the status line says exited: {text:?}"
        );
    }

    #[test]
    fn render_tail_paints_the_grid_with_colours_and_a_status_line() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::style::{Color, Modifier};

        let mut m = TailModel::new("s3", 5, 20, b"");
        m.apply(&chunk(b"plain \x1b[1;31mred\x1b[0m\r\nsecond".to_vec()));
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        terminal.draw(|f| render_tail(f, &m)).unwrap();
        let buf = terminal.backend().buffer();
        let row0: String = buf
            .content
            .iter()
            .take(40)
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(row0.starts_with("plain red"), "{row0:?}");
        let red = &buf.content[6];
        assert_eq!(red.symbol(), "r");
        assert_eq!(red.fg, Color::Red, "ANSI red maps by name");
        assert!(red.modifier.contains(Modifier::BOLD), "bold carries");
        assert_eq!(buf.content[0].fg, Color::Reset, "default colour is Reset");
        let row1: String = buf
            .content
            .iter()
            .skip(40)
            .take(40)
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(row1.starts_with("second"), "{row1:?}");
        let bottom: String = buf
            .content
            .iter()
            .skip(7 * 40)
            .take(40)
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(
            bottom.contains("tail s3") && bottom.contains("5×20") && bottom.contains("following"),
            "{bottom:?}"
        );
    }

    #[test]
    fn render_tail_clips_a_grid_wider_than_the_terminal() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut m = TailModel::new("s3", 3, 60, b"");
        m.apply(&chunk(format!("{}\r\n", "x".repeat(60)).into_bytes()));
        let mut terminal = Terminal::new(TestBackend::new(20, 4)).unwrap();
        terminal.draw(|f| render_tail(f, &m)).unwrap();
        let buf = terminal.backend().buffer();
        let row0: String = buf
            .content
            .iter()
            .take(20)
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert_eq!(
            row0,
            "x".repeat(20),
            "the top-left corner shows, nothing panics"
        );
        let bottom: String = buf
            .content
            .iter()
            .skip(3 * 20)
            .take(20)
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(bottom.starts_with("tail s3"), "{bottom:?}");
    }

    fn lines(n: usize) -> Vec<u8> {
        (0..n)
            .map(|i| format!("line {i}\r\n"))
            .collect::<String>()
            .into_bytes()
    }

    fn chunk(bytes: Vec<u8>) -> Response {
        Response::ShellChunk { bytes }
    }

    fn top_line(m: &TailModel) -> String {
        m.screen()
            .contents()
            .lines()
            .next()
            .unwrap_or_default()
            .to_string()
    }

    #[test]
    fn tail_model_draws_the_screen_at_the_shells_size() {
        let m = TailModel::new("s3", 30, 100, b"\x1b[1;34mhello\x1b[0m");
        assert_eq!(m.screen().size(), (30, 100), "the grid is the shell's size");
        assert!(
            m.screen().contents().starts_with("hello"),
            "{:?}",
            m.screen().contents()
        );
        assert!(m.follow, "a new tail follows");
        assert!(!m.exited);
    }

    #[test]
    fn tail_model_follows_new_output_until_scrolled_back() {
        let mut m = TailModel::new("s3", 5, 20, b"");
        assert!(m.apply(&chunk(lines(30))));
        assert_eq!(top_line(&m), "line 26", "following shows the newest lines");
        m.scroll_up(3);
        assert!(!m.follow, "scrolling back pauses follow");
        assert_eq!(top_line(&m), "line 23");
        m.apply(&chunk(b"fresh\r\n".to_vec()));
        assert_eq!(
            top_line(&m),
            "line 23",
            "a paused view stays put while output arrives"
        );
        m.scroll_down(usize::MAX);
        assert!(m.follow, "scrolling back to the bottom resumes follow");
        assert!(
            m.screen().contents().contains("fresh"),
            "{:?}",
            m.screen().contents()
        );
    }

    #[test]
    fn tail_model_toggle_follow_jumps_to_the_newest_output() {
        let mut m = TailModel::new("s3", 5, 20, b"");
        m.apply(&chunk(lines(30)));
        m.scroll_up(10);
        m.apply(&chunk(b"newest\r\n".to_vec()));
        assert!(!m.screen().contents().contains("newest"));
        m.toggle_follow();
        assert!(m.follow);
        assert_eq!(m.offset(), 0);
        assert!(
            m.screen().contents().contains("newest"),
            "{:?}",
            m.screen().contents()
        );
        m.toggle_follow();
        assert!(!m.follow, "toggling again pauses");
    }

    #[test]
    fn tail_model_marks_the_shell_exited() {
        let mut m = TailModel::new("s3", 5, 20, b"");
        assert!(m.apply(&Response::ShellExited));
        assert!(m.exited);
        assert!(!m.apply(&Response::Ok), "other frames change nothing");
    }
}
