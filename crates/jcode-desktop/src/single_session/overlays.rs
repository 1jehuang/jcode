use super::*;

impl SingleSessionApp {
    pub(crate) fn mark_inline_widget_opened(&mut self) {
        self.view.inline_widget_opened_at = Some(Instant::now());
        self.view.closing_inline_widget = None;
    }

    pub(crate) fn capture_inline_widget_exit(&mut self) {
        if crate::animation::desktop_reduced_motion_enabled() {
            self.view.closing_inline_widget = None;
            return;
        }
        let Some(kind) = self.active_inline_widget() else {
            return;
        };
        let lines = self.inline_widget_styled_lines();
        self.capture_inline_widget_exit_snapshot(kind, lines);
    }

    pub(crate) fn capture_inline_widget_exit_snapshot(
        &mut self,
        kind: InlineWidgetKind,
        lines: Vec<SingleSessionStyledLine>,
    ) {
        if crate::animation::desktop_reduced_motion_enabled() {
            self.view.closing_inline_widget = None;
            return;
        }
        if lines.is_empty() {
            self.view.closing_inline_widget = None;
            return;
        }
        self.view.closing_inline_widget = Some(ClosingInlineWidgetState {
            kind,
            lines,
            started_at: Instant::now(),
        });
    }

    pub(crate) fn close_inline_widgets(&mut self) {
        self.capture_inline_widget_exit();
        self.show_help = false;
        self.show_session_info = false;
        self.model_picker.close();
        self.session_switcher.close();
        self.view.inline_widget_opened_at = None;
    }

    pub(crate) fn open_read_only_inline_widget(&mut self, kind: InlineWidgetKind) {
        self.close_inline_widgets();
        match kind {
            InlineWidgetKind::HotkeyHelp => self.show_help = true,
            InlineWidgetKind::SessionInfo => self.show_session_info = true,
            InlineWidgetKind::ModelPicker
            | InlineWidgetKind::SessionSwitcher
            | InlineWidgetKind::SlashSuggestions => {}
        }
        self.mark_inline_widget_opened();
    }

    pub(crate) fn toggle_read_only_inline_widget(&mut self, kind: InlineWidgetKind) -> KeyOutcome {
        let was_active = self.active_inline_widget() == Some(kind);
        self.close_inline_widgets();
        if !was_active {
            self.open_read_only_inline_widget(kind);
        }
        self.scroll_body_to_bottom();
        KeyOutcome::Redraw
    }

    pub(crate) fn inline_widget_reveal_in_progress(&self) -> bool {
        self.active_inline_widget().is_some() && self.inline_widget_reveal_progress() < 1.0
    }

    pub(crate) fn inline_widget_exit_in_progress(&self) -> bool {
        self.active_inline_widget().is_none() && self.render_inline_widget_reveal_progress() > 0.001
    }

    pub(crate) fn inline_widget_reveal_progress(&self) -> f32 {
        if self.active_inline_widget().is_none() {
            return 0.0;
        }
        if crate::animation::desktop_reduced_motion_enabled() {
            return 1.0;
        }

        #[cfg(test)]
        {
            1.0
        }

        #[cfg(not(test))]
        {
            let Some(opened_at) = self.view.inline_widget_opened_at else {
                return 1.0;
            };
            let raw = (opened_at.elapsed().as_secs_f32()
                / INLINE_WIDGET_REVEAL_DURATION.as_secs_f32())
            .clamp(0.0, 1.0);
            1.0 - (1.0 - raw).powi(3)
        }
    }

    pub(crate) fn render_inline_widget_kind(&self) -> Option<InlineWidgetKind> {
        self.active_inline_widget().or_else(|| {
            (self.render_inline_widget_reveal_progress() > 0.001)
                .then(|| {
                    self.view
                        .closing_inline_widget
                        .as_ref()
                        .map(|closing| closing.kind)
                })
                .flatten()
        })
    }

    pub(crate) fn render_inline_widget_styled_lines(&self) -> Vec<SingleSessionStyledLine> {
        if self.active_inline_widget().is_some() {
            return self.inline_widget_styled_lines();
        }
        if self.render_inline_widget_reveal_progress() <= 0.001 {
            return Vec::new();
        }
        self.view
            .closing_inline_widget
            .as_ref()
            .map(|closing| closing.lines.clone())
            .unwrap_or_default()
    }

    pub(crate) fn render_inline_widget_line_count(&self) -> usize {
        if let Some(kind) = self.active_inline_widget() {
            return self.active_inline_widget_line_count(kind);
        }
        if self.render_inline_widget_reveal_progress() <= 0.001 {
            return 0;
        }
        self.view
            .closing_inline_widget
            .as_ref()
            .map(|closing| closing.lines.len())
            .unwrap_or(0)
    }

