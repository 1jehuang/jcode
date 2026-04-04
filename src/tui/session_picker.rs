//! Interactive session picker with preview
//!
//! Shows a list of sessions on the left, with a preview of the selected session's
//! conversation on the right. Sessions are grouped by server for multi-server support.

use super::color_support::rgb;
use crate::session::{CrashedSessionsInfo, Session, SessionStatus};
use crate::tui::{DisplayMessage, markdown};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph},
};
use std::collections::HashSet;
use std::io::IsTerminal;
use std::time::Duration;

mod filter;
mod loading;
mod navigation;
mod render;

use loading::{build_messages_preview, crashed_sessions_from_all_sessions};
#[cfg(test)]
use loading::{build_search_index, collect_recent_session_stems};
pub use loading::{load_servers, load_sessions, load_sessions_grouped};

/// Session info for display
#[derive(Clone)]
pub struct SessionInfo {
    pub id: String,
    pub parent_id: Option<String>,
    pub short_name: String,
    pub icon: String,
    pub title: String,
    pub message_count: usize,
    pub user_message_count: usize,
    pub assistant_message_count: usize,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_message_time: chrono::DateTime<chrono::Utc>,
    pub last_active_at: Option<chrono::DateTime<chrono::Utc>>,
    pub working_dir: Option<String>,
    pub model: Option<String>,
    pub provider_key: Option<String>,
    pub is_canary: bool,
    pub is_debug: bool,
    pub saved: bool,
    pub save_label: Option<String>,
    pub status: SessionStatus,
    pub needs_catchup: bool,
    pub estimated_tokens: usize,
    pub messages_preview: Vec<PreviewMessage>,
    /// Lowercased searchable text used by picker filtering
    pub search_index: String,
    /// Server name this session belongs to (if running)
    pub server_name: Option<String>,
    /// Server icon
    pub server_icon: Option<String>,
    /// Human/session source classification shown in the UI.
    pub source: SessionSource,
    /// How this entry should be resumed when selected.
    pub resume_target: ResumeTarget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionSource {
    Jcode,
    ClaudeCode,
    Codex,
    Pi,
    OpenCode,
}

impl SessionSource {
    pub fn badge(self) -> Option<&'static str> {
        match self {
            Self::Jcode => None,
            Self::ClaudeCode => Some("🧵 Claude Code"),
            Self::Codex => Some("🧠 Codex"),
            Self::Pi => Some("π Pi"),
            Self::OpenCode => Some("◌ OpenCode"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeTarget {
    JcodeSession { session_id: String },
    CodexSession { session_id: String },
    PiSession { session_path: String },
    OpenCodeSession { session_id: String },
}

impl ResumeTarget {
    pub fn stable_id(&self) -> &str {
        match self {
            Self::JcodeSession { session_id } => session_id,
            Self::CodexSession { session_id } => session_id,
            Self::PiSession { session_path } => session_path,
            Self::OpenCodeSession { session_id } => session_id,
        }
    }
}

/// A group of sessions under a server
#[derive(Clone)]
pub struct ServerGroup {
    pub name: String,
    pub icon: String,
    pub version: String,
    pub git_hash: String,
    pub is_running: bool,
    pub sessions: Vec<SessionInfo>,
}

#[derive(Clone)]
pub struct PreviewMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<String>,
    pub tool_data: Option<crate::message::ToolCall>,
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
}

const SEARCH_CONTENT_BUDGET_BYTES: usize = 12_000;
const DEFAULT_SESSION_SCAN_LIMIT: usize = 300;
const MIN_SESSION_SCAN_LIMIT: usize = 50;
const MAX_SESSION_SCAN_LIMIT: usize = 10_000;

#[derive(Clone, Debug)]
pub enum PickerResult {
    Selected(Vec<ResumeTarget>),
    RestoreAllCrashed,
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

/// Which pane has keyboard focus
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaneFocus {
    /// Session list (left pane) - j/k navigate sessions
    Sessions,
    /// Preview (right pane) - j/k scroll preview
    Preview,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionFilterMode {
    All,
    CatchUp,
    Saved,
    ClaudeCode,
    Codex,
    Pi,
    OpenCode,
}

impl SessionFilterMode {
    fn next(self) -> Self {
        match self {
            Self::All => Self::CatchUp,
            Self::CatchUp => Self::Saved,
            Self::Saved => Self::ClaudeCode,
            Self::ClaudeCode => Self::Codex,
            Self::Codex => Self::Pi,
            Self::Pi => Self::OpenCode,
            Self::OpenCode => Self::All,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::All => Self::OpenCode,
            Self::CatchUp => Self::All,
            Self::Saved => Self::CatchUp,
            Self::ClaudeCode => Self::Saved,
            Self::Codex => Self::ClaudeCode,
            Self::Pi => Self::Codex,
            Self::OpenCode => Self::Pi,
        }
    }

    fn label(self) -> Option<&'static str> {
        match self {
            Self::All => None,
            Self::CatchUp => Some("⏭ catch up"),
            Self::Saved => Some("📌 saved"),
            Self::ClaudeCode => Some("🧵 Claude Code"),
            Self::Codex => Some("🧠 Codex"),
            Self::Pi => Some("π Pi"),
            Self::OpenCode => Some("◌ OpenCode"),
        }
    }
}

const PREVIEW_SCROLL_STEP: u16 = 3;
const PREVIEW_PAGE_SCROLL: u16 = PREVIEW_SCROLL_STEP * 3;
const SESSION_PAGE_STEP_COUNT: usize = 3;

/// An item in the picker list - either a server header or a session
#[derive(Clone)]
pub enum PickerItem {
    ServerHeader {
        name: String,
        icon: String,
        version: String,
        session_count: usize,
    },
    Session,
    OrphanHeader {
        session_count: usize,
    },
    SavedHeader {
        session_count: usize,
    },
}

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
        };
        picker.rebuild_items();
        picker
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

        let mut picker = Self {
            items: Vec::new(),
            visible_sessions: Vec::new(),
            all_sessions: Vec::new(),
            all_server_groups: server_groups,
            all_orphan_sessions: orphan_sessions,
            item_to_session: Vec::new(),
            list_state: ListState::default(),
            scroll_offset: 0,
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
        };
        picker.rebuild_items();
        picker
    }

