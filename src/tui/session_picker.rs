//! Interactive session picker with preview
//!
//! Shows a list of sessions on the left, with a preview of the selected session's
//! conversation on the right. Sessions are grouped by server for multi-server support.

use crate::id::session_icon;
use crate::message::{ContentBlock, Role};
use crate::registry::{self, ServerInfo};
use crate::session::{self, CrashedSessionsInfo, Session, SessionStatus};
use crate::storage;
use crate::tui::{markdown, DisplayMessage};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};
use std::collections::{HashMap, HashSet};
use std::io::IsTerminal;
use std::time::Duration;

/// Session info for display
#[derive(Clone)]
pub struct SessionInfo {
    pub id: String,
    pub short_name: String,
    pub icon: String,
    pub title: String,
    pub message_count: usize,
    pub user_message_count: usize,
    pub assistant_message_count: usize,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_message_time: chrono::DateTime<chrono::Utc>,
    pub working_dir: Option<String>,
    pub is_canary: bool,
    pub is_debug: bool,
    pub status: SessionStatus,
    pub estimated_tokens: usize,
    pub messages_preview: Vec<PreviewMessage>,
    /// Lowercased searchable text used by picker filtering
    pub search_index: String,
    /// Server name this session belongs to (if running)
    pub server_name: Option<String>,
    /// Server icon
    pub server_icon: Option<String>,
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

fn push_with_byte_budget(dst: &mut String, src: &str, budget: &mut usize) {
    if *budget == 0 || src.is_empty() {
        return;
    }

    let mut end = src.len().min(*budget);
    while end > 0 && !src.is_char_boundary(end) {
        end -= 1;
    }
    if end == 0 {
        return;
    }

    dst.push_str(&src[..end]);
    *budget = budget.saturating_sub(end);
}

fn build_search_index(
    id: &str,
    short_name: &str,
    title: &str,
    working_dir: Option<&str>,
    messages_preview: &[PreviewMessage],
) -> String {
    let mut combined = String::new();
    combined.push_str(title);
    combined.push(' ');
    combined.push_str(short_name);
    combined.push(' ');
    combined.push_str(id);

    if let Some(dir) = working_dir {
        combined.push(' ');
        combined.push_str(dir);
    }

    let mut budget = SEARCH_CONTENT_BUDGET_BYTES;
    for msg in messages_preview {
        let content = msg.content.trim();
        if content.is_empty() {
            continue;
        }
        combined.push(' ');
        push_with_byte_budget(&mut combined, content, &mut budget);
        if budget == 0 {
            break;
        }
    }

    combined.to_lowercase()
}

#[derive(Clone, Debug)]
pub enum PickerResult {
    Selected(String),
    RestoreAllCrashed,
}

/// Load all sessions with their preview data
pub fn load_sessions() -> Result<Vec<SessionInfo>> {
    let sessions_dir = storage::jcode_dir()?.join("sessions");

    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions: Vec<SessionInfo> = Vec::new();

    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "json").unwrap_or(false) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if let Ok(mut session) = Session::load(stem) {
                    let updated = session.detect_crash();
                    if updated {
                        let _ = session.save();
                    }
                    let short_name = session.display_name().to_string();
                    let icon = session_icon(&short_name);

                    // Count messages and estimate tokens
                    let mut user_message_count = 0;
                    let mut assistant_message_count = 0;
                    let mut total_chars = 0;

                    for msg in &session.messages {
                        match msg.role {
                            Role::User => user_message_count += 1,
                            Role::Assistant => assistant_message_count += 1,
                        }
                        for block in &msg.content {
                            if let ContentBlock::Text { text, .. } = block {
                                total_chars += text.len();
                            }
                        }
                    }

                    // Rough token estimate: ~4 chars per token
                    let estimated_tokens = total_chars / 4;

                    // Extract preview messages (last 20 rendered entries)
                    let messages_preview: Vec<PreviewMessage> = session::render_messages(&session)
                        .into_iter()
                        .rev()
                        .take(20)
                        .rev()
                        .map(|msg| PreviewMessage {
                            role: msg.role,
                            content: msg.content,
                            tool_calls: msg.tool_calls,
                            tool_data: msg.tool_data,
                            timestamp: None,
                        })
                        .collect();

                    let status = session.status.clone();

                    // Skip sessions with no messages
                    if session.messages.is_empty() {
                        continue;
                    }

                    let title = session.title.unwrap_or_else(|| "Untitled".to_string());
                    let search_index = build_search_index(
                        stem,
                        &short_name,
                        &title,
                        session.working_dir.as_deref(),
                        &messages_preview,
                    );

                    sessions.push(SessionInfo {
                        id: stem.to_string(),
                        short_name,
                        icon: icon.to_string(),
                        title,
                        message_count: session.messages.len(),
                        user_message_count,
                        assistant_message_count,
                        created_at: session.created_at,
                        last_message_time: session.updated_at,
                        working_dir: session.working_dir,
                        is_canary: session.is_canary,
                        is_debug: session.is_debug,
                        status,
                        estimated_tokens,
                        messages_preview,
                        search_index,
                        server_name: None,
                        server_icon: None,
                    });
                }
            }
        }
    }

    // Sort by last message time (most recent first)
    sessions.sort_by(|a, b| b.last_message_time.cmp(&a.last_message_time));

    Ok(sessions)
}

