use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::collections::VecDeque;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::thread;
use vte::{Params, Parser, Perform};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Color {
    Named(u8),
    TrueColor(u8, u8, u8),
}

#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub struct TextAttrs {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub blink: bool,
    pub reverse: bool,
    pub strikethrough: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Cell {
    pub c: char,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub attrs: TextAttrs,
}

impl Cell {
    pub fn new(c: char) -> Self {
        Self { c, fg: None, bg: None, attrs: TextAttrs::default() }
    }
}

pub struct TerminalState {
    pub grid: Vec<Vec<Cell>>,
    pub cols: usize,
    pub rows: usize,
    pub cursor_x: usize,
    pub cursor_y: usize,
    pub dirty: bool,
    pub current_fg: Option<Color>,
    pub current_bg: Option<Color>,
    pub current_attrs: TextAttrs,
    pub scroll_top: usize,
    pub scroll_bottom: usize,
    pub wrap_pending: bool,
    pub auto_wrap: bool,
    // Cursor save/restore
    pub saved_cursor_x: usize,
    pub saved_cursor_y: usize,
    pub saved_fg: Option<Color>,
    pub saved_bg: Option<Color>,
    pub saved_attrs: TextAttrs,
    // Alternate screen buffer
    pub alt_grid: Option<Vec<Vec<Cell>>>,
    // Cursor visibility
    pub cursor_visible: bool,
    // Queue of raw bytes to write back to the PTY (DSR responses, DA, etc.)
    pub pending_responses: Vec<Vec<u8>>,
    // OSC window title update
    pub pending_title: Option<String>,
    // --- Tier 2 fields ---
    pub tab_stops: Vec<bool>,
    pub charset_g0: u8,       // b'B' = ASCII (default), b'0' = DEC line drawing
    pub insert_mode: bool,    // IRM — CSI 4h / CSI 4l
    pub bracketed_paste: bool, // CSI ?2004h / CSI ?2004l
    pub mouse_mode: u16,      // 0=off, 1000=X10, 1002=button-event, 1006=SGR
    // --- Tier 3 fields ---
    pub pending_clipboard: Option<String>, // OSC 52 clipboard content to set
    // --- Scrollback ---
    pub scrollback: VecDeque<Vec<Cell>>,
    pub scrollback_limit: usize,
    pub scroll_offset: usize, // Lines up from current grid
}

