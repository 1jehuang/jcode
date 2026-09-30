//! Editable text selection in the prompt composer (input box).
//!
//! The selection is stored as a byte-offset anchor into `App::input`; the
//! other end is always the caret (`App::cursor_pos`), the same model every
//! native text field uses. Mouse drags over the composer still run through
//! the shared copy-selection machinery (issue #430), which mirrors its points
//! into this anchor/caret pair, so there is exactly one composer selection.

use super::App;
use crossterm::event::{KeyCode, KeyModifiers};
use std::time::{Duration, Instant};

/// Two presses closer together than this (and on the same or an adjacent cell)
/// count as a multi-click. Crossterm reports no click count, so we derive it.
pub(crate) const MULTI_CLICK_WINDOW: Duration = Duration::from_millis(400);

/// Derives single/double/triple clicks from raw mouse-down events.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ClickCounter {
    last: Option<(Instant, u16, u16, u8)>,
}

impl ClickCounter {
    /// Record a left press at `(column, row)` and return its click count
    /// (1 = single, 2 = double, 3 = triple). A fourth rapid click starts over.
    pub(crate) fn register(&mut self, now: Instant, column: u16, row: u16) -> u8 {
        let count = match self.last {
            Some((at, last_col, last_row, count))
                if count < 3
                    && now.saturating_duration_since(at) <= MULTI_CLICK_WINDOW
                    && last_col.abs_diff(column) <= 1
                    && last_row.abs_diff(row) <= 1 =>
            {
                count + 1
            }
            _ => 1,
        };
        self.last = Some((now, column, row, count));
        count
    }

