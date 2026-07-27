use crate::{
    desktop_rich_text, session_data,
    session_launch::{
        DesktopModelChoice, DesktopSessionEvent, DesktopSessionHandle, DesktopSessionStatus,
    },
    workspace,
};
use jcode_tui_messages::DisplayMessage;
use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};
use std::collections::{HashSet, hash_map::DefaultHasher};
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use workspace::{KeyInput, KeyOutcome, SessionTranscriptMessage};

mod issue_browser;
pub(crate) use issue_browser::*;

mod overlay_render;
pub(crate) use overlay_render::*;

mod welcome;
pub(crate) use welcome::*;

mod pickers;
pub(crate) use pickers::*;

mod markdown;
pub(crate) use markdown::*;

mod tool_lines;
pub(crate) use tool_lines::*;
mod commands;
mod input;
mod overlays;
mod transcript;

pub(crate) const SINGLE_SESSION_FONT_FAMILY: &str = "JetBrainsMono Nerd Font";
pub(crate) const SINGLE_SESSION_USER_FONT_FAMILY: &str = "Kalam";
pub(crate) const SINGLE_SESSION_ASSISTANT_FONT_FAMILY: &str = SINGLE_SESSION_FONT_FAMILY;
pub(crate) const SINGLE_SESSION_WELCOME_FONT_FAMILY: &str = "Homemade Apple";
pub(crate) const SINGLE_SESSION_FONT_WEIGHT: &str = "Light";
pub(crate) const SINGLE_SESSION_FONT_FALLBACKS: &[&str] = &[
    "JetBrainsMono Nerd Font Mono",
    "JetBrains Mono",
    "monospace",
];

pub(crate) const SINGLE_SESSION_HANDWRITING_FONT_FAMILIES: &[&str] = &[
    "Homemade Apple",
    "Kalam",
    "Shadows Into Light Two",
    "Patrick Hand",
    "Gaegu",
    "Caveat",
    "Indie Flower",
    "Gloria Hallelujah",
    "Handlee",
    "Reenie Beanie",
];

static DESKTOP_USER_FONT_INDEX: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(usize::MAX);
static DESKTOP_ASSISTANT_FONT_INDEX: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(usize::MAX);

pub(crate) fn single_session_user_font_family() -> &'static str {
    desktop_font_family_from_index(
        DESKTOP_USER_FONT_INDEX.load(std::sync::atomic::Ordering::Relaxed),
    )
    .or_else(|| desktop_font_family_from_env("JCODE_DESKTOP_USER_FONT"))
    .unwrap_or(SINGLE_SESSION_USER_FONT_FAMILY)
}

pub(crate) fn single_session_assistant_font_family() -> &'static str {
    desktop_font_family_from_index(
        DESKTOP_ASSISTANT_FONT_INDEX.load(std::sync::atomic::Ordering::Relaxed),
    )
    .or_else(|| desktop_font_family_from_env("JCODE_DESKTOP_AI_FONT"))
    .or_else(|| desktop_font_family_from_env("JCODE_DESKTOP_ASSISTANT_FONT"))
    .unwrap_or(SINGLE_SESSION_ASSISTANT_FONT_FAMILY)
}

pub(crate) fn set_single_session_user_font_family(value: &str) -> Option<&'static str> {
    let (index, family) = desktop_font_family_index_from_key(value)?;
    DESKTOP_USER_FONT_INDEX.store(index, std::sync::atomic::Ordering::Relaxed);
    Some(family)
}

pub(crate) fn set_single_session_assistant_font_family(value: &str) -> Option<&'static str> {
    let (index, family) = desktop_font_family_index_from_key(value)?;
    DESKTOP_ASSISTANT_FONT_INDEX.store(index, std::sync::atomic::Ordering::Relaxed);
    Some(family)
}

fn desktop_font_family_from_index(index: usize) -> Option<&'static str> {
    match index {
        0 => Some(SINGLE_SESSION_FONT_FAMILY),
        index => SINGLE_SESSION_HANDWRITING_FONT_FAMILIES
            .get(index - 1)
            .copied(),
    }
}

