//! Interactive session picker with preview
//!
//! Shows a list of sessions on the left, with a preview of the selected session's
//! conversation on the right. Sessions are grouped by server for multi-server support.

use super::color_support::rgb;
use crate::session::{CrashedSessionsInfo, Session};
use crate::tui::{DisplayMessage, markdown};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use jcode_session_types::SessionStatus;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph},
};
use std::collections::HashSet;
use std::io::IsTerminal;
use std::time::Duration;

pub use jcode_tui_session_picker::{
    PickerItem, PreviewMessage, ResumeTarget, ServerGroup, SessionFilterMode, SessionInfo,
    SessionSource,
};

mod filter;
mod loading;
mod memory;
mod navigation;
mod render;

#[cfg(test)]
use loading::collect_recent_session_stems;
use loading::{build_messages_preview, build_search_index, crashed_sessions_from_all_sessions};
pub use loading::{
    invalidate_session_list_cache, load_cached_sessions_grouped, load_servers, load_sessions,
    load_sessions_grouped,
};

const SEARCH_CONTENT_BUDGET_BYTES: usize = 12_000;
const DEFAULT_SESSION_SCAN_LIMIT: usize = 100;
const MIN_SESSION_SCAN_LIMIT: usize = 50;
const MAX_SESSION_SCAN_LIMIT: usize = 10_000;

#[derive(Clone, Debug)]
pub enum PickerResult {
    Selected(Vec<ResumeTarget>),
    SelectedInCurrentTerminal(Vec<ResumeTarget>),
    SelectedInNewTerminal(Vec<ResumeTarget>),
    /// The user explicitly confirmed handing a live Claude Code process over
    /// to Jcode. This is never emitted by ordinary Enter/resume behavior.
    TakeOverClaude(ResumeTarget),
    RestoreCrashedGroup(Vec<String>),
    /// The onboarding "Start a new session" row was chosen.
    StartNewSession,
    /// The onboarding read-only recent-project architecture review was chosen.
    ReviewRecentProject,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OnboardingAction {
    StartNewSession,
    ReviewRecentProject,
}

#[derive(Clone, Debug)]
pub enum OverlayAction {
    Continue,
    Close,
    Selected(PickerResult),
}

/// Safely truncate a string at a character boundary
fn safe_truncate(s: &str, max_chars: usize) -> &str {
    if s.chars().count() <= max_chars {
        return s;
    }

    s.char_indices()
        .nth(max_chars)
        .map(|(idx, _)| &s[..idx])
        .unwrap_or(s)
}

/// Normalize a working directory string for equality comparison: trim trailing
/// slashes (except root) and surrounding whitespace so `/foo/bar` and
/// `/foo/bar/` match.
fn normalize_dir(dir: &str) -> String {
    let trimmed = dir.trim();
    let stripped = trimmed.trim_end_matches('/');
    if stripped.is_empty() {
        trimmed.to_string()
    } else {
        stripped.to_string()
    }
}

/// Format duration since a time in a human-readable way
fn format_time_ago(time: chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let duration = now.signed_duration_since(time);

    let seconds = duration.num_seconds();
    if seconds < 60 {
        return format!("{}s ago", seconds);
    }

    let minutes = duration.num_minutes();
    if minutes < 60 {
        return format!("{}m ago", minutes);
    }

    let hours = duration.num_hours();
    if hours < 24 {
        return format!("{}h ago", hours);
    }

    let days = duration.num_days();
    if days < 7 {
        return format!("{}d ago", days);
    }

    if days < 30 {
        return format!("{}w ago", days / 7);
    }

    format!("{}mo ago", days / 30)
}

/// Compact duration for the "working Ns" badge in the Active view.
fn format_short_duration(duration: std::time::Duration) -> String {
    let secs = duration.as_secs();
    if secs < 60 {
        return format!("{}s", secs);
    }
    let minutes = secs / 60;
    if minutes < 60 {
        return format!("{}m", minutes);
    }
    format!("{}h{}m", minutes / 60, minutes % 60)
}

/// Which pane has keyboard focus
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaneFocus {
    /// Session list (left pane) - j/k navigate sessions
    Sessions,
    /// Preview (right pane) - j/k scroll preview
    Preview,
}

const PREVIEW_SCROLL_STEP: u16 = 3;
const PREVIEW_PAGE_SCROLL: u16 = PREVIEW_SCROLL_STEP * 3;
const SESSION_PAGE_STEP_COUNT: usize = 3;

/// How often the Active view re-snapshots live presence (which sessions are
/// still working vs ready) while it is on screen.
const LIVE_PRESENCE_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// Interactive session picker
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionRef {
    Flat(usize),
    Group {
        group_idx: usize,
        session_idx: usize,
    },
    Orphan(usize),
}

struct PendingSessionPreviewLoad {
    session_id: String,
    receiver: std::sync::mpsc::Receiver<Option<Vec<PreviewMessage>>>,
}

/// Fingerprint of every input that affects the *content* of the preview pane
/// (before scrolling). When this matches the cached value, scrolling can reuse
/// the already-wrapped lines instead of re-rendering markdown and re-wrapping
/// every frame, mirroring the main chat viewport's prepared-frame cache.
#[derive(Clone, PartialEq, Eq)]
struct PreviewCacheKey {
    /// Hash over the selected session id, its preview messages, and the
    /// header-affecting fields (status label, saved/selection/batch flags, …).
    content_hash: u64,
    /// Inner geometry of the preview pane; width drives wrapping and height
    /// drives the scrollbar decision (which can narrow the content one column).
    inner_width: u16,
    inner_height: u16,
    centered: bool,
    diff_mode: crate::config::DiffDisplayMode,
    /// Normalized (trimmed + lowercased) active search query. Included so the
    /// wrapped-line cache is rebuilt (and match highlighting reapplied) whenever
    /// the `/resume` search text changes.
    search_query: String,
}

/// Cached, fully-wrapped preview content. Built on a cache miss (selection
/// change, resize, preview load, config change) and reused on every subsequent
/// frame - notably while scrolling, which then only clamps the offset and
/// materializes the visible window.
struct PreviewRenderCache {
    key: PreviewCacheKey,
    /// Wrapped lines at the final (post-scrollbar-decision) width.
    wrapped_lines: Vec<Line<'static>>,
    /// For each pre-wrap source line, the index of its first wrapped line. Used
    /// to locate user prompts for the sticky "previous prompt" header.
    prewrap_to_wrapped: Vec<usize>,
    /// (pre-wrap line index, display number, text) for every user prompt.
    user_prompt_markers: Vec<(usize, usize, String)>,
    /// Whether the content overflows and a scrollbar column is reserved.
    show_scrollbar: bool,
    /// First wrapped-line index that contains a highlighted search match, if
    /// any. Used to auto-scroll the preview to the first hit when searching.
    first_match_line: Option<usize>,
}

pub struct SessionPicker {
    /// Flat list of items (headers and sessions)
    items: Vec<PickerItem>,
    /// References into the backing session collections for the filtered view.
    visible_sessions: Vec<SessionRef>,
    /// All sessions (unfiltered, for rebuilding)
    all_sessions: Vec<SessionInfo>,
    /// All server groups (unfiltered, for rebuilding)
    all_server_groups: Vec<ServerGroup>,
    /// All orphan sessions (unfiltered, for rebuilding)
    all_orphan_sessions: Vec<SessionInfo>,
    /// Map from items index to sessions index (only for Session items)
    item_to_session: Vec<Option<usize>>,
    list_state: ListState,
    scroll_offset: u16,
    /// Last rendered maximum preview scroll offset (total wrapped lines minus
    /// the visible height). Lets scroll handlers clamp without re-wrapping and
    /// lets the shared mouse-momentum know when it has reached the bottom.
    preview_max_scroll: u16,
    auto_scroll_preview: bool,
    /// Crashed sessions pending batch restore
    crashed_sessions: Option<CrashedSessionsInfo>,
    /// IDs of sessions that are eligible for current batch restore
    crashed_session_ids: HashSet<String>,
    last_list_area: Option<Rect>,
    last_preview_area: Option<Rect>,
    /// Whether to show debug/test/canary sessions
    show_test_sessions: bool,
    /// Current list filter mode
    filter_mode: SessionFilterMode,
    /// Search query for filtering sessions
    search_query: String,
    /// Whether we're in search input mode
    search_active: bool,
    /// Hidden test session count (debug + canary)
    hidden_test_count: usize,
    /// Which pane has keyboard focus
    focus: PaneFocus,
    /// Sessions explicitly selected for multi-resume / multi-catchup.
    selected_session_ids: HashSet<String>,
    last_mouse_scroll: Option<std::time::Instant>,
    /// Normalized query from the most recent search pass.
    cached_search_query: String,
    /// Session refs that matched the cached search query.
    cached_search_refs: Vec<SessionRef>,
    /// Lightweight placeholder shown while the picker list is loading.
    loading_message: Option<String>,
    pending_preview_load: Option<PendingSessionPreviewLoad>,
    preview_load_failures: HashSet<String>,
    /// Onboarding banner shown at the top of the picker (first-run "resume or
    /// start new" experience). When set, the picker reserves space at the top
    /// for the formatted onboarding prompt and shows selectable action rows
    /// above the session list.
    onboarding_banner: Option<Vec<Line<'static>>>,
    /// The highlighted onboarding action. `None` means focus is in the session
    /// list below the action rows.
    onboarding_action: Option<OnboardingAction>,
    /// Cached, fully-wrapped preview content so scrolling does not re-render and
    /// re-wrap the whole preview every frame. Invalidated by content hash and
    /// pane geometry (see [`PreviewCacheKey`]).
    preview_cache: Option<PreviewRenderCache>,
    /// Working directory `/resume` was opened from. Sessions whose `working_dir`
    /// matches this are highlighted so the user can quickly spot sessions from
    /// the same project they are currently in.
    current_dir: Option<String>,
    /// Live process presence keyed by session ID (active-pid registry snapshot).
    /// Drives the working/ready badges in the list and the Active filter
    /// membership. Refreshed on load/reseed and periodically while the Active
    /// view is on screen.
    live_presence: std::collections::HashMap<String, crate::session::SessionPresence>,
    /// When `live_presence` was last snapshotted (throttles periodic refresh).
    live_presence_refreshed_at: Option<std::time::Instant>,
    /// ID of the session the picker was opened from, labeled "current" in the
    /// list so the user can orient themselves in the Active view.
    current_session_id: Option<String>,
    /// Explicit Claude takeover confirmation. Merely detecting or selecting a
    /// live Claude session never stops it; only confirming this prompt emits
    /// `PickerResult::TakeOverClaude`.
    pending_claude_takeover: Option<ResumeTarget>,
}

impl SessionPicker {
    pub fn new(sessions: Vec<SessionInfo>) -> Self {
        let hidden_test_count = sessions.iter().filter(|s| s.is_debug).count();

        let crashed_sessions = crashed_sessions_from_all_sessions(&sessions);
        let crashed_session_ids: HashSet<String> = crashed_sessions
            .as_ref()
            .map(|info| info.session_ids.iter().cloned().collect())
            .unwrap_or_default();

        let mut picker = Self {
            items: Vec::new(),
            visible_sessions: Vec::new(),
            all_sessions: sessions,
            all_server_groups: Vec::new(),
            all_orphan_sessions: Vec::new(),
            item_to_session: Vec::new(),
            list_state: ListState::default(),
            scroll_offset: 0,
            preview_max_scroll: 0,
            auto_scroll_preview: true,
            crashed_sessions,
            crashed_session_ids,
            last_list_area: None,
            last_preview_area: None,
            show_test_sessions: false,
            filter_mode: SessionFilterMode::All,
            search_query: String::new(),
            search_active: false,
            hidden_test_count,
            focus: PaneFocus::Sessions,
            selected_session_ids: HashSet::new(),
            last_mouse_scroll: None,
            cached_search_query: String::new(),
            cached_search_refs: Vec::new(),
            loading_message: None,
            pending_preview_load: None,
            preview_load_failures: HashSet::new(),
            onboarding_banner: None,
            onboarding_action: None,
            preview_cache: None,
            current_dir: None,
            live_presence: std::collections::HashMap::new(),
            live_presence_refreshed_at: None,
            current_session_id: None,
            pending_claude_takeover: None,
        };
        picker.refresh_live_presence();
        picker.rebuild_items();
        picker
    }