    pub(crate) fn active_inline_widget_line_count(&self, kind: InlineWidgetKind) -> usize {
        match kind {
            InlineWidgetKind::HotkeyHelp => hotkey_help_inline_line_count(),
            InlineWidgetKind::ModelPicker => model_picker_inline_line_count(&self.model_picker),
            InlineWidgetKind::SessionSwitcher => {
                session_switcher_line_count(&self.session_switcher, self.current_session_id())
            }
            InlineWidgetKind::SessionInfo => session_info_inline_line_count(self),
            InlineWidgetKind::SlashSuggestions => self.slash_suggestion_line_count(),
        }
    }

    pub(crate) fn render_inline_widget_visible_line_count(&self) -> usize {
        let line_count = self.render_inline_widget_line_count();
        let limit = self
            .render_inline_widget_kind()
            .map(InlineWidgetKind::visible_line_limit)
            .unwrap_or(INLINE_WIDGET_DEFAULT_VISIBLE_LINE_LIMIT);
        line_count.min(limit)
    }

    pub(crate) fn render_inline_widget_reveal_progress(&self) -> f32 {
        if self.active_inline_widget().is_some() {
            return self.inline_widget_reveal_progress();
        }
        if crate::animation::desktop_reduced_motion_enabled() {
            return 0.0;
        }
        let Some(closing) = &self.view.closing_inline_widget else {
            return 0.0;
        };
        let raw = (closing.started_at.elapsed().as_secs_f32()
            / INLINE_WIDGET_EXIT_DURATION.as_secs_f32())
        .clamp(0.0, 1.0);
        1.0 - raw.powi(3)
    }

    pub(crate) fn open_model_picker(&mut self) -> KeyOutcome {
        let was_open = self.model_picker.open;
        self.close_inline_widgets();
        self.model_picker.open_loading();
        if !was_open {
            self.mark_inline_widget_opened();
        }
        self.set_status(SingleSessionStatus::LoadingModels);
        self.scroll_body_to_bottom();
        KeyOutcome::LoadModelCatalog
    }

    pub(crate) fn open_model_picker_preview(&mut self, filter: String) -> KeyOutcome {
        let was_open = self.model_picker.open;
        self.close_inline_widgets();
        self.model_picker.open_preview_loading(filter);
        if !was_open {
            self.mark_inline_widget_opened();
        }
        self.set_status(SingleSessionStatus::LoadingModels);
        self.scroll_body_to_bottom();
        KeyOutcome::LoadModelCatalog
    }

    pub(crate) fn open_session_switcher_preview(&mut self, filter: String) -> KeyOutcome {
        let was_open = self.session_switcher.open;
        self.close_inline_widgets();
        let current_session_id = self.current_session_id().map(str::to_string);
        self.session_switcher
            .open_preview_loading(current_session_id.as_deref(), filter);
        if !was_open {
            self.mark_inline_widget_opened();
        }
        self.set_status(SingleSessionStatus::LoadingRecentSessions);
        self.scroll_body_to_bottom();
        KeyOutcome::LoadSessionSwitcher
    }

    pub(crate) fn sync_model_picker_preview_from_draft(&mut self) -> Option<KeyOutcome> {
        let Some(filter) = model_picker_preview_filter(&self.draft) else {
            if self.model_picker.open && self.model_picker.preview {
                self.capture_inline_widget_exit();
                self.model_picker.close();
                return Some(KeyOutcome::Redraw);
            }
            return None;
        };

        if self.model_picker.open && self.model_picker.preview {
            self.model_picker.set_filter(filter);
            Some(KeyOutcome::Redraw)
        } else {
            Some(self.open_model_picker_preview(filter))
        }
    }

    pub(crate) fn sync_session_switcher_preview_from_draft(&mut self) -> Option<KeyOutcome> {
        if !self.pending_images.is_empty() {
            if self.session_switcher.open && self.session_switcher.preview {
                self.capture_inline_widget_exit();
                self.session_switcher.close();
                return Some(KeyOutcome::Redraw);
            }
            return None;
        }

        let Some(filter) = session_switcher_preview_filter(&self.draft) else {
            if self.session_switcher.open && self.session_switcher.preview {
                self.capture_inline_widget_exit();
                self.session_switcher.close();
                return Some(KeyOutcome::Redraw);
            }
            return None;
        };

        if self.session_switcher.open && self.session_switcher.preview {
            self.session_switcher.set_filter(filter);
            Some(KeyOutcome::Redraw)
        } else {
            Some(self.open_session_switcher_preview(filter))
        }
    }

    pub(crate) fn sync_inline_previews_from_draft(&mut self) -> Option<KeyOutcome> {
        self.sync_slash_suggestions_from_draft();
        self.sync_model_picker_preview_from_draft()
            .or_else(|| self.sync_session_switcher_preview_from_draft())
    }

