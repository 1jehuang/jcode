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
const DESKTOP_REASONING_EFFORTS_ANTHROPIC_STANDARD: &[&str] = &["none", "low", "medium", "high"];
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct GitHubIssueSyncUiState {
    pub(crate) syncing: bool,
    pub(crate) last_message: Option<String>,
    pub(crate) last_error: Option<String>,
}

impl GitHubIssueSyncUiState {
    pub(crate) fn label(&self) -> Option<String> {
        if self.syncing {
            return Some("syncing from GitHub in the background".to_string());
        }
        if let Some(error) = &self.last_error {
            return Some(format!("sync failed · {error}"));
        }
        self.last_message.clone()
    }

    pub(crate) fn guidance(&self) -> Option<String> {
        let error = self.last_error.as_deref()?;
        Some(issue_sync_error_guidance(error).to_string())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GitHubIssueBrowserState {
    pub(crate) repo: String,
    pub(crate) filter_label: String,
    pub(crate) selected: usize,
    pub(crate) list_scroll: usize,
    pub(crate) preview_scroll: usize,
    pub(crate) issues: Vec<GitHubIssuePreview>,
}

impl GitHubIssueBrowserState {
    fn sample() -> Self {
        Self {
            repo: "1jehuang/jcode".to_string(),
            filter_label: "priority · open · local cache".to_string(),
            selected: 0,
            list_scroll: 0,
            preview_scroll: 0,
            issues: vec![
                GitHubIssuePreview {
                    number: 342,
                    priority: "P0".to_string(),
                    title: "Desktop reload can lose the active chat surface".to_string(),
                    labels: vec!["bug".to_string(), "desktop".to_string(), "regression".to_string()],
                    age: "2d".to_string(),
                    comments: 8,
                    state: GitHubIssueVisualState::Selected,
                    body_lines: vec![
                        "When the desktop process reloads while a session is streaming, the window sometimes returns to the welcome state instead of the active chat.".to_string(),
                        "Expected: reload handoff preserves the session id, transcript, draft, and scroll position.".to_string(),
                        "Observed: the app opens, paints the shell, then falls back to a fresh session.".to_string(),
                    ],
                    comment_lines: vec![
                        "maintainer: happens more often after resizing during handoff".to_string(),
                        "agent note: likely snapshot restore ordering or worker init race".to_string(),
                    ],
                    priority_reason: "explicit regression label, data-loss risk, bounded desktop repro".to_string(),
                },
                GitHubIssuePreview {
                    number: 337,
                    priority: "P1".to_string(),
                    title: "Tool-card animation does too much work offscreen".to_string(),
                    labels: vec!["performance".to_string(), "desktop".to_string()],
                    age: "5d".to_string(),
                    comments: 4,
                    state: GitHubIssueVisualState::Idle,
                    body_lines: vec![
                        "Large transcripts still spend frame time walking tool-card metadata for rows far outside the viewport.".to_string(),
                        "The UI remains correct, but long sessions can miss frame budget during streaming.".to_string(),
                    ],
                    comment_lines: vec![
                        "profiling: check viewport clipping before card motion".to_string(),
                    ],
                    priority_reason: "perf label plus objective frame-time validation path".to_string(),
                },
                GitHubIssuePreview {
                    number: 329,
                    priority: "P2".to_string(),
                    title: "Provider auth errors should link to doctor output".to_string(),
                    labels: vec!["auth".to_string(), "ux".to_string()],
                    age: "1w".to_string(),
                    comments: 2,
                    state: GitHubIssueVisualState::Idle,
                    body_lines: vec![
                        "Desktop auth failures currently show a terse provider error.".to_string(),
                        "It should offer a one-click path to the same diagnostic information as `jcode auth doctor`.".to_string(),
                    ],
                    comment_lines: vec!["nice to have after core desktop stability".to_string()],
                    priority_reason: "important UX improvement, but not blocking active work".to_string(),
                },
            ],
        }
    }

    pub(crate) fn selected_issue(&self) -> Option<&GitHubIssuePreview> {
        self.issues.get(self.selected)
    }

    fn selected_issue_mut(&mut self) -> Option<&mut GitHubIssuePreview> {
        self.issues.get_mut(self.selected)
    }

    fn select_first(&mut self) {
        self.set_selected(0);
    }

    fn select_last(&mut self) {
        self.set_selected(self.issues.len().saturating_sub(1));
    }

    fn move_selection(&mut self, delta: i32) {
        if self.issues.is_empty() {
            self.selected = 0;
            self.list_scroll = 0;
            self.preview_scroll = 0;
            return;
        }
        let selected = self.selected as i32 + delta;
        self.set_selected(selected.clamp(0, self.issues.len().saturating_sub(1) as i32) as usize);
    }

    fn set_selected(&mut self, selected: usize) {
        if self.issues.is_empty() {
            self.selected = 0;
            self.list_scroll = 0;
            self.preview_scroll = 0;
            return;
        }
        self.selected = selected.min(self.issues.len() - 1);
        self.preview_scroll = 0;
        let visible_rows = 6usize;
        if self.selected < self.list_scroll {
            self.list_scroll = self.selected;
        } else if self.selected >= self.list_scroll.saturating_add(visible_rows) {
            self.list_scroll = self.selected.saturating_sub(visible_rows - 1);
        }
        self.sync_visual_selection_state();
    }

    fn sync_visual_selection_state(&mut self) {
        for (index, issue) in self.issues.iter_mut().enumerate() {
            if issue.state != GitHubIssueVisualState::Active {
                issue.state = if index == self.selected {
                    GitHubIssueVisualState::Selected
                } else {
                    GitHubIssueVisualState::Idle
                };
            }
        }
    }

    fn scroll_preview_lines(&mut self, lines: i32) {
        let max_scroll = self
            .selected_issue()
            .map(|issue| issue.body_lines.len().saturating_sub(1))
            .unwrap_or_default();
        if lines > 0 {
            self.preview_scroll = self.preview_scroll.saturating_sub(lines as usize);
        } else if lines < 0 {
            self.preview_scroll = self
                .preview_scroll
                .saturating_add(lines.unsigned_abs() as usize)
                .min(max_scroll);
        }
    }

    fn mark_selected_active(&mut self) {
        for issue in &mut self.issues {
            if issue.state == GitHubIssueVisualState::Active {
                issue.state = GitHubIssueVisualState::Idle;
            }
        }
        if let Some(issue) = self.selected_issue_mut() {
            issue.state = GitHubIssueVisualState::Active;
        }
    }

    pub(crate) fn selected_issue_context_prompt(&self) -> Option<String> {
        let issue = self.selected_issue()?;
        Some(issue_context_prompt(&self.repo, issue))
    }
}

fn issue_context_prompt(repo: &str, issue: &GitHubIssuePreview) -> String {
    let labels = if issue.labels.is_empty() {
        "none".to_string()
    } else {
        issue.labels.join(", ")
    };
    let body = issue.body_lines.join("\n");
    let comments = if issue.comment_lines.is_empty() {
        "none".to_string()
    } else {
        issue.comment_lines.join("\n")
    };
    format!(
        "GitHub issue mission\n\nRepository: {repo}\nIssue: #{}\nTitle: {}\nPriority: {}\nLabels: {labels}\nAge: {}\nComment count: {}\nPriority rationale: {}\n\nIssue body:\n{body}\n\nRecent comments:\n{comments}\n\nMission objective: investigate and, when safe, implement a fix for this issue in the local repository.\n\nOperating instructions:\n1. Start by inspecting the relevant code and reproducing or narrowing the behavior.\n2. Preserve existing user changes and avoid destructive actions.\n3. If implementing a fix, add or update targeted tests.\n4. Run the maximum reasonable validation before reporting completion.\n5. Report evidence, remaining gaps, and any follow-up work.\n6. Do not rely on the GitHub web UI unless local cache context is insufficient.",
        issue.number, issue.title, issue.priority, issue.age, issue.comments, issue.priority_reason
    )
}

fn issue_sync_error_guidance(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if lower.contains("not installed")
        || lower.contains("not on path")
        || lower.contains("no such file")
    {
        "Install GitHub CLI `gh`, authenticate it, then press r or Ctrl+R to sync."
    } else if lower.contains("auth")
        || lower.contains("authentication")
        || lower.contains("login")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
    {
        "Run `gh auth login` or refresh GitHub CLI auth, then press r or Ctrl+R to sync."
    } else if lower.contains("could not find a github origin") || lower.contains("origin remote") {
        "Add a GitHub origin remote for this repository, then press r or Ctrl+R to sync."
    } else {
        "Using cached GitHub issues. Press r or Ctrl+R to retry background sync."
    }
}

fn compact_issue_sync_error(error: &str) -> String {
    let mut compact = error.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.len() > 160 {
        compact.truncate(157);
        compact.push_str("...");
    }
    if compact.is_empty() {
        "unknown error".to_string()
    } else {
        compact
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GitHubIssueVisualState {
    Idle,
    Selected,
    Active,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GitHubIssuePreview {
    pub(crate) number: u64,
    pub(crate) priority: String,
    pub(crate) title: String,
    pub(crate) labels: Vec<String>,
    pub(crate) age: String,
    pub(crate) comments: u32,
    pub(crate) state: GitHubIssueVisualState,
    pub(crate) body_lines: Vec<String>,
    pub(crate) comment_lines: Vec<String>,
    pub(crate) priority_reason: String,
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
        Self {
            name,
            recovery_session_count: 0,
            continuation_suggestion: latest_external_cli_continuation_suggestion(),
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
            Self::SessionInfo => 10,
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
    search_texts: Vec<String>,
    visible_indices: Vec<usize>,
    pub(crate) error: Option<String>,
}

impl ModelPickerState {
    fn open_loading(&mut self) {
        self.open = true;
        self.loading = true;
        self.preview = false;
        self.error = None;
        self.refresh_visible_indices();
        self.selected = self.current_choice_index().unwrap_or(0);
        self.column = 0;
    }

    fn open_preview_loading(&mut self, filter: String) {
        self.open = true;
        self.loading = true;
        self.preview = true;
        self.filter = filter;
        self.error = None;
        self.refresh_visible_indices();
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
            self.rebuild_search_texts();
        }
        self.loading = false;
        self.error = None;
        self.ensure_current_choice_present();
        self.refresh_visible_indices();
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
        self.refresh_visible_indices();
        self.selected = self.current_visible_position().unwrap_or(self.selected);
        self.clamp_selection();
    }

    fn selected_model(&self) -> Option<String> {
        let visible = self.filtered_indices();
        visible
            .get(self.selected)
            .and_then(|index| self.choices.get(*index))
            .map(desktop_model_choice_switch_spec)
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
        self.refresh_visible_indices();
        self.selected = 0;
        self.column = 0;
    }

    fn pop_filter_char(&mut self) {
        self.filter.pop();
        self.refresh_visible_indices();
        self.selected = 0;
        self.column = 0;
    }

    fn set_filter(&mut self, filter: String) {
        if self.filter != filter {
            self.filter = filter;
            self.refresh_visible_indices();
            self.selected = 0;
            self.column = 0;
        }
        self.clamp_selection();
    }

    fn filtered_indices(&self) -> &[usize] {
        &self.visible_indices
    }

    fn refresh_visible_indices(&mut self) {
        self.ensure_search_texts_current();
        let query = self.filter.trim().to_lowercase();
        if query.is_empty() {
            self.visible_indices = (0..self.choices.len()).collect();
            return;
        }

        let substring_matches = self
            .search_texts
            .iter()
            .enumerate()
            .filter_map(|(index, search_text)| search_text.contains(&query).then_some(index))
            .collect::<Vec<_>>();
        if !substring_matches.is_empty() {
            self.visible_indices = substring_matches;
            return;
        }

        let mut fuzzy_matches = Vec::new();
        for (index, search_text) in self.search_texts.iter().enumerate() {
            if let Some(score) = model_picker_fuzzy_score(&query, search_text) {
                fuzzy_matches.push((score, search_text.len(), index));
            }
        }
        fuzzy_matches.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.cmp(&b.2))
        });
        self.visible_indices = fuzzy_matches
            .into_iter()
            .map(|(_, _, index)| index)
            .collect();
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

    fn rebuild_search_texts(&mut self) {
        self.search_texts = self.choices.iter().map(model_choice_search_text).collect();
    }

    fn ensure_search_texts_current(&mut self) {
        if self.search_texts.len() != self.choices.len() {
            self.rebuild_search_texts();
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
        let choice = DesktopModelChoice {
            model: current_model,
            provider: self.provider_name.clone(),
            api_method: Some("current".to_string()),
            detail: Some("current model".to_string()),
            available: true,
        };
        let search_text = model_choice_search_text(&choice);
        self.choices.insert(0, choice);
        self.search_texts.insert(0, search_text);
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub(crate) struct SessionSwitcherState {
    pub(crate) open: bool,
    pub(crate) loading: bool,
    pub(crate) preview: bool,
    pub(crate) filter: String,
    pub(crate) selected: usize,
    pub(crate) sessions: Vec<workspace::SessionCard>,
    visible_indices: Vec<usize>,
    preview_scroll: usize,
    pub(crate) focus: SessionSwitcherPane,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) enum SessionSwitcherPane {
    #[default]
    Sessions,
    Preview,
}

impl SessionSwitcherState {
    fn open_loading(&mut self, current_session_id: Option<&str>) {
        self.open_loading_with_filter(current_session_id, String::new(), false);
    }

    fn open_preview_loading(&mut self, current_session_id: Option<&str>, filter: String) {
        self.open_loading_with_filter(current_session_id, filter, true);
    }

    fn refresh_loading(&mut self, current_session_id: Option<&str>) {
        let filter = self.filter.clone();
        let preview = self.preview;
        self.open_loading_with_filter(current_session_id, filter, preview);
    }

    fn open_loading_with_filter(
        &mut self,
        current_session_id: Option<&str>,
        filter: String,
        preview: bool,
    ) {
        self.open = true;
        self.loading = true;
        self.preview = preview;
        self.filter = filter;
        self.refresh_visible_indices();
        self.focus = SessionSwitcherPane::Sessions;
        self.preview_scroll = 0;
        self.selected = self
            .current_visible_position(current_session_id)
            .unwrap_or(self.selected);
        self.clamp_selection();
    }

    fn close(&mut self) {
        self.open = false;
        self.loading = false;
        self.preview = false;
    }

    fn apply_sessions(
        &mut self,
        sessions: Vec<workspace::SessionCard>,
        current_session_id: Option<&str>,
    ) {
        self.sessions = sessions;
        self.refresh_visible_indices();
        self.loading = false;
        self.selected = self
            .current_visible_position(current_session_id)
            .unwrap_or(0);
        self.preview_scroll = 0;
        self.clamp_selection();
    }

    fn selected_session(&self) -> Option<workspace::SessionCard> {
        self.selected_session_ref().cloned()
    }

    fn selected_session_ref(&self) -> Option<&workspace::SessionCard> {
        let visible = self.filtered_indices();
        visible
            .get(self.selected)
            .and_then(|index| self.sessions.get(*index))
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
        self.preview_scroll = 0;
    }

    fn select_first(&mut self) {
        self.selected = 0;
        self.preview_scroll = 0;
    }

    fn select_last(&mut self) {
        self.selected = self.filtered_indices().len().saturating_sub(1);
        self.preview_scroll = 0;
    }

    fn push_filter_text(&mut self, text: &str) {
        self.filter.push_str(text);
        self.refresh_visible_indices();
        self.selected = 0;
        self.preview_scroll = 0;
    }

    fn pop_filter_char(&mut self) {
        self.filter.pop();
        self.refresh_visible_indices();
        self.selected = 0;
        self.preview_scroll = 0;
    }

    fn set_filter(&mut self, filter: String) {
        if self.filter != filter {
            self.filter = filter;
            self.refresh_visible_indices();
            self.selected = 0;
            self.preview_scroll = 0;
        }
        self.clamp_selection();
    }

    fn filtered_indices(&self) -> &[usize] {
        &self.visible_indices
    }

    fn refresh_visible_indices(&mut self) {
        let query = self.filter.trim().to_lowercase();
        if query.is_empty() {
            self.visible_indices = (0..self.sessions.len()).collect();
            return;
        }

        let mut substring_matches = Vec::new();
        let mut fuzzy_matches = Vec::new();
        for (index, session) in self.sessions.iter().enumerate() {
            let search_text = session_card_search_text(session);
            if search_text.contains(&query) {
                substring_matches.push(index);
            } else if let Some(score) = session_switcher_fuzzy_score(&query, &search_text) {
                fuzzy_matches.push((score, search_text.len(), index));
            }
        }
        fuzzy_matches.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.cmp(&b.2))
        });

        self.visible_indices = substring_matches
            .into_iter()
            .chain(fuzzy_matches.into_iter().map(|(_, _, index)| index))
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

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            SessionSwitcherPane::Sessions => SessionSwitcherPane::Preview,
            SessionSwitcherPane::Preview => SessionSwitcherPane::Sessions,
        };
    }

    fn focus_sessions(&mut self) {
        self.focus = SessionSwitcherPane::Sessions;
    }

    fn focus_preview(&mut self) {
        self.focus = SessionSwitcherPane::Preview;
    }

    fn scroll_preview(&mut self, delta: i32) {
        if delta < 0 {
            self.preview_scroll = self
                .preview_scroll
                .saturating_sub(delta.unsigned_abs() as usize);
        } else {
            self.preview_scroll = self.preview_scroll.saturating_add(delta as usize);
        }
        let max_scroll = self.preview_line_count().saturating_sub(1);
        self.preview_scroll = self.preview_scroll.min(max_scroll);
    }

    fn preview_line_count(&self) -> usize {
        self.selected_session_ref()
            .map(session_switcher_preview_line_count_for_session)
            .unwrap_or(0)
    }

    fn visible_row_window(&self, limit: usize) -> (usize, Vec<usize>) {
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

    pub(crate) fn take_github_issue_sync_request(&mut self) -> bool {
        std::mem::take(&mut self.pending_issue_sync_request)
    }

    pub(crate) fn note_github_issue_sync_already_running(&mut self) {
        self.side_panel.github_issue_sync.syncing = true;
        self.side_panel.github_issue_sync.last_error = None;
        self.side_panel.github_issue_sync.last_message =
            Some("GitHub issue sync already running; cached issues remain interactive".to_string());
    }

    pub(crate) fn apply_github_issue_sync_result(
        &mut self,
        result: std::result::Result<crate::desktop_issue_cache::GitHubIssueSyncSummary, String>,
    ) {
        self.pending_issue_sync_request = false;
        self.side_panel.github_issue_sync.syncing = false;
        match result {
            Ok(summary) => {
                let warning_label = if summary.comment_fetch_errors == 0 {
                    String::new()
                } else {
                    format!(
                        " · {} comment refresh warning(s)",
                        summary.comment_fetch_errors
                    )
                };
                let message = format!(
                    "synced {} GitHub issues for {} in {}ms · cache {}{}",
                    summary.issue_count,
                    summary.repo,
                    summary.elapsed.as_millis(),
                    summary.cache_path.display(),
                    warning_label
                );
                self.side_panel.github_issues = summary.browser;
                self.side_panel.github_issue_sync.last_error = None;
                self.side_panel.github_issue_sync.last_message = Some(message.clone());
                self.set_status(SingleSessionStatus::Info(message));
            }
            Err(error) => {
                let compact_error = compact_issue_sync_error(&error);
                self.side_panel.github_issue_sync.last_error = Some(compact_error.clone());
                self.side_panel.github_issue_sync.last_message =
                    Some(issue_sync_error_guidance(&error).to_string());
                self.set_status(SingleSessionStatus::Info(format!(
                    "GitHub issue sync failed · {compact_error}"
                )));
            }
        }
    }

    pub(crate) fn issue_browser_visible(&self) -> bool {
        self.side_panel.visible
    }

    fn request_issue_browser_sync(&mut self) {
        self.pending_issue_sync_request = true;
        self.side_panel.github_issue_sync.syncing = true;
        self.side_panel.github_issue_sync.last_error = None;
        self.side_panel.github_issue_sync.last_message =
            Some("syncing from GitHub via gh; cached issues remain interactive".to_string());
    }

    fn toggle_issue_browser(&mut self, visible: Option<bool>) -> KeyOutcome {
        let visible = visible.unwrap_or(!self.side_panel.visible);
        self.side_panel.visible = visible;
        self.side_panel.focus = if visible {
            DesktopSidePanelFocus::IssueList
        } else {
            DesktopSidePanelFocus::Chat
        };
        let cache_status = visible
            .then(|| self.refresh_issue_browser_from_cache())
            .flatten();
        if visible {
            self.request_issue_browser_sync();
        }
        self.draft.clear();
        self.draft_cursor = 0;
        self.composer.input_undo_stack.clear();
        self.set_status(SingleSessionStatus::Info(cache_status.unwrap_or_else(
            || {
                if visible {
                    "showing local GitHub issue browser".to_string()
                } else {
                    "hid local GitHub issue browser".to_string()
                }
            },
        )));
        KeyOutcome::Redraw
    }

    #[cfg(not(test))]
    fn refresh_issue_browser_from_cache(&mut self) -> Option<String> {
        match crate::desktop_issue_cache::load_current_repo_issue_browser() {
            Ok(Some(browser)) => {
                let repo = browser.repo.clone();
                let count = browser.issues.len();
                self.side_panel.github_issues = browser;
                Some(format!("showing {count} cached GitHub issues for {repo}"))
            }
            Ok(None) => None,
            Err(error) => Some(format!(
                "showing sample issues; cache unavailable: {error:#}"
            )),
        }
    }

    #[cfg(test)]
    fn refresh_issue_browser_from_cache(&mut self) -> Option<String> {
        None
    }

    fn handle_issue_browser_key(&mut self, key: &KeyInput) -> Option<KeyOutcome> {
        if !self.side_panel.visible {
            return None;
        }

        if matches!(key, KeyInput::Autocomplete) && self.draft.is_empty() {
            self.side_panel.focus_next();
            return Some(KeyOutcome::Redraw);
        }

        if let KeyInput::Character(text) = key
            && text.starts_with('/')
        {
            self.side_panel.focus = DesktopSidePanelFocus::Chat;
            return None;
        }

        if matches!(key, KeyInput::RefreshSessions) {
            self.request_issue_browser_sync();
            return Some(KeyOutcome::Redraw);
        }

        match self.side_panel.focus {
            DesktopSidePanelFocus::Chat => None,
            DesktopSidePanelFocus::IssueList => Some(self.handle_issue_list_key(key)),
            DesktopSidePanelFocus::IssuePreview => Some(self.handle_issue_preview_key(key)),
        }
    }

    fn handle_issue_list_key(&mut self, key: &KeyInput) -> KeyOutcome {
        match key {
            KeyInput::Escape => {
                self.side_panel.focus = DesktopSidePanelFocus::Chat;
                KeyOutcome::Redraw
            }
            KeyInput::SubmitDraft => self.investigate_selected_issue(),
            KeyInput::Character(text) if text.eq_ignore_ascii_case("r") => {
                self.request_issue_browser_sync();
                KeyOutcome::Redraw
            }
            KeyInput::ModelPickerMove(delta) => {
                self.side_panel.github_issues.move_selection(*delta);
                KeyOutcome::Redraw
            }
            KeyInput::ScrollBodyPages(pages) => {
                self.side_panel.github_issues.move_selection(-pages * 5);
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "j" => {
                self.side_panel.github_issues.move_selection(1);
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "k" => {
                self.side_panel.github_issues.move_selection(-1);
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "g" => {
                self.side_panel.github_issues.select_first();
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "G" => {
                self.side_panel.github_issues.select_last();
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "l" => {
                self.side_panel.focus = DesktopSidePanelFocus::IssuePreview;
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "h" => {
                self.side_panel.focus_previous();
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text.eq_ignore_ascii_case("q") => {
                self.toggle_issue_browser(Some(false))
            }
            _ => KeyOutcome::None,
        }
    }

    fn handle_issue_preview_key(&mut self, key: &KeyInput) -> KeyOutcome {
        match key {
            KeyInput::Escape => {
                self.side_panel.focus = DesktopSidePanelFocus::Chat;
                KeyOutcome::Redraw
            }
            KeyInput::SubmitDraft => self.investigate_selected_issue(),
            KeyInput::Character(text) if text.eq_ignore_ascii_case("r") => {
                self.request_issue_browser_sync();
                KeyOutcome::Redraw
            }
            KeyInput::ScrollBodyLines(lines) => {
                self.side_panel.github_issues.scroll_preview_lines(*lines);
                KeyOutcome::Redraw
            }
            KeyInput::ScrollBodyPages(pages) => {
                self.side_panel
                    .github_issues
                    .scroll_preview_lines(*pages * 6);
                KeyOutcome::Redraw
            }
            KeyInput::ScrollBodyToTop => {
                self.side_panel.github_issues.preview_scroll = 0;
                KeyOutcome::Redraw
            }
            KeyInput::ScrollBodyToBottom => {
                self.side_panel
                    .github_issues
                    .scroll_preview_lines(i32::MIN + 1);
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "j" => {
                self.side_panel.github_issues.scroll_preview_lines(-1);
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "k" => {
                self.side_panel.github_issues.scroll_preview_lines(1);
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "h" => {
                self.side_panel.focus = DesktopSidePanelFocus::IssueList;
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text == "l" => {
                self.side_panel.focus = DesktopSidePanelFocus::Chat;
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) if text.eq_ignore_ascii_case("q") => {
                self.toggle_issue_browser(Some(false))
            }
            KeyInput::ModelPickerMove(delta) => {
                self.side_panel.github_issues.move_selection(*delta);
                KeyOutcome::Redraw
            }
            _ => KeyOutcome::None,
        }
    }

    fn investigate_selected_issue(&mut self) -> KeyOutcome {
        let Some(message) = self
            .side_panel
            .github_issues
            .selected_issue_context_prompt()
        else {
            return KeyOutcome::None;
        };
        self.side_panel.github_issues.mark_selected_active();
        self.side_panel.focus = DesktopSidePanelFocus::Chat;
        self.record_user_submit(&message, &[]);
        if let Some(session) = &self.session {
            KeyOutcome::SendDraft {
                session_id: session.session_id.clone(),
                title: session.title.clone(),
                message,
                images: Vec::new(),
            }
        } else {
            KeyOutcome::StartFreshSession {
                message,
                images: Vec::new(),
            }
        }
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
        self.has_activity_indicator()
            || self.inline_widget_reveal_in_progress()
            || self.inline_widget_exit_in_progress()
    }

    fn mark_inline_widget_opened(&mut self) {
        self.view.inline_widget_opened_at = Some(Instant::now());
        self.view.closing_inline_widget = None;
    }

    fn capture_inline_widget_exit(&mut self) {
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

    fn capture_inline_widget_exit_snapshot(
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

    fn close_inline_widgets(&mut self) {
        self.capture_inline_widget_exit();
        self.show_help = false;
        self.show_session_info = false;
        self.model_picker.close();
        self.session_switcher.close();
        self.view.inline_widget_opened_at = None;
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

    fn inline_widget_exit_in_progress(&self) -> bool {
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

    fn active_inline_widget_line_count(&self, kind: InlineWidgetKind) -> usize {
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

    pub(crate) fn handle_key(&mut self, key: KeyInput) -> KeyOutcome {
        if key == KeyInput::ExitApp {
            return KeyOutcome::Exit;
        }

        if self.stdin_response.is_some() {
            return self.handle_stdin_response_key(key);
        }

        if self.session_switcher.open
            && self.session_switcher.preview
            && let Some(outcome) = self.handle_session_switcher_preview_key(&key)
        {
            return outcome;
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

        if let Some(outcome) = self.handle_issue_browser_key(&key) {
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
            KeyInput::CycleReasoningEffort(direction) => {
                KeyOutcome::CycleReasoningEffort(direction)
            }
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
                self.close_inline_widgets();
                KeyOutcome::Redraw
            }
            KeyInput::Escape if self.show_session_info => {
                self.close_inline_widgets();
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

    fn open_session_switcher_preview(&mut self, filter: String) -> KeyOutcome {
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

    fn sync_model_picker_preview_from_draft(&mut self) -> Option<KeyOutcome> {
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

    fn sync_session_switcher_preview_from_draft(&mut self) -> Option<KeyOutcome> {
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

    fn sync_inline_previews_from_draft(&mut self) -> Option<KeyOutcome> {
        self.sync_slash_suggestions_from_draft();
        self.sync_model_picker_preview_from_draft()
            .or_else(|| self.sync_session_switcher_preview_from_draft())
    }

    fn sync_slash_suggestions_from_draft(&mut self) {
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

    fn handle_slash_suggestion_key(&mut self, key: &KeyInput) -> Option<KeyOutcome> {
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

    fn handle_session_switcher_preview_key(&mut self, key: &KeyInput) -> Option<KeyOutcome> {
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

    fn handle_session_switcher_key(&mut self, key: KeyInput) -> KeyOutcome {
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
        let session_id = session.session_id.clone();
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
        self.hydrate_resumed_session_from_disk(&session_id);
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

    fn slash_suggestion_line_count(&self) -> usize {
        let candidate_count = self.slash_suggestion_candidate_count();
        if candidate_count == 0 {
            0
        } else {
            1 + candidate_count
        }
    }

    fn slash_suggestion_candidate_count(&self) -> usize {
        self.slash_suggestion_candidates().len()
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

        let mut prefix_matches = Vec::new();
        let mut fuzzy_matches: Vec<(usize, usize, &'static str, &'static str)> = Vec::new();
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
            a.0.cmp(&b.0)
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
                        let tool_run =
                            self.tool_run_for_message_index(group_start.saturating_add(offset));
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
                            tool_run,
                        );
                    }
                }
                continue;
            }
            append_chat_message_lines(&mut lines, message, &mut user_turn, false, None, None);
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
        hash_tool_cache_fingerprint(&self.tool, &mut hasher);
        self.status.hash(&mut hasher);
        self.error.hash(&mut hasher);
        self.show_help.hash(&mut hasher);
        self.show_session_info.hash(&mut hasher);
        self.model_picker.open.hash(&mut hasher);
        self.model_picker.filter.hash(&mut hasher);
        self.model_picker.selected.hash(&mut hasher);
        hash_session_switcher_cache_state(&self.session_switcher, &mut hasher);
        self.stdin_response.hash(&mut hasher);
        self.welcome.name.hash(&mut hasher);
        self.welcome.recovery_session_count.hash(&mut hasher);
        self.welcome.continuation_suggestion.hash(&mut hasher);
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
        hash_tool_cache_fingerprint(&self.tool, &mut hasher);
        self.status.hash(&mut hasher);
        self.error.hash(&mut hasher);
        self.show_help.hash(&mut hasher);
        self.show_session_info.hash(&mut hasher);
        self.model_picker.open.hash(&mut hasher);
        self.model_picker.filter.hash(&mut hasher);
        self.model_picker.selected.hash(&mut hasher);
        hash_session_switcher_cache_state(&self.session_switcher, &mut hasher);
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
            DesktopSessionEvent::ReloadProgress {
                step,
                message,
                success,
                output,
            } => {
                let marker = match success {
                    Some(true) => "✓ ",
                    Some(false) => "✗ ",
                    None => "",
                };
                let mut line = format!("reload {step}: {marker}{message}");
                if let Some(output) = output.as_deref().filter(|output| !output.trim().is_empty()) {
                    line.push_str(" — ");
                    line.push_str(output.trim());
                }
                self.messages.push(SingleSessionMessage::meta(line));
                self.set_status(SingleSessionStatus::Info(format!("reload: {message}")));
            }
            DesktopSessionEvent::RuntimeMetadata {
                connection_type,
                status_detail,
                upstream_provider,
            } => {
                if let Some(connection_type) = connection_type {
                    self.runtime_settings.connection_type = Some(connection_type);
                }
                if let Some(upstream_provider) = upstream_provider {
                    self.runtime_settings.upstream_provider = Some(upstream_provider);
                }
                if let Some(status_detail) = status_detail {
                    self.runtime_settings.status_detail = Some(status_detail.clone());
                    self.set_status(SingleSessionStatus::Info(status_detail));
                }
            }
            DesktopSessionEvent::TokenUsage {
                input,
                output,
                cache_read_input,
                cache_creation_input,
            } => {
                self.runtime_settings.token_usage = Some(SingleSessionTokenUsage {
                    input,
                    output,
                    cache_read_input,
                    cache_creation_input,
                });
            }
            DesktopSessionEvent::SystemNotice { title, message } => {
                let line = message
                    .as_deref()
                    .filter(|message| !message.trim().is_empty())
                    .map(|message| format!("{title}: {}", message.trim()))
                    .unwrap_or(title.clone());
                self.messages.push(SingleSessionMessage::meta(line));
                self.set_status(SingleSessionStatus::Info(title));
            }
            DesktopSessionEvent::SessionCloseRequested { reason } => {
                self.finish_streaming_response();
                self.is_processing = false;
                self.stdin_response = None;
                self.runtime.session_handle = None;
                self.set_status(SingleSessionStatus::Info(
                    "session close requested".to_string(),
                ));
                self.messages.push(SingleSessionMessage::meta(format!(
                    "session close requested by server: {reason}"
                )));
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
            DesktopSessionEvent::ToolStarted { id, name } => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                self.finish_streaming_response();
                self.collapse_active_tool_message();
                self.tool.input_buffer.clear();
                self.set_status(SingleSessionStatus::ToolPreparing(name.clone()));
                self.messages
                    .push(SingleSessionMessage::tool(format!("▾ {name} preparing")));
                self.tool.active_message_index = Some(self.messages.len().saturating_sub(1));
                let message_index = self.messages.len().saturating_sub(1);
                self.start_tool_run(id, &name, message_index);
            }
            DesktopSessionEvent::ToolExecuting { id, name } => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                self.finish_streaming_response();
                self.set_status(SingleSessionStatus::ToolUsing(name.clone()));
                self.update_tool_run_state(id, &name, SingleSessionToolVisualState::Running, None);
                self.replace_active_tool_header(&format!("▾ {name} running"));
            }
            DesktopSessionEvent::ToolInput { id, delta } => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                self.finish_streaming_response();
                self.append_tool_run_input(id, &delta);
                self.append_active_tool_input(&delta);
            }
            DesktopSessionEvent::ToolFinished {
                id,
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
                let finished_call_id = self.update_tool_run_state(
                    id,
                    &name,
                    if is_error {
                        SingleSessionToolVisualState::Failed
                    } else {
                        SingleSessionToolVisualState::Succeeded
                    },
                    Some(summary.clone()),
                );
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
                    let message_index = self.messages.len().saturating_sub(1);
                    self.tool.active_message_index = Some(message_index);
                    if let Some(run) = self
                        .tool
                        .runs
                        .iter_mut()
                        .find(|run| run.call_id == finished_call_id)
                    {
                        run.message_index = message_index;
                    }
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
                self.mark_tool_stdin_prompt(&tool_call_id, display_prompt);
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
                self.tool.active_call_id = None;
                self.tool.input_buffer.clear();
                self.clear_tool_stdin_prompts();
                self.set_status(SingleSessionStatus::Ready);
            }
            DesktopSessionEvent::Error(error) => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                self.finish_streaming_response();
                self.is_processing = false;
                self.stdin_response = None;
                self.runtime.session_handle = None;
                self.tool.active_message_index = None;
                self.tool.active_call_id = None;
                self.tool.input_buffer.clear();
                self.clear_tool_stdin_prompts();
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
                self.clear_tool_stdin_prompts();
                self.set_status(SingleSessionStatus::Cancelling);
                true
            }
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                self.is_processing = false;
                self.stdin_response = None;
                self.clear_tool_stdin_prompts();
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

        if self.active_inline_widget() == Some(InlineWidgetKind::SlashSuggestions) {
            self.capture_inline_widget_exit();
        }

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
            "/issues" => {
                if matches!(args, "refresh" | "sync") {
                    return Some(self.toggle_issue_browser(Some(true)));
                }
                if args == "preview" {
                    let outcome = self.toggle_issue_browser(Some(true));
                    self.side_panel.focus = DesktopSidePanelFocus::IssuePreview;
                    return Some(outcome);
                }
                let visible = match args {
                    "on" | "open" | "show" => Some(true),
                    "off" | "close" | "hide" => Some(false),
                    _ => None,
                };
                self.toggle_issue_browser(visible)
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
            "/reload" | "/force-reload" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                self.set_status(SingleSessionStatus::Info(
                    "force reloading desktop".to_string(),
                ));
                KeyOutcome::ForceReload
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
                        "effort: {current} · use /effort <none|low|medium|high|xhigh|max>"
                    )));
                    KeyOutcome::Redraw
                } else if matches!(args, "none" | "low" | "medium" | "high" | "xhigh" | "max") {
                    KeyOutcome::SetReasoningEffort(args.to_string())
                } else {
                    self.set_status(SingleSessionStatus::Info(
                        "usage: /effort <none|low|medium|high|xhigh|max>".to_string(),
                    ));
                    KeyOutcome::Redraw
                }
            }
            "/font" | "/fonts" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                let mut args = args.split_whitespace();
                match (args.next(), args.collect::<Vec<_>>().join(" ")) {
                    (None, _) | (Some("status"), _) => {
                        let options = SINGLE_SESSION_HANDWRITING_FONT_FAMILIES.join(", ");
                        self.set_status(SingleSessionStatus::Info(format!(
                            "fonts: user={} · ai={} · options: default, {options}",
                            single_session_user_font_family(),
                            single_session_assistant_font_family()
                        )));
                        KeyOutcome::Redraw
                    }
                    (Some("user"), value) if !value.is_empty() => {
                        if let Some(family) = set_single_session_user_font_family(&value) {
                            self.set_status(SingleSessionStatus::Info(format!(
                                "user font set to {family}"
                            )));
                        } else {
                            self.set_status(SingleSessionStatus::Info(
                                "unknown font · try /font status".to_string(),
                            ));
                        }
                        KeyOutcome::Redraw
                    }
                    (Some("ai" | "assistant"), value) if !value.is_empty() => {
                        if let Some(family) = set_single_session_assistant_font_family(&value) {
                            self.set_status(SingleSessionStatus::Info(format!(
                                "AI font set to {family}"
                            )));
                        } else {
                            self.set_status(SingleSessionStatus::Info(
                                "unknown font · try /font status".to_string(),
                            ));
                        }
                        KeyOutcome::Redraw
                    }
                    _ => {
                        self.set_status(SingleSessionStatus::Info(
                            "usage: /font [status|user <name>|ai <name>]".to_string(),
                        ));
                        KeyOutcome::Redraw
                    }
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
            "/commit" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                let message = desktop_commit_prompt();
                let Some(session) = &self.session else {
                    return Some(KeyOutcome::StartFreshSession {
                        message,
                        images: Vec::new(),
                    });
                };
                let session_id = session.session_id.clone();
                let title = session.title.clone();
                self.set_status(SingleSessionStatus::Info(
                    "starting logical commits".to_string(),
                ));
                return Some(KeyOutcome::SendDraft {
                    session_id,
                    title,
                    message,
                    images: Vec::new(),
                });
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
            "/status" | "/info" => {
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
        self.clear_tool_stdin_prompts();
        self.set_status(SingleSessionStatus::Info(
            "interactive input sent".to_string(),
        ));
        Ok(())
    }

    pub(crate) fn set_reasoning_effort_via_active_session(
        &mut self,
        effort: String,
    ) -> anyhow::Result<()> {
        let Some(handle) = &self.runtime.session_handle else {
            anyhow::bail!("no active desktop session to receive reasoning effort change");
        };
        handle.set_reasoning_effort(effort)
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

    fn next_tool_event_sequence(&mut self) -> u64 {
        self.tool.event_sequence = self.tool.event_sequence.saturating_add(1);
        self.tool.event_sequence
    }

    fn start_tool_run(&mut self, id: Option<String>, name: &str, message_index: usize) -> String {
        let sequence = self.next_tool_event_sequence();
        let call_id =
            normalized_tool_call_id(id).unwrap_or_else(|| format!("desktop-tool-{sequence}"));
        if let Some(run) = self.tool.runs.iter_mut().find(|run| run.call_id == call_id) {
            run.message_index = message_index;
            run.name = name.to_string();
            run.state = SingleSessionToolVisualState::Preparing;
            run.summary = None;
            run.input_raw.clear();
            run.input_preview = None;
            run.stdin_prompt = None;
            run.updated_sequence = sequence;
            run.completed_sequence = None;
        } else {
            self.tool.runs.push(SingleSessionToolRun {
                call_id: call_id.clone(),
                message_index,
                name: name.to_string(),
                state: SingleSessionToolVisualState::Preparing,
                summary: None,
                input_raw: String::new(),
                input_preview: None,
                stdin_prompt: None,
                started_sequence: sequence,
                updated_sequence: sequence,
                completed_sequence: None,
            });
        }
        self.tool.active_call_id = Some(call_id.clone());
        call_id
    }

    fn update_tool_run_state(
        &mut self,
        id: Option<String>,
        name: &str,
        state: SingleSessionToolVisualState,
        summary: Option<String>,
    ) -> String {
        let sequence = self.next_tool_event_sequence();
        let call_id = normalized_tool_call_id(id)
            .or_else(|| self.tool.active_call_id.clone())
            .or_else(|| {
                self.tool
                    .runs
                    .iter()
                    .rev()
                    .find(|run| run.name == name && run.state.is_active())
                    .map(|run| run.call_id.clone())
            })
            .unwrap_or_else(|| format!("desktop-tool-{sequence}"));
        let message_index = self
            .tool
            .active_message_index
            .unwrap_or_else(|| self.messages.len().saturating_sub(1));
        let summary = summary.filter(|summary| !summary.trim().is_empty());
        if let Some(run) = self.tool.runs.iter_mut().find(|run| run.call_id == call_id) {
            run.message_index = message_index;
            run.name = name.to_string();
            run.state = state;
            run.summary = summary.clone();
            run.updated_sequence = sequence;
            run.completed_sequence = matches!(
                state,
                SingleSessionToolVisualState::Succeeded | SingleSessionToolVisualState::Failed
            )
            .then_some(sequence);
        } else {
            self.tool.runs.push(SingleSessionToolRun {
                call_id: call_id.clone(),
                message_index,
                name: name.to_string(),
                state,
                summary,
                input_raw: String::new(),
                input_preview: None,
                stdin_prompt: None,
                started_sequence: sequence,
                updated_sequence: sequence,
                completed_sequence: matches!(
                    state,
                    SingleSessionToolVisualState::Succeeded | SingleSessionToolVisualState::Failed
                )
                .then_some(sequence),
            });
        }
        self.tool.active_call_id = Some(call_id.clone());
        call_id
    }

    fn append_tool_run_input(&mut self, id: Option<String>, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let sequence = self.next_tool_event_sequence();
        let call_id = normalized_tool_call_id(id)
            .or_else(|| self.tool.active_call_id.clone())
            .or_else(|| self.tool.runs.last().map(|run| run.call_id.clone()));
        let Some(call_id) = call_id else {
            return;
        };
        if let Some(run) = self.tool.runs.iter_mut().find(|run| run.call_id == call_id) {
            run.input_raw.push_str(delta);
            run.input_preview =
                compact_tool_metadata(&formatted_tool_input_lines(&run.name, &run.input_raw));
            run.updated_sequence = sequence;
        }
    }

    fn mark_tool_stdin_prompt(&mut self, tool_call_id: &str, prompt: &str) {
        let sequence = self.next_tool_event_sequence();
        if let Some(run) = self
            .tool
            .runs
            .iter_mut()
            .rev()
            .find(|run| run.call_id == tool_call_id)
        {
            run.stdin_prompt = Some(prompt.to_string());
            run.updated_sequence = sequence;
            return;
        }
        if let Some(run) = self
            .tool
            .runs
            .iter_mut()
            .rev()
            .find(|run| run.state.is_active())
        {
            run.stdin_prompt = Some(prompt.to_string());
            run.updated_sequence = sequence;
        }
    }

    fn clear_tool_stdin_prompts(&mut self) {
        let sequence = self.next_tool_event_sequence();
        for run in &mut self.tool.runs {
            if run.stdin_prompt.is_some() {
                run.stdin_prompt = None;
                run.updated_sequence = sequence;
            }
        }
    }

    fn tool_run_for_message_index(&self, message_index: usize) -> Option<&SingleSessionToolRun> {
        self.tool
            .runs
            .iter()
            .find(|run| run.message_index == message_index)
    }

    fn collapse_active_tool_message(&mut self) {
        let Some(index) = self.tool.active_message_index.take() else {
            return;
        };
        self.tool.active_call_id = None;
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
            self.capture_inline_widget_exit();
            self.model_picker.close();
        }
        if self.session_switcher.open && self.session_switcher.preview {
            self.capture_inline_widget_exit();
            self.session_switcher.close();
        }
        self.set_status(SingleSessionStatus::Info(
            "Input cleared - Ctrl+Z to restore".to_string(),
        ));
        KeyOutcome::Redraw
    }

    fn autocomplete_draft(&mut self) -> KeyOutcome {
        let completions = DESKTOP_SLASH_COMMANDS
            .iter()
            .map(|(usage, _)| usage.split_whitespace().next().unwrap_or(*usage))
            .collect::<Vec<_>>();
        let Some((draft, cursor)) =
            complete_slash_command(&self.draft, self.draft_cursor, &completions)
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

#[derive(Clone, Debug)]
struct ExternalCliSessionCandidate {
    source: &'static str,
    modified: SystemTime,
    working_dir: Option<String>,
    context: Option<String>,
}

fn latest_external_cli_continuation_suggestion() -> Option<String> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    std::panic::catch_unwind(AssertUnwindSafe(|| {
        latest_external_cli_continuation_suggestion_from_home(&home)
    }))
    .ok()
    .flatten()
}

fn latest_external_cli_continuation_suggestion_from_home(home: &Path) -> Option<String> {
    let mut candidates = Vec::new();
    candidates.extend(latest_jsonl_candidates(
        &home.join(".codex/sessions"),
        "Codex",
        32,
    ));
    candidates.extend(latest_jsonl_candidates(
        &home.join(".claude/projects"),
        "Claude Code",
        32,
    ));
    latest_external_cli_continuation_suggestion_from_candidates(candidates)
}

fn latest_external_cli_continuation_suggestion_from_candidates(
    candidates: Vec<ExternalCliSessionCandidate>,
) -> Option<String> {
    let candidate = candidates
        .into_iter()
        .max_by_key(|candidate| candidate.modified)?;
    let location = candidate
        .working_dir
        .as_deref()
        .and_then(|dir| Path::new(dir).file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(|name| format!(" in {name}"))
        .unwrap_or_default();
    let context = candidate
        .context
        .as_deref()
        .map(|context| format!(": {}", compact_tool_text(context, 72)))
        .unwrap_or_default();
    Some(format!(
        "continue the latest {source} session{location}{context}",
        source = candidate.source
    ))
}

fn latest_jsonl_candidates(
    root: &Path,
    source: &'static str,
    scan_limit: usize,
) -> Vec<ExternalCliSessionCandidate> {
    if !root.is_dir() {
        return Vec::new();
    }
    let mut files = Vec::new();
    collect_recent_jsonl_files(root, &mut files, scan_limit.saturating_mul(8));
    files.sort_by(|a, b| b.1.cmp(&a.1));
    files.truncate(scan_limit);
    files
        .into_iter()
        .filter_map(|(path, modified)| external_cli_candidate_from_jsonl(&path, source, modified))
        .collect()
}

fn collect_recent_jsonl_files(
    root: &Path,
    files: &mut Vec<(PathBuf, SystemTime)>,
    max_files: usize,
) {
    if files.len() >= max_files {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if files.len() >= max_files {
            break;
        }
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            collect_recent_jsonl_files(&path, files, max_files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            files.push((path, metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
        }
    }
}

fn external_cli_candidate_from_jsonl(
    path: &Path,
    source: &'static str,
    modified: SystemTime,
) -> Option<ExternalCliSessionCandidate> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut working_dir = None;
    let mut last_user_text = None;
    let mut summary_text = None;
    for line in reader.lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        if working_dir.is_none() {
            working_dir = value
                .get("cwd")
                .or_else(|| value.get("payload").and_then(|payload| payload.get("cwd")))
                .and_then(|value| value.as_str())
                .map(str::to_string);
        }
        if summary_text.is_none() {
            summary_text = value
                .get("summary")
                .or_else(|| {
                    value
                        .get("payload")
                        .and_then(|payload| payload.get("summary"))
                })
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string);
        }
        if jsonl_message_role(&value) == Some("user")
            && let Some(text) = jsonl_message_text(&value)
            && !text.trim().is_empty()
        {
            last_user_text = Some(text);
        }
    }
    if working_dir.is_none() && last_user_text.is_none() && summary_text.is_none() {
        return None;
    }
    Some(ExternalCliSessionCandidate {
        source,
        modified,
        working_dir,
        context: last_user_text.or(summary_text),
    })
}

fn jsonl_message_role(value: &serde_json::Value) -> Option<&str> {
    value
        .get("message")
        .and_then(|message| message.get("role"))
        .or_else(|| value.get("role"))
        .or_else(|| value.get("payload").and_then(|payload| payload.get("role")))
        .and_then(|role| role.as_str())
}

fn jsonl_message_text(value: &serde_json::Value) -> Option<String> {
    let content = value
        .get("message")
        .and_then(|message| message.get("content"))
        .or_else(|| value.get("content"))
        .or_else(|| {
            value
                .get("payload")
                .and_then(|payload| payload.get("content"))
        })?;
    text_from_json_content(content)
}

fn text_from_json_content(value: &serde_json::Value) -> Option<String> {
    if let Some(text) = value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        return Some(text.to_string());
    }
    let blocks = value.as_array()?;
    let text = blocks
        .iter()
        .filter_map(|block| {
            block
                .get("text")
                .or_else(|| block.get("content"))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|text| !text.is_empty())
        })
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() { None } else { Some(text) }
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
    let visible = switcher.filtered_indices();
    let session_count = if switcher.filter.trim().is_empty() {
        switcher.sessions.len().to_string()
    } else {
        format!("{}/{}", visible.len(), switcher.sessions.len())
    };
    let filter_label = if switcher.filter.trim().is_empty() {
        "all".to_string()
    } else {
        format!("filter {}", switcher.filter.trim())
    };
    let selected_label = switcher
        .selected_session()
        .map(|session| compact_tool_text(&session.title, 44))
        .unwrap_or_else(|| "no session selected".to_string());
    let mut lines = vec![
        styled_line(
            format!("Resume sessions · {session_count} sessions · {filter_label}"),
            SingleSessionLineStyle::OverlayTitle,
        ),
        styled_line(
            "Type to filter · Up/Down select · Tab or Left/Right preview · PageUp/PageDown scroll · Enter resume here · Ctrl+Enter open terminal · Esc close",
            SingleSessionLineStyle::Overlay,
        ),
        styled_line(
            format!(
                "selected: {} · filter: {} · focus: {}",
                selected_label,
                if switcher.filter.is_empty() {
                    "<none>"
                } else {
                    switcher.filter.as_str()
                },
                session_switcher_focus_label(switcher.focus),
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

    const CARD_LIMIT: usize = 5;
    const BODY_ROW_LIMIT: usize = 9;
    const CONTENT_COLUMNS: usize = 92;

    let sessions_header = if switcher.focus == SessionSwitcherPane::Sessions {
        "Recent sessions · focused"
    } else {
        "Recent sessions"
    };
    lines.push(styled_line(
        format!("{sessions_header} · newest first"),
        SingleSessionLineStyle::OverlayTitle,
    ));

    let (window_start, row_indices) = switcher.visible_row_window(CARD_LIMIT);
    let row_count = row_indices.len();
    for (offset, index) in row_indices.iter().enumerate() {
        if let Some(session) = switcher.sessions.get(*index) {
            let position = window_start + offset;
            lines.extend(
                session_switcher_list_card_lines(
                    switcher,
                    current_session_id,
                    position,
                    session,
                    CONTENT_COLUMNS,
                )
                .into_iter()
                .map(|line| styled_line(line.text, line.style)),
            );
            if offset + 1 < row_count {
                lines.push(blank_styled_line());
            }
        }
    }

    if window_start + row_indices.len() < visible.len() {
        lines.push(styled_line(
            format!(
                "{} more sessions · keep pressing ↓ or type to filter",
                visible.len() - window_start - row_indices.len()
            ),
            SingleSessionLineStyle::Overlay,
        ));
    }

    lines.push(blank_styled_line());
    let preview_header = if switcher.focus == SessionSwitcherPane::Preview {
        "Preview · focused"
    } else {
        "Preview"
    };
    lines.push(styled_line(
        format!("{preview_header} · selected session transcript"),
        SingleSessionLineStyle::OverlayTitle,
    ));

    let preview_lines = switcher
        .selected_session()
        .map(|session| session_switcher_preview_lines_for_session(&session))
        .unwrap_or_else(|| {
            vec![SessionSwitcherRenderedLine::new(
                "No session selected".to_string(),
                SingleSessionLineStyle::Meta,
            )]
        });
    let preview_scroll = switcher
        .preview_scroll
        .min(preview_lines.len().saturating_sub(1));
    let preview_visible = preview_lines
        .iter()
        .skip(preview_scroll)
        .take(BODY_ROW_LIMIT)
        .cloned()
        .collect::<Vec<_>>();
    let preview_visible_len = preview_visible.len();
    lines.extend(
        preview_visible
            .into_iter()
            .map(|line| styled_line(truncate_chars(&line.text, CONTENT_COLUMNS), line.style)),
    );
    if preview_scroll > 0 || preview_scroll + preview_visible_len < preview_lines.len() {
        lines.push(styled_line(
            format!(
                "preview lines {}-{} of {}",
                preview_scroll + 1,
                preview_scroll + preview_visible_len,
                preview_lines.len()
            ),
            SingleSessionLineStyle::Meta,
        ));
    }

    lines
}

fn session_switcher_line_count(
    switcher: &SessionSwitcherState,
    current_session_id: Option<&str>,
) -> usize {
    let visible_len = switcher.filtered_indices().len();
    let mut count = 4;

    if switcher.loading {
        count += 1;
    }

    if visible_len == 0 && !switcher.loading {
        return count + 2;
    }

    const CARD_LIMIT: usize = 5;
    const BODY_ROW_LIMIT: usize = 9;
    count += 1; // Recent sessions header.
    let (window_start, window_len) = row_window_bounds(visible_len, switcher.selected, CARD_LIMIT);
    count += window_len * 4 + window_len.saturating_sub(1);
    if window_start + window_len < visible_len {
        count += 1;
    }

    count += 2; // Blank spacer and preview header.
    let preview_len = switcher
        .selected_session_ref()
        .map(session_switcher_preview_line_count_for_session)
        .unwrap_or(1);
    let preview_scroll = switcher.preview_scroll.min(preview_len.saturating_sub(1));
    let preview_visible_len = preview_len
        .saturating_sub(preview_scroll)
        .min(BODY_ROW_LIMIT);
    count += preview_visible_len;
    if preview_scroll > 0 || preview_scroll + preview_visible_len < preview_len {
        count += 1;
    }

    let _ = current_session_id;
    count
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SessionSwitcherRenderedLine {
    text: String,
    style: SingleSessionLineStyle,
}

impl SessionSwitcherRenderedLine {
    fn new(text: impl Into<String>, style: SingleSessionLineStyle) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }
}

fn session_switcher_focus_label(focus: SessionSwitcherPane) -> &'static str {
    match focus {
        SessionSwitcherPane::Sessions => "sessions",
        SessionSwitcherPane::Preview => "preview",
    }
}

fn session_switcher_list_card_lines(
    switcher: &SessionSwitcherState,
    current_session_id: Option<&str>,
    position: usize,
    session: &workspace::SessionCard,
    width: usize,
) -> Vec<SessionSwitcherRenderedLine> {
    let current_marker = if Some(session.session_id.as_str()) == current_session_id {
        "current · "
    } else {
        ""
    };
    let selected = position == switcher.selected;
    let primary_style = if selected {
        SingleSessionLineStyle::OverlaySelection
    } else {
        SingleSessionLineStyle::Overlay
    };
    let meta_style = if selected {
        SingleSessionLineStyle::OverlaySelection
    } else {
        SingleSessionLineStyle::Meta
    };
    let preview_style = if selected {
        SingleSessionLineStyle::OverlaySelection
    } else {
        SingleSessionLineStyle::Overlay
    };
    let status = session_status_badge(session);
    let model = session_model_label(session).unwrap_or_else(|| "model unknown".to_string());
    let preview = session
        .preview_lines
        .last()
        .or_else(|| session.detail_lines.last())
        .map(|line| session_switcher_compact_transcript_line(line, width.saturating_sub(8)))
        .unwrap_or_else(|| "no transcript preview yet".to_string());
    let card_text = |text: String| -> String {
        format!("      {}", truncate_chars(&text, width.saturating_sub(6)))
    };
    vec![
        SessionSwitcherRenderedLine::new(
            card_text(format!(
                "{} session · {current_marker}{}",
                session_status_label(session),
                session.title
            )),
            primary_style,
        ),
        SessionSwitcherRenderedLine::new(
            card_text(format!("Status {status} · Model {model}")),
            meta_style,
        ),
        SessionSwitcherRenderedLine::new(card_text(session.detail.clone()), meta_style),
        SessionSwitcherRenderedLine::new(card_text(preview), preview_style),
    ]
}

fn session_switcher_preview_lines_for_session(
    session: &workspace::SessionCard,
) -> Vec<SessionSwitcherRenderedLine> {
    let mut lines = vec![
        SessionSwitcherRenderedLine::new(
            session.title.clone(),
            SingleSessionLineStyle::OverlayTitle,
        ),
        SessionSwitcherRenderedLine::new(
            format!("id: {}", session.session_id),
            SingleSessionLineStyle::Meta,
        ),
    ];
    if !session.subtitle.is_empty() {
        lines.push(SessionSwitcherRenderedLine::new(
            session.subtitle.to_string(),
            SingleSessionLineStyle::Status,
        ));
    }
    if !session.detail.is_empty() {
        lines.push(SessionSwitcherRenderedLine::new(
            session.detail.clone(),
            SingleSessionLineStyle::Meta,
        ));
    }
    let transcript = if session.detail_lines.is_empty() {
        &session.preview_lines
    } else {
        &session.detail_lines
    };
    if transcript.is_empty() {
        lines.push(SessionSwitcherRenderedLine::new(
            "no transcript preview available".to_string(),
            SingleSessionLineStyle::Meta,
        ));
    } else {
        lines.push(SessionSwitcherRenderedLine::new(
            "recent transcript".to_string(),
            SingleSessionLineStyle::OverlayTitle,
        ));
        let mut user_turn = 1usize;
        for line in transcript {
            lines.push(session_switcher_transcript_preview_line(
                line,
                &mut user_turn,
            ));
        }
    }
    lines
}

fn session_switcher_preview_line_count_for_session(session: &workspace::SessionCard) -> usize {
    let mut count = 2;
    if !session.subtitle.is_empty() {
        count += 1;
    }
    if !session.detail.is_empty() {
        count += 1;
    }
    let transcript_len = if session.detail_lines.is_empty() {
        session.preview_lines.len()
    } else {
        session.detail_lines.len()
    };
    if transcript_len == 0 {
        count + 1
    } else {
        count + 1 + transcript_len
    }
}

fn session_status_badge(session: &workspace::SessionCard) -> String {
    let status = session_status_label(session);
    status.to_string()
}

fn session_status_label(session: &workspace::SessionCard) -> &str {
    session
        .subtitle
        .split('·')
        .next()
        .map(str::trim)
        .filter(|status| !status.is_empty())
        .unwrap_or("unknown")
}

fn session_model_label(session: &workspace::SessionCard) -> Option<String> {
    session
        .subtitle
        .split('·')
        .nth(1)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToOwned::to_owned)
}

fn session_switcher_compact_transcript_line(line: &str, width: usize) -> String {
    let (role, content) = session_switcher_split_preview_role(line);
    let compact = match role {
        Some("user") => format!("latest prompt: {content}"),
        Some("asst" | "assistant") => format!("latest answer: {content}"),
        Some("tool") => format!("tool: {content}"),
        Some("sys" | "system") => format!("system: {content}"),
        Some("task" | "background_task") => format!("task: {content}"),
        Some("meta") => format!("meta: {content}"),
        _ => line.trim().to_string(),
    };
    truncate_chars(&compact, width)
}

fn session_switcher_transcript_preview_line(
    line: &str,
    user_turn: &mut usize,
) -> SessionSwitcherRenderedLine {
    let (role, content) = session_switcher_split_preview_role(line);
    match role {
        Some("user") => {
            let rendered = format!("Prompt {}  {}", *user_turn, content);
            *user_turn = (*user_turn).saturating_add(1);
            SessionSwitcherRenderedLine::new(rendered, SingleSessionLineStyle::User)
        }
        Some("asst" | "assistant") => SessionSwitcherRenderedLine::new(
            format!("Assistant  {content}"),
            SingleSessionLineStyle::Assistant,
        ),
        Some("tool") => SessionSwitcherRenderedLine::new(
            format!("Tool  {content}"),
            SingleSessionLineStyle::Tool,
        ),
        Some("sys" | "system") => SessionSwitcherRenderedLine::new(
            format!("System  {content}"),
            SingleSessionLineStyle::Meta,
        ),
        Some("task" | "background_task") => SessionSwitcherRenderedLine::new(
            format!("Task  {content}"),
            SingleSessionLineStyle::Meta,
        ),
        Some("meta") => SessionSwitcherRenderedLine::new(
            format!("Meta  {content}"),
            SingleSessionLineStyle::Meta,
        ),
        _ => SessionSwitcherRenderedLine::new(
            line.trim().to_string(),
            SingleSessionLineStyle::Overlay,
        ),
    }
}

fn session_switcher_split_preview_role(line: &str) -> (Option<&str>, &str) {
    let trimmed = line.trim();
    let Some((role, content)) = trimmed.split_once(char::is_whitespace) else {
        return (None, trimmed);
    };
    match role {
        "user" | "asst" | "assistant" | "tool" | "sys" | "system" | "task" | "background_task"
        | "meta" => (Some(role), content.trim()),
        _ => (None, trimmed),
    }
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

fn session_switcher_fuzzy_score(needle: &str, haystack: &str) -> Option<usize> {
    let needle = needle.trim();
    if needle.is_empty() {
        return Some(0);
    }

    haystack
        .split_whitespace()
        .filter_map(|token| session_switcher_token_fuzzy_score(needle, token))
        .min()
}

fn session_switcher_token_fuzzy_score(needle: &str, haystack: &str) -> Option<usize> {
    let mut score = 0usize;
    let mut position = 0usize;
    for ch in needle.chars() {
        let offset = haystack[position..].find(ch)?;
        score += offset;
        position += offset + ch.len_utf8();
    }

    if needle.len() > 1 && score > needle.len() * 6 {
        return None;
    }

    Some(score)
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

fn session_info_inline_line_count(app: &SingleSessionApp) -> usize {
    12 + usize::from(
        app.session
            .as_ref()
            .is_some_and(|session| !session.subtitle.trim().is_empty()),
    ) + usize::from(
        app.session
            .as_ref()
            .is_some_and(|session| !session.detail.trim().is_empty()),
    ) + usize::from(app.error.is_some())
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
                "Choose model  ·  current {}",
                model_picker_current_label(
                    picker.provider_name.as_deref(),
                    picker.current_model.as_deref(),
                )
            ),
            SingleSessionLineStyle::OverlayTitle,
        ),
        styled_line(
            format!("{filter}  ·  {count}"),
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
                "     {}{}",
                truncate_chars(&choice.model, 49),
                current_badge,
            ),
            row_style,
        ));
        lines.push(styled_line(
            format!(
                "       {} · {} · {}",
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
                "{} more models",
                visible.len() - window_start - window.len()
            ),
            SingleSessionLineStyle::Overlay,
        ));
    }
    let footer = if picker.preview {
        "↑↓ select  ·  PgUp/PgDn jump  ·  Enter use model  ·  Esc clear /model"
    } else {
        "↑↓ select  ·  type to filter  ·  Enter use model  ·  Esc close"
    };
    lines.push(styled_line(footer, SingleSessionLineStyle::Overlay));

    lines
}

fn model_picker_inline_line_count(picker: &ModelPickerState) -> usize {
    let visible_len = picker.filtered_indices().len();
    let mut count = 2;
    if picker.loading {
        count += 1;
    }
    if picker.error.is_some() {
        count += 1;
    }
    if visible_len == 0 && !picker.loading {
        return count + 2;
    }

    let (window_start, window_len) =
        row_window_bounds(visible_len, picker.selected, MODEL_PICKER_INLINE_ROW_LIMIT);
    count += window_len * 2;
    if visible_len > window_start + window_len {
        count += 1;
    }
    count + 1
}

fn row_window_bounds(visible_len: usize, selected: usize, limit: usize) -> (usize, usize) {
    if visible_len == 0 || limit == 0 {
        return (0, 0);
    }
    let max_start = visible_len.saturating_sub(limit);
    let selected = selected.min(visible_len - 1);
    let start = selected.saturating_sub(limit / 2).min(max_start);
    let end = (start + limit).min(visible_len);
    (start, end - start)
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

fn session_switcher_preview_filter(input: &str) -> Option<String> {
    let trimmed = input.trim_start();
    let rest = trimmed
        .strip_prefix("/resume")
        .or_else(|| trimmed.strip_prefix("/sessions"))
        .or_else(|| trimmed.strip_prefix("/session"))?;
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

fn inferred_desktop_reasoning_efforts(
    provider_name: Option<&str>,
    model_name: Option<&str>,
    current_effort: Option<&str>,
) -> &'static [&'static str] {
    let provider = provider_name.unwrap_or_default().to_ascii_lowercase();
    let model = model_name.unwrap_or_default().to_ascii_lowercase();
    let current = current_effort.unwrap_or_default().to_ascii_lowercase();

    if provider.contains("openrouter") {
        if model.contains("deepseek") || current == "max" {
            return DESKTOP_REASONING_EFFORTS_DEEPSEEK;
        }
        return DESKTOP_REASONING_EFFORTS_OPENAI;
    }

    if provider.contains("deepseek") || model.contains("deepseek") || current == "max" {
        return DESKTOP_REASONING_EFFORTS_DEEPSEEK;
    }

    let is_anthropic = provider.contains("anthropic")
        || provider.contains("claude")
        || model.starts_with("claude-")
        || model.contains("/claude-");
    if is_anthropic {
        if model.contains("claude-opus-4-7") || current == "xhigh" {
            return DESKTOP_REASONING_EFFORTS_OPENAI;
        }
        return DESKTOP_REASONING_EFFORTS_ANTHROPIC_STANDARD;
    }

    // Before the model catalog arrives, the desktop may only know the current
    // runtime setting. Keep the shortcut responsive by falling back to the
    // common OpenAI/Anthropic order instead of doing a blocking history lookup.
    DESKTOP_REASONING_EFFORTS_OPENAI
}

pub(crate) fn desktop_model_choice_switch_spec(choice: &DesktopModelChoice) -> String {
    let model = choice.model.as_str();
    let provider = choice.provider.as_deref().unwrap_or_default();
    let api_method = choice.api_method.as_deref().unwrap_or_default();

    if api_method == "copilot" {
        format!("copilot:{model}")
    } else if api_method == "claude-oauth"
        || (api_method == "oauth" && desktop_model_choice_is_anthropic(provider, model))
    {
        format!("claude-oauth:{model}")
    } else if (api_method == "api-key" || api_method == "claude-api")
        && desktop_model_choice_is_anthropic(provider, model)
    {
        format!("claude-api:{model}")
    } else if api_method == "cursor" {
        format!("cursor:{model}")
    } else if api_method == "bedrock" {
        format!("bedrock:{model}")
    } else if api_method == "openai-api-key" || api_method == "openai-api" {
        format!("openai-api:{model}")
    } else if api_method == "openai-oauth" {
        format!("openai-oauth:{model}")
    } else if provider == "Antigravity" {
        format!("antigravity:{model}")
    } else if let Some(profile_id) = desktop_openai_compatible_profile_id_for_route(api_method) {
        format!("{profile_id}:{model}")
    } else if api_method == "openrouter" && !provider.is_empty() && provider != "auto" {
        format!("{model}@{provider}")
    } else {
        model.to_string()
    }
}

fn desktop_model_choice_is_anthropic(provider: &str, model: &str) -> bool {
    let provider = provider.to_ascii_lowercase();
    provider.contains("anthropic")
        || provider.contains("claude")
        || model.starts_with("claude-")
        || model.contains("/claude-")
}

fn desktop_openai_compatible_profile_id_for_route(api_method: &str) -> Option<&str> {
    let (kind, profile_id) = api_method.split_once(':')?;
    if kind == "openai-compatible" {
        let profile_id = profile_id.trim();
        if !profile_id.is_empty() {
            return Some(profile_id);
        }
    }
    None
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

fn model_picker_fuzzy_score(needle: &str, haystack: &str) -> Option<usize> {
    let needle = needle.trim();
    if needle.is_empty() {
        return Some(0);
    }

    haystack
        .split_whitespace()
        .filter_map(|token| model_picker_token_fuzzy_score(needle, token))
        .min()
}

fn model_picker_token_fuzzy_score(needle: &str, haystack: &str) -> Option<usize> {
    let mut score = 0usize;
    let mut position = 0usize;
    for ch in needle.chars() {
        let offset = haystack[position..].find(ch)?;
        score += offset;
        position += offset + ch.len_utf8();
    }

    if needle.len() > 1 && score > needle.len() * 6 {
        return None;
    }

    Some(score)
}

fn desktop_slash_fuzzy_score(needle: &str, haystack: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }

    let needle = needle.strip_prefix('/').unwrap_or(needle);
    let haystack = haystack.strip_prefix('/').unwrap_or(haystack);
    if needle.is_empty() {
        return Some(0);
    }

    if let Some(first_char) = needle.chars().next()
        && !haystack.starts_with(&needle[..first_char.len_utf8()])
    {
        return None;
    }

    let mut score = 0usize;
    let mut position = 0usize;
    for ch in needle.chars() {
        let offset = haystack[position..].find(ch)?;
        score += offset;
        position += offset + ch.len_utf8();
    }

    if needle.len() > 1 && score > needle.len() * 3 {
        return None;
    }

    Some(score)
}

fn dedupe_model_choices(choices: Vec<DesktopModelChoice>) -> Vec<DesktopModelChoice> {
    let mut seen = HashSet::with_capacity(choices.len());
    let mut deduped: Vec<DesktopModelChoice> = Vec::with_capacity(choices.len());
    for choice in choices {
        let key = (
            choice.model.clone(),
            choice.provider.clone(),
            choice.api_method.clone(),
            choice.detail.clone(),
        );
        if !seen.insert(key) {
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
            ("Alt+←/→", "change thinking level"),
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
            ("Ctrl+←/→, Ctrl+B/F", "move by word"),
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

fn hotkey_help_inline_line_count() -> usize {
    single_session_help_styled_line_count() + 2
}

fn single_session_help_styled_line_count() -> usize {
    DESKTOP_SLASH_COMMANDS.len()
        + 1
        + SINGLE_SESSION_HELP_SECTIONS
            .iter()
            .map(|section| 2 + section.shortcuts.len())
            .sum::<usize>()
}

fn append_chat_message_lines(
    lines: &mut Vec<SingleSessionStyledLine>,
    message: &SingleSessionMessage,
    user_turn: &mut usize,
    is_active_tool: bool,
    active_tool_input: Option<&str>,
    tool_run: Option<&SingleSessionToolRun>,
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
            tool_run,
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
        format!("{turn}  {}", compact_single_session_visible_line(first)),
        SingleSessionLineStyle::User,
    ));
    for line in content_lines {
        lines.push(styled_line(
            format!("   {}", compact_single_session_visible_line(line)),
            SingleSessionLineStyle::UserContinuation,
        ));
    }
}

fn compact_single_session_visible_line(line: &str) -> String {
    const MAX_VISIBLE_BYTES: usize = 512;

    if line.len() <= MAX_VISIBLE_BYTES {
        return line.to_string();
    }

    let prefix_len = safe_utf8_prefix_len(line, MAX_VISIBLE_BYTES);
    let omitted = line.len().saturating_sub(prefix_len);
    format!("{}… <{} bytes omitted>", &line[..prefix_len], omitted)
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
            self.line_style_override = Some(SingleSessionLineStyle::AssistantMedia);
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
    tool_run: Option<&SingleSessionToolRun>,
) {
    if content.is_empty() {
        return;
    }
    let mut raw_lines = content.lines();
    let Some(header) = raw_lines.next() else {
        return;
    };
    let header_is_expanded = header.trim_start().starts_with('▾');
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
    let tool_state = tool_run
        .map(|run| run.state)
        .or_else(|| {
            header
                .state
                .as_deref()
                .map(SingleSessionToolVisualState::from_tool_state_text)
        })
        .unwrap_or(SingleSessionToolVisualState::Unknown);
    let expanded = active || header_is_expanded;
    let base_metadata = SingleSessionToolLineMetadata {
        call_id: tool_run
            .map(|run| run.call_id.clone())
            .unwrap_or_else(|| fallback_tool_line_call_id(&header)),
        name: tool_run
            .map(|run| run.name.clone())
            .unwrap_or_else(|| header.name.clone()),
        state: tool_state,
        kind: SingleSessionToolLineKind::Header,
        active: active && tool_state.is_active(),
        expanded,
        stdin_prompt: tool_run.and_then(|run| run.stdin_prompt.clone()),
    };
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
    if metadata_lines.is_empty()
        && let Some(input_preview) = tool_run.and_then(|run| run.input_preview.as_deref())
    {
        metadata_lines.push(input_preview.to_string());
    }
    if let Some(stdin_prompt) = &base_metadata.stdin_prompt {
        metadata_lines.push(format!("input needed: {stdin_prompt}"));
    }

    lines.push(
        styled_line(
            format_tool_header_line_with_metadata(&header, &metadata_lines),
            SingleSessionLineStyle::Tool,
        )
        .with_tool_metadata(base_metadata.clone()),
    );

    if active
        && widget_lines.is_empty()
        && matches!(header.state.as_deref(), Some("preparing") | Some("running"))
    {
        widget_lines.push("waiting for tool output…".to_string());
    }

    if expanded && !widget_lines.is_empty() {
        append_tool_content_widget(lines, &widget_lines, &base_metadata);
    }
}

fn fallback_tool_line_call_id(header: &ToolHeader) -> String {
    format!(
        "legacy-tool:{}:{}:{}",
        header.name,
        header.state.as_deref().unwrap_or("unknown"),
        header.summary.as_deref().unwrap_or_default()
    )
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

fn append_tool_content_widget(
    lines: &mut Vec<SingleSessionStyledLine>,
    content_lines: &[String],
    base_metadata: &SingleSessionToolLineMetadata,
) {
    const MAX_WIDGET_LINES: usize = 12;
    for line in content_lines.iter().take(MAX_WIDGET_LINES) {
        lines.push(tool_detail_styled_line(line, base_metadata));
    }
    if content_lines.len() > MAX_WIDGET_LINES {
        lines.push(tool_detail_styled_line(
            &format!("… {} more lines", content_lines.len() - MAX_WIDGET_LINES),
            base_metadata,
        ));
    }
}

fn tool_detail_styled_line(
    line: &str,
    base_metadata: &SingleSessionToolLineMetadata,
) -> SingleSessionStyledLine {
    let mut metadata = base_metadata.clone();
    metadata.kind = SingleSessionToolLineKind::Detail;
    styled_line(
        format!("    {}", compact_tool_widget_text(line, 118)),
        SingleSessionLineStyle::Tool,
    )
    .with_tool_metadata(metadata)
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
    const TOOL_GROUP_SUMMARY_VISIBLE_FRAGMENT_LIMIT: usize = 6;

    if tool_messages.is_empty() {
        return;
    }

    let mut names: Vec<String> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();
    let mut approx_tokens = 0usize;

    for message in tool_messages {
        // This is only a collapsed-card estimate. Counting Unicode scalar
        // values scans every byte of large tool outputs and made first content
        // frames visibly stall for transcripts with huge tool groups. Use the
        // byte length's O(1) metadata instead; it is a good enough token proxy
        // for a summary that is intentionally approximate.
        approx_tokens += message.content().len().div_ceil(4);
        let name = tool_summary_name(message.content());
        if let Some(index) = names.iter().position(|existing| existing == &name) {
            counts[index] += 1;
        } else {
            names.push(name);
            counts.push(1);
        }
    }

    let total_distinct_tools = names.len();
    let mut summary_hasher = DefaultHasher::new();
    names.hash(&mut summary_hasher);
    counts.hash(&mut summary_hasher);
    approx_tokens.hash(&mut summary_hasher);
    let summary_hash = summary_hasher.finish();

    let mut fragments = names
        .into_iter()
        .zip(counts)
        .take(TOOL_GROUP_SUMMARY_VISIBLE_FRAGMENT_LIMIT)
        .map(|(name, count)| format!("{count} {name}"))
        .collect::<Vec<_>>();
    if total_distinct_tools > TOOL_GROUP_SUMMARY_VISIBLE_FRAGMENT_LIMIT {
        fragments.push(format!(
            "{} more kinds",
            total_distinct_tools - TOOL_GROUP_SUMMARY_VISIBLE_FRAGMENT_LIMIT
        ));
    }
    let fragments = fragments.join(", ");
    let token_fragment = format_approx_tokens(approx_tokens);
    let line = format!("  ▸ tools: {fragments} · ~{token_fragment} tokens");
    lines.push(
        styled_line(line.clone(), SingleSessionLineStyle::Tool).with_tool_metadata(
            SingleSessionToolLineMetadata {
                call_id: format!("tool-group:{summary_hash:016x}"),
                name: "tools".to_string(),
                state: SingleSessionToolVisualState::Group,
                kind: SingleSessionToolLineKind::GroupSummary,
                active: false,
                expanded: false,
                stdin_prompt: None,
            },
        ),
    );
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

    if !looks_like_json_value(raw_input) {
        return vec![format!("input: {}", compact_tool_text(raw_input, 132))];
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

fn looks_like_json_value(text: &str) -> bool {
    matches!(
        text.as_bytes().first().copied(),
        Some(b'{' | b'[' | b'"' | b'-' | b'0'..=b'9' | b't' | b'f' | b'n')
    )
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
    let mut compacted = String::new();
    let mut chars = 0usize;
    let mut first_word = true;

    for word in text.split_whitespace() {
        if first_word {
            first_word = false;
        } else if chars == max_chars {
            compacted.push('…');
            return compacted;
        } else {
            compacted.push(' ');
            chars += 1;
        }

        for ch in word.chars() {
            if chars == max_chars {
                compacted.push('…');
                return compacted;
            }
            compacted.push(ch);
            chars += 1;
        }
    }

    compacted
}

fn normalized_tool_call_id(id: Option<String>) -> Option<String> {
    id.map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
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
    if completion.len() <= prefix.len() {
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
        a.0.cmp(&b.0)
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
mod tests {
    use super::*;

    fn rendered_tool_lines(content: &str, active: bool) -> Vec<SingleSessionStyledLine> {
        let mut lines = Vec::new();
        append_tool_lines(&mut lines, content, active, None, None);
        lines
    }

    fn rendered_tool_text(content: &str, active: bool) -> Vec<String> {
        rendered_tool_lines(content, active)
            .into_iter()
            .map(|line| line.text)
            .collect()
    }

    fn test_model_choice(model: &str) -> DesktopModelChoice {
        DesktopModelChoice {
            model: model.to_string(),
            provider: Some("test-provider".to_string()),
            api_method: Some("chat".to_string()),
            detail: Some("available".to_string()),
            available: true,
        }
    }

    fn test_session_card(session_id: &str, title: &str) -> workspace::SessionCard {
        workspace::SessionCard {
            session_id: session_id.to_string(),
            title: title.to_string(),
            subtitle: "active · test-model".to_string(),
            detail: "4 msgs · just now · test".to_string(),
            preview_lines: vec!["user latest compact prompt".to_string()],
            detail_lines: vec![
                "user first question".to_string(),
                "assistant first answer".to_string(),
                "tool bash completed".to_string(),
            ],
            transcript_messages: Vec::new(),
        }
    }

    fn assert_render_line_count_matches(app: &SingleSessionApp) {
        assert_eq!(
            app.render_inline_widget_line_count(),
            app.render_inline_widget_styled_lines().len(),
            "render line count should match styled-line rendering"
        );
    }

    #[test]
    fn latest_external_cli_suggestion_uses_newest_candidate_context() {
        let old = ExternalCliSessionCandidate {
            source: "Claude Code",
            modified: SystemTime::UNIX_EPOCH,
            working_dir: Some("/tmp/old-project".to_string()),
            context: Some("old task".to_string()),
        };
        let new = ExternalCliSessionCandidate {
            source: "Codex",
            modified: SystemTime::UNIX_EPOCH + Duration::from_secs(10),
            working_dir: Some("/home/user/jcode".to_string()),
            context: Some("implement startup continuation suggestions".to_string()),
        };

        let suggestion =
            latest_external_cli_continuation_suggestion_from_candidates(vec![old, new])
                .expect("newest external session should produce a suggestion");

        assert_eq!(
            suggestion,
            "continue the latest Codex session in jcode: implement startup continuation suggestions"
        );
    }

    #[test]
    fn latest_external_cli_suggestion_missing_roots_returns_none() {
        let home =
            std::env::temp_dir().join(format!("jcode-missing-external-cli-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);

        assert_eq!(
            latest_external_cli_continuation_suggestion_from_home(&home),
            None
        );
    }

    #[test]
    fn latest_external_cli_suggestion_ignores_malformed_jsonl() {
        let home = std::env::temp_dir().join(format!(
            "jcode-malformed-external-cli-{}-{:?}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let codex_dir = home.join(".codex/sessions");
        std::fs::create_dir_all(&codex_dir).expect("create fake codex dir");
        std::fs::write(
            codex_dir.join("broken.jsonl"),
            "not json\n{\"type\":\"message\",\"role\":\"assistant\",\"content\":[]\n",
        )
        .expect("write malformed jsonl");

        assert_eq!(
            latest_external_cli_continuation_suggestion_from_home(&home),
            None
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn inline_widget_line_count_matches_rendered_lines_for_active_widgets() {
        let mut help = SingleSessionApp::new(None);
        help.show_help = true;
        assert_render_line_count_matches(&help);

        let mut info = SingleSessionApp::new(None);
        info.show_session_info = true;
        info.error = Some("test error".to_string());
        info.session = Some(test_session_card("session_info", "Info Session"));
        assert_render_line_count_matches(&info);

        let mut slash = SingleSessionApp::new(None);
        slash.handle_key(KeyInput::Character("/re".to_string()));
        assert_render_line_count_matches(&slash);

        let mut model = SingleSessionApp::new(None);
        model.model_picker.open = true;
        model.model_picker.choices = vec![test_model_choice("alpha"), test_model_choice("beta")];
        model.model_picker.selected = 1;
        model.model_picker.refresh_visible_indices();
        assert_render_line_count_matches(&model);

        let mut switcher = SingleSessionApp::new(None);
        switcher.session_switcher.open = true;
        switcher.session_switcher.sessions = vec![
            test_session_card("session_alpha", "Alpha"),
            test_session_card("session_beta", "Beta"),
        ];
        switcher.session_switcher.selected = 1;
        switcher.session_switcher.refresh_visible_indices();
        assert_render_line_count_matches(&switcher);
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
                "    waiting for tool output…",
            ]
        );
    }

    #[test]
    fn active_tool_lines_carry_visual_metadata_for_native_cards() {
        let lines = rendered_tool_lines(
            "▾ bash running\n  input: {\"command\":\"cargo test -p jcode-desktop\"}",
            true,
        );

        let header = lines[0]
            .tool
            .as_ref()
            .expect("tool header should carry native card metadata");
        assert_eq!(header.name, "bash");
        assert_eq!(header.state, SingleSessionToolVisualState::Running);
        assert_eq!(header.kind, SingleSessionToolLineKind::Header);
        assert!(header.active);
        assert!(header.expanded);

        let detail = lines[1]
            .tool
            .as_ref()
            .expect("tool detail should share native card metadata");
        assert_eq!(detail.call_id, header.call_id);
        assert_eq!(detail.kind, SingleSessionToolLineKind::Detail);
        assert!(detail.active);
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
            "    error[E0425]: cannot find value `foo` in this scope"
        );
        assert_eq!(lines[2], "    test result: FAILED");
    }

    #[test]
    fn inactive_tool_result_compacts_to_metadata_only() {
        let lines = rendered_tool_text(
            "▸ bash done: tests passed\n  input: {\"command\":\"cargo test -p jcode-desktop\"}\n  test result: ok",
            false,
        );

        assert_eq!(
            lines,
            vec!["  ✓ bash · done · tests passed · $ cargo test -p jcode-desktop"]
        );
    }

    #[test]
    fn expanded_inactive_tool_result_shows_detail_widget() {
        let lines = rendered_tool_text(
            "▾ edit done: updated file\n  input: {\"file_path\":\"src/lib.rs\",\"old_string\":\"old\",\"new_string\":\"new\"}\n  Edited src/lib.rs: replaced 1 occurrence",
            false,
        );

        assert_eq!(lines[0], "  ✓ edit · done · updated file · src/lib.rs");
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Edited src/lib.rs: replaced 1 occurrence")),
            "expanded inactive edit tool should render its detail widget: {lines:?}"
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
    fn plain_tool_input_skips_json_probe_and_renders_compactly() {
        let lines = formatted_tool_input_lines("bash", " chunk-0 chunk-1 chunk-2");
        assert_eq!(lines, vec!["input: chunk-0 chunk-1 chunk-2"]);
        assert!(!looks_like_json_value("chunk-0"));
        assert!(looks_like_json_value("{\"command\":\"cargo test\"}"));
    }

    #[test]
    fn active_tool_cache_key_ignores_input_suffix_after_visible_preview_stabilizes() {
        let mut app = SingleSessionApp::new(None);
        app.apply_session_event(DesktopSessionEvent::ToolStarted {
            id: Some("tool-a".to_string()),
            name: "bash".to_string(),
        });
        app.apply_session_event(DesktopSessionEvent::ToolExecuting {
            id: Some("tool-a".to_string()),
            name: "bash".to_string(),
        });
        app.apply_session_event(DesktopSessionEvent::ToolInput {
            id: Some("tool-a".to_string()),
            delta: "a".repeat(160),
        });

        let body_before = app.body_lines();
        let body_key_before = app.rendered_body_cache_key((900, 700));
        let static_key_before = app.rendered_body_static_cache_key((900, 700));

        app.apply_session_event(DesktopSessionEvent::ToolInput {
            id: Some("tool-a".to_string()),
            delta: "b".repeat(40),
        });

        assert_eq!(app.body_lines(), body_before);
        assert_eq!(app.rendered_body_cache_key((900, 700)), body_key_before);
        assert_eq!(
            app.rendered_body_static_cache_key((900, 700)),
            static_key_before
        );
    }

    #[test]
    fn compact_tool_text_collapses_whitespace_and_stops_after_visible_prefix() {
        assert_eq!(
            compact_tool_text("  alpha\n\tbeta   gamma  ", 32),
            "alpha beta gamma"
        );
        assert_eq!(compact_tool_text("alpha beta gamma", 10), "alpha beta…");
        assert_eq!(compact_tool_text("你好 世界 again", 5), "你好 世界…");
        assert_eq!(compact_tool_text("alpha", 0), "…");
        assert_eq!(compact_tool_text("   ", 0), "");
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