impl TerminalState {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            grid: vec![vec![Cell::new(' '); cols]; rows],
            cols,
            rows,
            cursor_x: 0,
            cursor_y: 0,
            dirty: true,
            current_fg: None,
            current_bg: None,
            current_attrs: TextAttrs::default(),
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            wrap_pending: false,
            auto_wrap: true,
            saved_cursor_x: 0,
            saved_cursor_y: 0,
            saved_fg: None,
            saved_bg: None,
            saved_attrs: TextAttrs::default(),
            alt_grid: None,
            cursor_visible: true,
            pending_responses: Vec::new(),
            pending_title: None,
            tab_stops: {
                let mut ts = vec![false; cols];
                for i in (0..cols).step_by(8) { ts[i] = true; }
                ts
            },
            charset_g0: b'B',
            insert_mode: false,
            bracketed_paste: false,
            mouse_mode: 0,
            pending_clipboard: None,
            scrollback: VecDeque::new(),
            scrollback_limit: 10000,
            scroll_offset: 0,
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.rows = rows;
        self.grid.resize(rows, vec![Cell::new(' '); cols]);
        for row in self.grid.iter_mut() {
            row.resize(cols, Cell::new(' '));
        }
        self.cursor_x = self.cursor_x.min(cols.saturating_sub(1));
        self.cursor_y = self.cursor_y.min(rows.saturating_sub(1));
        self.scroll_top = 0;
        self.scroll_bottom = rows.saturating_sub(1);
        self.scroll_offset = 0;
        self.wrap_pending = false;
        // Rebuild tab stops for new column count
        self.tab_stops = {
            let mut ts = vec![false; cols];
            for i in (0..cols).step_by(8) { ts[i] = true; }
            ts
        };
        self.dirty = true;
    }

    pub fn render_markup(&mut self) -> Option<String> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        let mut s = String::with_capacity(self.cols * self.rows * 8);

        let mut last_fg: Option<Color> = None;
        let mut last_bg: Option<Color> = None;
        let mut last_attrs = TextAttrs::default();
        let mut in_span = false;

        // Virtual window into scrollback + grid
        let total_lines = self.scrollback.len() + self.rows;
        let start_idx = total_lines.saturating_sub(self.rows).saturating_sub(self.scroll_offset);
        
        for i in 0..self.rows {
            let virtual_y = start_idx + i;
            let row: &[Cell] = if virtual_y < self.scrollback.len() {
                &self.scrollback[virtual_y]
            } else {
                let grid_y = virtual_y - self.scrollback.len();
                if grid_y < self.grid.len() {
                    &self.grid[grid_y]
                } else {
                    // Should not happen if rows are consistent, but be safe
                    &[]
                }
            };

            let y_in_grid = if virtual_y < self.scrollback.len() { None } else { Some(virtual_y - self.scrollback.len()) };

            for (x, cell) in row.iter().enumerate() {
                // Cursor is only visible if we are at the bottom (scroll_offset == 0) 
                // and the current cell matches the cursor position
                let is_cursor = self.scroll_offset == 0 && self.cursor_visible 
                    && y_in_grid == Some(self.cursor_y) && x == self.cursor_x;

                let mut eff_fg = cell.fg;
                let mut eff_bg = cell.bg;
                let eff_attrs = cell.attrs;

                if cell.attrs.reverse {
                    std::mem::swap(&mut eff_fg, &mut eff_bg);
                }
                if is_cursor {
                    let tmp = eff_fg;
                    eff_fg = Some(eff_bg.unwrap_or(Color::Named(0)));
                    eff_bg = Some(tmp.unwrap_or(Color::Named(7)));
                }

                if eff_fg != last_fg || eff_bg != last_bg || eff_attrs != last_attrs {
                    if in_span { s.push_str("</span>"); in_span = false; }
                    last_fg = eff_fg;
                    last_bg = eff_bg;
                    last_attrs = eff_attrs;

                    let need_span = eff_fg.is_some() || eff_bg.is_some()
                        || eff_attrs.bold || eff_attrs.dim || eff_attrs.italic
                        || eff_attrs.underline || eff_attrs.strikethrough;

                    if need_span {
                        s.push_str("<span");
                        if let Some(f) = eff_fg { s.push_str(&format!(" foreground=\"{}\"", color_to_hex(f))); }
                        if let Some(b) = eff_bg { s.push_str(&format!(" background=\"{}\"", color_to_hex(b))); }
                        if eff_attrs.bold        { s.push_str(" weight=\"bold\""); }
                        if eff_attrs.dim         { s.push_str(" weight=\"ultralight\""); }
                        if eff_attrs.italic      { s.push_str(" style=\"italic\""); }
                        if eff_attrs.underline   { s.push_str(" underline=\"single\""); }
                        if eff_attrs.strikethrough { s.push_str(" strikethrough=\"true\""); }
                        s.push('>');
                        in_span = true;
                    }
                }

                match cell.c {
                    '<' => s.push_str("&lt;"),
                    '>' => s.push_str("&gt;"),
                    '&' => s.push_str("&amp;"),
                    '\'' => s.push_str("&apos;"),
                    '"' => s.push_str("&quot;"),
                    c => s.push(c),
                }
            }
            if i < self.rows.saturating_sub(1) {
                if in_span { s.push_str("</span>"); in_span = false; }
                last_fg = None;
                last_bg = None;
                last_attrs = TextAttrs::default();
                s.push('\n');
            }
        }
        if in_span { s.push_str("</span>"); }
        Some(s)
    }

    fn scroll_up(&mut self) {
        if self.scroll_top <= self.scroll_bottom && self.scroll_bottom < self.rows {
            // Push to scrollback only if the scroll region is the whole screen
            if self.scroll_top == 0 && self.scroll_bottom == self.rows.saturating_sub(1) {
                let evicted_row = self.grid[0].clone();
                self.scrollback.push_back(evicted_row);
                if self.scrollback.len() > self.scrollback_limit {
                    self.scrollback.pop_front();
                }
                // If we are currently "scrolled up", scrolling the grid moves the window back relative to history
                // but usually, new output forces a reset to bottom (scroll_offset = 0)
                // We'll reset it to 0 in main.rs upon output, or here if we want strict follow.
            }
            
            // Use rotation to keep scrolling strictly localized to the region
            self.grid[self.scroll_top..=self.scroll_bottom].rotate_left(1);
            // Clear the new bottom line with the current background
            let empty_row = vec![Cell { c: ' ', fg: None, bg: self.current_bg , attrs: TextAttrs::default() }; self.cols];
            self.grid[self.scroll_bottom] = empty_row;
        }
    }

    pub fn scroll_by(&mut self, delta: i32) {
        let new_offset = (self.scroll_offset as i32 + delta).max(0);
        self.scroll_offset = (new_offset as usize).min(self.scrollback.len());
        self.dirty = true;
    }
}

