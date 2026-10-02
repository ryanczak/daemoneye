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
    let status = format!(
        "tail {} · {rows}×{cols} · {mode} · f follow · ↑↓ PgUp/PgDn · esc to close",
        model.id
    );
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