    /// Create a lightweight picker that can render immediately while sessions
    /// are scanned in the background.
    pub fn loading() -> Self {
        Self {
            items: Vec::new(),
            visible_sessions: Vec::new(),
            all_sessions: Vec::new(),
            all_server_groups: Vec::new(),
            all_orphan_sessions: Vec::new(),
            item_to_session: Vec::new(),
            list_state: ListState::default(),
            scroll_offset: 0,
            preview_max_scroll: 0,
            auto_scroll_preview: true,
            crashed_sessions: None,
            crashed_session_ids: HashSet::new(),
            last_list_area: None,
            last_preview_area: None,
            show_test_sessions: false,
            filter_mode: SessionFilterMode::All,
            search_query: String::new(),
            search_active: false,
            hidden_test_count: 0,
            focus: PaneFocus::Sessions,
            selected_session_ids: HashSet::new(),
            last_mouse_scroll: None,
            cached_search_query: String::new(),
            cached_search_refs: Vec::new(),
            loading_message: Some("Loading sessions…".to_string()),
            pending_preview_load: None,
            preview_load_failures: HashSet::new(),
            onboarding_banner: None,
            onboarding_action: None,
            preview_cache: None,
            current_dir: None,
            live_presence: std::collections::HashMap::new(),
            live_presence_refreshed_at: None,
            current_session_id: None,
            pending_claude_takeover: None,
        }
    }

    pub fn debug_memory_profile(&self) -> serde_json::Value {
        memory::debug_memory_profile(self)
    }

    /// Create a picker with server grouping
    pub fn new_grouped(server_groups: Vec<ServerGroup>, orphan_sessions: Vec<SessionInfo>) -> Self {
        // Count totals before filtering
        let _total_session_count: usize = server_groups
            .iter()
            .map(|g| g.sessions.len())
            .sum::<usize>()
            + orphan_sessions.len();
        let hidden_test_count: usize = server_groups
            .iter()
            .flat_map(|g| g.sessions.iter())
            .chain(orphan_sessions.iter())
            .filter(|s| s.is_debug)
            .count();

        // Gather all sessions for crash detection
        let all_for_crash: Vec<SessionInfo> = server_groups
            .iter()
            .flat_map(|g| g.sessions.iter())
            .chain(orphan_sessions.iter())
            .cloned()
            .collect();
        let crashed_sessions = crashed_sessions_from_all_sessions(&all_for_crash);
        let crashed_session_ids: HashSet<String> = crashed_sessions
            .as_ref()
            .map(|info| info.session_ids.iter().cloned().collect())
            .unwrap_or_default();

        let (all_sessions, all_orphan_sessions) = if server_groups.is_empty() {
            (orphan_sessions, Vec::new())
        } else {
            (Vec::new(), orphan_sessions)
        };

        let mut picker = Self {
            items: Vec::new(),
            visible_sessions: Vec::new(),
            all_sessions,
            all_server_groups: server_groups,
            all_orphan_sessions,
            item_to_session: Vec::new(),
            list_state: ListState::default(),
            scroll_offset: 0,
            preview_max_scroll: 0,
            auto_scroll_preview: true,
            crashed_sessions,
            crashed_session_ids,
            last_list_area: None,
            last_preview_area: None,
            show_test_sessions: false,
            filter_mode: SessionFilterMode::All,
            search_query: String::new(),
            search_active: false,
            hidden_test_count,
            focus: PaneFocus::Sessions,
            selected_session_ids: HashSet::new(),
            last_mouse_scroll: None,
            cached_search_query: String::new(),
            cached_search_refs: Vec::new(),
            loading_message: None,
            pending_preview_load: None,
            preview_load_failures: HashSet::new(),
            onboarding_banner: None,
            onboarding_action: None,
            preview_cache: None,
            current_dir: None,
            live_presence: std::collections::HashMap::new(),
            live_presence_refreshed_at: None,
            current_session_id: None,
            pending_claude_takeover: None,
        };
        picker.refresh_live_presence();
        picker.rebuild_items();
        picker
    }

    pub fn activate_catchup_filter(&mut self) {
        self.filter_mode = SessionFilterMode::CatchUp;
        self.rebuild_items();
    }

    /// Record the working directory `/resume` was opened from so sessions that
    /// share it can be visually highlighted in the list.
    pub fn set_current_dir(&mut self, dir: Option<String>) {
        self.current_dir = dir.map(|d| normalize_dir(&d));
    }

    /// Whether the given session's working directory matches the directory the
    /// picker was opened from.
    pub(super) fn session_in_current_dir(&self, session: &SessionInfo) -> bool {
        match (self.current_dir.as_deref(), session.working_dir.as_deref()) {
            (Some(current), Some(dir)) => normalize_dir(dir) == current,
            _ => false,
        }
    }

    /// Record the session the picker was opened from so it can be labeled in
    /// the list (most useful in the Active view).
    pub fn set_current_session_id(&mut self, session_id: Option<String>) {
        self.current_session_id = session_id;
    }

    /// Whether this entry is the session the picker was opened from.
    pub(super) fn session_is_current(&self, session: &SessionInfo) -> bool {
        self.current_session_id.as_deref() == Some(session.id.as_str())
    }

    /// Switch to the Active (live sessions) view: snapshot presence and filter
    /// the list down to sessions with a running process.
    pub fn activate_active_filter(&mut self) {
        self.filter_mode = SessionFilterMode::Active;
        self.refresh_live_presence();
        self.rebuild_items();
    }

    /// Snapshot the active-pid registry + streaming markers into the picker.
    pub(super) fn refresh_live_presence(&mut self) {
        self.live_presence = crate::session::session_presence()
            .into_iter()
            .map(|presence| (presence.session_id.clone(), presence))
            .collect();
        if let Ok(sessions) = crate::claude_live::live_claude_sessions() {
            for session in sessions {
                let session_id = format!("claude:{}", session.session_id);
                self.live_presence.insert(
                    session_id.clone(),
                    crate::session::SessionPresence {
                        session_id,
                        pid: session.pid,
                        streaming: false,
                        streaming_since: None,
                        internal: false,
                    },
                );
            }
        }
        self.live_presence_refreshed_at = Some(std::time::Instant::now());
    }

    /// Periodically re-snapshot live presence while the picker is on screen so
    /// working/ready badges track reality. Rebuilds the list when the Active
    /// view is showing and membership or streaming state changed (a session
    /// finished its turn, went idle, or exited). Returns true when anything
    /// changed so callers can redraw.
    pub fn maybe_refresh_live_presence(&mut self) -> bool {
        if self.loading_message.is_some() {
            return false;
        }
        let due = self
            .live_presence_refreshed_at
            .is_none_or(|at| at.elapsed() >= LIVE_PRESENCE_REFRESH_INTERVAL);
        if !due {
            return false;
        }
        let before = std::mem::take(&mut self.live_presence);
        self.refresh_live_presence();
        let changed = before != self.live_presence;
        if changed && self.filter_mode == SessionFilterMode::Active {
            self.rebuild_items();
        }
        changed
    }