    pub(crate) fn reset(&mut self) {
        self.last = None;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Word,
    Space,
    Punct,
}

fn char_class(c: char) -> CharClass {
    if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else if c.is_whitespace() {
        CharClass::Space
    } else {
        CharClass::Punct
    }
}

/// Byte range of the word (or whitespace / punctuation run) under `byte`.
///
/// Words are unicode alphanumerics plus `_`. Whitespace and punctuation runs
/// select as a unit, so a double-click on the gap between words selects the
/// gap. A position at the end of a line resolves to the character before it.
/// Newlines are never selected. Both ends are on char boundaries.
pub(crate) fn word_range_at(text: &str, byte: usize) -> Option<(usize, usize)> {
    let mut pos = super::input::floor_char_boundary(text, byte);
    let at = |pos: usize| text[pos..].chars().next();
    if at(pos).is_none_or(|c| c == '\n') {
        // End of text or end of a line: use the previous char on this line.
        let prev = text[..pos].chars().next_back()?;
        if prev == '\n' {
            return None;
        }
        pos -= prev.len_utf8();
    }
    let class = char_class(at(pos)?);
    let same = |c: char| c != '\n' && char_class(c) == class;

    let mut start = pos;
    for c in text[..pos].chars().rev() {
        if !same(c) {
            break;
        }
        start -= c.len_utf8();
    }
    let mut end = pos;
    for c in text[pos..].chars() {
        if !same(c) {
            break;
        }
        end += c.len_utf8();
    }
    Some((start, end))
}

/// Byte range of the logical line containing `byte`, excluding its newline.
pub(crate) fn line_range_at(text: &str, byte: usize) -> (usize, usize) {
    let pos = super::input::floor_char_boundary(text, byte);
    let start = text[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end = text[pos..]
        .find('\n')
        .map(|i| pos + i)
        .unwrap_or(text.len());
    (start, end)
}

impl App {
    /// The active composer selection as an ordered, non-empty byte range.
    pub(crate) fn input_selection(&self) -> Option<(usize, usize)> {
        let anchor = self.input_selection_anchor?;
        let len = self.input.len();
        if anchor > len || !self.input.is_char_boundary(anchor) {
            return None;
        }
        let head = super::input::floor_char_boundary(&self.input, self.cursor_pos);
        (anchor != head).then(|| (anchor.min(head), anchor.max(head)))
    }

    pub(crate) fn clear_input_selection(&mut self) {
        self.input_selection_anchor = None;
    }

    /// Select `anchor..head`, leaving the caret at `head`. Both ends snap to
    /// char boundaries.
    pub(crate) fn set_input_selection(&mut self, anchor: usize, head: usize) {
        let anchor = super::input::floor_char_boundary(&self.input, anchor);
        let head = super::input::floor_char_boundary(&self.input, head);
        self.cursor_pos = head;
        self.input_selection_anchor = (anchor != head).then_some(anchor);
    }

    pub(crate) fn input_selection_text(&self) -> Option<String> {
        let (start, end) = self.input_selection()?;
        Some(self.input[start..end].to_string())
    }

    /// Remove the selected text (recording one undo step) and leave the caret
    /// where it was. Returns the removed text.
    pub(crate) fn delete_input_selection(&mut self) -> Option<String> {
        let (start, end) = self.input_selection()?;
        self.remember_input_undo_state();
        let removed: String = self.input.drain(start..end).collect();
        self.cursor_pos = start;
        self.input_selection_anchor = None;
        self.reset_tab_completion();
        self.sync_model_picker_preview_from_input();
        Some(removed)
    }

    fn copy_input_selection(&mut self) -> bool {
        let Some(text) = self.input_selection_text() else {
            return false;
        };
        if super::copy_to_clipboard(&text) {
            self.set_status_notice("Copied selection");
        } else {
            self.set_status_notice("Failed to copy selection");
        }
        true
    }

    fn cut_input_selection(&mut self) -> bool {
        let Some(text) = self.input_selection_text() else {
            return false;
        };
        if !super::copy_to_clipboard(&text) {
            self.set_status_notice("Failed to copy selection");
            return true;
        }
        self.delete_input_selection();
        self.set_status_notice("✂ Cut selection");
        true
    }

    /// Move the caret to `head`, extending (or starting) a selection anchored
    /// at the current caret.
    fn extend_input_selection_to(&mut self, head: usize) {
        let anchor = self
            .input_selection()
            .and(self.input_selection_anchor)
            .unwrap_or(self.cursor_pos.min(self.input.len()));
        self.set_input_selection(anchor, head);
    }

    /// Mirror a mouse drag over the composer (tracked by the shared
    /// copy-selection machinery) into the editable composer selection, so the
    /// dragged text can be cut, deleted, or typed over. On release the copy
    /// points are dropped: from then on the composer selection is the only
    /// selection over the composer and the only one drawn there.
    pub(super) fn mirror_input_copy_selection(&mut self, kind: crossterm::event::MouseEventKind) {
        use crossterm::event::{MouseButton, MouseEventKind};
        if self.copy_selection_mode
            || !matches!(
                kind,
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
            )
        {
            return;
        }
        let (Some(anchor), Some(cursor)) = (self.copy_selection_anchor, self.copy_selection_cursor)
        else {
            return;
        };
        let input_pane = crate::tui::CopySelectionPane::Input;
        if anchor.pane != input_pane || cursor.pane != input_pane {
            return;
        }
        let to_byte = |point| crate::tui::ui::input_byte_offset_for_copy_point(&self.input, point);
        if let (Some(anchor), Some(head)) = (to_byte(anchor), to_byte(cursor)) {
            self.set_input_selection(anchor, head);
            self.reset_tab_completion();
        }
        if matches!(kind, MouseEventKind::Up(_)) {
            self.copy_selection_anchor = None;
            self.copy_selection_cursor = None;
            self.copy_selection_goal_column = None;
        }
    }

    /// Mouse press in the composer at byte `pos` (the char under the pointer)
    /// with the derived click count. Returns true when it made a selection.
    pub(super) fn select_input_for_click(&mut self, pos: usize, clicks: u8) -> bool {
        let range = match clicks {
            2 => word_range_at(&self.input, pos),
            3 => Some(line_range_at(&self.input, pos)),
            _ => None,
        };
        match range {
            Some((start, end)) if start < end => {
                self.set_input_selection(start, end);
                true
            }
            _ => false,
        }
    }
}

/// Keys that act on (or extend) the composer selection. Runs ahead of the
/// ordinary shortcut handlers in both the local and remote key paths. Returns
/// true when the key was consumed.
///
/// With a selection active:
/// - Ctrl/Cmd+C copies it (instead of clearing the input or quitting).
/// - Ctrl/Cmd+X cuts it (instead of cutting the whole line).
/// - Backspace/Delete remove it.
/// - Left/Right collapse it to its start/end; Esc drops it.
/// - Printable text, Shift/Alt+Enter and paste chords fall through and replace
///   it at the shared insertion boundary (`insert_input_text`).
/// - Any other key drops the selection and then runs as usual.
///
/// Shift+Left/Right/Home/End extend (or start) a selection at any time.
pub(super) fn handle_input_selection_key(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    if matches!(code, KeyCode::Modifier(_)) {
        return false;
    }

    if modifiers == KeyModifiers::SHIFT
        && !app.input.is_empty()
        && !app.diff_pane_focus
        && !app.diagram_focus
    {
        let cursor = app.cursor_pos.min(app.input.len());
        let head = match code {
            KeyCode::Left => Some(crate::tui::core::prev_char_boundary(&app.input, cursor)),
            KeyCode::Right => Some(crate::tui::core::next_char_boundary(&app.input, cursor)),
            KeyCode::Home => Some(0),
            KeyCode::End => Some(app.input.len()),
            _ => None,
        };
        if let Some(head) = head {
            app.extend_input_selection_to(head);
            app.reset_tab_completion();
            return true;
        }
    }

    let Some((start, end)) = app.input_selection() else {
        app.input_selection_anchor = None;
        return false;
    };

    let command = modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
        && !modifiers.contains(KeyModifiers::ALT);
    match code {
        KeyCode::Char('c' | 'C') if command => app.copy_input_selection(),
        KeyCode::Char('x' | 'X') if command => app.cut_input_selection(),
        KeyCode::Backspace | KeyCode::Delete | KeyCode::Char('\u{7f}')
            if !modifiers.contains(KeyModifiers::SHIFT) =>
        {
            app.delete_input_selection();
            true
        }
        KeyCode::Left if modifiers.is_empty() => {
            app.clear_input_selection();
            app.cursor_pos = start;
            true
        }
        KeyCode::Right if modifiers.is_empty() => {
            app.clear_input_selection();
            app.cursor_pos = end;
            true
        }
        KeyCode::Esc => {
            // First Esc drops the selection. While a turn is running, let the
            // same press still reach the interrupt handler.
            app.clear_input_selection();
            !app.is_processing
        }
        // These replace the selection at the insertion boundary.
        KeyCode::Enter if modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => false,
        KeyCode::Char('v' | 'V')
            if modifiers.intersects(
                KeyModifiers::CONTROL
                    | KeyModifiers::ALT
                    | KeyModifiers::SUPER
                    | KeyModifiers::META,
            ) =>
        {
            false
        }
        _ if super::input::text_input_for_key(code, modifiers).is_some() => false,
        _ => {
            app.clear_input_selection();
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64) -> Instant {
        // A fixed base keeps the arithmetic obvious.
        static BASE: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        *BASE.get_or_init(Instant::now) + Duration::from_millis(ms)
    }

    #[test]
    fn click_counter_counts_single_double_triple() {
        let mut clicks = ClickCounter::default();
        assert_eq!(clicks.register(at(0), 10, 5), 1);
        assert_eq!(clicks.register(at(150), 10, 5), 2);
        assert_eq!(clicks.register(at(300), 10, 5), 3);
        // A fourth rapid click starts a new sequence.
        assert_eq!(clicks.register(at(450), 10, 5), 1);
    }

    #[test]
    fn click_counter_tolerates_one_cell_of_jitter() {
        let mut clicks = ClickCounter::default();
        assert_eq!(clicks.register(at(0), 10, 5), 1);
        assert_eq!(clicks.register(at(100), 11, 5), 2);
        assert_eq!(clicks.register(at(200), 10, 6), 3);
    }

    #[test]
    fn click_counter_resets_after_timeout() {
        let mut clicks = ClickCounter::default();
        assert_eq!(clicks.register(at(0), 10, 5), 1);
        assert_eq!(clicks.register(at(401), 10, 5), 1);
        assert_eq!(clicks.register(at(500), 10, 5), 2);
    }

    #[test]
    fn click_counter_resets_when_pointer_moves_away() {
        let mut clicks = ClickCounter::default();
        assert_eq!(clicks.register(at(0), 10, 5), 1);
        assert_eq!(clicks.register(at(100), 13, 5), 1);
        assert_eq!(clicks.register(at(200), 13, 5), 2);
        assert_eq!(clicks.register(at(300), 13, 8), 1);
    }

    #[test]
    fn word_range_selects_word_under_byte() {
        let text = "hello big world";
        assert_eq!(word_range_at(text, 6), Some((6, 9)));
        assert_eq!(word_range_at(text, 8), Some((6, 9)));
        assert_eq!(word_range_at(text, 0), Some((0, 5)));
        // End of text resolves to the last character's word.
        assert_eq!(word_range_at(text, text.len()), Some((10, 15)));
        assert_eq!(word_range_at("snake_case_1 x", 3), Some((0, 12)));
    }

    #[test]
    fn word_range_on_whitespace_selects_the_whitespace_run() {
        assert_eq!(word_range_at("a   b", 2), Some((1, 4)));
        assert_eq!(word_range_at("", 0), None);
        // A click past the end of a line (caret on its newline) picks the
        // word before it, like native editors; a newline is never selected.
        assert_eq!(word_range_at("ab\ncd", 2), Some((0, 2)));
        assert_eq!(word_range_at("a\n\nb", 2), None);
    }

    #[test]
    fn word_range_groups_punctuation_runs() {
        assert_eq!(word_range_at("foo::bar", 3), Some((3, 5)));
    }

    #[test]
    fn word_range_respects_unicode_and_emoji_char_boundaries() {
        let text = "café naïve 🙂🙂 end";
        let cafe_end = "café".len();
        assert_eq!(word_range_at(text, 1), Some((0, cafe_end)));
        let naive_start = "café ".len();
        let naive_end = "café naïve".len();
        // Byte inside the multi-byte 'ï' snaps to its char.
        assert_eq!(
            word_range_at(text, naive_start + 3),
            Some((naive_start, naive_end))
        );
        let emoji_start = "café naïve ".len();
        let emoji_end = emoji_start + "🙂🙂".len();
        let (s, e) = word_range_at(text, emoji_start + 1).expect("emoji run");
        assert_eq!((s, e), (emoji_start, emoji_end));
        assert!(text.is_char_boundary(s) && text.is_char_boundary(e));
    }

    #[test]
    fn line_range_selects_logical_line_without_newline() {
        let text = "first line\nsecond line\nthird";
        assert_eq!(line_range_at(text, 3), (0, 10));
        assert_eq!(line_range_at(text, 11), (11, 22));
        assert_eq!(line_range_at(text, 13), (11, 22));
        assert_eq!(line_range_at(text, text.len()), (23, 28));
        // Single-line input: the whole input.
        assert_eq!(line_range_at("héllo 🙂", 2), (0, "héllo 🙂".len()));
    }
}
