/// A single editable line: a character buffer and a cursor position.
#[derive(Default)]
pub struct InputLine {
    buf: Vec<char>,
    cursor: usize, // character index, 0 ..= buf.len()
}

impl InputLine {
    pub fn new() -> Self {
        Self::default()
    }

    fn from_str(s: &str) -> Self {
        let buf: Vec<char> = s.chars().collect();
        let cursor = buf.len();
        Self { buf, cursor }
    }

    pub fn insert(&mut self, c: char) {
        self.buf.insert(self.cursor, c);
        self.cursor += 1;
    }
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.buf.remove(self.cursor - 1);
            self.cursor -= 1;
        }
    }
    pub fn delete(&mut self) {
        if self.cursor < self.buf.len() {
            self.buf.remove(self.cursor);
        }
    }
    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }
    pub fn move_right(&mut self) {
        if self.cursor < self.buf.len() {
            self.cursor += 1;
        }
    }
    pub fn move_home(&mut self) {
        self.cursor = 0;
    }
    pub fn move_end(&mut self) {
        self.cursor = self.buf.len();
    }
    pub fn kill_to_end(&mut self) {
        self.buf.truncate(self.cursor);
    }
    pub fn kill_to_start(&mut self) {
        self.buf.drain(..self.cursor);
        self.cursor = 0;
    }
    fn as_string(&self) -> String {
        self.buf.iter().collect()
    }
    pub fn as_str(&self) -> String {
        self.buf.iter().collect()
    }
}

/// Session-wide input state: the current line plus the navigable history.
pub struct InputState {
    current: InputLine,
    history: Vec<String>,
    history_idx: Option<usize>,
    saved: String, // current line stashed while browsing history
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            current: InputLine::new(),
            history: Vec::new(),
            history_idx: None,
            saved: String::new(),
        }
    }
}

impl InputState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Commit a query to history and reset the current line to empty.
    pub fn push_history(&mut self, s: String) {
        if !s.is_empty() && self.history.last().map(|l| l.as_str()) != Some(&s) {
            self.history.push(s);
        }
        self.history_idx = None;
        self.saved = String::new();
        self.current = InputLine::new();
    }

    /// Access the current editable line.
    pub fn current_line(&self) -> &InputLine {
        &self.current
    }

    /// Mutable access to the current editable line.
    pub fn current_line_mut(&mut self) -> &mut InputLine {
        &mut self.current
    }

    pub fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let new_idx = match self.history_idx {
            None => {
                self.saved = self.current.as_string();
                self.history.len() - 1
            }
            Some(0) => return,
            Some(i) => i - 1,
        };
        self.history_idx = Some(new_idx);
        self.current = InputLine::from_str(&self.history[new_idx].clone());
    }

    pub fn history_down(&mut self) {
        match self.history_idx {
            None => {}
            Some(i) if i + 1 >= self.history.len() => {
                self.history_idx = None;
                let s = self.saved.clone();
                self.current = InputLine::from_str(&s);
            }
            Some(i) => {
                let new_idx = i + 1;
                self.history_idx = Some(new_idx);
                self.current = InputLine::from_str(&self.history[new_idx].clone());
            }
        }
    }

    /// Clear the history navigation state (used on Ctrl+C to reset).
    pub fn clear_history_nav(&mut self) {
        self.history_idx = None;
    }
}
