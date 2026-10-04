//! `/tail`'s model: one shell's terminal, rebuilt in the client from a
//! `ShellTail` stream (`ShellScreen`, then `ShellChunk`s, then `ShellExited`).
//! Pure state, no terminal I/O, so every rule here is tested without a
//! daemon or a screen.

use crate::ipc::{LogCommand, LogEvent, Response};

/// Lines of history the client keeps for scrolling back. The shell's own
/// history before the tail started is not sent; this holds what arrives.
pub const TAIL_SCROLLBACK: usize = 5_000;

/// The most bytes fed to the grid between two scroll counts while a log is
/// loaded. The count reads how far the grid moved a parked view, and the grid
/// stops moving it at `TAIL_SCROLLBACK`; ordinary output scrolls at most one
/// line per newline byte, so a piece this size stays far inside that limit.
const LOG_FEED_PIECE: usize = 1024;

/// The longest pause a replay keeps between two events, in seconds. A
/// recording where nothing happened for ten minutes replays the gap as this.
pub const REPLAY_IDLE_CAP: f64 = 2.0;

/// How often a running replay advances its clock and redraws.
const REPLAY_TICK: std::time::Duration = std::time::Duration::from_millis(33);

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
    /// True for a `/log` view of a recording rather than a live `/tail`.
    pub log: bool,
    /// For a log, where each recorded command begins, oldest first.
    pub marks: Vec<LogMark>,
    /// The mark `n`/`p` last moved to.
    pub at_mark: Option<usize>,
    /// For a log, true when the daemon dropped the oldest output to fit.
    pub truncated: bool,
    /// For a log, its output events, kept so `space` can replay them.
    events: Vec<LogEvent>,
    /// The replay in progress, if any.
    pub replay: Option<Replay>,
    /// True while the user is attached: keys go to the shell, not the view.
    pub attached: bool,
    /// While attached, true after one `Ctrl-d` while a second one is awaited.
    pub ctrl_d_pending: bool,
    /// While attached, true when the shell's foreground job is stopped.
    pub paused: bool,
}

/// A replay of a log at its recorded timing. `due[i]` is when event `i` is
/// shown, in seconds from the start, with every idle gap capped at
/// `REPLAY_IDLE_CAP`; `clock` is how far the replay has got.
#[derive(Debug, Clone, PartialEq)]
pub struct Replay {
    pub due: Vec<f64>,
    pub next: usize,
    pub clock: f64,
    pub paused: bool,
}

/// When each event is shown in a replay: its recorded time, with every gap
/// (the first event's time included) capped at `REPLAY_IDLE_CAP` and a time
/// that goes backwards treated as no gap.
pub fn replay_schedule(events: &[LogEvent]) -> Vec<f64> {
    let mut due = Vec::with_capacity(events.len());
    let mut at = 0.0;
    let mut prev = 0.0;
    for event in events {
        at += (event.t - prev).clamp(0.0, REPLAY_IDLE_CAP);
        prev = event.t;
        due.push(at);
    }
    due
}