fn desktop_font_family_index_from_key(value: &str) -> Option<(usize, &'static str)> {
    let family = desktop_font_family_from_key(value)?;
    if family == SINGLE_SESSION_FONT_FAMILY {
        return Some((0, family));
    }
    SINGLE_SESSION_HANDWRITING_FONT_FAMILIES
        .iter()
        .position(|candidate| *candidate == family)
        .map(|index| (index + 1, family))
}

fn desktop_font_family_from_env(name: &str) -> Option<&'static str> {
    let value = std::env::var(name).ok()?;
    desktop_font_family_from_key(&value)
}

pub(crate) fn desktop_font_family_from_key(value: &str) -> Option<&'static str> {
    let normalized = value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>();
    match normalized.as_str() {
        "jetbrains" | "jetbrainsmono" | "jetbrainsmononerdfont" | "default" => {
            Some(SINGLE_SESSION_FONT_FAMILY)
        }
        "homemadeapple" => Some("Homemade Apple"),
        "kalam" => Some("Kalam"),
        "shadowsintolighttwo" | "shadowsintolight" => Some("Shadows Into Light Two"),
        "patrickhand" => Some("Patrick Hand"),
        "gaegu" => Some("Gaegu"),
        "caveat" => Some("Caveat"),
        "indieflower" => Some("Indie Flower"),
        "gloriahallelujah" => Some("Gloria Hallelujah"),
        "handlee" => Some("Handlee"),
        "reeniebeanie" => Some("Reenie Beanie"),
        _ => None,
    }
}
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
    ("/session", "alias for /sessions"),
    ("/issues", "toggle the local GitHub issue browser"),
    ("/model [name]", "open model picker or switch to a model"),
    ("/models", "alias for /model"),
    ("/refresh-model-list", "refresh provider model catalogs"),
    ("/reload", "force reload the desktop window using handoff"),
    ("/force-reload", "alias for /reload"),
    ("/effort [level]", "show or change reasoning effort"),
    (
        "/font [user|ai] [name]",
        "show or hot-swap desktop transcript fonts",
    ),
    ("/fonts", "alias for /font"),
    ("/fast [on|off|status]", "show or toggle OpenAI fast mode"),
    ("/transport [mode]", "show or change OpenAI transport"),
    (
        "/compact [mode <mode>]",
        "compact context or set compaction mode",
    ),
    ("/rename <title|--clear>", "rename the current session"),
    ("/usage", "desktop parity notice for TUI usage overlay"),
    ("/todo", "desktop parity notice for TUI todo panel"),
    ("/todos", "alias for /todo"),
    ("/memory", "desktop parity notice for TUI memory panel"),
    (
        "/changelog",
        "desktop parity notice for TUI changelog overlay",
    ),
    ("/diff", "desktop parity notice for TUI diff viewer"),
    ("/account", "desktop parity notice for TUI account picker"),
    ("/swarm", "desktop parity notice for TUI swarm panel"),
    ("/bg", "desktop parity notice for TUI background task panel"),
    (
        "/copy [latest|code|transcript]",
        "copy latest response, latest code block, or transcript",
    ),
    (
        "/search <query>",
        "count transcript matches and jump to the first one",
    ),
    ("/commit", "make logical commits from current changes"),
    ("/stop", "interrupt the running generation"),
    ("/cancel", "alias for /stop"),
    ("/status", "show current desktop session status"),
    ("/info", "alias for /status"),
    ("/quit", "exit the desktop app"),
    ("/exit", "alias for /quit"),
];
pub(crate) const DESKTOP_SLASH_SUGGESTION_ROW_LIMIT: usize = 7;

const DESKTOP_REASONING_EFFORTS_OPENAI: &[&str] = &["none", "low", "medium", "high", "xhigh"];
const DESKTOP_REASONING_EFFORTS_ANTHROPIC_STANDARD: &[&str] =
    &["none", "low", "medium", "high", "max"];
const DESKTOP_REASONING_EFFORTS_ANTHROPIC_XHIGH: &[&str] =
    &["none", "low", "medium", "high", "xhigh", "max"];
const DESKTOP_REASONING_EFFORTS_DEEPSEEK: &[&str] = &["none", "low", "medium", "high", "max"];

#[cfg_attr(test, allow(dead_code))]
const INLINE_WIDGET_REVEAL_DURATION: Duration = Duration::from_millis(180);
const INLINE_WIDGET_EXIT_DURATION: Duration = Duration::from_millis(140);
pub(crate) const MODEL_PICKER_INLINE_ROW_LIMIT: usize = 5;
pub(crate) const INLINE_WIDGET_DEFAULT_VISIBLE_LINE_LIMIT: usize = 12;