fn color_to_hex(color: Color) -> String {
    match color {
        Color::Named(c) => match c {
            0 => "#000000".to_string(), // black
            1 => "#cd0000".to_string(), // red
            2 => "#00cd00".to_string(), // green
            3 => "#cdcd00".to_string(), // yellow
            4 => "#0000ee".to_string(), // blue
            5 => "#cd00cd".to_string(), // magenta
            6 => "#00cdcd".to_string(), // cyan
            7 => "#e5e5e5".to_string(), // white
            8 => "#7f7f7f".to_string(), // bright black
            9 => "#ff0000".to_string(), // bright red
            10 => "#00ff00".to_string(), // bright green
            11 => "#ffff00".to_string(), // bright yellow
            12 => "#5c5cff".to_string(), // bright blue
            13 => "#ff00ff".to_string(), // bright magenta
            14 => "#00ffff".to_string(), // bright cyan
            15 => "#ffffff".to_string(), // bright white
            16..=231 => {
                let idx = c - 16;
                let r = if idx / 36 == 0 { 0 } else { (idx / 36) * 40 + 55 };
                let g = if (idx / 6) % 6 == 0 { 0 } else { ((idx / 6) % 6) * 40 + 55 };
                let b = if idx % 6 == 0 { 0 } else { (idx % 6) * 40 + 55 };
                format!("#{:02x}{:02x}{:02x}", r, g, b)
            }
            232..=255 => {
                let gray = (c - 232) * 10 + 8;
                format!("#{:02x}{:02x}{:02x}", gray, gray, gray)
            }
        },
        Color::TrueColor(r, g, b) => format!("#{:02x}{:02x}{:02x}", r, g, b),
    }
}

impl Perform for TerminalState {
    fn print(&mut self, c: char) {
        // DEC line-drawing charset translation
        let c = if self.charset_g0 == b'0' {
            match c {
                'j' => '\u{2518}', // ┘
                'k' => '\u{2510}', // ┐
                'l' => '\u{250C}', // ┌
                'm' => '\u{2514}', // └
                'n' => '\u{253C}', // ┼
                'q' => '\u{2500}', // ─
                't' => '\u{251C}', // ├
                'u' => '\u{2524}', // ┤
                'v' => '\u{2534}', // ┴
                'w' => '\u{252C}', // ┬
                'x' => '\u{2502}', // │
                'a' => '\u{2592}', // ▒
                '`' => '\u{25C6}', // ◆
                _ => c,
            }
        } else {
            c
        };

        // 1. Handle "Pending Wrap" logic
        if self.wrap_pending && self.auto_wrap {
            self.wrap_pending = false;
            self.cursor_x = 0;
            if self.cursor_y == self.scroll_bottom {
                self.scroll_up();
            } else if self.cursor_y < self.rows.saturating_sub(1) {
                self.cursor_y += 1;
            }
        }

        if self.cursor_y < self.rows && self.cursor_x < self.cols {
            // Insert mode: shift characters right before writing
            if self.insert_mode {
                let row = &mut self.grid[self.cursor_y];
                row.pop(); // drop last char
                row.insert(self.cursor_x, Cell {
                    c,
                    fg: self.current_fg,
                    bg: self.current_bg,
                    attrs: self.current_attrs,
                });
            } else {
                self.grid[self.cursor_y][self.cursor_x] = Cell {
                    c,
                    fg: self.current_fg,
                    bg: self.current_bg,
                    attrs: self.current_attrs,
                };
            }
            
            // 2. Advance cursor or set pending wrap
            if self.cursor_x < self.cols.saturating_sub(1) {
                self.cursor_x += 1;
            } else {
                if self.auto_wrap {
                    self.wrap_pending = true;
                }
            }
            self.dirty = true;
        }
    }

