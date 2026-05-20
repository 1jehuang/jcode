use crate::{
    desktop_rich_text,
    session_launch::{
        DesktopModelChoice, DesktopSessionEvent, DesktopSessionHandle, DesktopSessionStatus,
    },
    workspace,
};
use jcode_tui_messages::DisplayMessage;
use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};
use workspace::{KeyInput, KeyOutcome};

pub(crate) const SINGLE_SESSION_FONT_FAMILY: &str = "JetBrainsMono Nerd Font";
pub(crate) const SINGLE_SESSION_ASSISTANT_FONT_FAMILY: &str = SINGLE_SESSION_FONT_FAMILY;
pub(crate) const SINGLE_SESSION_WELCOME_FONT_FAMILY: &str = "Homemade Apple";
pub(crate) const SINGLE_SESSION_FONT_WEIGHT: &str = "Light";
pub(crate) const SINGLE_SESSION_FONT_FALLBACKS: &[&str] = &[
    "JetBrainsMono Nerd Font Mono",
    "JetBrains Mono",
    "monospace",
];
pub(crate) const SINGLE_SESSION_DEFAULT_FONT_SIZE: f32 = 22.0;
pub(crate) const SINGLE_SESSION_TITLE_FONT_SIZE: f32 = SINGLE_SESSION_DEFAULT_FONT_SIZE;
pub(crate) const SINGLE_SESSION_BODY_FONT_SIZE: f32 = SINGLE_SESSION_DEFAULT_FONT_SIZE * 1.55;
pub(crate) const SINGLE_SESSION_META_FONT_SIZE: f32 = SINGLE_SESSION_DEFAULT_FONT_SIZE;
pub(crate) const SINGLE_SESSION_CODE_FONT_SIZE: f32 = SINGLE_SESSION_BODY_FONT_SIZE;
pub(crate) const SINGLE_SESSION_BODY_LINE_HEIGHT: f32 = 1.45;
pub(crate) const SINGLE_SESSION_CODE_LINE_HEIGHT: f32 = 1.35;
pub(crate) const SINGLE_SESSION_META_LINE_HEIGHT: f32 = 1.25;
pub(crate) const SINGLE_SESSION_TEXT_SCALE_STEP: f32 = 0.10;
pub(crate) const SINGLE_SESSION_MIN_TEXT_SCALE: f32 = 0.65;
pub(crate) const SINGLE_SESSION_MAX_TEXT_SCALE: f32 = 1.35;
pub(crate) const HANDWRITTEN_WELCOME_PHRASES: &[&str] = &["Hello there"];

const DESKTOP_SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/help", "show desktop shortcuts and slash commands"),
    ("/?", "alias for /help"),
    ("/commands", "alias for /help"),
    ("/clear", "clear conversation history"),
    ("/new", "reset to a fresh desktop session"),
    ("/resume", "open the recent session switcher"),
    ("/sessions", "open the recent session switcher"),
    ("/model [name]", "open model picker or switch to a model"),
    ("/models", "alias for /model"),
    ("/refresh-model-list", "refresh provider model catalogs"),
    ("/effort [level]", "show or change reasoning effort"),
    ("/fast [on|off|status]", "show or toggle OpenAI fast mode"),
    ("/transport [mode]", "show or change OpenAI transport"),
    (
        "/compact [mode <mode>]",
        "compact context or set compaction mode",
    ),
    ("/rename <title|--clear>", "rename the current session"),
    (
        "/copy [latest|code|transcript]",
        "copy latest response, latest code block, or transcript",
    ),
    (
        "/search <query>",
        "count transcript matches and jump to the first one",
    ),
    ("/stop", "interrupt the running generation"),
    ("/status", "show current desktop session status"),
    ("/quit", "exit the desktop app"),
];
const DESKTOP_SLASH_SUGGESTION_ROW_LIMIT: usize = 7;

#[cfg_attr(test, allow(dead_code))]
const INLINE_WIDGET_REVEAL_DURATION: Duration = Duration::from_millis(180);
pub(crate) const MODEL_PICKER_INLINE_ROW_LIMIT: usize = 5;

const BODY_CACHE_TEXT_EDGE_BYTES: usize = 256;
const BODY_CACHE_MESSAGE_EDGE_COUNT: usize = 12;
const BODY_CACHE_MESSAGE_MIDDLE_SAMPLE_COUNT: usize = 8;

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

