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
    /// Selection anchor. `Some` means there is a selection between the anchor
    /// and the cursor; the cursor is the moving end.
    anchor: Option<usize>,
    /// Undo stack of (text, cursor) snapshots.
    undo: Vec<(String, usize)>,
    /// Redo stack, filled by [`Editor::undo`] and cleared by any fresh edit,
    /// exactly like a browser text field: Ctrl+Shift+Z after a new keystroke
    /// must not resurrect an abandoned future.
    redo: Vec<(String, usize)>,
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

    /// Selected byte range, normalized to (start, end). `None` when there is
    /// no selection or it is empty.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        let (start, end) = if anchor <= self.cursor {
            (anchor, self.cursor)
        } else {
            (self.cursor, anchor)
        };
        (start != end).then_some((start, end))
    }

    /// The selected text, if any.
    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|(start, end)| &self.text[start..end])
    }

    /// Begin or extend a selection anchored at the current cursor. Called
    /// before a motion when Shift is held, or on mouse-down.
    pub fn anchor_selection(&mut self) {
        if self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }
    }

    /// Drop any selection, keeping the cursor where it is.
    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    /// Select everything (Ctrl+A style select-all is not bound, but mouse
    /// double/triple click and callers may want it).
    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.text.len();
    }

    /// Place the cursor at a byte offset, collapsing any selection.
    pub fn place_cursor(&mut self, offset: usize) {
        self.set_cursor(offset);
        self.anchor = None;
    }

    /// Extend the selection to a byte offset, anchoring first if needed.
    pub fn extend_to(&mut self, offset: usize) {
        self.anchor_selection();
        self.set_cursor(offset);
    }

    /// Delete the selection if there is one. Returns the removed text so it
    /// can feed the clipboard, and `None` when nothing was selected.
    pub fn delete_selection(&mut self) -> Option<String> {
        let (start, end) = self.selection()?;
        self.snapshot();
        let removed: String = self.text[start..end].to_string();
        self.text.drain(start..end);
        self.cursor = start;
        self.anchor = None;
        Some(removed)
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
        self.cursor = self.snap(cursor);
    }

    /// Clamp a byte offset into the buffer and snap it to a char boundary, so
    /// offsets from mouse hit-testing can never split a character.
    fn snap(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.text.len());
        while offset > 0 && !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    fn snapshot(&mut self) {
        self.undo.push((self.text.clone(), self.cursor));
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        // A fresh edit forks history: the redo branch is gone.
        self.redo.clear();
    }

    /// Restore the previous snapshot. Returns false when there is nothing to
    /// undo, so callers can surface that instead of silently doing nothing.
    pub fn undo(&mut self) -> bool {
        match self.undo.pop() {
            Some((text, cursor)) => {
                self.redo.push((self.text.clone(), self.cursor));
                self.text = text;
                self.cursor = cursor.min(self.text.len());
                self.anchor = None;
                true
            }
            None => false,
        }
    }

    /// Redo the most recently undone edit. Returns false when the redo branch
    /// is empty.
    pub fn redo(&mut self) -> bool {
        match self.redo.pop() {
            Some((text, cursor)) => {
                self.undo.push((self.text.clone(), self.cursor));
                self.text = text;
                self.cursor = cursor.min(self.text.len());
                self.anchor = None;
                true
            }
            None => false,
        }
    }

    // --- editing ---

    pub fn insert_str(&mut self, s: &str) {
        // Newlines are content in a multi-line composer; other control
        // characters are not. Normalize CRLF so pasted text behaves.
        let s: String = s
            .replace("\r\n", "\n")
            .chars()
            .filter(|c| *c == '\n' || !c.is_control())
            .collect();
        if s.is_empty() {
            return;
        }
        // Typing over a selection replaces it, like any normal input box.
        self.delete_selection();
        self.snapshot();
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
    }

    pub fn insert_char(&mut self, c: char) {
        if c.is_control() && c != '\n' {
            return;
        }
        self.delete_selection();
        self.snapshot();
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Backspace: delete the char before the cursor.
    pub fn delete_back(&mut self) {
        if self.delete_selection().is_some() {
            return;
        }
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
        if self.delete_selection().is_some() {
            return;
        }
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

    /// Ctrl+U: delete from the cursor to the start of the current line.
    /// Returns the removed text so it can go to the clipboard.
    pub fn kill_to_start(&mut self) -> String {
        let start = self.line_start(self.cursor);
        if start == self.cursor {
            return String::new();
        }
        self.snapshot();
        let killed: String = self.text.drain(start..self.cursor).collect();
        self.cursor = start;
        self.anchor = None;
        killed
    }

    /// Ctrl+K: delete from the cursor to the end of the current line.
    pub fn kill_to_end(&mut self) -> String {
        let end = self.line_end(self.cursor);
        if end == self.cursor {
            return String::new();
        }
        self.snapshot();
        self.anchor = None;
        self.text.drain(self.cursor..end).collect()
    }

    /// Ctrl+X: cut the whole line.
    pub fn cut_line(&mut self) -> String {
        if self.text.is_empty() {
            return String::new();
        }
        self.snapshot();
        self.cursor = 0;
        self.anchor = None;
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
        self.anchor = None;
    }

    /// Take the buffer for submission, recording it in history.
    pub fn take_for_submit(&mut self) -> String {
        let content = std::mem::take(&mut self.text);
        self.cursor = 0;
        self.anchor = None;
        self.undo.clear();
        self.redo.clear();
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
        self.anchor = None;
    }

    /// Motion while extending a selection: anchors first, then moves.
    pub fn extend_left(&mut self) {
        self.anchor_selection();
        self.cursor = self.prev_boundary(self.cursor);
    }

    pub fn extend_right(&mut self) {
        self.anchor_selection();
        self.cursor = self.next_boundary(self.cursor);
    }

    pub fn extend_word_left(&mut self) {
        self.anchor_selection();
        self.cursor = self.word_back();
    }

    pub fn extend_word_right(&mut self) {
        self.anchor_selection();
        self.cursor = self.word_forward();
    }

    pub fn extend_home(&mut self) {
        self.anchor_selection();
        self.cursor = self.line_start(self.cursor);
    }

    pub fn extend_end(&mut self) {
        self.anchor_selection();
        self.cursor = self.line_end(self.cursor);
    }

    /// Shift+Up / Shift+Down in a browser textarea: extend the selection a
    /// line at a time, and at the buffer edges extend to the very start or end
    /// rather than doing nothing. Returns false when the selection could not
    /// grow at all, so callers can leave history recall alone.
    pub fn extend_line(&mut self, delta: isize) -> bool {
        self.anchor_selection();
        let (line, col) = self.cursor_line_col();
        let target = if delta < 0 {
            if line == 0 {
                // Already on the first line: select to the top of the buffer.
                let moved = self.cursor != 0;
                self.cursor = 0;
                return moved;
            }
            line - 1
        } else {
            if line + 1 >= self.line_count() {
                let moved = self.cursor != self.text.len();
                self.cursor = self.text.len();
                return moved;
            }
            line + 1
        };
        let mut start = 0usize;
        for _ in 0..target {
            start = self.line_end(start) + 1;
        }
        let end = self.line_end(start);
        self.cursor = self.snap((start + col).min(end));
        true
    }

    /// Ctrl+Shift+Home / End: extend to the start or end of the buffer.
    pub fn extend_to_start(&mut self) {
        self.anchor_selection();
        self.cursor = 0;
    }

    pub fn extend_to_end(&mut self) {
        self.anchor_selection();
        self.cursor = self.text.len();
    }

    /// Ctrl+Home / Ctrl+End inside the composer: move to the buffer edges.
    pub fn move_to_start(&mut self) {
        self.cursor = 0;
        self.anchor = None;
    }

    pub fn move_to_end(&mut self) {
        self.cursor = self.text.len();
        self.anchor = None;
    }

    /// Word range containing `offset`, for double-click selection.
    pub fn word_range_at(&self, offset: usize) -> (usize, usize) {
        let len = self.text.len();
        if len == 0 {
            return (0, 0);
        }
        // Snap into the buffer on a char boundary: a click can land anywhere,
        // and slicing mid-char would panic.
        let offset = self.snap(offset);
        let is_word = |at: usize| {
            self.text[at..]
                .chars()
                .next()
                .map(|c| !c.is_whitespace())
                .unwrap_or(false)
        };
        // Clicking in whitespace selects the whitespace run. At the very end
        // of the buffer, classify by the character before it.
        let probe = if offset < len {
            offset
        } else {
            self.prev_boundary(len)
        };
        let target_is_word = is_word(probe);
        let mut start = offset;
        while start > 0 {
            let prev = self.prev_boundary(start);
            if is_word(prev) != target_is_word {
                break;
            }
            start = prev;
        }
        let mut end = offset;
        while end < len && is_word(end) == target_is_word {
            end = self.next_boundary(end);
        }
        (start, end)
    }

    pub fn move_right(&mut self) {
        self.cursor = self.next_boundary(self.cursor);
        self.anchor = None;
    }

    /// Home goes to the start of the *visual line*, like any multi-line
    /// editor, not the start of the buffer.
    pub fn move_home(&mut self) {
        self.cursor = self.line_start(self.cursor);
        self.anchor = None;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.line_end(self.cursor);
        self.anchor = None;
    }

    /// Byte offset of the start of the line containing `at`.
    pub fn line_start(&self, at: usize) -> usize {
        self.text[..at.min(self.text.len())]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0)
    }

    /// Byte offset of the end of the line containing `at`.
    pub fn line_end(&self, at: usize) -> usize {
        let at = at.min(self.text.len());
        self.text[at..]
            .find('\n')
            .map(|i| at + i)
            .unwrap_or(self.text.len())
    }

    /// Lines in the buffer. Always at least one, so the composer never
    /// collapses to zero height.
    pub fn line_count(&self) -> usize {
        self.text.matches('\n').count() + 1
    }

    /// The cursor's line index and byte column within that line.
    pub fn cursor_line_col(&self) -> (usize, usize) {
        let start = self.line_start(self.cursor);
        let line = self.text[..start].matches('\n').count();
        (line, self.cursor - start)
    }

    /// Move the cursor up or down a line, preserving the column where
    /// possible. Returns false when there is no line in that direction, so
    /// callers can fall back to history recall.
    pub fn move_line(&mut self, delta: isize) -> bool {
        let (line, col) = self.cursor_line_col();
        // A plain motion always collapses the selection, including when it
        // cannot move: otherwise pressing Up at the first line would leave a
        // stale highlight behind.
        self.anchor = None;
        let target = match delta {
            d if d < 0 => {
                if line == 0 {
                    return false;
                }
                line - 1
            }
            _ => {
                if line + 1 >= self.line_count() {
                    return false;
                }
                line + 1
            }
        };
        let mut start = 0usize;
        for _ in 0..target {
            start = self.line_end(start) + 1;
        }
        let end = self.line_end(start);
        self.cursor = self.snap((start + col).min(end));
        self.anchor = None;
        true
    }

    pub fn move_word_left(&mut self) {
        self.cursor = self.word_back();
        self.anchor = None;
    }

    pub fn move_word_right(&mut self) {
        self.cursor = self.word_forward();
        self.anchor = None;
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
        self.anchor = None;
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
        self.anchor = None;
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
    fn control_characters_are_stripped_but_newlines_are_kept() {
        // The composer is multi-line, so a newline is content. Other control
        // characters (tabs, DEL, escapes) would corrupt layout.
        let mut editor = Editor::default();
        editor.insert_str("a\u{7f}\tb\nc\u{1b}");
        assert_eq!(editor.text(), "ab\nc");
    }

    #[test]
    fn pasted_crlf_is_normalized() {
        let mut editor = Editor::default();
        editor.insert_str("one\r\ntwo");
        assert_eq!(editor.text(), "one\ntwo");
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
    fn redo_replays_an_undone_edit_and_a_new_edit_forks_the_branch() {
        let mut editor = Editor::with_text("alpha");
        editor.insert_str(" beta");
        assert_eq!(editor.text(), "alpha beta");
        assert!(editor.undo());
        assert_eq!(editor.text(), "alpha");
        assert!(editor.redo(), "Ctrl+Shift+Z had nothing to replay");
        assert_eq!(editor.text(), "alpha beta");
        assert_eq!(editor.cursor(), editor.text().len());

        // A fresh edit after an undo abandons the redo branch, as in a browser:
        // resurrecting it would paste text the user never asked for.
        assert!(editor.undo());
        editor.insert_str(" gamma");
        assert!(!editor.redo(), "a stale redo branch survived a new edit");
    }

    #[test]
    fn redo_reports_when_there_is_nothing_to_redo() {
        let mut editor = Editor::with_text("alpha");
        assert!(!editor.redo());
    }

    #[test]
    fn shift_arrows_extend_by_line_and_clamp_at_the_edges() {
        let mut editor = Editor::with_text("one\ntwo\nthree");
        editor.set_cursor(editor.text().len());
        // The column clamps to the shorter line, as in a textarea.
        assert!(editor.extend_line(-1));
        assert_eq!(editor.selected_text(), Some("\nthree"));
        assert!(editor.extend_line(-1));
        assert_eq!(editor.selected_text(), Some("\ntwo\nthree"));
        // At the first line, extend to the very start rather than stalling.
        assert!(editor.extend_line(-1));
        assert_eq!(editor.selected_text(), Some("one\ntwo\nthree"));
        assert!(
            !editor.extend_line(-1),
            "a pinned edge kept reporting motion"
        );
    }

    #[test]
    fn ctrl_home_and_end_reach_the_ends_of_the_field() {
        let mut editor = Editor::with_text("one\ntwo");
        editor.set_cursor(5);
        editor.move_to_start();
        assert_eq!(editor.cursor(), 0);
        assert!(editor.selection().is_none());
        editor.extend_to_end();
        assert_eq!(editor.selected_text(), Some("one\ntwo"));
        editor.move_to_end();
        assert_eq!(editor.cursor(), editor.text().len());
        editor.extend_to_start();
        assert_eq!(editor.selected_text(), Some("one\ntwo"));
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

#[cfg(test)]
mod selection_tests {
    use super::*;

    #[test]
    fn there_is_no_selection_by_default() {
        let editor = Editor::with_text("hello");
        assert_eq!(editor.selection(), None);
        assert_eq!(editor.selected_text(), None);
    }

    #[test]
    fn shift_motion_extends_a_selection_from_the_cursor() {
        let mut editor = Editor::with_text("hello");
        editor.set_cursor(0);
        editor.extend_right();
        editor.extend_right();
        assert_eq!(editor.selected_text(), Some("he"));
    }

    #[test]
    fn a_selection_normalizes_regardless_of_drag_direction() {
        // Selecting right-to-left must produce the same range as left-to-right.
        let mut forward = Editor::with_text("hello");
        forward.set_cursor(1);
        forward.extend_to(4);
        let mut backward = Editor::with_text("hello");
        backward.set_cursor(4);
        backward.extend_to(1);
        assert_eq!(forward.selection(), backward.selection());
        assert_eq!(backward.selected_text(), Some("ell"));
    }

    /// *Every* plain motion must collapse the selection, not just whichever one
    /// happened to be tested: a lingering highlight after any arrow key is a
    /// bug, and testing one motion lets the others regress silently.
    #[test]
    fn every_plain_motion_collapses_the_selection() {
        type Motion = (&'static str, fn(&mut Editor));
        let motions: Vec<Motion> = vec![
            ("move_left", |e| e.move_left()),
            ("move_right", |e| e.move_right()),
            ("move_home", |e| e.move_home()),
            ("move_end", |e| e.move_end()),
            ("move_word_left", |e| e.move_word_left()),
            ("move_word_right", |e| e.move_word_right()),
            ("move_line_up", |e| {
                e.move_line(-1);
            }),
            ("move_line_down", |e| {
                e.move_line(1);
            }),
        ];
        for (name, motion) in motions {
            let mut editor = Editor::with_text("alpha bravo\ncharlie delta");
            editor.place_cursor(3);
            editor.extend_to(8);
            assert!(editor.selection().is_some(), "{name}: setup failed");
            motion(&mut editor);
            assert_eq!(
                editor.selection(),
                None,
                "{name} left a stale selection behind"
            );
        }
    }

    /// And every extending motion must *keep* a selection, so the two families
    /// can never be confused for one another.
    #[test]
    fn every_extending_motion_keeps_a_selection() {
        type Motion = (&'static str, fn(&mut Editor));
        let motions: Vec<Motion> = vec![
            ("extend_left", |e| e.extend_left()),
            ("extend_right", |e| e.extend_right()),
            ("extend_home", |e| e.extend_home()),
            ("extend_end", |e| e.extend_end()),
            ("extend_word_left", |e| e.extend_word_left()),
            ("extend_word_right", |e| e.extend_word_right()),
        ];
        for (name, motion) in motions {
            let mut editor = Editor::with_text("alpha bravo charlie");
            editor.place_cursor(8);
            motion(&mut editor);
            assert!(
                editor.selection().is_some(),
                "{name} did not create a selection"
            );
        }
    }

    #[test]
    fn plain_motion_collapses_the_selection() {
        // Otherwise the highlight would linger after an arrow key.
        let mut editor = Editor::with_text("hello");
        editor.set_cursor(0);
        editor.extend_right();
        assert!(editor.selection().is_some());
        editor.move_right();
        assert_eq!(
            editor.selection(),
            None,
            "selection survived a plain motion"
        );
    }

    #[test]
    fn typing_replaces_the_selection() {
        let mut editor = Editor::with_text("hello world");
        editor.set_cursor(0);
        for _ in 0..5 {
            editor.extend_right();
        }
        editor.insert_str("goodbye");
        assert_eq!(editor.text(), "goodbye world");
        assert_eq!(editor.selection(), None);
    }

    #[test]
    fn backspace_and_delete_remove_the_selection_only() {
        for delete_forward in [false, true] {
            let mut editor = Editor::with_text("hello world");
            editor.set_cursor(5);
            editor.extend_to(0);
            if delete_forward {
                editor.delete_forward();
            } else {
                editor.delete_back();
            }
            assert_eq!(
                editor.text(),
                " world",
                "selection delete removed the wrong range"
            );
            assert_eq!(editor.cursor(), 0);
        }
    }

    #[test]
    fn deleting_a_selection_is_undoable() {
        let mut editor = Editor::with_text("hello world");
        editor.select_all();
        editor.delete_back();
        assert!(editor.is_empty());
        assert!(editor.undo());
        assert_eq!(editor.text(), "hello world");
    }

    #[test]
    fn delete_selection_returns_the_removed_text_for_the_clipboard() {
        let mut editor = Editor::with_text("cut this out");
        editor.set_cursor(4);
        editor.extend_to(8);
        assert_eq!(editor.delete_selection().as_deref(), Some("this"));
        assert_eq!(editor.text(), "cut  out");
    }

    #[test]
    fn delete_selection_is_a_no_op_without_a_selection() {
        let mut editor = Editor::with_text("keep");
        assert_eq!(editor.delete_selection(), None);
        assert_eq!(editor.text(), "keep");
        assert!(!editor.undo(), "a no-op consumed an undo state");
    }

    #[test]
    fn select_all_covers_the_whole_buffer() {
        let mut editor = Editor::with_text("everything");
        editor.select_all();
        assert_eq!(editor.selected_text(), Some("everything"));
    }

    #[test]
    fn an_empty_selection_reads_as_no_selection() {
        // Anchoring without moving must not render a zero-width highlight.
        let mut editor = Editor::with_text("hello");
        editor.anchor_selection();
        assert_eq!(editor.selection(), None);
    }

    #[test]
    fn double_click_selects_the_word_under_the_offset() {
        let editor = Editor::with_text("alpha beta gamma");
        let (start, end) = editor.word_range_at(7);
        assert_eq!(&editor.text()[start..end], "beta");
    }

    #[test]
    fn double_click_at_a_word_edge_selects_that_word() {
        let editor = Editor::with_text("alpha beta");
        let (start, end) = editor.word_range_at(6);
        assert_eq!(&editor.text()[start..end], "beta");
        let (start, end) = editor.word_range_at(0);
        assert_eq!(&editor.text()[start..end], "alpha");
    }

    #[test]
    fn double_click_in_whitespace_selects_the_whitespace_run() {
        let editor = Editor::with_text("alpha   beta");
        let (start, end) = editor.word_range_at(6);
        assert_eq!(&editor.text()[start..end], "   ");
    }

    #[test]
    fn word_range_is_safe_on_empty_text_and_past_the_end() {
        let editor = Editor::default();
        assert_eq!(editor.word_range_at(0), (0, 0));
        let editor = Editor::with_text("hi");
        let (start, end) = editor.word_range_at(999);
        assert!(start <= end && end <= editor.text().len());
    }

    #[test]
    fn selection_offsets_stay_on_char_boundaries() {
        let mut editor = Editor::with_text("héllo wörld");
        editor.place_cursor(0);
        for _ in 0..20 {
            editor.extend_right();
            let (start, end) = editor.selection().unwrap_or((0, 0));
            assert!(editor.text().is_char_boundary(start));
            assert!(editor.text().is_char_boundary(end));
        }
        let (start, end) = editor.word_range_at(2);
        assert!(editor.text().is_char_boundary(start));
        assert!(editor.text().is_char_boundary(end));
    }

    #[test]
    fn place_cursor_snaps_into_the_buffer() {
        let mut editor = Editor::with_text("héllo");
        editor.place_cursor(2);
        assert!(editor.text().is_char_boundary(editor.cursor()));
        editor.place_cursor(9999);
        assert_eq!(editor.cursor(), editor.text().len());
    }

    #[test]
    fn submitting_and_history_clear_the_selection() {
        let mut editor = Editor::with_text("first");
        editor.select_all();
        editor.take_for_submit();
        assert_eq!(editor.selection(), None);
        editor.insert_str("draft");
        editor.select_all();
        editor.history_prev();
        assert_eq!(editor.selection(), None, "stale selection after recall");
    }
}

#[cfg(test)]
mod multiline_tests {
    use super::*;

    #[test]
    fn shift_enter_inserts_a_real_newline() {
        let mut editor = Editor::with_text("first");
        editor.insert_char('\n');
        editor.insert_str("second");
        assert_eq!(editor.text(), "first\nsecond");
        assert_eq!(editor.line_count(), 2);
    }

    #[test]
    fn an_empty_buffer_still_has_one_line() {
        // The composer must never collapse to zero height.
        assert_eq!(Editor::default().line_count(), 1);
    }

    #[test]
    fn a_trailing_newline_opens_a_new_line() {
        let editor = Editor::with_text("one\n");
        assert_eq!(editor.line_count(), 2);
    }

    #[test]
    fn home_and_end_work_on_the_current_line_not_the_buffer() {
        let mut editor = Editor::with_text("alpha\nbeta gamma");
        editor.set_cursor(11); // inside "gamma"
        editor.move_home();
        assert_eq!(&editor.text()[editor.cursor()..], "beta gamma");
        editor.move_end();
        assert_eq!(editor.cursor(), editor.text().len());

        // On the first line, End must stop at the newline.
        editor.set_cursor(2);
        editor.move_end();
        assert_eq!(&editor.text()[editor.cursor()..], "\nbeta gamma");
    }

    #[test]
    fn kill_to_end_stops_at_the_line_break() {
        let mut editor = Editor::with_text("alpha\nbeta");
        editor.set_cursor(2);
        assert_eq!(editor.kill_to_end(), "pha");
        assert_eq!(
            editor.text(),
            "al\nbeta",
            "Ctrl+K ate the rest of the buffer"
        );
    }

    #[test]
    fn kill_to_start_stops_at_the_line_break() {
        let mut editor = Editor::with_text("alpha\nbeta");
        editor.move_end();
        assert_eq!(editor.kill_to_start(), "beta");
        assert_eq!(editor.text(), "alpha\n");
    }

    #[test]
    fn cursor_line_col_tracks_position() {
        let mut editor = Editor::with_text("ab\ncde");
        editor.set_cursor(0);
        assert_eq!(editor.cursor_line_col(), (0, 0));
        editor.set_cursor(2);
        assert_eq!(editor.cursor_line_col(), (0, 2));
        editor.set_cursor(4);
        assert_eq!(editor.cursor_line_col(), (1, 1));
    }

    #[test]
    fn moving_between_lines_preserves_the_column() {
        let mut editor = Editor::with_text("alpha\nbravo");
        editor.set_cursor(3); // column 3 of line 0
        assert!(editor.move_line(1));
        assert_eq!(editor.cursor_line_col(), (1, 3));
        assert!(editor.move_line(-1));
        assert_eq!(editor.cursor_line_col(), (0, 3));
    }

    #[test]
    fn moving_to_a_shorter_line_clamps_to_its_end() {
        let mut editor = Editor::with_text("alphabet\nhi");
        editor.set_cursor(7);
        assert!(editor.move_line(1));
        assert_eq!(
            editor.cursor(),
            editor.text().len(),
            "column was not clamped"
        );
    }

    #[test]
    fn line_motion_reports_when_there_is_nowhere_to_go() {
        // This is what lets Up/Down fall back to history recall.
        let mut editor = Editor::with_text("only line");
        assert!(!editor.move_line(-1));
        assert!(!editor.move_line(1));

        let mut editor = Editor::with_text("one\ntwo");
        editor.set_cursor(0);
        assert!(!editor.move_line(-1), "moved above the first line");
        assert!(editor.move_line(1));
        assert!(!editor.move_line(1), "moved below the last line");
    }

    #[test]
    fn line_motion_stays_on_char_boundaries() {
        let mut editor = Editor::with_text("héllo\nwörld");
        editor.set_cursor(3);
        editor.move_line(1);
        assert!(editor.text().is_char_boundary(editor.cursor()));
        editor.move_line(-1);
        assert!(editor.text().is_char_boundary(editor.cursor()));
    }

    #[test]
    fn selection_can_span_lines() {
        let mut editor = Editor::with_text("alpha\nbravo");
        editor.place_cursor(3);
        editor.extend_to(8);
        assert_eq!(editor.selected_text(), Some("ha\nbr"));
    }

    #[test]
    fn word_motion_crosses_line_breaks() {
        let mut editor = Editor::with_text("alpha\nbravo");
        editor.set_cursor(0);
        editor.move_word_right();
        // A newline is whitespace, so forward motion lands on the next word.
        assert_eq!(&editor.text()[editor.cursor()..], "bravo");
        editor.move_word_left();
        assert_eq!(&editor.text()[editor.cursor()..], "alpha\nbravo");
    }
}