/// Where one recorded command begins in a log view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogMark {
    /// Its first line, oldest line held = 0 (the same numbering as search).
    pub line: usize,
    pub command: String,
    pub exit_code: i32,
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
            log: false,
            marks: Vec::new(),
            at_mark: None,
            truncated: false,
            events: Vec::new(),
            replay: None,
            attached: false,
            ctrl_d_pending: false,
            paused: false,
        }
    }

    /// Resize the grid, as the shell's own terminal is resized. Clamped to
    /// one row and one column, as `new` does.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows.max(1), cols.max(1));
    }

    /// The model for a recording: every output event fed through a grid of
    /// the recorded size, each command's mark placed on the line it began on,
    /// and the view opened at the oldest line. A mark whose line has scrolled
    /// out of the kept history is dropped.
    ///
    /// vt100 numbers nothing, so the lines are counted while feeding: with the
    /// view parked one line back, the grid moves the view back by one for every
    /// line that scrolls off the screen.
    pub fn from_log(
        id: &str,
        rows: u16,
        cols: u16,
        events: &[LogEvent],
        commands: &[LogCommand],
        truncated: bool,
    ) -> Self {
        let mut model = TailModel::new(id, rows, cols, b"");
        model.follow = false;
        model.exited = true;
        model.log = true;
        model.truncated = truncated;
        model.events = events.to_vec();
        // Lines that have scrolled off the top of the screen so far.
        let mut scrolled = 0usize;
        let mut starts: Vec<usize> = Vec::with_capacity(commands.len());
        let mut next = 0;
        for (i, event) in events.iter().enumerate() {
            while next < commands.len() && commands[next].event == i {
                starts.push(scrolled + model.parser.screen().cursor_position().0 as usize);
                next += 1;
            }
            for piece in event.data.as_bytes().chunks(LOG_FEED_PIECE) {
                scrolled += model.feed_counting(piece);
            }
        }
        let first_held = scrolled.saturating_sub(model.history_len());
        model.marks = commands
            .iter()
            .zip(starts)
            .filter_map(|(c, start)| {
                start.checked_sub(first_held).map(|line| LogMark {
                    line,
                    command: c.command.clone(),
                    exit_code: c.exit_code,
                })
            })
            .collect();
        model.parser.screen_mut().set_scrollback(usize::MAX);
        model
    }

    /// Lines of history behind the screen. Leaves the view where it was.
    fn history_len(&mut self) -> usize {
        let keep = self.offset();
        self.parser.screen_mut().set_scrollback(usize::MAX);
        let held = self.offset();
        self.parser.screen_mut().set_scrollback(keep);
        held
    }

    /// Feed `bytes` and return how many lines scrolled off the screen. With
    /// history to stand in, the view is parked one line back, and the grid
    /// moves it one further back per scrolled line; with no history yet,
    /// every new history line is a scrolled one. Leaves the view at 0.
    fn feed_counting(&mut self, bytes: &[u8]) -> usize {
        let had = self.history_len();
        self.parser.screen_mut().set_scrollback(1);
        self.parser.process(bytes);
        let moved_back = self.offset();
        let now = self.history_len();
        self.parser.screen_mut().set_scrollback(0);
        if had == 0 { now } else { moved_back - 1 }
    }

    /// Move to the next (`forward`) or previous recorded command, bring its
    /// first line to the top of the view, and name it in the status line.
    fn step_mark(&mut self, forward: bool) {
        if self.marks.is_empty() {
            self.note = Some("no commands recorded in this log".to_string());
            return;
        }
        let to = match (self.at_mark, forward) {
            (None, true) => Some(0),
            (None, false) => None,
            (Some(at), true) => (at + 1 < self.marks.len()).then_some(at + 1),
            (Some(at), false) => at.checked_sub(1),
        };
        let Some(to) = to else {
            self.note = Some(
                if forward {
                    "no next command"
                } else {
                    "no previous command"
                }
                .to_string(),
            );
            return;
        };
        let rows = self.parser.screen().size().0 as usize;
        let total = self.history_len() + rows;
        self.show_line(self.marks[to].line, total);
        self.at_mark = Some(to);
        let mark = &self.marks[to];
        let command = mark.command.lines().next().unwrap_or("");
        self.note = Some(format!(
            "command {}/{} · exit {} · {command}",
            to + 1,
            self.marks.len(),
            mark.exit_code
        ));
    }

    /// True while a replay is on screen, paused or not.
    pub fn replaying(&self) -> bool {
        self.replay.is_some()
    }

    /// True while a replay is on screen and its clock is running.
    pub fn replay_running(&self) -> bool {
        self.replay.as_ref().is_some_and(|r| !r.paused)
    }

    /// Start replaying the log: clear the grid and show each event as its
    /// time comes, following the newest output.
    fn start_replay(&mut self) {
        if self.events.is_empty() {
            self.note = Some("nothing to replay".to_string());
            return;
        }
        let (rows, cols) = self.parser.screen().size();
        self.parser = vt100::Parser::new(rows, cols, TAIL_SCROLLBACK);
        self.follow = true;
        self.at_mark = None;
        self.replay = Some(Replay {
            due: replay_schedule(&self.events),
            next: 0,
            clock: 0.0,
            paused: false,
        });
        self.replay_tick(0.0);
    }

    /// Advance a running replay by `dt` seconds and show every event now due.
    /// When the last one is shown the replay ends and the log view returns.
    pub fn replay_tick(&mut self, dt: f64) {
        let Some(r) = self.replay.as_mut() else {
            return;
        };
        if r.paused {
            return;
        }
        r.clock += dt;
        while r.next < r.due.len() && r.due[r.next] <= r.clock {
            self.parser.process(self.events[r.next].data.as_bytes());
            r.next += 1;
        }
        if r.next == r.due.len() {
            self.replay = None;
            self.note = Some("replay finished".to_string());
        }
    }

    /// Show the rest of the log at once and end the replay. The grid then
    /// holds what `from_log` built, so the command marks still line up.
    fn skip_replay(&mut self) {
        let Some(r) = self.replay.take() else {
            return;
        };
        for event in &self.events[r.next..] {
            self.parser.process(event.data.as_bytes());
        }
        self.note = Some("replay skipped to the end".to_string());
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
    NextCommand,
    PrevCommand,
    ReplayStart,
    ReplayPause,
    ReplaySkip,
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

/// Decode one key for a `/log` view: `n` and `p` move between recorded
/// commands. There is no live output to follow and no search, so `f`, `/` and
/// `N` do nothing; every other key means what it means in `/tail`.
pub fn log_key_action(key: &crate::cli::input::Key) -> TailAction {
    use crate::cli::input::Key;
    match key {
        Key::Char('n') => TailAction::NextCommand,
        Key::Char('p') => TailAction::PrevCommand,
        Key::Char(' ') => TailAction::ReplayStart,
        Key::Char('f') | Key::Char('/') | Key::Char('N') => TailAction::Ignore,
        other => tail_key_action(other, false),
    }
}

/// Decode one key while a replay is on screen: `space` pauses and resumes,
/// `Ctrl-C` closes the view, and every other key skips to the end. Focus
/// changes and mouse reports are not keys the user pressed, so they do
/// nothing.
pub fn replay_key_action(key: &crate::cli::input::Key) -> TailAction {
    use crate::cli::input::Key;
    match key {
        Key::Char(' ') => TailAction::ReplayPause,
        Key::CtrlC => TailAction::Quit,
        Key::FocusGained | Key::FocusLost | Key::Mouse { .. } => TailAction::Ignore,
        _ => TailAction::ReplaySkip,
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
            TailAction::NextCommand => self.step_mark(true),
            TailAction::PrevCommand => self.step_mark(false),
            TailAction::ReplayStart => self.start_replay(),
            TailAction::ReplayPause => {
                if let Some(r) = self.replay.as_mut() {
                    r.paused = !r.paused;
                }
            }
            TailAction::ReplaySkip => self.skip_replay(),
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

    fn ev(data: &str) -> crate::ipc::LogEvent {
        crate::ipc::LogEvent {
            t: 0.0,
            data: data.to_string(),
        }
    }

    fn cmd(event: usize, command: &str, exit_code: i32) -> crate::ipc::LogCommand {
        crate::ipc::LogCommand {
            event,
            command: command.to_string(),
            exit_code,
        }
    }

    /// `n` commands, each its typed line then `out` lines of output: the
    /// events and the commands marked at each typed line.
    fn session(n: usize, out: usize) -> (Vec<crate::ipc::LogEvent>, Vec<crate::ipc::LogCommand>) {
        let mut events = Vec::new();
        let mut commands = Vec::new();
        for k in 0..n {
            commands.push(cmd(events.len(), &format!("cmd-{k}"), (k % 3) as i32));
            events.push(ev(&format!("$ cmd-{k}\r\n")));
            events.push(ev(&(0..out)
                .map(|j| format!("out {k}.{j}\r\n"))
                .collect::<String>()));
        }
        (events, commands)
    }

    /// Every kept mark is on the line that shows its command.
    fn assert_marks_on_their_lines(m: &mut TailModel) {
        let marks = m.marks.clone();
        let all = m.all_lines();
        for mark in &marks {
            assert!(
                all[mark.line].starts_with(&format!("$ {}", mark.command)),
                "mark {mark:?} is on {:?}",
                all[mark.line]
            );
        }
    }

    #[test]
    fn log_model_marks_each_command_on_its_line() {
        let (events, commands) = session(20, 3);
        let mut m = TailModel::from_log("s3-17", 24, 80, &events, &commands, false);
        assert!(m.log && m.exited && !m.follow, "a log is static");
        assert_eq!(m.marks.len(), 20);
        assert_marks_on_their_lines(&mut m);
        assert_eq!(top_line(&m), "$ cmd-0", "a log opens at its oldest line");
    }

    #[test]
    fn log_model_counts_lines_past_the_scrollback_cap() {
        // 300 commands of 26 lines each scroll ~7 800 lines past a 5 000 cap.
        let (events, commands) = session(300, 25);
        let mut m = TailModel::from_log("s3-17", 24, 80, &events, &commands, false);
        assert_eq!(
            m.marks.len(),
            193,
            "marks whose lines scrolled away are dropped"
        );
        assert_eq!(m.marks[0].command, "cmd-107", "the oldest kept mark");
        assert_marks_on_their_lines(&mut m);
    }

    #[test]
    fn log_model_counts_one_event_longer_than_the_scrollback() {
        // History already held, then one event of 6 000 lines: fed whole,
        // the parked view would stop at the 5 000-line history, the count
        // would come up ~1 000 short, and "first" would wrongly be kept.
        let events = [
            ev(&"y\r\n".repeat(100)),
            ev("$ first\r\n"),
            ev(&"x\r\n".repeat(6000)),
            ev("$ second\r\n"),
        ];
        let commands = [cmd(1, "first", 0), cmd(3, "second", 1)];
        let mut m = TailModel::from_log("s3-17", 24, 80, &events, &commands, false);
        assert_eq!(m.marks.len(), 1, "the first command scrolled away");
        assert_eq!(m.marks[0].command, "second");
        assert_marks_on_their_lines(&mut m);
    }

    #[test]
    fn log_n_and_p_step_through_commands() {
        let (events, commands) = session(20, 3);
        let mut m = TailModel::from_log("s3-17", 24, 80, &events, &commands, false);
        m.act(TailAction::PrevCommand, 10);
        assert_eq!(m.note.as_deref(), Some("no previous command"));
        m.act(TailAction::NextCommand, 10);
        assert_eq!(top_line(&m), "$ cmd-0");
        assert_eq!(m.note.as_deref(), Some("command 1/20 · exit 0 · cmd-0"));
        m.act(TailAction::NextCommand, 10);
        assert_eq!(top_line(&m), "$ cmd-1", "n moves to the next command");
        assert_eq!(m.note.as_deref(), Some("command 2/20 · exit 1 · cmd-1"));
        m.act(TailAction::PrevCommand, 10);
        assert_eq!(top_line(&m), "$ cmd-0", "p moves back");
        for _ in 0..19 {
            m.act(TailAction::NextCommand, 10);
        }
        assert_eq!(m.at_mark, Some(19));
        m.act(TailAction::NextCommand, 10);
        assert_eq!(m.note.as_deref(), Some("no next command"));
        assert_eq!(m.at_mark, Some(19), "past the end it stays");

        let mut empty = TailModel::from_log("s3-17", 24, 80, &events, &[], false);
        empty.act(TailAction::NextCommand, 10);
        assert_eq!(
            empty.note.as_deref(),
            Some("no commands recorded in this log")
        );
    }

    #[test]
    fn log_key_action_steps_commands_and_drops_follow_and_search() {
        use crate::cli::input::Key;
        assert_eq!(log_key_action(&Key::Char('n')), TailAction::NextCommand);
        assert_eq!(log_key_action(&Key::Char('p')), TailAction::PrevCommand);
        for none in [Key::Char('f'), Key::Char('/'), Key::Char('N')] {
            assert_eq!(log_key_action(&none), TailAction::Ignore, "{none:?}");
        }
        assert_eq!(log_key_action(&Key::Char('q')), TailAction::Quit);
        assert_eq!(log_key_action(&Key::CtrlC), TailAction::Quit);
        assert_eq!(log_key_action(&Key::PageUp), TailAction::PageUp);
        assert_eq!(
            tail_key_action(&Key::Char('p'), false),
            TailAction::Ignore,
            "/tail keeps its own keys"
        );
    }

    #[test]
    fn open_log_builds_a_log_or_returns_the_error() {
        let reply = Response::ShellLog {
            id: "s3-17".into(),
            rows: 24,
            cols: 80,
            events: vec![ev("$ ls\r\n")],
            commands: vec![cmd(0, "ls", 0)],
            truncated: true,
        };
        let m = open_log("s3-17", reply).expect("a ShellLog opens");
        assert!(m.log && m.truncated);
        assert_eq!(m.marks.len(), 1);
        let err = open_log("s9-1", Response::Error("no recording named s9-1".into()))
            .err()
            .unwrap();
        assert_eq!(err, "no recording named s9-1");
        let odd = open_log("s3-17", Response::ShellExited).err().unwrap();
        assert!(odd.contains("unexpected reply"), "{odd}");
    }

    #[test]
    fn render_tail_shows_the_log_status_line() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let (events, commands) = session(3, 1);
        let m = TailModel::from_log("s3-17", 5, 20, &events, &commands, true);
        let mut terminal = Terminal::new(TestBackend::new(100, 8)).unwrap();
        terminal.draw(|f| render_tail(f, &m)).unwrap();
        let bottom: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .skip(7 * 100)
            .take(100)
            .flat_map(|c| c.symbol().chars())
            .collect();
        assert!(bottom.starts_with("log s3-17"), "{bottom:?}");
        assert!(bottom.contains("3 commands"), "{bottom:?}");
        assert!(bottom.contains("oldest output dropped"), "{bottom:?}");
        assert!(bottom.contains("n/p commands"), "{bottom:?}");
    }

    #[tokio::test]
    async fn tail_loop_steps_a_log_with_n() {
        use crate::cli::input::Key;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use tokio::io::AsyncBufReadExt;
        let (events, commands) = session(5, 1);
        let mut model = TailModel::from_log("s3-17", 24, 80, &events, &commands, false);
        let mut frames = tokio::io::BufReader::new(tokio::io::empty()).lines();
        let mut keys = futures_util::stream::iter([Key::Char('n'), Key::Char('n'), Key::Char('q')]);
        let mut resizes = Box::pin(futures_util::stream::pending::<()>());
        let mut terminal = Terminal::new(TestBackend::new(80, 25)).unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tail_loop(
                &mut terminal,
                &mut model,
                &mut frames,
                &mut keys,
                &mut resizes,
            ),
        )
        .await
        .expect("q closes the loop")
        .expect("the loop ends cleanly");
        assert_eq!(model.at_mark, Some(1), "in a log, n steps commands");
    }

    /// Three events at 0 s, 1 s and 30 s: the 29 s gap replays as 2 s, so
    /// the last one is due at 3 s.
    fn timed() -> Vec<crate::ipc::LogEvent> {
        [(0.0, "one\r\n"), (1.0, "two\r\n"), (30.0, "three\r\n")]
            .iter()
            .map(|(t, data)| crate::ipc::LogEvent {
                t: *t,
                data: data.to_string(),
            })
            .collect()
    }

    fn shown(m: &TailModel) -> String {
        m.screen().contents().trim_end().to_string()
    }

    #[test]
    fn replay_schedule_caps_idle_gaps() {
        let at = |ts: &[f64]| -> Vec<crate::ipc::LogEvent> {
            ts.iter()
                .map(|t| crate::ipc::LogEvent {
                    t: *t,
                    data: String::new(),
                })
                .collect()
        };
        assert_eq!(
            replay_schedule(&at(&[0.5, 1.0, 30.0, 30.25, 29.0, 40.0])),
            [0.5, 1.0, 3.0, 3.25, 3.25, 5.25],
            "gaps over 2 s shrink to 2 s; a time going backwards is no gap"
        );
        assert_eq!(
            replay_schedule(&at(&[3600.0])),
            [2.0],
            "the wait before the first event is capped too"
        );
    }

    #[test]
    fn replay_shows_each_event_when_its_time_comes() {
        let mut m = TailModel::from_log("s3-17", 24, 80, &timed(), &[], false);
        m.act(TailAction::ReplayStart, 10);
        assert!(m.replay_running());
        assert_eq!(shown(&m), "one", "a replay starts from a clear grid");
        m.replay_tick(0.5);
        assert_eq!(shown(&m), "one");
        m.replay_tick(0.5);
        assert_eq!(shown(&m), "one\ntwo");
        m.replay_tick(1.5);
        assert!(m.replaying(), "the 29 s gap replays as 2 s, not less");
        assert_eq!(shown(&m), "one\ntwo");
        m.replay_tick(0.5);
        assert_eq!(shown(&m), "one\ntwo\nthree");
        assert!(!m.replaying(), "the replay ends with its last event");
        assert_eq!(m.note.as_deref(), Some("replay finished"));
    }

    #[test]
    fn replay_pause_holds_the_clock() {
        let mut m = TailModel::from_log("s3-17", 24, 80, &timed(), &[], false);
        m.act(TailAction::ReplayStart, 10);
        m.act(TailAction::ReplayPause, 10);
        assert!(m.replaying() && !m.replay_running());
        m.replay_tick(100.0);
        assert_eq!(shown(&m), "one", "a paused replay shows nothing new");
        m.act(TailAction::ReplayPause, 10);
        assert!(m.replay_running(), "space again resumes");
        m.replay_tick(1.0);
        assert_eq!(shown(&m), "one\ntwo");
    }

    #[test]
    fn replay_skip_shows_the_whole_log_with_its_marks() {
        let (mut events, commands) = session(20, 3);
        for (i, e) in events.iter_mut().enumerate() {
            e.t = i as f64;
        }
        let mut whole = TailModel::from_log("s3-17", 24, 80, &events, &commands, false);
        let mut m = TailModel::from_log("s3-17", 24, 80, &events, &commands, false);
        m.act(TailAction::ReplayStart, 10);
        m.replay_tick(1.0);
        assert!(m.replaying());
        m.act(TailAction::ReplaySkip, 10);
        assert!(!m.replaying());
        assert_eq!(m.note.as_deref(), Some("replay skipped to the end"));
        assert_eq!(m.all_lines(), whole.all_lines(), "the whole log is back");
        assert_marks_on_their_lines(&mut m);
        m.act(TailAction::NextCommand, 10);
        assert_eq!(top_line(&m), "$ cmd-0", "n still finds the commands");

        let mut empty = TailModel::from_log("s3-17", 24, 80, &[], &[], false);
        empty.act(TailAction::ReplayStart, 10);
        assert!(!empty.replaying());
        assert_eq!(empty.note.as_deref(), Some("nothing to replay"));
    }

    #[test]
    fn replay_keys_pause_close_or_skip() {
        use crate::cli::input::Key;
        assert_eq!(log_key_action(&Key::Char(' ')), TailAction::ReplayStart);
        assert_eq!(replay_key_action(&Key::Char(' ')), TailAction::ReplayPause);
        assert_eq!(replay_key_action(&Key::CtrlC), TailAction::Quit);
        for skip in [
            Key::Char('q'),
            Key::Char('\x1b'),
            Key::Char('n'),
            Key::Down,
            Key::Enter,
        ] {
            assert_eq!(replay_key_action(&skip), TailAction::ReplaySkip, "{skip:?}");
        }
        for none in [
            Key::FocusGained,
            Key::FocusLost,
            Key::Mouse {
                button: 0,
                col: 1,
                row: 1,
                pressed: true,
            },
        ] {
            assert_eq!(replay_key_action(&none), TailAction::Ignore, "{none:?}");
        }
    }

    #[test]
    fn render_tail_shows_the_replay_status_line() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let bottom = |m: &TailModel| -> String {
            let mut terminal = Terminal::new(TestBackend::new(140, 8)).unwrap();
            terminal.draw(|f| render_tail(f, m)).unwrap();
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .skip(7 * 140)
                .take(140)
                .flat_map(|c| c.symbol().chars())
                .collect()
        };
        let mut m = TailModel::from_log("s3-17", 5, 20, &timed(), &[], false);
        assert!(bottom(&m).contains("space replay"), "{:?}", bottom(&m));
        m.act(TailAction::ReplayStart, 10);
        m.replay_tick(0.5);
        let running = bottom(&m);
        assert!(running.starts_with("log s3-17"), "{running:?}");
        assert!(running.contains("replay 0.5s/3.0s"), "{running:?}");
        assert!(running.contains("space to pause"), "{running:?}");
        m.act(TailAction::ReplayPause, 10);
        assert!(
            bottom(&m).contains("paused · space to resume"),
            "{:?}",
            bottom(&m)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn tail_loop_replays_a_log_in_recorded_time() {
        use crate::cli::input::Key;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use std::time::Duration;
        use tokio::io::AsyncBufReadExt;
        let mut model = TailModel::from_log("s3-17", 24, 80, &timed(), &[], false);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut keys = Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|key| (key, rx))
        }));
        // Five seconds before space, then 1.5 s of replay, then close.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let _ = tx.send(Key::Char(' '));
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let _ = tx.send(Key::CtrlC);
        });
        let mut frames = tokio::io::BufReader::new(tokio::io::empty()).lines();
        let mut resizes = Box::pin(futures_util::stream::pending::<()>());
        let mut terminal = Terminal::new(TestBackend::new(80, 25)).unwrap();
        tokio::time::timeout(
            Duration::from_secs(60),
            tail_loop(
                &mut terminal,
                &mut model,
                &mut frames,
                &mut keys,
                &mut resizes,
            ),
        )
        .await
        .expect("ctrl-c closes the loop")
        .expect("the loop ends cleanly");
        assert!(model.replaying(), "closed mid-replay");
        assert_eq!(
            shown(&model),
            "one\ntwo",
            "1.5 s in: the 1 s event is shown, the 3 s one is not, and the \
             5 s before space did not count"
        );
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
