//! Text editing model for the composer: a real input box.
//!
//! Pure and UI-free so every keybinding can be tested as a state transition.
//! Word motion deliberately mirrors the TUI's
//! `state_ui_input_helpers::find_word_boundary_{back,forward}` so muscle
//! memory transfers exactly: back skips whitespace then word characters,
//! forward skips the current word then trailing whitespace. Byte offsets are
//! always kept on `char` boundaries so multi-byte input can never panic.

/// A single-line text buffer with a cursor, undo, and a blinking caret.
#[derive(Clone, Debug, Default)]
pub struct Editor {
    text: String,
    /// Cursor as a byte offset, always on a char boundary.
    cursor: usize,
    /// Undo stack of (text, cursor) snapshots.
    undo: Vec<(String, usize)>,
    /// Submitted history, oldest first.
    history: Vec<String>,
    /// Position while recalling history; `None` means editing live input.
    history_index: Option<usize>,
    /// Live input parked while browsing history, restored on the way back.
    stashed: Option<String>,
}

/// Maximum undo depth. Deep enough for real editing, bounded so a long
/// session cannot grow without limit.
const UNDO_LIMIT: usize = 64;

impl Editor {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Text before the cursor, used to place the caret when rendering.
    pub fn before_cursor(&self) -> &str {
        &self.text[..self.cursor]
    }

    #[cfg(test)]
    pub fn with_text(text: &str) -> Self {
        let mut editor = Self::default();
        editor.text = text.to_string();
        editor.cursor = editor.text.len();
        editor
    }

    /// Place the cursor at a byte offset, snapped to a char boundary. Used by
    /// state-space nodes to render a caret parked mid-text.
    pub fn set_cursor_public(&mut self, cursor: usize) {
        self.set_cursor(cursor);
    }

    fn set_cursor(&mut self, cursor: usize) {
        self.cursor = cursor.min(self.text.len());
        while !self.text.is_char_boundary(self.cursor) {
            self.cursor -= 1;
        }
    }

    fn snapshot(&mut self) {
        self.undo.push((self.text.clone(), self.cursor));
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
    }

    /// Restore the previous snapshot. Returns false when there is nothing to
    /// undo, so callers can surface that instead of silently doing nothing.
    pub fn undo(&mut self) -> bool {
        match self.undo.pop() {
            Some((text, cursor)) => {
                self.text = text;
                self.cursor = cursor.min(self.text.len());
                true
            }
            None => false,
        }
    }

    // --- editing ---

    pub fn insert_str(&mut self, s: &str) {
        let s: String = s.chars().filter(|c| !c.is_control()).collect();
        if s.is_empty() {
            return;
        }
        self.snapshot();
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
    }