pub(crate) fn single_session_typography_for_scale(scale: f32) -> SingleSessionTypography {
    let base = single_session_typography();
    let scale = scale.clamp(SINGLE_SESSION_MIN_TEXT_SCALE, SINGLE_SESSION_MAX_TEXT_SCALE);
    SingleSessionTypography {
        title_size: base.title_size * scale,
        body_size: base.body_size * scale,
        meta_size: base.meta_size * scale,
        code_size: base.code_size * scale,
        ..base
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
    status_kind: Option<SingleSessionStatus>,
    pub(crate) error: Option<String>,
    pub(crate) is_processing: bool,
    pub(crate) body_scroll_lines: f32,
    pub(crate) show_help: bool,
    pub(crate) show_session_info: bool,
    pub(crate) pending_images: Vec<(String, String)>,
    pub(crate) model_picker: ModelPickerState,
    pub(crate) session_switcher: SessionSwitcherState,
    pub(crate) stdin_response: Option<StdinResponseState>,
    slash_suggestions: SlashSuggestionState,
    runtime_settings: SingleSessionRuntimeSettings,
    welcome: SingleSessionWelcomeState,
    composer: SingleSessionComposerState,
    selection: SingleSessionSelectionState,
    runtime: SingleSessionRuntimeState,
    tool: SingleSessionToolState,
    view: SingleSessionViewState,
}

#[derive(Clone, Debug)]
struct SingleSessionWelcomeState {
    name: Option<String>,
    recovery_session_count: usize,
    // True for the fresh-start chat that owns the welcome hero as visual UI.
    // The hero must stay out of `body_styled_lines()` so it never becomes part
    // of the persisted/rendered transcript text.
    timeline: bool,
    hero_phrase_index: usize,
}

impl SingleSessionWelcomeState {
    fn new(has_session: bool) -> Self {
        let name = desktop_welcome_name();
        let hero_phrase_index = welcome_phrase_index(&name);
        Self {
            name,
            recovery_session_count: 0,
            timeline: !has_session,
            hero_phrase_index,
        }
    }

    fn reset_fresh(&mut self) {
        *self = Self::new(false);
    }
}

#[derive(Clone, Debug, Default)]
struct SingleSessionComposerState {
    queued_drafts: Vec<(String, Vec<(String, String)>)>,
    input_undo_stack: Vec<(String, usize)>,
}

#[derive(Clone, Debug, Default)]
struct SlashSuggestionState {
    selected: usize,
    query: String,
    dismissed_for_draft: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct SingleSessionSelectionState {
    anchor: Option<SelectionPoint>,
    focus: Option<SelectionPoint>,
    draft_anchor: Option<SelectionPoint>,
    draft_focus: Option<SelectionPoint>,
}

#[derive(Clone, Debug)]
struct SingleSessionRuntimeState {
    session_handle: Option<DesktopSessionHandle>,
    reload_phase: ReloadPhase,
}

impl Default for SingleSessionRuntimeState {
    fn default() -> Self {
        Self {
            session_handle: None,
            reload_phase: ReloadPhase::Stable,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct SingleSessionRuntimeSettings {
    reasoning_effort: Option<String>,
    service_tier: Option<String>,
    transport: Option<String>,
    compaction_mode: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct SingleSessionToolState {
    active_message_index: Option<usize>,
    input_buffer: String,
}

#[derive(Clone, Debug)]
struct SingleSessionViewState {
    inline_widget_opened_at: Option<Instant>,
    text_scale: f32,
}

impl Default for SingleSessionViewState {
    fn default() -> Self {
        Self {
            inline_widget_opened_at: None,
            text_scale: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReloadPhase {
    Stable,
    AwaitingReconnect,
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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SingleSessionStyledLine {
    pub(crate) text: String,
    pub(crate) style: SingleSessionLineStyle,
    pub(crate) inline_spans: Vec<SingleSessionInlineSpan>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SingleSessionInlineSpan {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) kind: SingleSessionInlineSpanKind,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum SingleSessionInlineSpanKind {
    Code,
    Math,
    Strong,
    Emphasis,
    Strike,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReadOnlyInlineWidget {
    pub(crate) title: String,
    pub(crate) lines: Vec<SingleSessionStyledLine>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InlineWidgetMode {
    ReadOnly,
    Interactive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InlineWidgetKind {
    HotkeyHelp,
    SessionInfo,
    ModelPicker,
    SessionSwitcher,
    SlashSuggestions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SingleSessionOverlay {
    None,
    StdinResponse,
    Inline {
        kind: InlineWidgetKind,
        mode: InlineWidgetMode,
    },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum SingleSessionStatus {
    LoadingModels,
    LoadingRecentSessions,
    Receiving,
    Connected,
    SendingInteractiveInput,
    Cancelling,
    ServerReloading,
    ServerReconnected,
    InteractiveInputRequested,
    InteractiveInputPending,
    Ready,
    Sending,
    Error,
    ModelsLoaded,
    ModelPickerError,
    ModelSwitchFailed,
    ModelSelected(String),
    ToolPreparing(String),
    ToolUsing(String),
    ToolFinished { name: String, is_error: bool },
    AttachedImages(usize),
    Info(String),
    Backend(DesktopSessionStatus),
}

impl SingleSessionStatus {
    fn label(&self) -> String {
        match self {
            Self::LoadingModels => "loading models".to_string(),
            Self::LoadingRecentSessions => "loading recent sessions".to_string(),
            Self::Receiving => "receiving".to_string(),
            Self::Connected => "connected".to_string(),
            Self::SendingInteractiveInput => "sending interactive input".to_string(),
            Self::Cancelling => "cancelling".to_string(),
            Self::ServerReloading => "server reloading, reconnecting".to_string(),
            Self::ServerReconnected => "server reconnected".to_string(),
            Self::InteractiveInputRequested => "interactive input requested".to_string(),
            Self::InteractiveInputPending => {
                "interactive input pending · Esc to cancel".to_string()
            }
            Self::Ready => "ready".to_string(),
            Self::Sending => "sending".to_string(),
            Self::Error => "error".to_string(),
            Self::ModelsLoaded => "models loaded".to_string(),
            Self::ModelPickerError => "model picker error".to_string(),
            Self::ModelSwitchFailed => "model switch failed".to_string(),
            Self::ModelSelected(label) => format!("model: {label}"),
            Self::ToolPreparing(name) => format!("preparing tool {name}"),
            Self::ToolUsing(name) => format!("using tool {name}"),
            Self::ToolFinished { name, is_error } => {
                format!("tool {name} {}", if *is_error { "failed" } else { "done" })
            }
            Self::AttachedImages(count) => format!("attached {count} image(s)"),
            Self::Info(label) => label.clone(),
            Self::Backend(status) => status.label(),
        }
    }

    fn is_in_flight(&self) -> bool {
        match self {
            Self::LoadingModels
            | Self::LoadingRecentSessions
            | Self::Receiving
            | Self::Connected
            | Self::SendingInteractiveInput
            | Self::Cancelling
            | Self::Sending
            | Self::ToolPreparing(_)
            | Self::ToolUsing(_)
            | Self::AttachedImages(_) => true,
            Self::Backend(status) => status.is_in_flight(),
            Self::ServerReloading
            | Self::ServerReconnected
            | Self::InteractiveInputRequested
            | Self::InteractiveInputPending
            | Self::Ready
            | Self::Error
            | Self::ModelsLoaded
            | Self::ModelPickerError
            | Self::ModelSwitchFailed
            | Self::ModelSelected(_)
            | Self::ToolFinished { .. }
            | Self::Info(_) => false,
        }
    }
}

impl SingleSessionOverlay {
    pub(crate) fn blocks_composer_caret(self) -> bool {
        match self {
            Self::None => false,
            Self::StdinResponse => true,
            Self::Inline {
                kind: InlineWidgetKind::ModelPicker,
                mode: InlineWidgetMode::ReadOnly,
            } => false,
            Self::Inline {
                kind: InlineWidgetKind::SlashSuggestions,
                mode: InlineWidgetMode::ReadOnly,
            } => false,
            Self::Inline { .. } => true,
        }
    }
}

impl InlineWidgetKind {
    pub(crate) fn mode(self, app: &SingleSessionApp) -> InlineWidgetMode {
        match self {
            Self::HotkeyHelp | Self::SessionInfo | Self::SlashSuggestions => {
                InlineWidgetMode::ReadOnly
            }
            Self::ModelPicker if app.model_picker.preview => InlineWidgetMode::ReadOnly,
            Self::ModelPicker => InlineWidgetMode::Interactive,
            Self::SessionSwitcher => InlineWidgetMode::Interactive,
        }
    }
}

impl ReadOnlyInlineWidget {
    fn new(title: impl Into<String>, lines: Vec<SingleSessionStyledLine>) -> Self {
        Self {
            title: title.into(),
            lines,
        }
    }

    fn styled_lines(self) -> Vec<SingleSessionStyledLine> {
        let mut styled = Vec::with_capacity(self.lines.len().saturating_add(2));
        styled.push(styled_line(
            self.title,
            SingleSessionLineStyle::OverlayTitle,
        ));
        if !self.lines.is_empty() {
            styled.push(blank_styled_line());
            styled.extend(self.lines);
        }
        styled
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) enum SingleSessionLineStyle {
    #[default]
    Assistant,
    AssistantHeading,
    AssistantQuote,
    AssistantTable,
    AssistantLink,
    CodeHeader,
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
    pub(crate) fn new(text: impl Into<String>, style: SingleSessionLineStyle) -> Self {
        Self {
            text: text.into(),
            style,
            inline_spans: Vec::new(),
        }
    }

    pub(crate) fn with_inline_spans(
        text: impl Into<String>,
        style: SingleSessionLineStyle,
        inline_spans: Vec<SingleSessionInlineSpan>,
    ) -> Self {
        Self {
            text: text.into(),
            style,
            inline_spans,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct StdinResponseState {
    pub(crate) request_id: String,
    pub(crate) prompt: String,
    pub(crate) is_password: bool,
    pub(crate) tool_call_id: String,
    pub(crate) input: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ModelPickerState {
    pub(crate) open: bool,
    pub(crate) loading: bool,
    pub(crate) preview: bool,
    pub(crate) filter: String,
    pub(crate) selected: usize,
    pub(crate) column: usize,
    pub(crate) current_model: Option<String>,
    pub(crate) provider_name: Option<String>,
    pub(crate) choices: Vec<DesktopModelChoice>,
    pub(crate) error: Option<String>,
}

impl ModelPickerState {
    fn open_loading(&mut self) {
        self.open = true;
        self.loading = true;
        self.preview = false;
        self.error = None;
        self.selected = self.current_choice_index().unwrap_or(0);
        self.column = 0;
    }

    fn open_preview_loading(&mut self, filter: String) {
        self.open = true;
        self.loading = true;
        self.preview = true;
        self.filter = filter;
        self.error = None;
        self.selected = self.current_visible_position().unwrap_or(0);
        self.column = 0;
    }

    fn close(&mut self) {
        self.open = false;
        self.loading = false;
        self.preview = false;
        self.error = None;
        self.column = 0;
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
        self.column = self.column.min(2);
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

    fn select_first(&mut self) {
        self.selected = 0;
    }

    fn select_last(&mut self) {
        self.selected = self.filtered_indices().len().saturating_sub(1);
    }

    fn push_filter_text(&mut self, text: &str) {
        self.filter.push_str(text);
        self.selected = 0;
        self.column = 0;
    }

    fn pop_filter_char(&mut self) {
        self.filter.pop();
        self.selected = 0;
        self.column = 0;
    }

    fn set_filter(&mut self, filter: String) {
        if self.filter != filter {
            self.filter = filter;
            self.selected = 0;
            self.column = 0;
        }
        self.clamp_selection();
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

    pub(crate) fn visible_row_window(&self, limit: usize) -> (usize, Vec<usize>) {
        let visible = self.filtered_indices();
        if visible.is_empty() || limit == 0 {
            return (0, Vec::new());
        }
        let max_start = visible.len().saturating_sub(limit);
        let selected = self.selected.min(visible.len() - 1);
        let start = selected.saturating_sub(limit / 2).min(max_start);
        let end = (start + limit).min(visible.len());
        (start, visible[start..end].to_vec())
    }

    pub(crate) fn selected_row_in_window(&self, limit: usize) -> Option<usize> {
        let (start, visible) = self.visible_row_window(limit);
        if visible.is_empty() {
            None
        } else {
            Some(self.selected.saturating_sub(start).min(visible.len() - 1))
        }
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
                api_method: Some("current".to_string()),
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

    fn select_first(&mut self) {
        self.selected = 0;
    }

    fn select_last(&mut self) {
        self.selected = self.filtered_indices().len().saturating_sub(1);
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

#[derive(Clone, Debug)]
pub(crate) struct SingleSessionMessage {
    display: DisplayMessage,
    rich_attachments: Vec<desktop_rich_text::RichAttachment>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
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

fn rich_role_from_single_session_role(
    role: SingleSessionRole,
) -> desktop_rich_text::TranscriptRole {
    match role {
        SingleSessionRole::User => desktop_rich_text::TranscriptRole::User,
        SingleSessionRole::Assistant => desktop_rich_text::TranscriptRole::Assistant,
        SingleSessionRole::Tool => desktop_rich_text::TranscriptRole::Tool,
        SingleSessionRole::System => desktop_rich_text::TranscriptRole::System,
        SingleSessionRole::Meta => desktop_rich_text::TranscriptRole::Meta,
    }
}

impl SingleSessionMessage {
    pub(crate) fn user(content: impl Into<String>) -> Self {
        Self::from_display_message(DisplayMessage::user(content))
    }

    pub(crate) fn assistant(content: impl Into<String>) -> Self {
        Self::from_display_message(DisplayMessage::assistant(content))
    }

    pub(crate) fn tool(content: impl Into<String>) -> Self {
        Self::from_display_message(DisplayMessage::tool_text(content))
    }

    #[allow(dead_code)]
    pub(crate) fn system(content: impl Into<String>) -> Self {
        Self::from_display_message(DisplayMessage::system(content))
    }

    #[allow(dead_code)]
    pub(crate) fn meta(content: impl Into<String>) -> Self {
        Self::from_display_message(DisplayMessage::meta(content))
    }

    pub(crate) fn from_display_message(display: DisplayMessage) -> Self {
        Self {
            display,
            rich_attachments: Vec::new(),
        }
    }

    fn with_rich_attachments(
        mut self,
        attachments: Vec<desktop_rich_text::RichAttachment>,
    ) -> Self {
        self.rich_attachments = attachments;
        self
    }

    fn role(&self) -> SingleSessionRole {
        match self.display.role.as_str() {
            "user" => SingleSessionRole::User,
            "assistant" => SingleSessionRole::Assistant,
            "tool" => SingleSessionRole::Tool,
            "system" | "background_task" => SingleSessionRole::System,
            _ => SingleSessionRole::Meta,
        }
    }

    fn content(&self) -> &str {
        &self.display.content
    }

    fn set_content(&mut self, content: impl Into<String>) {
        self.display.content = content.into();
    }

    fn content_mut(&mut self) -> &mut String {
        &mut self.display.content
    }

    fn rich_attachments(&self) -> &[desktop_rich_text::RichAttachment] {
        &self.rich_attachments
    }
}

impl PartialEq for SingleSessionMessage {
    fn eq(&self, other: &Self) -> bool {
        self.display.role == other.display.role
            && self.display.content == other.display.content
            && self.rich_attachments == other.rich_attachments
    }
}

impl Eq for SingleSessionMessage {}

fn hash_messages_cache_fingerprint<H: Hasher>(messages: &[SingleSessionMessage], hasher: &mut H) {
    messages.len().hash(hasher);
    if messages.len() <= BODY_CACHE_MESSAGE_EDGE_COUNT * 2 + BODY_CACHE_MESSAGE_MIDDLE_SAMPLE_COUNT
    {
        for message in messages {
            hash_message_cache_fingerprint(message, hasher);
        }
        return;
    }

    for message in &messages[..BODY_CACHE_MESSAGE_EDGE_COUNT] {
        hash_message_cache_fingerprint(message, hasher);
    }
    let middle_start = BODY_CACHE_MESSAGE_EDGE_COUNT;
    let middle_len = messages
        .len()
        .saturating_sub(BODY_CACHE_MESSAGE_EDGE_COUNT * 2);
    for sample in 1..=BODY_CACHE_MESSAGE_MIDDLE_SAMPLE_COUNT {
        let index =
            middle_start + sample * middle_len / (BODY_CACHE_MESSAGE_MIDDLE_SAMPLE_COUNT + 1);
        index.hash(hasher);
        hash_message_cache_fingerprint(&messages[index], hasher);
    }
    for message in &messages[messages.len() - BODY_CACHE_MESSAGE_EDGE_COUNT..] {
        hash_message_cache_fingerprint(message, hasher);
    }
}

fn hash_message_cache_fingerprint<H: Hasher>(message: &SingleSessionMessage, hasher: &mut H) {
    message.role().hash(hasher);
    hash_text_cache_fingerprint(message.content(), hasher);
    message.rich_attachments.hash(hasher);
}

fn hash_text_cache_fingerprint<H: Hasher>(text: &str, hasher: &mut H) {
    let bytes = text.as_bytes();
    bytes.len().hash(hasher);
    if bytes.len() <= BODY_CACHE_TEXT_EDGE_BYTES * 2 {
        bytes.hash(hasher);
        return;
    }

    bytes[..BODY_CACHE_TEXT_EDGE_BYTES].hash(hasher);
    bytes[bytes.len() - BODY_CACHE_TEXT_EDGE_BYTES..].hash(hasher);
}

impl SingleSessionApp {
    pub(crate) fn new(session: Option<workspace::SessionCard>) -> Self {
        let welcome = SingleSessionWelcomeState::new(session.is_some());
        Self {
            session,
            draft: String::new(),
            draft_cursor: 0,
            detail_scroll: 0,
            live_session_id: None,
            messages: Vec::new(),
            streaming_response: String::new(),
            status: None,
            status_kind: None,
            error: None,
            is_processing: false,
            body_scroll_lines: 0.0,
            show_help: false,
            show_session_info: false,
            pending_images: Vec::new(),
            model_picker: ModelPickerState::default(),
            session_switcher: SessionSwitcherState::default(),
            stdin_response: None,
            slash_suggestions: SlashSuggestionState::default(),
            runtime_settings: SingleSessionRuntimeSettings::default(),
            welcome,
            composer: SingleSessionComposerState::default(),
            selection: SingleSessionSelectionState::default(),
            runtime: SingleSessionRuntimeState::default(),
            tool: SingleSessionToolState::default(),
            view: SingleSessionViewState::default(),
        }
    }

    pub(crate) fn replace_session(&mut self, session: Option<workspace::SessionCard>) {
        let replacing_with_session = session.is_some();
        self.session = session;
        if let Some(session) = &self.session {
            self.live_session_id = Some(session.session_id.clone());
        }
        if replacing_with_session
            && self.messages.is_empty()
            && self.streaming_response.is_empty()
            && self.error.is_none()
        {
            self.welcome.timeline = false;
        } else if !replacing_with_session {
            self.welcome.timeline = true;
        }
        self.detail_scroll = 0;
    }

    pub(crate) fn initialize_resumed_session(&mut self, session_id: &str) {
        self.live_session_id = Some(session_id.to_string());
        self.detail_scroll = 0;
        self.messages.clear();
        self.streaming_response.clear();
        self.status = None;
        self.status_kind = None;
        self.error = None;
        self.stdin_response = None;
        self.body_scroll_lines = 0.0;
        self.show_help = false;
        self.show_session_info = false;
        self.is_processing = false;
        self.tool.active_message_index = None;
        self.tool.input_buffer.clear();
        self.runtime.reload_phase = ReloadPhase::Stable;
        self.view.inline_widget_opened_at = None;
        self.welcome.timeline = false;
    }

    pub(crate) fn set_recovery_session_count(&mut self, count: usize) {
        self.welcome.recovery_session_count = count;
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
        self.status_kind = None;
        self.error = None;
        self.is_processing = false;
        self.body_scroll_lines = 0.0;
        self.show_help = false;
        self.show_session_info = false;
        self.pending_images.clear();
        self.model_picker = ModelPickerState::default();
        self.session_switcher = SessionSwitcherState::default();
        self.stdin_response = None;
        self.welcome.reset_fresh();
        self.composer = SingleSessionComposerState::default();
        self.selection = SingleSessionSelectionState::default();
        self.runtime = SingleSessionRuntimeState::default();
        self.runtime_settings = SingleSessionRuntimeSettings::default();
        self.tool = SingleSessionToolState::default();
        self.view.inline_widget_opened_at = None;
    }

    pub(crate) fn status_title(&self) -> String {
        format!("Jcode · {}", self.title())
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
        String::new()
    }

    pub(crate) fn should_show_session_title_header(&self) -> bool {
        self.messages.is_empty()
            && self.streaming_response.is_empty()
            && self.error.is_none()
            && !self.model_picker.open
            && !self.session_switcher.open
            && self.stdin_response.is_none()
            && !self.show_help
            && !self.show_session_info
            && self.session.is_some()
    }

    pub(crate) fn has_background_work(&self) -> bool {
        self.has_activity_indicator()
    }

    pub(crate) fn has_frame_animation(&self) -> bool {
        self.has_activity_indicator() || self.inline_widget_reveal_in_progress()
    }

    fn mark_inline_widget_opened(&mut self) {
        self.view.inline_widget_opened_at = Some(Instant::now());
    }

    fn close_inline_widgets(&mut self) {
        self.show_help = false;
        self.show_session_info = false;
        self.model_picker.close();
        self.session_switcher.close();
    }

    fn open_read_only_inline_widget(&mut self, kind: InlineWidgetKind) {
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

    fn toggle_read_only_inline_widget(&mut self, kind: InlineWidgetKind) -> KeyOutcome {
        let was_active = self.active_inline_widget() == Some(kind);
        self.close_inline_widgets();
        if !was_active {
            self.open_read_only_inline_widget(kind);
        }
        self.scroll_body_to_bottom();
        KeyOutcome::Redraw
    }

    fn inline_widget_reveal_in_progress(&self) -> bool {
        self.active_inline_widget().is_some() && self.inline_widget_reveal_progress() < 1.0
    }

    pub(crate) fn inline_widget_reveal_progress(&self) -> f32 {
        if self.active_inline_widget().is_none() {
            return 0.0;
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
            .filter(|message| message.role().is_user())
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
    pub(crate) fn queued_draft_count(&self) -> usize {
        self.composer.queued_drafts.len()
    }

    #[cfg(test)]
    pub(crate) fn queued_draft_messages(&self) -> Vec<String> {
        self.composer
            .queued_drafts
            .iter()
            .map(|(message, _)| message.clone())
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn activity_indicator_active(&self) -> bool {
        self.has_activity_indicator()
    }

    pub(crate) fn has_activity_indicator(&self) -> bool {
        self.is_processing
            || self.model_picker.loading
            || self.session_switcher.loading
            || self
                .status_kind
                .as_ref()
                .is_some_and(SingleSessionStatus::is_in_flight)
    }

    fn set_status(&mut self, status: SingleSessionStatus) {
        self.status = Some(status.label());
        self.status_kind = Some(status);
    }

    pub(crate) fn set_status_label(&mut self, label: impl Into<String>) {
        self.set_status(SingleSessionStatus::Info(label.into()));
    }

    fn set_backend_status(&mut self, status: DesktopSessionStatus) {
        match &status {
            DesktopSessionStatus::ReasoningEffort(effort) => {
                self.runtime_settings.reasoning_effort = Some(effort.clone());
                self.messages.push(SingleSessionMessage::meta(format!(
                    "reasoning effort set to {effort}"
                )));
            }
            DesktopSessionStatus::ServiceTier(service_tier) => {
                self.runtime_settings.service_tier = Some(service_tier.clone());
                self.messages.push(SingleSessionMessage::meta(format!(
                    "fast mode set to {service_tier}"
                )));
            }
            DesktopSessionStatus::Transport(transport) => {
                self.runtime_settings.transport = Some(transport.clone());
                self.messages.push(SingleSessionMessage::meta(format!(
                    "transport set to {transport}"
                )));
            }
            DesktopSessionStatus::CompactionMode(mode) => {
                self.runtime_settings.compaction_mode = Some(mode.clone());
                self.messages.push(SingleSessionMessage::meta(format!(
                    "compaction mode set to {mode}"
                )));
            }
            DesktopSessionStatus::ReasoningEffortFailed(error)
            | DesktopSessionStatus::ServiceTierFailed(error)
            | DesktopSessionStatus::TransportFailed(error)
            | DesktopSessionStatus::CompactionModeFailed(error) => {
                self.messages.push(SingleSessionMessage::meta(format!(
                    "slash command failed: {error}"
                )));
            }
            DesktopSessionStatus::CompactResult { message, .. } => {
                self.messages
                    .push(SingleSessionMessage::meta(message.clone()));
            }
            _ => {}
        }
        self.set_status(SingleSessionStatus::Backend(status));
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn status_kind(&self) -> Option<&SingleSessionStatus> {
        self.status_kind.as_ref()
    }

    pub(crate) fn handle_key(&mut self, key: KeyInput) -> KeyOutcome {
        if key == KeyInput::ExitApp {
            return KeyOutcome::Exit;
        }

        if self.stdin_response.is_some() {
            return self.handle_stdin_response_key(key);
        }

        if self.session_switcher.open {
            return self.handle_session_switcher_key(key);
        }

        if matches!(
            self.active_inline_widget_mode(),
            Some(InlineWidgetMode::Interactive)
        ) && self.model_picker.open
        {
            return self.handle_model_picker_key(key);
        }

        if self.model_picker.open
            && self.model_picker.preview
            && let Some(outcome) = self.handle_model_picker_preview_key(&key)
        {
            return outcome;
        }

        if self.active_inline_widget() == Some(InlineWidgetKind::SlashSuggestions)
            && let Some(outcome) = self.handle_slash_suggestion_key(&key)
        {
            return outcome;
        }

        match key {
            KeyInput::SpawnPanel => KeyOutcome::SpawnSession,
            KeyInput::OpenSessionSwitcher => self.open_session_switcher(),
            KeyInput::OpenModelPicker => self.open_model_picker(),
            KeyInput::HotkeyHelp => {
                self.toggle_read_only_inline_widget(InlineWidgetKind::HotkeyHelp)
            }
            KeyInput::ToggleSessionInfo => {
                self.toggle_read_only_inline_widget(InlineWidgetKind::SessionInfo)
            }
            KeyInput::RefreshSessions if self.welcome.recovery_session_count > 0 => {
                KeyOutcome::RestoreCrashedSessions
            }
            KeyInput::RefreshSessions => KeyOutcome::Redraw,
            KeyInput::ExitApp => KeyOutcome::Exit,
            KeyInput::AdjustTextScale(direction) => {
                self.adjust_text_scale(direction);
                KeyOutcome::Redraw
            }
            KeyInput::ResetTextScale => {
                self.view.text_scale = 1.0;
                KeyOutcome::Redraw
            }
            KeyInput::CancelGeneration => {
                if self.is_processing {
                    KeyOutcome::CancelGeneration
                } else {
                    KeyOutcome::None
                }
            }
            KeyInput::ScrollBodyPages(pages) => {
                self.scroll_body_lines((pages * 12) as f32);
                KeyOutcome::Redraw
            }
            KeyInput::ScrollBodyLines(lines) => {
                self.scroll_body_lines(lines as f32);
                KeyOutcome::Redraw
            }
            KeyInput::ScrollBodyToTop => {
                self.scroll_body_to_top();
                KeyOutcome::Redraw
            }
            KeyInput::ScrollBodyToBottom => {
                self.scroll_body_to_bottom();
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
            KeyInput::CopyLatestCodeBlock => self.copy_latest_code_block(),
            KeyInput::CopyTranscript => self.copy_transcript(),
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
            KeyInput::RetrieveQueuedDraft => self.retrieve_queued_draft_for_edit(),
            KeyInput::QueueDraft => self.submit_draft(),
            KeyInput::SubmitDraft => self.submit_draft(),
            KeyInput::Escape if self.show_help => {
                self.show_help = false;
                KeyOutcome::Redraw
            }
            KeyInput::Escape if self.show_session_info => {
                self.show_session_info = false;
                KeyOutcome::Redraw
            }
            KeyInput::Character(text)
                if (self.show_help || self.show_session_info) && text.eq_ignore_ascii_case("q") =>
            {
                self.close_inline_widgets();
                KeyOutcome::Redraw
            }
            KeyInput::Escape => {
                if self.is_processing {
                    KeyOutcome::CancelGeneration
                } else {
                    self.clear_draft_for_escape()
                }
            }
            KeyInput::Enter => {
                self.insert_draft_text("\n");
                KeyOutcome::Redraw
            }
            KeyInput::Backspace => {
                self.delete_previous_char();
                self.sync_inline_previews_from_draft()
                    .unwrap_or(KeyOutcome::Redraw)
            }
            KeyInput::DeletePreviousWord => {
                self.delete_previous_word();
                self.sync_inline_previews_from_draft()
                    .unwrap_or(KeyOutcome::Redraw)
            }
            KeyInput::DeleteNextWord => {
                self.delete_next_word();
                self.sync_slash_suggestions_from_draft();
                KeyOutcome::Redraw
            }
            KeyInput::DeleteNextChar => {
                self.delete_next_char();
                self.sync_slash_suggestions_from_draft();
                KeyOutcome::Redraw
            }
            KeyInput::MoveCursorWordLeft => {
                self.move_cursor_word_left();
                self.sync_slash_suggestions_from_draft();
                KeyOutcome::Redraw
            }
            KeyInput::MoveCursorWordRight => {
                self.move_cursor_word_right();
                self.sync_slash_suggestions_from_draft();
                KeyOutcome::Redraw
            }
            KeyInput::MoveCursorLeft => {
                self.move_cursor_left();
                self.sync_slash_suggestions_from_draft();
                KeyOutcome::Redraw
            }
            KeyInput::MoveCursorRight => {
                self.move_cursor_right();
                self.sync_slash_suggestions_from_draft();
                KeyOutcome::Redraw
            }
            KeyInput::MoveToLineStart => {
                self.move_to_line_start();
                self.sync_slash_suggestions_from_draft();
                KeyOutcome::Redraw
            }
            KeyInput::MoveToLineEnd => {
                self.move_to_line_end();
                self.sync_slash_suggestions_from_draft();
                KeyOutcome::Redraw
            }
            KeyInput::DeleteToLineStart => {
                self.delete_to_line_start();
                self.sync_inline_previews_from_draft()
                    .unwrap_or(KeyOutcome::Redraw)
            }
            KeyInput::DeleteToLineEnd => {
                self.delete_to_line_end();
                self.sync_inline_previews_from_draft()
                    .unwrap_or(KeyOutcome::Redraw)
            }
            KeyInput::CutInputLine => self.cut_input_line(),
            KeyInput::UndoInput => {
                self.undo_input_change();
                self.sync_inline_previews_from_draft();
                KeyOutcome::Redraw
            }
            KeyInput::Autocomplete => self.autocomplete_draft(),
            KeyInput::Character(text) => {
                self.insert_draft_text(&text);
                self.sync_inline_previews_from_draft()
                    .unwrap_or(KeyOutcome::Redraw)
            }
            _ => KeyOutcome::None,
        }
    }

    pub(crate) fn text_scale(&self) -> f32 {
        self.view.text_scale
    }

    pub(crate) fn has_active_selection(&self) -> bool {
        self.selection.anchor.is_some()
            || self.selection.focus.is_some()
            || self.selection.draft_anchor.is_some()
            || self.selection.draft_focus.is_some()
    }

    fn adjust_text_scale(&mut self, direction: i8) {
        let delta = direction as f32 * SINGLE_SESSION_TEXT_SCALE_STEP;
        self.view.text_scale = (self.view.text_scale + delta)
            .clamp(SINGLE_SESSION_MIN_TEXT_SCALE, SINGLE_SESSION_MAX_TEXT_SCALE);
    }

    fn open_model_picker(&mut self) -> KeyOutcome {
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

    fn open_model_picker_preview(&mut self, filter: String) -> KeyOutcome {
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

    fn sync_model_picker_preview_from_draft(&mut self) -> Option<KeyOutcome> {
        let Some(filter) = model_picker_preview_filter(&self.draft) else {
            if self.model_picker.open && self.model_picker.preview {
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

    fn sync_inline_previews_from_draft(&mut self) -> Option<KeyOutcome> {
        self.sync_slash_suggestions_from_draft();
        self.sync_model_picker_preview_from_draft()
    }

    fn sync_slash_suggestions_from_draft(&mut self) {
        let was_visible = self.slash_suggestions_visible();
        let Some(query) = slash_suggestion_query(&self.draft, self.draft_cursor) else {
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

        if self.slash_suggestions.query != query {
            self.slash_suggestions.query = query;
            self.slash_suggestions.selected = 0;
        }
        let candidate_count = self.slash_suggestion_candidates().len();
        if candidate_count == 0 {
            self.slash_suggestions.selected = 0;
            return;
        }
        self.slash_suggestions.selected = self.slash_suggestions.selected.min(candidate_count - 1);
        if !was_visible {
            self.mark_inline_widget_opened();
            self.scroll_body_to_bottom();
        }
    }

    fn handle_slash_suggestion_key(&mut self, key: &KeyInput) -> Option<KeyOutcome> {
        match key {
            KeyInput::Escape => {
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
                self.complete_selected_slash_suggestion();
                Some(self.submit_draft())
            }
            _ => None,
        }
    }

    fn move_slash_suggestion_selection(&mut self, delta: i32) {
        let count = self.slash_suggestion_candidates().len();
        if count == 0 {
            self.slash_suggestions.selected = 0;
            return;
        }
        let selected = self.slash_suggestions.selected as i32 + delta;
        self.slash_suggestions.selected =
            selected.clamp(0, count.saturating_sub(1) as i32) as usize;
    }

    fn complete_selected_slash_suggestion(&mut self) -> Option<KeyOutcome> {
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

    fn handle_model_picker_preview_key(&mut self, key: &KeyInput) -> Option<KeyOutcome> {
        match key {
            KeyInput::Escape => {
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
                    self.model_picker.close();
                    self.draft.clear();
                    self.draft_cursor = 0;
                    self.composer.input_undo_stack.clear();
                    return Some(KeyOutcome::Redraw);
                };
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

    fn open_session_switcher(&mut self) -> KeyOutcome {
        self.close_inline_widgets();
        let current_session_id = self.current_session_id().map(str::to_string);
        self.session_switcher
            .open_loading(current_session_id.as_deref());
        self.set_status(SingleSessionStatus::LoadingRecentSessions);
        self.scroll_body_to_bottom();
        self.mark_inline_widget_opened();
        KeyOutcome::LoadSessionSwitcher
    }

    fn handle_model_picker_key(&mut self, key: KeyInput) -> KeyOutcome {
        match key {
            KeyInput::Escape if !self.model_picker.filter.is_empty() => {
                self.model_picker.set_filter(String::new());
                KeyOutcome::Redraw
            }
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
                self.set_status(SingleSessionStatus::LoadingRecentSessions);
                self.mark_inline_widget_opened();
                KeyOutcome::LoadSessionSwitcher
            }
            KeyInput::ModelPickerMove(delta) => {
                self.session_switcher.move_selection(delta);
                KeyOutcome::Redraw
            }
            KeyInput::ScrollBodyPages(pages) => {
                self.session_switcher
                    .move_selection(if pages > 0 { -5 } else { 5 });
                KeyOutcome::Redraw
            }
            KeyInput::MoveToLineStart => {
                self.session_switcher.select_first();
                KeyOutcome::Redraw
            }
            KeyInput::MoveToLineEnd => {
                self.session_switcher.select_last();
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
                self.open_read_only_inline_widget(InlineWidgetKind::HotkeyHelp);
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
            self.set_status(SingleSessionStatus::Info(format!(
                "{} recent session(s)",
                self.session_switcher.sessions.len()
            )));
        }
    }

    fn resume_selected_switcher_session(&mut self) -> KeyOutcome {
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
        self.body_scroll_lines = 0.0;
        self.show_help = false;
        self.welcome.timeline = false;
        self.session_switcher.close();
        self.set_status(SingleSessionStatus::Info(format!("resumed {title}")));
        KeyOutcome::Redraw
    }

    fn handle_stdin_response_key(&mut self, key: KeyInput) -> KeyOutcome {
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
        self.body_styled_lines_without_inline_widgets()
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

    pub(crate) fn inline_widget_line_count(&self) -> usize {
        self.inline_widget_styled_lines().len()
    }

    fn slash_suggestions_visible(&self) -> bool {
        !self.slash_suggestion_candidates().is_empty()
    }

    fn slash_suggestion_styled_lines(&self) -> Vec<SingleSessionStyledLine> {
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
                    styled_line(format!("  {usage:<24} {description}"), style)
                }),
        );
        lines
    }

    fn slash_suggestion_candidates(&self) -> Vec<(&'static str, &'static str)> {
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
        DESKTOP_SLASH_COMMANDS
            .iter()
            .copied()
            .filter(|(usage, _)| {
                usage
                    .split_whitespace()
                    .next()
                    .unwrap_or(usage)
                    .starts_with(&prefix)
            })
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
                mode: InlineWidgetMode::Interactive,
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

    pub(crate) fn active_inline_widget_uses_card_chrome(&self) -> bool {
        self.active_inline_widget().is_some()
    }

    pub(crate) fn should_draw_composer_caret(&self) -> bool {
        !self.active_overlay_state().blocks_composer_caret()
    }

    fn body_styled_lines_without_inline_widgets(&self) -> Vec<SingleSessionStyledLine> {
        if !self.messages.is_empty() || !self.streaming_response.is_empty() || self.error.is_some()
        {
            return self.transcript_styled_lines(true);
        }

        if self.is_welcome_timeline_visible() {
            if let Some(status) = &self.status
                && self.session.is_none()
                && !self.model_picker.open
                && !self.show_session_info
            {
                return vec![styled_line(status.clone(), SingleSessionLineStyle::Status)];
            }
            if self.welcome.recovery_session_count > 0 {
                return welcome_recovery_styled_lines(self.welcome.recovery_session_count);
            }
            return Vec::new();
        }

        if let Some(status) = &self.status
            && self.session.is_none()
            && !self.model_picker.open
            && !self.show_session_info
        {
            return vec![styled_line(status.clone(), SingleSessionLineStyle::Status)];
        }

        single_session_styled_lines(self.session.as_ref())
    }

    pub(crate) fn body_styled_lines_for_tick(&self, _tick: u64) -> Vec<SingleSessionStyledLine> {
        self.body_styled_lines()
    }

    pub(crate) fn body_styled_lines_without_streaming_response(
        &self,
    ) -> Option<Vec<SingleSessionStyledLine>> {
        if self.stdin_response.is_some()
            || self.session_switcher.open
            || self.model_picker.open
            || self.show_help
            || self.error.is_some()
        {
            return None;
        }
        if self.messages.is_empty() && self.streaming_response.is_empty() {
            return None;
        }
        Some(self.transcript_styled_lines(false))
    }

    pub(crate) fn streaming_response_styled_lines(&self) -> Vec<SingleSessionStyledLine> {
        let mut lines = Vec::new();
        if !self.streaming_response.is_empty() {
            append_streaming_assistant_lines(&mut lines, self.streaming_response.trim_end());
        }
        lines
    }

    fn transcript_styled_lines(
        &self,
        include_streaming_response: bool,
    ) -> Vec<SingleSessionStyledLine> {
        let mut lines = Vec::new();
        let mut user_turn = 1;
        let mut message_index = 0;
        while message_index < self.messages.len() {
            if !lines.is_empty() {
                lines.push(blank_styled_line());
            }
            let message = &self.messages[message_index];
            if message.role() == SingleSessionRole::Tool {
                let group_start = message_index;
                while message_index < self.messages.len()
                    && self.messages[message_index].role() == SingleSessionRole::Tool
                {
                    message_index += 1;
                }
                let tool_messages = &self.messages[group_start..message_index];
                let group_contains_active_tool = self
                    .tool
                    .active_message_index
                    .is_some_and(|index| (group_start..message_index).contains(&index));
                if tool_messages.len() > 1 && !group_contains_active_tool {
                    append_tool_group_summary(&mut lines, tool_messages);
                } else {
                    for (offset, tool_message) in tool_messages.iter().enumerate() {
                        let is_active_tool = self.tool.active_message_index
                            == Some(group_start.saturating_add(offset));
                        append_chat_message_lines(
                            &mut lines,
                            tool_message,
                            &mut user_turn,
                            is_active_tool,
                            if is_active_tool {
                                Some(self.tool.input_buffer.as_str())
                            } else {
                                None
                            },
                        );
                    }
                }
                continue;
            }
            append_chat_message_lines(&mut lines, message, &mut user_turn, false, None);
            message_index += 1;
        }
        if include_streaming_response && !self.streaming_response.is_empty() {
            if !lines.is_empty() {
                lines.push(blank_styled_line());
            }
            append_streaming_assistant_lines(&mut lines, self.streaming_response.trim_end());
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
        lines
    }

    pub(crate) fn rendered_body_cache_key(&self, size: (u32, u32)) -> u64 {
        let mut hasher = DefaultHasher::new();
        size.hash(&mut hasher);
        self.session
            .as_ref()
            .map(|session| {
                (
                    session.session_id.as_str(),
                    session.title.as_str(),
                    session.subtitle.as_str(),
                    session.detail.as_str(),
                    session.preview_lines.as_slice(),
                    session.detail_lines.as_slice(),
                )
            })
            .hash(&mut hasher);
        hash_messages_cache_fingerprint(&self.messages, &mut hasher);
        hash_text_cache_fingerprint(&self.streaming_response, &mut hasher);
        self.tool.active_message_index.hash(&mut hasher);
        hash_text_cache_fingerprint(&self.tool.input_buffer, &mut hasher);
        self.status.hash(&mut hasher);
        self.error.hash(&mut hasher);
        self.show_help.hash(&mut hasher);
        self.show_session_info.hash(&mut hasher);
        self.model_picker.open.hash(&mut hasher);
        self.model_picker.filter.hash(&mut hasher);
        self.model_picker.selected.hash(&mut hasher);
        self.session_switcher.open.hash(&mut hasher);
        self.session_switcher.filter.hash(&mut hasher);
        self.session_switcher.selected.hash(&mut hasher);
        self.stdin_response.hash(&mut hasher);
        self.welcome.name.hash(&mut hasher);
        self.welcome.recovery_session_count.hash(&mut hasher);
        self.welcome.timeline.hash(&mut hasher);
        self.welcome.hero_phrase_index.hash(&mut hasher);
        self.view.text_scale.to_bits().hash(&mut hasher);
        hasher.finish()
    }

    pub(crate) fn rendered_body_static_cache_key(&self, size: (u32, u32)) -> u64 {
        let mut hasher = DefaultHasher::new();
        size.hash(&mut hasher);
        self.session
            .as_ref()
            .map(|session| {
                (
                    session.session_id.as_str(),
                    session.title.as_str(),
                    session.subtitle.as_str(),
                    session.detail.as_str(),
                    session.preview_lines.as_slice(),
                    session.detail_lines.as_slice(),
                )
            })
            .hash(&mut hasher);
        hash_messages_cache_fingerprint(&self.messages, &mut hasher);
        self.tool.active_message_index.hash(&mut hasher);
        hash_text_cache_fingerprint(&self.tool.input_buffer, &mut hasher);
        self.status.hash(&mut hasher);
        self.error.hash(&mut hasher);
        self.show_help.hash(&mut hasher);
        self.show_session_info.hash(&mut hasher);
        self.model_picker.open.hash(&mut hasher);
        self.model_picker.filter.hash(&mut hasher);
        self.model_picker.selected.hash(&mut hasher);
        self.session_switcher.open.hash(&mut hasher);
        self.session_switcher.filter.hash(&mut hasher);
        self.session_switcher.selected.hash(&mut hasher);
        self.stdin_response.hash(&mut hasher);
        self.welcome.name.hash(&mut hasher);
        self.welcome.recovery_session_count.hash(&mut hasher);
        self.welcome.timeline.hash(&mut hasher);
        self.welcome.hero_phrase_index.hash(&mut hasher);
        self.view.text_scale.to_bits().hash(&mut hasher);
        hasher.finish()
    }

    pub(crate) fn welcome_hero_text(&self) -> String {
        handwritten_welcome_phrase(self.welcome.hero_phrase_index).to_string()
    }

    pub(crate) fn is_welcome_timeline_visible(&self) -> bool {
        self.welcome.timeline
            && !self.show_help
            && !self.show_session_info
            && !self.session_switcher.open
            && self.stdin_response.is_none()
    }

    pub(crate) fn has_welcome_timeline_transcript(&self) -> bool {
        !self.messages.is_empty() || !self.streaming_response.is_empty() || self.error.is_some()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_fresh_welcome_visible(&self) -> bool {
        self.session.is_none()
            && self.live_session_id.is_none()
            && self.messages.is_empty()
            && self.streaming_response.is_empty()
            && self.status.is_none()
            && self.error.is_none()
            && self.pending_images.is_empty()
            && !self.show_help
            && !self.model_picker.open
            && !self.session_switcher.open
            && self.stdin_response.is_none()
    }

    pub(crate) fn apply_session_event(&mut self, event: DesktopSessionEvent) {
        match event {
            DesktopSessionEvent::Status(status) => self.set_backend_status(status),
            DesktopSessionEvent::Reloading { .. } => {
                self.set_status(SingleSessionStatus::ServerReloading);
                self.is_processing = true;
                self.runtime.reload_phase = ReloadPhase::AwaitingReconnect;
            }
            DesktopSessionEvent::Reloaded { session_id } => {
                self.live_session_id = Some(session_id);
                self.set_status(SingleSessionStatus::ServerReconnected);
                self.is_processing = true;
                self.runtime.reload_phase = ReloadPhase::Stable;
            }
            DesktopSessionEvent::SessionStarted { session_id } => {
                self.live_session_id = Some(session_id);
                self.set_status(SingleSessionStatus::Connected);
            }
            DesktopSessionEvent::SessionRenamed {
                title,
                display_title,
            } => {
                if let Some(session) = &mut self.session {
                    session.title = display_title.clone();
                }
                let message = if title.is_some() {
                    format!("renamed session to {display_title}")
                } else {
                    format!("cleared session name; title is now {display_title}")
                };
                self.messages.push(SingleSessionMessage::meta(message));
                self.set_status(SingleSessionStatus::Info(if title.is_some() {
                    "session renamed".to_string()
                } else {
                    "session name cleared".to_string()
                }));
            }
            DesktopSessionEvent::TextDelta(text) => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                self.streaming_response.push_str(&text);
                self.set_status(SingleSessionStatus::Receiving);
            }
            DesktopSessionEvent::TextReplace(text) => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                self.streaming_response = text;
                self.set_status(SingleSessionStatus::Receiving);
            }
            DesktopSessionEvent::ToolStarted { name } => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                self.finish_streaming_response();
                self.collapse_active_tool_message();
                self.tool.input_buffer.clear();
                self.set_status(SingleSessionStatus::ToolPreparing(name.clone()));
                self.messages
                    .push(SingleSessionMessage::tool(format!("▾ {name} preparing")));
                self.tool.active_message_index = Some(self.messages.len().saturating_sub(1));
            }
            DesktopSessionEvent::ToolExecuting { name } => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                self.finish_streaming_response();
                self.set_status(SingleSessionStatus::ToolUsing(name.clone()));
                self.replace_active_tool_header(&format!("▾ {name} running"));
            }
            DesktopSessionEvent::ToolInput { delta } => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                self.finish_streaming_response();
                self.append_active_tool_input(&delta);
            }
            DesktopSessionEvent::ToolFinished {
                name,
                summary,
                is_error,
            } => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                self.finish_streaming_response();
                self.set_status(SingleSessionStatus::ToolFinished {
                    name: name.clone(),
                    is_error,
                });
                let marker = if is_error { "failed" } else { "done" };
                let line = format!("▾ {name} {marker}: {summary}");
                self.flush_active_tool_input_to_message();
                if let Some(index) = self.tool.active_message_index
                    && let Some(message) = self.messages.get_mut(index)
                    && message.role() == SingleSessionRole::Tool
                {
                    let replacement =
                        merge_tool_finish_with_existing_context(message.content(), &line);
                    message.set_content(replacement);
                } else {
                    self.messages.push(SingleSessionMessage::tool(line));
                    self.tool.active_message_index = Some(self.messages.len().saturating_sub(1));
                }
            }
            DesktopSessionEvent::ModelChanged {
                model,
                provider_name,
                error,
            } => {
                if let Some(error) = error {
                    self.set_status(SingleSessionStatus::ModelSwitchFailed);
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
                self.set_status(SingleSessionStatus::ModelSelected(label.clone()));
                self.messages.push(SingleSessionMessage::meta(format!(
                    "model switched to {label}"
                )));
            }
            DesktopSessionEvent::ModelCatalog {
                current_model,
                provider_name,
                models,
                reasoning_effort,
                service_tier,
                compaction_mode,
            } => {
                if let Some(reasoning_effort) = reasoning_effort {
                    self.runtime_settings.reasoning_effort = Some(reasoning_effort);
                }
                if let Some(service_tier) = service_tier {
                    self.runtime_settings.service_tier = Some(service_tier);
                }
                if let Some(compaction_mode) = compaction_mode {
                    self.runtime_settings.compaction_mode = Some(compaction_mode);
                }
                self.model_picker
                    .apply_catalog(current_model, provider_name, models);
                self.set_status(SingleSessionStatus::ModelsLoaded);
            }
            DesktopSessionEvent::ModelCatalogError { error } => {
                self.model_picker.apply_error(error.clone());
                self.set_status(SingleSessionStatus::ModelPickerError);
            }
            DesktopSessionEvent::StdinRequest {
                request_id,
                prompt,
                is_password,
                tool_call_id,
            } => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                self.set_status(SingleSessionStatus::InteractiveInputRequested);
                self.close_inline_widgets();
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
                if self.runtime.reload_phase == ReloadPhase::AwaitingReconnect {
                    self.set_status(SingleSessionStatus::ServerReloading);
                    self.is_processing = true;
                    return;
                }
                self.finish_streaming_response();
                self.is_processing = false;
                self.stdin_response = None;
                self.runtime.session_handle = None;
                self.tool.active_message_index = None;
                self.tool.input_buffer.clear();
                self.set_status(SingleSessionStatus::Ready);
            }
            DesktopSessionEvent::Error(error) => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                self.finish_streaming_response();
                self.is_processing = false;
                self.stdin_response = None;
                self.runtime.session_handle = None;
                self.tool.active_message_index = None;
                self.tool.input_buffer.clear();
                self.set_status(SingleSessionStatus::Error);
                self.error = Some(error);
            }
        }
    }

    pub(crate) fn set_session_handle(&mut self, handle: DesktopSessionHandle) {
        self.runtime.session_handle = Some(handle);
    }

    pub(crate) fn cancel_generation(&mut self) -> bool {
        let Some(handle) = &self.runtime.session_handle else {
            return false;
        };
        match handle.cancel() {
            Ok(()) => {
                self.stdin_response = None;
                self.set_status(SingleSessionStatus::Cancelling);
                true
            }
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                self.is_processing = false;
                self.stdin_response = None;
                self.runtime.session_handle = None;
                true
            }
        }
    }

    pub(crate) fn scroll_body_lines(&mut self, lines: impl Into<f64>) {
        let lines = lines.into() as f32;
        if !lines.is_finite() || lines.abs() < f32::EPSILON {
            return;
        }
        self.body_scroll_lines = (self.body_scroll_lines + lines).max(0.0);
    }

    pub(crate) fn scroll_body_to_top(&mut self) {
        self.body_scroll_lines = self
            .body_styled_lines_without_inline_widgets()
            .len()
            .saturating_sub(1) as f32;
    }

    pub(crate) fn scroll_body_to_bottom(&mut self) {
        self.body_scroll_lines = 0.0;
    }

    fn copy_latest_code_block(&mut self) -> KeyOutcome {
        if let Some(text) = self
            .latest_rich_code_block_text()
            .filter(|text| !text.trim().is_empty())
        {
            return KeyOutcome::CopyText {
                text,
                success_notice: "copied latest code block",
            };
        }
        self.set_status(SingleSessionStatus::Info(
            "no code block to copy".to_string(),
        ));
        KeyOutcome::Redraw
    }

    fn copy_transcript(&mut self) -> KeyOutcome {
        if let Some(text) = self
            .copy_rich_transcript_text(desktop_rich_text::TranscriptCopyMode::TranscriptPlainText)
            .filter(|text| !text.trim().is_empty())
        {
            return KeyOutcome::CopyText {
                text,
                success_notice: "copied transcript",
            };
        }
        self.set_status(SingleSessionStatus::Info(
            "no transcript to copy".to_string(),
        ));
        KeyOutcome::Redraw
    }

    pub(crate) fn latest_assistant_response(&self) -> Option<String> {
        if !self.streaming_response.trim().is_empty() {
            return Some(self.streaming_response.trim().to_string());
        }
        self.messages
            .iter()
            .rev()
            .find(|message| message.role() == SingleSessionRole::Assistant)
            .map(|message| message.content().trim().to_string())
            .filter(|message| !message.is_empty())
    }

    pub(crate) fn rich_transcript_document(&self) -> desktop_rich_text::RichTranscriptDocument {
        desktop_rich_text::build_rich_transcript(
            &self.rich_transcript_messages(true),
            &desktop_rich_text::RichTranscriptBuildOptions::default(),
        )
    }

    pub(crate) fn search_rich_transcript(
        &self,
        query: &str,
    ) -> Vec<desktop_rich_text::TranscriptSearchMatch> {
        let document = self.rich_transcript_document();
        desktop_rich_text::search_transcript(&document, query, false)
    }

    pub(crate) fn copy_rich_transcript_text(
        &self,
        mode: desktop_rich_text::TranscriptCopyMode,
    ) -> Option<String> {
        let document = self.rich_transcript_document();
        desktop_rich_text::copy_transcript_text(&document, mode)
    }

    pub(crate) fn latest_rich_code_block_text(&self) -> Option<String> {
        let document = self.rich_transcript_document();
        document.blocks.iter().rev().find_map(|block| {
            matches!(
                block.kind,
                desktop_rich_text::TranscriptBlockKind::CodeBlock { .. }
            )
            .then(|| block.copy_text.clone())
        })
    }

    #[allow(dead_code)]
    pub(crate) fn rich_transcript_jump_targets(
        &self,
    ) -> Vec<desktop_rich_text::TranscriptJumpTarget> {
        self.rich_transcript_document().jumps
    }

    fn rich_transcript_messages(
        &self,
        include_streaming_response: bool,
    ) -> Vec<desktop_rich_text::RichTranscriptMessage> {
        let mut messages = self
            .messages
            .iter()
            .enumerate()
            .map(|(index, message)| {
                let mut rich = desktop_rich_text::RichTranscriptMessage::new(
                    format!("message-{index}"),
                    rich_role_from_single_session_role(message.role()),
                    message.content().to_string(),
                );
                rich.attachments = message.rich_attachments().to_vec();
                rich
            })
            .collect::<Vec<_>>();

        if include_streaming_response && !self.streaming_response.trim().is_empty() {
            messages.push(desktop_rich_text::RichTranscriptMessage::new(
                "streaming-assistant",
                desktop_rich_text::TranscriptRole::Assistant,
                self.streaming_response.trim().to_string(),
            ));
        }
        if let Some(error) = &self.error {
            messages.push(desktop_rich_text::RichTranscriptMessage::new(
                "desktop-error",
                desktop_rich_text::TranscriptRole::System,
                format!("error: {error}"),
            ));
        }
        messages
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
            .saturating_sub(self.body_scroll_lines.floor().max(0.0) as usize)
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
            self.body_scroll_lines = lines.len().saturating_sub(target + 1) as f32;
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

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn set_draft_cursor_line_col(&mut self, target_line: usize, target_col: usize) {
        self.draft_cursor = self.draft_byte_index_for_line_col(target_line, target_col);
        self.clamp_draft_cursor();
        self.clear_selection();
        self.clear_draft_selection();
    }

    fn draft_byte_index_for_line_col(&self, target_line: usize, target_col: usize) -> usize {
        let mut line = 0usize;
        let mut line_start = 0usize;
        for (index, ch) in self.draft.char_indices() {
            if line == target_line {
                break;
            }
            if ch == '\n' {
                line += 1;
                line_start = index + ch.len_utf8();
            }
        }

        if line < target_line {
            return self.draft.len();
        }

        let line_end = line_end(&self.draft, line_start);
        self.draft[line_start..line_end]
            .char_indices()
            .map(|(offset, _)| line_start + offset)
            .chain(std::iter::once(line_end))
            .nth(target_col)
            .unwrap_or(line_end)
    }

    fn submit_draft(&mut self) -> KeyOutcome {
        let message = self.draft.trim().to_string();
        if message.is_empty() && self.pending_images.is_empty() {
            return KeyOutcome::None;
        }
        if self.pending_images.is_empty()
            && let Some(outcome) = self.handle_slash_command(&message)
        {
            return outcome;
        }
        let images = std::mem::take(&mut self.pending_images);
        self.record_user_submit(&message, &images);
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

    fn handle_slash_command(&mut self, message: &str) -> Option<KeyOutcome> {
        if !message.starts_with('/') {
            return None;
        }

        let mut parts = message.splitn(2, char::is_whitespace);
        let command = parts.next().unwrap_or_default();
        let args = parts.next().unwrap_or_default().trim();

        let outcome = match command {
            "/help" | "/?" | "/commands" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                self.show_help = true;
                self.model_picker.close();
                self.session_switcher.close();
                self.mark_inline_widget_opened();
                self.set_status(SingleSessionStatus::Info(
                    "showing desktop slash commands".to_string(),
                ));
                self.scroll_body_to_bottom();
                KeyOutcome::Redraw
            }
            "/clear" => {
                self.messages.clear();
                self.streaming_response.clear();
                self.error = None;
                self.is_processing = false;
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                self.set_status(SingleSessionStatus::Info("session cleared".to_string()));
                self.scroll_body_to_bottom();
                if self.session.is_some() || self.live_session_id.is_some() {
                    KeyOutcome::ClearServerSession
                } else {
                    KeyOutcome::Redraw
                }
            }
            "/new" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                KeyOutcome::SpawnSession
            }
            "/sessions" | "/session" | "/resume" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                return Some(self.open_session_switcher());
            }
            "/model" | "/models" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                if args.is_empty() {
                    return Some(self.open_model_picker());
                }
                KeyOutcome::SetModel(args.to_string())
            }
            "/refresh-model-list" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                self.model_picker.open_loading();
                self.set_status(SingleSessionStatus::Info(
                    "refreshing model list".to_string(),
                ));
                KeyOutcome::RefreshModelCatalog
            }
            "/effort" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                if args.is_empty() || args == "status" {
                    let current = self
                        .runtime_settings
                        .reasoning_effort
                        .as_deref()
                        .unwrap_or("default");
                    self.set_status(SingleSessionStatus::Info(format!(
                        "effort: {current} · use /effort <none|low|medium|high|xhigh>"
                    )));
                    KeyOutcome::Redraw
                } else if matches!(args, "none" | "low" | "medium" | "high" | "xhigh") {
                    KeyOutcome::SetReasoningEffort(args.to_string())
                } else {
                    self.set_status(SingleSessionStatus::Info(
                        "usage: /effort <none|low|medium|high|xhigh>".to_string(),
                    ));
                    KeyOutcome::Redraw
                }
            }
            "/fast" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                match args {
                    "" | "status" => {
                        let current = self
                            .runtime_settings
                            .service_tier
                            .as_deref()
                            .unwrap_or("standard");
                        self.set_status(SingleSessionStatus::Info(format!(
                            "fast mode: {current} · use /fast <on|off|status>"
                        )));
                        KeyOutcome::Redraw
                    }
                    "on" => KeyOutcome::SetServiceTier("priority".to_string()),
                    "off" => KeyOutcome::SetServiceTier("off".to_string()),
                    _ => {
                        self.set_status(SingleSessionStatus::Info(
                            "usage: /fast [on|off|status]".to_string(),
                        ));
                        KeyOutcome::Redraw
                    }
                }
            }
            "/transport" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                match args {
                    "" | "status" => {
                        let current = self
                            .runtime_settings
                            .transport
                            .as_deref()
                            .unwrap_or("unknown");
                        self.set_status(SingleSessionStatus::Info(format!(
                            "transport: {current} · use /transport <auto|https|websocket>"
                        )));
                        KeyOutcome::Redraw
                    }
                    "auto" | "https" | "websocket" => KeyOutcome::SetTransport(args.to_string()),
                    _ => {
                        self.set_status(SingleSessionStatus::Info(
                            "usage: /transport <auto|https|websocket>".to_string(),
                        ));
                        KeyOutcome::Redraw
                    }
                }
            }
            "/compact" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                if args.is_empty() {
                    KeyOutcome::CompactSession
                } else if args == "mode" || args == "mode status" {
                    let current = self
                        .runtime_settings
                        .compaction_mode
                        .as_deref()
                        .unwrap_or("reactive");
                    self.set_status(SingleSessionStatus::Info(format!(
                        "compaction: {current} · use /compact mode <reactive|proactive|semantic>"
                    )));
                    KeyOutcome::Redraw
                } else if let Some(mode) = args.strip_prefix("mode ") {
                    let mode = mode.trim();
                    if matches!(mode, "reactive" | "proactive" | "semantic") {
                        KeyOutcome::SetCompactionMode(mode.to_string())
                    } else {
                        self.set_status(SingleSessionStatus::Info(
                            "usage: /compact mode <reactive|proactive|semantic>".to_string(),
                        ));
                        KeyOutcome::Redraw
                    }
                } else {
                    self.set_status(SingleSessionStatus::Info(
                        "usage: /compact [mode <reactive|proactive|semantic>]".to_string(),
                    ));
                    KeyOutcome::Redraw
                }
            }
            "/rename" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                if args.is_empty() {
                    self.set_status(SingleSessionStatus::Info(
                        "usage: /rename <session name> or /rename --clear".to_string(),
                    ));
                    KeyOutcome::Redraw
                } else if args == "--clear" {
                    KeyOutcome::RenameSession(None)
                } else {
                    KeyOutcome::RenameSession(Some(args.to_string()))
                }
            }
            "/copy" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                return Some(match args {
                    "" | "latest" | "response" => self
                        .latest_assistant_response()
                        .map(KeyOutcome::CopyLatestResponse)
                        .unwrap_or_else(|| {
                            self.set_status(SingleSessionStatus::Info(
                                "no assistant response to copy".to_string(),
                            ));
                            KeyOutcome::Redraw
                        }),
                    "code" | "codeblock" | "code-block" => self
                        .latest_rich_code_block_text()
                        .map(|text| KeyOutcome::CopyText {
                            text,
                            success_notice: "copied latest code block",
                        })
                        .unwrap_or_else(|| {
                            self.set_status(SingleSessionStatus::Info(
                                "no code block to copy".to_string(),
                            ));
                            KeyOutcome::Redraw
                        }),
                    "transcript" | "all" => self
                        .copy_rich_transcript_text(
                            desktop_rich_text::TranscriptCopyMode::TranscriptPlainText,
                        )
                        .filter(|text| !text.trim().is_empty())
                        .map(|text| KeyOutcome::CopyText {
                            text,
                            success_notice: "copied transcript",
                        })
                        .unwrap_or_else(|| {
                            self.set_status(SingleSessionStatus::Info(
                                "no transcript to copy".to_string(),
                            ));
                            KeyOutcome::Redraw
                        }),
                    _ => {
                        self.set_status(SingleSessionStatus::Info(
                            "usage: /copy [latest|code|transcript]".to_string(),
                        ));
                        KeyOutcome::Redraw
                    }
                });
            }
            "/search" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                if args.is_empty() {
                    self.set_status(SingleSessionStatus::Info(
                        "usage: /search <query>".to_string(),
                    ));
                    KeyOutcome::Redraw
                } else {
                    let matches = self.search_rich_transcript(args);
                    if let Some(first) = matches.first() {
                        let body_len = self.body_lines().len();
                        self.body_scroll_lines =
                            body_len.saturating_sub(first.line_index + 1) as f32;
                    }
                    self.set_status(SingleSessionStatus::Info(format!(
                        "{} match(es) for \"{}\"",
                        matches.len(),
                        args
                    )));
                    KeyOutcome::Redraw
                }
            }
            "/stop" | "/cancel" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                if self.is_processing {
                    KeyOutcome::CancelGeneration
                } else {
                    self.set_status(SingleSessionStatus::Info("nothing is running".to_string()));
                    KeyOutcome::Redraw
                }
            }
            "/status" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                self.show_help = false;
                self.show_session_info = true;
                self.model_picker.close();
                self.session_switcher.close();
                self.mark_inline_widget_opened();
                self.set_status(SingleSessionStatus::Info(
                    "showing session info".to_string(),
                ));
                self.scroll_body_to_bottom();
                KeyOutcome::Redraw
            }
            "/quit" | "/exit" => KeyOutcome::Exit,
            _ => {
                self.set_status(SingleSessionStatus::Info(format!(
                    "unknown desktop slash command: {command} · try /help"
                )));
                KeyOutcome::Redraw
            }
        };

        Some(outcome)
    }

    pub(crate) fn attach_image(&mut self, media_type: String, base64_data: String) {
        self.pending_images.push((media_type, base64_data));
        self.set_status(SingleSessionStatus::AttachedImages(
            self.pending_images.len(),
        ));
    }

    pub(crate) fn clear_attached_images(&mut self) -> bool {
        if self.pending_images.is_empty() {
            return false;
        }
        self.pending_images.clear();
        self.set_status(SingleSessionStatus::Info(
            "cleared image attachments".to_string(),
        ));
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
        let Some(handle) = &self.runtime.session_handle else {
            anyhow::bail!("no active desktop session to receive interactive input");
        };
        handle.send_stdin_response(request_id, input)?;
        self.set_status(SingleSessionStatus::Info(
            "interactive input sent".to_string(),
        ));
        Ok(())
    }

    fn queue_draft(&mut self) -> KeyOutcome {
        let message = self.draft.trim().to_string();
        if message.is_empty() && self.pending_images.is_empty() {
            return KeyOutcome::None;
        }
        let images = std::mem::take(&mut self.pending_images);
        self.composer.queued_drafts.push((message.clone(), images));
        self.messages.push(SingleSessionMessage::meta(format!(
            "queued prompt: {message}"
        )));
        self.draft.clear();
        self.draft_cursor = 0;
        self.composer.input_undo_stack.clear();
        self.set_status(SingleSessionStatus::Info(format!(
            "{} prompt(s) queued",
            self.composer.queued_drafts.len()
        )));
        KeyOutcome::Redraw
    }

    fn retrieve_queued_draft_for_edit(&mut self) -> KeyOutcome {
        let Some((message, images)) = self.composer.queued_drafts.pop() else {
            return KeyOutcome::None;
        };
        self.remember_input_undo_state();
        self.draft = message;
        self.draft_cursor = self.draft.len();
        self.pending_images = images;
        self.set_status(SingleSessionStatus::Info(format!(
            "{} prompt(s) queued",
            self.composer.queued_drafts.len()
        )));
        KeyOutcome::Redraw
    }

    fn cut_input_line(&mut self) -> KeyOutcome {
        if self.draft.is_empty() {
            return KeyOutcome::None;
        }
        self.remember_input_undo_state();
        let text = std::mem::take(&mut self.draft);
        self.draft_cursor = 0;
        self.set_status(SingleSessionStatus::Info("cut input line".to_string()));
        KeyOutcome::CutDraftToClipboard(text)
    }

    pub(crate) fn take_next_queued_draft(&mut self) -> Option<(String, Vec<(String, String)>)> {
        if self.is_processing || self.error.is_some() || self.composer.queued_drafts.is_empty() {
            return None;
        }
        let (message, images) = self.composer.queued_drafts.remove(0);
        self.record_user_submit(&message, &images);
        Some((message, images))
    }

    pub(crate) fn begin_selection(&mut self, point: SelectionPoint) {
        self.selection.anchor = Some(point);
        self.selection.focus = Some(point);
    }

    pub(crate) fn update_selection(&mut self, point: SelectionPoint) {
        if self.selection.anchor.is_some() {
            self.selection.focus = Some(point);
        }
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selection.anchor = None;
        self.selection.focus = None;
    }

    pub(crate) fn begin_draft_selection(&mut self, point: SelectionPoint) {
        self.clear_selection();
        self.selection.draft_anchor = Some(point);
        self.selection.draft_focus = Some(point);
        self.draft_cursor = self.draft_byte_index_for_line_col(point.line, point.column);
        self.clamp_draft_cursor();
    }

    pub(crate) fn update_draft_selection(&mut self, point: SelectionPoint) {
        if self.selection.draft_anchor.is_some() {
            self.selection.draft_focus = Some(point);
            self.draft_cursor = self.draft_byte_index_for_line_col(point.line, point.column);
            self.clamp_draft_cursor();
        }
    }

    pub(crate) fn clear_draft_selection(&mut self) {
        self.selection.draft_anchor = None;
        self.selection.draft_focus = None;
    }

    pub(crate) fn draft_selection_points(&self) -> Option<(SelectionPoint, SelectionPoint)> {
        let anchor = self.selection.draft_anchor?;
        let focus = self.selection.draft_focus?;
        if selection_point_cmp(anchor, focus).is_gt() {
            Some((focus, anchor))
        } else {
            Some((anchor, focus))
        }
    }

    pub(crate) fn draft_selection_segments(&self) -> Vec<SelectionLineSegment> {
        let lines: Vec<String> = self.draft.split('\n').map(ToString::to_string).collect();
        let Some((start, end)) = self.draft_selection_points() else {
            return Vec::new();
        };
        if start == end || start.line >= lines.len() {
            return Vec::new();
        }
        let end_line = end.line.min(lines.len().saturating_sub(1));
        let mut segments = Vec::new();
        for (line_index, line) in lines.iter().enumerate().take(end_line + 1).skip(start.line) {
            let line_len = line.chars().count();
            let prompt_columns = if line_index == 0 {
                self.composer_prompt().chars().count()
            } else {
                0
            };
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
                    start_column: start_column + prompt_columns,
                    end_column: end_column + prompt_columns,
                });
            }
        }
        segments
    }

    pub(crate) fn selected_draft_text(&mut self) -> Option<String> {
        let (start, end) = self.draft_selection_points()?;
        if start == end {
            self.clear_draft_selection();
            return None;
        }
        let start_index = self.draft_byte_index_for_line_col(start.line, start.column);
        let end_index = self.draft_byte_index_for_line_col(end.line, end.column);
        let (start_index, end_index) = if start_index <= end_index {
            (start_index, end_index)
        } else {
            (end_index, start_index)
        };
        let selected = self.draft.get(start_index..end_index).map(str::to_string);
        self.clear_draft_selection();
        selected.filter(|text| !text.is_empty())
    }

    fn draft_selection_range(&self) -> Option<(usize, usize)> {
        let (start, end) = self.draft_selection_points()?;
        if start == end {
            return None;
        }
        let start_index = self.draft_byte_index_for_line_col(start.line, start.column);
        let end_index = self.draft_byte_index_for_line_col(end.line, end.column);
        if start_index <= end_index {
            Some((start_index, end_index)).filter(|(start, end)| start != end)
        } else {
            Some((end_index, start_index)).filter(|(start, end)| start != end)
        }
    }

    fn replace_draft_selection_with(&mut self, text: &str) -> bool {
        let Some((start, end)) = self.draft_selection_range() else {
            return false;
        };
        self.remember_input_undo_state();
        self.draft.replace_range(start..end, text);
        self.draft_cursor = start + text.len();
        self.clear_draft_selection();
        true
    }

    fn delete_draft_selection(&mut self) -> bool {
        self.replace_draft_selection_with("")
    }

    pub(crate) fn selection_points(&self) -> Option<(SelectionPoint, SelectionPoint)> {
        let anchor = self.selection.anchor?;
        let focus = self.selection.focus?;
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
        for (line_index, line) in lines.iter().enumerate().take(end_line + 1).skip(start.line) {
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

    pub(crate) fn has_body_selection(&self) -> bool {
        self.selection.anchor.is_some() && self.selection.focus.is_some()
    }

    pub(crate) fn has_draft_selection(&self) -> bool {
        self.selection.draft_anchor.is_some() && self.selection.draft_focus.is_some()
    }

    pub(crate) fn selected_text_from_lines(&self, lines: &[String]) -> Option<String> {
        let (start, end) = self.selection_points()?;
        if start == end || start.line >= lines.len() {
            return None;
        }
        let end_line = end.line.min(lines.len().saturating_sub(1));
        let mut selected = Vec::new();
        for (line_index, line) in lines.iter().enumerate().take(end_line + 1).skip(start.line) {
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

    fn record_user_submit(&mut self, message: &str, images: &[(String, String)]) {
        let attachments = images
            .iter()
            .enumerate()
            .map(|(index, (media_type, base64_data))| {
                desktop_rich_text::RichAttachment::image(
                    format!("user-{}-image-{index}", self.messages.len() + 1),
                    media_type.clone(),
                    format!("attached image {}", index + 1),
                    base64_data.len(),
                )
            })
            .collect::<Vec<_>>();
        self.messages
            .push(SingleSessionMessage::user(message).with_rich_attachments(attachments));
        self.draft.clear();
        self.draft_cursor = 0;
        self.composer.input_undo_stack.clear();
        self.streaming_response.clear();
        self.scroll_body_to_bottom();
        self.set_status(SingleSessionStatus::Sending);
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

    fn collapse_active_tool_message(&mut self) {
        let Some(index) = self.tool.active_message_index.take() else {
            return;
        };
        let Some(message) = self.messages.get_mut(index) else {
            return;
        };
        if message.role() != SingleSessionRole::Tool {
            return;
        }
        if let Some(first_line) = message.content().lines().next() {
            message.set_content(first_line.replacen('▾', "▸", 1));
        }
    }

    fn append_active_tool_input(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        self.tool.input_buffer.push_str(delta);
    }

    fn flush_active_tool_input_to_message(&mut self) {
        if self.tool.input_buffer.is_empty() {
            return;
        }
        let Some(index) = self.tool.active_message_index else {
            return;
        };
        let Some(message) = self.messages.get_mut(index) else {
            return;
        };
        if message.role() != SingleSessionRole::Tool {
            return;
        }
        if !message.content().contains("\n  input: ") {
            message.content_mut().push_str("\n  input: ");
        }
        message.content_mut().push_str(&self.tool.input_buffer);
        self.tool.input_buffer.clear();
    }

    fn replace_active_tool_header(&mut self, header: &str) {
        let Some(index) = self.tool.active_message_index else {
            self.messages
                .push(SingleSessionMessage::tool(header.to_string()));
            self.tool.active_message_index = Some(self.messages.len().saturating_sub(1));
            return;
        };
        let Some(message) = self.messages.get_mut(index) else {
            self.messages
                .push(SingleSessionMessage::tool(header.to_string()));
            self.tool.active_message_index = Some(self.messages.len().saturating_sub(1));
            return;
        };
        if message.role() == SingleSessionRole::Tool {
            let replacement = merge_tool_finish_with_existing_context(message.content(), header);
            if message.content() != replacement {
                message.set_content(replacement);
            }
        }
    }

    fn insert_draft_text(&mut self, text: &str) {
        if self.replace_draft_selection_with(text) {
            return;
        }
        if !text.is_empty() {
            self.remember_input_undo_state();
        }
        self.clamp_draft_cursor();
        self.draft.insert_str(self.draft_cursor, text);
        self.draft_cursor += text.len();
    }

    fn delete_previous_char(&mut self) {
        if self.delete_draft_selection() {
            return;
        }
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
        if self.delete_draft_selection() {
            return;
        }
        self.clamp_draft_cursor();
        if self.draft_cursor >= self.draft.len() {
            return;
        }
        self.remember_input_undo_state();
        let next = next_char_boundary(&self.draft, self.draft_cursor);
        self.draft.replace_range(self.draft_cursor..next, "");
    }

    fn delete_previous_word(&mut self) {
        if self.delete_draft_selection() {
            return;
        }
        self.clamp_draft_cursor();
        let start = previous_word_start(&self.draft, self.draft_cursor);
        if start < self.draft_cursor {
            self.remember_input_undo_state();
        }
        self.draft.replace_range(start..self.draft_cursor, "");
        self.draft_cursor = start;
    }

    fn delete_next_word(&mut self) {
        if self.delete_draft_selection() {
            return;
        }
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
        self.clear_draft_selection();
    }

    fn move_cursor_right(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = next_char_boundary(&self.draft, self.draft_cursor);
        self.clear_draft_selection();
    }

    fn move_cursor_word_left(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = previous_word_start(&self.draft, self.draft_cursor);
        self.clear_draft_selection();
    }

    fn move_cursor_word_right(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = next_word_end(&self.draft, self.draft_cursor);
        self.clear_draft_selection();
    }

    fn move_to_line_start(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = line_start(&self.draft, self.draft_cursor);
        self.clear_draft_selection();
    }

    fn move_to_line_end(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = line_end(&self.draft, self.draft_cursor);
        self.clear_draft_selection();
    }

    fn delete_to_line_start(&mut self) {
        if self.delete_draft_selection() {
            return;
        }
        self.clamp_draft_cursor();
        let start = line_start(&self.draft, self.draft_cursor);
        if start < self.draft_cursor {
            self.remember_input_undo_state();
        }
        self.draft.replace_range(start..self.draft_cursor, "");
        self.draft_cursor = start;
    }

    fn delete_to_line_end(&mut self) {
        if self.delete_draft_selection() {
            return;
        }
        self.clamp_draft_cursor();
        let end = line_end(&self.draft, self.draft_cursor);
        if end > self.draft_cursor {
            self.remember_input_undo_state();
        }
        self.draft.replace_range(self.draft_cursor..end, "");
    }

    fn clear_draft_for_escape(&mut self) -> KeyOutcome {
        if self.draft.is_empty() {
            return KeyOutcome::None;
        }
        self.remember_input_undo_state();
        self.draft.clear();
        self.draft_cursor = 0;
        self.clear_draft_selection();
        if self.model_picker.open && self.model_picker.preview {
            self.model_picker.close();
        }
        self.set_status(SingleSessionStatus::Info(
            "Input cleared - Ctrl+Z to restore".to_string(),
        ));
        KeyOutcome::Redraw
    }

    fn autocomplete_draft(&mut self) -> KeyOutcome {
        const DESKTOP_SLASH_COMPLETIONS: &[&str] = &[
            "/help",
            "/clear",
            "/new",
            "/sessions",
            "/model",
            "/copy",
            "/search",
            "/stop",
            "/status",
            "/quit",
        ];
        let Some((draft, cursor)) =
            complete_slash_command(&self.draft, self.draft_cursor, DESKTOP_SLASH_COMPLETIONS)
        else {
            return KeyOutcome::None;
        };
        self.remember_input_undo_state();
        self.draft = draft;
        self.draft_cursor = cursor;
        self.clear_draft_selection();
        self.sync_model_picker_preview_from_draft()
            .unwrap_or(KeyOutcome::Redraw)
    }

    fn remember_input_undo_state(&mut self) {
        if self
            .composer
            .input_undo_stack
            .last()
            .is_some_and(|(draft, cursor)| draft == &self.draft && *cursor == self.draft_cursor)
        {
            return;
        }
        self.composer
            .input_undo_stack
            .push((self.draft.clone(), self.draft_cursor));
        const MAX_UNDO: usize = 64;
        if self.composer.input_undo_stack.len() > MAX_UNDO {
            self.composer.input_undo_stack.remove(0);
        }
    }

    fn undo_input_change(&mut self) {
        if let Some((draft, cursor)) = self.composer.input_undo_stack.pop() {
            self.draft = draft;
            self.draft_cursor = cursor.min(self.draft.len());
            self.clamp_draft_cursor();
            self.clear_draft_selection();
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

fn scroll_status_fragment(scroll_lines: f32) -> String {
    if !scroll_lines.is_finite() || scroll_lines < 0.05 {
        return String::new();
    }
    if (scroll_lines - 1.0).abs() < 0.05 {
        return " · scrolled up 1 line".to_string();
    }
    let rounded = (scroll_lines * 10.0).round() / 10.0;
    if (rounded - rounded.round()).abs() < 0.05 {
        format!(" · scrolled up {} lines", rounded.round() as usize)
    } else {
        format!(" · scrolled up {rounded:.1} lines")
    }
}

fn blank_styled_line() -> SingleSessionStyledLine {
    styled_line(String::new(), SingleSessionLineStyle::Blank)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn welcome_styled_lines(
    name: &Option<String>,
    tick: u64,
    recovery_session_count: usize,
) -> Vec<SingleSessionStyledLine> {
    let greeting = welcome_greeting_text(name, 0);
    let prompts = [
        "Start with a prompt",
        "Ask anything",
        "Ready when you are",
        "Enter sends · Shift+Enter adds a line",
    ];
    let prompt = prompts[((tick / 42) as usize) % prompts.len()];
    let ellipsis = match (tick / 14) % 4 {
        0 => "",
        1 => ".",
        2 => "..",
        _ => "...",
    };

    let mut lines = vec![
        styled_line(greeting, SingleSessionLineStyle::AssistantHeading),
        blank_styled_line(),
        styled_line(
            format!("{prompt}{ellipsis}"),
            SingleSessionLineStyle::Status,
        ),
        styled_line("Ctrl+P opens recent sessions", SingleSessionLineStyle::Meta),
    ];

    if recovery_session_count > 0 {
        lines.push(blank_styled_line());
        lines.push(styled_line(
            format!(
                "Found {recovery_session_count} crashed session(s). Press Ctrl+R to open them in new windows."
            ),
            SingleSessionLineStyle::Status,
        ));
    }

    lines
}

fn welcome_recovery_styled_lines(recovery_session_count: usize) -> Vec<SingleSessionStyledLine> {
    vec![styled_line(
        format!(
            "Found {recovery_session_count} crashed session(s). Press Ctrl+R to open them in new windows."
        ),
        SingleSessionLineStyle::Status,
    )]
}

fn welcome_greeting_text(name: &Option<String>, phrase_index: usize) -> String {
    name.as_deref()
        .map(|name| format!("Welcome, {name}"))
        .unwrap_or_else(|| handwritten_welcome_phrase(phrase_index).to_string())
}

pub(crate) fn handwritten_welcome_phrase(index: usize) -> &'static str {
    HANDWRITTEN_WELCOME_PHRASES[index % HANDWRITTEN_WELCOME_PHRASES.len()]
}

fn welcome_phrase_index(name: &Option<String>) -> usize {
    let time_seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos() as usize)
        .unwrap_or(0);
    let name_seed = name
        .as_deref()
        .unwrap_or_default()
        .bytes()
        .fold(0usize, |hash, byte| {
            hash.wrapping_mul(31).wrapping_add(byte as usize)
        });
    (time_seed ^ name_seed) % HANDWRITTEN_WELCOME_PHRASES.len()
}

#[cfg(any(target_os = "macos", windows))]
fn desktop_welcome_name() -> Option<String> {
    sanitize_welcome_name(&whoami::realname())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn desktop_welcome_name() -> Option<String> {
    None
}

#[cfg_attr(not(any(test, target_os = "macos", windows)), allow(dead_code))]
pub(crate) fn sanitize_welcome_name(raw: &str) -> Option<String> {
    let name = raw
        .trim()
        .trim_matches(|ch: char| ch == ',' || ch == ';')
        .split_whitespace()
        .next()?;
    if name.is_empty() || name.eq_ignore_ascii_case("unknown") {
        return None;
    }
    Some(name.to_string())
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
            "Enter send · Ctrl+Enter send · Shift+Enter newline · Ctrl+V paste · Ctrl+U clear · Esc cancel",
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
            "↑/↓ select · PgUp/PgDn jump · Home/End top/bottom · type filter · Enter resume · Ctrl+R reload · Ctrl+P/Esc close",
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

fn session_info_inline_styled_lines(app: &SingleSessionApp) -> Vec<SingleSessionStyledLine> {
    let (user_count, assistant_count, tool_count, system_count, meta_count) =
        session_message_role_counts(&app.messages);
    let session_id = app
        .current_session_id()
        .map(|id| format!("{} ({})", short_session_id(id), id))
        .unwrap_or_else(|| "fresh / not started".to_string());
    let model = model_picker_current_label(
        app.model_picker.provider_name.as_deref(),
        app.model_picker.current_model.as_deref(),
    );
    let status = app.status.as_deref().unwrap_or("ready");
    let transcript_chars: usize = app
        .messages
        .iter()
        .map(|message| message.content().len())
        .sum();
    let streaming_chars = app.streaming_response.len();
    let streaming_lines = app.streaming_response.lines().count();
    let body_lines = app.body_styled_lines_without_inline_widgets().len();
    let selection = if app.has_body_selection() || app.has_draft_selection() {
        "active"
    } else {
        "none"
    };
    let stdin = app
        .stdin_response
        .as_ref()
        .map(|state| {
            if state.is_password {
                "password requested"
            } else {
                "input requested"
            }
        })
        .unwrap_or("none");
    let active_tool = app
        .tool
        .active_message_index
        .map(|index| format!("message #{index}"))
        .unwrap_or_else(|| "none".to_string());

    let mut lines = vec![
        styled_line(
            "╭─ session info · Ctrl+Shift+S/Esc close",
            SingleSessionLineStyle::OverlayTitle,
        ),
        styled_line(
            format!("│ title        {}", compact_tool_text(&app.title(), 92)),
            SingleSessionLineStyle::Overlay,
        ),
        styled_line(
            format!("│ session id   {}", compact_tool_text(&session_id, 92)),
            SingleSessionLineStyle::Overlay,
        ),
        styled_line(
            format!("│ status       {}", compact_tool_text(status, 92)),
            SingleSessionLineStyle::Status,
        ),
        styled_line(
            format!("│ model        {}", compact_tool_text(&model, 92)),
            SingleSessionLineStyle::Overlay,
        ),
        styled_line(
            format!(
                "│ work         {} · worker {} · active tool {}",
                if app.is_processing { "running" } else { "idle" },
                if app.runtime.session_handle.is_some() {
                    "attached"
                } else {
                    "none"
                },
                active_tool
            ),
            SingleSessionLineStyle::Overlay,
        ),
        styled_line(
            format!(
                "│ messages     {} total · {user_count} user · {assistant_count} assistant · {tool_count} tool · {system_count} system · {meta_count} meta",
                app.messages.len()
            ),
            SingleSessionLineStyle::Overlay,
        ),
        styled_line(
            format!(
                "│ transcript   {body_lines} visible lines · {transcript_chars} chars · streaming {streaming_chars} chars/{streaming_lines} lines"
            ),
            SingleSessionLineStyle::Overlay,
        ),
        styled_line(
            format!(
                "│ composer     prompt #{} · draft {} chars · {} image(s) · {} queued · stdin {}",
                app.next_prompt_number(),
                app.draft.len(),
                app.pending_images.len(),
                app.composer.queued_drafts.len(),
                stdin
            ),
            SingleSessionLineStyle::Overlay,
        ),
        styled_line(
            format!(
                "│ viewport     scroll {} · text scale {:.0}% · selection {} · welcome {}",
                scroll_status_fragment(app.body_scroll_lines).trim_start_matches(" · "),
                app.view.text_scale * 100.0,
                selection,
                if app.is_welcome_timeline_visible() {
                    "visible"
                } else {
                    "hidden"
                }
            ),
            SingleSessionLineStyle::Overlay,
        ),
        styled_line(
            "│ tokens       not yet emitted by desktop stream; showing local transcript stats instead",
            SingleSessionLineStyle::Meta,
        ),
    ];

    if let Some(session) = &app.session {
        if !session.subtitle.trim().is_empty() {
            lines.push(styled_line(
                format!(
                    "│ subtitle     {}",
                    compact_tool_text(&session.subtitle, 92)
                ),
                SingleSessionLineStyle::Meta,
            ));
        }
        if !session.detail.trim().is_empty() {
            lines.push(styled_line(
                format!("│ detail       {}", compact_tool_text(&session.detail, 92)),
                SingleSessionLineStyle::Meta,
            ));
        }
    }

    if let Some(error) = &app.error {
        lines.push(styled_line(
            format!("│ error        {}", compact_tool_text(error, 92)),
            SingleSessionLineStyle::Error,
        ));
    }

    lines.push(styled_line(
        "╰─ /status opens this panel",
        SingleSessionLineStyle::Overlay,
    ));
    lines
}

fn session_message_role_counts(
    messages: &[SingleSessionMessage],
) -> (usize, usize, usize, usize, usize) {
    let mut user = 0;
    let mut assistant = 0;
    let mut tool = 0;
    let mut system = 0;
    let mut meta = 0;
    for message in messages {
        match message.role() {
            SingleSessionRole::User => user += 1,
            SingleSessionRole::Assistant => assistant += 1,
            SingleSessionRole::Tool => tool += 1,
            SingleSessionRole::System => system += 1,
            SingleSessionRole::Meta => meta += 1,
        }
    }
    (user, assistant, tool, system, meta)
}

fn model_picker_inline_styled_lines(picker: &ModelPickerState) -> Vec<SingleSessionStyledLine> {
    let visible = picker.filtered_indices();
    let count = if visible.len() == picker.choices.len() {
        format!("{} models", picker.choices.len())
    } else {
        format!("{} of {} models", visible.len(), picker.choices.len())
    };
    let filter = if picker.filter.trim().is_empty() {
        "type to filter".to_string()
    } else {
        format!("filter \"{}\"", truncate_chars(picker.filter.trim(), 28))
    };
    let mut lines = vec![
        styled_line(
            format!(
                "Model picker    current {}",
                model_picker_current_label(
                    picker.provider_name.as_deref(),
                    picker.current_model.as_deref(),
                )
            ),
            SingleSessionLineStyle::OverlayTitle,
        ),
        styled_line(
            format!("{filter}    {count}"),
            SingleSessionLineStyle::Overlay,
        ),
    ];

    if picker.loading {
        lines.push(styled_line(
            "Loading models from shared server...",
            SingleSessionLineStyle::Status,
        ));
    }

    if let Some(error) = &picker.error {
        lines.push(styled_line(
            format!("Error: {error}"),
            SingleSessionLineStyle::Error,
        ));
    }

    if visible.is_empty() && !picker.loading {
        lines.push(styled_line(
            "No matching models",
            SingleSessionLineStyle::Status,
        ));
        lines.push(styled_line(
            "Clear the filter or press Ctrl+R to reload",
            SingleSessionLineStyle::Overlay,
        ));
        return lines;
    }

    let current = picker.current_model.as_deref();
    let (window_start, window) = picker.visible_row_window(MODEL_PICKER_INLINE_ROW_LIMIT);
    for (row_offset, index) in window.iter().enumerate() {
        let Some(choice) = picker.choices.get(*index) else {
            continue;
        };
        let visible_position = window_start + row_offset;
        let selector = if visible_position == picker.selected {
            "›"
        } else {
            " "
        };
        let provider = choice.provider.as_deref().unwrap_or("auto");
        let method = choice.api_method.as_deref().unwrap_or("auto");
        let current_badge = if Some(choice.model.as_str()) == current {
            "  Current"
        } else {
            ""
        };
        let availability = if choice.available {
            "available"
        } else {
            "unavailable"
        };
        let detail = choice
            .detail
            .as_deref()
            .filter(|detail| !detail.is_empty())
            .unwrap_or(availability);
        let row_style = if visible_position == picker.selected {
            SingleSessionLineStyle::OverlaySelection
        } else {
            SingleSessionLineStyle::Overlay
        };
        lines.push(styled_line(
            format!(
                "{selector} {}{}",
                truncate_chars(&choice.model, 54),
                current_badge,
            ),
            row_style,
        ));
        lines.push(styled_line(
            format!(
                "  {} · {} · {}",
                truncate_chars(provider, 22),
                truncate_chars(method, 18),
                truncate_chars(detail, 42),
            ),
            SingleSessionLineStyle::Meta,
        ));
    }
    if visible.len() > window_start + window.len() {
        lines.push(styled_line(
            format!(
                "… {} more models",
                visible.len() - window_start - window.len()
            ),
            SingleSessionLineStyle::Overlay,
        ));
    }
    let footer = if picker.preview {
        "↑↓/PgUp/PgDn select   Home/End top/bottom   Enter use model   Esc clear /model"
    } else {
        "↑↓/PgUp/PgDn select   Home/End top/bottom   Type filter   Enter use   Esc close"
    };
    lines.push(styled_line(footer, SingleSessionLineStyle::Overlay));

    lines
}

fn model_picker_preview_filter(input: &str) -> Option<String> {
    let trimmed = input.trim_start();
    let rest = trimmed
        .strip_prefix("/model")
        .or_else(|| trimmed.strip_prefix("/models"))?;
    if rest.is_empty() {
        return Some(String::new());
    }
    rest.chars()
        .next()
        .filter(|ch| ch.is_whitespace())
        .map(|_| rest.trim_start().to_string())
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    if max_chars == 0 {
        return String::new();
    }
    if max_chars == 1 {
        return "…".to_string();
    }
    format!("{}…", text.chars().take(max_chars - 1).collect::<String>())
}

fn model_picker_current_label(provider_name: Option<&str>, current_model: Option<&str>) -> String {
    match (provider_name, current_model) {
        (Some(provider), Some(model)) if !provider.is_empty() => format!("{provider} · {model}"),
        (_, Some(model)) => model.to_string(),
        (Some(provider), None) if !provider.is_empty() => provider.to_string(),
        _ => "unknown".to_string(),
    }
}

fn model_choice_search_text(choice: &DesktopModelChoice) -> String {
    format!(
        "{} {} {} {}",
        choice.model,
        choice.provider.as_deref().unwrap_or_default(),
        choice.api_method.as_deref().unwrap_or_default(),
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
                && existing.api_method == choice.api_method
                && existing.detail == choice.detail
        }) {
            continue;
        }
        deduped.push(choice);
    }
    deduped
}

struct HelpSection {
    title: &'static str,
    shortcuts: &'static [(&'static str, &'static str)],
}

const SINGLE_SESSION_HELP_SECTIONS: &[HelpSection] = &[
    HelpSection {
        title: "chat",
        shortcuts: &[
            ("Enter", "send prompt"),
            ("Shift/Alt+Enter", "insert newline"),
            ("Ctrl+Enter", "queue while running, send when idle"),
            ("Esc", "interrupt running generation"),
            ("Ctrl+C/D", "interrupt running generation"),
            ("Ctrl+Shift+C", "copy latest assistant response"),
            ("Ctrl+Shift+K", "copy latest code block"),
            ("Ctrl+Shift+T", "copy transcript"),
            ("Ctrl+V", "paste clipboard text"),
            ("Ctrl+V", "paste clipboard image when no text is present"),
            ("Alt+V", "attach clipboard image, terminal-style"),
            ("Ctrl+I", "attach clipboard image to next prompt"),
            ("Ctrl+Shift+I", "clear pending image attachments"),
            ("Ctrl+Shift+M", "open model/account picker"),
            ("Ctrl+M/N", "switch to next/previous model"),
            ("Ctrl+Tab", "switch to next model"),
            ("Ctrl+Shift+Tab", "switch to previous model"),
            ("Ctrl+P/O", "open recent session switcher"),
            ("Ctrl+Shift+S", "toggle inline session info/stats"),
        ],
    },
    HelpSection {
        title: "navigation",
        shortcuts: &[
            ("Ctrl+Up", "pull latest queued prompt back into the input"),
            ("PageUp/PageDown", "scroll transcript"),
            ("Ctrl+Home/End", "jump transcript to top/bottom"),
            ("Super+K/J", "scroll transcript by one line"),
            ("Alt+Up/Down", "jump between user prompts"),
            ("Ctrl+[/]", "jump between user prompts"),
            ("Mouse wheel", "scroll transcript"),
        ],
    },
    HelpSection {
        title: "editing",
        shortcuts: &[
            ("Ctrl+A/E", "start/end of line"),
            ("Ctrl+U/K", "delete to line start/end"),
            ("Ctrl+W/Ctrl+Backspace", "delete previous word"),
            ("Alt+Backspace", "delete previous word, terminal-style"),
            ("Ctrl/Alt+←/→, Ctrl+B/F", "move by word"),
            ("Alt+B/F", "move by word, terminal-style"),
            ("Alt+D", "delete next word"),
            ("Tab", "complete slash command suggestion"),
            ("↑/↓ PgUp/PgDn", "navigate slash suggestions"),
            ("Ctrl+X", "cut input line to clipboard"),
            ("Ctrl+Z", "undo input edit"),
        ],
    },
    HelpSection {
        title: "window",
        shortcuts: &[
            ("Ctrl+;", "reset/spawn fresh desktop session"),
            ("Ctrl+R", "reload sessions/models while a picker is open"),
            ("Ctrl+?", "toggle this help"),
            ("q", "close help or session info"),
            ("Ctrl+Q/Super+Q", "quit desktop app"),
            ("Esc", "close help; interrupt while running; idle no-op"),
        ],
    },
];

fn single_session_help_styled_lines() -> Vec<SingleSessionStyledLine> {
    let mut lines = Vec::new();

    lines.push(styled_line(
        "slash commands",
        SingleSessionLineStyle::OverlayTitle,
    ));
    lines.extend(DESKTOP_SLASH_COMMANDS.iter().map(|(command, description)| {
        let separator = if command.len() >= 16 { " " } else { "" };
        styled_line(
            format!("  {command:<16}{separator}{description}"),
            SingleSessionLineStyle::Overlay,
        )
    }));

    for (section_index, section) in SINGLE_SESSION_HELP_SECTIONS.iter().enumerate() {
        let _ = section_index;
        lines.push(blank_styled_line());
        lines.push(styled_line(
            section.title,
            SingleSessionLineStyle::OverlayTitle,
        ));
        lines.extend(section.shortcuts.iter().map(|(shortcut, description)| {
            let separator = if shortcut.len() >= 12 { " " } else { "" };
            styled_line(
                format!("  {shortcut:<12}{separator}{description}"),
                SingleSessionLineStyle::Overlay,
            )
        }));
    }

    lines
}

fn hotkey_help_inline_widget() -> ReadOnlyInlineWidget {
    ReadOnlyInlineWidget::new("desktop shortcuts", single_session_help_styled_lines())
}

fn append_chat_message_lines(
    lines: &mut Vec<SingleSessionStyledLine>,
    message: &SingleSessionMessage,
    user_turn: &mut usize,
    is_active_tool: bool,
    active_tool_input: Option<&str>,
) {
    match message.role() {
        SingleSessionRole::User => {
            append_user_lines(lines, *user_turn, message.content().trim());
            *user_turn += 1;
        }
        SingleSessionRole::Assistant => append_assistant_lines(lines, message.content().trim()),
        SingleSessionRole::Tool => append_tool_lines(
            lines,
            message.content().trim(),
            is_active_tool,
            active_tool_input,
        ),
        SingleSessionRole::System | SingleSessionRole::Meta => {
            append_meta_lines(lines, message.content().trim())
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

fn append_streaming_assistant_lines(lines: &mut Vec<SingleSessionStyledLine>, content: &str) {
    lines.extend(render_assistant_markdown_lines(content));
}

fn take_current_inline_spans(
    inline_spans: &mut Vec<SingleSessionInlineSpan>,
    trimmed_len: usize,
) -> Vec<SingleSessionInlineSpan> {
    let mut spans = std::mem::take(inline_spans);
    spans = spans
        .into_iter()
        .filter_map(|span| {
            let start = span.start.min(trimmed_len);
            let end = span.end.min(trimmed_len);
            (start < end).then_some(SingleSessionInlineSpan {
                start,
                end,
                kind: span.kind,
            })
        })
        .collect();
    spans.sort_by_key(|span| (span.start, span.end));
    spans
}

fn safe_utf8_prefix_len(text: &str, desired_len: usize) -> usize {
    let mut len = desired_len.min(text.len());
    while len > 0 && !text.is_char_boundary(len) {
        len -= 1;
    }
    len
}

pub(crate) fn single_session_trimmed_line_end_preserving_inline_code_whitespace(
    text: &str,
    inline_spans: &[SingleSessionInlineSpan],
) -> usize {
    let trimmed_len = text.trim_end().len();
    let inline_code_end = inline_spans
        .iter()
        .filter(|span| span.kind == SingleSessionInlineSpanKind::Code)
        .filter_map(|span| {
            let end = span.end.min(text.len());
            (end > trimmed_len && text.is_char_boundary(end)).then_some(end)
        })
        .max()
        .unwrap_or(trimmed_len);

    trimmed_len.max(inline_code_end)
}

fn render_assistant_markdown_lines(content: &str) -> Vec<SingleSessionStyledLine> {
    let markdown_options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM
        | Options::ENABLE_DEFINITION_LIST;
    let mut renderer = AssistantMarkdownRenderer::default();

    for event in Parser::new_ext(content, markdown_options) {
        renderer.handle_event(event);
    }

    let mut lines = renderer.finish();
    if lines.is_empty() && !content.trim().is_empty() {
        lines.extend(
            content
                .lines()
                .map(|line| styled_line(line, SingleSessionLineStyle::Assistant)),
        );
    }
    lines
}

#[derive(Default)]
struct AssistantMarkdownRenderer {
    lines: Vec<SingleSessionStyledLine>,
    current: String,
    current_inline_spans: Vec<SingleSessionInlineSpan>,
    active_inline_spans: Vec<AssistantMarkdownActiveInlineSpan>,
    current_style: SingleSessionLineStyle,
    line_style_override: Option<SingleSessionLineStyle>,
    quote_depth: usize,
    list_stack: Vec<AssistantMarkdownList>,
    item_continuation_prefixes: Vec<String>,
    pending_line_prefix: String,
    continuation_prefix: String,
    in_code_block: bool,
    in_footnote_definition: bool,
    table: Option<AssistantMarkdownTable>,
    image_stack: Vec<AssistantMarkdownImage>,
    link_stack: Vec<AssistantMarkdownLink>,
}

#[derive(Clone, Copy, Debug)]
struct AssistantMarkdownActiveInlineSpan {
    kind: SingleSessionInlineSpanKind,
    start: usize,
}

#[derive(Clone, Debug)]
struct AssistantMarkdownList {
    next_number: Option<u64>,
}

#[derive(Clone, Debug)]
struct AssistantMarkdownLink {
    dest_url: String,
    start_byte: usize,
}

#[derive(Clone, Debug, Default)]
struct AssistantMarkdownImage {
    dest_url: String,
    alt_text: String,
}

#[derive(Clone, Debug, Default)]
struct AssistantMarkdownTable {
    rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    current_cell: String,
    header_rows: usize,
    alignments: Vec<Alignment>,
}

impl AssistantMarkdownRenderer {
    fn handle_event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => self.start_heading(level),
            Event::End(TagEnd::Heading(_)) => self.end_heading(),
            Event::Start(Tag::Paragraph) => self.start_paragraph(),
            Event::End(TagEnd::Paragraph) => self.end_paragraph(),
            Event::Start(Tag::BlockQuote(kind)) => self.start_block_quote(kind),
            Event::End(TagEnd::BlockQuote(_)) => self.end_block_quote(),
            Event::Start(Tag::List(start)) => self.start_list(start),
            Event::End(TagEnd::List(_)) => self.end_list(),
            Event::Start(Tag::Item) => self.start_list_item(),
            Event::End(TagEnd::Item) => self.end_list_item(),
            Event::Start(Tag::FootnoteDefinition(label)) => {
                self.start_footnote_definition(label.as_ref())
            }
            Event::End(TagEnd::FootnoteDefinition) => self.end_footnote_definition(),
            Event::Start(Tag::DefinitionList) => self.start_definition_list(),
            Event::End(TagEnd::DefinitionList) => self.end_definition_list(),
            Event::Start(Tag::DefinitionListTitle) => self.start_definition_list_title(),
            Event::End(TagEnd::DefinitionListTitle) => self.end_definition_list_title(),
            Event::Start(Tag::DefinitionListDefinition) => self.start_definition_list_definition(),
            Event::End(TagEnd::DefinitionListDefinition) => self.end_definition_list_definition(),
            Event::TaskListMarker(checked) => self.apply_task_marker(checked),
            Event::Start(Tag::CodeBlock(kind)) => self.start_code_block(kind),
            Event::End(TagEnd::CodeBlock) => self.end_code_block(),
            Event::Start(Tag::Table(alignments)) => self.start_table(alignments),
            Event::End(TagEnd::Table) => self.end_table(),
            Event::Start(Tag::TableHead) => self.start_table_head(),
            Event::End(TagEnd::TableHead) => self.end_table_head(),
            Event::Start(Tag::TableRow) => self.start_table_row(),
            Event::End(TagEnd::TableRow) => self.end_table_row(),
            Event::Start(Tag::TableCell) => self.start_table_cell(),
            Event::End(TagEnd::TableCell) => self.end_table_cell(),
            Event::Start(Tag::Link { dest_url, .. }) => self.start_link(dest_url.as_ref()),
            Event::End(TagEnd::Link) => self.end_link(),
            Event::Start(Tag::Image { dest_url, .. }) => self.start_image(dest_url.as_ref()),
            Event::End(TagEnd::Image) => self.end_image(),
            Event::Start(Tag::Emphasis) => {
                self.start_inline_span(SingleSessionInlineSpanKind::Emphasis)
            }
            Event::End(TagEnd::Emphasis) => {
                self.end_inline_span(SingleSessionInlineSpanKind::Emphasis)
            }
            Event::Start(Tag::Strong) => {
                self.start_inline_span(SingleSessionInlineSpanKind::Strong)
            }
            Event::End(TagEnd::Strong) => self.end_inline_span(SingleSessionInlineSpanKind::Strong),
            Event::Start(Tag::Strikethrough) => {
                self.start_inline_span(SingleSessionInlineSpanKind::Strike)
            }
            Event::End(TagEnd::Strikethrough) => {
                self.end_inline_span(SingleSessionInlineSpanKind::Strike)
            }
            Event::Text(text) => self.push_text(text.as_ref()),
            Event::Code(code) => self.push_inline_code(code.as_ref()),
            Event::InlineMath(math) => self.push_inline_math(math.as_ref()),
            Event::DisplayMath(math) => self.push_display_math(math.as_ref()),
            Event::SoftBreak => self.soft_break(),
            Event::HardBreak => self.hard_break(),
            Event::Rule => self.rule(),
            Event::Html(html) => self.push_html_block(html.as_ref()),
            Event::InlineHtml(html) => self.push_inline_code(html.as_ref()),
            Event::FootnoteReference(name) => {
                self.push_text("[^");
                self.push_text(name.as_ref());
                self.push_text("]");
            }
            _ => {}
        }
    }

    fn finish(mut self) -> Vec<SingleSessionStyledLine> {
        self.flush_current_line();
        if self
            .lines
            .last()
            .is_some_and(|line| line.style == SingleSessionLineStyle::Blank)
        {
            self.lines.pop();
        }
        self.lines
    }

    fn start_heading(&mut self, level: HeadingLevel) {
        self.flush_current_line();
        self.ensure_block_gap();
        self.current_style = SingleSessionLineStyle::AssistantHeading;
        self.pending_line_prefix = heading_prefix(level).to_string();
    }

    fn end_heading(&mut self) {
        self.flush_current_line_as(SingleSessionLineStyle::AssistantHeading);
        self.current_style = self.prose_style();
        self.pending_line_prefix.clear();
    }

    fn start_paragraph(&mut self) {
        if self.list_stack.is_empty() && self.quote_depth == 0 {
            self.ensure_block_gap();
        }
        self.current_style = self.prose_style();
    }

    fn end_paragraph(&mut self) {
        self.flush_current_line();
        if !self.item_continuation_prefixes.is_empty() {
            self.pending_line_prefix = self.continuation_prefix.clone();
        }
    }

    fn start_block_quote(&mut self, kind: Option<BlockQuoteKind>) {
        self.flush_current_line();
        self.ensure_block_gap();
        let parent_quote_prefix = self.quote_prefix();
        self.quote_depth += 1;
        self.current_style = SingleSessionLineStyle::AssistantQuote;
        if let Some(kind) = kind {
            self.pending_line_prefix =
                format!("{parent_quote_prefix}{} │ ", block_quote_kind_label(kind));
        }
    }

    fn end_block_quote(&mut self) {
        self.flush_current_line_as(SingleSessionLineStyle::AssistantQuote);
        self.quote_depth = self.quote_depth.saturating_sub(1);
        self.current_style = self.prose_style();
        self.pending_line_prefix.clear();
        self.continuation_prefix.clear();
    }

    fn start_list(&mut self, start: Option<u64>) {
        self.flush_current_line();
        if self.list_stack.is_empty() && self.quote_depth == 0 {
            self.ensure_block_gap();
        }
        self.list_stack
            .push(AssistantMarkdownList { next_number: start });
    }

    fn end_list(&mut self) {
        self.flush_current_line();
        self.list_stack.pop();
        if self.list_stack.is_empty() {
            self.pending_line_prefix.clear();
            self.continuation_prefix.clear();
            self.item_continuation_prefixes.clear();
        }
    }

    fn start_list_item(&mut self) {
        self.flush_current_line();
        let (prefix, continuation) = self.list_item_prefix(false);
        self.pending_line_prefix = prefix;
        self.continuation_prefix = continuation.clone();
        self.item_continuation_prefixes.push(continuation);
        self.current_style = self.prose_style();
    }

    fn end_list_item(&mut self) {
        self.flush_current_line();
        self.item_continuation_prefixes.pop();
        self.continuation_prefix = self
            .item_continuation_prefixes
            .last()
            .cloned()
            .unwrap_or_default();
        self.pending_line_prefix.clear();
    }

    fn apply_task_marker(&mut self, checked: bool) {
        let (prefix, continuation) = self.task_item_prefix(checked);
        if self.current.is_empty() {
            self.pending_line_prefix = prefix;
            self.continuation_prefix = continuation.clone();
            if let Some(last) = self.item_continuation_prefixes.last_mut() {
                *last = continuation;
            }
        } else {
            self.current.push_str(if checked { "✓ " } else { "☐ " });
        }
    }

    fn start_footnote_definition(&mut self, label: &str) {
        self.flush_current_line();
        self.ensure_block_gap();
        self.in_footnote_definition = true;
        self.current_style = SingleSessionLineStyle::Meta;
        self.pending_line_prefix = format!("[^{label}]: ");
    }

    fn end_footnote_definition(&mut self) {
        self.flush_current_line_as(SingleSessionLineStyle::Meta);
        self.in_footnote_definition = false;
        self.current_style = self.prose_style();
        self.pending_line_prefix.clear();
    }

    fn start_definition_list(&mut self) {
        self.flush_current_line();
        self.ensure_block_gap();
    }

    fn end_definition_list(&mut self) {
        self.flush_current_line();
        self.pending_line_prefix.clear();
        self.current_style = self.prose_style();
    }

    fn start_definition_list_title(&mut self) {
        self.flush_current_line();
        self.current_style = SingleSessionLineStyle::AssistantHeading;
    }

    fn end_definition_list_title(&mut self) {
        self.flush_current_line_as(SingleSessionLineStyle::AssistantHeading);
        self.current_style = self.prose_style();
    }

    fn start_definition_list_definition(&mut self) {
        self.flush_current_line();
        self.current_style = self.prose_style();
        self.pending_line_prefix = "  : ".to_string();
    }

    fn end_definition_list_definition(&mut self) {
        self.flush_current_line();
        self.pending_line_prefix.clear();
    }

    fn start_code_block(&mut self, kind: CodeBlockKind<'_>) {
        self.flush_current_line();
        self.ensure_block_gap();
        self.in_code_block = true;
        if let CodeBlockKind::Fenced(language) = kind {
            let language = language.as_ref().trim();
            if !language.is_empty() {
                self.lines.push(styled_line(
                    format!("  {language}"),
                    SingleSessionLineStyle::CodeHeader,
                ));
            }
        }
    }

    fn end_code_block(&mut self) {
        self.in_code_block = false;
    }

    fn start_table(&mut self, alignments: Vec<Alignment>) {
        self.flush_current_line();
        self.ensure_block_gap();
        self.table = Some(AssistantMarkdownTable {
            alignments,
            ..AssistantMarkdownTable::default()
        });
    }

    fn end_table(&mut self) {
        if let Some(table) = self.table.take() {
            self.render_table(table);
        }
    }

    fn start_table_head(&mut self) {}

    fn end_table_head(&mut self) {
        if let Some(table) = &mut self.table {
            if !table.current_cell.trim().is_empty() {
                table.finish_cell();
            }
            table.finish_row();
            table.header_rows = table.rows.len();
        }
    }

    fn start_table_row(&mut self) {
        if let Some(table) = &mut self.table {
            table.current_row.clear();
        }
    }

    fn end_table_row(&mut self) {
        if let Some(table) = &mut self.table {
            if !table.current_cell.trim().is_empty() {
                table.finish_cell();
            }
            table.finish_row();
        }
    }

    fn start_table_cell(&mut self) {
        if let Some(table) = &mut self.table {
            table.current_cell.clear();
        }
    }

    fn end_table_cell(&mut self) {
        if let Some(table) = &mut self.table {
            table.finish_cell();
        }
    }

    fn start_link(&mut self, dest_url: &str) {
        self.begin_line_if_needed();
        self.link_stack.push(AssistantMarkdownLink {
            dest_url: dest_url.to_string(),
            start_byte: self.current.len(),
        });
    }

    fn end_link(&mut self) {
        let Some(link) = self.link_stack.pop() else {
            return;
        };
        if link.dest_url.is_empty() {
            return;
        }
        self.begin_line_if_needed();
        let label = self
            .current
            .get(link.start_byte..)
            .unwrap_or_default()
            .trim();
        if !label.contains(&link.dest_url) {
            self.current.push_str(" ↗ ");
            self.current.push_str(&link.dest_url);
        }
        if self.current_style == SingleSessionLineStyle::Assistant {
            self.line_style_override = Some(SingleSessionLineStyle::AssistantLink);
        }
    }

    fn start_image(&mut self, dest_url: &str) {
        self.image_stack.push(AssistantMarkdownImage {
            dest_url: dest_url.to_string(),
            alt_text: String::new(),
        });
    }

    fn end_image(&mut self) {
        let Some(image) = self.image_stack.pop() else {
            return;
        };
        self.begin_line_if_needed();
        let alt = image.alt_text.trim();
        if alt.is_empty() {
            self.current.push_str("🖼 image");
        } else {
            self.current.push_str("🖼 ");
            self.current.push_str(alt);
        }
        if !image.dest_url.is_empty() {
            self.current.push_str(" ↗ ");
            self.current.push_str(&image.dest_url);
        }
        if self.current_style == SingleSessionLineStyle::Assistant {
            self.line_style_override = Some(SingleSessionLineStyle::AssistantLink);
        }
    }

    fn push_text(&mut self, text: &str) {
        if let Some(image) = self.image_stack.last_mut() {
            image.alt_text.push_str(text);
            return;
        }
        if let Some(table) = &mut self.table {
            table.push_text(text);
            return;
        }
        if self.in_code_block {
            self.push_code_text(text);
            return;
        }
        self.begin_line_if_needed();
        self.current.push_str(&text.replace('\n', " "));
    }

    fn push_inline_code(&mut self, code: &str) {
        if let Some(image) = self.image_stack.last_mut() {
            image.alt_text.push_str(code);
            return;
        }
        if let Some(table) = &mut self.table {
            table.push_text(code);
            return;
        }
        self.begin_line_if_needed();
        let start = self.current.len();
        self.current.push_str(code);
        self.push_current_inline_span(start, self.current.len(), SingleSessionInlineSpanKind::Code);
    }

    fn push_inline_math(&mut self, math: &str) {
        if let Some(image) = self.image_stack.last_mut() {
            image.alt_text.push_str(math);
            return;
        }
        if let Some(table) = &mut self.table {
            table.push_text(math);
            return;
        }
        self.begin_line_if_needed();
        let start = self.current.len();
        self.current.push_str(math);
        self.push_current_inline_span(start, self.current.len(), SingleSessionInlineSpanKind::Math);
    }

    fn start_inline_span(&mut self, kind: SingleSessionInlineSpanKind) {
        if self.image_stack.last_mut().is_some() || self.table.is_some() {
            return;
        }
        self.begin_line_if_needed();
        self.active_inline_spans
            .push(AssistantMarkdownActiveInlineSpan {
                kind,
                start: self.current.len(),
            });
    }

    fn end_inline_span(&mut self, kind: SingleSessionInlineSpanKind) {
        if self.image_stack.last_mut().is_some() || self.table.is_some() {
            return;
        }
        let Some(index) = self
            .active_inline_spans
            .iter()
            .rposition(|span| span.kind == kind)
        else {
            return;
        };
        let active = self.active_inline_spans.remove(index);
        self.push_current_inline_span(active.start, self.current.len(), kind);
    }

    fn push_current_inline_span(
        &mut self,
        start: usize,
        end: usize,
        kind: SingleSessionInlineSpanKind,
    ) {
        if start < end {
            self.current_inline_spans
                .push(SingleSessionInlineSpan { start, end, kind });
        }
    }

    fn push_display_math(&mut self, math: &str) {
        if let Some(image) = self.image_stack.last_mut() {
            image.alt_text.push_str("$$");
            image.alt_text.push_str(math);
            image.alt_text.push_str("$$");
            return;
        }
        if let Some(table) = &mut self.table {
            table.push_text("$$ ");
            table.push_text(math.trim());
            table.push_text(" $$");
            return;
        }

        self.flush_current_line();
        self.ensure_block_gap();
        self.lines
            .push(styled_line("  $$", SingleSessionLineStyle::Code));
        for line in math.trim_matches('\n').lines() {
            self.lines.push(styled_line(
                format!("  {line}"),
                SingleSessionLineStyle::Code,
            ));
        }
        self.lines
            .push(styled_line("  $$", SingleSessionLineStyle::Code));
    }

    fn push_html_block(&mut self, html: &str) {
        if let Some(image) = self.image_stack.last_mut() {
            image.alt_text.push_str(html.trim());
            return;
        }
        if let Some(table) = &mut self.table {
            table.push_text("html ");
            table.push_text(html.trim());
            return;
        }
        if self.in_code_block {
            self.push_code_text(html);
            return;
        }

        self.flush_current_line();
        self.ensure_block_gap();
        for line in html.trim_matches('\n').lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            self.lines.push(styled_line(
                format!("html │ {trimmed}"),
                SingleSessionLineStyle::Meta,
            ));
        }
    }

    fn soft_break(&mut self) {
        if let Some(table) = &mut self.table {
            table.push_space();
            return;
        }
        if self.in_code_block {
            self.lines
                .push(styled_line("  ", SingleSessionLineStyle::Code));
            return;
        }
        self.push_space();
    }

    fn hard_break(&mut self) {
        if let Some(table) = &mut self.table {
            table.push_space();
            return;
        }
        self.flush_current_line();
        if !self.continuation_prefix.is_empty() {
            self.pending_line_prefix = self.continuation_prefix.clone();
        } else if self.quote_depth > 0 {
            self.pending_line_prefix = self.quote_prefix();
        }
    }

    fn rule(&mut self) {
        self.flush_current_line();
        self.ensure_block_gap();
        self.lines
            .push(styled_line("────────────", SingleSessionLineStyle::Meta));
    }

    fn begin_line_if_needed(&mut self) {
        if !self.current.is_empty() {
            return;
        }
        if !self.pending_line_prefix.is_empty() {
            self.current.push_str(&self.pending_line_prefix);
            self.pending_line_prefix.clear();
            self.reset_active_inline_span_starts();
            return;
        }
        if self.quote_depth > 0 {
            self.current.push_str(&self.quote_prefix());
            self.reset_active_inline_span_starts();
        }
    }

    fn reset_active_inline_span_starts(&mut self) {
        let start = self.current.len();
        for span in &mut self.active_inline_spans {
            span.start = start;
        }
    }

    fn push_space(&mut self) {
        self.begin_line_if_needed();
        if !self.current.chars().last().is_some_and(char::is_whitespace) {
            self.current.push(' ');
        }
    }

    fn push_code_text(&mut self, text: &str) {
        if text.is_empty() {
            self.lines
                .push(styled_line("  ", SingleSessionLineStyle::Code));
            return;
        }
        for line in text.lines() {
            self.lines.push(styled_line(
                format!("  {line}"),
                SingleSessionLineStyle::Code,
            ));
        }
    }

    fn flush_current_line(&mut self) {
        let style = self
            .line_style_override
            .take()
            .unwrap_or(self.current_style);
        self.flush_current_line_as(style);
    }

    fn flush_current_line_as(&mut self, style: SingleSessionLineStyle) {
        let trimmed_len = single_session_trimmed_line_end_preserving_inline_code_whitespace(
            &self.current,
            &self.current_inline_spans,
        );
        if trimmed_len > 0 {
            let safe_trimmed_len = safe_utf8_prefix_len(&self.current, trimmed_len);
            let trimmed = &self.current[..safe_trimmed_len];
            let inline_spans =
                take_current_inline_spans(&mut self.current_inline_spans, safe_trimmed_len);
            self.lines.push(SingleSessionStyledLine::with_inline_spans(
                trimmed,
                style,
                inline_spans,
            ));
        } else {
            self.current_inline_spans.clear();
        }
        self.current.clear();
        self.active_inline_spans.clear();
        self.line_style_override = None;
    }

    fn ensure_block_gap(&mut self) {
        if self
            .lines
            .last()
            .is_some_and(|line| line.style != SingleSessionLineStyle::Blank)
        {
            self.lines.push(blank_styled_line());
        }
    }

    fn prose_style(&self) -> SingleSessionLineStyle {
        if self.in_footnote_definition {
            SingleSessionLineStyle::Meta
        } else if self.quote_depth > 0 {
            SingleSessionLineStyle::AssistantQuote
        } else {
            SingleSessionLineStyle::Assistant
        }
    }

    fn quote_prefix(&self) -> String {
        "│ ".repeat(self.quote_depth)
    }

    fn list_item_prefix(&mut self, task: bool) -> (String, String) {
        let quote_prefix = self.quote_prefix();
        let depth = self.list_stack.len().saturating_sub(1);
        let indent = "  ".repeat(depth);
        let marker = if task {
            "☐ ".to_string()
        } else if let Some(list) = self.list_stack.last_mut() {
            if let Some(next_number) = &mut list.next_number {
                let marker = format!("{next_number}. ");
                *next_number += 1;
                marker
            } else {
                bullet_for_depth(depth).to_string()
            }
        } else {
            "• ".to_string()
        };
        let continuation = format!(
            "{quote_prefix}{indent}{}",
            " ".repeat(marker.chars().count())
        );
        (format!("{quote_prefix}{indent}{marker}"), continuation)
    }

    fn task_item_prefix(&self, checked: bool) -> (String, String) {
        let quote_prefix = self.quote_prefix();
        let depth = self.list_stack.len().saturating_sub(1);
        let indent = "  ".repeat(depth);
        let marker = if checked { "✓ " } else { "☐ " };
        let continuation = format!(
            "{quote_prefix}{indent}{}",
            " ".repeat(marker.chars().count())
        );
        (format!("{quote_prefix}{indent}{marker}"), continuation)
    }

    fn render_table(&mut self, table: AssistantMarkdownTable) {
        let header_rows = table.header_rows;
        let alignments = table.alignments.clone();
        let rows = table.non_empty_rows();
        if rows.is_empty() {
            return;
        }
        let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
        if column_count == 0 {
            return;
        }
        let mut widths = vec![0usize; column_count];
        for row in &rows {
            for (column, cell) in row.iter().enumerate() {
                widths[column] = widths[column].max(cell.chars().count());
            }
        }
        for (row_index, row) in rows.iter().enumerate() {
            self.lines.push(styled_line(
                format_table_row(row, &widths, &alignments),
                SingleSessionLineStyle::AssistantTable,
            ));
            if header_rows > 0 && row_index + 1 == header_rows.min(rows.len()) {
                self.lines.push(styled_line(
                    format_table_separator(&widths, &alignments),
                    SingleSessionLineStyle::AssistantTable,
                ));
            }
        }
    }
}

impl AssistantMarkdownTable {
    fn push_text(&mut self, text: &str) {
        self.current_cell.push_str(&text.replace('\n', " "));
    }

    fn push_space(&mut self) {
        if !self
            .current_cell
            .chars()
            .last()
            .is_some_and(char::is_whitespace)
        {
            self.current_cell.push(' ');
        }
    }

    fn finish_cell(&mut self) {
        self.current_row.push(self.current_cell.trim().to_string());
        self.current_cell.clear();
    }

    fn finish_row(&mut self) {
        if !self.current_row.is_empty() {
            self.rows.push(std::mem::take(&mut self.current_row));
        }
    }

    fn non_empty_rows(mut self) -> Vec<Vec<String>> {
        if !self.current_cell.trim().is_empty() {
            self.finish_cell();
        }
        self.finish_row();
        self.rows
            .into_iter()
            .filter(|row| row.iter().any(|cell| !cell.is_empty()))
            .collect()
    }
}

fn heading_prefix(level: HeadingLevel) -> &'static str {
    match level {
        HeadingLevel::H1 | HeadingLevel::H2 => "",
        HeadingLevel::H3 => "› ",
        _ => "· ",
    }
}

fn block_quote_kind_label(kind: BlockQuoteKind) -> &'static str {
    match kind {
        BlockQuoteKind::Note => "NOTE",
        BlockQuoteKind::Tip => "TIP",
        BlockQuoteKind::Important => "IMPORTANT",
        BlockQuoteKind::Warning => "WARNING",
        BlockQuoteKind::Caution => "CAUTION",
    }
}

fn bullet_for_depth(depth: usize) -> &'static str {
    match depth % 3 {
        0 => "• ",
        1 => "◦ ",
        _ => "▪ ",
    }
}

fn format_table_row(row: &[String], widths: &[usize], alignments: &[Alignment]) -> String {
    let mut rendered = String::new();
    for (column, width) in widths.iter().enumerate() {
        if column > 0 {
            rendered.push_str(" │ ");
        }
        let cell = row.get(column).map(String::as_str).unwrap_or_default();
        let alignment = alignments.get(column).copied().unwrap_or(Alignment::None);
        rendered.push_str(&format_table_cell(cell, *width, alignment));
    }
    rendered.trim_end().to_string()
}

fn format_table_cell(cell: &str, width: usize, alignment: Alignment) -> String {
    let padding = width.saturating_sub(cell.chars().count());
    match alignment {
        Alignment::Right => format!("{}{cell}", " ".repeat(padding)),
        Alignment::Center => {
            let left = padding / 2;
            let right = padding.saturating_sub(left);
            format!("{}{cell}{}", " ".repeat(left), " ".repeat(right))
        }
        Alignment::Left | Alignment::None => format!("{cell}{}", " ".repeat(padding)),
    }
}

fn format_table_separator(widths: &[usize], alignments: &[Alignment]) -> String {
    let mut rendered = String::new();
    for (column, width) in widths.iter().enumerate() {
        if column > 0 {
            rendered.push_str("─┼─");
        }
        let width = (*width).max(1);
        match alignments.get(column).copied().unwrap_or(Alignment::None) {
            Alignment::Left => {
                rendered.push('╾');
                rendered.push_str(&"─".repeat(width.saturating_sub(1)));
            }
            Alignment::Right => {
                rendered.push_str(&"─".repeat(width.saturating_sub(1)));
                rendered.push('╼');
            }
            Alignment::Center => {
                rendered.push('╾');
                if width > 1 {
                    rendered.push_str(&"─".repeat(width.saturating_sub(2)));
                    rendered.push('╼');
                }
            }
            Alignment::None => rendered.push_str(&"─".repeat(width)),
        }
    }
    rendered
}

fn append_tool_lines(
    lines: &mut Vec<SingleSessionStyledLine>,
    content: &str,
    active: bool,
    active_input: Option<&str>,
) {
    if content.is_empty() {
        return;
    }
    let mut raw_lines = content.lines();
    let Some(header) = raw_lines.next() else {
        return;
    };
    if !header.trim_start().starts_with(['▾', '▸']) {
        for line in std::iter::once(header).chain(raw_lines) {
            if !line.trim().is_empty() {
                lines.push(styled_line(
                    format!("  {}", line.trim()),
                    SingleSessionLineStyle::Tool,
                ));
            }
        }
        return;
    }
    let header = parse_tool_header(header);
    let mut metadata_lines = Vec::new();
    let mut widget_lines = Vec::new();
    for line in raw_lines {
        if let Some(raw_input) = line.strip_prefix("  input: ") {
            metadata_lines.extend(formatted_tool_input_lines(&header.name, raw_input));
        } else if !line.trim().is_empty() {
            widget_lines.push(compact_tool_widget_text(line.trim(), 112));
        }
    }
    if let Some(raw_input) = active_input.filter(|input| !input.is_empty()) {
        metadata_lines.extend(formatted_tool_input_lines(&header.name, raw_input));
    }

    lines.push(styled_line(
        format_tool_header_line_with_metadata(&header, &metadata_lines),
        SingleSessionLineStyle::Tool,
    ));

    if active
        && widget_lines.is_empty()
        && matches!(header.state.as_deref(), Some("preparing") | Some("running"))
    {
        widget_lines.push("waiting for tool output…".to_string());
    }

    if active && !widget_lines.is_empty() {
        append_tool_content_widget(lines, &widget_lines);
    }
}

#[derive(Debug, Eq, PartialEq)]
struct ToolHeader {
    name: String,
    state: Option<String>,
    summary: Option<String>,
}

fn parse_tool_header(line: &str) -> ToolHeader {
    let line = line.trim().trim_start_matches(['▾', '▸']).trim();
    let mut parts = line.splitn(2, char::is_whitespace);
    let name = parts
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or("tool");
    let rest = parts.next().unwrap_or_default().trim();
    if rest.is_empty() {
        return ToolHeader {
            name: name.to_string(),
            state: None,
            summary: None,
        };
    }

    let (state, summary) = rest
        .split_once(':')
        .map(|(state, summary)| (state.trim(), Some(summary.trim())))
        .unwrap_or((rest, None));

    ToolHeader {
        name: name.to_string(),
        state: Some(state.to_string()).filter(|state| !state.is_empty()),
        summary: summary
            .filter(|summary| !summary.is_empty())
            .map(|summary| compact_tool_text(summary, 116)),
    }
}

#[cfg(test)]
fn format_tool_header_line(header: &ToolHeader) -> String {
    format_tool_header_line_with_metadata(header, &[])
}

fn format_tool_header_line_with_metadata(header: &ToolHeader, metadata_lines: &[String]) -> String {
    let icon = match header.state.as_deref() {
        Some("done") => "✓",
        Some("failed") => "✕",
        Some("running") => "●",
        Some("preparing") => "○",
        _ => "•",
    };
    let mut line = match (&header.state, &header.summary) {
        (Some(state), Some(summary)) => format!("  {icon} {} · {state} · {summary}", header.name),
        (Some(state), None) => format!("  {icon} {} · {state}", header.name),
        (None, Some(summary)) => format!("  {icon} {} · {summary}", header.name),
        (None, None) => format!("  {icon} {}", header.name),
    };

    if let Some(metadata) = compact_tool_metadata(metadata_lines) {
        line.push_str(" · ");
        line.push_str(&metadata);
    }
    line
}

fn compact_tool_metadata(metadata_lines: &[String]) -> Option<String> {
    let metadata = metadata_lines
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join(" · ");
    (!metadata.is_empty()).then(|| compact_tool_text(&metadata, 116))
}

fn append_tool_content_widget(lines: &mut Vec<SingleSessionStyledLine>, content_lines: &[String]) {
    const MAX_WIDGET_LINES: usize = 12;
    const WIDGET_WIDTH: usize = 68;

    lines.push(styled_line(
        format!("  ╭{}╮", "─".repeat(WIDGET_WIDTH)),
        SingleSessionLineStyle::Tool,
    ));
    for line in content_lines.iter().take(MAX_WIDGET_LINES) {
        lines.push(styled_line(
            format_tool_widget_content_line(line, WIDGET_WIDTH),
            SingleSessionLineStyle::Tool,
        ));
    }
    if content_lines.len() > MAX_WIDGET_LINES {
        lines.push(styled_line(
            format_tool_widget_content_line(
                &format!("… {} more lines", content_lines.len() - MAX_WIDGET_LINES),
                WIDGET_WIDTH,
            ),
            SingleSessionLineStyle::Tool,
        ));
    }
    lines.push(styled_line(
        format!("  ╰{}╯", "─".repeat(WIDGET_WIDTH)),
        SingleSessionLineStyle::Tool,
    ));
}

fn format_tool_widget_content_line(line: &str, width: usize) -> String {
    let line = compact_tool_widget_text(line, width);
    let padding = width.saturating_sub(line.chars().count());
    format!("  │{line}{}│", " ".repeat(padding))
}

fn compact_tool_widget_text(text: &str, max_chars: usize) -> String {
    let text = text.trim().replace('\t', "    ");
    if text.chars().count() > max_chars {
        format!(
            "{}…",
            text.chars()
                .take(max_chars.saturating_sub(1))
                .collect::<String>()
        )
    } else {
        text
    }
}

fn append_tool_group_summary(
    lines: &mut Vec<SingleSessionStyledLine>,
    tool_messages: &[SingleSessionMessage],
) {
    if tool_messages.is_empty() {
        return;
    }

    let mut names: Vec<String> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();
    let mut approx_tokens = 0usize;

    for message in tool_messages {
        approx_tokens += message.content().chars().count().div_ceil(4);
        let name = tool_summary_name(message.content());
        if let Some(index) = names.iter().position(|existing| existing == &name) {
            counts[index] += 1;
        } else {
            names.push(name);
            counts.push(1);
        }
    }

    let fragments = names
        .into_iter()
        .zip(counts)
        .map(|(name, count)| format!("{count} {name}"))
        .collect::<Vec<_>>()
        .join(", ");
    let token_fragment = format_approx_tokens(approx_tokens);
    lines.push(styled_line(
        format!("  ▸ tools: {fragments} · ~{token_fragment} tokens"),
        SingleSessionLineStyle::Tool,
    ));
}

fn tool_summary_name(content: &str) -> String {
    content
        .lines()
        .next()
        .unwrap_or("tool")
        .trim_start_matches(['▾', '▸'])
        .split_whitespace()
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("tool")
        .to_string()
}

fn format_approx_tokens(tokens: usize) -> String {
    if tokens >= 10_000 {
        format!("{}k", ((tokens + 500) / 1000))
    } else if tokens >= 1_000 {
        let tenths = (tokens + 50) / 100;
        format!("{}.{}k", tenths / 10, tenths % 10)
    } else {
        tokens.to_string()
    }
}

fn formatted_tool_input_lines(tool_name: &str, raw_input: &str) -> Vec<String> {
    const MAX_INPUT_LINES: usize = 6;
    let raw_input = raw_input.trim();
    if raw_input.is_empty() {
        return vec!["input: <empty>".to_string()];
    }

    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw_input) else {
        return vec![format!("input: {}", compact_tool_text(raw_input, 132))];
    };

    let serde_json::Value::Object(map) = value else {
        return vec![format!(
            "input: {}",
            compact_tool_json_value("input", &value)
        )];
    };

    if map.is_empty() {
        return vec!["input: {}".to_string()];
    }

    if let Some(lines) = formatted_tool_input_summary(tool_name, &map) {
        return lines;
    }

    let mut keys = map.keys().cloned().collect::<Vec<_>>();
    keys.sort_by(|left, right| {
        tool_input_key_priority(left)
            .cmp(&tool_input_key_priority(right))
            .then_with(|| left.cmp(right))
    });

    let total = keys.len();
    let mut rendered = keys
        .into_iter()
        .take(MAX_INPUT_LINES)
        .filter_map(|key| {
            map.get(&key)
                .map(|value| format!("{key}: {}", compact_tool_json_value(&key, value)))
        })
        .collect::<Vec<_>>();
    if total > MAX_INPUT_LINES {
        rendered.push(format!("… {} more", total - MAX_INPUT_LINES));
    }
    rendered
}

fn formatted_tool_input_summary(
    tool_name: &str,
    map: &serde_json::Map<String, serde_json::Value>,
) -> Option<Vec<String>> {
    let string_value = |key: &str| map.get(key).and_then(serde_json::Value::as_str);
    let bool_value = |key: &str| map.get(key).and_then(serde_json::Value::as_bool);
    let mut lines = Vec::new();

    match tool_name {
        "bash" => {
            if let Some(command) = string_value("command") {
                lines.push(format!("$ {}", compact_tool_text(command, 132)));
            }
        }
        "read" => {
            if let Some(path) = string_value("file_path") {
                lines.push(format!("read {}", compact_tool_text(path, 132)));
            }
        }
        "write" | "edit" | "multiedit" => {
            if let Some(path) = string_value("file_path") {
                let mut summary = compact_tool_text(path, 132);
                if tool_name == "multiedit"
                    && let Some(count) = map
                        .get("edits")
                        .and_then(serde_json::Value::as_array)
                        .map(Vec::len)
                {
                    summary.push_str(&format!(" ({count} edits)"));
                }
                lines.push(summary);
            }
        }
        "glob" => {
            if let Some(pattern) = string_value("pattern") {
                lines.push(format!("'{}'", compact_tool_text(pattern, 96)));
            }
        }
        "agentgrep" | "grep" => {
            let query = string_value("query").or_else(|| string_value("pattern"));
            if tool_name == "agentgrep" {
                let mode = string_value("mode").unwrap_or("grep");
                if let Some(query) = query.filter(|query| !query.trim().is_empty()) {
                    lines.push(format!("{mode} '{}'", compact_tool_text(query, 72)));
                } else {
                    lines.push(mode.to_string());
                }
            } else if let Some(query) = query {
                lines.push(format!("'{}'", compact_tool_text(query, 72)));
            }
            if let Some(path) = string_value("path") {
                lines.push(format!("in {}", compact_tool_text(path, 132)));
            }
        }
        "webfetch" | "websearch" => {
            if let Some(query) = string_value("query").or_else(|| string_value("url")) {
                lines.push(compact_tool_text(query, 132));
            }
        }
        "browser" => {
            if let Some(action) = string_value("action") {
                let target = string_value("url")
                    .or_else(|| string_value("selector"))
                    .or_else(|| string_value("text"));
                lines.push(match target {
                    Some(target) => format!("{action} {}", compact_tool_text(target, 112)),
                    None => action.to_string(),
                });
            }
        }
        "open" | "launch" => {
            let action = string_value("action").unwrap_or("open");
            if let Some(target) = string_value("target") {
                lines.push(format!("{action} {}", compact_tool_text(target, 96)));
            } else {
                lines.push(action.to_string());
            }
        }
        "todo" => {
            if let Some(count) = map
                .get("todos")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len)
            {
                lines.push(format!("{count} items"));
            }
        }
        "memory" | "goal" | "side_panel" | "bg" | "mcp" | "selfdev" | "swarm" => {
            if let Some(action) = string_value("action") {
                let target = string_value("title")
                    .or_else(|| string_value("id"))
                    .or_else(|| string_value("task_id"))
                    .or_else(|| string_value("server"))
                    .or_else(|| string_value("server_name"));
                lines.push(match target {
                    Some(target) => format!("{action} {}", compact_tool_text(target, 96)),
                    None => action.to_string(),
                });
            }
        }
        "batch" => {
            if let Some(count) = map
                .get("tool_calls")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len)
            {
                lines.push(format!("{count} calls"));
            }
        }
        "subagent" | "task" => {
            let desc = string_value("description").unwrap_or("task");
            let agent_type = string_value("subagent_type").unwrap_or("agent");
            lines.push(format!(
                "{} ({})",
                compact_tool_text(desc, 84),
                compact_tool_text(agent_type, 28)
            ));
        }
        _ => {}
    }

    if bool_value("run_in_background") == Some(true) {
        lines.push("background: yes".to_string());
    }

    (!lines.is_empty()).then_some(lines)
}

fn tool_input_key_priority(key: &str) -> usize {
    match key {
        "command" => 0,
        "file_path" | "path" => 1,
        "query" => 2,
        "pattern" | "glob" => 3,
        "url" => 4,
        "action" => 5,
        "task" | "prompt" | "description" => 6,
        "intent" => 90,
        _ => 100,
    }
}

fn compact_tool_json_value(key: &str, value: &serde_json::Value) -> String {
    if is_sensitive_tool_input_key(key) {
        return "••••".to_string();
    }
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(value) => value.to_string(),
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::String(value) => {
            if key.to_ascii_lowercase().contains("base64") {
                format!("<base64, {} chars>", value.chars().count())
            } else {
                compact_tool_text(value, 108)
            }
        }
        serde_json::Value::Array(values) => {
            if values.is_empty() {
                "[]".to_string()
            } else if values.len() <= 3 && values.iter().all(is_compact_tool_scalar) {
                let joined = values
                    .iter()
                    .map(|value| compact_tool_json_value(key, value))
                    .collect::<Vec<_>>()
                    .join(", ");
                compact_tool_text(&format!("[{joined}]"), 108)
            } else {
                format!("[{} items]", values.len())
            }
        }
        serde_json::Value::Object(map) => format!("{{{} fields}}", map.len()),
    }
}

fn is_compact_tool_scalar(value: &serde_json::Value) -> bool {
    matches!(
        value,
        serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_)
    )
}

fn is_sensitive_tool_input_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("password") || key.contains("token") || key.contains("secret")
}

fn compact_tool_text(text: &str, max_chars: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() > max_chars {
        format!("{}…", text.chars().take(max_chars).collect::<String>())
    } else {
        text
    }
}

fn merge_tool_finish_with_existing_context(existing: &str, finish_line: &str) -> String {
    let context = existing
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if context.is_empty() {
        finish_line.to_string()
    } else {
        format!("{}\n{}", finish_line, context.join("\n"))
    }
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

fn slash_suggestion_query(input: &str, cursor: usize) -> Option<String> {
    let (start, end) = slash_suggestion_prefix_bounds(input, cursor)?;
    Some(input[start..end].to_string())
}

fn slash_suggestion_prefix_bounds(input: &str, cursor: usize) -> Option<(usize, usize)> {
    let cursor = cursor.min(input.len());
    if !input.is_char_boundary(cursor) {
        return None;
    }
    let prefix = &input[..cursor];
    let start = prefix.len() - prefix.trim_start().len();
    let command_prefix = &input[start..cursor];
    if !command_prefix.starts_with('/') || command_prefix.contains(char::is_whitespace) {
        return None;
    }
    Some((start, cursor))
}

fn complete_slash_command(
    input: &str,
    cursor: usize,
    completions: &[&'static str],
) -> Option<(String, usize)> {
    let cursor = cursor.min(input.len());
    if !input.is_char_boundary(cursor) || !input.starts_with('/') {
        return None;
    }
    let prefix = &input[..cursor];
    if prefix.contains(char::is_whitespace) {
        return None;
    }
    let suffix = &input[cursor..];
    let matches = completions
        .iter()
        .copied()
        .filter(|command| command.starts_with(prefix))
        .collect::<Vec<_>>();
    let completion = match matches.as_slice() {
        [] => return None,
        [only] => *only,
        _ => longest_common_prefix(&matches)?,
    };
    if completion.len() <= prefix.len() {
        return None;
    }
    let mut completed = completion.to_string();
    completed.push_str(suffix);
    Some((completed, completion.len()))
}

fn longest_common_prefix<'a>(values: &'a [&'a str]) -> Option<&'a str> {
    let first = *values.first()?;
    let mut end = first.len();
    for value in values.iter().skip(1) {
        while end > 0 && !value.starts_with(&first[..end]) {
            end = previous_char_boundary(first, end);
        }
    }
    (end > 0).then_some(&first[..end])
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
        kind: if session.is_some() {
            workspace::SurfaceKind::Session
        } else {
            workspace::SurfaceKind::Scratch
        },
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered_tool_text(content: &str, active: bool) -> Vec<String> {
        let mut lines = Vec::new();
        append_tool_lines(&mut lines, content, active, None);
        lines.into_iter().map(|line| line.text).collect()
    }

    #[test]
    fn tool_header_uses_status_icons_and_compact_summary() {
        assert_eq!(
            format_tool_header_line(&parse_tool_header("▾ bash done: completed successfully")),
            "  ✓ bash · done · completed successfully"
        );
        assert_eq!(
            format_tool_header_line(&parse_tool_header("▾ browser failed: selector missing")),
            "  ✕ browser · failed · selector missing"
        );
    }

    #[test]
    fn bash_tool_rendering_shows_intent_command_and_background_flag() {
        let lines = rendered_tool_text(
            "▾ bash running\n  input: {\"intent\":\"run the desktop tests\",\"command\":\"cargo test -p jcode-desktop\",\"run_in_background\":true}",
            true,
        );
        assert_eq!(
            lines,
            vec![
                "  ● bash · running · $ cargo test -p jcode-desktop · background: yes",
                "  ╭────────────────────────────────────────────────────────────────────╮",
                "  │waiting for tool output…                                            │",
                "  ╰────────────────────────────────────────────────────────────────────╯",
            ]
        );
    }

    #[test]
    fn desktop_tool_metadata_prioritizes_tui_like_summary_over_intent() {
        assert_eq!(
            formatted_tool_input_lines(
                "agentgrep",
                "{\"intent\":\"Locate rendering code\",\"query\":\"tool call\",\"path\":\"src/tui\"}",
            ),
            vec!["grep 'tool call'", "in src/tui"]
        );
        assert_eq!(
            formatted_tool_input_lines(
                "side_panel",
                "{\"intent\":\"Show notes\",\"action\":\"write\",\"title\":\"Plan\"}",
            ),
            vec!["write Plan"]
        );
        assert_eq!(
            formatted_tool_input_lines(
                "subagent",
                "{\"intent\":\"Delegate\",\"description\":\"Inspect parser\",\"subagent_type\":\"agent\"}",
            ),
            vec!["Inspect parser (agent)"]
        );
    }

    #[test]
    fn tool_result_content_renders_inside_inline_widget() {
        let lines = rendered_tool_text(
            "▾ bash failed: tests failed\n  input: {\"command\":\"cargo test -p jcode-desktop\"}\n  error[E0425]: cannot find value `foo` in this scope\n  test result: FAILED",
            true,
        );

        assert_eq!(
            lines[0],
            "  ✕ bash · failed · tests failed · $ cargo test -p jcode-desktop"
        );
        assert_eq!(
            lines[1],
            "  ╭────────────────────────────────────────────────────────────────────╮"
        );
        assert_eq!(
            lines[2],
            "  │error[E0425]: cannot find value `foo` in this scope                 │"
        );
        assert_eq!(
            lines[3],
            "  │test result: FAILED                                                 │"
        );
        assert_eq!(
            lines[4],
            "  ╰────────────────────────────────────────────────────────────────────╯"
        );
    }

    #[test]
    fn inactive_tool_result_compacts_to_metadata_only() {
        let lines = rendered_tool_text(
            "▾ bash done: tests passed\n  input: {\"command\":\"cargo test -p jcode-desktop\"}\n  test result: ok",
            false,
        );

        assert_eq!(
            lines,
            vec!["  ✓ bash · done · tests passed · $ cargo test -p jcode-desktop"]
        );
    }

    #[test]
    fn unknown_tool_falls_back_to_prioritized_key_value_lines() {
        let lines = formatted_tool_input_lines(
            "custom",
            "{\"token\":\"secret\",\"query\":\"tool calls\",\"extra\":42}",
        );
        assert_eq!(lines, vec!["query: tool calls", "extra: 42", "token: ••••"]);
    }

    #[test]
    fn unknown_tool_uses_intent_only_as_fallback() {
        let lines = formatted_tool_input_lines(
            "custom",
            "{\"intent\":\"describe action\",\"query\":\"tool calls\"}",
        );
        assert_eq!(lines, vec!["query: tool calls", "intent: describe action"]);
    }

    #[test]
    fn safe_utf8_prefix_len_rounds_down_to_char_boundary() {
        let text = "aé🚀";

        assert_eq!(safe_utf8_prefix_len(text, 0), 0);
        assert_eq!(safe_utf8_prefix_len(text, 1), 1);
        assert_eq!(safe_utf8_prefix_len(text, 2), 1);
        assert_eq!(safe_utf8_prefix_len(text, 3), 3);
        assert_eq!(safe_utf8_prefix_len(text, 6), 3);
        assert_eq!(safe_utf8_prefix_len(text, usize::MAX), text.len());
    }
}
