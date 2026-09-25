//! Search box for the session picker.
//!
//! Resume-style pickers open search-first: the search row is always visible
//! and focused, every printable key is query text, and Tab hands focus to the
//! list where the single-letter shortcuts live.

use super::*;

impl SessionPicker {
    /// Delete the word immediately before the (implicit) end-of-line cursor in
    /// the search query. Used for Ctrl+W / Ctrl+Backspace inside the search bar.
    pub(super) fn delete_search_word_back(&mut self) {
        let query = &self.search_query;
        let mut end = query.len();
        // Skip trailing whitespace.
        while end > 0 {
            let prev = crate::tui::core::prev_char_boundary(query, end);
            let ch = query[prev..].chars().next().unwrap_or(' ');
            if !ch.is_whitespace() {
                break;
            }
            end = prev;
        }
        // Skip the word characters.
        while end > 0 {
            let prev = crate::tui::core::prev_char_boundary(query, end);
            let ch = query[prev..].chars().next().unwrap_or(' ');
            if ch.is_whitespace() {
                break;
            }
            end = prev;
        }
        self.search_query.truncate(end);
    }

    /// Shared handling for key events while the search bar is active. Used by
    /// both the overlay (`handle_overlay_key`) and the standalone `run` loop so
    /// the editing and navigation keybindings stay consistent.
    pub(super) fn handle_search_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<OverlayAction> {
        let ctrl = modifiers.contains(KeyModifiers::CONTROL);
        match code {
            // Esc clears a non-empty query (staying in search); on an empty
            // query it closes the picker so one Esc returns to the chat.
            KeyCode::Esc => {
                if self.search_query.is_empty() {
                    return Ok(OverlayAction::Close);
                }
                self.search_query.clear();
                self.rebuild_items();
            }
            // Tab leaves the search box for the list, keeping the query as the
            // active filter, so the single-letter shortcuts work.
            KeyCode::Tab => {
                self.search_active = false;
            }
            // Crash-restore safety: the banner says "Press R (or B)", so honor
            // uppercase R/B while the query is still empty.
            KeyCode::Char(c @ ('R' | 'B')) if self.search_query.is_empty() && !ctrl => {
                if let Some(info) = &self.crashed_sessions {
                    return Ok(OverlayAction::Selected(PickerResult::RestoreCrashedGroup(
                        info.session_ids.clone(),
                    )));
                }
                self.search_query.push(c);
                self.rebuild_items();
            }
            KeyCode::Enter => {
                self.search_active = false;
                if self.visible_sessions.is_empty() {
                    self.search_query.clear();
                    self.rebuild_items();
                } else {
                    let targets = self.selection_or_current_targets();
                    if !targets.is_empty() {
                        return Ok(OverlayAction::Selected(
                            self.selection_result_for_enter(targets, modifiers),
                        ));
                    }
                }
            }
            // Ctrl+W / Ctrl+Backspace (and the \u{8} BS alias some terminals
            // send for Ctrl+Backspace) delete the previous word in the query.
            KeyCode::Backspace if ctrl => {
                self.delete_search_word_back();
                self.rebuild_items();
            }
            KeyCode::Char('\u{8}') => {
                self.delete_search_word_back();
                self.rebuild_items();
            }
            // Backspace on an empty query is a no-op: holding Backspace must
            // not drop the user into list mode where the next 'q' closes.
            KeyCode::Backspace if self.search_query.is_empty() => {}
            KeyCode::Backspace => {
                self.search_query.pop();
                self.rebuild_items();
            }
            // Ctrl+U clears the whole query (like readline's kill-to-start).
            KeyCode::Char('u') if ctrl => {
                self.search_query.clear();
                self.rebuild_items();
            }
            // Vim-style / readline navigation that keeps working while typing.
            KeyCode::Char('j') | KeyCode::Char('n') if ctrl => self.next(),
            KeyCode::Char('k') | KeyCode::Char('p') if ctrl => self.previous(),
            KeyCode::Char('w') if ctrl => {
                self.delete_search_word_back();
                self.rebuild_items();
            }
            KeyCode::Char(c) => {
                if ctrl && c == 'c' {
                    return Ok(OverlayAction::Close);
                }
                // Ignore other control-modified characters so they don't get
                // inserted as literal text in the search bar.
                if ctrl {
                    return Ok(OverlayAction::Continue);
                }
                self.search_query.push(c);
                self.rebuild_items();
            }
            KeyCode::Down => self.next(),
            KeyCode::Up => self.previous(),
            // Page through results without leaving the search box.
            KeyCode::PageDown | KeyCode::PageUp => {
                self.handle_focus_navigation_key(code, modifiers);
            }
            _ => {}
        }
        Ok(OverlayAction::Continue)
    }

