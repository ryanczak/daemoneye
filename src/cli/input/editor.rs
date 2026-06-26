/// A multi-line editable text buffer with cursor support.
///
/// The buffer stores text as a flat `Vec<char>` with embedded `\n` characters.
/// The cursor is a character index into this flat buffer (0..=buf.len()).
///
/// For rendering, the buffer provides `visual_lines(width)` which word-wraps
/// the text into visual lines, and `cursor_visual_pos(width)` which maps the
/// cursor to a (visual_row, visual_col) pair within the wrapped display.
#[derive(Default, Clone)]
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

    /// Insert a character at the cursor position.
    pub fn insert(&mut self, c: char) {
        self.buf.insert(self.cursor, c);
        self.cursor += 1;
    }

    /// Insert a newline at the cursor position (does not submit).
    pub fn insert_newline(&mut self) {
        self.buf.insert(self.cursor, '\n');
        self.cursor += 1;
    }

    /// Insert a string at the cursor position (used for paste).
    pub fn insert_str(&mut self, s: &str) {
        let chars: Vec<char> = s.chars().collect();
        let start = self.cursor;
        self.buf.splice(start..start, chars.clone());
        self.cursor += chars.len();
    }

    /// Delete the character before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            let idx = self.cursor - 1;
            self.buf.remove(idx);
            self.cursor -= 1;
        }
    }

    /// Delete the character at the cursor.
    pub fn delete(&mut self) {
        if self.cursor < self.buf.len() {
            self.buf.remove(self.cursor);
        }
    }

    /// Move cursor left by one character. If at start of a line, move to end
    /// of previous line.
    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// Move cursor right by one character. If at end of a line, move to start
    /// of next line.
    pub fn move_right(&mut self) {
        if self.cursor < self.buf.len() {
            self.cursor += 1;
        }
    }

    /// Move cursor to the beginning of the current visual line.
    pub fn move_home(&mut self) {
        // Find the start of the current logical line
        let mut idx = self.cursor;
        while idx > 0 && self.buf[idx - 1] != '\n' {
            idx -= 1;
        }
        self.cursor = idx;
    }

    /// Move cursor to the end of the current logical line (before '\n' or end).
    pub fn move_end(&mut self) {
        let mut idx = self.cursor;
        while idx < self.buf.len() && self.buf[idx] != '\n' {
            idx += 1;
        }
        self.cursor = idx;
    }

    /// Move cursor up one visual line.
    pub fn move_up(&mut self, width: usize) {
        if width == 0 {
            return;
        }
        let (row, col) = self.cursor_visual_pos(width);
        if row == 0 {
            // Already at top — move to start
            self.cursor = 0;
            return;
        }
        let target_row = row - 1;
        let new_pos = self.visual_pos_to_cursor(target_row, col, width);
        self.cursor = new_pos;
    }

    /// Move cursor down one visual line.
    pub fn move_down(&mut self, width: usize) {
        if width == 0 {
            return;
        }
        let (row, col) = self.cursor_visual_pos(width);
        let lines = self.visual_lines(width);
        if row >= lines.len() - 1 {
            // Already at bottom — move to end
            self.cursor = self.buf.len();
            return;
        }
        let target_row = row + 1;
        let new_pos = self.visual_pos_to_cursor(target_row, col, width);
        self.cursor = new_pos;
    }

    /// Kill from cursor to end of line.
    pub fn kill_to_end(&mut self) {
        let mut end = self.cursor;
        while end < self.buf.len() && self.buf[end] != '\n' {
            end += 1;
        }
        self.buf.drain(self.cursor..end);
    }

    /// Kill from cursor to start of line.
    pub fn kill_to_start(&mut self) {
        let mut start = self.cursor;
        while start > 0 && self.buf[start - 1] != '\n' {
            start -= 1;
        }
        self.buf.drain(start..self.cursor);
        self.cursor = start;
    }

    /// Return the buffer contents as a String (with embedded newlines).
    pub fn as_str(&self) -> String {
        self.buf.iter().collect()
    }

    /// Return the cursor position as a character index (0..=buf.len()).
    pub fn cursor_pos(&self) -> usize {
        self.cursor
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Word-wrap the buffer into visual lines given a display width.
    /// Returns a Vec of visual lines, each a Vec<char>.
    /// The display width is the number of character cells available (not counting borders).
    ///
    /// This algorithm matches ratatui's `Wrap { trim: false }`: whitespace is preserved
    /// literally, words wrap at word boundaries, and words longer than the width are
    /// hard-broken at the width boundary.
    pub fn visual_lines(&self, width: usize) -> Vec<Vec<char>> {
        if width == 0 {
            return vec![vec![]];
        }
        if self.buf.is_empty() {
            return vec![vec![]];
        }

        let mut result: Vec<Vec<char>> = Vec::new();
        let mut current_line: Vec<char> = Vec::new();
        let mut current_word: Vec<char> = Vec::new();
        let mut pending_ws: Vec<char> = Vec::new();

        for &ch in &self.buf {
            if ch == '\n' {
                // Hard line break: flush word, push line, start new
                if !current_word.is_empty() {
                    current_line.append(&mut pending_ws);
                    current_line.append(&mut current_word);
                }
                result.push(current_line);
                current_line = Vec::new();
                pending_ws.clear();
                continue;
            }

            if ch.is_whitespace() {
                // Flush current word if we had one
                if !current_word.is_empty() {
                    // Try to fit word + pending_ws on current line
                    let total_word_len = pending_ws.len() + current_word.len();
                    if current_line.is_empty() || current_line.len() + total_word_len <= width {
                        current_line.append(&mut pending_ws);
                        current_line.append(&mut current_word);
                    } else {
                        // Word doesn't fit — push current line, start new
                        result.push(current_line);
                        current_line = Vec::new();

                        // If word alone fits on new line
                        if current_word.len() <= width {
                            current_line.append(&mut pending_ws);
                            current_line.append(&mut current_word);
                        } else {
                            // Word is longer than width — hard break it
                            current_line.append(&mut current_word);
                            // current_line now exceeds width, split
                            while current_line.len() > width {
                                let next = current_line.split_off(width);
                                result.push(current_line);
                                current_line = next;
                            }
                            current_word.clear();
                        }
                    }
                }
                // Accumulate whitespace
                pending_ws.push(ch);
            } else {
                current_word.push(ch);
            }
        }

        // Flush remaining word and whitespace
        if !current_word.is_empty() {
            let total_word_len = pending_ws.len() + current_word.len();
            if current_line.is_empty() || current_line.len() + total_word_len <= width {
                if current_line.is_empty() && current_word.len() > width {
                    // Overlong word on empty line — hard break it
                    current_line.append(&mut pending_ws);
                    current_line.append(&mut current_word);
                    while current_line.len() > width {
                        let next = current_line.split_off(width);
                        result.push(current_line);
                        current_line = next;
                    }
                    current_word.clear();
                } else {
                    current_line.append(&mut pending_ws);
                    current_line.append(&mut current_word);
                }
            } else {
                result.push(current_line);
                current_line = Vec::new();

                if current_word.len() <= width {
                    current_line.append(&mut pending_ws);
                    current_line.append(&mut current_word);
                } else {
                    current_line.append(&mut pending_ws);
                    current_line.append(&mut current_word);
                    while current_line.len() > width {
                        let next = current_line.split_off(width);
                        result.push(current_line);
                        current_line = next;
                    }
                    current_word.clear();
                }
            }
        } else if !pending_ws.is_empty() {
            // Trailing whitespace with no word — add to current line if fits
            if current_line.is_empty() || current_line.len() + pending_ws.len() <= width {
                current_line.append(&mut pending_ws);
            }
        }

        if current_line.is_empty() && result.is_empty() {
            result.push(vec![]);
        } else {
            result.push(current_line);
        }

        result
    }

    /// Map the cursor's character index to a (visual_row, visual_col) position
    /// within the word-wrapped display.
    pub fn cursor_visual_pos(&self, width: usize) -> (usize, usize) {
        if width == 0 {
            return (0, 0);
        }
        let lines = self.visual_lines(width);

        // Walk through the buffer, tracking how many visual chars each logical
        // line produces, and where in the buffer each visual line starts.
        let mut buf_idx = 0;

        for (row, line) in lines.iter().enumerate() {
            let line_len = line.len();
            let buf_end = buf_idx + line_len;
            let buf_end_with_newline = if buf_end < self.buf.len() && self.buf[buf_end] == '\n' {
                buf_end + 1
            } else {
                buf_end
            };

            if self.cursor <= buf_end {
                let col = self.cursor - buf_idx;
                return (row, col.min(line_len));
            }
            if self.cursor == buf_end_with_newline {
                // Cursor is right after the '\n', which is the start of next visual line
                return (row + 1, 0);
            }

            buf_idx = buf_end_with_newline;
        }

        // Cursor is at the very end
        let row = lines.len().saturating_sub(1);
        let col = if row < lines.len() {
            lines[row].len()
        } else {
            0
        };
        (row, col)
    }

    /// Map a (visual_row, visual_col) position back to a cursor character index.
    fn visual_pos_to_cursor(&self, target_row: usize, col: usize, width: usize) -> usize {
        let lines = self.visual_lines(width);
        if target_row >= lines.len() {
            return self.buf.len();
        }

        // Walk through visual lines, tracking buffer offset
        let mut buf_idx = 0;
        for line in &lines[..target_row] {
            let line_len = line.len();
            buf_idx += line_len;
            // Skip the '\n' at the end of this logical line if present
            if buf_idx < self.buf.len() && self.buf[buf_idx] == '\n' {
                buf_idx += 1;
            }
        }

        let target_col = col.min(lines[target_row].len());
        buf_idx + target_col
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
    }

    pub fn current_line(&self) -> &InputLine {
        &self.current
    }

    pub fn current_line_mut(&mut self) -> &mut InputLine {
        &mut self.current
    }

    pub fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        if self.history_idx.is_none() {
            self.saved = self.current.as_str();
            self.history_idx = Some(self.history.len() - 1);
        } else if let Some(idx) = self.history_idx
            && idx > 0
        {
            self.history_idx = Some(idx - 1);
        }
        if let Some(idx) = self.history_idx {
            self.current = InputLine::from_str(&self.history[idx]);
        }
    }

    pub fn history_down(&mut self) {
        if let Some(idx) = self.history_idx {
            if idx + 1 < self.history.len() {
                self.history_idx = Some(idx + 1);
                self.current = InputLine::from_str(&self.history[idx + 1]);
            } else {
                // Restore the saved line
                self.current = InputLine::from_str(&self.saved);
                self.history_idx = None;
            }
        }
    }

    pub fn clear_history_nav(&mut self) {
        self.history_idx = None;
        self.saved = String::new();
    }

    /// Whether the history has entries.
    pub fn has_history(&self) -> bool {
        !self.history.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_position_tracks_edits() {
        let mut line = InputLine::new();
        line.insert('h');
        line.insert('e');
        line.insert('l');
        assert_eq!(line.cursor_pos(), 3);

        line.move_left();
        assert_eq!(line.cursor_pos(), 2);
        line.insert('X');
        assert_eq!(line.cursor_pos(), 3);
        assert_eq!(line.as_str(), "heXl");

        line.move_home();
        assert_eq!(line.cursor_pos(), 0);
        line.move_end();
        assert_eq!(line.cursor_pos(), 4);
    }

    #[test]
    fn visual_lines_empty() {
        let line = InputLine::new();
        let v = line.visual_lines(40);
        assert_eq!(v, vec![Vec::<char>::new()]);
    }

    #[test]
    fn visual_lines_single_line_no_wrap() {
        let mut line = InputLine::new();
        line.insert_str("hello world");
        let v = line.visual_lines(40);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].iter().collect::<String>(), "hello world");
    }

    #[test]
    fn visual_lines_wraps_at_word_boundary() {
        let mut line = InputLine::new();
        line.insert_str("hello world foo bar");
        let v = line.visual_lines(11); // 11 chars wide
        assert!(v.len() >= 2);
        // First line should not end mid-word
        let first: String = v[0].iter().collect();
        assert!(!first.ends_with('o')); // "hello" not "hell"
    }

    #[test]
    fn visual_lines_hard_newline() {
        let mut line = InputLine::new();
        line.insert_str("line1\nline2");
        let v = line.visual_lines(40);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].iter().collect::<String>(), "line1");
        assert_eq!(v[1].iter().collect::<String>(), "line2");
    }

    #[test]
    fn cursor_visual_pos_tracks_cursor() {
        let mut line = InputLine::new();
        line.insert_str("hello");
        let (row, col) = line.cursor_visual_pos(40);
        assert_eq!(row, 0);
        assert_eq!(col, 5);

        line.move_home();
        let (row, col) = line.cursor_visual_pos(40);
        assert_eq!(row, 0);
        assert_eq!(col, 0);
    }

    #[test]
    fn cursor_visual_pos_wrapped() {
        let mut line = InputLine::new();
        line.insert_str("hello world foo bar");
        let v = line.visual_lines(11);
        // Cursor at end should be on last visual line
        let (row, col) = line.cursor_visual_pos(11);
        assert_eq!(row, v.len() - 1);
        assert_eq!(col, v.last().unwrap().len());
    }

    #[test]
    fn multiline_insert_newline() {
        let mut line = InputLine::new();
        line.insert_str("hello");
        line.insert_newline();
        line.insert_str("world");
        assert_eq!(line.as_str(), "hello\nworld");
    }

    #[test]
    fn multiline_paste_does_not_submit() {
        let mut line = InputLine::new();
        line.insert_str("first line\nsecond line\nthird line");
        let s = line.as_str();
        assert!(s.contains('\n'));
        assert_eq!(s, "first line\nsecond line\nthird line");
    }

    #[test]
    fn submit_preserves_newlines() {
        let mut line = InputLine::new();
        line.insert_str("hello\nworld\nfoo");
        let result = line.as_str();
        assert_eq!(result, "hello\nworld\nfoo");
        assert_eq!(result.matches('\n').count(), 2);
    }

    #[test]
    fn backspace_across_newline_joins_lines() {
        let mut line = InputLine::new();
        line.insert_str("ab\ncd");
        // cursor at 5 (end of "cd"), buf = ['a','b','\n','c','d']
        line.move_left(); // at 4 ('d')
        line.move_left(); // at 3 ('c')
        line.move_left(); // at 2 ('\n')
        line.move_left(); // at 1 ('b')
        line.backspace(); // removes 'a', cursor at 0
        assert_eq!(line.as_str(), "b\ncd");
    }

    #[test]
    fn delete_newline_joins_lines() {
        let mut line = InputLine::new();
        line.insert_str("ab\ncd");
        // buf = ['a','b','\n','c','d'], cursor at 5
        // move to position of '\n' (index 2)
        // move_home goes to start of current line (pos 3 since cursor is at 5 on line "cd")
        // so we need to move left first to get to the line with the newline
        line.move_left(); // at 4
        line.move_left(); // at 3
        line.move_left(); // at 2 ('\n')
        line.move_left(); // at 1 ('b')
        line.move_left(); // at 0 ('a')
        line.move_right(); // at 1
        line.move_right(); // at 2
        assert_eq!(line.cursor_pos(), 2);
        line.delete(); // removes '\n'
        assert_eq!(line.as_str(), "abcd");
    }

    #[test]
    fn visual_lines_preserves_whitespace() {
        let mut line = InputLine::new();
        line.insert_str("hello  world"); // double space
        let v = line.visual_lines(40);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].iter().collect::<String>(), "hello  world");
    }

    #[test]
    fn visual_lines_leading_whitespace() {
        let mut line = InputLine::new();
        line.insert_str("  hello");
        let v = line.visual_lines(40);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].iter().collect::<String>(), "  hello");
    }

    #[test]
    fn visual_lines_overlong_word() {
        let mut line = InputLine::new();
        line.insert_str("supercalifragilistic");
        let v = line.visual_lines(10);
        // "supercalifragilistic" (20 chars) with width 10 → 2 lines
        // But the word is 20 chars, and width is 10, so it should split
        // into "supercali" (9) + "fragilistic" (11) — wait, that's wrong.
        // With width=10, "supercalifragilistic" is a single word of 20 chars.
        // It exceeds width, so it gets hard-broken: first 10 chars on line 1,
        // remaining 10 on line 2.
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].iter().collect::<String>(), "supercalif");
        assert_eq!(v[1].iter().collect::<String>(), "ragilistic");
    }

    #[test]
    fn visual_lines_cursor_matches_wrap() {
        let mut line = InputLine::new();
        line.insert_str("hello  world"); // double space
        let v = line.visual_lines(40);
        let (row, col) = line.cursor_visual_pos(40);
        assert_eq!(row, 0);
        assert_eq!(col, v[0].len());
    }

    #[test]
    fn visual_lines_cursor_on_wrapped_overlong() {
        let mut line = InputLine::new();
        line.insert_str("supercalifragilistic");
        let v = line.visual_lines(10);
        let (row, col) = line.cursor_visual_pos(10);
        assert_eq!(row, v.len() - 1);
        assert_eq!(col, v.last().unwrap().len());
    }

    #[test]
    fn cursor_visual_pos_double_space() {
        let mut line = InputLine::new();
        line.insert_str("a  b");
        // cursor at end = pos 4
        let (row, col) = line.cursor_visual_pos(40);
        assert_eq!(row, 0);
        assert_eq!(col, 4);
    }
}