    pub fn activate_catchup_filter(&mut self) {
        self.filter_mode = SessionFilterMode::CatchUp;
        self.rebuild_items();
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

    fn push_visible_session(&mut self, session_ref: SessionRef) {
        let session_idx = self.visible_sessions.len();
        self.visible_sessions.push(session_ref);
        self.items.push(PickerItem::Session);
        self.item_to_session.push(Some(session_idx));
    }

    #[cfg(test)]
    fn visible_session_iter(&self) -> impl Iterator<Item = &SessionInfo> + '_ {
        self.visible_sessions
            .iter()
            .filter_map(|session_ref| self.session_by_ref(*session_ref))
    }

    fn ensure_selected_preview_loaded(&mut self) {
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

        let Some(session_id) =
            self.session_by_ref(session_ref)
                .and_then(|s| match &s.resume_target {
                    ResumeTarget::JcodeSession { session_id } => Some(session_id.clone()),
                    _ => None,
                })
        else {
            return;
        };
        let Ok(session) = Session::load(&session_id) else {
            return;
        };
        let preview = build_messages_preview(&session);

        if let Some(s) = self.session_by_ref_mut(session_ref) {
            s.messages_preview = preview;
        }
    }

    /// Handle a key event when used as an overlay inside the main TUI.
    /// Returns:
    /// - `Some(PickerResult::Selected(targets))` if user selected one or more sessions
    /// - `Some(PickerResult::RestoreAllCrashed)` if user chose batch restore
    /// - `None` if the overlay should close (Esc/q/Ctrl+C)
    /// - The method returns `Ok(true)` to keep the overlay open (still navigating)
    pub fn handle_overlay_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<OverlayAction> {
        if self.search_active {
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
                            return Ok(OverlayAction::Selected(PickerResult::Selected(targets)));
                        }
                    }
                }
                KeyCode::Backspace => {
                    self.search_query.pop();
                    self.rebuild_items();
                }
                KeyCode::Char(c) => {
                    if modifiers.contains(KeyModifiers::CONTROL) && c == 'c' {
                        return Ok(OverlayAction::Close);
                    }
                    self.search_query.push(c);
                    self.rebuild_items();
                }
                KeyCode::Down => self.next(),
                KeyCode::Up => self.previous(),
                _ => {}
            }
            return Ok(OverlayAction::Continue);
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
                let targets = self.selection_or_current_targets();
                if !targets.is_empty() {
                    return Ok(OverlayAction::Selected(PickerResult::Selected(targets)));
                }
            }
            KeyCode::Char('R') | KeyCode::Char('B') | KeyCode::Char('b') => {
                if self.crashed_sessions.is_some() {
                    return Ok(OverlayAction::Selected(PickerResult::RestoreAllCrashed));
                }
            }
            KeyCode::Char('/') => {
                self.search_active = true;
            }
            KeyCode::Char('d') => {
                self.toggle_test_sessions();
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

    fn render_preview(&mut self, frame: &mut Frame, area: Rect) {
        // Colors matching the actual TUI
        let user_color: Color = rgb(138, 180, 248); // Soft blue
        let user_text: Color = rgb(245, 245, 255); // Bright cool white
        let dim_color: Color = rgb(80, 80, 80); // Dim gray
        let header_icon_color: Color = rgb(120, 210, 230); // Teal
        let header_session_color: Color = rgb(255, 255, 255); // White

        let empty_border_color = if self.focus == PaneFocus::Preview {
            rgb(130, 130, 160)
        } else {
            rgb(50, 50, 50)
        };
        self.ensure_selected_preview_loaded();

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
        let preview_inner_width = area.width.saturating_sub(2);
        let assistant_width = preview_inner_width.saturating_sub(2);

        // Build preview content
        let mut lines: Vec<Line> = Vec::new();

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
                        if super::mermaid::parse_image_placeholder(&line).is_some() {
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
            lines.push(
                Line::from(vec![Span::styled(
                    "(empty session)",
                    Style::default().fg(dim_color),
                )])
                .alignment(align),
            );
        }

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

        // Pre-wrap preview lines to keep rendering and scroll bounds aligned.
        let preview_width = preview_inner_width as usize;
        let lines = if preview_width > 0 {
            markdown::wrap_lines(lines, preview_width)
        } else {
            lines
        };

        let visible_height = area.height.saturating_sub(2) as usize;
        let max_scroll = lines.len().saturating_sub(visible_height) as u16;
        if self.auto_scroll_preview {
            self.scroll_offset = max_scroll;
            self.auto_scroll_preview = false;
        } else {
            self.scroll_offset = self.scroll_offset.min(max_scroll);
        }

        let paragraph = Paragraph::new(lines)
            .block(block)
            .scroll((self.scroll_offset, 0));

        frame.render_widget(paragraph, area);
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let has_banner = self.crashed_sessions.is_some();
        let has_search = self.search_active || !self.search_query.is_empty();

        // Build vertical constraints
        let mut v_constraints = Vec::new();
        if has_banner {
            v_constraints.push(Constraint::Length(1));
        }
        if has_search {
            v_constraints.push(Constraint::Length(1));
        }
        v_constraints.push(Constraint::Min(10));

        let v_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(v_constraints)
            .split(frame.area());

        let mut chunk_idx = 0;

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
    }

    /// Run the interactive picker, returns selected session ID or None if cancelled
    pub fn run(mut self) -> Result<Option<PickerResult>> {
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            anyhow::bail!(
                "session picker requires an interactive terminal (stdin/stdout must be a TTY)"
            );
        }
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
        let keyboard_enhanced = super::enable_keyboard_enhancement();
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;

        let result = loop {
            terminal.draw(|frame| self.render(frame))?;

            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(key) => {
                        if key.kind != KeyEventKind::Press {
                            continue;
                        }

                        // Search mode: capture typed characters
                        if self.search_active {
                            match key.code {
                                KeyCode::Esc => {
                                    self.search_active = false;
                                    self.search_query.clear();
                                    self.rebuild_items();
                                }
                                KeyCode::Enter => {
                                    self.search_active = false;
                                    if self.visible_sessions.is_empty() {
                                        // No results - clear search and return to full list
                                        self.search_query.clear();
                                        self.rebuild_items();
                                    } else {
                                        let targets = self.selection_or_current_targets();
                                        if targets.is_empty() {
                                            break Ok(None);
                                        }
                                        break Ok(Some(PickerResult::Selected(targets)));
                                    }
                                }
                                KeyCode::Backspace => {
                                    self.search_query.pop();
                                    self.rebuild_items();
                                }
                                KeyCode::Char(c) => {
                                    if key.modifiers.contains(KeyModifiers::CONTROL) && c == 'c' {
                                        break Ok(None);
                                    }
                                    self.search_query.push(c);
                                    self.rebuild_items();
                                }
                                KeyCode::Down => self.next(),
                                KeyCode::Up => self.previous(),
                                _ => {}
                            }
                            continue;
                        }

                        // Normal mode
                        match key.code {
                            KeyCode::Esc => {
                                if !self.search_query.is_empty() {
                                    // Clear active search filter first
                                    self.search_query.clear();
                                    self.rebuild_items();
                                } else {
                                    break Ok(None);
                                }
                            }
                            KeyCode::Char('q') => {
                                break Ok(None);
                            }
                            KeyCode::Char(' ') => {
                                self.toggle_selected_session();
                            }
                            KeyCode::Enter => {
                                let targets = self.selection_or_current_targets();
                                if targets.is_empty() {
                                    break Ok(None);
                                }
                                break Ok(Some(PickerResult::Selected(targets)));
                            }
                            KeyCode::Char('R') | KeyCode::Char('B') | KeyCode::Char('b') => {
                                if self.crashed_sessions.is_some() {
                                    break Ok(Some(PickerResult::RestoreAllCrashed));
                                }
                            }
                            KeyCode::Char('/') => {
                                self.search_active = true;
                            }
                            KeyCode::Char('d') => {
                                self.toggle_test_sessions();
                            }
                            KeyCode::Char('s') => {
                                self.cycle_filter_mode();
                            }
                            KeyCode::Char('S') => {
                                self.cycle_filter_mode_backwards();
                            }
                            code if self.handle_focus_navigation_key(code, key.modifiers) => {}
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                break Ok(None);
                            }
                            _ => {}
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
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
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
mod tests {
    use super::*;
    use chrono::{Duration as ChronoDuration, Utc};

    fn make_session(
        id: &str,
        short_name: &str,
        is_debug: bool,
        status: SessionStatus,
    ) -> SessionInfo {
        make_session_with_flags(id, short_name, is_debug, false, status)
    }

    fn make_session_with_flags(
        id: &str,
        short_name: &str,
        is_debug: bool,
        is_canary: bool,
        status: SessionStatus,
    ) -> SessionInfo {
        let now = Utc::now();
        let title = "Test session".to_string();
        let working_dir = Some("/tmp".to_string());
        let messages_preview = vec![
            PreviewMessage {
                role: "user".to_string(),
                content: "hello".to_string(),
                tool_calls: Vec::new(),
                tool_data: None,
                timestamp: None,
            },
            PreviewMessage {
                role: "assistant".to_string(),
                content: "world".to_string(),
                tool_calls: Vec::new(),
                tool_data: None,
                timestamp: None,
            },
        ];
        let search_index = build_search_index(
            id,
            short_name,
            &title,
            working_dir.as_deref(),
            None,
            &messages_preview,
        );

        SessionInfo {
            id: id.to_string(),
            parent_id: None,
            short_name: short_name.to_string(),
            icon: "🧪".to_string(),
            title,
            message_count: 2,
            user_message_count: 1,
            assistant_message_count: 1,
            created_at: now - ChronoDuration::minutes(5),
            last_message_time: now - ChronoDuration::minutes(1),
            last_active_at: Some(now - ChronoDuration::minutes(1)),
            working_dir,
            model: None,
            provider_key: None,
            is_canary,
            is_debug,
            saved: false,
            save_label: None,
            status,
            needs_catchup: false,
            estimated_tokens: 200,
            messages_preview,
            search_index,
            server_name: None,
            server_icon: None,
            source: SessionSource::Jcode,
            resume_target: ResumeTarget::JcodeSession {
                session_id: id.to_string(),
            },
        }
    }

    #[test]
    fn test_status_inference() {
        // Load sessions and ensure status display works
        let sessions = load_sessions().unwrap();
        for session in &sessions {
            let _ = session.status.display();
        }
    }

    #[test]
    fn test_collect_recent_session_stems_skips_empty_recent_sessions() {
        let dir = tempfile::TempDir::new().expect("tempdir");

        std::fs::write(
            dir.path().join("session_alpha_1000.json"),
            r#"{"messages":[{"role":"user","content":"hi"}]}"#,
        )
        .expect("write alpha");
        std::fs::write(
            dir.path().join("session_beta_2000.json"),
            r#"{"messages":[]}"#,
        )
        .expect("write beta");
        std::fs::write(
            dir.path().join("session_gamma_3000.json"),
            r#"{"messages":[{"role":"user","content":"hello"}]}"#,
        )
        .expect("write gamma");
        std::fs::write(
            dir.path().join("session_delta_4000.json"),
            r#"{"messages":[]}"#,
        )
        .expect("write delta");

        let stems = collect_recent_session_stems(dir.path(), 2).expect("collect stems");
        assert_eq!(stems, vec!["session_gamma_3000", "session_alpha_1000"]);
    }

    #[test]
    fn test_collect_recent_session_stems_orders_by_timestamp_desc() {
        let dir = tempfile::TempDir::new().expect("tempdir");

        std::fs::write(
            dir.path().join("session_old_1111.json"),
            r#"{"messages":[{"role":"user","content":"old"}]}"#,
        )
        .expect("write old");
        std::fs::write(
            dir.path().join("session_mid_2222.json"),
            r#"{"messages":[{"role":"user","content":"mid"}]}"#,
        )
        .expect("write mid");
        std::fs::write(
            dir.path().join("session_new_3333.json"),
            r#"{"messages":[{"role":"user","content":"new"}]}"#,
        )
        .expect("write new");

        let stems = collect_recent_session_stems(dir.path(), 3).expect("collect stems");
        assert_eq!(
            stems,
            vec!["session_new_3333", "session_mid_2222", "session_old_1111"]
        );
    }

    #[test]
    fn test_toggle_test_sessions_rebuilds_visibility() {
        let normal = make_session("session_normal", "normal", false, SessionStatus::Closed);
        let debug = make_session("session_debug", "debug", true, SessionStatus::Closed);

        let mut picker = SessionPicker::new(vec![normal.clone(), debug.clone()]);

        assert_eq!(picker.visible_sessions.len(), 1);
        assert!(!picker.show_test_sessions);
        assert_eq!(picker.hidden_test_count, 1);

        picker.toggle_test_sessions();
        assert!(picker.show_test_sessions);
        assert_eq!(picker.visible_sessions.len(), 2);
        assert_eq!(picker.hidden_test_count, 0);

        picker.toggle_test_sessions();
        assert!(!picker.show_test_sessions);
        assert_eq!(picker.visible_sessions.len(), 1);
        assert_eq!(picker.hidden_test_count, 1);
    }

    #[test]
    fn test_new_grouped_hides_debug_by_default() {
        let normal = make_session("session_normal", "normal", false, SessionStatus::Closed);
        let debug = make_session("session_debug", "debug", true, SessionStatus::Closed);
        let canary = make_session_with_flags(
            "session_canary",
            "canary",
            false,
            true,
            SessionStatus::Closed,
        );
        let orphan_normal = make_session(
            "orphan_normal",
            "orphan-normal",
            false,
            SessionStatus::Closed,
        );
        let orphan_debug =
            make_session("orphan_debug", "orphan-debug", true, SessionStatus::Closed);

        let groups = vec![ServerGroup {
            name: "main".to_string(),
            icon: "🛰".to_string(),
            version: "v0.1.0".to_string(),
            git_hash: "abc1234".to_string(),
            is_running: true,
            sessions: vec![normal.clone(), debug.clone(), canary.clone()],
        }];

        let mut picker = SessionPicker::new_grouped(groups, vec![orphan_normal, orphan_debug]);

        assert!(!picker.show_test_sessions);
        // Canary sessions are now visible by default, only debug sessions are hidden
        assert_eq!(picker.visible_sessions.len(), 3); // normal + canary + orphan_normal
        assert!(picker.visible_session_iter().all(|s| !s.is_debug));
        assert_eq!(picker.hidden_test_count, 2); // debug + orphan_debug

        picker.toggle_test_sessions();
        assert!(picker.show_test_sessions);
        assert_eq!(picker.visible_sessions.len(), 5);
        assert_eq!(picker.hidden_test_count, 0);
        assert!(picker.visible_session_iter().any(|s| s.is_debug));
        assert!(picker.visible_session_iter().any(|s| s.is_canary));
    }

    #[test]
    fn test_crash_reason_line_for_crashed_sessions() {
        let crashed = make_session(
            "session_crash",
            "crash",
            false,
            SessionStatus::Crashed {
                message: Some("Terminal or window closed (SIGHUP)".to_string()),
            },
        );
        let line = SessionPicker::crash_reason_line(&crashed).expect("crash reason should render");
        let text: String = line
            .spans
            .into_iter()
            .map(|s| s.content.to_string())
            .collect();
        assert!(text.contains("reason:"));
        assert!(text.contains("SIGHUP"));
    }

    #[test]
    fn test_batch_restore_detection_excludes_already_recovered_parent_sessions() {
        let crashed = make_session(
            "session_crash_source",
            "crash-source",
            false,
            SessionStatus::Crashed {
                message: Some("boom".to_string()),
            },
        );

        let mut recovered = make_session(
            "session_recovery_rec123",
            "recovered",
            false,
            SessionStatus::Closed,
        );
        recovered.parent_id = Some(crashed.id.clone());

        let picker = SessionPicker::new(vec![crashed, recovered]);

        assert!(picker.crashed_sessions.is_none());
        assert!(picker.crashed_session_ids.is_empty());
    }

    #[test]
    fn test_grouped_batch_restore_uses_last_active_at_and_includes_debug_sessions() {
        let now = Utc::now();

        let mut recent_normal = make_session(
            "session_recent_normal",
            "recent-normal",
            false,
            SessionStatus::Crashed {
                message: Some("recent crash".to_string()),
            },
        );
        recent_normal.last_message_time = now - ChronoDuration::minutes(10);
        recent_normal.last_active_at = Some(now - ChronoDuration::seconds(10));

        let mut recent_debug = make_session(
            "session_recent_debug",
            "recent-debug",
            true,
            SessionStatus::Crashed {
                message: Some("debug crash".to_string()),
            },
        );
        recent_debug.last_message_time = now - ChronoDuration::minutes(9);
        recent_debug.last_active_at = Some(now - ChronoDuration::seconds(20));

        let mut stale_crash = make_session(
            "session_stale_crash",
            "stale-crash",
            false,
            SessionStatus::Crashed {
                message: Some("old crash".to_string()),
            },
        );
        stale_crash.last_message_time = now - ChronoDuration::seconds(30);
        stale_crash.last_active_at = Some(now - ChronoDuration::minutes(3));

        let picker = SessionPicker::new_grouped(
            vec![ServerGroup {
                name: "main".to_string(),
                icon: "🛰".to_string(),
                version: "v0.1.0".to_string(),
                git_hash: "abc1234".to_string(),
                is_running: true,
                sessions: vec![recent_normal.clone(), recent_debug.clone(), stale_crash],
            }],
            Vec::new(),
        );

        let crashed = picker
            .crashed_sessions
            .as_ref()
            .expect("expected eligible crashed sessions");

        assert_eq!(crashed.session_ids.len(), 2);
        assert!(crashed.session_ids.contains(&recent_normal.id));
        assert!(crashed.session_ids.contains(&recent_debug.id));
        assert!(
            !crashed
                .session_ids
                .iter()
                .any(|id| id == "session_stale_crash")
        );
    }

    #[test]
    fn test_filter_matches_recent_message_content() {
        let mut picker = SessionPicker::new(vec![make_session(
            "session_content",
            "content",
            false,
            SessionStatus::Closed,
        )]);

        picker.search_query = "world".to_string();
        picker.rebuild_items();
        assert_eq!(picker.visible_sessions.len(), 1);

        picker.search_query = "not-in-preview".to_string();
        picker.rebuild_items();
        assert!(picker.visible_sessions.is_empty());
    }

    #[test]
    fn test_filter_mode_cycles_through_requested_session_sources() {
        let mut saved = make_session("session_saved", "saved", false, SessionStatus::Closed);
        saved.saved = true;
        saved.needs_catchup = true;

        let claude_code = make_session(
            "imported_cc_demo",
            "claude-code",
            false,
            SessionStatus::Closed,
        );

        let mut codex = make_session("session_codex", "codex", false, SessionStatus::Closed);
        codex.model = Some("gpt-5.3-codex".to_string());
        codex.source = SessionSource::Codex;

        let mut pi = make_session("session_pi", "pi", false, SessionStatus::Closed);
        pi.provider_key = Some("pi".to_string());
        pi.source = SessionSource::Pi;

        let mut opencode =
            make_session("session_opencode", "opencode", false, SessionStatus::Closed);
        opencode.provider_key = Some("opencode".to_string());
        opencode.source = SessionSource::OpenCode;

        let mut picker = SessionPicker::new(vec![saved, claude_code, codex, pi, opencode]);

        assert_eq!(picker.filter_mode, SessionFilterMode::All);
        assert_eq!(picker.visible_sessions.len(), 5);

        picker.cycle_filter_mode();
        assert_eq!(picker.filter_mode, SessionFilterMode::CatchUp);
        assert_eq!(picker.visible_sessions.len(), 1);
        assert!(
            picker
                .visible_session_iter()
                .all(|session| session.needs_catchup)
        );

        picker.cycle_filter_mode();
        assert_eq!(picker.filter_mode, SessionFilterMode::Saved);
        assert_eq!(picker.visible_sessions.len(), 1);
        assert!(picker.visible_session_iter().all(|session| session.saved));
        assert_eq!(picker.items.len(), picker.visible_sessions.len());

        picker.cycle_filter_mode();
        assert_eq!(picker.filter_mode, SessionFilterMode::ClaudeCode);
        assert_eq!(picker.visible_sessions.len(), 1);
        assert!(
            picker
                .visible_session_iter()
                .all(SessionPicker::session_is_claude_code)
        );

        picker.cycle_filter_mode();
        assert_eq!(picker.filter_mode, SessionFilterMode::Codex);
        assert_eq!(picker.visible_sessions.len(), 1);
        assert!(
            picker
                .visible_session_iter()
                .all(SessionPicker::session_is_codex)
        );

        picker.cycle_filter_mode();
        assert_eq!(picker.filter_mode, SessionFilterMode::Pi);
        assert_eq!(picker.visible_sessions.len(), 1);
        assert!(
            picker
                .visible_session_iter()
                .all(SessionPicker::session_is_pi)
        );

        picker.cycle_filter_mode();
        assert_eq!(picker.filter_mode, SessionFilterMode::OpenCode);
        assert_eq!(picker.visible_sessions.len(), 1);
        assert!(
            picker
                .visible_session_iter()
                .all(SessionPicker::session_is_open_code)
        );

        picker.cycle_filter_mode();
        assert_eq!(picker.filter_mode, SessionFilterMode::All);
        assert_eq!(picker.visible_sessions.len(), 5);
    }

    #[test]
    fn test_filter_mode_keyboard_shortcuts_cycle_both_directions() {
        let mut picker = SessionPicker::new(vec![make_session(
            "session_saved",
            "saved",
            false,
            SessionStatus::Closed,
        )]);
        picker
            .handle_overlay_key(KeyCode::Char('s'), KeyModifiers::empty())
            .unwrap();
        assert_eq!(picker.filter_mode, SessionFilterMode::CatchUp);

        picker
            .handle_overlay_key(KeyCode::Char('S'), KeyModifiers::empty())
            .unwrap();
        assert_eq!(picker.filter_mode, SessionFilterMode::All);
    }

    #[test]
    fn test_space_selects_multiple_sessions_and_enter_returns_them() {
        let mut newer = make_session("session_newer", "newer", false, SessionStatus::Closed);
        let mut older = make_session("session_older", "older", false, SessionStatus::Closed);
        newer.last_message_time = Utc::now();
        older.last_message_time = Utc::now() - ChronoDuration::minutes(1);

        let mut picker = SessionPicker::new(vec![older, newer]);

        picker
            .handle_overlay_key(KeyCode::Char(' '), KeyModifiers::empty())
            .unwrap();
        picker
            .handle_overlay_key(KeyCode::Down, KeyModifiers::empty())
            .unwrap();
        picker
            .handle_overlay_key(KeyCode::Char(' '), KeyModifiers::empty())
            .unwrap();

        let action = picker
            .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
            .unwrap();

        match action {
            OverlayAction::Selected(PickerResult::Selected(ids)) => {
                assert_eq!(
                    ids,
                    vec![
                        ResumeTarget::JcodeSession {
                            session_id: "session_older".to_string(),
                        },
                        ResumeTarget::JcodeSession {
                            session_id: "session_newer".to_string(),
                        }
                    ]
                );
            }
            other => panic!("expected selected sessions, got {other:?}"),
        }
    }

    #[test]
    fn test_rebuild_items_prunes_selected_sessions_hidden_by_filter() {
        let mut saved = make_session("session_saved", "saved", false, SessionStatus::Closed);
        saved.saved = true;
        let normal = make_session("session_normal", "normal", false, SessionStatus::Closed);

        let mut picker = SessionPicker::new(vec![saved, normal]);
        picker
            .selected_session_ids
            .insert("session_saved".to_string());
        picker
            .selected_session_ids
            .insert("session_normal".to_string());

        picker.filter_mode = SessionFilterMode::Saved;
        picker.rebuild_items();

        assert_eq!(picker.selected_session_ids.len(), 1);
        assert!(picker.selected_session_ids.contains("session_saved"));
    }

    #[test]
    fn test_mouse_scroll_only_affects_hovered_pane_without_changing_focus() {
        let s1 = make_session("session_1", "one", false, SessionStatus::Closed);
        let s2 = make_session("session_2", "two", false, SessionStatus::Closed);
        let s3 = make_session("session_3", "three", false, SessionStatus::Closed);
        let mut picker = SessionPicker::new(vec![s1, s2, s3]);

        picker.focus = PaneFocus::Preview;
        picker.scroll_offset = 7;
        picker.last_list_area = Some(Rect::new(0, 0, 20, 10));
        picker.last_preview_area = Some(Rect::new(20, 0, 20, 10));

        picker.handle_overlay_mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 5,
            row: 5,
            modifiers: KeyModifiers::empty(),
        });

        assert_eq!(picker.focus, PaneFocus::Preview);
        assert_eq!(picker.scroll_offset, 0);
        assert_eq!(
            picker.selected_session().map(|s| s.id.as_str()),
            Some("session_2")
        );
    }

    #[test]
    fn test_keyboard_scroll_uses_sessions_focus_for_paging() {
        let s1 = make_session("session_1", "one", false, SessionStatus::Closed);
        let s2 = make_session("session_2", "two", false, SessionStatus::Closed);
        let s3 = make_session("session_3", "three", false, SessionStatus::Closed);
        let s4 = make_session("session_4", "four", false, SessionStatus::Closed);
        let mut picker = SessionPicker::new(vec![s1, s2, s3, s4]);

        picker.focus = PaneFocus::Sessions;
        picker.scroll_offset = 6;

        let result = picker.handle_overlay_key(KeyCode::PageDown, KeyModifiers::empty());

        assert!(matches!(result, Ok(OverlayAction::Continue)));
        assert_eq!(picker.focus, PaneFocus::Sessions);
        assert_eq!(picker.scroll_offset, 0);
        assert_eq!(
            picker.selected_session().map(|s| s.id.as_str()),
            Some("session_4")
        );
    }

    #[test]
    fn test_keyboard_scroll_uses_preview_focus_for_paging() {
        let s1 = make_session("session_1", "one", false, SessionStatus::Closed);
        let s2 = make_session("session_2", "two", false, SessionStatus::Closed);
        let mut picker = SessionPicker::new(vec![s1, s2]);

        picker.focus = PaneFocus::Preview;

        let result = picker.handle_overlay_key(KeyCode::PageDown, KeyModifiers::empty());

        assert!(matches!(result, Ok(OverlayAction::Continue)));
        assert_eq!(picker.focus, PaneFocus::Preview);
        assert_eq!(picker.scroll_offset, PREVIEW_PAGE_SCROLL);
        assert_eq!(
            picker.selected_session().map(|s| s.id.as_str()),
            Some("session_1")
        );
    }
}