    /// Whether the session has a live process right now.
    pub(super) fn session_is_live(&self, session: &SessionInfo) -> bool {
        self.live_presence.contains_key(&session.id)
    }

    fn selected_live_claude_target(&self) -> Option<ResumeTarget> {
        let session = self.selected_session()?;
        if session.source != SessionSource::ClaudeCode || !self.session_is_live(session) {
            return None;
        }
        Some(session.resume_target.clone())
    }

    fn begin_claude_takeover_confirmation(&mut self) -> bool {
        let Some(target) = self.selected_live_claude_target() else {
            return false;
        };
        self.pending_claude_takeover = Some(target);
        true
    }

    fn handle_claude_takeover_confirmation_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Option<OverlayAction> {
        self.pending_claude_takeover.as_ref()?;
        if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
            self.pending_claude_takeover = None;
            return Some(OverlayAction::Close);
        }
        Some(match code {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                let target = self.pending_claude_takeover.take()?;
                OverlayAction::Selected(PickerResult::TakeOverClaude(target))
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('q') => {
                self.pending_claude_takeover = None;
                OverlayAction::Continue
            }
            _ => OverlayAction::Continue,
        })
    }

    #[cfg(test)]
    pub(crate) fn claude_takeover_confirmation_active_for_test(&self) -> bool {
        self.pending_claude_takeover.is_some()
    }

    /// Whether the session's live process is streaming a model response.
    pub(super) fn session_is_streaming(&self, session: &SessionInfo) -> bool {
        self.live_presence
            .get(&session.id)
            .is_some_and(|presence| presence.streaming)
    }

    /// Whether the current picker view contains a running session whose status
    /// glyph needs animated redraws.
    pub(crate) fn has_visible_running_sessions(&self) -> bool {
        self.visible_session_iter()
            .any(|session| self.session_is_streaming(session))
    }

    /// How long the session's current streaming turn has been running.
    pub(super) fn session_streaming_duration(
        &self,
        session: &SessionInfo,
    ) -> Option<std::time::Duration> {
        self.live_presence
            .get(&session.id)
            .and_then(|presence| presence.streaming_since)
            .and_then(|since| std::time::SystemTime::now().duration_since(since).ok())
    }

    /// Test-only: inject a synthetic live-presence snapshot so Active-view
    /// behavior can be exercised without real processes. Marks the snapshot as
    /// just-refreshed so the periodic refresh does not immediately overwrite it.
    #[cfg(test)]
    pub(crate) fn set_live_presence_for_test(
        &mut self,
        presences: Vec<crate::session::SessionPresence>,
    ) {
        self.live_presence = presences
            .into_iter()
            .map(|presence| (presence.session_id.clone(), presence))
            .collect();
        self.live_presence_refreshed_at = Some(std::time::Instant::now());
        self.rebuild_items();
    }

    /// Replace the backing session data in place while preserving the user's
    /// current view state: the highlighted session (by id), preview scroll, list
    /// scroll, search query/mode, filter, focused pane, test-session visibility,
    /// and multi-select set. Used by the background `/resume` refresh so the
    /// freshly loaded list does not yank the picker out from under the user
    /// (which previously reset their selection, scroll, and search every time the
    /// async load completed a second or two after opening).
    pub fn reseed_grouped(
        &mut self,
        server_groups: Vec<ServerGroup>,
        orphan_sessions: Vec<SessionInfo>,
    ) {
        // Remember what the user was looking at so we can restore it after the
        // data swap + item rebuild.
        let selected_id = self.selected_session().map(|session| session.id.clone());
        let preview_scroll = self.scroll_offset;
        let list_offset = self.list_state.offset();

        let hidden_test_count: usize = server_groups
            .iter()
            .flat_map(|g| g.sessions.iter())
            .chain(orphan_sessions.iter())
            .filter(|s| s.is_debug)
            .count();

        let all_for_crash: Vec<SessionInfo> = server_groups
            .iter()
            .flat_map(|g| g.sessions.iter())
            .chain(orphan_sessions.iter())
            .cloned()
            .collect();
        self.crashed_sessions = crashed_sessions_from_all_sessions(&all_for_crash);
        self.crashed_session_ids = self
            .crashed_sessions
            .as_ref()
            .map(|info| info.session_ids.iter().cloned().collect())
            .unwrap_or_default();

        let (all_sessions, all_orphan_sessions) = if server_groups.is_empty() {
            (orphan_sessions, Vec::new())
        } else {
            (Vec::new(), orphan_sessions)
        };
        self.all_sessions = all_sessions;
        self.all_server_groups = server_groups;
        self.all_orphan_sessions = all_orphan_sessions;
        self.hidden_test_count = hidden_test_count;
        self.loading_message = None;
        // Invalidate the cached search pass so the rebuild re-evaluates matches
        // against the new data instead of stale session refs.
        self.cached_search_query.clear();
        self.cached_search_refs.clear();
        // Clear the stale selection/items before rebuilding: the old
        // `visible_sessions` refs index into the just-replaced backing arrays, so
        // `rebuild_items`' own "preserve selection" lookup would resolve them
        // against mismatched data (causing index drift when the refreshed list is
        // reordered). We restore the selection explicitly, by id, afterwards.
        self.list_state.select(None);
        self.items.clear();
        self.visible_sessions.clear();
        self.item_to_session.clear();

        self.refresh_live_presence();
        self.rebuild_items();

        // Restore the highlighted session by id (not index) so it follows the
        // session across a reordered/extended refresh.
        let restored = selected_id
            .as_deref()
            .and_then(|id| self.find_item_index_for_session_id(id));
        if let Some(idx) = restored {
            self.list_state.select(Some(idx));
        } else {
            self.list_state
                .select(self.item_to_session.iter().position(|x| x.is_some()));
        }

        // Restore the user's scroll position only when their selection survived
        // the refresh so the view feels stable; otherwise fall back to the
        // rebuild's defaults.
        let still_selected = selected_id
            .as_deref()
            .and_then(|id| self.selected_session().map(|s| s.id == id))
            .unwrap_or(false);
        if still_selected {
            self.scroll_offset = preview_scroll;
            self.auto_scroll_preview = false;
            *self.list_state.offset_mut() = list_offset;
        }
    }

    /// Restrict the picker to a single external CLI source (onboarding flow:
    /// "continue where you left off" in Codex or Claude Code).
    pub fn activate_external_cli_filter(&mut self, mode: SessionFilterMode) {
        self.filter_mode = mode;
        self.rebuild_items();
    }

    /// Turn this picker into the first-run action-only experience. The suggested
    /// Git-based bug review starts highlighted above the blank-session action.
    pub fn activate_onboarding_banner(&mut self, banner_lines: Vec<Line<'static>>) {
        self.onboarding_banner = Some(banner_lines);
        self.onboarding_action = Some(OnboardingAction::ReviewRecentProject);
    }

    /// Whether the onboarding banner experience is active.
    pub fn onboarding_banner_active(&self) -> bool {
        self.onboarding_banner.is_some()
    }

    /// Whether the onboarding "Start a new session" row is currently highlighted.
    pub fn onboarding_start_new_highlighted(&self) -> bool {
        self.onboarding_banner.is_some()
            && self.onboarding_action == Some(OnboardingAction::StartNewSession)
    }

    /// Whether the onboarding recent-project review row is currently highlighted.
    pub fn onboarding_review_recent_project_highlighted(&self) -> bool {
        self.onboarding_banner.is_some()
            && self.onboarding_action == Some(OnboardingAction::ReviewRecentProject)
    }

    /// Number of sessions currently visible under the active filter.
    pub fn visible_session_count(&self) -> usize {
        self.visible_sessions
            .iter()
            .filter_map(|session_ref| self.session_by_ref(*session_ref))
            .count()
    }

    /// Resume target for the most recently active visible session, used by the
    /// onboarding flow to auto-select the latest transcript on timeout.
    pub fn latest_visible_resume_target(&self) -> Option<ResumeTarget> {
        self.visible_sessions
            .iter()
            .filter_map(|session_ref| self.session_by_ref(*session_ref))
            .max_by_key(|session| session.last_active_at.unwrap_or(session.last_message_time))
            .map(|session| session.resume_target.clone())
    }

    pub fn selected_session(&self) -> Option<&SessionInfo> {
        self.list_state.selected().and_then(|i| {
            self.item_to_session
                .get(i)
                .and_then(|opt| opt.as_ref())
                .and_then(|session_idx| self.visible_sessions.get(*session_idx))
                .copied()
                .and_then(|session_ref| self.session_by_ref(session_ref))
        })
    }

    pub fn session_for_target(&self, target: &ResumeTarget) -> Option<&SessionInfo> {
        self.visible_sessions
            .iter()
            .filter_map(|session_ref| self.session_by_ref(*session_ref))
            .find(|session| &session.resume_target == target)
    }

    fn selection_or_current_targets(&self) -> Vec<ResumeTarget> {
        if !self.selected_session_ids.is_empty() {
            return self
                .visible_sessions
                .iter()
                .filter_map(|session_ref| self.session_by_ref(*session_ref))
                .filter(|session| self.selected_session_ids.contains(&session.id))
                .map(|session| session.resume_target.clone())
                .collect();
        }

        self.selected_session()
            .map(|session| vec![session.resume_target.clone()])
            .unwrap_or_default()
    }