    /// While the session index is still loading, a focused search box keeps
    /// collecting the query (it survives `reseed_grouped` and filters the
    /// loaded list). Returns `true` when the key was consumed.
    pub(super) fn handle_loading_search_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> bool {
        if !self.search_active || modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
            return false;
        }
        match code {
            KeyCode::Char(c) if !c.is_control() => {
                self.search_query.push(c);
                true
            }
            KeyCode::Backspace => {
                self.search_query.pop();
                true
            }
            _ => false,
        }
    }

    /// In list mode, a printable character that is not a single-key shortcut
    /// starts a search and is inserted. Returns `true` when the key was
    /// consumed.
    pub(super) fn start_search_from_list_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> bool {
        let KeyCode::Char(c) = code else {
            return false;
        };
        if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            || c.is_control()
            || Self::is_normal_mode_shortcut_char(c)
        {
            return false;
        }
        self.search_active = true;
        self.search_query.push(c);
        self.rebuild_items();
        true
    }

    /// Characters that act as single-key shortcuts in the normal (non-search)
    /// picker state. Any other printable character starts a search instead.
    /// Keep in sync with the match in `handle_overlay_key` and
    /// `handle_focus_navigation_key`.
    fn is_normal_mode_shortcut_char(c: char) -> bool {
        matches!(
            c,
            'q' | ' '
                | 'd'
                | 'T'
                | 's'
                | 'S'
                | 'R'
                | 'B'
                | 'b'
                | '/'
                | 'h'
                | 'l'
                | 'j'
                | 'k'
                | 'J'
                | 'K'
        )
    }

    /// Focus the search box without touching the query. Resume-style pickers
    /// open search-first so typing any word (even "quota") filters instead of
    /// triggering single-letter shortcuts. Not used for catch-up, active
    /// sessions, or onboarding pickers.
    pub fn focus_search_input(&mut self) {
        self.search_active = true;
    }

    /// Whether the search box currently has keyboard focus.
    pub fn search_input_focused(&self) -> bool {
        self.search_active
    }

    /// Render the always-visible search row, with a placeholder when empty.
    pub(super) fn render_search_bar(&self, frame: &mut Frame, area: Rect) {
        let accent = Style::default().fg(rgb(186, 139, 255));
        let hint = Style::default().fg(rgb(60, 60, 60));
        let cursor_char = if self.search_active { "▎" } else { "" };
        let mut spans = vec![Span::styled(" 🔍 ", accent)];
        if self.search_query.is_empty() {
            let placeholder = if self.search_active {
                "Type to search sessions…  Tab for shortcuts"
            } else {
                "Type or / to search"
            };
            spans.push(Span::styled(cursor_char, accent));
            spans.push(Span::styled(
                placeholder,
                Style::default().fg(rgb(90, 90, 100)),
            ));
        } else {
            spans.push(Span::styled(
                self.search_query.as_str(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(cursor_char, accent));
            spans.push(if self.search_active {
                Span::styled("  Esc to clear · Tab for shortcuts", hint)
            } else {
                Span::styled("  / to edit", hint)
            });
        }
        let widget = Paragraph::new(Line::from(spans)).style(Style::default().bg(rgb(25, 25, 30)));
        frame.render_widget(widget, area);
    }
}