    fn execute(&mut self, byte: u8) {
        self.wrap_pending = false;
        match byte {
            b'\n' => {
                if self.cursor_y == self.scroll_bottom {
                    self.scroll_up();
                } else if self.cursor_y < self.rows.saturating_sub(1) {
                    self.cursor_y += 1;
                }
            }
            b'\r' => {
                self.cursor_x = 0;
            }
            b'\x08' => {
                if self.cursor_x > 0 {
                    self.cursor_x -= 1;
                }
            }
            b'\t' => {
                // Advance to next tab stop
                let start = self.cursor_x + 1;
                let mut found = false;
                for i in start..self.cols {
                    if i < self.tab_stops.len() && self.tab_stops[i] {
                        self.cursor_x = i;
                        found = true;
                        break;
                    }
                }
                if !found {
                    self.cursor_x = self.cols.saturating_sub(1);
                }
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.len() >= 2 {
            let cmd = params[0];
            if cmd == b"0" || cmd == b"2" {
                // OSC 0/2: set window title
                if let Ok(title) = std::str::from_utf8(params[1]) {
                    self.pending_title = Some(title.to_string());
                }
            } else if cmd == b"52" {
                // OSC 52: clipboard operation
                // Format: OSC 52 ; selection ; base64-data ST
                // We store the base64 data for main.rs to decode and set
                if params.len() >= 3 {
                    if let Ok(data) = std::str::from_utf8(params[2]) {
                        if data != "?" {
                            // base64-encoded clipboard content
                            self.pending_clipboard = Some(data.to_string());
                        }
                    }
                }
            }
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        self.wrap_pending = false;
        match action {
            'H' | 'f' => {
                let mut it = params.iter();
                let y_param = it.next().map(|p| p[0] as usize).unwrap_or(1);
                let x_param = it.next().map(|p| p[0] as usize).unwrap_or(1);
                self.cursor_y = if y_param > 0 { y_param - 1 } else { 0 }.min(self.rows.saturating_sub(1));
                self.cursor_x = if x_param > 0 { x_param - 1 } else { 0 }.min(self.cols.saturating_sub(1));
            }
            'A' => {
                let mut n = 1;
                if let Some(param) = params.iter().next() { if param[0] > 0 { n = param[0] as usize; } }
                self.cursor_y = self.cursor_y.saturating_sub(n);
            }
            'B' | 'e' => {
                let mut n = 1;
                if let Some(param) = params.iter().next() { if param[0] > 0 { n = param[0] as usize; } }
                self.cursor_y = (self.cursor_y + n).min(self.rows.saturating_sub(1));
            }
            'C' | 'a' => {
                let mut n = 1;
                if let Some(param) = params.iter().next() { if param[0] > 0 { n = param[0] as usize; } }
                self.cursor_x = (self.cursor_x + n).min(self.cols.saturating_sub(1));
            }
            'D' => {
                let mut n = 1;
                if let Some(param) = params.iter().next() { if param[0] > 0 { n = param[0] as usize; } }
                self.cursor_x = self.cursor_x.saturating_sub(n);
            }
            'G' | '`' => {
                let mut n = 1;
                if let Some(param) = params.iter().next() { if param[0] > 0 { n = param[0] as usize; } }
                self.cursor_x = (n.saturating_sub(1)).min(self.cols.saturating_sub(1));
            }
            'd' => {
                let mut n = 1;
                if let Some(param) = params.iter().next() { if param[0] > 0 { n = param[0] as usize; } }
                self.cursor_y = (n.saturating_sub(1)).min(self.rows.saturating_sub(1));
            }
            'P' => {
                let mut n = 1;
                if let Some(param) = params.iter().next() { if param[0] > 0 { n = param[0] as usize; } }
                if self.cursor_y < self.rows && self.cursor_x < self.cols {
                    for _ in 0..n {
                        self.grid[self.cursor_y].remove(self.cursor_x);
                        self.grid[self.cursor_y].push(Cell { c: ' ', fg: None, bg: self.current_bg , attrs: TextAttrs::default() });
                    }
                }
            }
            '@' => {
                let mut n = 1;
                if let Some(param) = params.iter().next() { if param[0] > 0 { n = param[0] as usize; } }
                if self.cursor_y < self.rows && self.cursor_x < self.cols {
                    for _ in 0..n {
                        self.grid[self.cursor_y].insert(self.cursor_x, Cell { c: ' ', fg: None, bg: self.current_bg , attrs: TextAttrs::default() });
                        self.grid[self.cursor_y].pop();
                    }
                }
            }
            'r' => {
                let mut it = params.iter();
                let top = it.next().map(|p| p[0] as usize).unwrap_or(1);
                let bottom = it.next().map(|p| p[0] as usize).unwrap_or(self.rows);
                self.scroll_top = if top > 0 { top - 1 } else { 0 }.min(self.rows.saturating_sub(1));
                self.scroll_bottom = if bottom > 0 { bottom - 1 } else { self.rows.saturating_sub(1) }.min(self.rows.saturating_sub(1));
                if self.scroll_top > self.scroll_bottom {
                    self.scroll_top = 0;
                    self.scroll_bottom = self.rows.saturating_sub(1);
                }
                self.cursor_x = 0;
                self.cursor_y = 0;
            }
            'L' => {
                let mut n = 1;
                if let Some(param) = params.iter().next() { if param[0] > 0 { n = param[0] as usize; } }
                if self.cursor_y >= self.scroll_top && self.cursor_y <= self.scroll_bottom {
                    let n = n.min(self.scroll_bottom - self.cursor_y + 1);
                    self.grid[self.cursor_y..=self.scroll_bottom].rotate_right(n);
                    for i in 0..n {
                        self.grid[self.cursor_y + i] = vec![Cell { c: ' ', fg: None, bg: self.current_bg , attrs: TextAttrs::default() }; self.cols];
                    }
                }
            }
            'M' => {
                let mut n = 1;
                if let Some(param) = params.iter().next() { if param[0] > 0 { n = param[0] as usize; } }
                if self.cursor_y >= self.scroll_top && self.cursor_y <= self.scroll_bottom {
                    let n = n.min(self.scroll_bottom - self.cursor_y + 1);
                    self.grid[self.cursor_y..=self.scroll_bottom].rotate_left(n);
                    for i in 0..n {
                        self.grid[self.scroll_bottom - i] = vec![Cell { c: ' ', fg: None, bg: self.current_bg , attrs: TextAttrs::default() }; self.cols];
                    }
                }
            }
            'h' => {
                let is_dec = intermediates == b"?";
                for param_group in params.iter() {
                    for &code in param_group {
                        if is_dec {
                            match code {
                                7 => self.auto_wrap = true,
                                25 => self.cursor_visible = true,
                                1000 | 1002 | 1006 => self.mouse_mode = code,
                                2004 => self.bracketed_paste = true,
                                1049 => {
                                    // Enter alternate screen — save cursor & swap grid
                                    self.saved_cursor_x = self.cursor_x;
                                    self.saved_cursor_y = self.cursor_y;
                                    let alt = vec![vec![Cell { c: ' ', fg: None, bg: None, attrs: TextAttrs::default() }; self.cols]; self.rows];
                                    let primary = std::mem::replace(&mut self.grid, alt);
                                    self.alt_grid = Some(primary);
                                    self.cursor_x = 0;
                                    self.cursor_y = 0;
                                    self.scroll_top = 0;
                                    self.scroll_bottom = self.rows.saturating_sub(1);
                                }
                                _ => {}
                            }
                        } else {
                            match code {
                                4 => self.insert_mode = true,
                                7 => self.auto_wrap = true,
                                _ => {}
                            }
                        }
                    }
                }
            }
            'l' => {
                let is_dec = intermediates == b"?";
                for param_group in params.iter() {
                    for &code in param_group {
                        if is_dec {
                            match code {
                                7 => self.auto_wrap = false,
                                25 => self.cursor_visible = false,
                                1000 | 1002 | 1006 => self.mouse_mode = 0,
                                2004 => self.bracketed_paste = false,
                                1049 => {
                                    // Exit alternate screen — restore primary grid & cursor
                                    if let Some(primary) = self.alt_grid.take() {
                                        self.grid = primary;
                                    }
                                    self.cursor_x = self.saved_cursor_x;
                                    self.cursor_y = self.saved_cursor_y;
                                    self.scroll_top = 0;
                                    self.scroll_bottom = self.rows.saturating_sub(1);
                                }
                                _ => {}
                            }
                        } else {
                            match code {
                                4 => self.insert_mode = false,
                                7 => self.auto_wrap = false,
                                _ => {}
                            }
                        }
                    }
                }
            }
            's' => {
                // Save cursor (ANSI variant)
                self.saved_cursor_x = self.cursor_x;
                self.saved_cursor_y = self.cursor_y;
                self.saved_fg = self.current_fg;
                self.saved_bg = self.current_bg;
                self.saved_attrs = self.current_attrs;
            }
            'u' => {
                // Restore cursor (ANSI variant)
                self.cursor_x = self.saved_cursor_x.min(self.cols.saturating_sub(1));
                self.cursor_y = self.saved_cursor_y.min(self.rows.saturating_sub(1));
                self.current_fg = self.saved_fg;
                self.current_bg = self.saved_bg;
                self.current_attrs = self.saved_attrs;
            }
            'J' => {
                let mode = params.iter().next().map(|p| p[0]).unwrap_or(0);
                match mode {
                    0 => {
                        for x in self.cursor_x..self.cols {
                            if self.cursor_y < self.rows { self.grid[self.cursor_y][x] = Cell { c: ' ', fg: None, bg: self.current_bg , attrs: TextAttrs::default() }; }
                        }
                        for y in (self.cursor_y + 1)..self.rows {
                            if y < self.rows { self.grid[y] = vec![Cell { c: ' ', fg: None, bg: self.current_bg , attrs: TextAttrs::default() }; self.cols]; }
                        }
                    }
                    1 => {
                        for y in 0..self.cursor_y {
                            if y < self.rows { self.grid[y] = vec![Cell { c: ' ', fg: None, bg: self.current_bg , attrs: TextAttrs::default() }; self.cols]; }
                        }
                        for x in 0..=self.cursor_x {
                            if self.cursor_y < self.rows { self.grid[self.cursor_y][x] = Cell { c: ' ', fg: None, bg: self.current_bg , attrs: TextAttrs::default() }; }
                        }
                    }
                    2 | 3 => {
                        self.grid = vec![vec![Cell { c: ' ', fg: None, bg: self.current_bg , attrs: TextAttrs::default() }; self.cols]; self.rows];
                    }
                    _ => {}
                }
            }
            'K' => {
                let mode = params.iter().next().map(|p| p[0]).unwrap_or(0);
                match mode {
                    0 => {
                        for x in self.cursor_x..self.cols {
                            if self.cursor_y < self.rows { self.grid[self.cursor_y][x] = Cell { c: ' ', fg: None, bg: self.current_bg , attrs: TextAttrs::default() }; }
                        }
                    }
                    1 => {
                        for x in 0..=self.cursor_x {
                            if self.cursor_y < self.rows { self.grid[self.cursor_y][x] = Cell { c: ' ', fg: None, bg: self.current_bg , attrs: TextAttrs::default() }; }
                        }
                    }
                    2 => {
                        if self.cursor_y < self.rows { self.grid[self.cursor_y] = vec![Cell { c: ' ', fg: None, bg: self.current_bg , attrs: TextAttrs::default() }; self.cols]; }
                    }
                    _ => {}
                }
            }
            'm' => {
                let mut all_params: Vec<u16> = Vec::new();
                if params.is_empty() {
                    all_params.push(0);
                } else {
                    for param_group in params.iter() {
                        all_params.extend_from_slice(param_group);
                    }
                }
                let mut i = 0;
                while i < all_params.len() {
                    let code = all_params[i];
                    match code {
                        0 => {
                            self.current_fg = None;
                            self.current_bg = None;
                            self.current_attrs = TextAttrs::default();
                        }
                        1 => self.current_attrs.bold = true,
                        2 => self.current_attrs.dim = true,
                        3 => self.current_attrs.italic = true,
                        4 => self.current_attrs.underline = true,
                        5 | 6 => self.current_attrs.blink = true,
                        7 => self.current_attrs.reverse = true,
                        9 => self.current_attrs.strikethrough = true,
                        22 => { self.current_attrs.bold = false; self.current_attrs.dim = false; }
                        23 => self.current_attrs.italic = false,
                        24 => self.current_attrs.underline = false,
                        25 => self.current_attrs.blink = false,
                        27 => self.current_attrs.reverse = false,
                        29 => self.current_attrs.strikethrough = false,
                        30..=37 => self.current_fg = Some(Color::Named((code - 30) as u8)),
                        40..=47 => self.current_bg = Some(Color::Named((code - 40) as u8)),
                        90..=97 => self.current_fg = Some(Color::Named((code - 90 + 8) as u8)),
                        100..=107 => self.current_bg = Some(Color::Named((code - 100 + 8) as u8)),
                        39 => self.current_fg = None,
                        49 => self.current_bg = None,
                        38 => {
                            if i + 2 < all_params.len() && all_params[i + 1] == 5 {
                                self.current_fg = Some(Color::Named(all_params[i + 2] as u8));
                                i += 2;
                            } else if i + 4 < all_params.len() && all_params[i + 1] == 2 {
                                self.current_fg = Some(Color::TrueColor(
                                    all_params[i + 2] as u8,
                                    all_params[i + 3] as u8,
                                    all_params[i + 4] as u8,
                                ));
                                i += 4;
                            }
                        }
                        48 => {
                            if i + 2 < all_params.len() && all_params[i + 1] == 5 {
                                self.current_bg = Some(Color::Named(all_params[i + 2] as u8));
                                i += 2;
                            } else if i + 4 < all_params.len() && all_params[i + 1] == 2 {
                                self.current_bg = Some(Color::TrueColor(
                                    all_params[i + 2] as u8,
                                    all_params[i + 3] as u8,
                                    all_params[i + 4] as u8,
                                ));
                                i += 4;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
            }
            'n' => {
                // Device Status Report
                let mode = params.iter().next().map(|p| p[0]).unwrap_or(0);
                match mode {
                    5 => self.pending_responses.push(b"\x1b[0n".to_vec()),
                    6 => {
                        let row = self.cursor_y + 1;
                        let col = self.cursor_x + 1;
                        self.pending_responses.push(
                            format!("\x1b[{};{}R", row, col).into_bytes()
                        );
                    }
                    _ => {}
                }
            }
            'c' => {
                if intermediates == b">" {
                    // Secondary DA — identify as xterm version 136
                    self.pending_responses.push(b"\x1b[>0;136;0c".to_vec());
                } else {
                    // Primary Device Attributes — identify as a VT220
                    let p = params.iter().next().map(|p| p[0]).unwrap_or(0);
                    if p == 0 {
                        self.pending_responses.push(b"\x1b[?62;1;22c".to_vec());
                    }
                }
            }
            't' => {
                // XTWINOPS — window manipulation
                let mode = params.iter().next().map(|p| p[0]).unwrap_or(0);
                match mode {
                    18 => {
                        // Report terminal size in characters
                        let resp = format!("\x1b[8;{};{}t", self.rows, self.cols);
                        self.pending_responses.push(resp.into_bytes());
                    }
                    14 => {
                        // Report terminal size in pixels (estimate)
                        let pw = self.cols * 8; // approximate
                        let ph = self.rows * 16;
                        let resp = format!("\x1b[4;{};{}t", ph, pw);
                        self.pending_responses.push(resp.into_bytes());
                    }
                    _ => {}
                }
            }
            'p' => {
                if intermediates == b"!" {
                    // DECSTR — Soft Terminal Reset
                    self.current_fg = None;
                    self.current_bg = None;
                    self.current_attrs = TextAttrs::default();
                    self.cursor_x = 0;
                    self.cursor_y = 0;
                    self.scroll_top = 0;
                    self.scroll_bottom = self.rows.saturating_sub(1);
                    self.auto_wrap = true;
                    self.wrap_pending = false;
                    self.insert_mode = false;
                    self.charset_g0 = b'B';
                    self.cursor_visible = true;
                    self.mouse_mode = 0;
                    self.bracketed_paste = false;
                    // Reset tab stops to default every-8
                    for (i, ts) in self.tab_stops.iter_mut().enumerate() {
                        *ts = i % 8 == 0;
                    }
                }
            }
            'X' => {
                // Erase Character — blank n chars at cursor, don't move cursor
                let mut n = 1;
                if let Some(param) = params.iter().next() { if param[0] > 0 { n = param[0] as usize; } }
                for i in 0..n {
                    let x = self.cursor_x + i;
                    if x < self.cols && self.cursor_y < self.rows {
                        self.grid[self.cursor_y][x] = Cell { c: ' ', fg: None, bg: self.current_bg, attrs: TextAttrs::default() };
                    }
                }
            }
            'S' => {
                // Scroll Up — scroll the scroll region up by n lines
                let mut n = 1;
                if let Some(param) = params.iter().next() { if param[0] > 0 { n = param[0] as usize; } }
                if self.scroll_top < self.scroll_bottom && self.scroll_bottom < self.rows {
                    let n = n.min(self.scroll_bottom - self.scroll_top + 1);
                    self.grid[self.scroll_top..=self.scroll_bottom].rotate_left(n);
                    for i in 0..n {
                        self.grid[self.scroll_bottom - i] = vec![Cell { c: ' ', fg: None, bg: self.current_bg, attrs: TextAttrs::default() }; self.cols];
                    }
                }
            }
            'T' => {
                // Scroll Down — scroll the scroll region down by n lines
                let mut n = 1;
                if let Some(param) = params.iter().next() { if param[0] > 0 { n = param[0] as usize; } }
                if self.scroll_top < self.scroll_bottom && self.scroll_bottom < self.rows {
                    let n = n.min(self.scroll_bottom - self.scroll_top + 1);
                    self.grid[self.scroll_top..=self.scroll_bottom].rotate_right(n);
                    for i in 0..n {
                        self.grid[self.scroll_top + i] = vec![Cell { c: ' ', fg: None, bg: self.current_bg, attrs: TextAttrs::default() }; self.cols];
                    }
                }
            }
            'I' => {
                // Cursor Forward Tabulation — move cursor to the nth next tab stop
                let mut n = 1;
                if let Some(param) = params.iter().next() { if param[0] > 0 { n = param[0] as usize; } }
                for _ in 0..n {
                    let start = self.cursor_x + 1;
                    let mut found = false;
                    for i in start..self.cols {
                        if i < self.tab_stops.len() && self.tab_stops[i] {
                            self.cursor_x = i;
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        self.cursor_x = self.cols.saturating_sub(1);
                        break;
                    }
                }
            }
            'Z' => {
                // Cursor Backward Tabulation — move cursor to the nth previous tab stop
                let mut n = 1;
                if let Some(param) = params.iter().next() { if param[0] > 0 { n = param[0] as usize; } }
                for _ in 0..n {
                    if self.cursor_x == 0 { break; }
                    let mut found = false;
                    for i in (0..self.cursor_x).rev() {
                        if i < self.tab_stops.len() && self.tab_stops[i] {
                            self.cursor_x = i;
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        self.cursor_x = 0;
                        break;
                    }
                }
            }
            'g' => {
                // Tab Clear
                let mode = params.iter().next().map(|p| p[0]).unwrap_or(0);
                match mode {
                    0 => {
                        // Clear tab stop at current cursor position
                        if self.cursor_x < self.tab_stops.len() {
                            self.tab_stops[self.cursor_x] = false;
                        }
                    }
                    3 => {
                        // Clear all tab stops
                        for ts in self.tab_stops.iter_mut() { *ts = false; }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        self.wrap_pending = false;
        match byte {
            b'7' => {
                // DECSC — save cursor
                self.saved_cursor_x = self.cursor_x;
                self.saved_cursor_y = self.cursor_y;
                self.saved_fg = self.current_fg;
                self.saved_bg = self.current_bg;
                self.saved_attrs = self.current_attrs;
            }
            b'8' => {
                // DECRC — restore cursor
                self.cursor_x = self.saved_cursor_x.min(self.cols.saturating_sub(1));
                self.cursor_y = self.saved_cursor_y.min(self.rows.saturating_sub(1));
                self.current_fg = self.saved_fg;
                self.current_bg = self.saved_bg;
                self.current_attrs = self.saved_attrs;
            }
            b'M' => {
                if self.cursor_y == self.scroll_top {
                    if self.scroll_top < self.scroll_bottom && self.scroll_bottom < self.rows {
                        self.grid[self.scroll_top..=self.scroll_bottom].rotate_right(1);
                        self.grid[self.scroll_top] = vec![Cell { c: ' ', fg: None, bg: self.current_bg , attrs: TextAttrs::default() }; self.cols];
                    }
                } else if self.cursor_y > 0 {
                    self.cursor_y -= 1;
                }
            }
            b'D' => {
                if self.cursor_y == self.scroll_bottom {
                    self.scroll_up();
                } else if self.cursor_y < self.rows.saturating_sub(1) {
                    self.cursor_y += 1;
                }
            }
            b'E' => {
                self.cursor_x = 0;
                if self.cursor_y == self.scroll_bottom {
                    self.scroll_up();
                } else if self.cursor_y < self.rows.saturating_sub(1) {
                    self.cursor_y += 1;
                }
            }
            b'H' => {
                // HTS — set tab stop at current cursor column
                if self.cursor_x < self.tab_stops.len() {
                    self.tab_stops[self.cursor_x] = true;
                }
            }
            b'0' if _intermediates == b"(" => {
                // ESC ( 0 — switch to DEC Special Graphics (line drawing)
                self.charset_g0 = b'0';
            }
            b'B' if _intermediates == b"(" => {
                // ESC ( B — switch to ASCII charset
                self.charset_g0 = b'B';
            }
            _ => {}
        }
        self.dirty = true;
    }
}

pub struct PTYHandle {
    pub master: Box<dyn portable_pty::MasterPty + Send>,
}

pub fn spawn_pty(state: Arc<Mutex<TerminalState>>, initial_cols: u16, initial_rows: u16) -> Result<PTYHandle, anyhow::Error> {
    let pty_system = NativePtySystem::default();
    let pair = pty_system.openpty(PtySize {
        rows: initial_rows,
        cols: initial_cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut cmd = CommandBuilder::new("tmux");
    cmd.env("COLORTERM", "truecolor");
    cmd.args(["new-session", "-A", "-s", "t1000_main"]);
    let _child = pair.slave.spawn_command(cmd)?;
    
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    
    thread::spawn(move || {
        let mut parser = Parser::new();
        let mut buf = [0u8; 1024];

        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let mut state_lock = state.lock().unwrap();
                    parser.advance(&mut *state_lock, &buf[..n]);
                }
                Err(_) => break,
            }
        }
    });

    Ok(PTYHandle { master: pair.master })
}
