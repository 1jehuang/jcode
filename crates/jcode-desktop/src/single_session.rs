use crate::{
    session_launch::{DesktopModelChoice, DesktopSessionEvent, DesktopSessionHandle},
    workspace,
};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use workspace::{KeyInput, KeyOutcome};

pub(crate) const SINGLE_SESSION_FONT_FAMILY: &str = "JetBrainsMono Nerd Font";
pub(crate) const SINGLE_SESSION_FONT_WEIGHT: &str = "Light";
pub(crate) const SINGLE_SESSION_FONT_FALLBACKS: &[&str] = &[
    "JetBrainsMono Nerd Font Mono",
    "JetBrains Mono",
    "monospace",
];
pub(crate) const SINGLE_SESSION_TITLE_FONT_SIZE: f32 = 28.0;
pub(crate) const SINGLE_SESSION_BODY_FONT_SIZE: f32 = 22.0;
pub(crate) const SINGLE_SESSION_META_FONT_SIZE: f32 = 16.0;
pub(crate) const SINGLE_SESSION_CODE_FONT_SIZE: f32 = 21.0;
pub(crate) const SINGLE_SESSION_BODY_LINE_HEIGHT: f32 = 1.45;
pub(crate) const SINGLE_SESSION_CODE_LINE_HEIGHT: f32 = 1.35;
pub(crate) const SINGLE_SESSION_META_LINE_HEIGHT: f32 = 1.25;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct SingleSessionTypography {
    pub(crate) family: &'static str,
    pub(crate) weight: &'static str,
    pub(crate) fallbacks: &'static [&'static str],
    pub(crate) title_size: f32,
    pub(crate) body_size: f32,
    pub(crate) meta_size: f32,
    pub(crate) code_size: f32,
    pub(crate) body_line_height: f32,
    pub(crate) code_line_height: f32,
    pub(crate) meta_line_height: f32,
}