    pub fn insert_char(&mut self, c: char) {
        if c.is_control() {
            return;
        }
        self.snapshot();
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Backspace: delete the char before the cursor.
    pub fn delete_back(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.prev_boundary(self.cursor);
        self.snapshot();
        self.text.drain(start..self.cursor);
        self.cursor = start;
    }

    /// Delete: remove the char after the cursor.
    pub fn delete_forward(&mut self) {
        if self.cursor >= self.text.len() {
            return;
        }
        let end = self.next_boundary(self.cursor);
        self.snapshot();
        self.text.drain(self.cursor..end);
    }

    /// Ctrl+W / Alt+Backspace: delete the word before the cursor.
    pub fn delete_word_back(&mut self) {
        let start = self.word_back();
        if start == self.cursor {
            return;
        }
        self.snapshot();
        self.text.drain(start..self.cursor);
        self.cursor = start;
    }

    /// Alt+D: delete the word after the cursor.
    pub fn delete_word_forward(&mut self) {
        let end = self.word_forward();
        if end == self.cursor {
            return;
        }
        self.snapshot();
        self.text.drain(self.cursor..end);
    }

    /// Ctrl+U: delete from the cursor to the start of the line.
    /// Returns the removed text so it can go to the clipboard.
    pub fn kill_to_start(&mut self) -> String {
        if self.cursor == 0 {
            return String::new();
        }
        self.snapshot();
        let killed: String = self.text.drain(..self.cursor).collect();
        self.cursor = 0;
        killed
    }

    /// Ctrl+K: delete from the cursor to the end of the line.
    pub fn kill_to_end(&mut self) -> String {
        if self.cursor >= self.text.len() {
            return String::new();
        }
        self.snapshot();
        self.text.split_off(self.cursor)
    }

    /// Ctrl+X: cut the whole line.
    pub fn cut_line(&mut self) -> String {
        if self.text.is_empty() {
            return String::new();
        }
        self.snapshot();
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    /// Escape: clear the input. Undoable, so it is never destructive.
    pub fn clear(&mut self) {
        if self.text.is_empty() {
            return;
        }
        self.snapshot();
        self.text.clear();
        self.cursor = 0;
    }

    /// Take the buffer for submission, recording it in history.
    pub fn take_for_submit(&mut self) -> String {
        let content = std::mem::take(&mut self.text);
        self.cursor = 0;
        self.undo.clear();
        self.history_index = None;
        self.stashed = None;
        if !content.trim().is_empty() && self.history.last().map(String::as_str) != Some(&content) {
            self.history.push(content.clone());
        }
        content
    }

    // --- motion ---

    pub fn move_left(&mut self) {
        self.cursor = self.prev_boundary(self.cursor);
    }

    pub fn move_right(&mut self) {
        self.cursor = self.next_boundary(self.cursor);
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.text.len();
    }

    pub fn move_word_left(&mut self) {
        self.cursor = self.word_back();
    }

    pub fn move_word_right(&mut self) {
        self.cursor = self.word_forward();
    }

    fn prev_boundary(&self, from: usize) -> usize {
        let mut i = from.saturating_sub(1);
        while i > 0 && !self.text.is_char_boundary(i) {
            i -= 1;
        }
        i
    }

    fn next_boundary(&self, from: usize) -> usize {
        let mut i = (from + 1).min(self.text.len());
        while i < self.text.len() && !self.text.is_char_boundary(i) {
            i += 1;
        }
        i
    }

    /// Word boundary going back: skip whitespace, then word characters.
    /// Mirrors the TUI's `find_word_boundary_back`.
    fn word_back(&self) -> usize {
        if self.cursor == 0 {
            return 0;
        }
        let mut pos = self.prev_boundary(self.cursor);
        while pos > 0 {
            let ch = self.text[pos..].chars().next().unwrap_or(' ');
            if !ch.is_whitespace() {
                break;
            }
            pos = self.prev_boundary(pos);
        }
        while pos > 0 {
            let prev = self.prev_boundary(pos);
            let ch = self.text[prev..].chars().next().unwrap_or(' ');
            if ch.is_whitespace() {
                break;
            }
            pos = prev;
        }
        pos
    }

    /// Word boundary going forward: skip the current word, then whitespace.
    /// Mirrors the TUI's `find_word_boundary_forward`.
    fn word_forward(&self) -> usize {
        let len = self.text.len();
        if self.cursor >= len {
            return len;
        }
        let mut pos = self.cursor;
        while pos < len {
            let ch = self.text[pos..].chars().next().unwrap_or(' ');
            if ch.is_whitespace() {
                break;
            }
            pos = self.next_boundary(pos);
        }
        while pos < len {
            let ch = self.text[pos..].chars().next().unwrap_or(' ');
            if !ch.is_whitespace() {
                break;
            }
            pos = self.next_boundary(pos);
        }
        pos
    }

    // --- history ---

    /// Up: recall an older submission. Live input is stashed on the way out
    /// and restored when navigating back past the newest entry.
    pub fn history_prev(&mut self) -> bool {
        if self.history.is_empty() {
            return false;
        }
        let next = match self.history_index {
            None => {
                self.stashed = Some(self.text.clone());
                self.history.len() - 1
            }
            Some(0) => return false,
            Some(index) => index - 1,
        };
        self.history_index = Some(next);
        self.text = self.history[next].clone();
        self.cursor = self.text.len();
        true
    }

    /// Down: move back toward the live input.
    pub fn history_next(&mut self) -> bool {
        let Some(index) = self.history_index else {
            return false;
        };
        if index + 1 < self.history.len() {
            self.history_index = Some(index + 1);
            self.text = self.history[index + 1].clone();
        } else {
            self.history_index = None;
            self.text = self.stashed.take().unwrap_or_default();
        }
        self.cursor = self.text.len();
        true
    }

    /// True while browsing history, so the UI can say so.
    pub fn browsing_history(&self) -> bool {
        self.history_index.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_backspace_track_the_cursor() {
        let mut editor = Editor::default();
        editor.insert_str("hello");
        assert_eq!((editor.text(), editor.cursor()), ("hello", 5));
        editor.delete_back();
        assert_eq!((editor.text(), editor.cursor()), ("hell", 4));
    }

    #[test]
    fn insertion_happens_at_the_cursor_not_the_end() {
        // The whole point of a real input box: the caret is an index, so text
        // goes where the caret is.
        let mut editor = Editor::with_text("ac");
        editor.set_cursor(1);
        editor.insert_char('b');
        assert_eq!((editor.text(), editor.cursor()), ("abc", 2));
    }

    #[test]
    fn control_characters_are_never_inserted() {
        let mut editor = Editor::default();
        editor.insert_str("a\u{7f}\tb\n");
        assert_eq!(editor.text(), "ab");
    }

    #[test]
    fn home_end_and_arrows_move_the_cursor() {
        let mut editor = Editor::with_text("abc");
        editor.move_home();
        assert_eq!(editor.cursor(), 0);
        editor.move_right();
        assert_eq!(editor.cursor(), 1);
        editor.move_end();
        assert_eq!(editor.cursor(), 3);
        editor.move_left();
        assert_eq!(editor.cursor(), 2);
    }

    #[test]
    fn motion_clamps_at_both_ends() {
        let mut editor = Editor::with_text("ab");
        editor.move_home();
        editor.move_left();
        assert_eq!(editor.cursor(), 0);
        editor.move_end();
        editor.move_right();
        assert_eq!(editor.cursor(), 2);
    }

    #[test]
    fn word_motion_matches_the_tui_semantics() {
        // back: skip whitespace, then word chars. forward: skip word, then
        // whitespace (landing on the next word's first char).
        let mut editor = Editor::with_text("alpha beta gamma");
        editor.move_word_left();
        assert_eq!(&editor.text()[editor.cursor()..], "gamma");
        editor.move_word_left();
        assert_eq!(&editor.text()[editor.cursor()..], "beta gamma");
        editor.move_home();
        editor.move_word_right();
        assert_eq!(&editor.text()[editor.cursor()..], "beta gamma");
    }

    #[test]
    fn word_motion_skips_runs_of_whitespace() {
        let mut editor = Editor::with_text("a   b");
        editor.move_word_left();
        assert_eq!(&editor.text()[editor.cursor()..], "b");
        editor.move_word_left();
        assert_eq!(editor.cursor(), 0);
    }

    #[test]
    fn delete_word_back_removes_one_word() {
        let mut editor = Editor::with_text("alpha beta");
        editor.delete_word_back();
        assert_eq!(editor.text(), "alpha ");
        editor.delete_word_back();
        assert_eq!(editor.text(), "");
    }

    #[test]
    fn delete_word_forward_removes_the_next_word() {
        let mut editor = Editor::with_text("alpha beta");
        editor.set_cursor(0);
        editor.delete_word_forward();
        assert_eq!((editor.text(), editor.cursor()), ("beta", 0));
    }

    #[test]
    fn kill_to_start_and_end_return_the_removed_text() {
        let mut editor = Editor::with_text("alpha beta");
        editor.set_cursor(6);
        assert_eq!(editor.kill_to_start(), "alpha ");
        assert_eq!((editor.text(), editor.cursor()), ("beta", 0));

        let mut editor = Editor::with_text("alpha beta");
        editor.set_cursor(5);
        assert_eq!(editor.kill_to_end(), " beta");
        assert_eq!((editor.text(), editor.cursor()), ("alpha", 5));
    }

    #[test]
    fn cut_line_takes_everything() {
        let mut editor = Editor::with_text("alpha");
        assert_eq!(editor.cut_line(), "alpha");
        assert!(editor.is_empty());
        assert_eq!(editor.cursor(), 0);
    }

    #[test]
    fn undo_restores_text_and_cursor_for_every_edit() {
        // Every mutating operation must be undoable: an edit that cannot be
        // undone is a data-loss bug in an input box.
        type Op = (&'static str, fn(&mut Editor));
        let ops: Vec<Op> = vec![
            ("insert", |e| e.insert_str("xyz")),
            ("delete_back", |e| e.delete_back()),
            ("delete_forward", |e| {
                e.set_cursor(0);
                e.delete_forward()
            }),
            ("delete_word_back", |e| e.delete_word_back()),
            ("delete_word_forward", |e| {
                e.set_cursor(0);
                e.delete_word_forward()
            }),
            ("kill_to_start", |e| {
                e.kill_to_start();
            }),
            ("kill_to_end", |e| {
                e.set_cursor(3);
                e.kill_to_end();
            }),
            ("cut_line", |e| {
                e.cut_line();
            }),
            ("clear", |e| e.clear()),
        ];
        for (name, op) in ops {
            let mut editor = Editor::with_text("alpha beta");
            let before = editor.text().to_string();
            op(&mut editor);
            assert_ne!(editor.text(), before, "{name} did not change anything");
            assert!(editor.undo(), "{name} left nothing to undo");
            assert_eq!(editor.text(), before, "{name} did not restore on undo");
        }
    }

    #[test]
    fn undo_is_bounded() {
        let mut editor = Editor::default();
        for _ in 0..(UNDO_LIMIT + 50) {
            editor.insert_char('a');
        }
        assert!(editor.undo.len() <= UNDO_LIMIT);
    }

    #[test]
    fn undo_reports_when_there_is_nothing_to_undo() {
        let mut editor = Editor::default();
        assert!(!editor.undo());
    }

    #[test]
    fn no_op_edits_do_not_push_undo_states() {
        // Otherwise Ctrl+Z appears to do nothing after a no-op keypress.
        let mut editor = Editor::default();
        editor.delete_back();
        editor.delete_forward();
        editor.delete_word_back();
        editor.clear();
        assert!(!editor.undo());
    }

    #[test]
    fn multibyte_text_never_splits_a_char() {
        let mut editor = Editor::with_text("héllo wörld");
        for _ in 0..20 {
            editor.move_left();
            assert!(editor.text().is_char_boundary(editor.cursor()));
        }
        for _ in 0..40 {
            editor.move_right();
            assert!(editor.text().is_char_boundary(editor.cursor()));
        }
        editor.move_end();
        editor.delete_back();
        assert_eq!(editor.text(), "héllo wörl");
        editor.move_home();
        editor.move_word_right();
        assert!(editor.text().is_char_boundary(editor.cursor()));
    }

    #[test]
    fn emoji_deletes_as_one_unit() {
        let mut editor = Editor::with_text("hi 🌼");
        editor.delete_back();
        assert_eq!(editor.text(), "hi ");
    }

    #[test]
    fn submit_records_history_and_resets() {
        let mut editor = Editor::with_text("first");
        assert_eq!(editor.take_for_submit(), "first");
        assert!(editor.is_empty());
        assert_eq!(editor.cursor(), 0);
        assert_eq!(editor.history, vec!["first".to_string()]);
    }

    #[test]
    fn submit_ignores_blank_and_duplicate_history() {
        let mut editor = Editor::with_text("   ");
        editor.take_for_submit();
        let mut editor2 = Editor::with_text("same");
        editor2.take_for_submit();
        editor2.insert_str("same");
        editor2.take_for_submit();
        assert_eq!(editor2.history.len(), 1);
    }

    #[test]
    fn history_recall_round_trips_and_restores_live_input() {
        let mut editor = Editor::default();
        editor.insert_str("one");
        editor.take_for_submit();
        editor.insert_str("two");
        editor.take_for_submit();
        editor.insert_str("draft");

        assert!(editor.history_prev());
        assert_eq!(editor.text(), "two");
        assert!(editor.history_prev());
        assert_eq!(editor.text(), "one");
        assert!(!editor.history_prev(), "walked past the oldest entry");
        assert!(editor.history_next());
        assert_eq!(editor.text(), "two");
        assert!(editor.history_next());
        assert_eq!(editor.text(), "draft", "live input was not restored");
        assert!(!editor.browsing_history());
        assert!(!editor.history_next());
    }

    #[test]
    fn history_recall_puts_the_cursor_at_the_end() {
        let mut editor = Editor::default();
        editor.insert_str("recalled");
        editor.take_for_submit();
        editor.history_prev();
        assert_eq!(editor.cursor(), editor.text().len());
    }
}