/// Load running servers from the registry
pub fn load_servers() -> Vec<ServerInfo> {
    // Check if we're inside an async runtime
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        // We're inside a runtime - use block_in_place to safely block
        tokio::task::block_in_place(|| {
            handle.block_on(async { registry::list_servers().await.unwrap_or_default() })
        })
    } else {
        // No runtime - create a new one (sync context)
        tokio::runtime::Runtime::new()
            .map(|rt| rt.block_on(async { registry::list_servers().await.unwrap_or_default() }))
            .unwrap_or_default()
    }
}

/// Load sessions grouped by server
/// Returns (running_servers, orphan_sessions)
pub fn load_sessions_grouped() -> Result<(Vec<ServerGroup>, Vec<SessionInfo>)> {
    let all_sessions = load_sessions()?;
    let servers = load_servers();

    // Build a map of session names to their server
    let mut session_to_server: HashMap<String, &ServerInfo> = HashMap::new();
    for server in &servers {
        for session_name in &server.sessions {
            session_to_server.insert(session_name.clone(), server);
        }
    }

    // Group sessions by server
    let mut server_sessions: HashMap<String, Vec<SessionInfo>> = HashMap::new();
    let mut orphan_sessions: Vec<SessionInfo> = Vec::new();

    for mut session in all_sessions {
        if let Some(server) = session_to_server.get(&session.short_name) {
            session.server_name = Some(server.name.clone());
            session.server_icon = Some(server.icon.clone());
            server_sessions
                .entry(server.name.clone())
                .or_default()
                .push(session);
        } else {
            orphan_sessions.push(session);
        }
    }

    // Build server groups
    let mut groups: Vec<ServerGroup> = servers
        .iter()
        .map(|server| {
            let sessions = server_sessions.remove(&server.name).unwrap_or_default();
            ServerGroup {
                name: server.name.clone(),
                icon: server.icon.clone(),
                version: server.version.clone(),
                git_hash: server.git_hash.clone(),
                is_running: true,
                sessions,
            }
        })
        .collect();

    // Sort groups by newest session activity
    groups.sort_by(|a, b| {
        let a_latest = a.sessions.iter().map(|s| s.last_message_time).max();
        let b_latest = b.sessions.iter().map(|s| s.last_message_time).max();
        b_latest.cmp(&a_latest)
    });

    Ok((groups, orphan_sessions))
}