pub(crate) const fn single_session_typography() -> SingleSessionTypography {
    SingleSessionTypography {
        family: SINGLE_SESSION_FONT_FAMILY,
        weight: SINGLE_SESSION_FONT_WEIGHT,
        fallbacks: SINGLE_SESSION_FONT_FALLBACKS,
        title_size: SINGLE_SESSION_TITLE_FONT_SIZE,
        body_size: SINGLE_SESSION_BODY_FONT_SIZE,
        meta_size: SINGLE_SESSION_META_FONT_SIZE,
        code_size: SINGLE_SESSION_CODE_FONT_SIZE,
        body_line_height: SINGLE_SESSION_BODY_LINE_HEIGHT,
        code_line_height: SINGLE_SESSION_CODE_LINE_HEIGHT,
        meta_line_height: SINGLE_SESSION_META_LINE_HEIGHT,
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SingleSessionApp {
    pub(crate) session: Option<workspace::SessionCard>,
    pub(crate) draft: String,
    pub(crate) draft_cursor: usize,
    pub(crate) detail_scroll: usize,
    pub(crate) live_session_id: Option<String>,
    pub(crate) messages: Vec<SingleSessionMessage>,
    pub(crate) streaming_response: String,
    pub(crate) status: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) is_processing: bool,
    pub(crate) body_scroll_lines: usize,
    pub(crate) show_help: bool,
    pub(crate) pending_images: Vec<(String, String)>,
    pub(crate) model_picker: ModelPickerState,
    pub(crate) session_switcher: SessionSwitcherState,
    pub(crate) stdin_response: Option<StdinResponseState>,
    queued_drafts: Vec<(String, Vec<(String, String)>)>,
    selection_anchor: Option<SelectionPoint>,
    selection_focus: Option<SelectionPoint>,
    input_undo_stack: Vec<(String, usize)>,
    session_handle: Option<DesktopSessionHandle>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SelectionPoint {
    pub(crate) line: usize,
    pub(crate) column: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SelectionLineSegment {
    pub(crate) line: usize,
    pub(crate) start_column: usize,
    pub(crate) end_column: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SingleSessionStyledLine {
    pub(crate) text: String,
    pub(crate) style: SingleSessionLineStyle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SingleSessionLineStyle {
    Assistant,
    AssistantHeading,
    AssistantQuote,
    AssistantTable,
    AssistantLink,
    Code,
    User,
    UserContinuation,
    Tool,
    Meta,
    Status,
    Error,
    OverlayTitle,
    Overlay,
    OverlaySelection,
    Blank,
}

impl SingleSessionStyledLine {
    fn new(text: impl Into<String>, style: SingleSessionLineStyle) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StdinResponseState {
    pub(crate) request_id: String,
    pub(crate) prompt: String,
    pub(crate) is_password: bool,
    pub(crate) tool_call_id: String,
    pub(crate) input: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelPickerState {
    pub(crate) open: bool,
    pub(crate) loading: bool,
    pub(crate) filter: String,
    pub(crate) selected: usize,
    pub(crate) current_model: Option<String>,
    pub(crate) provider_name: Option<String>,
    pub(crate) choices: Vec<DesktopModelChoice>,
    pub(crate) error: Option<String>,
}

impl Default for ModelPickerState {
    fn default() -> Self {
        Self {
            open: false,
            loading: false,
            filter: String::new(),
            selected: 0,
            current_model: None,
            provider_name: None,
            choices: Vec::new(),
            error: None,
        }
    }
}

impl ModelPickerState {
    fn open_loading(&mut self) {
        self.open = true;
        self.loading = true;
        self.error = None;
        self.selected = self.current_choice_index().unwrap_or(0);
    }

    fn close(&mut self) {
        self.open = false;
        self.loading = false;
        self.error = None;
    }

    fn apply_catalog(
        &mut self,
        current_model: Option<String>,
        provider_name: Option<String>,
        choices: Vec<DesktopModelChoice>,
    ) {
        if current_model.is_some() {
            self.current_model = current_model;
        }
        if provider_name.is_some() {
            self.provider_name = provider_name;
        }
        if !choices.is_empty() {
            self.choices = dedupe_model_choices(choices);
        }
        self.loading = false;
        self.error = None;
        self.ensure_current_choice_present();
        self.selected = self.current_visible_position().unwrap_or(0);
        self.clamp_selection();
    }

    fn apply_error(&mut self, error: String) {
        self.open = true;
        self.loading = false;
        self.error = Some(error);
    }

    fn apply_model_change(&mut self, model: String, provider_name: Option<String>) {
        self.current_model = Some(model);
        if provider_name.is_some() {
            self.provider_name = provider_name;
        }
        self.ensure_current_choice_present();
        self.selected = self.current_visible_position().unwrap_or(self.selected);
        self.clamp_selection();
    }

    fn selected_model(&self) -> Option<String> {
        let visible = self.filtered_indices();
        visible
            .get(self.selected)
            .and_then(|index| self.choices.get(*index))
            .map(|choice| choice.model.clone())
    }

    fn move_selection(&mut self, delta: i32) {
        let visible_len = self.filtered_indices().len();
        if visible_len == 0 {
            self.selected = 0;
            return;
        }
        if delta < 0 {
            self.selected = self.selected.saturating_sub(delta.unsigned_abs() as usize);
        } else {
            self.selected = (self.selected + delta as usize).min(visible_len - 1);
        }
    }

    fn push_filter_text(&mut self, text: &str) {
        self.filter.push_str(text);
        self.selected = 0;
    }

    fn pop_filter_char(&mut self) {
        self.filter.pop();
        self.selected = 0;
    }

    fn filtered_indices(&self) -> Vec<usize> {
        let query = self.filter.trim().to_lowercase();
        self.choices
            .iter()
            .enumerate()
            .filter_map(|(index, choice)| {
                if query.is_empty() || model_choice_search_text(choice).contains(&query) {
                    Some(index)
                } else {
                    None
                }
            })
            .collect()
    }

    fn current_choice_index(&self) -> Option<usize> {
        let current = self.current_model.as_deref()?;
        self.choices
            .iter()
            .position(|choice| choice.model == current)
    }

    fn current_visible_position(&self) -> Option<usize> {
        let current = self.current_choice_index()?;
        self.filtered_indices()
            .iter()
            .position(|index| *index == current)
    }

    fn clamp_selection(&mut self) {
        let visible_len = self.filtered_indices().len();
        if visible_len == 0 {
            self.selected = 0;
        } else if self.selected >= visible_len {
            self.selected = visible_len - 1;
        }
    }

    fn ensure_current_choice_present(&mut self) {
        let Some(current_model) = self.current_model.clone() else {
            return;
        };
        if self
            .choices
            .iter()
            .any(|choice| choice.model == current_model)
        {
            return;
        }
        self.choices.insert(
            0,
            DesktopModelChoice {
                model: current_model,
                provider: self.provider_name.clone(),
                detail: Some("current model".to_string()),
                available: true,
            },
        );
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub(crate) struct SessionSwitcherState {
    pub(crate) open: bool,
    pub(crate) loading: bool,
    pub(crate) filter: String,
    pub(crate) selected: usize,
    pub(crate) sessions: Vec<workspace::SessionCard>,
}

impl SessionSwitcherState {
    fn open_loading(&mut self, current_session_id: Option<&str>) {
        self.open = true;
        self.loading = true;
        self.selected = self
            .current_visible_position(current_session_id)
            .unwrap_or(self.selected);
        self.clamp_selection();
    }

    fn close(&mut self) {
        self.open = false;
        self.loading = false;
    }

    fn apply_sessions(
        &mut self,
        sessions: Vec<workspace::SessionCard>,
        current_session_id: Option<&str>,
    ) {
        self.sessions = sessions;
        self.loading = false;
        self.selected = self
            .current_visible_position(current_session_id)
            .unwrap_or(0);
        self.clamp_selection();
    }

    fn selected_session(&self) -> Option<workspace::SessionCard> {
        let visible = self.filtered_indices();
        visible
            .get(self.selected)
            .and_then(|index| self.sessions.get(*index))
            .cloned()
    }

    fn move_selection(&mut self, delta: i32) {
        let visible_len = self.filtered_indices().len();
        if visible_len == 0 {
            self.selected = 0;
            return;
        }
        if delta < 0 {
            self.selected = self.selected.saturating_sub(delta.unsigned_abs() as usize);
        } else {
            self.selected = (self.selected + delta as usize).min(visible_len - 1);
        }
    }

    fn push_filter_text(&mut self, text: &str) {
        self.filter.push_str(text);
        self.selected = 0;
    }

    fn pop_filter_char(&mut self) {
        self.filter.pop();
        self.selected = 0;
    }

    fn filtered_indices(&self) -> Vec<usize> {
        let query = self.filter.trim().to_lowercase();
        self.sessions
            .iter()
            .enumerate()
            .filter_map(|(index, session)| {
                if query.is_empty() || session_card_search_text(session).contains(&query) {
                    Some(index)
                } else {
                    None
                }
            })
            .collect()
    }

    fn current_visible_position(&self, current_session_id: Option<&str>) -> Option<usize> {
        let current_session_id = current_session_id?;
        self.filtered_indices().iter().position(|index| {
            self.sessions
                .get(*index)
                .is_some_and(|session| session.session_id == current_session_id)
        })
    }

    fn clamp_selection(&mut self) {
        let visible_len = self.filtered_indices().len();
        if visible_len == 0 {
            self.selected = 0;
        } else if self.selected >= visible_len {
            self.selected = visible_len - 1;
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SingleSessionMessage {
    role: SingleSessionRole,
    content: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(crate) enum SingleSessionRole {
    User,
    Assistant,
    Tool,
    System,
    Meta,
}

impl SingleSessionRole {
    pub(crate) fn is_user(self) -> bool {
        matches!(self, Self::User)
    }
}

impl SingleSessionMessage {
    pub(crate) fn user(content: impl Into<String>) -> Self {
        Self {
            role: SingleSessionRole::User,
            content: content.into(),
        }
    }

    pub(crate) fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: SingleSessionRole::Assistant,
            content: content.into(),
        }
    }

    pub(crate) fn tool(content: impl Into<String>) -> Self {
        Self {
            role: SingleSessionRole::Tool,
            content: content.into(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn system(content: impl Into<String>) -> Self {
        Self {
            role: SingleSessionRole::System,
            content: content.into(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn meta(content: impl Into<String>) -> Self {
        Self {
            role: SingleSessionRole::Meta,
            content: content.into(),
        }
    }
}

impl SingleSessionApp {
    pub(crate) fn new(session: Option<workspace::SessionCard>) -> Self {
        Self {
            session,
            draft: String::new(),
            draft_cursor: 0,
            detail_scroll: 0,
            live_session_id: None,
            messages: Vec::new(),
            streaming_response: String::new(),
            status: None,
            error: None,
            is_processing: false,
            body_scroll_lines: 0,
            show_help: false,
            pending_images: Vec::new(),
            model_picker: ModelPickerState::default(),
            session_switcher: SessionSwitcherState::default(),
            stdin_response: None,
            queued_drafts: Vec::new(),
            selection_anchor: None,
            selection_focus: None,
            input_undo_stack: Vec::new(),
            session_handle: None,
        }
    }

    pub(crate) fn replace_session(&mut self, session: Option<workspace::SessionCard>) {
        self.session = session;
        if let Some(session) = &self.session {
            self.live_session_id = Some(session.session_id.clone());
        }
        self.detail_scroll = 0;
    }

    pub(crate) fn reset_fresh_session(&mut self) {
        self.session = None;
        self.draft.clear();
        self.draft_cursor = 0;
        self.detail_scroll = 0;
        self.live_session_id = None;
        self.messages.clear();
        self.streaming_response.clear();
        self.status = None;
        self.error = None;
        self.is_processing = false;
        self.body_scroll_lines = 0;
        self.show_help = false;
        self.pending_images.clear();
        self.model_picker = ModelPickerState::default();
        self.session_switcher = SessionSwitcherState::default();
        self.stdin_response = None;
        self.queued_drafts.clear();
        self.clear_selection();
        self.input_undo_stack.clear();
        self.session_handle = None;
    }

    pub(crate) fn status_title(&self) -> String {
        let title = self.title();
        format!(
            "Jcode Desktop · single session · {title} · Enter send · Shift+Enter newline · Ctrl+Enter queue · Ctrl+P sessions · Ctrl+Shift+M models · Ctrl+; spawn · Esc quit · --workspace for Niri layout"
        )
    }

    pub(crate) fn title(&self) -> String {
        if let Some(session) = &self.session {
            session.title.clone()
        } else if let Some(session_id) = &self.live_session_id {
            format!("session {}", short_session_id(session_id))
        } else {
            "fresh session".to_string()
        }
    }

    pub(crate) fn header_title(&self) -> String {
        if self.should_show_session_title_header() {
            return self.title();
        }
        if self.is_processing || !self.streaming_response.is_empty() {
            "active conversation".to_string()
        } else if self.session.is_some() || self.live_session_id.is_some() {
            "conversation".to_string()
        } else {
            "fresh session".to_string()
        }
    }

    pub(crate) fn should_show_session_title_header(&self) -> bool {
        self.messages.is_empty()
            && self.streaming_response.is_empty()
            && self.error.is_none()
            && !self.model_picker.open
            && !self.session_switcher.open
            && self.stdin_response.is_none()
            && self.show_help == false
            && self.session.is_some()
    }

    pub(crate) fn has_background_work(&self) -> bool {
        self.is_processing
    }

    fn current_session_id(&self) -> Option<&str> {
        self.live_session_id.as_deref().or_else(|| {
            self.session
                .as_ref()
                .map(|session| session.session_id.as_str())
        })
    }

    pub(crate) fn user_turn_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|message| message.role.is_user())
            .count()
    }

    pub(crate) fn next_prompt_number(&self) -> usize {
        self.user_turn_count() + 1
    }

    pub(crate) fn composer_prompt(&self) -> String {
        format!("{}› ", self.next_prompt_number())
    }

    pub(crate) fn composer_text(&self) -> String {
        format!("{}{}", self.composer_prompt(), self.draft)
    }

    #[cfg(test)]
    pub(crate) fn composer_status_line(&self) -> String {
        self.composer_status_line_for_tick(0)
    }

    pub(crate) fn composer_status_line_for_tick(&self, tick: u64) -> String {
        let status = self.status.as_deref().unwrap_or("ready");
        let spinner = self
            .activity_spinner_for_tick(tick)
            .map(|spinner| format!("{spinner} "))
            .unwrap_or_default();
        let mode = if self.is_processing {
            "Ctrl+C interrupt"
        } else {
            "Enter send · Shift+Enter newline · Ctrl+Enter queue/send"
        };
        let scroll = match self.body_scroll_lines {
            0 => String::new(),
            1 => " · scrolled up 1 line".to_string(),
            lines => format!(" · scrolled up {lines} lines"),
        };
        let images = match self.pending_images.len() {
            0 => String::new(),
            1 => " · 1 image".to_string(),
            count => format!(" · {count} images"),
        };
        let queued = match self.queued_drafts.len() {
            0 => String::new(),
            1 => " · 1 queued".to_string(),
            count => format!(" · {count} queued"),
        };
        let stdin = self
            .stdin_response
            .as_ref()
            .map(|state| {
                if state.is_password {
                    " · password input requested".to_string()
                } else {
                    " · interactive input requested".to_string()
                }
            })
            .unwrap_or_default();
        let model = self
            .model_picker
            .current_model
            .as_ref()
            .map(|model| {
                self.model_picker
                    .provider_name
                    .as_deref()
                    .filter(|provider| !provider.is_empty())
                    .map(|provider| format!(" · model {provider}/{model}"))
                    .unwrap_or_else(|| format!(" · model {model}"))
            })
            .unwrap_or_default();
        format!("{spinner}{status}{images}{queued}{stdin}{model}{scroll} · {mode}")
    }

    #[cfg(test)]
    pub(crate) fn activity_spinner(&self) -> Option<&'static str> {
        self.activity_spinner_for_tick(0)
    }

    pub(crate) fn activity_spinner_for_tick(&self, tick: u64) -> Option<&'static str> {
        if self.is_processing
            || self.model_picker.loading
            || self.session_switcher.loading
            || self.status.as_deref().is_some_and(is_in_flight_status)
        {
            Some(match tick % 4 {
                0 => "◴",
                1 => "◷",
                2 => "◶",
                _ => "◵",
            })
        } else {
            None
        }
    }

    pub(crate) fn handle_key(&mut self, key: KeyInput) -> KeyOutcome {
        if self.stdin_response.is_some() {
            return self.handle_stdin_response_key(key);
        }

        if self.session_switcher.open {
            return self.handle_session_switcher_key(key);
        }

        if self.model_picker.open {
            return self.handle_model_picker_key(key);
        }

        match key {
            KeyInput::SpawnPanel => KeyOutcome::SpawnSession,
            KeyInput::OpenSessionSwitcher => self.open_session_switcher(),
            KeyInput::OpenModelPicker => self.open_model_picker(),
            KeyInput::HotkeyHelp => {
                self.show_help = !self.show_help;
                self.scroll_body_to_bottom();
                KeyOutcome::Redraw
            }
            KeyInput::RefreshSessions => KeyOutcome::Redraw,
            KeyInput::CancelGeneration => {
                if self.is_processing {
                    KeyOutcome::CancelGeneration
                } else {
                    KeyOutcome::None
                }
            }
            KeyInput::ScrollBodyPages(pages) => {
                self.scroll_body_lines(pages * 12);
                KeyOutcome::Redraw
            }
            KeyInput::JumpPrompt(direction) => {
                self.jump_prompt(direction);
                KeyOutcome::Redraw
            }
            KeyInput::CopyLatestResponse => self
                .latest_assistant_response()
                .map(KeyOutcome::CopyLatestResponse)
                .unwrap_or(KeyOutcome::None),
            KeyInput::ModelPickerMove(_) => KeyOutcome::None,
            KeyInput::CycleModel(direction) => KeyOutcome::CycleModel(direction),
            KeyInput::AttachClipboardImage => KeyOutcome::AttachClipboardImage,
            KeyInput::ClearAttachedImages => {
                if self.clear_attached_images() {
                    KeyOutcome::Redraw
                } else {
                    KeyOutcome::None
                }
            }
            KeyInput::PasteText => KeyOutcome::PasteText,
            KeyInput::QueueDraft if self.is_processing => self.queue_draft(),
            KeyInput::QueueDraft => self.submit_draft(),
            KeyInput::SubmitDraft => self.submit_draft(),
            KeyInput::Escape if self.show_help => {
                self.show_help = false;
                KeyOutcome::Redraw
            }
            KeyInput::Escape => KeyOutcome::Exit,
            KeyInput::Enter => {
                self.insert_draft_text("\n");
                KeyOutcome::Redraw
            }
            KeyInput::Backspace => {
                self.delete_previous_char();
                KeyOutcome::Redraw
            }
            KeyInput::DeletePreviousWord => {
                self.delete_previous_word();
                KeyOutcome::Redraw
            }
            KeyInput::DeleteNextWord => {
                self.delete_next_word();
                KeyOutcome::Redraw
            }
            KeyInput::DeleteNextChar => {
                self.delete_next_char();
                KeyOutcome::Redraw
            }
            KeyInput::MoveCursorWordLeft => {
                self.move_cursor_word_left();
                KeyOutcome::Redraw
            }
            KeyInput::MoveCursorWordRight => {
                self.move_cursor_word_right();
                KeyOutcome::Redraw
            }
            KeyInput::MoveCursorLeft => {
                self.move_cursor_left();
                KeyOutcome::Redraw
            }
            KeyInput::MoveCursorRight => {
                self.move_cursor_right();
                KeyOutcome::Redraw
            }
            KeyInput::MoveToLineStart => {
                self.move_to_line_start();
                KeyOutcome::Redraw
            }
            KeyInput::MoveToLineEnd => {
                self.move_to_line_end();
                KeyOutcome::Redraw
            }
            KeyInput::DeleteToLineStart => {
                self.delete_to_line_start();
                KeyOutcome::Redraw
            }
            KeyInput::DeleteToLineEnd => {
                self.delete_to_line_end();
                KeyOutcome::Redraw
            }
            KeyInput::UndoInput => {
                self.undo_input_change();
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) => {
                self.insert_draft_text(&text);
                KeyOutcome::Redraw
            }
            _ => KeyOutcome::None,
        }
    }

    fn open_model_picker(&mut self) -> KeyOutcome {
        self.show_help = false;
        self.session_switcher.close();
        self.model_picker.open_loading();
        self.status = Some("loading models".to_string());
        self.scroll_body_to_bottom();
        KeyOutcome::LoadModelCatalog
    }

    fn open_session_switcher(&mut self) -> KeyOutcome {
        self.show_help = false;
        self.model_picker.close();
        let current_session_id = self.current_session_id().map(str::to_string);
        self.session_switcher
            .open_loading(current_session_id.as_deref());
        self.status = Some("loading recent sessions".to_string());
        self.scroll_body_to_bottom();
        KeyOutcome::LoadSessionSwitcher
    }

    fn handle_model_picker_key(&mut self, key: KeyInput) -> KeyOutcome {
        match key {
            KeyInput::Escape | KeyInput::OpenModelPicker => {
                self.model_picker.close();
                KeyOutcome::Redraw
            }
            KeyInput::OpenSessionSwitcher => {
                self.model_picker.close();
                self.open_session_switcher()
            }
            KeyInput::RefreshSessions => {
                self.model_picker.open_loading();
                self.status = Some("loading models".to_string());
                KeyOutcome::LoadModelCatalog
            }
            KeyInput::ModelPickerMove(delta) => {
                self.model_picker.move_selection(delta);
                KeyOutcome::Redraw
            }
            KeyInput::CycleModel(direction) => KeyOutcome::CycleModel(direction),
            KeyInput::SubmitDraft => self
                .model_picker
                .selected_model()
                .map(KeyOutcome::SetModel)
                .unwrap_or(KeyOutcome::None),
            KeyInput::Backspace => {
                self.model_picker.pop_filter_char();
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) => {
                self.model_picker.push_filter_text(&text);
                KeyOutcome::Redraw
            }
            KeyInput::HotkeyHelp => {
                self.model_picker.close();
                self.show_help = true;
                KeyOutcome::Redraw
            }
            _ => KeyOutcome::None,
        }
    }

    fn handle_session_switcher_key(&mut self, key: KeyInput) -> KeyOutcome {
        match key {
            KeyInput::Escape | KeyInput::OpenSessionSwitcher => {
                self.session_switcher.close();
                KeyOutcome::Redraw
            }
            KeyInput::RefreshSessions => {
                let current_session_id = self.current_session_id().map(str::to_string);
                self.session_switcher
                    .open_loading(current_session_id.as_deref());
                self.status = Some("loading recent sessions".to_string());
                KeyOutcome::LoadSessionSwitcher
            }
            KeyInput::ModelPickerMove(delta) => {
                self.session_switcher.move_selection(delta);
                KeyOutcome::Redraw
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
                self.session_switcher.close();
                self.show_help = true;
                KeyOutcome::Redraw
            }
            KeyInput::OpenModelPicker => {
                self.session_switcher.close();
                self.open_model_picker()
            }
            KeyInput::SpawnPanel => {
                self.session_switcher.close();
                KeyOutcome::SpawnSession
            }
            _ => KeyOutcome::None,
        }
    }

    pub(crate) fn apply_session_switcher_cards(&mut self, cards: Vec<workspace::SessionCard>) {
        let current_session_id = self.current_session_id().map(str::to_string);
        self.session_switcher
            .apply_sessions(cards, current_session_id.as_deref());
        if self.session_switcher.open {
            self.status = Some(format!(
                "{} recent session(s)",
                self.session_switcher.sessions.len()
            ));
        }
    }

    fn resume_selected_switcher_session(&mut self) -> KeyOutcome {
        if self.is_processing {
            self.status = Some(
                "finish or Ctrl+C interrupt the running generation before switching sessions"
                    .to_string(),
            );
            return KeyOutcome::Redraw;
        }

        let Some(session) = self.session_switcher.selected_session() else {
            return KeyOutcome::None;
        };
        let title = session.title.clone();
        self.session = Some(session);
        self.live_session_id = self
            .session
            .as_ref()
            .map(|session| session.session_id.clone());
        self.detail_scroll = 0;
        self.messages.clear();
        self.streaming_response.clear();
        self.error = None;
        self.stdin_response = None;
        self.body_scroll_lines = 0;
        self.show_help = false;
        self.session_switcher.close();
        self.status = Some(format!("resumed {title}"));
        KeyOutcome::Redraw
    }

    fn handle_stdin_response_key(&mut self, key: KeyInput) -> KeyOutcome {
        match key {
            KeyInput::SubmitDraft | KeyInput::QueueDraft => {
                let Some(state) = self.stdin_response.take() else {
                    return KeyOutcome::None;
                };
                self.status = Some("sending interactive input".to_string());
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
                self.status = Some("interactive input pending · Ctrl+C to cancel".to_string());
                KeyOutcome::Redraw
            }
            _ => KeyOutcome::None,
        }
    }

    pub(crate) fn body_lines(&self) -> Vec<String> {
        self.body_styled_lines()
            .into_iter()
            .map(|line| line.text)
            .collect()
    }

    pub(crate) fn body_styled_lines(&self) -> Vec<SingleSessionStyledLine> {
        if let Some(stdin_response) = &self.stdin_response {
            return stdin_response_styled_lines(stdin_response);
        }
        if self.session_switcher.open {
            return session_switcher_styled_lines(
                &self.session_switcher,
                self.current_session_id(),
            );
        }
        if self.model_picker.open {
            return model_picker_styled_lines(&self.model_picker);
        }
        if self.show_help {
            return single_session_help_styled_lines();
        }
        if !self.messages.is_empty() || !self.streaming_response.is_empty() || self.error.is_some()
        {
            let mut lines = Vec::new();
            let mut user_turn = 1;
            for message in &self.messages {
                if !lines.is_empty() {
                    lines.push(blank_styled_line());
                }
                append_chat_message_lines(&mut lines, message, &mut user_turn);
            }
            if !self.streaming_response.is_empty() {
                if !lines.is_empty() {
                    lines.push(blank_styled_line());
                }
                append_assistant_lines(&mut lines, self.streaming_response.trim_end());
            }
            if let Some(error) = &self.error {
                if !lines.is_empty() {
                    lines.push(blank_styled_line());
                }
                lines.push(styled_line(
                    format!("error: {error}"),
                    SingleSessionLineStyle::Error,
                ));
            }
            return lines;
        }

        if let Some(status) = &self.status
            && self.session.is_none()
        {
            return vec![styled_line(status.clone(), SingleSessionLineStyle::Status)];
        }

        single_session_styled_lines(self.session.as_ref())
    }

    pub(crate) fn apply_session_event(&mut self, event: DesktopSessionEvent) {
        match event {
            DesktopSessionEvent::Status(status) => self.status = Some(status),
            DesktopSessionEvent::Reloading { .. } => {
                self.status = Some("server reloading, reconnecting".to_string());
                self.is_processing = true;
            }
            DesktopSessionEvent::SessionStarted { session_id } => {
                self.live_session_id = Some(session_id);
                self.status = Some("connected".to_string());
            }
            DesktopSessionEvent::TextDelta(text) => {
                self.streaming_response.push_str(&text);
                self.status = Some("receiving".to_string());
            }
            DesktopSessionEvent::TextReplace(text) => {
                self.streaming_response = text;
                self.status = Some("receiving".to_string());
            }
            DesktopSessionEvent::ToolStarted { name } => {
                self.status = Some(format!("using tool {name}"));
                self.messages
                    .push(SingleSessionMessage::tool(format!("{name} running")));
            }
            DesktopSessionEvent::ToolFinished {
                name,
                summary,
                is_error,
            } => {
                self.status = Some(if is_error {
                    format!("tool {name} failed")
                } else {
                    format!("tool {name} done")
                });
                let marker = if is_error { "failed" } else { "done" };
                self.messages.push(SingleSessionMessage::tool(format!(
                    "{name} {marker}: {summary}"
                )));
            }
            DesktopSessionEvent::ModelChanged {
                model,
                provider_name,
                error,
            } => {
                if let Some(error) = error {
                    self.status = Some("model switch failed".to_string());
                    self.model_picker.apply_error(error.clone());
                    self.messages.push(SingleSessionMessage::meta(format!(
                        "model switch failed: {error}"
                    )));
                    return;
                }
                let label = provider_name
                    .as_deref()
                    .filter(|provider| !provider.is_empty())
                    .map(|provider| format!("{provider} · {model}"))
                    .unwrap_or_else(|| model.clone());
                self.model_picker
                    .apply_model_change(model.clone(), provider_name.clone());
                self.status = Some(format!("model: {label}"));
                self.messages.push(SingleSessionMessage::meta(format!(
                    "model switched to {label}"
                )));
            }
            DesktopSessionEvent::ModelCatalog {
                current_model,
                provider_name,
                models,
            } => {
                self.model_picker
                    .apply_catalog(current_model, provider_name, models);
                self.status = Some("models loaded".to_string());
            }
            DesktopSessionEvent::ModelCatalogError { error } => {
                self.model_picker.apply_error(error.clone());
                self.status = Some("model picker error".to_string());
            }
            DesktopSessionEvent::StdinRequest {
                request_id,
                prompt,
                is_password,
                tool_call_id,
            } => {
                self.status = Some("interactive input requested".to_string());
                self.show_help = false;
                self.model_picker.close();
                let raw_prompt = prompt.trim();
                let display_prompt = if raw_prompt.is_empty() {
                    "interactive input requested"
                } else {
                    raw_prompt
                };
                self.stdin_response = Some(StdinResponseState {
                    request_id: request_id.clone(),
                    prompt: display_prompt.to_string(),
                    is_password,
                    tool_call_id: tool_call_id.clone(),
                    input: String::new(),
                });
                let sensitive = if is_password { " password" } else { "" };
                self.messages.push(SingleSessionMessage::meta(format!(
                    "interactive{sensitive} input requested by {tool_call_id} ({request_id}): {display_prompt}"
                )));
            }
            DesktopSessionEvent::Done => {
                self.finish_streaming_response();
                self.is_processing = false;
                self.stdin_response = None;
                self.session_handle = None;
                self.status = Some("ready".to_string());
            }
            DesktopSessionEvent::Error(error) => {
                self.finish_streaming_response();
                self.is_processing = false;
                self.stdin_response = None;
                self.session_handle = None;
                self.status = Some("error".to_string());
                self.error = Some(error);
            }
        }
    }

    pub(crate) fn set_session_handle(&mut self, handle: DesktopSessionHandle) {
        self.session_handle = Some(handle);
    }

    pub(crate) fn cancel_generation(&mut self) -> bool {
        let Some(handle) = &self.session_handle else {
            return false;
        };
        match handle.cancel() {
            Ok(()) => {
                self.stdin_response = None;
                self.status = Some("cancelling".to_string());
                true
            }
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                self.is_processing = false;
                self.stdin_response = None;
                self.session_handle = None;
                true
            }
        }
    }

    pub(crate) fn scroll_body_lines(&mut self, lines: i32) {
        if lines > 0 {
            self.body_scroll_lines = self.body_scroll_lines.saturating_add(lines as usize);
        } else {
            self.body_scroll_lines = self
                .body_scroll_lines
                .saturating_sub(lines.unsigned_abs() as usize);
        }
    }

    pub(crate) fn scroll_body_to_bottom(&mut self) {
        self.body_scroll_lines = 0;
    }

    pub(crate) fn latest_assistant_response(&self) -> Option<String> {
        if !self.streaming_response.trim().is_empty() {
            return Some(self.streaming_response.trim().to_string());
        }
        self.messages
            .iter()
            .rev()
            .find(|message| message.role == SingleSessionRole::Assistant)
            .map(|message| message.content.trim().to_string())
            .filter(|message| !message.is_empty())
    }

    pub(crate) fn jump_prompt(&mut self, direction: i32) {
        let lines = self.body_lines();
        let prompt_indices = lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| is_user_prompt_line(line).then_some(index))
            .collect::<Vec<_>>();
        if prompt_indices.is_empty() {
            return;
        }
        let current_line = lines
            .len()
            .saturating_sub(self.body_scroll_lines)
            .saturating_sub(1);
        let target = if direction < 0 {
            prompt_indices
                .iter()
                .rev()
                .copied()
                .find(|index| *index < current_line)
                .or_else(|| prompt_indices.first().copied())
        } else {
            let next = prompt_indices
                .iter()
                .copied()
                .find(|index| *index > current_line);
            if next.is_none() {
                self.scroll_body_to_bottom();
                return;
            }
            next
        };
        if let Some(target) = target {
            self.body_scroll_lines = lines.len().saturating_sub(target + 1);
        }
    }

    pub(crate) fn draft_cursor_line_col(&self) -> (usize, usize) {
        let before_cursor = &self.draft[..self.draft_cursor.min(self.draft.len())];
        let line = before_cursor.chars().filter(|ch| *ch == '\n').count();
        let column = before_cursor
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .chars()
            .count();
        (line, column)
    }

    pub(crate) fn draft_cursor_line_byte_index(&self) -> (usize, usize) {
        let cursor = self.draft_cursor.min(self.draft.len());
        let line = self.draft[..cursor]
            .chars()
            .filter(|ch| *ch == '\n')
            .count();
        let line_start = line_start(&self.draft, cursor);
        (line, cursor - line_start)
    }

    pub(crate) fn composer_cursor_line_byte_index(&self) -> (usize, usize) {
        let (line, index) = self.draft_cursor_line_byte_index();
        if line == 0 {
            (line, self.composer_prompt().len() + index)
        } else {
            (line, index)
        }
    }

    fn submit_draft(&mut self) -> KeyOutcome {
        let message = self.draft.trim().to_string();
        if message.is_empty() && self.pending_images.is_empty() {
            return KeyOutcome::None;
        }
        let images = std::mem::take(&mut self.pending_images);
        self.record_user_submit(&message);
        let Some(session) = &self.session else {
            return KeyOutcome::StartFreshSession { message, images };
        };
        let session_id = session.session_id.clone();
        let title = session.title.clone();
        KeyOutcome::SendDraft {
            session_id,
            title,
            message,
            images,
        }
    }

    pub(crate) fn attach_image(&mut self, media_type: String, base64_data: String) {
        self.pending_images.push((media_type, base64_data));
        self.status = Some(format!("attached {} image(s)", self.pending_images.len()));
    }

    pub(crate) fn clear_attached_images(&mut self) -> bool {
        if self.pending_images.is_empty() {
            return false;
        }
        self.pending_images.clear();
        self.status = Some("cleared image attachments".to_string());
        true
    }

    pub(crate) fn accepts_clipboard_image_paste(&self) -> bool {
        self.stdin_response.is_none() && !self.model_picker.open && !self.session_switcher.open
    }

    pub(crate) fn paste_text(&mut self, text: &str) {
        if !text.is_empty() {
            if let Some(stdin_response) = &mut self.stdin_response {
                stdin_response.input.push_str(text);
                return;
            }
            self.insert_draft_text(text);
        }
    }

    pub(crate) fn send_stdin_response(
        &mut self,
        request_id: String,
        input: String,
    ) -> anyhow::Result<()> {
        let Some(handle) = &self.session_handle else {
            anyhow::bail!("no active desktop session to receive interactive input");
        };
        handle.send_stdin_response(request_id, input)?;
        self.status = Some("interactive input sent".to_string());
        Ok(())
    }

    fn queue_draft(&mut self) -> KeyOutcome {
        let message = self.draft.trim().to_string();
        if message.is_empty() && self.pending_images.is_empty() {
            return KeyOutcome::None;
        }
        let images = std::mem::take(&mut self.pending_images);
        self.queued_drafts.push((message.clone(), images));
        self.messages.push(SingleSessionMessage::meta(format!(
            "queued prompt: {message}"
        )));
        self.draft.clear();
        self.draft_cursor = 0;
        self.input_undo_stack.clear();
        self.status = Some(format!("{} prompt(s) queued", self.queued_drafts.len()));
        KeyOutcome::Redraw
    }

    pub(crate) fn take_next_queued_draft(&mut self) -> Option<(String, Vec<(String, String)>)> {
        if self.is_processing || self.queued_drafts.is_empty() {
            return None;
        }
        let (message, images) = self.queued_drafts.remove(0);
        self.record_user_submit(&message);
        Some((message, images))
    }

    pub(crate) fn begin_selection(&mut self, point: SelectionPoint) {
        self.selection_anchor = Some(point);
        self.selection_focus = Some(point);
    }

    pub(crate) fn update_selection(&mut self, point: SelectionPoint) {
        if self.selection_anchor.is_some() {
            self.selection_focus = Some(point);
        }
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selection_anchor = None;
        self.selection_focus = None;
    }

    pub(crate) fn selection_points(&self) -> Option<(SelectionPoint, SelectionPoint)> {
        let anchor = self.selection_anchor?;
        let focus = self.selection_focus?;
        if selection_point_cmp(anchor, focus).is_gt() {
            Some((focus, anchor))
        } else {
            Some((anchor, focus))
        }
    }

    pub(crate) fn selection_segments(&self, lines: &[String]) -> Vec<SelectionLineSegment> {
        let Some((start, end)) = self.selection_points() else {
            return Vec::new();
        };
        if start == end || start.line >= lines.len() {
            return Vec::new();
        }

        let end_line = end.line.min(lines.len().saturating_sub(1));
        let mut segments = Vec::new();
        for line_index in start.line..=end_line {
            let line_len = lines[line_index].chars().count();
            let start_column = if line_index == start.line {
                start.column.min(line_len)
            } else {
                0
            };
            let end_column = if line_index == end_line {
                end.column.min(line_len)
            } else {
                line_len
            };
            if start_column != end_column || (start.line != end.line && line_len == 0) {
                segments.push(SelectionLineSegment {
                    line: line_index,
                    start_column,
                    end_column,
                });
            }
        }
        segments
    }

    pub(crate) fn selected_text_from_lines(&self, lines: &[String]) -> Option<String> {
        let (start, end) = self.selection_points()?;
        if start == end || start.line >= lines.len() {
            return None;
        }
        let end_line = end.line.min(lines.len().saturating_sub(1));
        let mut selected = Vec::new();
        for line_index in start.line..=end_line {
            let line = &lines[line_index];
            let line_len = line.chars().count();
            let start_column = if line_index == start.line {
                start.column.min(line_len)
            } else {
                0
            };
            let end_column = if line_index == end_line {
                end.column.min(line_len)
            } else {
                line_len
            };
            selected.push(slice_by_char_columns(line, start_column, end_column));
        }
        let text = selected.join("\n");
        (!text.is_empty()).then_some(text)
    }

    fn record_user_submit(&mut self, message: &str) {
        self.messages.push(SingleSessionMessage::user(message));
        self.draft.clear();
        self.draft_cursor = 0;
        self.input_undo_stack.clear();
        self.streaming_response.clear();
        self.scroll_body_to_bottom();
        self.status = Some("sending".to_string());
        self.error = None;
        self.is_processing = true;
    }

    fn finish_streaming_response(&mut self) {
        let response = self.streaming_response.trim().to_string();
        if !response.is_empty() {
            self.messages
                .push(SingleSessionMessage::assistant(response));
        }
        self.streaming_response.clear();
    }

    fn insert_draft_text(&mut self, text: &str) {
        if !text.is_empty() {
            self.remember_input_undo_state();
        }
        self.clamp_draft_cursor();
        self.draft.insert_str(self.draft_cursor, text);
        self.draft_cursor += text.len();
    }

    fn delete_previous_char(&mut self) {
        self.clamp_draft_cursor();
        if self.draft_cursor == 0 {
            return;
        }
        self.remember_input_undo_state();
        let previous = previous_char_boundary(&self.draft, self.draft_cursor);
        self.draft.replace_range(previous..self.draft_cursor, "");
        self.draft_cursor = previous;
    }

    fn delete_next_char(&mut self) {
        self.clamp_draft_cursor();
        if self.draft_cursor >= self.draft.len() {
            return;
        }
        self.remember_input_undo_state();
        let next = next_char_boundary(&self.draft, self.draft_cursor);
        self.draft.replace_range(self.draft_cursor..next, "");
    }

    fn delete_previous_word(&mut self) {
        self.clamp_draft_cursor();
        let start = previous_word_start(&self.draft, self.draft_cursor);
        if start < self.draft_cursor {
            self.remember_input_undo_state();
        }
        self.draft.replace_range(start..self.draft_cursor, "");
        self.draft_cursor = start;
    }

    fn delete_next_word(&mut self) {
        self.clamp_draft_cursor();
        let end = next_word_end(&self.draft, self.draft_cursor);
        if end > self.draft_cursor {
            self.remember_input_undo_state();
        }
        self.draft.replace_range(self.draft_cursor..end, "");
    }

    fn move_cursor_left(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = previous_char_boundary(&self.draft, self.draft_cursor);
    }

    fn move_cursor_right(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = next_char_boundary(&self.draft, self.draft_cursor);
    }

    fn move_cursor_word_left(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = previous_word_start(&self.draft, self.draft_cursor);
    }

    fn move_cursor_word_right(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = next_word_end(&self.draft, self.draft_cursor);
    }

    fn move_to_line_start(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = line_start(&self.draft, self.draft_cursor);
    }

    fn move_to_line_end(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = line_end(&self.draft, self.draft_cursor);
    }

    fn delete_to_line_start(&mut self) {
        self.clamp_draft_cursor();
        let start = line_start(&self.draft, self.draft_cursor);
        if start < self.draft_cursor {
            self.remember_input_undo_state();
        }
        self.draft.replace_range(start..self.draft_cursor, "");
        self.draft_cursor = start;
    }

    fn delete_to_line_end(&mut self) {
        self.clamp_draft_cursor();
        let end = line_end(&self.draft, self.draft_cursor);
        if end > self.draft_cursor {
            self.remember_input_undo_state();
        }
        self.draft.replace_range(self.draft_cursor..end, "");
    }

    fn remember_input_undo_state(&mut self) {
        if self
            .input_undo_stack
            .last()
            .is_some_and(|(draft, cursor)| draft == &self.draft && *cursor == self.draft_cursor)
        {
            return;
        }
        self.input_undo_stack
            .push((self.draft.clone(), self.draft_cursor));
        const MAX_UNDO: usize = 64;
        if self.input_undo_stack.len() > MAX_UNDO {
            self.input_undo_stack.remove(0);
        }
    }

    fn undo_input_change(&mut self) {
        if let Some((draft, cursor)) = self.input_undo_stack.pop() {
            self.draft = draft;
            self.draft_cursor = cursor.min(self.draft.len());
            self.clamp_draft_cursor();
        }
    }

    fn clamp_draft_cursor(&mut self) {
        self.draft_cursor = self.draft_cursor.min(self.draft.len());
        while !self.draft.is_char_boundary(self.draft_cursor) {
            self.draft_cursor -= 1;
        }
    }
}

fn styled_line(text: impl Into<String>, style: SingleSessionLineStyle) -> SingleSessionStyledLine {
    SingleSessionStyledLine::new(text, style)
}

fn is_in_flight_status(status: &str) -> bool {
    matches!(
        status,
        "loading models"
            | "loading recent sessions"
            | "receiving"
            | "connected"
            | "sending interactive input"
            | "switching model"
            | "cancelling"
    ) || status.starts_with("using tool ")
        || status.starts_with("attached ")
}

fn blank_styled_line() -> SingleSessionStyledLine {
    styled_line(String::new(), SingleSessionLineStyle::Blank)
}

fn stdin_response_styled_lines(state: &StdinResponseState) -> Vec<SingleSessionStyledLine> {
    let kind = if state.is_password {
        "interactive password input"
    } else {
        "interactive input"
    };
    let input = if state.is_password {
        "•".repeat(state.input.chars().count())
    } else if state.input.is_empty() {
        "<empty>".to_string()
    } else {
        state.input.replace(' ', "·")
    };
    vec![
        styled_line(
            format!("{kind} requested"),
            SingleSessionLineStyle::OverlayTitle,
        ),
        styled_line(
            format!("tool: {}", state.tool_call_id),
            SingleSessionLineStyle::Tool,
        ),
        styled_line(
            format!("request: {}", state.request_id),
            SingleSessionLineStyle::Meta,
        ),
        styled_line(
            format!("prompt: {}", state.prompt),
            SingleSessionLineStyle::Meta,
        ),
        blank_styled_line(),
        styled_line(
            format!("input: {input}"),
            SingleSessionLineStyle::OverlaySelection,
        ),
        blank_styled_line(),
        styled_line(
            "Enter send · Ctrl+Enter send · Shift+Enter newline · Ctrl+V paste · Ctrl+U clear · Ctrl+C cancel",
            SingleSessionLineStyle::Overlay,
        ),
    ]
}

fn selection_point_cmp(left: SelectionPoint, right: SelectionPoint) -> std::cmp::Ordering {
    left.line
        .cmp(&right.line)
        .then_with(|| left.column.cmp(&right.column))
}

fn slice_by_char_columns(line: &str, start_column: usize, end_column: usize) -> String {
    let start = byte_index_at_char_column(line, start_column);
    let end = byte_index_at_char_column(line, end_column.max(start_column));
    line.get(start..end).unwrap_or_default().to_string()
}

fn byte_index_at_char_column(line: &str, column: usize) -> usize {
    line.char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(line.len()))
        .nth(column)
        .unwrap_or(line.len())
}

fn session_switcher_styled_lines(
    switcher: &SessionSwitcherState,
    current_session_id: Option<&str>,
) -> Vec<SingleSessionStyledLine> {
    let mut lines = vec![
        styled_line(
            "desktop session switcher",
            SingleSessionLineStyle::OverlayTitle,
        ),
        styled_line(
            "↑/↓ select · type filter · Backspace edit filter · Enter resume · Ctrl+R reload · Ctrl+P/Esc close",
            SingleSessionLineStyle::Overlay,
        ),
        styled_line(
            format!(
                "filter: {}",
                if switcher.filter.is_empty() {
                    "<none>"
                } else {
                    switcher.filter.as_str()
                }
            ),
            SingleSessionLineStyle::Meta,
        ),
        blank_styled_line(),
    ];

    if switcher.loading {
        lines.push(styled_line(
            "loading recent sessions from ~/.jcode/sessions...",
            SingleSessionLineStyle::Status,
        ));
    }

    let visible = switcher.filtered_indices();
    if visible.is_empty() && !switcher.loading {
        let message = if switcher.sessions.is_empty() {
            "no recent sessions found"
        } else {
            "no matching sessions"
        };
        lines.push(styled_line(message, SingleSessionLineStyle::Status));
        lines.push(styled_line(
            "try clearing the filter, pressing Ctrl+R, or starting a fresh session with Ctrl+;",
            SingleSessionLineStyle::Overlay,
        ));
        return lines;
    }

    let limit = 28;
    for (position, index) in visible.iter().take(limit).enumerate() {
        let Some(session) = switcher.sessions.get(*index) else {
            continue;
        };
        let selector = if position == switcher.selected {
            "›"
        } else {
            " "
        };
        let current_marker = if Some(session.session_id.as_str()) == current_session_id {
            "✓"
        } else {
            " "
        };
        lines.push(styled_line(
            format!(
                "{selector} {current_marker} {}",
                session_card_display_line(session)
            ),
            if position == switcher.selected {
                SingleSessionLineStyle::OverlaySelection
            } else {
                SingleSessionLineStyle::Overlay
            },
        ));
    }
    if visible.len() > limit {
        lines.push(styled_line(
            format!("… {} more sessions", visible.len() - limit),
            SingleSessionLineStyle::Overlay,
        ));
    }

    lines
}

fn session_card_display_line(session: &workspace::SessionCard) -> String {
    let subtitle = if session.subtitle.is_empty() {
        String::new()
    } else {
        format!(" · {}", session.subtitle)
    };
    let detail = if session.detail.is_empty() {
        String::new()
    } else {
        format!(" · {}", session.detail)
    };
    format!("{}{}{}", session.title, subtitle, detail)
}

fn session_card_search_text(session: &workspace::SessionCard) -> String {
    let mut text = format!(
        "{} {} {} {}",
        session.session_id, session.title, session.subtitle, session.detail
    );
    for line in session
        .preview_lines
        .iter()
        .chain(session.detail_lines.iter())
    {
        text.push(' ');
        text.push_str(line);
    }
    text.to_lowercase()
}

fn model_picker_styled_lines(picker: &ModelPickerState) -> Vec<SingleSessionStyledLine> {
    let mut lines = vec![
        styled_line(
            "desktop model/account picker",
            SingleSessionLineStyle::OverlayTitle,
        ),
        styled_line(
            format!(
                "current: {}",
                model_picker_current_label(
                    picker.provider_name.as_deref(),
                    picker.current_model.as_deref(),
                )
            ),
            SingleSessionLineStyle::Meta,
        ),
        styled_line(
            "↑/↓ select · type filter · Backspace edit filter · Enter switch · Ctrl+R reload · Esc close",
            SingleSessionLineStyle::Overlay,
        ),
        styled_line(
            format!(
                "filter: {}",
                if picker.filter.is_empty() {
                    "<none>"
                } else {
                    picker.filter.as_str()
                }
            ),
            SingleSessionLineStyle::Meta,
        ),
        blank_styled_line(),
    ];

    if picker.loading {
        lines.push(styled_line(
            "loading models from shared server...",
            SingleSessionLineStyle::Status,
        ));
    }

    if let Some(error) = &picker.error {
        lines.push(styled_line(
            format!("error: {error}"),
            SingleSessionLineStyle::Error,
        ));
    }

    let visible = picker.filtered_indices();
    if visible.is_empty() && !picker.loading {
        lines.push(styled_line(
            "no matching models",
            SingleSessionLineStyle::Status,
        ));
        lines.push(styled_line(
            "try clearing the filter or pressing Ctrl+R to reload the catalog",
            SingleSessionLineStyle::Overlay,
        ));
        return lines;
    }

    let current = picker.current_model.as_deref();
    let limit = 28;
    for (position, index) in visible.iter().take(limit).enumerate() {
        let Some(choice) = picker.choices.get(*index) else {
            continue;
        };
        let selector = if position == picker.selected {
            "›"
        } else {
            " "
        };
        let current_marker = if Some(choice.model.as_str()) == current {
            "✓"
        } else {
            " "
        };
        lines.push(styled_line(
            format!(
                "{selector} {current_marker} {}",
                model_choice_display_line(choice)
            ),
            if position == picker.selected {
                SingleSessionLineStyle::OverlaySelection
            } else {
                SingleSessionLineStyle::Overlay
            },
        ));
    }
    if visible.len() > limit {
        lines.push(styled_line(
            format!("… {} more models", visible.len() - limit),
            SingleSessionLineStyle::Overlay,
        ));
    }

    lines
}

fn model_picker_current_label(provider_name: Option<&str>, current_model: Option<&str>) -> String {
    match (provider_name, current_model) {
        (Some(provider), Some(model)) if !provider.is_empty() => format!("{provider} · {model}"),
        (_, Some(model)) => model.to_string(),
        (Some(provider), None) if !provider.is_empty() => provider.to_string(),
        _ => "unknown".to_string(),
    }
}

fn model_choice_display_line(choice: &DesktopModelChoice) -> String {
    let provider = choice
        .provider
        .as_deref()
        .filter(|provider| !provider.is_empty())
        .map(|provider| format!(" · provider {provider}"))
        .unwrap_or_default();
    let availability = if choice.available {
        ""
    } else {
        " · unavailable"
    };
    let detail = choice
        .detail
        .as_deref()
        .filter(|detail| !detail.is_empty())
        .map(|detail| format!(" · {detail}"))
        .unwrap_or_default();
    format!("{}{provider}{availability}{detail}", choice.model)
}

fn model_choice_search_text(choice: &DesktopModelChoice) -> String {
    format!(
        "{} {} {}",
        choice.model,
        choice.provider.as_deref().unwrap_or_default(),
        choice.detail.as_deref().unwrap_or_default()
    )
    .to_lowercase()
}

fn dedupe_model_choices(choices: Vec<DesktopModelChoice>) -> Vec<DesktopModelChoice> {
    let mut deduped: Vec<DesktopModelChoice> = Vec::new();
    for choice in choices {
        if deduped.iter().any(|existing| {
            existing.model == choice.model
                && existing.provider == choice.provider
                && existing.detail == choice.detail
        }) {
            continue;
        }
        deduped.push(choice);
    }
    deduped
}

fn single_session_help_styled_lines() -> Vec<SingleSessionStyledLine> {
    [
        "desktop shortcuts",
        "",
        "chat",
        "  Enter       send prompt",
        "  Shift+Enter insert newline",
        "  Ctrl+Enter  queue while running, send when idle",
        "  Ctrl+C      interrupt running generation",
        "  Ctrl+Shift+C copy latest assistant response",
        "  Ctrl+V      paste clipboard text",
        "  Ctrl+V      paste clipboard image when no text is present",
        "  Ctrl+I      attach clipboard image to next prompt",
        "  Ctrl+Shift+I clear pending image attachments",
        "  Ctrl+Shift+M open model/account picker",
        "  Ctrl+M/N    switch to next/previous model",
        "  Ctrl+P/O    open recent session switcher",
        "",
        "navigation",
        "  PageUp/PageDown scroll transcript",
        "  Alt+Up/Down jump between user prompts",
        "  Mouse wheel scroll transcript",
        "",
        "editing",
        "  Ctrl+A/E    start/end of line",
        "  Ctrl+U/K    delete to line start/end",
        "  Ctrl+Backspace delete previous word",
        "  Alt+B/F     move by word",
        "  Alt+D       delete next word",
        "  Ctrl+Z      undo input edit",
        "",
        "window",
        "  Ctrl+;      reset/spawn fresh desktop session",
        "  Ctrl+R      reload sessions/models while a picker is open",
        "  Ctrl+?      toggle this help",
        "  Esc         close help or quit",
    ]
    .into_iter()
    .map(|line| {
        let style = if line.is_empty() {
            SingleSessionLineStyle::Blank
        } else if line.starts_with("  ") {
            SingleSessionLineStyle::Overlay
        } else {
            SingleSessionLineStyle::OverlayTitle
        };
        styled_line(line, style)
    })
    .collect()
}

fn append_chat_message_lines(
    lines: &mut Vec<SingleSessionStyledLine>,
    message: &SingleSessionMessage,
    user_turn: &mut usize,
) {
    match message.role {
        SingleSessionRole::User => {
            append_user_lines(lines, *user_turn, message.content.trim());
            *user_turn += 1;
        }
        SingleSessionRole::Assistant => append_assistant_lines(lines, message.content.trim()),
        SingleSessionRole::Tool => append_tool_lines(lines, message.content.trim()),
        SingleSessionRole::System | SingleSessionRole::Meta => {
            append_meta_lines(lines, message.content.trim())
        }
    }
}

fn append_user_lines(lines: &mut Vec<SingleSessionStyledLine>, turn: usize, content: &str) {
    let mut content_lines = content.lines();
    let Some(first) = content_lines.next() else {
        return;
    };
    lines.push(styled_line(
        format!("{turn}  {first}"),
        SingleSessionLineStyle::User,
    ));
    for line in content_lines {
        lines.push(styled_line(
            format!("   {line}"),
            SingleSessionLineStyle::UserContinuation,
        ));
    }
}

fn is_user_prompt_line(line: &str) -> bool {
    let Some((number, rest)) = line.split_once("  ") else {
        return false;
    };
    !number.is_empty() && number.chars().all(|ch| ch.is_ascii_digit()) && !rest.trim().is_empty()
}

fn append_assistant_lines(lines: &mut Vec<SingleSessionStyledLine>, content: &str) {
    lines.extend(render_assistant_markdown_lines(content));
}

fn render_assistant_markdown_lines(content: &str) -> Vec<SingleSessionStyledLine> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut list_stack = Vec::<Option<u64>>::new();
    let mut in_code_block = false;
    let mut in_quote = false;
    let mut in_table = false;
    let mut in_table_row = false;
    let mut table_cell_count = 0usize;
    let mut link_stack = Vec::<String>::new();
    let mut current_style = SingleSessionLineStyle::Assistant;

    let markdown_options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    for event in Parser::new_ext(content, markdown_options) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                flush_current_line(&mut lines, &mut current, current_style);
                current_style = SingleSessionLineStyle::AssistantHeading;
                current.push_str(heading_prefix(level));
            }
            Event::End(TagEnd::Heading(_)) => {
                flush_current_line(
                    &mut lines,
                    &mut current,
                    SingleSessionLineStyle::AssistantHeading,
                );
                current_style = if in_quote {
                    SingleSessionLineStyle::AssistantQuote
                } else {
                    SingleSessionLineStyle::Assistant
                };
            }
            Event::Start(Tag::BlockQuote(_)) => {
                flush_current_line(&mut lines, &mut current, current_style);
                in_quote = true;
                current_style = SingleSessionLineStyle::AssistantQuote;
                current.push_str("▌ ");
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                if current.trim() == "▌" {
                    current.clear();
                }
                flush_current_line(
                    &mut lines,
                    &mut current,
                    SingleSessionLineStyle::AssistantQuote,
                );
                in_quote = false;
                current_style = SingleSessionLineStyle::Assistant;
            }
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                flush_current_line(&mut lines, &mut current, current_style);
                if in_quote {
                    current.push_str("▌ ");
                }
            }
            Event::Start(Tag::List(start)) => list_stack.push(start),
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
                flush_current_line(&mut lines, &mut current, current_style);
            }
            Event::Start(Tag::Item) => {
                flush_current_line(&mut lines, &mut current, current_style);
                if in_quote {
                    current.push_str("▌ ");
                }
                if let Some(Some(next)) = list_stack.last_mut() {
                    current.push_str(&format!("{next}. "));
                    *next += 1;
                } else {
                    current.push_str("• ");
                }
            }
            Event::End(TagEnd::Item) => flush_current_line(&mut lines, &mut current, current_style),
            Event::Start(Tag::CodeBlock(kind)) => {
                flush_current_line(&mut lines, &mut current, current_style);
                let lang = match kind {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => format!(" {lang}"),
                    _ => String::new(),
                };
                lines.push(styled_line(
                    format!("```{lang}"),
                    SingleSessionLineStyle::Code,
                ));
                in_code_block = true;
            }
            Event::End(TagEnd::CodeBlock) => {
                flush_current_line(&mut lines, &mut current, SingleSessionLineStyle::Code);
                lines.push(styled_line("```", SingleSessionLineStyle::Code));
                in_code_block = false;
            }
            Event::Start(Tag::Table(_)) => {
                flush_current_line(&mut lines, &mut current, current_style);
                in_table = true;
                current_style = SingleSessionLineStyle::AssistantTable;
            }
            Event::End(TagEnd::Table) => {
                flush_current_line(
                    &mut lines,
                    &mut current,
                    SingleSessionLineStyle::AssistantTable,
                );
                in_table = false;
                current_style = if in_quote {
                    SingleSessionLineStyle::AssistantQuote
                } else {
                    SingleSessionLineStyle::Assistant
                };
            }
            Event::Start(Tag::TableHead) => {
                flush_current_line(
                    &mut lines,
                    &mut current,
                    SingleSessionLineStyle::AssistantTable,
                );
                in_table_row = true;
                table_cell_count = 0;
                current.push_str("┆ ");
            }
            Event::End(TagEnd::TableHead) => {
                flush_current_line(
                    &mut lines,
                    &mut current,
                    SingleSessionLineStyle::AssistantTable,
                );
                in_table_row = false;
                lines.push(styled_line("┆ ─", SingleSessionLineStyle::AssistantTable));
            }
            Event::Start(Tag::TableRow) => {
                flush_current_line(
                    &mut lines,
                    &mut current,
                    SingleSessionLineStyle::AssistantTable,
                );
                in_table_row = true;
                table_cell_count = 0;
                current.push_str("┆ ");
            }
            Event::End(TagEnd::TableRow) => {
                flush_current_line(
                    &mut lines,
                    &mut current,
                    SingleSessionLineStyle::AssistantTable,
                );
                in_table_row = false;
            }
            Event::Start(Tag::TableCell) => {
                if in_table && !in_table_row {
                    in_table_row = true;
                    table_cell_count = 0;
                    current.push_str("┆ ");
                }
                if in_table_row && table_cell_count > 0 {
                    current.push_str(" │ ");
                }
                table_cell_count += 1;
            }
            Event::End(TagEnd::TableCell) => {}
            Event::Start(Tag::Link { dest_url, .. }) => {
                link_stack.push(dest_url.to_string());
            }
            Event::End(TagEnd::Link) => {
                if let Some(dest_url) = link_stack.pop()
                    && !dest_url.is_empty()
                {
                    current.push_str(" ↗ ");
                    current.push_str(&dest_url);
                    current_style = SingleSessionLineStyle::AssistantLink;
                }
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                current.push_str("[image");
                if !dest_url.is_empty() {
                    current.push_str(" ↗ ");
                    current.push_str(&dest_url);
                }
                current.push(']');
            }
            Event::End(TagEnd::Image) => {}
            Event::Start(Tag::Emphasis) => current.push('_'),
            Event::End(TagEnd::Emphasis) => current.push('_'),
            Event::Start(Tag::Strong) => current.push_str("**"),
            Event::End(TagEnd::Strong) => current.push_str("**"),
            Event::Start(Tag::Strikethrough) => current.push('~'),
            Event::End(TagEnd::Strikethrough) => current.push('~'),
            Event::Text(text) => {
                if in_code_block {
                    for line in text.lines() {
                        lines.push(styled_line(
                            format!("    {line}"),
                            SingleSessionLineStyle::Code,
                        ));
                    }
                } else {
                    current.push_str(&text);
                }
            }
            Event::Code(code) => {
                current.push('`');
                current.push_str(&code);
                current.push('`');
            }
            Event::SoftBreak | Event::HardBreak => {
                flush_current_line(&mut lines, &mut current, current_style);
                if in_quote {
                    current.push_str("▌ ");
                }
            }
            Event::Rule => {
                flush_current_line(&mut lines, &mut current, current_style);
                lines.push(styled_line("───", SingleSessionLineStyle::Meta));
            }
            _ => {}
        }

        if !in_table && current_style == SingleSessionLineStyle::AssistantTable {
            current_style = SingleSessionLineStyle::Assistant;
        }
    }

    flush_current_line(&mut lines, &mut current, current_style);
    if lines.is_empty() && !content.trim().is_empty() {
        lines.extend(
            content
                .lines()
                .map(|line| styled_line(line, SingleSessionLineStyle::Assistant)),
        );
    }
    lines
}

fn flush_current_line(
    lines: &mut Vec<SingleSessionStyledLine>,
    current: &mut String,
    style: SingleSessionLineStyle,
) {
    let trimmed = current.trim_end();
    if !trimmed.is_empty() {
        lines.push(styled_line(trimmed, style));
    }
    current.clear();
}

fn heading_prefix(level: HeadingLevel) -> &'static str {
    match level {
        HeadingLevel::H1 => "# ",
        HeadingLevel::H2 => "## ",
        HeadingLevel::H3 => "### ",
        _ => "#### ",
    }
}

fn append_tool_lines(lines: &mut Vec<SingleSessionStyledLine>, content: &str) {
    if content.is_empty() {
        return;
    }
    lines.push(styled_line(
        format!("• {content}"),
        SingleSessionLineStyle::Tool,
    ));
}

fn append_meta_lines(lines: &mut Vec<SingleSessionStyledLine>, content: &str) {
    if content.is_empty() {
        return;
    }
    lines.push(styled_line(
        format!("  {content}"),
        SingleSessionLineStyle::Meta,
    ));
}

fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor.min(text.len())]
        .char_indices()
        .last()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }
    text[cursor..]
        .char_indices()
        .nth(1)
        .map(|(offset, _)| cursor + offset)
        .unwrap_or(text.len())
}