const BODY_CACHE_TEXT_EDGE_BYTES: usize = 256;
const BODY_CACHE_MESSAGE_EDGE_COUNT: usize = 12;
const BODY_CACHE_MESSAGE_MIDDLE_SAMPLE_COUNT: usize = 8;

fn desktop_commit_prompt() -> String {
    "Make interactive, logical commits for the current uncommitted work. Inspect the git state first, including unstaged and staged changes. Group related changes into small coherent commits, staging only the files or hunks that belong together. Preserve unrelated user or agent work, do not discard changes, and do not amend existing commits unless clearly necessary. For each commit, use a concise conventional-style message when possible. Validate as appropriate for the changed files before committing, and report the commits created plus any remaining uncommitted changes.".to_string()
}

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
    pub(crate) reasoning: SingleSessionReasoningState,
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
    side_panel: DesktopSidePanelState,
    pending_issue_sync_request: bool,
    /// Session id whose transcript should be hydrated from disk off the UI
    /// thread. Set when resuming from the session switcher; serviced by the
    /// event loop so large transcript parses never stall key handling.
    pending_transcript_hydration: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct DesktopSidePanelState {
    pub(crate) visible: bool,
    pub(crate) focus: DesktopSidePanelFocus,
    pub(crate) github_issues: GitHubIssueBrowserState,
    pub(crate) github_issue_sync: GitHubIssueSyncUiState,
}

impl Default for DesktopSidePanelState {
    fn default() -> Self {
        Self {
            visible: false,
            focus: DesktopSidePanelFocus::Chat,
            github_issues: GitHubIssueBrowserState::sample(),
            github_issue_sync: GitHubIssueSyncUiState::default(),
        }
    }
}

impl DesktopSidePanelState {
    fn focus_next(&mut self) {
        self.focus = match self.focus {
            DesktopSidePanelFocus::IssueList => DesktopSidePanelFocus::IssuePreview,
            DesktopSidePanelFocus::IssuePreview => DesktopSidePanelFocus::Chat,
            DesktopSidePanelFocus::Chat => DesktopSidePanelFocus::IssueList,
        };
    }

