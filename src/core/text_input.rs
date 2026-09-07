//! Reusable single/multi-line text input state: an owned `String` plus a caret
//! (char index) and the editing operations over them. Shared by every small
//! text field in the app (find query/replace, workspace search, git commit).
//!
//! Pure data + logic, no IO — the rendering lives in `ui::text_input`.
//! Home/End/Up/Down are line-aware, so the same type serves single-line fields
//! (no '\n' → one line) and the multi-line commit box alike.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What a key did to a [`TextInputState`], so the caller can react (e.g. re-run a
/// live search only when the content actually changed).
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum InputOutcome {
    /// The key was not for this input; the caller should handle it.
    Ignored,
    /// The caret moved; content is unchanged.
    Moved,
    /// The content changed.
    Changed,
}

#[derive(Default)]
pub struct TextInputState {
    content: String,
    /// Caret position as a char index into `content`.
    cursor: usize,
}

impl TextInputState {
    /// Char length of the content.
    fn len(&self) -> usize {
        self.content.chars().count()
    }

    /// Byte offset of char index `ci` (clamped to the end).
    fn byte_of(&self, ci: usize) -> usize {
        self.content
            .char_indices()
            .nth(ci)
            .map(|(b, _)| b)
            .unwrap_or(self.content.len())
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    /// Replaces the content and moves the caret to the end.
    pub fn set_content(&mut self, s: impl Into<String>) {
        self.content = s.into();
        self.cursor = self.len();
    }

    /// Clears the content and the caret.
    pub fn clear(&mut self) {
        self.content.clear();
        self.cursor = 0;
    }

    /// Moves the caret to the end of the content.
    pub fn cursor_to_end(&mut self) {
        self.cursor = self.len();
    }

    /// Handles a key event, mutating the content/caret, and emits an [`InputOutcome`]
    /// so the caller knows whether to fall through (`Ignored`) or react to an edit.
    ///
    /// Unconsumed keys (`Ignored`) are left to the caller: Enter/Tab/Esc,
    /// Ctrl-shortcuts, and — for single-line inputs — Up/Down (match/result nav).
    /// `multiline` enables Enter (newline) and Up/Down (line motion).
    pub fn handle_key(&mut self, key: KeyEvent, multiline: bool) -> InputOutcome {
        use InputOutcome::{Changed, Ignored, Moved};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            // Ctrl+char / Alt+char are shortcuts (Save, panel switching, …) —
            // never text. Without the Alt case, Alt+2 would type "2" into this
            // input instead of falling through to the panel shortcut.
            KeyCode::Char(_) if ctrl || alt => Ignored,
            KeyCode::Char(c) => {
                self.insert_char(c);
                Changed
            }
            KeyCode::Backspace => {
                self.backspace();
                Changed
            }
            KeyCode::Delete => {
                self.delete_forward();
                Changed
            }
            KeyCode::Enter if multiline => {
                self.insert_char('\n');
                Changed
            }
            KeyCode::Left if ctrl => {
                self.word_left();
                Moved
            }
            KeyCode::Right if ctrl => {
                self.word_right();
                Moved
            }
            KeyCode::Left => {
                self.left();
                Moved
            }
            KeyCode::Right => {
                self.right();
                Moved
            }
            KeyCode::Home => {
                self.home();
                Moved
            }
            KeyCode::End => {
                self.end();
                Moved
            }
            KeyCode::Up if multiline => {
                self.up();
                Moved
            }
            KeyCode::Down if multiline => {
                self.down();
                Moved
            }
            _ => Ignored,
        }
    }

    /// Inserts a pasted block at the caret in one edit. `'\r'` is dropped;
    /// `'\n'` is kept only when `multiline` (the git commit box), else folded to
    /// a space so a multi-line paste still lands as one line instead of losing
    /// everything after the first newline.
    pub fn insert_paste(&mut self, text: &str, multiline: bool) {
        for c in text.chars() {
            match c {
                '\r' => continue,
                '\n' if !multiline => self.insert_char(' '),
                c => self.insert_char(c),
            }
        }
    }

    // ----- editing -----

    /// Inserts `c` at the caret.
    pub fn insert_char(&mut self, c: char) {
        let b = self.byte_of(self.cursor);
        self.content.insert(b, c);
        self.cursor += 1;
    }