fn previous_word_start(text: &str, cursor: usize) -> usize {
    let mut start = cursor.min(text.len());
    while start > 0 {
        let previous = previous_char_boundary(text, start);
        let ch = text[previous..start].chars().next().unwrap_or_default();
        if !ch.is_whitespace() {
            break;
        }
        start = previous;
    }
    while start > 0 {
        let previous = previous_char_boundary(text, start);
        let ch = text[previous..start].chars().next().unwrap_or_default();
        if ch.is_whitespace() {
            break;
        }
        start = previous;
    }
    start
}

fn next_word_end(text: &str, cursor: usize) -> usize {
    let mut end = cursor.min(text.len());
    while end < text.len() {
        let next = next_char_boundary(text, end);
        let ch = text[end..next].chars().next().unwrap_or_default();
        if !ch.is_whitespace() {
            break;
        }
        end = next;
    }
    while end < text.len() {
        let next = next_char_boundary(text, end);
        let ch = text[end..next].chars().next().unwrap_or_default();
        if ch.is_whitespace() {
            break;
        }
        end = next;
    }
    end
}

fn line_start(text: &str, cursor: usize) -> usize {
    text[..cursor.min(text.len())]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0)
}

fn line_end(text: &str, cursor: usize) -> usize {
    text[cursor.min(text.len())..]
        .find('\n')
        .map(|offset| cursor + offset)
        .unwrap_or(text.len())
}