/// Safely truncate a string at a character boundary
fn safe_truncate(s: &str, max_chars: usize) -> &str {
    if s.chars().count() <= max_chars {
        s
    } else {
        let mut end = 0;
        for (i, (idx, _)) in s.char_indices().enumerate() {
            if i >= max_chars {
                break;
            }
            end = idx;
        }
        // Include the last character
        if let Some((idx, c)) = s.char_indices().nth(max_chars) {
            &s[..idx]
        } else {
            s
        }
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

/// Which pane has keyboard focus
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaneFocus {
    /// Session list (left pane) - j/k navigate sessions
    Sessions,
    /// Preview (right pane) - j/k scroll preview
    Preview,
}

/// An item in the picker list - either a server header or a session
#[derive(Clone)]
pub enum PickerItem {
    ServerHeader {
        name: String,
        icon: String,
        version: String,
        session_count: usize,
    },
    Session(SessionInfo),
    OrphanHeader {
        session_count: usize,
    },
}

/// Interactive session picker
pub struct SessionPicker {
    /// Flat list of items (headers and sessions)
    items: Vec<PickerItem>,
    /// Just the sessions for selection (filtered view)
    sessions: Vec<SessionInfo>,
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
    /// Number of running servers
    server_count: usize,
    /// Crashed sessions pending batch restore
    crashed_sessions: Option<CrashedSessionsInfo>,
    /// IDs of sessions that are eligible for current batch restore
    crashed_session_ids: HashSet<String>,
    last_list_area: Option<Rect>,
    last_preview_area: Option<Rect>,
    /// Whether to show debug/test/canary sessions
    show_test_sessions: bool,
    /// Search query for filtering sessions
    search_query: String,
    /// Whether we're in search input mode
    search_active: bool,
    /// Total session count (before filtering)
    total_session_count: usize,
    /// Hidden test session count (debug + canary)
    hidden_test_count: usize,
    /// Which pane has keyboard focus
    focus: PaneFocus,
}

impl SessionPicker {
    pub fn new(sessions: Vec<SessionInfo>) -> Self {
        let total_session_count = sessions.len();
        let hidden_test_count = sessions.iter().filter(|s| s.is_debug).count();

        // Filter out debug/test sessions by default (canary sessions are shown)
        let visible: Vec<SessionInfo> = sessions.iter().filter(|s| !s.is_debug).cloned().collect();

        let items: Vec<PickerItem> = visible.iter().cloned().map(PickerItem::Session).collect();
        let item_to_session: Vec<Option<usize>> = (0..visible.len()).map(Some).collect();

        let mut list_state = ListState::default();
        if !visible.is_empty() {
            list_state.select(Some(0));
        }

        let crashed_sessions = session::detect_crashed_sessions().ok().flatten();
        let crashed_session_ids: HashSet<String> = crashed_sessions
            .as_ref()
            .map(|info| info.session_ids.iter().cloned().collect())
            .unwrap_or_default();

        Self {
            items,
            sessions: visible,
            all_sessions: sessions,
            all_server_groups: Vec::new(),
            all_orphan_sessions: Vec::new(),
            item_to_session,
            list_state,
            scroll_offset: 0,
            auto_scroll_preview: true,
            server_count: 0,
            crashed_sessions,
            crashed_session_ids,
            last_list_area: None,
            last_preview_area: None,
            show_test_sessions: false,
            search_query: String::new(),
            search_active: false,
            total_session_count,
            hidden_test_count,
            focus: PaneFocus::Sessions,
        }
    }

    /// Create a picker with server grouping
    pub fn new_grouped(server_groups: Vec<ServerGroup>, orphan_sessions: Vec<SessionInfo>) -> Self {
        // Count totals before filtering
        let total_session_count: usize = server_groups
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

        // Build filtered view (hide debug/test sessions by default, canary sessions are shown)
        let mut items: Vec<PickerItem> = Vec::new();
        let mut all_sessions: Vec<SessionInfo> = Vec::new();
        let mut item_to_session: Vec<Option<usize>> = Vec::new();

        let server_count = server_groups.len();

        for group in &server_groups {
            let visible_sessions: Vec<&SessionInfo> =
                group.sessions.iter().filter(|s| !s.is_debug).collect();

            if visible_sessions.is_empty() {
                continue;
            }

            items.push(PickerItem::ServerHeader {
                name: group.name.clone(),
                icon: group.icon.clone(),
                version: group.version.clone(),
                session_count: visible_sessions.len(),
            });
            item_to_session.push(None);

            for session in visible_sessions {
                let session_idx = all_sessions.len();
                all_sessions.push(session.clone());
                items.push(PickerItem::Session(session.clone()));
                item_to_session.push(Some(session_idx));
            }
        }

        let visible_orphans: Vec<&SessionInfo> =
            orphan_sessions.iter().filter(|s| !s.is_debug).collect();
        if !visible_orphans.is_empty() {
            items.push(PickerItem::OrphanHeader {
                session_count: visible_orphans.len(),
            });
            item_to_session.push(None);

            for session in visible_orphans {
                let session_idx = all_sessions.len();
                all_sessions.push(session.clone());
                items.push(PickerItem::Session(session.clone()));
                item_to_session.push(Some(session_idx));
            }
        }

        let first_session_idx = item_to_session.iter().position(|x| x.is_some());
        let mut list_state = ListState::default();
        if let Some(idx) = first_session_idx {
            list_state.select(Some(idx));
        }

        let crashed_sessions = session::detect_crashed_sessions().ok().flatten();
        let crashed_session_ids: HashSet<String> = crashed_sessions
            .as_ref()
            .map(|info| info.session_ids.iter().cloned().collect())
            .unwrap_or_default();

        Self {
            items,
            sessions: all_sessions,
            all_sessions: Vec::new(), // will be populated in rebuild_items
            all_server_groups: server_groups,
            all_orphan_sessions: orphan_sessions,
            item_to_session,
            list_state,
            scroll_offset: 0,
            auto_scroll_preview: true,
            server_count,
            crashed_sessions,
            crashed_session_ids,
            last_list_area: None,
            last_preview_area: None,
            show_test_sessions: false,
            search_query: String::new(),
            search_active: false,
            total_session_count,
            hidden_test_count,
            focus: PaneFocus::Sessions,
        }
    }

    pub fn selected_session(&self) -> Option<&SessionInfo> {
        self.list_state.selected().and_then(|i| {
            self.item_to_session
                .get(i)
                .and_then(|opt| opt.as_ref())
                .and_then(|session_idx| self.sessions.get(*session_idx))
        })
    }

    /// Find next selectable item (skip headers)
    fn next_selectable(&self, from: usize) -> Option<usize> {
        for i in (from + 1)..self.items.len() {
            if self
                .item_to_session
                .get(i)
                .map(|x| x.is_some())
                .unwrap_or(false)
            {
                return Some(i);
            }
        }
        None
    }

    /// Find previous selectable item (skip headers)
    fn prev_selectable(&self, from: usize) -> Option<usize> {
        for i in (0..from).rev() {
            if self
                .item_to_session
                .get(i)
                .map(|x| x.is_some())
                .unwrap_or(false)
            {
                return Some(i);
            }
        }
        None
    }

    pub fn next(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        if let Some(next) = self.next_selectable(current) {
            self.list_state.select(Some(next));
            self.scroll_offset = 0;
            self.auto_scroll_preview = true;
        }
    }

    pub fn previous(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        if let Some(prev) = self.prev_selectable(current) {
            self.list_state.select(Some(prev));
            self.scroll_offset = 0;
            self.auto_scroll_preview = true;
        }
    }

    pub fn scroll_preview_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(3);
    }

    pub fn scroll_preview_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(3);
    }

    fn point_in_rect(col: u16, row: u16, rect: Rect) -> bool {
        col >= rect.x
            && col < rect.x.saturating_add(rect.width)
            && row >= rect.y
            && row < rect.y.saturating_add(rect.height)
    }

    fn handle_mouse_scroll(&mut self, col: u16, row: u16, kind: MouseEventKind) {
        let over_preview = self
            .last_preview_area
            .map(|r| Self::point_in_rect(col, row, r))
            .unwrap_or(false);
        let over_list = self
            .last_list_area
            .map(|r| Self::point_in_rect(col, row, r))
            .unwrap_or(false);

        if over_preview {
            match kind {
                MouseEventKind::ScrollUp => self.scroll_preview_up(),
                MouseEventKind::ScrollDown => self.scroll_preview_down(),
                _ => {}
            }
            return;
        }

        if over_list {
            match kind {
                MouseEventKind::ScrollUp => self.previous(),
                MouseEventKind::ScrollDown => self.next(),
                _ => {}
            }
        }
    }

    /// Check if a session matches the current search query
    fn session_matches_search(session: &SessionInfo, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        let q = query.to_lowercase();
        session.search_index.contains(&q)
    }

    /// Rebuild the items list based on current filters (show_test_sessions, search_query)
    fn rebuild_items(&mut self) {
        let show_test = self.show_test_sessions;
        let query = self.search_query.clone();

        let session_visible = |s: &SessionInfo| -> bool {
            (show_test || !s.is_debug) && Self::session_matches_search(s, &query)
        };

        self.items.clear();
        self.sessions.clear();
        self.item_to_session.clear();

        if !self.all_server_groups.is_empty() {
            // Grouped mode
            for group in &self.all_server_groups {
                let visible: Vec<&SessionInfo> = group
                    .sessions
                    .iter()
                    .filter(|s| session_visible(s))
                    .collect();

                if visible.is_empty() {
                    continue;
                }

                self.items.push(PickerItem::ServerHeader {
                    name: group.name.clone(),
                    icon: group.icon.clone(),
                    version: group.version.clone(),
                    session_count: visible.len(),
                });
                self.item_to_session.push(None);

                for session in visible {
                    let session_idx = self.sessions.len();
                    self.sessions.push(session.clone());
                    self.items.push(PickerItem::Session(session.clone()));
                    self.item_to_session.push(Some(session_idx));
                }
            }

            let visible_orphans: Vec<&SessionInfo> = self
                .all_orphan_sessions
                .iter()
                .filter(|s| session_visible(s))
                .collect();
            if !visible_orphans.is_empty() {
                self.items.push(PickerItem::OrphanHeader {
                    session_count: visible_orphans.len(),
                });
                self.item_to_session.push(None);

                for session in visible_orphans {
                    let session_idx = self.sessions.len();
                    self.sessions.push(session.clone());
                    self.items.push(PickerItem::Session(session.clone()));
                    self.item_to_session.push(Some(session_idx));
                }
            }
        } else {
            // Simple mode (no server grouping)
            for session in &self.all_sessions {
                if session_visible(session) {
                    let session_idx = self.sessions.len();
                    self.sessions.push(session.clone());
                    self.items.push(PickerItem::Session(session.clone()));
                    self.item_to_session.push(Some(session_idx));
                }
            }
        }

        // Update hidden debug count based on current search
        self.hidden_test_count = if show_test {
            0
        } else {
            self.all_server_groups
                .iter()
                .flat_map(|g| g.sessions.iter())
                .chain(self.all_orphan_sessions.iter())
                .chain(self.all_sessions.iter())
                .filter(|s| s.is_debug && Self::session_matches_search(s, &query))
                .count()
        };

        // Select first selectable item
        let first = self.item_to_session.iter().position(|x| x.is_some());
        self.list_state.select(first);
        self.scroll_offset = 0;
        self.auto_scroll_preview = true;
    }

    /// Toggle debug session visibility
    fn toggle_test_sessions(&mut self) {
        self.show_test_sessions = !self.show_test_sessions;
        self.rebuild_items();
    }

    fn crash_reason_line(session: &SessionInfo) -> Option<Line<'static>> {
        let reason = match &session.status {
            SessionStatus::Crashed { message } => message
                .as_deref()
                .unwrap_or("Unexpected termination (no additional details)"),
            _ => return None,
        };

        let reason_display = if reason.chars().count() > 54 {
            format!("{}...", safe_truncate(reason, 51))
        } else {
            reason.to_string()
        };

        Some(Line::from(vec![
            Span::styled("     ", Style::default()),
            Span::styled(
                format!("reason: {}", reason_display),
                Style::default().fg(Color::Rgb(220, 120, 120)),
            ),
        ]))
    }

    fn render_session_item(&self, session: &SessionInfo, is_selected: bool) -> ListItem<'static> {
        const DIM: Color = Color::Rgb(100, 100, 100);
        const DIMMER: Color = Color::Rgb(70, 70, 70);
        const USER_CLR: Color = Color::Rgb(138, 180, 248);
        const ACCENT: Color = Color::Rgb(186, 139, 255);
        const BATCH_RESTORE: Color = Color::Rgb(255, 140, 140);
        const BATCH_ROW_BG: Color = Color::Rgb(36, 18, 18);

        let last_msg_ago = format_time_ago(session.last_message_time);
        let created_ago = format_time_ago(session.created_at);
        let in_batch_restore = self.crashed_session_ids.contains(&session.id);

        // Name style
        let name_style = if is_selected {
            Style::default()
                .fg(Color::Rgb(140, 220, 160))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let canary_marker = if session.is_canary { " 🔬" } else { "" };
        let debug_marker = if session.is_debug { " 🧪" } else { "" };

        // Status indicator with color
        let (status_icon, status_color) = match &session.status {
            SessionStatus::Active => ("▶", Color::Rgb(100, 200, 100)),
            SessionStatus::Closed => ("✓", DIM),
            SessionStatus::Crashed { .. } => ("💥", Color::Rgb(220, 100, 100)),
            SessionStatus::Reloaded => ("🔄", USER_CLR),
            SessionStatus::Compacted => ("📦", Color::Rgb(255, 193, 7)),
            SessionStatus::RateLimited => ("⏳", ACCENT),
            SessionStatus::Error { .. } => ("❌", Color::Rgb(220, 100, 100)),
        };

        // Line 1: icon + name + status + last message time
        let mut line1_spans = vec![
            Span::styled("  ", Style::default()), // Indent for sessions under server
            Span::styled(
                format!("{} ", session.icon),
                Style::default().fg(Color::Rgb(110, 210, 255)),
            ),
            Span::styled(session.short_name.clone(), name_style),
            Span::styled(canary_marker, Style::default().fg(Color::Rgb(255, 193, 7))),
            Span::styled(debug_marker, Style::default().fg(Color::Rgb(180, 180, 180))),
            Span::styled(
                format!(" {}", status_icon),
                Style::default().fg(status_color),
            ),
            Span::styled(
                format!("  last: {}", last_msg_ago),
                Style::default().fg(DIM),
            ),
        ];
        if in_batch_restore {
            line1_spans.push(Span::styled(
                "  [BATCH]",
                Style::default()
                    .fg(BATCH_RESTORE)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        let line1 = Line::from(line1_spans);

        // Line 2: title (truncated)
        let title_display = if session.title.chars().count() > 42 {
            format!("{}...", safe_truncate(&session.title, 39))
        } else {
            session.title.clone()
        };
        let line2 = Line::from(vec![
            Span::styled("     ", Style::default()),
            Span::styled(
                title_display,
                Style::default().fg(Color::Rgb(180, 180, 180)),
            ),
        ]);

        // Line 3: stats - user msgs, assistant msgs, tokens
        let tokens_display = if session.estimated_tokens >= 1000 {
            format!("~{}k tok", session.estimated_tokens / 1000)
        } else {
            format!("~{} tok", session.estimated_tokens)
        };
        let line3 = Line::from(vec![
            Span::styled("     ", Style::default()),
            Span::styled(
                format!("{}", session.user_message_count),
                Style::default().fg(USER_CLR),
            ),
            Span::styled(" user", Style::default().fg(DIMMER)),
            Span::styled(" · ", Style::default().fg(DIMMER)),
            Span::styled(
                format!("{}", session.assistant_message_count),
                Style::default().fg(Color::Rgb(129, 199, 132)),
            ),
            Span::styled(" assistant", Style::default().fg(DIMMER)),
            Span::styled(" · ", Style::default().fg(DIMMER)),
            Span::styled(tokens_display, Style::default().fg(DIMMER)),
        ]);

        // Line 4: created time + working dir
        let dir_part = if let Some(ref dir) = session.working_dir {
            let dir_display = if dir.chars().count() > 28 {
                let chars: Vec<char> = dir.chars().collect();
                let suffix: String = chars
                    .iter()
                    .rev()
                    .take(25)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                format!("...{}", suffix)
            } else {
                dir.clone()
            };
            format!("  📁 {}", dir_display)
        } else {
            String::new()
        };
        let line4 = Line::from(vec![
            Span::styled("     ", Style::default()),
            Span::styled(
                format!("created: {}", created_ago),
                Style::default().fg(DIMMER),
            ),
            Span::styled(dir_part, Style::default().fg(DIMMER)),
        ]);

        let mut rows = vec![line1, line2, line3, line4];
        if let Some(reason_line) = Self::crash_reason_line(session) {
            rows.push(reason_line);
        }
        rows.push(Line::from(""));

        let mut item = ListItem::new(rows);
        if in_batch_restore && !is_selected {
            item = item.style(Style::default().bg(BATCH_ROW_BG));
        }
        item
    }

    fn render_session_list(&mut self, frame: &mut Frame, area: Rect) {
        const SERVER_COLOR: Color = Color::Rgb(255, 200, 100); // Amber for server headers
        const DIM: Color = Color::Rgb(100, 100, 100);

        let items: Vec<ListItem> = self
            .items
            .iter()
            .enumerate()
            .map(|(idx, item)| {
                let is_selected = self.list_state.selected() == Some(idx);

                match item {
                    PickerItem::ServerHeader {
                        name,
                        icon,
                        version,
                        session_count,
                    } => {
                        // Server header - not selectable, acts as a group label
                        let line1 = Line::from(vec![
                            Span::styled(format!("{} ", icon), Style::default().fg(SERVER_COLOR)),
                            Span::styled(
                                name.clone(),
                                Style::default()
                                    .fg(SERVER_COLOR)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("  {} · {} sessions", version, session_count),
                                Style::default().fg(DIM),
                            ),
                        ]);
                        ListItem::new(vec![line1])
                    }
                    PickerItem::OrphanHeader { session_count } => {
                        // Orphan sessions header
                        let line1 = Line::from(vec![
                            Span::styled("📦 ", Style::default().fg(DIM)),
                            Span::styled(
                                "Other sessions",
                                Style::default().fg(DIM).add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("  {} sessions", session_count),
                                Style::default().fg(DIM),
                            ),
                        ]);
                        ListItem::new(vec![line1])
                    }
                    PickerItem::Session(session) => self.render_session_item(session, is_selected),
                }
            })
            .collect();

        // Title with session count and filter state
        let mut title_parts: Vec<Span> = Vec::new();
        title_parts.push(Span::styled(
            format!(" {} ", self.sessions.len()),
            Style::default()
                .fg(Color::Rgb(200, 200, 200))
                .add_modifier(Modifier::BOLD),
        ));
        title_parts.push(Span::styled(
            "sessions",
            Style::default().fg(Color::Rgb(120, 120, 120)),
        ));

        if self.hidden_test_count > 0 {
            title_parts.push(Span::styled(
                format!(" (+{} hidden)", self.hidden_test_count),
                Style::default().fg(Color::Rgb(80, 80, 80)),
            ));
        }

        if !self.search_query.is_empty() {
            title_parts.push(Span::styled(
                format!("  🔍 \"{}\"", self.search_query),
                Style::default().fg(Color::Rgb(186, 139, 255)),
            ));
        }

        title_parts.push(Span::styled(" ", Style::default()));

        // Help text on the right
        let help = if self.search_active {
            " type to filter, Esc cancel "
        } else {
            " / search · d show all · h/l focus · ↑↓ · Enter · q "
        };

        const BORDER_DIM: Color = Color::Rgb(70, 70, 70);
        const BORDER_FOCUS: Color = Color::Rgb(130, 130, 160);
        let border_color = if self.focus == PaneFocus::Sessions {
            BORDER_FOCUS
        } else {
            BORDER_DIM
        };

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(Line::from(title_parts))
                    .title_bottom(Line::from(Span::styled(
                        help,
                        Style::default().fg(Color::Rgb(80, 80, 80)),
                    )))
                    .border_style(Style::default().fg(border_color)),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::Rgb(40, 44, 52))
                    .add_modifier(Modifier::BOLD),
            );

        frame.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn render_preview(&mut self, frame: &mut Frame, area: Rect) {
        // Colors matching the actual TUI
        const USER_COLOR: Color = Color::Rgb(138, 180, 248); // Soft blue
        const USER_TEXT: Color = Color::Rgb(245, 245, 255); // Bright cool white
        const DIM_COLOR: Color = Color::Rgb(80, 80, 80); // Dim gray
        const HEADER_ICON_COLOR: Color = Color::Rgb(120, 210, 230); // Teal
        const HEADER_SESSION_COLOR: Color = Color::Rgb(255, 255, 255); // White

        let empty_border_color = if self.focus == PaneFocus::Preview {
            Color::Rgb(130, 130, 160)
        } else {
            Color::Rgb(50, 50, 50)
        };
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
                    Style::default().fg(HEADER_ICON_COLOR),
                ),
                Span::styled(
                    session.short_name.clone(),
                    Style::default()
                        .fg(HEADER_SESSION_COLOR)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {}", format_time_ago(session.last_message_time)),
                    Style::default().fg(DIM_COLOR),
                ),
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

        // Working directory
        if let Some(ref dir) = session.working_dir {
            lines.push(
                Line::from(vec![Span::styled(
                    format!("📁 {}", dir),
                    Style::default().fg(DIM_COLOR),
                )])
                .alignment(align),
            );
        }

        // Status line with details
        let (status_icon, status_text, status_color) = match &session.status {
            SessionStatus::Active => ("▶", "Active".to_string(), Color::Rgb(100, 200, 100)),
            SessionStatus::Closed => ("✓", "Closed normally".to_string(), Color::DarkGray),
            SessionStatus::Crashed { message } => {
                let text = match message {
                    Some(msg) => format!("Crashed: {}", safe_truncate(msg, 80)),
                    None => "Crashed".to_string(),
                };
                ("💥", text, Color::Rgb(220, 100, 100))
            }
            SessionStatus::Reloaded => ("🔄", "Reloaded".to_string(), Color::Rgb(138, 180, 248)),
            SessionStatus::Compacted => (
                "📦",
                "Compacted (context too large)".to_string(),
                Color::Rgb(255, 193, 7),
            ),
            SessionStatus::RateLimited => {
                ("⏳", "Rate limited".to_string(), Color::Rgb(186, 139, 255))
            }
            SessionStatus::Error { message } => {
                let text = format!("Error: {}", safe_truncate(message, 40));
                ("❌", text, Color::Rgb(220, 100, 100))
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
                        .fg(Color::Rgb(255, 140, 140))
                        .add_modifier(Modifier::BOLD),
                )])
                .alignment(align),
            );
        }

        lines.push(Line::from("").alignment(align));
        lines.push(
            Line::from(vec![Span::styled(
                "─".repeat(area.width.saturating_sub(4) as usize),
                Style::default().fg(Color::Rgb(60, 60, 60)),
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
                                Style::default().fg(USER_COLOR),
                            ),
                            Span::styled("› ", Style::default().fg(USER_COLOR)),
                            Span::styled(display_msg.content, Style::default().fg(USER_TEXT)),
                        ])
                        .alignment(align),
                    );
                    rendered_messages += 1;
                }
                "assistant" => {
                    let md_lines =
                        super::ui::render_assistant_message(&display_msg, assistant_width, crate::config::DiffDisplayMode::Off);
                    let mut skip_mermaid_blank = false;

                    for line in md_lines {
                        if super::mermaid::parse_image_placeholder(&line).is_some() {
                            lines.push(
                                Line::from(vec![Span::styled(
                                    "[mermaid diagram]",
                                    Style::default().fg(DIM_COLOR),
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
                            Style::default().fg(DIM_COLOR),
                        )])
                        .alignment(align),
                    );
                    rendered_messages += 1;
                }
                "system" => {
                    let should_render_markdown = msg.content.contains('\n')
                        || msg.content.contains("```")
                        || msg.content.contains("# ")
                        || msg.content.contains("- ");

                    if should_render_markdown {
                        let md_lines = markdown::render_markdown_with_width(
                            &msg.content,
                            Some(assistant_width as usize),
                        );
                        for line in md_lines {
                            lines.push(super::ui::align_if_unset(line, align));
                            rendered_messages += 1;
                        }
                    } else {
                        lines.push(
                            Line::from(vec![Span::styled(
                                msg.content.clone(),
                                Style::default()
                                    .fg(Color::Rgb(186, 139, 255))
                                    .add_modifier(Modifier::ITALIC),
                            )])
                            .alignment(align),
                        );
                        rendered_messages += 1;
                    }
                }
                "usage" => {
                    lines.push(
                        Line::from(vec![Span::styled(
                            msg.content.clone(),
                            Style::default().fg(DIM_COLOR),
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
                    Style::default().fg(DIM_COLOR),
                )])
                .alignment(align),
            );
        }

        let preview_border_color = if self.focus == PaneFocus::Preview {
            Color::Rgb(130, 130, 160)
        } else {
            Color::Rgb(70, 70, 70)
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

    fn render_crash_banner(&self, frame: &mut Frame, area: Rect) {
        const CRASH_BG: Color = Color::Rgb(60, 30, 30);
        const CRASH_FG: Color = Color::Rgb(255, 140, 140);
        const CRASH_ICON: Color = Color::Rgb(255, 100, 100);
        const DIM: Color = Color::Rgb(180, 140, 140);

        let Some(info) = &self.crashed_sessions else {
            return;
        };

        let count = info.session_ids.len();
        let names: String = info
            .display_names
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let names_display = if count > 3 {
            format!("{} (+{} more)", names, count - 3)
        } else {
            names
        };

        let ago = format_time_ago(info.most_recent_crash);

        let line = Line::from(vec![
            Span::styled(" 💥 ", Style::default().fg(CRASH_ICON).bg(CRASH_BG)),
            Span::styled(
                format!(
                    "{} crashed session{}",
                    count,
                    if count == 1 { "" } else { "s" }
                ),
                Style::default()
                    .fg(CRASH_FG)
                    .bg(CRASH_BG)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({}) ", names_display),
                Style::default().fg(DIM).bg(CRASH_BG),
            ),
            Span::styled(format!("{} ", ago), Style::default().fg(DIM).bg(CRASH_BG)),
            Span::styled("— Press ", Style::default().fg(DIM).bg(CRASH_BG)),
            Span::styled(
                "B",
                Style::default()
                    .fg(Color::White)
                    .bg(CRASH_BG)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to batch restore ", Style::default().fg(DIM).bg(CRASH_BG)),
        ]);

        // Fill the rest of the line with background
        let paragraph = Paragraph::new(line).style(Style::default().bg(CRASH_BG));
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
                Span::styled(" 🔍 ", Style::default().fg(Color::Rgb(186, 139, 255))),
                Span::styled(
                    &self.search_query,
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(cursor_char, Style::default().fg(Color::Rgb(186, 139, 255))),
                if self.search_active {
                    Span::styled(
                        "  Esc to clear",
                        Style::default().fg(Color::Rgb(60, 60, 60)),
                    )
                } else {
                    Span::styled("  / to edit", Style::default().fg(Color::Rgb(60, 60, 60)))
                },
            ]);
            let search_widget =
                Paragraph::new(search_line).style(Style::default().bg(Color::Rgb(25, 25, 30)));
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
        // Initialize mermaid image picker (queries terminal for graphics protocol support)
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
                                    // Keep the query, just exit search input mode
                                    if self.sessions.is_empty() {
                                        // No results - clear and continue
                                    } else {
                                        // Select current item
                                        break Ok(self
                                            .selected_session()
                                            .map(|s| PickerResult::Selected(s.id.clone())));
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
                            KeyCode::Esc | KeyCode::Char('q') => {
                                break Ok(None);
                            }
                            KeyCode::Enter => {
                                break Ok(self
                                    .selected_session()
                                    .map(|s| PickerResult::Selected(s.id.clone())));
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
                            KeyCode::Char('h') | KeyCode::Left => {
                                self.focus = PaneFocus::Sessions;
                            }
                            KeyCode::Char('l') | KeyCode::Right => {
                                self.focus = PaneFocus::Preview;
                            }
                            KeyCode::Tab => {
                                self.focus = match self.focus {
                                    PaneFocus::Sessions => PaneFocus::Preview,
                                    PaneFocus::Preview => PaneFocus::Sessions,
                                };
                            }
                            KeyCode::Down => {
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    self.scroll_preview_down();
                                } else {
                                    match self.focus {
                                        PaneFocus::Sessions => self.next(),
                                        PaneFocus::Preview => self.scroll_preview_down(),
                                    }
                                }
                            }
                            KeyCode::Up => {
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    self.scroll_preview_up();
                                } else {
                                    match self.focus {
                                        PaneFocus::Sessions => self.previous(),
                                        PaneFocus::Preview => self.scroll_preview_up(),
                                    }
                                }
                            }
                            KeyCode::Char('j') | KeyCode::Char('J') => {
                                let force_preview = key.modifiers.contains(KeyModifiers::SHIFT)
                                    || matches!(key.code, KeyCode::Char('J'));
                                if force_preview {
                                    self.scroll_preview_down();
                                } else {
                                    match self.focus {
                                        PaneFocus::Sessions => self.next(),
                                        PaneFocus::Preview => self.scroll_preview_down(),
                                    }
                                }
                            }
                            KeyCode::Char('k') | KeyCode::Char('K') => {
                                let force_preview = key.modifiers.contains(KeyModifiers::SHIFT)
                                    || matches!(key.code, KeyCode::Char('K'));
                                if force_preview {
                                    self.scroll_preview_up();
                                } else {
                                    match self.focus {
                                        PaneFocus::Sessions => self.previous(),
                                        PaneFocus::Preview => self.scroll_preview_up(),
                                    }
                                }
                            }
                            KeyCode::PageDown => {
                                self.scroll_preview_down();
                                self.scroll_preview_down();
                                self.scroll_preview_down();
                            }
                            KeyCode::PageUp => {
                                self.scroll_preview_up();
                                self.scroll_preview_up();
                                self.scroll_preview_up();
                            }
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
            &messages_preview,
        );

        SessionInfo {
            id: id.to_string(),
            short_name: short_name.to_string(),
            icon: "🧪".to_string(),
            title,
            message_count: 2,
            user_message_count: 1,
            assistant_message_count: 1,
            created_at: now - ChronoDuration::minutes(5),
            last_message_time: now - ChronoDuration::minutes(1),
            working_dir,
            is_canary,
            is_debug,
            status,
            estimated_tokens: 200,
            messages_preview,
            search_index,
            server_name: None,
            server_icon: None,
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
    fn test_toggle_test_sessions_rebuilds_visibility() {
        let normal = make_session("session_normal", "normal", false, SessionStatus::Closed);
        let debug = make_session("session_debug", "debug", true, SessionStatus::Closed);

        let mut picker = SessionPicker::new(vec![normal.clone(), debug.clone()]);

        assert_eq!(picker.sessions.len(), 1);
        assert!(!picker.show_test_sessions);
        assert_eq!(picker.hidden_test_count, 1);

        picker.toggle_test_sessions();
        assert!(picker.show_test_sessions);
        assert_eq!(picker.sessions.len(), 2);
        assert_eq!(picker.hidden_test_count, 0);

        picker.toggle_test_sessions();
        assert!(!picker.show_test_sessions);
        assert_eq!(picker.sessions.len(), 1);
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
        assert_eq!(picker.sessions.len(), 3); // normal + canary + orphan_normal
        assert!(picker.sessions.iter().all(|s| !s.is_debug));
        assert_eq!(picker.hidden_test_count, 2); // debug + orphan_debug

        picker.toggle_test_sessions();
        assert!(picker.show_test_sessions);
        assert_eq!(picker.sessions.len(), 5);
        assert_eq!(picker.hidden_test_count, 0);
        assert!(picker.sessions.iter().any(|s| s.is_debug));
        assert!(picker.sessions.iter().any(|s| s.is_canary));
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
    fn test_filter_matches_recent_message_content() {
        let mut picker = SessionPicker::new(vec![make_session(
            "session_content",
            "content",
            false,
            SessionStatus::Closed,
        )]);

        picker.search_query = "world".to_string();
        picker.rebuild_items();
        assert_eq!(picker.sessions.len(), 1);

        picker.search_query = "not-in-preview".to_string();
        picker.rebuild_items();
        assert!(picker.sessions.is_empty());
    }
}