    /// Deletes the char before the caret.
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_of(self.cursor - 1);
        let end = self.byte_of(self.cursor);
        self.content.replace_range(start..end, "");
        self.cursor -= 1;
    }

    /// Deletes the char at the caret (forward delete).
    pub fn delete_forward(&mut self) {
        if self.cursor >= self.len() {
            return;
        }
        let start = self.byte_of(self.cursor);
        let end = self.byte_of(self.cursor + 1);
        self.content.replace_range(start..end, "");
    }

    // ----- caret motion -----

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.len());
    }

    /// One word left: skips whitespace, then the word before the caret.
    pub fn word_left(&mut self) {
        let chars: Vec<char> = self.content.chars().collect();
        let mut i = self.cursor.min(chars.len());
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        self.cursor = i;
    }

    /// One word right: skips whitespace, then the word after the caret.
    pub fn word_right(&mut self) {
        let chars: Vec<char> = self.content.chars().collect();
        let n = chars.len();
        let mut i = self.cursor.min(n);
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        while i < n && !chars[i].is_whitespace() {
            i += 1;
        }
        self.cursor = i;
    }

    /// Caret to the start of its current line.
    pub fn home(&mut self) {
        self.cursor = self.line_start(self.cursor);
    }

    /// Caret to the end of its current line.
    pub fn end(&mut self) {
        self.cursor = self.line_end(self.cursor);
    }

    /// Up one line, keeping the column where possible.
    pub fn up(&mut self) {
        let ls = self.line_start(self.cursor);
        if ls == 0 {
            return; // already on the first line
        }
        let col = self.cursor - ls;
        let prev_end = ls - 1;
        let prev_start = self.line_start(prev_end);
        self.cursor = (prev_start + col).min(prev_end);
    }

    /// Down one line, keeping the column where possible.
    pub fn down(&mut self) {
        let cur_end = self.line_end(self.cursor);
        if cur_end >= self.len() {
            return; // already on the last line
        }
        let col = self.cursor - self.line_start(self.cursor);
        let next_start = cur_end + 1;
        let next_end = self.line_end(next_start);
        self.cursor = (next_start + col).min(next_end);
    }

    /// Char index of the start of the line containing `at`.
    fn line_start(&self, at: usize) -> usize {
        let chars: Vec<char> = self.content.chars().collect();
        let mut i = at.min(chars.len());
        while i > 0 && chars[i - 1] != '\n' {
            i -= 1;
        }
        i
    }

    /// Char index of the end of the line containing `at` (before the '\n').
    fn line_end(&self, at: usize) -> usize {
        let chars: Vec<char> = self.content.chars().collect();
        let mut i = at.min(chars.len());
        while i < chars.len() && chars[i] != '\n' {
            i += 1;
        }
        i
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(content: &str, cursor: usize) -> TextInputState {
        TextInputState {
            content: content.to_string(),
            cursor,
        }
    }

    #[test]
    fn insert_backspace_delete_at_cursor() {
        let mut s = at("ac", 1);
        s.insert_char('b');
        assert_eq!((s.content(), s.cursor()), ("abc", 2));
        s.backspace();
        assert_eq!((s.content(), s.cursor()), ("ac", 1));
        s.delete_forward();
        assert_eq!((s.content(), s.cursor()), ("a", 1));
        s.delete_forward(); // at end: no-op
        s.cursor = 0;
        s.backspace(); // at start: no-op
        assert_eq!(s.content(), "a");
    }

    #[test]
    fn paste_folds_newlines_on_single_line_input() {
        let mut s = TextInputState::default();
        s.insert_paste("a\r\nb\nc", false);
        assert_eq!(s.content(), "a b c", "CR dropped, LF folded to a space");
    }

    #[test]
    fn paste_keeps_newlines_on_multiline_input() {
        let mut s = TextInputState::default();
        s.insert_paste("a\r\nb", true);
        assert_eq!(s.content(), "a\nb", "CR dropped, LF kept");
    }

    #[test]
    fn horizontal_moves_clamp() {
        let mut s = at("ab", 0);
        s.left();
        assert_eq!(s.cursor(), 0);
        s.cursor = 2;
        s.right();
        assert_eq!(s.cursor(), 2);
    }

    #[test]
    fn word_moves() {
        let mut s = at("foo bar baz", 11);
        s.word_left();
        assert_eq!(s.cursor(), 8); // start of "baz"
        s.word_left();
        assert_eq!(s.cursor(), 4); // start of "bar"
        s.cursor = 0;
        s.word_right();
        assert_eq!(s.cursor(), 3); // end of "foo"
    }

    #[test]
    fn line_bounds_and_vertical_moves() {
        // lines: "ab"(0..2), "cdef"(3..7), "g"(8..9)
        let mut s = at("ab\ncdef\ng", 5);
        s.home();
        assert_eq!(s.cursor(), 3);
        s.cursor = 5;
        s.end();
        assert_eq!(s.cursor(), 7);
        // Up keeps the column, clamped to the shorter previous line.
        s.cursor = 6; // col 3 on "cdef"
        s.up();
        assert_eq!(s.cursor(), 2); // clamp to end of "ab"
        // Down keeps the column, clamped to the shorter next line.
        s.cursor = 5; // col 2 on "cdef"
        s.down();
        assert_eq!(s.cursor(), 9); // clamp to end of "g"
        // Edges are no-ops.
        s.cursor = 1;
        s.up();
        assert_eq!(s.cursor(), 1);
        s.cursor = 8;
        s.down();
        assert_eq!(s.cursor(), 8);
    }
}