    pub(crate) fn sync_slash_suggestions_from_draft(&mut self) {
        let was_visible = self.slash_suggestions_visible();
        let Some(query) = slash_suggestion_query(&self.draft, self.draft_cursor) else {
            if was_visible
                && self.active_inline_widget() == Some(InlineWidgetKind::SlashSuggestions)
            {
                self.capture_inline_widget_exit();
            }
            self.slash_suggestions.query.clear();
            self.slash_suggestions.selected = 0;
            return;
        };

        if self
            .slash_suggestions
            .dismissed_for_draft
            .as_deref()
            .is_some_and(|dismissed| dismissed != self.draft)
        {
            self.slash_suggestions.dismissed_for_draft = None;
        }

        let previous_slash_lines = (was_visible
            && self.active_inline_widget() == Some(InlineWidgetKind::SlashSuggestions))
        .then(|| self.inline_widget_styled_lines());
        if self.slash_suggestions.query != query {
            self.slash_suggestions.query = query;
            self.slash_suggestions.selected = 0;
        }
        let candidate_count = self.slash_suggestion_candidates().len();
        if candidate_count == 0 {
            if let Some(lines) = previous_slash_lines {
                self.capture_inline_widget_exit_snapshot(InlineWidgetKind::SlashSuggestions, lines);
            }
            self.slash_suggestions.selected = 0;
            return;
        }
        self.slash_suggestions.selected = self.slash_suggestions.selected.min(candidate_count - 1);
        if !was_visible {
            self.mark_inline_widget_opened();
            self.scroll_body_to_bottom();
        }
    }

    pub(crate) fn handle_slash_suggestion_key(&mut self, key: &KeyInput) -> Option<KeyOutcome> {
        match key {
            KeyInput::Escape => {
                self.capture_inline_widget_exit();
                self.slash_suggestions.dismissed_for_draft = Some(self.draft.clone());
                Some(KeyOutcome::Redraw)
            }
            KeyInput::ModelPickerMove(delta) => {
                self.move_slash_suggestion_selection(*delta);
                Some(KeyOutcome::Redraw)
            }
            KeyInput::ScrollBodyPages(pages) => {
                self.move_slash_suggestion_selection(if *pages > 0 { -5 } else { 5 });
                Some(KeyOutcome::Redraw)
            }
            KeyInput::Autocomplete => self.complete_selected_slash_suggestion(),
            KeyInput::SubmitDraft => {
                self.capture_inline_widget_exit();
                self.complete_selected_slash_suggestion();
                Some(self.submit_draft())
            }
            _ => None,
        }
    }

    pub(crate) fn move_slash_suggestion_selection(&mut self, delta: i32) {
        let count = self.slash_suggestion_candidates().len();
        if count == 0 {
            self.slash_suggestions.selected = 0;
            return;
        }
        let selected = self.slash_suggestions.selected as i32 + delta;
        self.slash_suggestions.selected =
            selected.clamp(0, count.saturating_sub(1) as i32) as usize;
    }

    pub(crate) fn complete_selected_slash_suggestion(&mut self) -> Option<KeyOutcome> {
        let candidates = self.slash_suggestion_candidates();
        let selected = self
            .slash_suggestions
            .selected
            .min(candidates.len().saturating_sub(1));
        let (usage, _) = candidates.get(selected).copied()?;
        let command = usage.split_whitespace().next().unwrap_or(usage);
        let (start, end) = slash_suggestion_prefix_bounds(&self.draft, self.draft_cursor)?;
        if self.draft.get(start..end) == Some(command) {
            return None;
        }
        self.remember_input_undo_state();
        self.draft.replace_range(start..end, command);
        self.draft_cursor = start + command.len();
        self.clear_draft_selection();
        self.slash_suggestions.dismissed_for_draft = None;
        self.slash_suggestions.query = command.to_string();
        self.slash_suggestions.selected = selected;
        Some(KeyOutcome::Redraw)
    }

    pub(crate) fn handle_model_picker_preview_key(&mut self, key: &KeyInput) -> Option<KeyOutcome> {
        match key {
            KeyInput::Character(text) if text == "j" => {
                self.model_picker.move_selection(1);
                Some(KeyOutcome::Redraw)
            }
            KeyInput::Character(text) if text == "k" => {
                self.model_picker.move_selection(-1);
                Some(KeyOutcome::Redraw)
            }
            KeyInput::Character(text) if text == "g" => {
                self.model_picker.select_first();
                Some(KeyOutcome::Redraw)
            }
            KeyInput::Character(text) if text == "G" => {
                self.model_picker.select_last();
                Some(KeyOutcome::Redraw)
            }
            KeyInput::Character(text) if text == "q" => {
                self.capture_inline_widget_exit();
                self.model_picker.close();
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                Some(KeyOutcome::Redraw)
            }
            KeyInput::Escape => {
                self.capture_inline_widget_exit();
                self.model_picker.close();
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                Some(KeyOutcome::Redraw)
            }
            KeyInput::ModelPickerMove(delta) => {
                self.model_picker.move_selection(*delta);
                Some(KeyOutcome::Redraw)
            }
            KeyInput::ScrollBodyPages(pages) => {
                self.model_picker
                    .move_selection(if *pages > 0 { -5 } else { 5 });
                Some(KeyOutcome::Redraw)
            }
            KeyInput::MoveToLineStart => {
                self.model_picker.select_first();
                Some(KeyOutcome::Redraw)
            }
            KeyInput::MoveToLineEnd => {
                self.model_picker.select_last();
                Some(KeyOutcome::Redraw)
            }
            KeyInput::SubmitDraft => {
                let Some(model) = self.model_picker.selected_model() else {
                    self.capture_inline_widget_exit();
                    self.model_picker.close();
                    self.draft.clear();
                    self.draft_cursor = 0;
                    self.composer.input_undo_stack.clear();
                    return Some(KeyOutcome::Redraw);
                };
                self.capture_inline_widget_exit();
                self.model_picker.close();
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                Some(KeyOutcome::SetModel(model))
            }
            KeyInput::RefreshSessions => {
                let filter = self.model_picker.filter.clone();
                self.model_picker.open_preview_loading(filter);
                self.set_status(SingleSessionStatus::LoadingModels);
                Some(KeyOutcome::LoadModelCatalog)
            }
            _ => None,
        }
    }