    fn selection_count(&self) -> usize {
        self.selected_session_ids.len()
    }

    fn toggle_selected_session(&mut self) {
        let Some(session_id) = self.selected_session().map(|session| session.id.clone()) else {
            return;
        };

        if !self.selected_session_ids.insert(session_id.clone()) {
            self.selected_session_ids.remove(&session_id);
        }
    }

    pub fn clear_selected_sessions(&mut self) {
        self.selected_session_ids.clear();
    }

    fn selected_session_ref(&self) -> Option<SessionRef> {
        self.list_state.selected().and_then(|i| {
            self.item_to_session
                .get(i)
                .and_then(|opt| opt.as_ref())
                .and_then(|idx| self.visible_sessions.get(*idx))
                .copied()
        })
    }

    fn session_by_ref(&self, session_ref: SessionRef) -> Option<&SessionInfo> {
        match session_ref {
            SessionRef::Flat(idx) => self.all_sessions.get(idx),
            SessionRef::Group {
                group_idx,
                session_idx,
            } => self
                .all_server_groups
                .get(group_idx)
                .and_then(|group| group.sessions.get(session_idx)),
            SessionRef::Orphan(idx) => self.all_orphan_sessions.get(idx),
        }
    }

    fn session_by_ref_mut(&mut self, session_ref: SessionRef) -> Option<&mut SessionInfo> {
        match session_ref {
            SessionRef::Flat(idx) => self.all_sessions.get_mut(idx),
            SessionRef::Group {
                group_idx,
                session_idx,
            } => self
                .all_server_groups
                .get_mut(group_idx)
                .and_then(|group| group.sessions.get_mut(session_idx)),
            SessionRef::Orphan(idx) => self.all_orphan_sessions.get_mut(idx),
        }
    }

    fn session_ref_for_id(&self, session_id: &str) -> Option<SessionRef> {
        if !self.all_server_groups.is_empty() {
            for (group_idx, group) in self.all_server_groups.iter().enumerate() {
                if let Some(session_idx) = group.sessions.iter().position(|s| s.id == session_id) {
                    return Some(SessionRef::Group {
                        group_idx,
                        session_idx,
                    });
                }
            }
            if let Some(idx) = self
                .all_orphan_sessions
                .iter()
                .position(|s| s.id == session_id)
            {
                return Some(SessionRef::Orphan(idx));
            }
        } else if let Some(idx) = self.all_sessions.iter().position(|s| s.id == session_id) {
            return Some(SessionRef::Flat(idx));
        }

        None
    }

    fn push_visible_session(&mut self, session_ref: SessionRef) {
        let session_idx = self.visible_sessions.len();
        self.visible_sessions.push(session_ref);
        self.items.push(PickerItem::Session);
        self.item_to_session.push(Some(session_idx));
    }