    fn focus_previous(&mut self) {
        self.focus = match self.focus {
            DesktopSidePanelFocus::IssueList => DesktopSidePanelFocus::Chat,
            DesktopSidePanelFocus::IssuePreview => DesktopSidePanelFocus::IssueList,
            DesktopSidePanelFocus::Chat => DesktopSidePanelFocus::IssuePreview,
        };
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DesktopSidePanelFocus {
    IssueList,
    IssuePreview,
    Chat,
}

#[derive(Clone, Debug)]
struct SingleSessionWelcomeState {
    name: Option<String>,
    recovery_session_count: usize,
    continuation_suggestion: Option<String>,
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
        // The continuation suggestion is only rendered on the fresh welcome
        // screen (when there is no session). Scanning external CLI history
        // (`~/.codex`, `~/.claude`) is expensive, so skip it entirely when a
        // session is present. This keeps workspace pane construction cheap,
        // which matters because workspace rendering builds one ephemeral
        // `SingleSessionApp` per visible surface every frame.
        let continuation_suggestion = if has_session {
            None
        } else {
            latest_external_cli_continuation_suggestion()
        };
        Self {
            name,
            recovery_session_count: 0,
            continuation_suggestion,
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
    /// Whether the "thinking display is off" hint has already been shown, so
    /// reconnects do not repeat it.
    thinking_display_notice_shown: bool,
}

impl Default for SingleSessionRuntimeState {
    fn default() -> Self {
        Self {
            session_handle: None,
            reload_phase: ReloadPhase::Stable,
            thinking_display_notice_shown: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct SingleSessionRuntimeSettings {
    reasoning_effort: Option<String>,
    service_tier: Option<String>,
    transport: Option<String>,
    compaction_mode: Option<String>,
    connection_type: Option<String>,
    status_detail: Option<String>,
    upstream_provider: Option<String>,
    token_usage: Option<SingleSessionTokenUsage>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SingleSessionTokenUsage {
    input: u64,
    output: u64,
    cache_read_input: Option<u64>,
    cache_creation_input: Option<u64>,
}

/// Live reasoning ("thinking") state for the current turn.
///
/// The server streams `reasoning_delta` before any answer text. Without this
/// the desktop transcript stays empty until the final response lands, which
/// reads as a hang. We keep a rolling buffer, render its tail dim, and collapse
/// it to a `thought for Ns` trace once the model moves on.
#[derive(Clone, Debug, Default)]
pub(crate) struct SingleSessionReasoningState {
    /// Raw reasoning text streamed so far for the current step.
    pub(crate) live: String,
    /// Deltas seen since the last `ReasoningDone`, used to know a step is open.
    pub(crate) streaming: bool,
}

impl SingleSessionReasoningState {
    pub(crate) fn clear(&mut self) {
        self.live.clear();
        self.streaming = false;
    }

    /// Last few non-empty reasoning lines, which is what the transcript shows.
    pub(crate) fn visible_tail(&self) -> Vec<&str> {
        const MAX_VISIBLE_REASONING_LINES: usize = 3;
        let mut tail = self
            .live
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        if tail.len() > MAX_VISIBLE_REASONING_LINES {
            tail.drain(..tail.len() - MAX_VISIBLE_REASONING_LINES);
        }
        tail
    }
}

#[derive(Clone, Debug, Default)]
struct SingleSessionToolState {
    active_message_index: Option<usize>,
    active_call_id: Option<String>,
    input_buffer: String,
    event_sequence: u64,
    runs: Vec<SingleSessionToolRun>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SingleSessionToolRun {
    pub(crate) call_id: String,
    pub(crate) message_index: usize,
    pub(crate) name: String,
    pub(crate) state: SingleSessionToolVisualState,
    pub(crate) summary: Option<String>,
    pub(crate) input_raw: String,
    pub(crate) input_preview: Option<String>,
    pub(crate) stdin_prompt: Option<String>,
    pub(crate) started_sequence: u64,
    pub(crate) updated_sequence: u64,
    pub(crate) completed_sequence: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum SingleSessionToolVisualState {
    Preparing,
    Running,
    Succeeded,
    Failed,
    Unknown,
    Group,
}

impl SingleSessionToolVisualState {
    /// Human-readable state name. The visual chip that rendered this label was
    /// removed (it drew as an empty ghost pill), but the label stays for
    /// debugging and future textual chrome.
    #[allow(dead_code)]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::Running => "running",
            Self::Succeeded => "done",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
            Self::Group => "tools",
        }
    }

    pub(crate) fn from_tool_state_text(text: &str) -> Self {
        match text.trim().to_ascii_lowercase().as_str() {
            "preparing" | "pending" | "queued" | "waiting" => Self::Preparing,
            "running" | "executing" | "active" => Self::Running,
            "done" | "success" | "succeeded" | "passed" => Self::Succeeded,
            "failed" | "failure" | "error" | "errored" => Self::Failed,
            _ => Self::Unknown,
        }
    }

    pub(crate) fn is_active(self) -> bool {
        matches!(self, Self::Preparing | Self::Running)
    }
}

#[derive(Clone, Debug)]
struct SingleSessionViewState {
    inline_widget_opened_at: Option<Instant>,
    closing_inline_widget: Option<ClosingInlineWidgetState>,
    text_scale: f32,
}

#[derive(Clone, Debug)]
struct ClosingInlineWidgetState {
    kind: InlineWidgetKind,
    lines: Vec<SingleSessionStyledLine>,
    started_at: Instant,
}

impl Default for SingleSessionViewState {
    fn default() -> Self {
        Self {
            inline_widget_opened_at: None,
            closing_inline_widget: None,
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
    pub(crate) tool: Option<SingleSessionToolLineMetadata>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SingleSessionToolLineMetadata {
    pub(crate) call_id: String,
    pub(crate) name: String,
    pub(crate) state: SingleSessionToolVisualState,
    pub(crate) kind: SingleSessionToolLineKind,
    pub(crate) active: bool,
    pub(crate) expanded: bool,
    pub(crate) stdin_prompt: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum SingleSessionToolLineKind {
    Header,
    Detail,
    GroupSummary,
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ReasoningEffortCycleOutcome {
    Set(String),
    AlreadyAtLimit { effort: String, limit: &'static str },
    Unavailable,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum SingleSessionStatus {
    LoadingModels,
    LoadingRecentSessions,
    Receiving,
    Thinking,
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
            Self::Thinking => "thinking".to_string(),
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
            | Self::Thinking
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
                kind: InlineWidgetKind::SessionSwitcher,
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
            Self::SessionSwitcher if app.session_switcher.preview => InlineWidgetMode::ReadOnly,
            Self::SessionSwitcher => InlineWidgetMode::Interactive,
        }
    }

    pub(crate) fn visible_line_limit(self) -> usize {
        match self {
            Self::HotkeyHelp => 18,
            // Compact type fits the whole panel including the closing rail
            // corner; truncating mid-panel leaves the box-drawing rail open.
            Self::SessionInfo => 18,
            Self::ModelPicker => usize::MAX,
            Self::SessionSwitcher => 24,
            Self::SlashSuggestions => DESKTOP_SLASH_SUGGESTION_ROW_LIMIT + 1,
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
    AssistantMedia,
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
            tool: None,
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
            tool: None,
        }
    }

    pub(crate) fn with_tool_metadata(mut self, tool: SingleSessionToolLineMetadata) -> Self {
        self.tool = Some(tool);
        self
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

    pub(crate) fn from_session_transcript(message: SessionTranscriptMessage) -> Self {
        match message.role.as_str() {
            "user" => Self::user(message.content),
            "assistant" => Self::assistant(message.content),
            "tool" => Self::tool(message.content),
            "system" | "background_task" => Self::system(message.content),
            _ => Self::meta(message.content),
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

fn hash_tool_cache_fingerprint<H: Hasher>(tool: &SingleSessionToolState, hasher: &mut H) {
    tool.active_message_index.hash(hasher);
    tool.active_call_id.hash(hasher);
    visible_active_tool_input_preview(tool).hash(hasher);
    for run in &tool.runs {
        run.call_id.hash(hasher);
        run.message_index.hash(hasher);
        run.name.hash(hasher);
        run.state.hash(hasher);
        run.summary.hash(hasher);
        run.input_preview.hash(hasher);
        run.stdin_prompt.hash(hasher);
    }
}

fn visible_active_tool_input_preview(tool: &SingleSessionToolState) -> Option<String> {
    if tool.input_buffer.is_empty() {
        return None;
    }
    let tool_name = tool
        .active_call_id
        .as_ref()
        .and_then(|call_id| tool.runs.iter().find(|run| &run.call_id == call_id))
        .or_else(|| tool.runs.last())
        .map(|run| run.name.as_str())
        .unwrap_or("tool");
    compact_tool_metadata(&formatted_tool_input_lines(tool_name, &tool.input_buffer))
}

fn hash_session_switcher_cache_state<H: Hasher>(switcher: &SessionSwitcherState, hasher: &mut H) {
    switcher.open.hash(hasher);
    switcher.loading.hash(hasher);
    switcher.preview.hash(hasher);
    switcher.filter.hash(hasher);
    switcher.selected.hash(hasher);
    switcher.preview_scroll.hash(hasher);
    switcher.focus.hash(hasher);
    switcher
        .sessions
        .iter()
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
        .collect::<Vec<_>>()
        .hash(hasher);
}

impl SingleSessionApp {
    pub(crate) fn new(session: Option<workspace::SessionCard>) -> Self {
        let welcome = SingleSessionWelcomeState::new(session.is_some());
        let messages = session
            .as_ref()
            .filter(|session| !session.transcript_messages.is_empty())
            .map(|session| {
                session
                    .transcript_messages
                    .iter()
                    .cloned()
                    .map(SingleSessionMessage::from_session_transcript)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            session,
            draft: String::new(),
            draft_cursor: 0,
            detail_scroll: 0,
            live_session_id: None,
            messages,
            streaming_response: String::new(),
            reasoning: SingleSessionReasoningState::default(),
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
            side_panel: DesktopSidePanelState::default(),
            pending_issue_sync_request: false,
            pending_transcript_hydration: None,
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

    #[cfg(test)]
    pub(crate) fn reasoning_effort(&self) -> Option<&str> {
        self.runtime_settings.reasoning_effort.as_deref()
    }

    pub(crate) fn preview_reasoning_effort_set(&mut self, effort: &str) -> Option<String> {
        let normalized = self.normalize_reasoning_effort_for_current_context(effort)?;
        self.runtime_settings.reasoning_effort = Some(normalized.clone());
        self.set_status_label(format!("thinking level: {normalized}"));
        Some(normalized)
    }

    pub(crate) fn preview_reasoning_effort_cycle(
        &mut self,
        direction: i8,
    ) -> ReasoningEffortCycleOutcome {
        let efforts = self.available_reasoning_efforts_for_current_context();
        if efforts.is_empty() {
            self.set_status_label("thinking level is not available for this model");
            return ReasoningEffortCycleOutcome::Unavailable;
        }

        let current = self.runtime_settings.reasoning_effort.as_deref();
        let current_index = current
            .and_then(|effort| efforts.iter().position(|candidate| *candidate == effort))
            .unwrap_or(efforts.len() - 1);
        let next_index = if direction > 0 {
            (current_index + 1).min(efforts.len() - 1)
        } else {
            current_index.saturating_sub(1)
        };
        let next_effort = efforts[next_index];
        if next_index == current_index {
            let limit = if direction > 0 { "max" } else { "min" };
            self.set_status_label(format!(
                "thinking level: {next_effort} (already at {limit})"
            ));
            return ReasoningEffortCycleOutcome::AlreadyAtLimit {
                effort: next_effort.to_string(),
                limit,
            };
        }

        self.runtime_settings.reasoning_effort = Some(next_effort.to_string());
        self.set_status_label(format!("thinking level: {next_effort}"));
        ReasoningEffortCycleOutcome::Set(next_effort.to_string())
    }

    fn normalize_reasoning_effort_for_current_context(&self, raw: &str) -> Option<String> {
        let requested = raw.trim().to_ascii_lowercase();
        if requested.is_empty() {
            return None;
        }
        let efforts = self.available_reasoning_efforts_for_current_context();
        if efforts.is_empty() {
            return None;
        }
        if efforts.contains(&requested.as_str()) {
            return Some(requested);
        }
        if requested == "max" && efforts.contains(&"xhigh") {
            return Some("xhigh".to_string());
        }
        if requested == "xhigh" && efforts.contains(&"max") {
            return Some("max".to_string());
        }
        efforts.last().map(|effort| (*effort).to_string())
    }

    fn available_reasoning_efforts_for_current_context(&self) -> &'static [&'static str] {
        inferred_desktop_reasoning_efforts(
            self.model_picker.provider_name.as_deref(),
            self.model_picker.current_model.as_deref(),
            self.runtime_settings.reasoning_effort.as_deref(),
        )
    }

    pub(crate) fn initialize_resumed_session(&mut self, session_id: &str) {
        self.live_session_id = Some(session_id.to_string());
        self.detail_scroll = 0;
        self.messages.clear();
        self.streaming_response.clear();
        self.reasoning.clear();
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
        self.view.closing_inline_widget = None;
        self.welcome.timeline = false;
    }

    pub(crate) fn hydrate_resumed_session_from_disk(&mut self, session_id: &str) {
        match session_data::load_session_transcript_by_id(session_id) {
            Ok(Some(messages)) if !messages.is_empty() => {
                self.apply_resumed_session_transcript(messages);
            }
            Ok(_) => {}
            Err(error) => {
                crate::desktop_log::warn(format_args!(
                    "jcode-desktop: failed to hydrate resumed transcript for {session_id}: {error:#}"
                ));
                self.error = Some(format!("failed to load transcript: {error:#}"));
            }
        }
    }

    pub(crate) fn apply_resumed_session_transcript(
        &mut self,
        messages: Vec<SessionTranscriptMessage>,
    ) {
        self.messages = messages
            .into_iter()
            .map(SingleSessionMessage::from_session_transcript)
            .collect();
        self.streaming_response.clear();
        self.reasoning.clear();
        self.tool.active_message_index = None;
        self.tool.input_buffer.clear();
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
        self.reasoning.clear();
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
        self.view.closing_inline_widget = None;
        self.side_panel = DesktopSidePanelState::default();
        self.pending_issue_sync_request = false;
    }

    pub(crate) fn side_panel(&self) -> &DesktopSidePanelState {
        &self.side_panel
    }

    pub(crate) fn status_title(&self) -> String {
        format!("{} · {}", crate::DESKTOP_PRODUCT_NAME, self.title())
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
            && self.reasoning.live.trim().is_empty()
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
        self.has_activity_indicator()
            || self.inline_widget_reveal_in_progress()
            || self.inline_widget_exit_in_progress()
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
    pub(crate) fn finish_inline_widget_exit_animation_for_test(&mut self) {
        if let Some(closing) = &mut self.view.closing_inline_widget {
            closing.started_at = Instant::now() - INLINE_WIDGET_EXIT_DURATION * 2;
        }
    }

    /// Fast-forward entry/exit animations so captures render the settled
    /// frame instead of a mid-reveal state. Used by the headless gallery
    /// screenshot tool.
    pub(crate) fn settle_animations_for_capture(&mut self) {
        if let Some(opened_at) = &mut self.view.inline_widget_opened_at {
            *opened_at = Instant::now() - INLINE_WIDGET_REVEAL_DURATION * 2;
        }
        if let Some(closing) = &mut self.view.closing_inline_widget {
            closing.started_at = Instant::now() - INLINE_WIDGET_EXIT_DURATION * 2;
        }
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

    /// The standalone activity pill only shows while waiting for the first
    /// streamed token. Once text flows, the streaming tail cursor takes over
    /// as the "alive" cue at the end of the revealed text.
    pub(crate) fn streaming_activity_pill_visible(&self) -> bool {
        self.has_activity_indicator()
            && self.streaming_response.is_empty()
            // Live reasoning is itself the "alive" cue; the pill would be noise.
            && self.reasoning.live.trim().is_empty()
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
                    "thinking level set to {effort}"
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

    pub(crate) fn welcome_hero_text(&self) -> String {
        handwritten_welcome_phrase(self.welcome.hero_phrase_index).to_string()
    }

    pub(crate) fn welcome_continuation_suggestion(&self) -> Option<&str> {
        self.welcome.continuation_suggestion.as_deref()
    }

    pub(crate) fn is_welcome_timeline_visible(&self) -> bool {
        self.welcome.timeline
            && !self.show_help
            && !self.show_session_info
            && !self.session_switcher.open
            && self.stdin_response.is_none()
    }

    pub(crate) fn has_welcome_timeline_transcript(&self) -> bool {
        !self.messages.is_empty()
            || !self.streaming_response.is_empty()
            || !self.reasoning.live.trim().is_empty()
            || self.error.is_some()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_fresh_welcome_visible(&self) -> bool {
        self.session.is_none()
            && self.live_session_id.is_none()
            && self.messages.is_empty()
            && self.streaming_response.is_empty()
            && self.reasoning.live.trim().is_empty()
            && self.status.is_none()
            && self.error.is_none()
            && self.pending_images.is_empty()
            && !self.show_help
            && !self.model_picker.open
            && !self.session_switcher.open
            && self.stdin_response.is_none()
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
            "Enter\u{a0}send · Ctrl+Enter\u{a0}send · Shift+Enter\u{a0}newline · Ctrl+V\u{a0}paste · Ctrl+U\u{a0}clear · Esc\u{a0}cancel",
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
    let prefix_key = prefix.to_ascii_lowercase();
    let matches = completions
        .iter()
        .copied()
        .filter(|command| command.starts_with(&prefix_key))
        .collect::<Vec<_>>();
    let completion = match matches.as_slice() {
        [] => fuzzy_slash_completion(&prefix_key, completions)?,
        [only] => *only,
        _ => longest_common_prefix(&matches)?,
    };
    if completion.eq_ignore_ascii_case(prefix) {
        return None;
    }
    let mut completed = completion.to_string();
    completed.push_str(suffix);
    Some((completed, completion.len()))
}

fn fuzzy_slash_completion(needle: &str, completions: &[&'static str]) -> Option<&'static str> {
    let mut matches = completions
        .iter()
        .copied()
        .filter_map(|command| {
            desktop_slash_fuzzy_score(needle, command).map(|score| (score, command.len(), command))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(b.2))
    });
    matches.first().map(|(_, _, command)| *command)
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
        transcript_messages: Vec::new(),
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
mod tests;