    pub(crate) fn handle_session_switcher_preview_key(
        &mut self,
        key: &KeyInput,
    ) -> Option<KeyOutcome> {
        match key {
            KeyInput::Character(text) if text == "j" => {
                if self.session_switcher.focus == SessionSwitcherPane::Preview {
                    self.session_switcher.scroll_preview(1);
                } else {
                    self.session_switcher.move_selection(1);
                }
                Some(KeyOutcome::Redraw)
            }
            KeyInput::Character(text) if text == "k" => {
                if self.session_switcher.focus == SessionSwitcherPane::Preview {
                    self.session_switcher.scroll_preview(-1);
                } else {
                    self.session_switcher.move_selection(-1);
                }
                Some(KeyOutcome::Redraw)
            }
            KeyInput::Character(text) if text == "h" => {
                self.session_switcher.focus_sessions();
                Some(KeyOutcome::Redraw)
            }
            KeyInput::Character(text) if text == "l" => {
                self.session_switcher.focus_preview();
                Some(KeyOutcome::Redraw)
            }
            KeyInput::Character(text) if text == "g" => {
                if self.session_switcher.focus == SessionSwitcherPane::Preview {
                    self.session_switcher.preview_scroll = 0;
                } else {
                    self.session_switcher.select_first();
                }
                Some(KeyOutcome::Redraw)
            }
            KeyInput::Character(text) if text == "G" => {
                if self.session_switcher.focus == SessionSwitcherPane::Preview {
                    self.session_switcher.preview_scroll =
                        self.session_switcher.preview_line_count().saturating_sub(1);
                } else {
                    self.session_switcher.select_last();
                }
                Some(KeyOutcome::Redraw)
            }
            KeyInput::Character(text) if text == "q" => {
                self.capture_inline_widget_exit();
                self.session_switcher.close();
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                Some(KeyOutcome::Redraw)
            }
            KeyInput::Escape => {
                self.capture_inline_widget_exit();
                self.session_switcher.close();
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                Some(KeyOutcome::Redraw)
            }
            KeyInput::ModelPickerMove(delta) => {
                if self.session_switcher.focus == SessionSwitcherPane::Preview {
                    self.session_switcher.scroll_preview(*delta);
                } else {
                    self.session_switcher.move_selection(*delta);
                }
                Some(KeyOutcome::Redraw)
            }
            KeyInput::ScrollBodyPages(pages) => {
                if self.session_switcher.focus == SessionSwitcherPane::Preview {
                    self.session_switcher
                        .scroll_preview(if *pages > 0 { -8 } else { 8 });
                } else {
                    self.session_switcher
                        .move_selection(if *pages > 0 { -5 } else { 5 });
                }
                Some(KeyOutcome::Redraw)
            }
            KeyInput::Autocomplete => {
                self.session_switcher.toggle_focus();
                Some(KeyOutcome::Redraw)
            }
            KeyInput::MoveCursorLeft => {
                self.session_switcher.focus_sessions();
                Some(KeyOutcome::Redraw)
            }
            KeyInput::MoveCursorRight => {
                self.session_switcher.focus_preview();
                Some(KeyOutcome::Redraw)
            }
            KeyInput::MoveToLineStart => {
                if self.session_switcher.focus == SessionSwitcherPane::Preview {
                    self.session_switcher.preview_scroll = 0;
                } else {
                    self.session_switcher.select_first();
                }
                Some(KeyOutcome::Redraw)
            }
            KeyInput::MoveToLineEnd => {
                if self.session_switcher.focus == SessionSwitcherPane::Preview {
                    self.session_switcher.preview_scroll =
                        self.session_switcher.preview_line_count().saturating_sub(1);
                } else {
                    self.session_switcher.select_last();
                }
                Some(KeyOutcome::Redraw)
            }
            KeyInput::SubmitDraft => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                Some(self.resume_selected_switcher_session())
            }
            KeyInput::QueueDraft => {
                let Some(session) = self.session_switcher.selected_session() else {
                    return Some(KeyOutcome::None);
                };
                self.capture_inline_widget_exit();
                self.session_switcher.close();
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                Some(KeyOutcome::OpenSession {
                    session_id: session.session_id,
                    title: session.title,
                })
            }
            KeyInput::RefreshSessions => {
                let current_session_id = self.current_session_id().map(str::to_string);
                let filter = self.session_switcher.filter.clone();
                self.session_switcher
                    .open_preview_loading(current_session_id.as_deref(), filter);
                self.set_status(SingleSessionStatus::LoadingRecentSessions);
                Some(KeyOutcome::LoadSessionSwitcher)
            }
            _ => None,
        }
    }

    pub(crate) fn open_session_switcher(&mut self) -> KeyOutcome {
        self.close_inline_widgets();
        let current_session_id = self.current_session_id().map(str::to_string);
        self.session_switcher
            .open_loading(current_session_id.as_deref());
        self.set_status(SingleSessionStatus::LoadingRecentSessions);
        self.scroll_body_to_bottom();
        self.mark_inline_widget_opened();
        KeyOutcome::LoadSessionSwitcher
    }

    pub(crate) fn handle_model_picker_key(&mut self, key: KeyInput) -> KeyOutcome {
        match key {
            KeyInput::Character(text) if text == "j" => {
                self.model_picker.move_selection(1);
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "k" => {
                self.model_picker.move_selection(-1);
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "h" => {
                self.model_picker.column = self.model_picker.column.saturating_sub(1);
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "l" => {
                self.model_picker.column = (self.model_picker.column + 1).min(2);
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "g" => {
                self.model_picker.select_first();
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "G" => {
                self.model_picker.select_last();
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "q" => {
                self.capture_inline_widget_exit();
                self.model_picker.close();
                KeyOutcome::Redraw
            }
            KeyInput::Escape if !self.model_picker.filter.is_empty() => {
                self.model_picker.set_filter(String::new());
                KeyOutcome::Redraw
            }
            KeyInput::Escape | KeyInput::OpenModelPicker => {
                self.capture_inline_widget_exit();
                self.model_picker.close();
                KeyOutcome::Redraw
            }
            KeyInput::OpenSessionSwitcher => self.open_session_switcher(),
            KeyInput::RefreshSessions => {
                self.model_picker.open_loading();
                self.set_status(SingleSessionStatus::LoadingModels);
                KeyOutcome::LoadModelCatalog
            }
            KeyInput::ModelPickerMove(delta) => {
                self.model_picker.move_selection(delta);
                KeyOutcome::Redraw
            }
            KeyInput::ScrollBodyPages(pages) => {
                self.model_picker
                    .move_selection(if pages > 0 { -5 } else { 5 });
                KeyOutcome::Redraw
            }
            KeyInput::MoveToLineStart => {
                self.model_picker.select_first();
                KeyOutcome::Redraw
            }
            KeyInput::MoveToLineEnd => {
                self.model_picker.select_last();
                KeyOutcome::Redraw
            }
            KeyInput::MoveCursorRight => {
                self.model_picker.column = (self.model_picker.column + 1).min(2);
                KeyOutcome::Redraw
            }
            KeyInput::MoveCursorLeft => {
                self.model_picker.column = self.model_picker.column.saturating_sub(1);
                KeyOutcome::Redraw
            }
            KeyInput::CycleModel(direction) => KeyOutcome::CycleModel(direction),
            KeyInput::SubmitDraft => {
                let Some(model) = self.model_picker.selected_model() else {
                    return KeyOutcome::None;
                };
                self.capture_inline_widget_exit();
                self.model_picker.close();
                KeyOutcome::SetModel(model)
            }
            KeyInput::Backspace => {
                self.model_picker.pop_filter_char();
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) => {
                self.model_picker.push_filter_text(&text);
                KeyOutcome::Redraw
            }
            KeyInput::HotkeyHelp => {
                self.open_read_only_inline_widget(InlineWidgetKind::HotkeyHelp);
                KeyOutcome::Redraw
            }
            _ => KeyOutcome::None,
        }
    }

    pub(crate) fn handle_session_switcher_key(&mut self, key: KeyInput) -> KeyOutcome {
        match key {
            KeyInput::Character(text) if text == "j" => {
                if self.session_switcher.focus == SessionSwitcherPane::Preview {
                    self.session_switcher.scroll_preview(1);
                } else {
                    self.session_switcher.move_selection(1);
                }
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "k" => {
                if self.session_switcher.focus == SessionSwitcherPane::Preview {
                    self.session_switcher.scroll_preview(-1);
                } else {
                    self.session_switcher.move_selection(-1);
                }
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "h" => {
                self.session_switcher.focus_sessions();
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "l" => {
                self.session_switcher.focus_preview();
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "g" => {
                if self.session_switcher.focus == SessionSwitcherPane::Preview {
                    self.session_switcher.preview_scroll = 0;
                } else {
                    self.session_switcher.select_first();
                }
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "G" => {
                if self.session_switcher.focus == SessionSwitcherPane::Preview {
                    self.session_switcher.preview_scroll =
                        self.session_switcher.preview_line_count().saturating_sub(1);
                } else {
                    self.session_switcher.select_last();
                }
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "q" => {
                self.capture_inline_widget_exit();
                self.session_switcher.close();
                KeyOutcome::Redraw
            }
            KeyInput::Escape | KeyInput::OpenSessionSwitcher => {
                self.capture_inline_widget_exit();
                self.session_switcher.close();
                KeyOutcome::Redraw
            }
            KeyInput::Autocomplete => {
                self.session_switcher.toggle_focus();
                KeyOutcome::Redraw
            }
            KeyInput::MoveCursorLeft => {
                self.session_switcher.focus_sessions();
                KeyOutcome::Redraw
            }
            KeyInput::MoveCursorRight => {
                self.session_switcher.focus_preview();
                KeyOutcome::Redraw
            }
            KeyInput::RefreshSessions => {
                let current_session_id = self.current_session_id().map(str::to_string);
                self.session_switcher
                    .refresh_loading(current_session_id.as_deref());
                self.set_status(SingleSessionStatus::LoadingRecentSessions);
                self.mark_inline_widget_opened();
                KeyOutcome::LoadSessionSwitcher
            }
            KeyInput::ModelPickerMove(delta) => {
                if self.session_switcher.focus == SessionSwitcherPane::Preview {
                    self.session_switcher.scroll_preview(delta);
                } else {
                    self.session_switcher.move_selection(delta);
                }
                KeyOutcome::Redraw
            }
            KeyInput::ScrollBodyPages(pages) => {
                if self.session_switcher.focus == SessionSwitcherPane::Preview {
                    self.session_switcher
                        .scroll_preview(if pages > 0 { -8 } else { 8 });
                } else {
                    self.session_switcher
                        .move_selection(if pages > 0 { -5 } else { 5 });
                }
                KeyOutcome::Redraw
            }
            KeyInput::MoveToLineStart => {
                if self.session_switcher.focus == SessionSwitcherPane::Preview {
                    self.session_switcher.preview_scroll = 0;
                } else {
                    self.session_switcher.select_first();
                }
                KeyOutcome::Redraw
            }
            KeyInput::MoveToLineEnd => {
                if self.session_switcher.focus == SessionSwitcherPane::Preview {
                    self.session_switcher.preview_scroll =
                        self.session_switcher.preview_line_count().saturating_sub(1);
                } else {
                    self.session_switcher.select_last();
                }
                KeyOutcome::Redraw
            }
            KeyInput::QueueDraft => {
                let Some(session) = self.session_switcher.selected_session() else {
                    return KeyOutcome::None;
                };
                self.capture_inline_widget_exit();
                self.session_switcher.close();
                KeyOutcome::OpenSession {
                    session_id: session.session_id,
                    title: session.title,
                }
            }
            KeyInput::SubmitDraft => self.resume_selected_switcher_session(),
            KeyInput::Backspace => {
                self.session_switcher.pop_filter_char();
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) => {
                self.session_switcher.push_filter_text(&text);
                KeyOutcome::Redraw
            }
            KeyInput::HotkeyHelp => {
                self.open_read_only_inline_widget(InlineWidgetKind::HotkeyHelp);
                KeyOutcome::Redraw
            }
            KeyInput::OpenModelPicker => self.open_model_picker(),
            KeyInput::SpawnPanel => {
                self.capture_inline_widget_exit();
                self.session_switcher.close();
                KeyOutcome::SpawnSession
            }
            KeyInput::SpawnSelfDevSession => {
                self.capture_inline_widget_exit();
                self.session_switcher.close();
                KeyOutcome::SpawnSelfDevSession
            }
            KeyInput::SpawnHomeSession => {
                self.capture_inline_widget_exit();
                self.session_switcher.close();
                KeyOutcome::SpawnHomeSession
            }
            _ => KeyOutcome::None,
        }
    }

    pub(crate) fn apply_session_switcher_cards(&mut self, cards: Vec<workspace::SessionCard>) {
        let current_session_id = self.current_session_id().map(str::to_string);
        self.session_switcher
            .apply_sessions(cards, current_session_id.as_deref());
        if self.session_switcher.open {
            self.set_status(SingleSessionStatus::Info(format!(
                "{} recent session(s)",
                self.session_switcher.sessions.len()
            )));
        }
    }

    pub(crate) fn resume_selected_switcher_session(&mut self) -> KeyOutcome {
        if self.is_processing {
            self.set_status(SingleSessionStatus::Info(
                "finish or Esc interrupt the running generation before switching sessions"
                    .to_string(),
            ));
            return KeyOutcome::Redraw;
        }

        let Some(session) = self.session_switcher.selected_session() else {
            return KeyOutcome::None;
        };
        let title = session.title.clone();
        let session_id = session.session_id.clone();
        self.session = Some(session);
        self.live_session_id = self
            .session
            .as_ref()
            .map(|session| session.session_id.clone());
        self.detail_scroll = 0;
        self.messages.clear();
        self.streaming_response.clear();
        self.reasoning.clear();
        self.error = None;
        self.stdin_response = None;
        self.body_scroll_lines = 0.0;
        self.show_help = false;
        self.welcome.timeline = false;
        self.session_switcher.close();
        // Card previews (if any) are applied synchronously above via
        // replace-session state; the full transcript can be large, so defer
        // the disk parse to the event loop instead of blocking this key.
        self.pending_transcript_hydration = Some(session_id.clone());
        self.set_status(SingleSessionStatus::Info(format!("resumed {title}")));
        KeyOutcome::Redraw
    }

    /// Take the session id queued for off-thread transcript hydration.
    pub(crate) fn take_pending_transcript_hydration(&mut self) -> Option<String> {
        self.pending_transcript_hydration.take()
    }

    /// Queue a transcript hydration to be serviced off the UI thread.
    pub(crate) fn request_transcript_hydration(&mut self, session_id: &str) {
        self.pending_transcript_hydration = Some(session_id.to_string());
    }

    /// Apply a transcript loaded off the UI thread, if it still matches the
    /// live session. Returns true when the transcript was applied.
    pub(crate) fn apply_hydrated_transcript(
        &mut self,
        session_id: &str,
        result: Result<Option<Vec<SessionTranscriptMessage>>, String>,
    ) -> bool {
        if self.live_session_id.as_deref() != Some(session_id) {
            return false;
        }
        match result {
            Ok(Some(messages)) if !messages.is_empty() => {
                self.apply_resumed_session_transcript(messages);
                true
            }
            Ok(_) => false,
            Err(error) => {
                crate::desktop_log::warn(format_args!(
                    "jcode-desktop: failed to hydrate resumed transcript for {session_id}: {error}"
                ));
                self.error = Some(format!("failed to load transcript: {error}"));
                false
            }
        }
    }

    pub(crate) fn handle_stdin_response_key(&mut self, key: KeyInput) -> KeyOutcome {
        match key {
            KeyInput::SubmitDraft | KeyInput::QueueDraft => {
                let Some(state) = self.stdin_response.take() else {
                    return KeyOutcome::None;
                };
                self.set_status(SingleSessionStatus::SendingInteractiveInput);
                KeyOutcome::SendStdinResponse {
                    request_id: state.request_id,
                    input: state.input,
                }
            }
            KeyInput::Enter => {
                if let Some(state) = &mut self.stdin_response {
                    state.input.push('\n');
                }
                KeyOutcome::Redraw
            }
            KeyInput::Backspace => {
                if let Some(state) = &mut self.stdin_response {
                    state.input.pop();
                }
                KeyOutcome::Redraw
            }
            KeyInput::DeleteToLineStart => {
                if let Some(state) = &mut self.stdin_response {
                    state.input.clear();
                }
                KeyOutcome::Redraw
            }
            KeyInput::PasteText => KeyOutcome::PasteText,
            KeyInput::Character(text) => {
                if let Some(state) = &mut self.stdin_response {
                    state.input.push_str(&text);
                }
                KeyOutcome::Redraw
            }
            KeyInput::CancelGeneration => KeyOutcome::CancelGeneration,
            KeyInput::Escape => {
                self.set_status(SingleSessionStatus::InteractiveInputPending);
                KeyOutcome::Redraw
            }
            _ => KeyOutcome::None,
        }
    }

    pub(crate) fn inline_widget_styled_lines(&self) -> Vec<SingleSessionStyledLine> {
        match self.active_inline_widget() {
            Some(InlineWidgetKind::HotkeyHelp) => hotkey_help_inline_widget().styled_lines(),
            Some(InlineWidgetKind::ModelPicker) => {
                model_picker_inline_styled_lines(&self.model_picker)
            }
            Some(InlineWidgetKind::SessionSwitcher) => {
                session_switcher_styled_lines(&self.session_switcher, self.current_session_id())
            }
            Some(InlineWidgetKind::SessionInfo) => session_info_inline_styled_lines(self),
            Some(InlineWidgetKind::SlashSuggestions) => self.slash_suggestion_styled_lines(),
            None => Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn inline_widget_line_count(&self) -> usize {
        self.inline_widget_styled_lines().len()
    }

    #[cfg(test)]
    pub(crate) fn inline_widget_visible_line_count(&self) -> usize {
        let line_count = self.inline_widget_line_count();
        let limit = self
            .active_inline_widget()
            .map(InlineWidgetKind::visible_line_limit)
            .unwrap_or(INLINE_WIDGET_DEFAULT_VISIBLE_LINE_LIMIT);
        line_count.min(limit)
    }

    pub(crate) fn slash_suggestions_visible(&self) -> bool {
        !self.slash_suggestion_candidates().is_empty()
    }

    pub(crate) fn slash_suggestion_styled_lines(&self) -> Vec<SingleSessionStyledLine> {
        let candidates = self.slash_suggestion_candidates();
        if candidates.is_empty() {
            return Vec::new();
        }

        let mut lines = vec![styled_line(
            "slash command suggestions",
            SingleSessionLineStyle::OverlayTitle,
        )];
        let selected = self
            .slash_suggestions
            .selected
            .min(candidates.len().saturating_sub(1));
        let usage_width = candidates
            .iter()
            .map(|(usage, _)| usage.chars().count())
            .max()
            .unwrap_or(0)
            .clamp(10, 20);
        lines.extend(
            candidates
                .into_iter()
                .enumerate()
                .map(|(index, (usage, description))| {
                    let style = if index == selected {
                        SingleSessionLineStyle::OverlaySelection
                    } else {
                        SingleSessionLineStyle::Overlay
                    };
                    styled_line(
                        format!(" {:<width$}  {}", usage, description, width = usage_width),
                        style,
                    )
                }),
        );
        lines
    }

    pub(crate) fn slash_suggestion_line_count(&self) -> usize {
        let candidate_count = self.slash_suggestion_candidate_count();
        if candidate_count == 0 {
            0
        } else {
            1 + candidate_count
        }
    }

    pub(crate) fn slash_suggestion_candidate_count(&self) -> usize {
        self.slash_suggestion_candidates().len()
    }

    pub(crate) fn slash_suggestion_candidates(&self) -> Vec<(&'static str, &'static str)> {
        if self
            .slash_suggestions
            .dismissed_for_draft
            .as_deref()
            .is_some_and(|draft| draft == self.draft)
        {
            return Vec::new();
        }
        let cursor = self.draft_cursor.min(self.draft.len());
        if !self.draft.is_char_boundary(cursor) {
            return Vec::new();
        }
        let prefix = self.draft[..cursor].trim_start();
        if !prefix.starts_with('/') || prefix.contains(char::is_whitespace) {
            return Vec::new();
        }
        let prefix = if self.slash_suggestions.query.is_empty() {
            prefix
        } else {
            self.slash_suggestions.query.as_str()
        };
        let prefix = prefix.to_ascii_lowercase();

        let mut prefix_matches = Vec::new();
        let mut fuzzy_matches: Vec<(i32, usize, &'static str, &'static str)> = Vec::new();
        for (usage, description) in DESKTOP_SLASH_COMMANDS.iter().copied() {
            let command = usage.split_whitespace().next().unwrap_or(usage);
            let command_lower = command.to_ascii_lowercase();
            if command_lower.starts_with(&prefix) {
                prefix_matches.push((usage, description));
            } else if let Some(score) = desktop_slash_fuzzy_score(&prefix, &command_lower) {
                fuzzy_matches.push((score, command.len(), usage, description));
            }
        }

        fuzzy_matches.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.cmp(b.2))
        });

        prefix_matches
            .into_iter()
            .chain(
                fuzzy_matches
                    .into_iter()
                    .map(|(_, _, usage, description)| (usage, description)),
            )
            .take(DESKTOP_SLASH_SUGGESTION_ROW_LIMIT)
            .collect()
    }

    pub(crate) fn active_inline_widget(&self) -> Option<InlineWidgetKind> {
        match self.active_overlay_state() {
            SingleSessionOverlay::Inline { kind, .. } => Some(kind),
            SingleSessionOverlay::None | SingleSessionOverlay::StdinResponse => None,
        }
    }

    pub(crate) fn active_inline_widget_mode(&self) -> Option<InlineWidgetMode> {
        match self.active_overlay_state() {
            SingleSessionOverlay::Inline { mode, .. } => Some(mode),
            SingleSessionOverlay::None | SingleSessionOverlay::StdinResponse => None,
        }
    }

    pub(crate) fn active_overlay_state(&self) -> SingleSessionOverlay {
        if self.stdin_response.is_some() {
            return SingleSessionOverlay::StdinResponse;
        }
        if self.session_switcher.open {
            return SingleSessionOverlay::Inline {
                kind: InlineWidgetKind::SessionSwitcher,
                mode: InlineWidgetKind::SessionSwitcher.mode(self),
            };
        }
        if self.model_picker.open {
            return SingleSessionOverlay::Inline {
                kind: InlineWidgetKind::ModelPicker,
                mode: InlineWidgetKind::ModelPicker.mode(self),
            };
        }
        if self.show_help {
            return SingleSessionOverlay::Inline {
                kind: InlineWidgetKind::HotkeyHelp,
                mode: InlineWidgetMode::ReadOnly,
            };
        }
        if self.show_session_info {
            return SingleSessionOverlay::Inline {
                kind: InlineWidgetKind::SessionInfo,
                mode: InlineWidgetMode::ReadOnly,
            };
        }
        if self.slash_suggestions_visible() {
            return SingleSessionOverlay::Inline {
                kind: InlineWidgetKind::SlashSuggestions,
                mode: InlineWidgetMode::ReadOnly,
            };
        }
        SingleSessionOverlay::None
    }

    #[cfg(test)]
    pub(crate) fn active_inline_widget_uses_card_chrome(&self) -> bool {
        self.active_inline_widget().is_some()
    }

    pub(crate) fn should_draw_composer_caret(&self) -> bool {
        !self.active_overlay_state().blocks_composer_caret()
    }
}