    /// Iterate the sessions currently visible in the list (post filter/search).
    pub(super) fn visible_session_iter(&self) -> impl Iterator<Item = &SessionInfo> + '_ {
        self.visible_sessions
            .iter()
            .filter_map(|session_ref| self.session_by_ref(*session_ref))
    }

    /// Test-only accessor: the source classification of every currently visible
    /// session. Used by onboarding tests to assert the combined external-CLI
    /// picker surfaces both Codex and Claude Code transcripts.
    #[cfg(test)]
    pub(crate) fn visible_session_iter_for_test(&self) -> impl Iterator<Item = &SessionInfo> + '_ {
        self.visible_session_iter()
    }

    fn load_preview_for_target(
        resume_target: ResumeTarget,
        external_path: Option<String>,
    ) -> Option<Vec<PreviewMessage>> {
        match resume_target {
            ResumeTarget::JcodeSession { session_id } => {
                let Ok(session) = Session::load(&session_id) else {
                    return None;
                };
                Some(build_messages_preview(&session))
            }
            ResumeTarget::ClaudeCodeSession { session_id, .. } => external_path
                .as_deref()
                .and_then(|path| {
                    loading::load_claude_code_preview_from_path(std::path::Path::new(path))
                })
                .or_else(|| loading::load_claude_code_preview(&session_id)),
            ResumeTarget::CodexSession { session_id, .. } => external_path
                .as_deref()
                .and_then(|path| loading::load_codex_preview_from_path(std::path::Path::new(path)))
                .or_else(|| loading::load_codex_preview(&session_id)),
            ResumeTarget::PiSession { session_path } => {
                loading::load_pi_preview_from_path(std::path::Path::new(&session_path))
            }
            ResumeTarget::OpenCodeSession { .. } => external_path.as_deref().and_then(|path| {
                loading::load_opencode_preview_from_path(std::path::Path::new(path))
            }),
            ResumeTarget::CursorSession { .. } => external_path.as_deref().and_then(|path| {
                loading::load_cursor_preview_from_path(std::path::Path::new(path))
            }),
        }
    }

    fn apply_session_preview(&mut self, session_id: &str, preview: Vec<PreviewMessage>) {
        let Some(session_ref) = self.session_ref_for_id(session_id) else {
            return;
        };
        if let Some(s) = self.session_by_ref_mut(session_ref) {
            s.search_index = build_search_index(
                &s.id,
                &s.short_name,
                &s.title,
                s.working_dir.as_deref(),
                s.save_label.as_deref(),
                &preview,
            );
            s.messages_preview = preview;
        }
    }

    fn poll_preview_load(&mut self) -> bool {
        let recv_result = {
            let Some(pending) = self.pending_preview_load.as_ref() else {
                return false;
            };
            pending.receiver.try_recv()
        };

        match recv_result {
            Ok(Some(preview)) => {
                let session_id = self
                    .pending_preview_load
                    .as_ref()
                    .map(|pending| pending.session_id.clone())
                    .unwrap_or_default();
                self.pending_preview_load = None;
                self.preview_load_failures.remove(&session_id);
                self.apply_session_preview(&session_id, preview);
                true
            }
            Ok(None) => {
                if let Some(pending) = self.pending_preview_load.take() {
                    self.preview_load_failures.insert(pending.session_id);
                }
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                if let Some(pending) = self.pending_preview_load.take() {
                    self.preview_load_failures.insert(pending.session_id);
                }
                true
            }
        }
    }

    fn ensure_selected_preview_loading(&mut self) {
        let Some(session_ref) = self.selected_session_ref() else {
            return;
        };
        let needs_preview = self
            .session_by_ref(session_ref)
            .map(|s| s.messages_preview.is_empty())
            .unwrap_or(false);
        if !needs_preview {
            return;
        }

        let Some((cache_session_id, resume_target, external_path)) =
            self.session_by_ref(session_ref).map(|s| {
                (
                    s.id.clone(),
                    s.resume_target.clone(),
                    s.external_path.clone(),
                )
            })
        else {
            return;
        };

        if self.preview_load_failures.contains(&cache_session_id)
            || self
                .pending_preview_load
                .as_ref()
                .is_some_and(|pending| pending.session_id == cache_session_id)
        {
            return;
        }

        let (tx, rx) = std::sync::mpsc::channel();
        let thread_session_id = cache_session_id.clone();
        let _ = std::thread::Builder::new()
            .name("jcode-session-preview-loader".to_string())
            .spawn(move || {
                let preview = Self::load_preview_for_target(resume_target, external_path);
                let _ = tx.send(preview);
            });
        self.pending_preview_load = Some(PendingSessionPreviewLoad {
            session_id: thread_session_id,
            receiver: rx,
        });
    }

    #[cfg(test)]
    fn ensure_selected_preview_loaded(&mut self) {
        let Some(session_ref) = self.selected_session_ref() else {
            return;
        };
        let Some((session_id, resume_target, external_path)) =
            self.session_by_ref(session_ref).map(|s| {
                (
                    s.id.clone(),
                    s.resume_target.clone(),
                    s.external_path.clone(),
                )
            })
        else {
            return;
        };
        if let Some(preview) = Self::load_preview_for_target(resume_target, external_path) {
            self.apply_session_preview(&session_id, preview);
        }
    }

    /// Delete the word immediately before the (implicit) end-of-line cursor in
    /// the search query. Used for Ctrl+W / Ctrl+Backspace inside the search bar.
    fn delete_search_word_back(&mut self) {
        let query = &self.search_query;
        let mut end = query.len();
        // Skip trailing whitespace.
        while end > 0 {
            let prev = super::core::prev_char_boundary(query, end);
            let ch = query[prev..].chars().next().unwrap_or(' ');
            if !ch.is_whitespace() {
                break;
            }
            end = prev;
        }
        // Skip the word characters.
        while end > 0 {
            let prev = super::core::prev_char_boundary(query, end);
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
    fn handle_search_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<OverlayAction> {
        let ctrl = modifiers.contains(KeyModifiers::CONTROL);
        match code {
            KeyCode::Esc => {
                self.search_active = false;
                self.search_query.clear();
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
            _ => {}
        }
        Ok(OverlayAction::Continue)
    }

    /// Handle a key event when used as an overlay inside the main TUI.
    /// Returns:
    /// - `Some(PickerResult::Selected(targets))` if user selected one or more sessions
    /// - `Some(PickerResult::RestoreCrashedGroup)` if user chose crash-group restore
    /// - `None` if the overlay should close (Esc/q/Ctrl+C)
    /// - The method returns `Ok(true)` to keep the overlay open (still navigating)
    pub fn handle_overlay_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<OverlayAction> {
        if let Some(action) = self.handle_claude_takeover_confirmation_key(code, modifiers) {
            return Ok(action);
        }
        if self.loading_message.is_some() {
            return match code {
                KeyCode::Esc | KeyCode::Char('q') => Ok(OverlayAction::Close),
                KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                    Ok(OverlayAction::Close)
                }
                _ => Ok(OverlayAction::Continue),
            };
        }

        if self.search_active {
            return self.handle_search_key(code, modifiers);
        }

        match code {
            KeyCode::Esc => {
                if !self.search_query.is_empty() {
                    self.search_query.clear();
                    self.rebuild_items();
                    return Ok(OverlayAction::Continue);
                }
                return Ok(OverlayAction::Close);
            }
            KeyCode::Char('q') => return Ok(OverlayAction::Close),
            KeyCode::Char(' ') => {
                self.toggle_selected_session();
            }
            KeyCode::Enter => {
                if self.onboarding_start_new_highlighted() {
                    return Ok(OverlayAction::Selected(PickerResult::StartNewSession));
                }
                if self.onboarding_review_recent_project_highlighted() {
                    return Ok(OverlayAction::Selected(PickerResult::ReviewRecentProject));
                }
                let targets = self.selection_or_current_targets();
                if !targets.is_empty() {
                    return Ok(OverlayAction::Selected(
                        self.selection_result_for_enter(targets, modifiers),
                    ));
                }
            }
            KeyCode::Char('R') | KeyCode::Char('B') | KeyCode::Char('b') => {
                if let Some(info) = &self.crashed_sessions {
                    return Ok(OverlayAction::Selected(PickerResult::RestoreCrashedGroup(
                        info.session_ids.clone(),
                    )));
                }
            }
            KeyCode::Char('/') => {
                self.search_active = true;
            }
            KeyCode::Char('d') => {
                self.toggle_test_sessions();
            }
            KeyCode::Char('T') => {
                self.begin_claude_takeover_confirmation();
            }
            KeyCode::Char('s') => {
                self.cycle_filter_mode();
            }
            KeyCode::Char('S') => {
                self.cycle_filter_mode_backwards();
            }
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(OverlayAction::Close);
            }
            _ => {}
        }
        if self.handle_focus_navigation_key(code, modifiers) {
            return Ok(OverlayAction::Continue);
        }
        Ok(OverlayAction::Continue)
    }

    fn selection_result_for_enter(
        &self,
        targets: Vec<ResumeTarget>,
        modifiers: KeyModifiers,
    ) -> PickerResult {
        let configured = crate::config::config().keybindings.session_picker_enter;
        let action = if modifiers.contains(KeyModifiers::CONTROL) {
            configured.alternate()
        } else {
            configured
        };
        match action {
            crate::config::SessionPickerResumeAction::NewTerminal => {
                PickerResult::SelectedInNewTerminal(targets)
            }
            crate::config::SessionPickerResumeAction::CurrentTerminal => {
                PickerResult::SelectedInCurrentTerminal(targets)
            }
        }
    }

    fn render_preview(&mut self, frame: &mut Frame, area: Rect) {
        let empty_border_color = if self.focus == PaneFocus::Preview {
            rgb(130, 130, 160)
        } else {
            rgb(50, 50, 50)
        };

        if let Some(message) = self.loading_message.as_deref() {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Preview ")
                .border_style(Style::default().fg(empty_border_color));
            let body = vec![
                Line::from(vec![
                    Span::styled("⏳ ", Style::default().fg(rgb(255, 200, 100))),
                    Span::styled(
                        message.to_string(),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(""),
                Line::from(vec![Span::styled(
                    "The picker will update as soon as the session index is ready.",
                    Style::default().fg(Color::DarkGray),
                )]),
            ];
            let paragraph = Paragraph::new(body).block(block);
            frame.render_widget(paragraph, area);
            return;
        }

        let _ = self.poll_preview_load();
        self.ensure_selected_preview_loading();

        let Some(session) = self.selected_session().cloned() else {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Preview ")
                .border_style(Style::default().fg(empty_border_color));
            let paragraph = Paragraph::new("No session selected")
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(paragraph, area);
            return;
        };

        let centered = crate::config::config().display.centered;
        let diff_mode = crate::config::config().display.diff_mode;
        let align = if centered {
            Alignment::Center
        } else {
            Alignment::Left
        };

        // Draw the bordered block first so we know the inner rect (which drives
        // wrapping width and the scrollbar decision) before building content.
        let preview_border_color = if self.focus == PaneFocus::Preview {
            rgb(130, 130, 160)
        } else {
            rgb(70, 70, 70)
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" Preview ")
            .border_style(Style::default().fg(preview_border_color));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        // Build (or reuse) the fully-wrapped preview content. The expensive
        // markdown render + wrap only runs on a cache miss; scrolling, focus
        // changes, and idle redraws reuse the cached wrapped lines. This mirrors
        // the main chat viewport, whose prepared frame is cached the same way.
        let key = PreviewCacheKey {
            content_hash: self.preview_content_hash(&session, centered, diff_mode),
            inner_width: inner.width,
            inner_height: inner.height,
            centered,
            diff_mode,
            search_query: self.search_query.trim().to_lowercase(),
        };
        let cache_valid = self
            .preview_cache
            .as_ref()
            .is_some_and(|cache| cache.key == key);
        if !cache_valid {
            let rebuilt = self.build_preview_cache(&session, area, inner, key, align, diff_mode);
            self.preview_cache = Some(rebuilt);
        }
        // Read cache geometry through a short-lived borrow so the scroll-offset
        // clamp below can take `&mut self` without conflict.
        let (show_scrollbar, total_lines, first_match_line) = {
            let cache = self
                .preview_cache
                .as_ref()
                .expect("preview cache populated above");
            (
                cache.show_scrollbar,
                cache.wrapped_lines.len(),
                cache.first_match_line,
            )
        };

        let visible_height = inner.height as usize;
        let (content_area, scrollbar_area) =
            super::ui::split_native_scrollbar_area(inner, show_scrollbar);

        let max_scroll = total_lines.saturating_sub(visible_height) as u16;
        self.preview_max_scroll = max_scroll;
        // The transcript for the selected session may still be loading on a
        // background thread, in which case this frame only shows a "Loading…"
        // placeholder (max_scroll == 0). Keep the auto-scroll armed until the
        // real content lands; otherwise the flag is consumed on the placeholder
        // frame and the populated transcript stays pinned at the top, hiding the
        // sticky "previous prompt" header that should appear once we snap to the
        // bottom.
        let preview_still_loading = session.messages_preview.is_empty()
            && self
                .pending_preview_load
                .as_ref()
                .is_some_and(|pending| pending.session_id == session.id);
        if self.auto_scroll_preview {
            // When a search is active and the selected session has a match in the
            // preview body, scroll the first hit into view (a few lines of lead-in
            // context) instead of jumping to the bottom of the transcript.
            self.scroll_offset = match first_match_line {
                Some(line) if !self.search_query.trim().is_empty() => {
                    (line.saturating_sub(2) as u16).min(max_scroll)
                }
                _ => max_scroll,
            };
            if !preview_still_loading {
                self.auto_scroll_preview = false;
            }
        } else {
            self.scroll_offset = self.scroll_offset.min(max_scroll);
        }
        let scroll = self.scroll_offset as usize;

        // Materialize only the visible window of wrapped lines instead of cloning
        // and `.scroll()`ing the whole preview every frame (the main chat
        // viewport uses the same visible-slice strategy). This makes a scroll
        // tick O(viewport height) rather than O(total wrapped lines).
        let visible_end = (scroll + visible_height).min(total_lines);
        let visible_lines: Vec<Line<'static>> = {
            let cache = self
                .preview_cache
                .as_ref()
                .expect("preview cache populated above");
            if scroll < visible_end {
                cache.wrapped_lines[scroll..visible_end].to_vec()
            } else {
                Vec::new()
            }
        };
        frame.render_widget(Paragraph::new(visible_lines), content_area);

        // Sticky "previous prompt" header: when the view is scrolled past a user
        // prompt, pin a dimmed `N› …` line at the top of the content area, just
        // like the main TUI's `prompt_preview`.
        if scroll > 0 {
            let user_color: Color = rgb(138, 180, 248);
            let user_text: Color = rgb(245, 245, 255);
            self.render_preview_prompt_header(
                frame,
                content_area,
                scroll,
                user_color,
                user_text,
                align,
            );
        }

        if let Some(scrollbar_area) = scrollbar_area {
            super::ui::render_native_scrollbar(
                frame,
                scrollbar_area,
                scroll,
                total_lines,
                visible_height,
                self.focus == PaneFocus::Preview,
            );
        }
    }

    /// Fingerprint everything that affects the *content* of the preview pane so
    /// the wrapped-line cache can be reused across frames (notably while
    /// scrolling). Mirrors the main chat viewport's prepared-frame cache key.
    fn preview_content_hash(
        &self,
        session: &SessionInfo,
        centered: bool,
        diff_mode: crate::config::DiffDisplayMode,
    ) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        session.id.hash(&mut h);
        session.short_name.hash(&mut h);
        session.icon.hash(&mut h);
        session.title.hash(&mut h);
        session.working_dir.hash(&mut h);
        session.save_label.hash(&mut h);
        session.saved.hash(&mut h);
        std::mem::discriminant(&session.status).hash(&mut h);
        match &session.status {
            SessionStatus::Crashed { message } => message.hash(&mut h),
            SessionStatus::Error { message } => message.hash(&mut h),
            _ => {}
        }
        // The status line shows a relative "… 5m ago" label derived from real
        // time, so bucket wall-clock into ~15s windows: fresh enough for the
        // header without rebuilding the cache during a scroll burst.
        session.last_message_time.timestamp().hash(&mut h);
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        (now_secs / 15).hash(&mut h);
        self.crashed_session_ids.contains(&session.id).hash(&mut h);
        self.selected_session_ids.contains(&session.id).hash(&mut h);
        let is_loading = session.messages_preview.is_empty()
            && self
                .pending_preview_load
                .as_ref()
                .is_some_and(|pending| pending.session_id == session.id);
        is_loading.hash(&mut h);
        centered.hash(&mut h);
        diff_mode.hash(&mut h);
        for msg in &session.messages_preview {
            msg.role.hash(&mut h);
            msg.content.hash(&mut h);
            msg.tool_calls.hash(&mut h);
            if let Some(tool) = &msg.tool_data {
                tool.id.hash(&mut h);
                tool.name.hash(&mut h);
                tool.input.to_string().hash(&mut h);
            }
        }
        h.finish()
    }

    /// Build the fully-wrapped preview content for the current selection. This
    /// is the expensive path (markdown render + wrap of every preview message);
    /// it only runs on a cache miss (selection change, resize, preview load,
    /// config change), after which scrolling reuses the wrapped lines.
    fn build_preview_cache(
        &self,
        session: &SessionInfo,
        area: Rect,
        inner: Rect,
        key: PreviewCacheKey,
        align: Alignment,
        diff_mode: crate::config::DiffDisplayMode,
    ) -> PreviewRenderCache {
        let user_color: Color = rgb(138, 180, 248);
        let user_text: Color = rgb(245, 245, 255);
        let dim_color: Color = rgb(80, 80, 80);
        let header_icon_color: Color = rgb(120, 210, 230);
        let header_session_color: Color = rgb(255, 255, 255);

        let preview_inner_width = area.width.saturating_sub(2);
        let assistant_width = preview_inner_width.saturating_sub(2);

        // Build preview content
        let mut lines: Vec<Line<'static>> = Vec::new();

        // Header matching TUI style
        lines.push(
            Line::from(vec![
                Span::styled(
                    format!("{} ", session.icon),
                    Style::default().fg(header_icon_color),
                ),
                Span::styled(
                    session.short_name.clone(),
                    Style::default()
                        .fg(header_session_color)
                        .add_modifier(Modifier::BOLD),
                ),
                {
                    let ago = format_time_ago(session.last_message_time);
                    let label = match &session.status {
                        SessionStatus::Active => "active".to_string(),
                        SessionStatus::Closed => format!("closed {}", ago),
                        SessionStatus::Crashed { .. } => format!("crashed {}", ago),
                        SessionStatus::Reloaded => format!("reloaded {}", ago),
                        SessionStatus::Compacted => format!("compacted {}", ago),
                        SessionStatus::RateLimited => format!("rate-limited {}", ago),
                        SessionStatus::Error { .. } => format!("errored {}", ago),
                    };
                    Span::styled(format!("  {}", label), Style::default().fg(dim_color))
                },
            ])
            .alignment(align),
        );

        // Title
        lines.push(
            Line::from(vec![Span::styled(
                session.title.clone(),
                Style::default().fg(Color::White),
            )])
            .alignment(align),
        );

        // Saved/bookmark indicator
        if session.saved {
            let saved_label = if let Some(ref label) = session.save_label {
                format!("📌 Saved as \"{}\"", label)
            } else {
                "📌 Saved".to_string()
            };
            lines.push(
                Line::from(vec![Span::styled(
                    saved_label,
                    Style::default().fg(rgb(255, 180, 100)),
                )])
                .alignment(align),
            );
        }

        // Working directory
        if let Some(ref dir) = session.working_dir {
            lines.push(
                Line::from(vec![Span::styled(
                    format!("📁 {}", dir),
                    Style::default().fg(dim_color),
                )])
                .alignment(align),
            );
        }

        // Status line with details
        let (status_icon, status_text, status_color) = match &session.status {
            SessionStatus::Active => ("▶", "Active".to_string(), rgb(100, 200, 100)),
            SessionStatus::Closed => ("✓", "Closed normally".to_string(), Color::DarkGray),
            SessionStatus::Crashed { message } => {
                let text = match message {
                    Some(msg) => format!("Crashed: {}", safe_truncate(msg, 80)),
                    None => "Crashed".to_string(),
                };
                ("💥", text, rgb(220, 100, 100))
            }
            SessionStatus::Reloaded => ("🔄", "Reloaded".to_string(), rgb(138, 180, 248)),
            SessionStatus::Compacted => (
                "📦",
                "Compacted (context too large)".to_string(),
                rgb(255, 193, 7),
            ),
            SessionStatus::RateLimited => ("⏳", "Rate limited".to_string(), rgb(186, 139, 255)),
            SessionStatus::Error { message } => {
                let text = format!("Error: {}", safe_truncate(message, 40));
                ("❌", text, rgb(220, 100, 100))
            }
        };
        lines.push(
            Line::from(vec![
                Span::styled(
                    format!("{} ", status_icon),
                    Style::default().fg(status_color),
                ),
                Span::styled(status_text, Style::default().fg(status_color)),
            ])
            .alignment(align),
        );

        if self.crashed_session_ids.contains(&session.id) {
            lines.push(
                Line::from(vec![Span::styled(
                    "Included in batch restore",
                    Style::default()
                        .fg(rgb(255, 140, 140))
                        .add_modifier(Modifier::BOLD),
                )])
                .alignment(align),
            );
        }

        if self.selected_session_ids.contains(&session.id) {
            lines.push(
                Line::from(vec![Span::styled(
                    "✓ Selected for multi-resume",
                    Style::default()
                        .fg(rgb(140, 220, 160))
                        .add_modifier(Modifier::BOLD),
                )])
                .alignment(align),
            );
        }

        lines.push(Line::from("").alignment(align));
        lines.push(
            Line::from(vec![Span::styled(
                "─".repeat(area.width.saturating_sub(4) as usize),
                Style::default().fg(rgb(60, 60, 60)),
            )])
            .alignment(align),
        );
        lines.push(Line::from("").alignment(align));

        // Messages preview - styled like the actual TUI
        let mut prompt_num = 0;
        let mut rendered_messages = 0usize;
        // Track the pre-wrap line index + display number + text of every user
        // prompt so we can render a sticky "previous prompt" header (matching the
        // main TUI's `prompt_preview`) once it scrolls out of view.
        let mut user_prompt_markers: Vec<(usize, usize, String)> = Vec::new();
        for msg in &session.messages_preview {
            if msg.content.trim().is_empty() {
                continue;
            }

            if !lines.is_empty() && msg.role != "tool" && msg.role != "meta" {
                lines.push(Line::from("").alignment(align));
            }

            let display_msg = DisplayMessage {
                role: msg.role.clone(),
                content: msg.content.clone(),
                tool_calls: msg.tool_calls.clone(),
                duration_secs: None,
                title: None,
                tool_data: msg.tool_data.clone(),
            };

            match msg.role.as_str() {
                "user" => {
                    prompt_num += 1;
                    user_prompt_markers.push((
                        lines.len(),
                        prompt_num,
                        display_msg.content.clone(),
                    ));
                    lines.push(
                        Line::from(vec![
                            Span::styled(
                                format!("{}", prompt_num),
                                Style::default().fg(user_color),
                            ),
                            Span::styled("› ", Style::default().fg(user_color)),
                            Span::styled(display_msg.content, Style::default().fg(user_text)),
                        ])
                        .alignment(align),
                    );
                    rendered_messages += 1;
                }
                "assistant" => {
                    let md_lines = super::ui::render_assistant_message(
                        &display_msg,
                        assistant_width,
                        crate::config::DiffDisplayMode::Off,
                    );
                    let mut skip_mermaid_blank = false;

                    for line in md_lines {
                        if super::mermaid::parse_image_placeholder(&line).is_some()
                            || super::mermaid::parse_inline_image_placeholder(&line).is_some()
                        {
                            lines.push(
                                Line::from(vec![Span::styled(
                                    "[mermaid diagram]",
                                    Style::default().fg(dim_color),
                                )])
                                .alignment(align),
                            );
                            skip_mermaid_blank = true;
                            rendered_messages += 1;
                            continue;
                        }

                        if skip_mermaid_blank
                            && line.spans.len() == 1
                            && line.spans[0].content.trim().is_empty()
                        {
                            continue;
                        }

                        skip_mermaid_blank = false;
                        lines.push(super::ui::align_if_unset(line, align));
                        rendered_messages += 1;
                    }
                }
                "tool" => {
                    let tool_lines = super::ui::render_tool_message(
                        &display_msg,
                        preview_inner_width,
                        diff_mode,
                    );
                    for line in tool_lines {
                        lines.push(super::ui::align_if_unset(line, align));
                        rendered_messages += 1;
                    }
                }
                "meta" => {
                    lines.push(
                        Line::from(vec![Span::styled(
                            msg.content.clone(),
                            Style::default().fg(dim_color),
                        )])
                        .alignment(align),
                    );
                    rendered_messages += 1;
                }
                "system" => {
                    let md_lines = super::ui::render_system_message(
                        &DisplayMessage {
                            role: msg.role.clone(),
                            content: msg.content.clone(),
                            tool_calls: msg.tool_calls.clone(),
                            duration_secs: None,
                            title: None,
                            tool_data: msg.tool_data.clone(),
                        },
                        assistant_width,
                        crate::config::DiffDisplayMode::Off,
                    );
                    for line in md_lines {
                        lines.push(super::ui::align_if_unset(line, align));
                        rendered_messages += 1;
                    }
                }
                "background_task" => {
                    let md_lines = super::ui::render_background_task_message(
                        &DisplayMessage {
                            role: msg.role.clone(),
                            content: msg.content.clone(),
                            tool_calls: msg.tool_calls.clone(),
                            duration_secs: None,
                            title: None,
                            tool_data: msg.tool_data.clone(),
                        },
                        assistant_width,
                        crate::config::DiffDisplayMode::Off,
                    );
                    for line in md_lines {
                        lines.push(super::ui::align_if_unset(line, align));
                        rendered_messages += 1;
                    }
                }
                "memory" => {
                    lines.push(
                        Line::from(vec![
                            Span::styled("🧠 ", Style::default()),
                            Span::styled(
                                msg.content.clone(),
                                Style::default().fg(rgb(140, 210, 255)),
                            ),
                        ])
                        .alignment(align),
                    );
                    rendered_messages += 1;
                }
                "usage" => {
                    lines.push(
                        Line::from(vec![Span::styled(
                            msg.content.clone(),
                            Style::default().fg(dim_color),
                        )])
                        .alignment(align),
                    );
                    rendered_messages += 1;
                }
                "error" => {
                    lines.push(
                        Line::from(vec![
                            Span::styled("✗ ", Style::default().fg(Color::Red)),
                            Span::styled(msg.content.clone(), Style::default().fg(Color::Red)),
                        ])
                        .alignment(align),
                    );
                    rendered_messages += 1;
                }
                _ => {
                    lines.push(
                        Line::from(vec![Span::styled(
                            msg.content.clone(),
                            Style::default().fg(Color::White),
                        )])
                        .alignment(align),
                    );
                    rendered_messages += 1;
                }
            }
        }

        if rendered_messages == 0 {
            let preview_loading = session.messages_preview.is_empty()
                && self
                    .pending_preview_load
                    .as_ref()
                    .is_some_and(|pending| pending.session_id == session.id);
            let text = if preview_loading {
                "Loading preview…"
            } else {
                "(empty session)"
            };
            lines.push(
                Line::from(vec![Span::styled(text, Style::default().fg(dim_color))])
                    .alignment(align),
            );
        }

        // Pre-wrap preview lines to keep rendering and scroll bounds aligned.
        // Two-pass so the content reserves the scrollbar column before wrapping
        // (matching the main chat viewport): wrap at the full inner width, and if
        // that overflows the viewport we re-wrap one column narrower to leave room
        // for the scrollbar. Narrowing only ever adds lines, so the decision is
        // stable. We also record, for each pre-wrap line, its first wrapped-line
        // index so we can locate user prompts for the sticky header.
        let visible_height = inner.height as usize;
        let source_lines = lines;
        let wrap_lines_tracked = |width: usize| -> (Vec<Line<'static>>, Vec<usize>) {
            let mut mapped = Vec::with_capacity(source_lines.len() + 1);
            let mut wrapped: Vec<Line> = Vec::new();
            for line in source_lines.iter().cloned() {
                mapped.push(wrapped.len());
                if width > 0 {
                    wrapped.extend(markdown::wrap_lines(vec![line], width));
                } else {
                    wrapped.push(line);
                }
            }
            mapped.push(wrapped.len());
            (wrapped, mapped)
        };

        let full_width = inner.width as usize;
        let (full_lines, full_map) = wrap_lines_tracked(full_width);
        let show_scrollbar =
            super::ui::native_scrollbar_visible(true, full_lines.len(), visible_height);

        let (mut wrapped_lines, prewrap_to_wrapped) = if show_scrollbar {
            let content_width = inner.width.saturating_sub(1) as usize;
            wrap_lines_tracked(content_width)
        } else {
            // Reuse the full-width wrap; the map already matches.
            (full_lines, full_map)
        };

        // Highlight active search matches in the wrapped preview body and record
        // the first wrapped line that contains a hit, so the caller can scroll it
        // into view. Highlighting after wrapping keeps wrapped-line indices exact.
        let highlight_tokens = self.active_highlight_tokens();
        let first_match_line =
            Self::highlight_lines_in_place(&mut wrapped_lines, &highlight_tokens);

        PreviewRenderCache {
            key,
            wrapped_lines,
            prewrap_to_wrapped,
            user_prompt_markers,
            show_scrollbar,
            first_match_line,
        }
    }

    /// Apply search-match highlighting to already-built preview lines in place.
    /// Returns the index of the first line that contains a highlighted match, if
    /// any. Each token highlights independently (matching the AND-token filter).
    fn highlight_lines_in_place(lines: &mut [Line<'static>], tokens: &[String]) -> Option<usize> {
        if tokens.is_empty() {
            return None;
        }
        let mut first_match: Option<usize> = None;
        for (idx, line) in lines.iter_mut().enumerate() {
            let mut new_spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len());
            let mut line_had_match = false;
            for span in line.spans.drain(..) {
                let lower = span.content.to_lowercase();
                if tokens.iter().any(|token| lower.contains(token)) {
                    line_had_match = true;
                    new_spans.extend(Self::highlight_spans(
                        span.content.as_ref(),
                        tokens,
                        span.style,
                    ));
                } else {
                    new_spans.push(span);
                }
            }
            line.spans = new_spans;
            if line_had_match && first_match.is_none() {
                first_match = Some(idx);
            }
        }
        first_match
    }

    /// Render the pinned "previous prompt" header for the preview pane. Mirrors
    /// the main chat viewport's `prompt_preview`: find the last user prompt whose
    /// wrapped start has scrolled above the viewport and draw it dimmed at the top.
    fn render_preview_prompt_header(
        &self,
        frame: &mut Frame,
        content_area: Rect,
        scroll: usize,
        user_color: Color,
        user_text: Color,
        align: Alignment,
    ) {
        // Read the prompt markers + wrap map from the cached preview content so
        // the header costs nothing extra during a scroll burst.
        let Some(cache) = self.preview_cache.as_ref() else {
            return;
        };
        let prewrap_to_wrapped = &cache.prewrap_to_wrapped;
        // The last prompt whose wrapped start index is above the current scroll.
        let Some((_, prompt_num, text)) =
            cache
                .user_prompt_markers
                .iter()
                .rev()
                .find(|(prewrap_idx, _, _)| {
                    prewrap_to_wrapped
                        .get(*prewrap_idx)
                        .is_some_and(|wrapped_start| *wrapped_start < scroll)
                })
        else {
            return;
        };

        let text_flat = text.replace('\n', " ");
        let text_flat = text_flat.trim();
        if text_flat.is_empty() {
            return;
        }

        let num_str = format!("{}", prompt_num);
        let prefix_len = num_str.len() + 2;
        let content_width = (content_area.width as usize).saturating_sub(prefix_len + 1);
        if content_width == 0 {
            return;
        }
        let dim_style = Style::default().dim();
        let dim_num = rgb(80, 80, 80);
        let user_bg = rgb(30, 34, 42);

        let text_chars: Vec<char> = text_flat.chars().collect();
        let is_long = text_chars.len() > content_width;
        let preview_lines: Vec<Line<'static>> = if !is_long {
            vec![
                Line::from(vec![
                    Span::styled(num_str.clone(), dim_style.fg(dim_num).bg(user_bg)),
                    Span::styled("› ", dim_style.fg(user_color).bg(user_bg)),
                    Span::styled(text_flat.to_string(), dim_style.fg(user_text).bg(user_bg)),
                ])
                .alignment(align),
            ]
        } else {
            let half = content_width.max(4);
            let head: String = text_chars[..half.min(text_chars.len())].iter().collect();
            let tail_start = text_chars.len().saturating_sub(half);
            let tail: String = text_chars[tail_start..].iter().collect();
            let first = Line::from(vec![
                Span::styled(num_str.clone(), dim_style.fg(dim_num).bg(user_bg)),
                Span::styled("› ", dim_style.fg(user_color).bg(user_bg)),
                Span::styled(
                    format!("{} ...", head.trim_end()),
                    dim_style.fg(user_text).bg(user_bg),
                ),
            ])
            .alignment(align);
            let padding: String = " ".repeat(prefix_len);
            let second = Line::from(vec![
                Span::styled(padding, dim_style.bg(user_bg)),
                Span::styled(
                    format!("... {}", tail.trim_start()),
                    dim_style.fg(user_text).bg(user_bg),
                ),
            ])
            .alignment(align);
            vec![first, second]
        };

        let line_count = (preview_lines.len() as u16).min(content_area.height);
        if line_count == 0 {
            return;
        }
        let header_area = Rect {
            x: content_area.x,
            y: content_area.y,
            width: content_area.width,
            height: line_count,
        };
        frame.render_widget(Clear, header_area);
        frame.render_widget(Paragraph::new(preview_lines), header_area);
    }

    /// Render the suggested first-run prompt as the primary centered action,
    /// with the blank-session escape hatch kept secondary in the bottom-right.
    fn render_onboarding_band(&self, frame: &mut Frame, area: Rect) {
        if area.height == 0 {
            return;
        }
        let accent = rgb(186, 139, 255);
        let inner = area.inner(Margin {
            horizontal: 2,
            vertical: 1,
        });
        if inner.height == 0 {
            return;
        }

        let content_width = inner.width.min(108);
        let content_x = inner.x + inner.width.saturating_sub(content_width) / 2;
        let prompt_lines = self.onboarding_banner.clone().unwrap_or_default();
        let prompt_height = prompt_lines
            .iter()
            .map(|line| {
                let width = line.width().max(1) as u16;
                width.div_ceil(content_width.max(1))
            })
            .sum::<u16>()
            .min(inner.height.saturating_sub(2));
        let review_y = inner.y + inner.height.saturating_sub(1) / 2;
        let prompt_y = review_y
            .saturating_sub(prompt_height.saturating_add(2))
            .max(inner.y);
        let prompt_area = Rect {
            x: content_x,
            y: prompt_y,
            width: content_width,
            height: prompt_height.min(review_y.saturating_sub(prompt_y)),
        };

        if prompt_area.height > 0 {
            let prompt = Paragraph::new(prompt_lines)
                .alignment(Alignment::Center)
                .wrap(ratatui::widgets::Wrap { trim: false });
            frame.render_widget(prompt, prompt_area);
        }

        let action_line = |label: &'static str, selected: bool| {
            let (cap_style, body_style) = if selected {
                (
                    Style::default().fg(accent),
                    Style::default()
                        .fg(rgb(20, 24, 32))
                        .bg(accent)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                (
                    Style::default().fg(rgb(58, 62, 70)),
                    Style::default().fg(rgb(170, 174, 182)).bg(rgb(58, 62, 70)),
                )
            };
            Line::from(vec![
                Span::styled("\u{25D6}", cap_style),
                Span::styled(format!(" {label} "), body_style),
                Span::styled("\u{25D7}", cap_style),
            ])
        };

        let review_selected = self.onboarding_review_recent_project_highlighted();
        frame.render_widget(
            Paragraph::new(action_line(
                "Find bugs in what I've been working on",
                review_selected,
            ))
            .alignment(Alignment::Center),
            Rect {
                x: inner.x,
                y: review_y,
                width: inner.width,
                height: 1,
            },
        );

        let start_selected = self.onboarding_start_new_highlighted();
        frame.render_widget(
            Paragraph::new(action_line("Start a new session", start_selected))
                .alignment(Alignment::Right),
            Rect {
                x: inner.x,
                y: inner.y + inner.height.saturating_sub(1),
                width: inner.width,
                height: 1,
            },
        );
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let has_banner = self.crashed_sessions.is_some();
        let has_search = self.search_active || !self.search_query.is_empty();
        let has_onboarding = self.onboarding_banner.is_some();
        // The first-run picker is action-only. Do not render the session list or
        // preview panes underneath it, which would make this look like `/resume`.
        if has_onboarding && self.visible_sessions.is_empty() {
            self.last_list_area = None;
            self.last_preview_area = None;
            self.render_onboarding_band(frame, frame.area());
            return;
        }

        // Build vertical constraints
        let mut v_constraints = Vec::new();
        if has_onboarding {
            // Reserve ~20% of the height for the onboarding prompt and actions,
            // clamped to a sensible band.
            let total = frame.area().height;
            let reserved = ((total as u32 * 20 / 100) as u16).clamp(7, total.saturating_sub(6));
            v_constraints.push(Constraint::Length(reserved.max(7)));
        }
        if has_banner {
            v_constraints.push(Constraint::Length(1));
        }
        if has_search {
            v_constraints.push(Constraint::Length(1));
        }
        v_constraints.push(Constraint::Min(8));

        let v_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(v_constraints)
            .split(frame.area());

        let mut chunk_idx = 0;

        // Render the onboarding band (prompt + action rows) if present.
        if has_onboarding {
            self.render_onboarding_band(frame, v_chunks[chunk_idx]);
            chunk_idx += 1;
        }

        // Render banner if present
        if has_banner {
            self.render_crash_banner(frame, v_chunks[chunk_idx]);
            chunk_idx += 1;
        }

        // Render search bar if active
        if has_search {
            let search_area = v_chunks[chunk_idx];
            chunk_idx += 1;

            let cursor_char = if self.search_active { "▎" } else { "" };
            let search_line = Line::from(vec![
                Span::styled(" 🔍 ", Style::default().fg(rgb(186, 139, 255))),
                Span::styled(
                    &self.search_query,
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(cursor_char, Style::default().fg(rgb(186, 139, 255))),
                if self.search_active {
                    Span::styled("  Esc to clear", Style::default().fg(rgb(60, 60, 60)))
                } else {
                    Span::styled("  / to edit", Style::default().fg(rgb(60, 60, 60)))
                },
            ]);
            let search_widget =
                Paragraph::new(search_line).style(Style::default().bg(rgb(25, 25, 30)));
            frame.render_widget(search_widget, search_area);
        }

        let main_area = v_chunks[chunk_idx];

        // Split main area horizontally for list and preview
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(main_area);

        self.last_list_area = Some(chunks[0]);
        self.last_preview_area = Some(chunks[1]);

        self.render_session_list(frame, chunks[0]);
        self.render_preview(frame, chunks[1]);
        self.render_claude_takeover_confirmation(frame);
    }

    fn render_claude_takeover_confirmation(&self, frame: &mut Frame) {
        let Some(target) = self.pending_claude_takeover.as_ref() else {
            return;
        };
        let ResumeTarget::ClaudeCodeSession { session_id, .. } = target else {
            return;
        };
        let frame_area = frame.area();
        if frame_area.width < 8 || frame_area.height < 5 {
            return;
        }
        let width = frame_area.width.saturating_sub(4).min(74);
        let height = frame_area.height.saturating_sub(2).min(8);
        let area = Rect {
            x: frame_area.x + frame_area.width.saturating_sub(width) / 2,
            y: frame_area.y + frame_area.height.saturating_sub(height) / 2,
            width,
            height,
        };
        frame.render_widget(Clear, area);
        let body = vec![
            Line::from(Span::styled(
                format!(
                    "Take over live Claude session {}?",
                    jcode_core::util::truncate_str(session_id, 12)
                ),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Jcode will prepare the transcript first, then ask Claude to exit.",
                Style::default().fg(rgb(190, 190, 200)),
            )),
            Line::from(Span::styled(
                "Enter/Y confirm · Esc/N cancel",
                Style::default()
                    .fg(rgb(255, 193, 7))
                    .add_modifier(Modifier::BOLD),
            )),
        ];
        let modal = Paragraph::new(body)
            .block(
                Block::default()
                    .title(" Explicit Claude takeover ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(rgb(255, 193, 7))),
            )
            .wrap(ratatui::widgets::Wrap { trim: false });
        frame.render_widget(modal, area);
    }

    /// Run the interactive picker, returns selected session ID or None if cancelled
    pub fn run(mut self) -> Result<Option<PickerResult>> {
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            anyhow::bail!(
                "session picker requires an interactive terminal (stdin/stdout must be a TTY)"
            );
        }
        // Detect light/dark terminal background before raw mode (OSC 11 query).
        super::theme_detect::init_theme_mode();
        let mut terminal = std::panic::catch_unwind(std::panic::AssertUnwindSafe(ratatui::init))
            .map_err(|payload| {
                let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                    (*s).to_string()
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic payload".to_string()
                };
                anyhow::anyhow!("failed to initialize session picker terminal: {}", msg)
            })?;
        // Initialize mermaid image picker (fast default, optional probe via env)
        super::mermaid::init_picker();
        let perf_policy = crate::perf::tui_policy();
        let keyboard_enhanced = if perf_policy.enable_keyboard_enhancement {
            super::enable_keyboard_enhancement()
        } else {
            false
        };
        let mouse_capture = perf_policy.enable_mouse_capture;
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;
        if mouse_capture {
            crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
        }

        let result = loop {
            // Keep live working/ready badges current while the picker idles.
            self.maybe_refresh_live_presence();
            terminal.draw(|frame| {
                self.render(frame);
                // Standalone picker loop bypasses `ui::draw`; adapt for light
                // terminal themes here (no-op on dark).
                jcode_tui_style::adapt_buffer_for_theme(frame.buffer_mut());
            })?;

            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(key) => {
                        if key.kind != KeyEventKind::Press {
                            continue;
                        }

                        match self.handle_overlay_key(key.code, key.modifiers)? {
                            OverlayAction::Continue => {}
                            OverlayAction::Close => break Ok(None),
                            OverlayAction::Selected(result) => break Ok(Some(result)),
                        }
                    }
                    Event::Mouse(mouse) => match mouse.kind {
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                            self.handle_mouse_scroll(mouse.column, mouse.row, mouse.kind);
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        };

        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
        if mouse_capture {
            let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
        }
        if keyboard_enhanced {
            super::disable_keyboard_enhancement();
        }
        ratatui::restore();
        super::mermaid::clear_image_state();

        result
    }
}

/// Run the interactive session picker
/// Returns the selected session ID, or None if the user cancelled
pub fn pick_session() -> Result<Option<PickerResult>> {
    // Check if we have a TTY
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        anyhow::bail!(
            "Session picker requires an interactive terminal. Use --resume <session_id> directly."
        );
    }

    // Load sessions grouped by server
    let (server_groups, orphan_sessions) = load_sessions_grouped()?;

    // Check if there are any sessions at all
    let total_sessions: usize = server_groups
        .iter()
        .map(|g| g.sessions.len())
        .sum::<usize>()
        + orphan_sessions.len();

    if total_sessions == 0 {
        eprintln!("No sessions found.");
        return Ok(None);
    }

    let picker = SessionPicker::new_grouped(server_groups, orphan_sessions);
    picker.run()
}

#[cfg(test)]
#[path = "session_picker_tests.rs"]
mod tests;