fn short_session_id(session_id: &str) -> &str {
    session_id
        .strip_prefix("session_")
        .and_then(|rest| rest.split('_').next())
        .filter(|name| !name.is_empty())
        .unwrap_or(session_id)
}

pub(crate) fn single_session_surface(
    session: Option<&workspace::SessionCard>,
) -> workspace::Surface {
    let lines = single_session_lines(session);
    workspace::Surface {
        id: 1,
        title: session
            .map(|session| session.title.clone())
            .unwrap_or_else(|| "new jcode session".to_string()),
        body_lines: lines.clone(),
        detail_lines: lines,
        session_id: session.map(|session| session.session_id.clone()),
        lane: 0,
        column: 0,
        color_index: 0,
    }
}

pub(crate) fn single_session_lines(session: Option<&workspace::SessionCard>) -> Vec<String> {
    single_session_styled_lines(session)
        .into_iter()
        .map(|line| line.text)
        .collect()
}

pub(crate) fn single_session_styled_lines(
    session: Option<&workspace::SessionCard>,
) -> Vec<SingleSessionStyledLine> {
    let Some(session) = session else {
        return vec![
            styled_line("single session mode", SingleSessionLineStyle::OverlayTitle),
            styled_line(
                "fresh desktop-native session draft",
                SingleSessionLineStyle::Status,
            ),
            styled_line(
                "type here without nav or insert modes",
                SingleSessionLineStyle::Overlay,
            ),
            styled_line(
                "Enter sends through the shared desktop session runtime",
                SingleSessionLineStyle::Overlay,
            ),
            styled_line(
                "ctrl+; clears this draft and starts another fresh desktop session",
                SingleSessionLineStyle::Overlay,
            ),
            styled_line(
                "run with --workspace for the niri layout wrapper",
                SingleSessionLineStyle::Overlay,
            ),
        ];
    };

    let mut lines = vec![
        styled_line("single session mode", SingleSessionLineStyle::OverlayTitle),
        styled_line(session.subtitle.clone(), SingleSessionLineStyle::Status),
        styled_line(session.detail.clone(), SingleSessionLineStyle::Meta),
    ];
    if !session.preview_lines.is_empty() {
        lines.push(styled_line(
            "recent transcript",
            SingleSessionLineStyle::OverlayTitle,
        ));
        lines.extend(
            session
                .preview_lines
                .iter()
                .cloned()
                .map(|line| styled_line(line, SingleSessionLineStyle::Assistant)),
        );
    }
    if !session.detail_lines.is_empty() {
        lines.push(styled_line(
            "expanded transcript",
            SingleSessionLineStyle::OverlayTitle,
        ));
        lines.extend(
            session
                .detail_lines
                .iter()
                .cloned()
                .map(|line| styled_line(line, SingleSessionLineStyle::Assistant)),
        );
    }
    lines
}
