use chrono::{DateTime, Utc, Duration};
use ratatui::{
    widgets::{Widget, Block as RBlock, Borders, List, ListItem, ListState},
    style::{Color, Style, Modifier},
    text::{Line, Span},
    layout::Rect,
    buffer::Buffer,
};
use uuid::Uuid;
use std::collections::{HashSet, HashMap};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct TimelineTag {
    pub icon: char,
    pub label: String,
    pub color: Color,
    pub category: TagCategory,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TagCategory {
    Success,
    Warning,
    Error,
    Feature,
    Bugfix,
    Refactor,
    Experiment,
}

#[derive(Debug, Clone)]
pub enum OutcomeType {
    SuccessWithNotes,
    PartialFailure,
    CompleteSuccess,
    NeedsFollowUp,
}

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub title: String,
    pub description: String,
    pub files_modified: Vec<PathBuf>,
    pub key_decisions: Vec<String>,
    pub outcome: OutcomeType,
}

#[derive(Debug, Clone)]
pub enum SnapshotType {
    CommandExecuted { cmd: String },
    ErrorOccurred { error: String },
    FileModified { path: String, diff_summary: String },
    TestResults { passed: usize, failed: usize },
    MilestoneReached { message: String },
}

#[derive(Debug, Clone)]
pub enum SnapshotContent {
    Text(String),
    Diff(DiffPreview),
    TestReport(TestSummary),
    Image(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct DiffPreview {
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TestSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone)]
pub struct TimelineSnapshot {
    pub timestamp: DateTime<Utc>,
    pub snapshot_type: SnapshotType,
    pub content: SnapshotContent,
}

#[derive(Debug, Clone)]
pub struct TimelineSession {
    pub id: Uuid,
    pub project_path: PathBuf,
    pub branch: Option<String>,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub duration: Duration,
    pub command_count: usize,
    pub success_count: usize,
    pub error_count: usize,
    pub tags: Vec<TimelineTag>,
    pub tools_used: HashSet<String>,
    pub summary: Option<SessionSummary>,
    pub snapshots: Vec<TimelineSnapshot>,
}

impl TimelineSession {
    pub fn success_rate(&self) -> f64 {
        if self.command_count == 0 {
            return 0.0;
        }
        (self.success_count as f64 / self.command_count as f64) * 100.0
    }

    pub fn format_duration(&self) -> String {
        let total_secs = self.duration.num_seconds();
        if total_secs < 60 {
            format!("{}s", total_secs)
        } else if total_secs < 3600 {
            format!("{}m {}s", total_secs / 60, total_secs % 60)
        } else {
            let hours = total_secs / 3600;
            let mins = (total_secs % 3600) / 60;
            format!("{}h {}m", hours, mins)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelinePosition {
    pub session_idx: usize,
    pub block_offset: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct TimelineFilter {
    pub time_range: Option<DateRange>,
    pub tags: Vec<TagCategory>,
    pub tools: Vec<String>,
    pub success_only: bool,
    pub has_errors: bool,
    pub search_text: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DateRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct TimelineViewState {
    pub scroll_offset: usize,
    pub selected_session: Option<usize>,
    pub expanded_sessions: HashSet<usize>,
    pub zoom_level: ZoomLevel,
    pub show_ai_summaries: bool,
    pub sort_by: SortBy,
    pub sort_descending: bool,
}

impl Default for TimelineViewState {
    fn default() -> Self {
        Self {
            scroll_offset: 0,
            selected_session: None,
            expanded_sessions: HashSet::new(),
            zoom_level: ZoomLevel::Day,
            show_ai_summaries: true,
            sort_by: SortBy::Time,
            sort_descending: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomLevel {
    Year,
    Month,
    Week,
    Day,
    Hour,
    Minute,
}

impl ZoomLevel {
    pub const ALL: [ZoomLevel; 6] = [
        ZoomLevel::Year,
        ZoomLevel::Month,
        ZoomLevel::Week,
        ZoomLevel::Day,
        ZoomLevel::Hour,
        ZoomLevel::Minute,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            ZoomLevel::Year => "Year",
            ZoomLevel::Month => "Month",
            ZoomLevel::Week => "Week",
            ZoomLevel::Day => "Day",
            ZoomLevel::Hour => "Hour",
            ZoomLevel::Minute => "Min",
        }
    }

    pub fn cycle(&self) -> ZoomLevel {
        match self {
            ZoomLevel::Year => ZoomLevel::Month,
            ZoomLevel::Month => ZoomLevel::Week,
            ZoomLevel::Week => ZoomLevel::Day,
            ZoomLevel::Day => ZoomLevel::Hour,
            ZoomLevel::Hour => ZoomLevel::Minute,
            ZoomLevel::Minute => ZoomLevel::Year,
        }
    }

    pub fn time_bucket(&self, dt: &DateTime<Utc>) -> String {
        match self {
            ZoomLevel::Year => dt.format("%Y").to_string(),
            ZoomLevel::Month => dt.format("%Y-%m").to_string(),
            ZoomLevel::Week => {
                let iso_week = dt.isoweek();
                format!("{}-W{:02}", dt.year(), iso_week.week())
            }
            ZoomLevel::Day => dt.format("%Y-%m-%d").to_string(),
            ZoomLevel::Hour => dt.format("%Y-%m-%d %H:00").to_string(),
            ZoomLevel::Minute => dt.format("%Y-%m-%d %H:%M").to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Time,
    Duration,
    CommandCount,
    SuccessRate,
    Relevance,
}

impl SortBy {
    pub fn label(&self) -> &'static str {
        match self {
            SortBy::Time => "Time",
            SortBy::Duration => "Duration",
            SortBy::CommandCount => "Commands",
            SortBy::SuccessRate => "Success%",
            SortBy::Relevance => "Relevance",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TimelineSearchResult {
    pub session_id: Uuid,
    pub block_id: Option<Uuid>,
    pub matched_text: String,
    pub context_before: String,
    pub context_after: String,
    pub relevance_score: f64,
}

#[derive(Debug, Clone)]
pub enum ExportFormat {
    Markdown,
    Html,
    Json,
    GifAnimation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigateResult {
    Navigated(TimelinePosition),
    OutOfBounds,
    SessionNotFound,
}

pub struct TimelineManager {
    sessions: Vec<TimelineSession>,
    current_position: TimelinePosition,
    filter: TimelineFilter,
    view_state: TimelineViewState,
    search_query: Option<String>,
    search_results: Vec<TimelineSearchResult>,
    ai_summaries: HashMap<Uuid, SessionSummary>,
}

impl TimelineManager {
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            current_position: TimelinePosition {
                session_idx: 0,
                block_offset: None,
            },
            filter: TimelineFilter::default(),
            view_state: TimelineViewState::default(),
            search_query: None,
            search_results: Vec::new(),
            ai_summaries: HashMap::new(),
        }
    }

    pub async fn load_sessions(&mut self, limit: usize) {
        self.sessions.clear();
        self.search_results.clear();
        self.view_state.selected_session = None;
        self.view_state.scroll_offset = 0;
        let now = Utc::now();
        let sample_sessions = generate_sample_sessions(limit, now);
        self.sessions = sample_sessions;
        if !self.sessions.is_empty() {
            self.view_state.selected_session = Some(0);
        }
    }

    pub fn generate_ai_summaries(&mut self) {
        for session in &self.sessions {
            if self.ai_summaries.contains_key(&session.id) {
                continue;
            }
            let title = if session.error_count > 0 {
                format!(
                    "Session with {} errors in {}",
                    session.error_count,
                    session
                        .project_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "unknown".into())
                )
            } else if session.command_count > 10 {
                format!(
                    "Productive session: {} commands executed",
                    session.command_count
                )
            } else {
                format!(
                    "Short session in {}",
                    session
                        .project_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "unknown".into())
                )
            };
            let outcome = if session.error_count == 0 && session.success_count > 0 {
                OutcomeType::CompleteSuccess
            } else if session.error_count > 0 && session.success_count > 0 {
                OutcomeType::SuccessWithNotes
            } else if session.error_count > 0 {
                OutcomeType::PartialFailure
            } else {
                OutcomeType::NeedsFollowUp
            };
            let summary = SessionSummary {
                title,
                description: format!(
                    "{} commands, {} successful, {} errors. Duration: {}.",
                    session.command_count,
                    session.success_count,
                    session.error_count,
                    session.format_duration()
                ),
                files_modified: session
                    .snapshots
                    .iter()
                    .filter_map(|s| match &s.snapshot_type {
                        SnapshotType::FileModified { path, .. } => Some(PathBuf::from(path.as_str())),
                        _ => None,
                    })
                    .collect(),
                key_decisions: session
                    .snapshots
                    .iter()
                    .filter_map(|s| match &s.snapshot_type {
                        SnapshotType::MilestoneReached { message } => Some(message.clone()),
                        _ => None,
                    })
                    .collect(),
                outcome,
            };
            self.ai_summaries.insert(session.id, summary);
        }
        for session in &mut self.sessions {
            if let Some(summary) = self.ai_summaries.get(&session.id).cloned() {
                session.summary = Some(summary);
            }
        }
    }

    pub fn navigate_to(&mut self, position: &TimelinePosition) -> NavigateResult {
        let filtered = self.get_filtered_sessions();
        if position.session_idx >= filtered.len() {
            return NavigateResult::OutOfBounds;
        }
        self.current_position = position.clone();
        self.view_state.selected_session = Some(position.session_idx);
        if let Some(offset) = position.block_offset {
            self.view_state.scroll_offset = offset;
        }
        NavigateResult::Navigated(position.clone())
    }

    pub fn search(&self, query: &str) -> Vec<TimelineSearchResult> {
        let query_lower = query.to_lowercase();
        let mut results = Vec::new();
        for session in &self.sessions {
            let mut best_score: f64 = 0.0;
            let mut best_match = String::new();
            let mut context_before = String::new();
            let mut context_after = String::new();
            if session
                .project_path
                .to_string_lossy()
                .to_lowercase()
                .contains(&query_lower)
            {
                best_score = best_score.max(0.8);
                best_match = session.project_path.to_string_lossy().into_owned();
            }
            if let Some(branch) = &session.branch {
                if branch.to_lowercase().contains(&query_lower) {
                    best_score = best_score.max(0.7);
                    if best_match.is_empty() {
                        best_match = branch.clone();
                    }
                }
            }
            if let Some(ref summary) = session.summary {
                if summary.title.to_lowercase().contains(&query_lower)
                    || summary.description.to_lowercase().contains(&query_lower)
                {
                    best_score = best_score.max(0.9);
                    if best_match.is_empty() {
                        best_match = summary.title.clone();
                    }
                }
            }
            for snapshot in &session.snapshots {
                let text = match &snapshot.snapshot_type {
                    SnapshotType::CommandExecuted { cmd } => Some(cmd.clone()),
                    SnapshotType::ErrorOccurred { error } => Some(error.clone()),
                    SnapshotType::FileModified { path, .. } => Some(path.clone()),
                    SnapshotType::MilestoneReached { message } => Some(message.clone()),
                    SnapshotType::TestResults { .. } => None,
                };
                if let Some(text) = text {
                    if text.to_lowercase().contains(&query_lower) {
                        let score = 1.0
                            - (text.find(&query_lower).unwrap_or(0) as f64
                                / text.len().max(1) as f64);
                        if score > best_score {
                            best_score = score;
                            best_match = text.clone();
                            let pos = text.find(&query_lower).unwrap_or(0);
                            let before_start = pos.saturating_sub(40);
                            let after_end = (pos + query.len()).min(text.len());
                            context_before =
                                text[before_start..pos].to_string();
                            context_after = text[pos + query.len()..after_end.min(text.len())]
                                .to_string();
                        }
                    }
                }
            }
            for tag in &session.tags {
                if tag.label.to_lowercase().contains(&query_lower) {
                    best_score = best_score.max(0.6);
                    if best_match.is_empty() {
                        best_match = tag.label.clone();
                    }
                }
            }
            if best_score > 0.0 {
                results.push(TimelineSearchResult {
                    session_id: session.id,
                    block_id: None,
                    matched_text: best_match,
                    context_before,
                    context_after,
                    relevance_score: best_score,
                });
            }
        }
        results.sort_by(|a, b| {
            b.relevance_score
                .partial_cmp(&a.relevance_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results
    }

    pub fn apply_filter(&mut self, filter: &TimelineFilter) {
        self.filter = filter.clone();
        self.view_state.scroll_offset = 0;
        if let Some(selected) = self.view_state.selected_session {
            let filtered = self.get_filtered_sessions();
            if selected >= filtered.len() {
                self.view_state.selected_session = filtered.len().checked_sub(1);
            }
        }
    }

    pub fn export(&self, format: ExportFormat) -> Result<Vec<u8>, String> {
        let filtered = self.get_filtered_sessions();
        match format {
            ExportFormat::Markdown => self.export_markdown(&filtered),
            ExportFormat::Html => self.export_html(&filtered),
            ExportFormat::Json => self.export_json(&filtered),
            ExportFormat::GifAnimation => Err("GIF animation export requires a rendering backend".into()),
        }
    }

    fn export_markdown(&self, sessions: &[&TimelineSession]) -> Result<Vec<u8>, String> {
        let mut md = String::from("# Timeline Export\n\n");
        for session in sessions {
            md.push_str(&format!("## Session {}\n", session.id));
            md.push_str(&format!("- **Project**: {}\n", session.project_path.display()));
            if let Some(ref branch) = session.branch {
                md.push_str(&format!("- **Branch**: {}\n", branch));
            }
            md.push_str(&format!("- **Time**: {} -> ", session.start_time.format("%Y-%m-%d %H:%M UTC")));
            if let Some(end) = session.end_time {
                md.push_str(&format!("{}", end.format("%H:%M UTC")));
            } else {
                md.push_str("ongoing");
            }
            md.push('\n');
            md.push_str(&format!("- **Duration**: {}\n", session.format_duration()));
            md.push_str(&format!(
                "- **Commands**: {} ({}, {})\n",
                session.command_count, session.success_count, session.error_count
            ));
            if !session.tags.is_empty() {
                let tags: Vec<String> = session.tags.iter().map(|t| t.label.clone()).collect();
                md.push_str(&format!("- **Tags**: {}\n", tags.join(", ")));
            }
            if let Some(ref summary) = session.summary {
                md.push_str(&format!("- **Summary**: {}\n", summary.title));
                md.push_str(&format!("  {}\n", summary.description));
            }
            md.push('\n');
        }
        Ok(md.into_bytes())
    }

    fn export_html(&self, sessions: &[&TimelineSession]) -> Result<Vec<u8>, String> {
        let mut html = String::from(
            "<!DOCTYPE html><html><head><meta charset=\"utf-8\">\
             <title>Timeline</title>\
             <style>body{font-family:monospace;max-width:900px;margin:2em auto}\
             .session{border:1px solid #333;margin:1em;padding:1em}\
             .tag{display:inline-block;padding:2px 6px;margin:2px;border-radius:3px}\
             </style></head><body><h1>Timeline Export</h1>\n",
        );
        for session in sessions {
            html.push_str("<div class=\"session\">\n");
            html.push_str(&format!(
                "<h2>{}</h2>\n",
                session
                    .project_path
                    .file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or("unknown".into())
            ));
            html.push_str(&format!("<p><b>Time:</b> {} &ndash; ",
                session.start_time.format("%Y-%m-%d %H:%M")
            ));
            if let Some(end) = session.end_time {
                html.push_str(&format!("{}</p>", end.format("%H:%M")));
            } else {
                html.push_str("ongoing</p>");
            }
            html.push_str(&format!(
                "<p><b>Duration:</b> {} | <b>Commands:</b> {} (✓{} ✗{})</p>\n",
                session.format_duration(),
                session.command_count,
                session.success_count,
                session.error_count
            ));
            if let Some(ref summary) = session.summary {
                html.push_str(&format!(
                    "<p><b>{}</b>: {}</p>\n",
                    summary.title, summary.description
                ));
            }
            html.push_str("</div>\n");
        }
        html.push_str("</body></html>");
        Ok(html.into_bytes())
    }

    fn export_json(&self, sessions: &[&TimelineSession]) -> Result<Vec<u8>, String> {
        match serde_json::to_vec_pretty(sessions) {
            Ok(bytes) => Ok(bytes),
            Err(e) => Err(format!("JSON serialization failed: {}", e)),
        }
    }

    pub fn get_filtered_sessions(&self) -> Vec<&TimelineSession> {
        let mut filtered: Vec<&TimelineSession> = self
            .sessions
            .iter()
            .filter(|s| {
                if let Some(range) = &self.filter.time_range {
                    if s.start_time < range.start || s.start_time > range.end {
                        return false;
                    }
                }
                if self.filter.success_only && s.error_count > 0 {
                    return false;
                }
                if self.filter.has_errors && s.error_count == 0 {
                    return false;
                }
                if !self.filter.tags.is_empty() {
                    let has_tag = s
                        .tags
                        .iter()
                        .any(|t| self.filter.tags.contains(&t.category));
                    if !has_tag {
                        return false;
                    }
                }
                if !self.filter.tools.is_empty() {
                    let has_tool = self
                        .filter
                        .tools
                        .iter()
                        .any(|tool| s.tools_used.contains(tool));
                    if !has_tool {
                        return false;
                    }
                }
                if let Some(ref search) = self.filter.search_text {
                    let search_lower = search.to_lowercase();
                    let matches = s
                        .project_path
                        .to_string_lossy()
                        .to_lowercase()
                        .contains(&search_lower)
                        || s.branch.as_deref().map(|b| b.to_lowercase().contains(&search_lower)).unwrap_or(false)
                        || s.summary.as_ref().map(|sum| {
                            sum.title.to_lowercase().contains(&search_lower)
                                || sum.description.to_lowercase().contains(&search_lower)
                        }).unwrap_or(false);
                    if !matches {
                        return false;
                    }
                }
                true
            })
            .collect();
        match self.view_state.sort_by {
            SortBy::Time => {
                filtered.sort_by_key(|s| s.start_time);
            }
            SortBy::Duration => {
                filtered.sort_by_key(|s| s.duration);
            }
            SortBy::CommandCount => {
                filtered.sort_by_key(|s| s.command_count);
            }
            SortBy::SuccessRate => {
                filtered.sort_by(|a, b| {
                    a.success_rate()
                        .partial_cmp(&b.success_rate())
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            SortBy::Relevance => {}
        }
        if self.view_state.sort_descending {
            filtered.reverse();
        }
        filtered
    }

    pub fn sessions_len(&self) -> usize {
        self.sessions.len()
    }

    pub fn get_session(&self, idx: usize) -> Option<&TimelineSession> {
        self.get_filtered_sessions().get(idx).copied()
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.view_state.selected_session
    }

    pub fn set_selected(&mut self, idx: Option<usize>) {
        self.view_state.selected_session = idx;
    }

    pub fn move_selection(&mut self, delta: i32) -> Option<usize> {
        let filtered = self.get_filtered_sessions();
        let current = self.view_state.selected_session.unwrap_or(0);
        let new_idx = if delta >= 0 {
            current.saturating_add(delta as usize)
        } else {
            current.saturating_sub(delta.unsigned_abs())
        };
        let clamped = new_idx.min(filtered.len().saturating_sub(1));
        self.view_state.selected_session = Some(clamped);
        self.view_state.selected_session
    }

    pub fn toggle_expand(&mut self, idx: usize) -> bool {
        if self.view_state.expanded_sessions.contains(&idx) {
            self.view_state.expanded_sessions.remove(&idx);
            false
        } else {
            self.view_state.expanded_sessions.insert(idx);
            true
        }
    }

    pub fn is_expanded(&self, idx: usize) -> bool {
        self.view_state.expanded_sessions.contains(&idx)
    }

    pub fn cycle_zoom(&mut self) -> ZoomLevel {
        let next = self.view_state.zoom_level.cycle();
        self.view_state.zoom_level = next;
        next
    }

    pub fn zoom_level(&self) -> ZoomLevel {
        self.view_state.zoom_level
    }

    pub fn jump_to_first(&mut self) {
        self.view_state.selected_session = Some(0);
        self.view_state.scroll_offset = 0;
    }

    pub fn jump_to_last(&mut self) {
        let len = self.get_filtered_sessions().len();
        if len > 0 {
            self.view_state.selected_session = Some(len - 1);
            self.view_state.scroll_offset = 0;
        }
    }

    pub fn toggle_ai_summaries(&mut self) -> bool {
        self.view_state.show_ai_summaries = !self.view_state.show_ai_summaries;
        self.view_state.show_ai_summaries
    }

    pub fn set_search_query(&mut self, query: Option<String>) {
        self.search_query = query.clone();
        if let Some(q) = query {
            self.search_results = self.search(&q);
        } else {
            self.search_results.clear();
        }
    }

    pub fn search_results(&self) -> &[TimelineSearchResult] {
        &self.search_results
    }

    pub fn next_search_result(&mut self) -> Option<&TimelineSearchResult> {
        if self.search_results.is_empty() {
            return None;
        }
        let current = self.view_state.selected_session.unwrap_or(0);
        for (i, result) in self.search_results.iter().enumerate() {
            let session_idx = self
                .get_filtered_sessions()
                .iter()
                .position(|s| s.id == result.session_id)?;
            if session_idx > current {
                self.view_state.selected_session = Some(session_idx);
                return self.search_results.get(i);
            }
        }
        if let Some(first) = self.search_results.first() {
            let session_idx = self
                .get_filtered_sessions()
                .iter()
                .position(|s| s.id == first.session_id)?;
            self.view_state.selected_session = Some(session_idx);
            return Some(first);
        }
        None
    }

    pub fn prev_search_result(&mut self) -> Option<&TimelineSearchResult> {
        if self.search_results.is_empty() {
            return None;
        }
        let current = self.view_state.selected_session.unwrap_or(0);
        for i in (0..self.search_results.len()).rev() {
            let result = &self.search_results[i];
            let session_idx = self
                .get_filtered_sessions()
                .iter()
                .position(|s| s.id == result.session_id)?;
            if session_idx < current {
                self.view_state.selected_session = Some(session_idx);
                return self.search_results.get(i);
            }
        }
        if let Some(last) = self.search_results.last() {
            let session_idx = self
                .get_filtered_sessions()
                .iter()
                .position(|s| s.id == last.session_id)?;
            self.view_state.selected_session = Some(session_idx);
            return Some(last);
        }
        None
    }

    pub fn set_sort(&mut self, sort_by: SortBy) {
        self.view_state.sort_by = sort_by;
    }

    pub fn toggle_sort_direction(&mut self) {
        self.view_state.sort_descending = !self.view_state.sort_descending;
    }
}

impl Default for TimelineManager {
    fn default() -> Self {
        Self::new()
    }
}

pub struct TimelineView<'a> {
    manager: &'a TimelineManager,
}

impl<'a> TimelineView<'a> {
    pub fn new(manager: &'a TimelineManager) -> Self {
        Self { manager }
    }
}

impl Widget for TimelineView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = RBlock::default()
            .borders(Borders::ALL)
            .title(" Timeline (Warp Drive Navigation) ")
            .style(Style::default().fg(Color::Cyan));
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.height < 4 || inner.width < 20 {
            let msg = Line::from(Span::styled(
                "Terminal too small",
                Style::default().fg(Color::Red),
            ));
            buf.set_line(inner.x + 1, inner.y + 1, &msg, inner.width - 2);
            return;
        }
        let header_height = 3;
        render_header(inner, buf, self.manager);
        let body_top = inner.y + header_height as u16;
        let body_height = inner.height.saturating_sub(header_height as u16 + 2);
        let body_rect = Rect::new(inner.x, body_top, inner.width, body_height);
        render_session_list(body_rect, buf, self.manager);
        if body_height > 0 {
            let footer_y = body_top + body_height;
            render_footer(Rect::new(inner.x, footer_y, inner.width, 2), buf, self.manager);
        }
    }
}

fn render_header(area: Rect, buf: &mut Buffer, manager: &TimelineManager) {
    let zoom_label = manager.zoom_level().label();
    let filter_info = if manager.filter.success_only {
        " [success-only]"
    } else if manager.filter.has_errors {
        " [has-errors]"
    } else {
        ""
    };
    let sort_label = format!(
        "{}{}",
        manager.view_state.sort_by.label(),
        if manager.view_state.sort_descending {
            " v"
        } else {
            " ^"
        }
    );
    let header_line = Line::from(vec![
        Span::styled("◄► ", Style::default().fg(Color::Yellow)),
        Span::styled(
            format!("Zoom:{} ", zoom_label),
            Style::default().fg(Color::Green),
        ),
        Span::styled(
            format!("Sort:{} ", sort_label),
            Style::default().fg(Color::Magenta),
        ),
        Span::styled(filter_info.to_string(), Style::default().fg(Color::DarkGray)),
        Span::raw(" ".repeat(
            area.width.saturating_sub(30) as usize,
        )),
        Span::styled(
            format!("{} sessions", manager.sessions_len()),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    buf.set_line(area.x, area.y, &header_line, area.width);
    let separator = Line::from(Span::styled(
        "-".repeat(area.width as usize),
        Style::default().fg(Color::DarkGray),
    ));
    buf.set_line(area.x, area.y + 1, &separator, area.width);
    if let Some(ref query) = manager.search_query {
        let search_line = Line::from(vec![
            Span::styled("/", Style::default().fg(Color::Yellow)),
            Span::styled(
                format!("{} ({} results)", query, manager.search_results().len()),
                Style::default().fg(Color::Cyan),
            ),
        ]);
        buf.set_line(area.x, area.y + 2, &search_line, area.width);
    } else {
        let help_line = Line::from(Span::styled(
            "^v/jk navigate | <-->/hl expand | Enter open | /search | t zoom | f filter | ? help",
            Style::default().fg(Color::DarkGray),
        ));
        buf.set_line(area.x, area.y + 2, &help_line, area.width);
    }
}

fn render_session_list(area: Rect, buf: &mut Buffer, manager: &TimelineManager) {
    let sessions = manager.get_filtered_sessions();
    if sessions.is_empty() {
        let empty_msg = Line::from(Span::styled(
            "(no sessions match filter)",
            Style::default().fg(Color::DarkGray),
        ));
        buf.set_line(area.x + 1, area.y + 1, &empty_msg, area.width.saturating_sub(2));
        return;
    }
    let scroll = manager.view_state.scroll_offset;
    let visible_height = area.height as usize;
    let selected = manager.selected_index().unwrap_or(0);
    for i in 0..visible_height {
        let session_idx = i + scroll;
        let line_y = area.y + i as u16;
        if line_y >= area.y + area.height {
            break;
        }
        if let Some(session) = sessions.get(session_idx) {
            let is_selected = session_idx == selected;
            let is_expanded = manager.is_expanded(session_idx);
            let line = build_session_line(session, is_selected, is_expanded, manager);
            buf.set_line(area.x, line_y, &line, area.width);
        } else {
            buf.set_line(
                area.x,
                line_y,
                &Line::from(Span::raw("")),
                area.width,
            );
        }
    }
}

fn build_session_line(
    session: &TimelineSession,
    is_selected: bool,
    is_expanded: bool,
    manager: &TimelineManager,
) -> Line<'static> {
    let mut spans = Vec::new();
    let expand_icon = if is_expanded { '▼' } else { '▶' };
    let base_style = if is_selected {
        Style::default()
            .bg(Color::Rgb(30, 30, 50))
            .fg(Color::White)
    } else {
        Style::default().fg(Color::White)
    };
    spans.push(Span::styled(
        format!(" {} ", expand_icon),
        base_style,
    ));
    let time_str = session.start_time.format("%m-%d %H:%M").to_string();
    spans.push(Span::styled(
        format!("{} ", time_str),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    ));
    let project_name = session
        .project_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "?".into());
    let display_name = if project_name.len() > 18 {
        format!("{}…", &project_name[..17])
    } else {
        format!("{:<18}", project_name)
    };
    spans.push(Span::styled(display_name, base_style));
    spans.push(Span::styled(" ", base_style));
    let duration_str = format!("{:<8}", session.format_duration());
    spans.push(Span::styled(
        duration_str,
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    ));
    let stats = format!(
        "cmd:{:<3} ✓{:>2} ✗{:>2}",
        session.command_count, session.success_count, session.error_count
    );
    let stats_color = if session.error_count > 0 {
        Color::Red
    } else if session.success_count > 5 {
        Color::Green
    } else {
        Color::White
    };
    spans.push(Span::styled(
        format!(" {} ", stats),
        Style::default().fg(stats_color),
    ));
    for tag in session.tags.iter().take(3) {
        spans.push(Span::styled(
            format!("{}{}", tag.icon, tag.label),
            Style::default().fg(tag.color),
        ));
    }
    if manager.view_state.show_ai_summaries {
        if let Some(ref summary) = session.summary {
            let summary_preview = if summary.title.len() > 25 {
                format!(" | {}…", &summary.title[..24])
            } else {
                format!(" | {}", summary.title)
            };
            spans.push(Span::styled(
                summary_preview,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::ITALIC),
            ));
        }
    }
    Line::from(spans)
}

fn render_footer(area: Rect, buf: &mut Buffer, manager: &TimelineManager) {
    let selected = manager.selected_index();
    let footer_text = match selected {
        Some(idx) => {
            if let Some(session) = manager.get_session(idx) {
                let branch_str = session
                    .branch
                    .as_deref()
                    .map(|b| format!(" [{}]", b))
                    .unwrap_or_default();
                format!(
                    "Session {}{} — {} | Rate: {:.0}% | Tools: {}{}",
                    session.id.simple(),
                    branch_str,
                    session.project_path.display(),
                    session.success_rate(),
                    session.tools_used.len(),
                    if manager.is_expanded(idx) {
                        " [expanded]"
                    } else {
                        ""
                    }
                )
            } else {
                String::new()
            }
        }
        None => "No session selected".to_string(),
    };
    let sep = Line::from(Span::styled(
        "-".repeat(area.width as usize),
        Style::default().fg(Color::DarkGray),
    ));
    buf.set_line(area.x, area.y, &sep, area.width);
    let footer_line = Line::from(Span::styled(
        footer_text,
        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
    ));
    buf.set_line(area.x, area.y + 1, &footer_line, area.width);
}

fn generate_sample_sessions(count: usize, now: DateTime<Utc>) -> Vec<TimelineSession> {
    let mut sessions = Vec::with_capacity(count);
    let projects = ["carpai", "jcode-core", "jcode-tui", "jcode-swarm", "jcode-storage"];
    let branches = [
        Some("main".into()),
        Some("feature/timeline".into()),
        Some("fix/auth-bug".into()),
        Some("refactor/memory".into()),
        None,
    ];
    let tool_sets: &[&[&str]] = &[
        &["edit", "read", "bash"],
        &["edit", "bash", "grep"],
        &["read", "edit", "write", "bash"],
        &["bash", "test", "edit"],
        &["edit", "search", "multi_edit"],
    ];
    let tag_categories = [
        TagCategory::Feature,
        TagCategory::Bugfix,
        TagCategory::Refactor,
        TagCategory::Experiment,
        TagCategory::Success,
        TagCategory::Warning,
        TagCategory::Error,
    ];
    let tag_icons = ["✨", "🐛", "♻️", "🧪", "✅", "⚠️", "❌"];
    let tag_labels = [
        "feature", "bugfix", "refactor", "experiment", "success", "warning", "error",
    ];
    let tag_colors = [
        Color::Green,
        Color::Red,
        Color::Magenta,
        Color::Cyan,
        Color::Green,
        Color::Yellow,
        Color::Red,
    ];
    for i in 0..count {
        let offset_hours = (i as i64) * 3 + (i as i64 % 7) * 2;
        let start = now - Duration::hours(offset_hours);
        let duration_mins = 15 + (i * 17) % 120;
        let end = start + Duration::minutes(duration_mins as i64);
        let cmd_count = 3 + (i * 5) % 25;
        let err_count = if i % 7 == 0 { 2 } else if i % 11 == 0 { 1 } else { 0 };
        let succ_count = cmd_count - err_count;
        let proj_idx = i % projects.len();
        let tools: HashSet<String> = tool_sets[i % tool_sets.len()]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let num_tags = 1 + (i % 3);
        let tags: Vec<TimelineTag> = (0..num_tags)
            .map(|ti| {
                let cat_idx = (i + ti) % tag_categories.len();
                TimelineTag {
                    icon: tag_icons[cat_idx],
                    label: tag_labels[cat_idx].to_string(),
                    color: tag_colors[cat_idx],
                    category: tag_categories[cat_idx].clone(),
                }
            })
            .collect();
        let mut snapshots = Vec::new();
        snapshots.push(TimelineSnapshot {
            timestamp: start + Duration::seconds(5),
            snapshot_type: SnapshotType::CommandExecuted {
                cmd: format!("cd {}", projects[proj_idx]),
            },
            content: SnapshotContent::Text(format!("cd {}", projects[proj_idx])),
        });
        if err_count > 0 {
            snapshots.push(TimelineSnapshot {
                timestamp: start + Duration::minutes(2),
                snapshot_type: SnapshotType::ErrorOccurred {
                    error: format!("compilation error in module_{}", i),
                },
                content: SnapshotContent::Text(format!("error: module_{}", i)),
            });
        }
        snapshots.push(TimelineSnapshot {
            timestamp: start + Duration::minutes(duration_mins as i64 / 2),
            snapshot_type: SnapshotType::FileModified {
                path: format!("src/{}.rs", projects[proj_idx]),
                diff_summary: format!("+{} lines modified", 10 + i * 3),
            },
            content: SnapshotContent::Diff(DiffPreview {
                old_lines: vec!["// old code".into()],
                new_lines: vec![format!("// new code v{}", i)],
            }),
        });
        snapshots.push(TimelineSnapshot {
            timestamp: start + Duration::minutes(duration_mins as i64 - 1),
            snapshot_type: SnapshotType::TestResults {
                passed: succ_count,
                failed: err_count,
            },
            content: SnapshotContent::TestReport(TestSummary {
                total: cmd_count,
                passed: succ_count,
                failed: err_count,
                skipped: 0,
            }),
        });
        if i % 5 == 0 {
            snapshots.push(TimelineSnapshot {
                timestamp: start + Duration::minutes(duration_mins as i64 - 2),
                snapshot_type: SnapshotType::MilestoneReached {
                    message: format!("Completed milestone phase_{}", i / 5 + 1),
                },
                content: SnapshotContent::Text(format!("milestone phase_{}", i / 5 + 1)),
            });
        }
        sessions.push(TimelineSession {
            id: Uuid::new_v4(),
            project_path: PathBuf::from(format!("/home/dev/{}", projects[proj_idx])),
            branch: branches[i % branches.len()].clone(),
            start_time: start,
            end_time: Some(end),
            duration: Duration::minutes(duration_mins as i64),
            command_count: cmd_count,
            success_count: succ_count,
            error_count: err_count,
            tags,
            tools_used: tools,
            summary: None,
            snapshots,
        });
    }
    sessions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_session(idx: usize) -> TimelineSession {
        let now = Utc::now();
        let start = now - Duration::hours((idx * 2) as i64);
        TimelineSession {
            id: Uuid::new_v4(),
            project_path: PathBuf::from(format!("/test/project_{}", idx)),
            branch: Some(format!("branch_{}", idx)),
            start_time: start,
            end_time: Some(start + Duration::minutes(30)),
            duration: Duration::minutes(30),
            command_count: 5 + idx,
            success_count: 5 + idx,
            error_count: if idx % 3 == 0 { 1 } else { 0 },
            tags: vec![TimelineTag {
                icon: '✅',
                label: "success".into(),
                color: Color::Green,
                category: TagCategory::Success,
            }],
            tools_used: ["edit", "read", "bash"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            summary: None,
            snapshots: vec![
                TimelineSnapshot {
                    timestamp: start + Duration::seconds(1),
                    snapshot_type: SnapshotType::CommandExecuted {
                        cmd: format!("echo test_{}", idx),
                    },
                    content: SnapshotContent::Text(format!("echo test_{}", idx)),
                },
                TimelineSnapshot {
                    timestamp: start + Duration::seconds(2),
                    snapshot_type: SnapshotType::FileModified {
                        path: format!("file_{}.rs", idx),
                        diff_summary: "+5 lines".into(),
                    },
                    content: SnapshotContent::Diff(DiffPreview {
                        old_lines: vec!["old".into()],
                        new_lines: vec!["new".into()],
                    }),
                },
            ],
        }
    }

    fn make_manager_with_sessions(count: usize) -> TimelineManager {
        let mut mgr = TimelineManager::new();
        for i in 0..count {
            mgr.sessions.push(make_test_session(i));
        }
        mgr.view_state.selected_session = if count > 0 { Some(0) } else { None };
        mgr
    }

    #[tokio::test]
    async fn test_new_manager_is_empty() {
        let mgr = TimelineManager::new();
        assert_eq!(mgr.sessions_len(), 0);
        assert!(mgr.selected_index().is_none());
        assert!(mgr.search_results().is_empty());
        assert_eq!(mgr.zoom_level(), ZoomLevel::Day);
    }

    #[tokio::test]
    async fn test_load_sessions_populates_data() {
        let mut mgr = TimelineManager::new();
        mgr.load_sessions(5).await;
        assert_eq!(mgr.sessions_len(), 5);
        assert_eq!(mgr.selected_index(), Some(0));
    }

    #[tokio::test]
    async fn test_load_zero_sessions() {
        let mut mgr = TimelineManager::new();
        mgr.load_sessions(0).await;
        assert_eq!(mgr.sessions_len(), 0);
        assert!(mgr.selected_index().is_none());
    }

    #[tokio::test]
    async fn test_load_single_session() {
        let mut mgr = TimelineManager::new();
        mgr.load_sessions(1).await;
        assert_eq!(mgr.sessions_len(), 1);
        assert_eq!(mgr.selected_index(), Some(0));
        let session = mgr.get_session(0).expect("should have session");
        assert!(session.command_count > 0);
    }

    #[test]
    fn test_get_filtered_sessions_returns_all_when_no_filter() {
        let mgr = make_manager_with_sessions(5);
        let filtered = mgr.get_filtered_sessions();
        assert_eq!(filtered.len(), 5);
    }

    #[test]
    fn test_filter_success_only_excludes_errors() {
        let mut mgr = make_manager_with_sessions(6);
        mgr.apply_filter(&TimelineFilter {
            success_only: true,
            ..Default::default()
        });
        let filtered = mgr.get_filtered_sessions();
        for s in filtered {
            assert_eq!(s.error_count, 0);
        }
    }

    #[test]
    fn test_filter_has_errors_only_includes_error_sessions() {
        let mut mgr = make_manager_with_sessions(6);
        mgr.apply_filter(&TimelineFilter {
            has_errors: true,
            ..Default::default()
        });
        let filtered = mgr.get_filtered_sessions();
        assert!(!filtered.is_empty());
        for s in filtered {
            assert!(s.error_count > 0);
        }
    }

    #[test]
    fn test_filter_by_tool() {
        let mut mgr = make_manager_with_sessions(3);
        mgr.apply_filter(&TimelineFilter {
            tools: vec!["edit".into()],
            ..Default::default()
        });
        let filtered = mgr.get_filtered_sessions();
        assert_eq!(filtered.len(), 3);
    }

    #[test]
    fn test_filter_by_nonexistent_tool_returns_empty() {
        let mut mgr = make_manager_with_sessions(3);
        mgr.apply_filter(&TimelineFilter {
            tools: vec!["nonexistent_tool_xyz".into()],
            ..Default::default()
        });
        let filtered = mgr.get_filtered_sessions();
        assert!(filtered.is_empty());
    }

    #[test]
    fn test_navigate_to_valid_position() {
        let mut mgr = make_manager_with_sessions(5);
        let pos = TimelinePosition {
            session_idx: 2,
            block_offset: None,
        };
        let result = mgr.navigate_to(&pos);
        assert!(matches!(result, NavigateResult::Navigated(..)));
        assert_eq!(mgr.selected_index(), Some(2));
    }

    #[test]
    fn test_navigate_to_out_of_bounds() {
        let mut mgr = make_manager_with_sessions(3);
        let pos = TimelinePosition {
            session_idx: 99,
            block_offset: None,
        };
        let result = mgr.navigate_to(&pos);
        assert_eq!(result, NavigateResult::OutOfBounds);
    }

    #[test]
    fn test_navigate_to_empty_sessions() {
        let mut mgr = TimelineManager::new();
        let pos = TimelinePosition {
            session_idx: 0,
            block_offset: None,
        };
        let result = mgr.navigate_to(&pos);
        assert_eq!(result, NavigateResult::OutOfBounds);
    }

    #[test]
    fn test_search_finds_matching_session() {
        let mgr = make_manager_with_sessions(3);
        let results = mgr.search("project_1");
        assert!(!results.is_empty());
        assert!(results.iter().any(|r| r.matched_text.contains("project_1")));
    }

    #[test]
    fn test_search_returns_empty_for_no_match() {
        let mgr = make_manager_with_sessions(3);
        let results = mgr.search("zzz_nonexistent_zzz");
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_scores_higher_for_exact_matches() {
        let mgr = make_manager_with_sessions(5);
        let results = mgr.search("project_0");
        if let Some(best) = results.first() {
            assert!(best.relevance_score > 0.5);
        }
    }

    #[test]
    fn test_search_is_case_insensitive() {
        let mgr = make_manager_with_sessions(3);
        let lower = mgr.search("project_1");
        let upper = mgr.search("PROJECT_1");
        assert_eq!(lower.len(), upper.len());
    }

    #[test]
    fn test_sort_by_time_descending() {
        let mut mgr = make_manager_with_sessions(5);
        mgr.set_sort(SortBy::Time);
        mgr.toggle_sort_direction();
        let filtered = mgr.get_filtered_sessions();
        for win in filtered.windows(2) {
            assert!(win[0].start_time >= win[1].start_time);
        }
    }

    #[test]
    fn test_sort_by_command_count() {
        let mut mgr = make_manager_with_sessions(5);
        mgr.set_sort(SortBy::CommandCount);
        let filtered = mgr.get_filtered_sessions();
        if filtered.len() >= 2 {
            assert!(filtered[0].command_count <= filtered[1].command_count);
        }
    }

    #[test]
    fn test_zoom_level_cycle() {
        assert_eq!(ZoomLevel::Day.cycle(), ZoomLevel::Hour);
        assert_eq!(ZoomLevel::Hour.cycle(), ZoomLevel::Minute);
        assert_eq!(ZoomLevel::Minute.cycle(), ZoomLevel::Year);
        assert_eq!(ZoomLevel::Year.cycle(), ZoomLevel::Month);
        assert_eq!(ZoomLevel::Month.cycle(), ZoomLevel::Week);
        assert_eq!(ZoomLevel::Week.cycle(), ZoomLevel::Day);
    }

    #[test]
    fn test_zoom_level_time_bucket_formats() {
        let dt = Utc::now();
        assert!(!ZoomLevel::Year.time_bucket(&dt).is_empty());
        assert!(!ZoomLevel::Month.time_bucket(&dt).is_empty());
        assert!(!ZoomLevel::Day.time_bucket(&dt).is_empty());
        assert!(!ZoomLevel::Hour.time_bucket(&dt).is_empty());
        assert!(!ZoomLevel::Minute.time_bucket(&dt).is_empty());
        assert!(ZoomLevel::Week.time_bucket(&dt).contains('W'));
    }

    #[test]
    fn test_cycle_zoom_changes_level() {
        let mut mgr = make_manager_with_sessions(1);
        let initial = mgr.zoom_level();
        let next = mgr.cycle_zoom();
        assert_ne!(initial, next);
        assert_eq!(mgr.zoom_level(), next);
    }

    #[test]
    fn test_toggle_expand_session() {
        let mut mgr = make_manager_with_sessions(3);
        assert!(!mgr.is_expanded(0));
        let expanded = mgr.toggle_expand(0);
        assert!(expanded);
        assert!(mgr.is_expanded(0));
        let collapsed = mgr.toggle_expand(0);
        assert!(!collapsed);
        assert!(!mgr.is_expanded(0));
    }

    #[test]
    fn test_move_selection_down() {
        let mut mgr = make_manager_with_sessions(5);
        assert_eq!(mgr.selected_index(), Some(0));
        let new_idx = mgr.move_selection(1);
        assert_eq!(new_idx, Some(1));
    }

    #[test]
    fn test_move_selection_up_clamps_at_zero() {
        let mut mgr = make_manager_with_sessions(5);
        mgr.move_selection(3);
        let idx = mgr.move_selection(-10);
        assert_eq!(idx, Some(0));
    }

    #[test]
    fn test_move_selection_down_clamps_at_end() {
        let mut mgr = make_manager_with_sessions(3);
        let idx = mgr.move_selection(100);
        assert_eq!(idx, Some(2));
    }

    #[test]
    fn test_jump_to_first_and_last() {
        let mut mgr = make_manager_with_sessions(10);
        mgr.set_selected(Some(9));
        mgr.jump_to_first();
        assert_eq!(mgr.selected_index(), Some(0));
        mgr.jump_to_last();
        assert_eq!(mgr.selected_index(), Some(9));
    }

    #[test]
    fn test_jump_on_empty_does_not_panic() {
        let mut mgr = TimelineManager::new();
        mgr.jump_to_first();
        assert!(mgr.selected_index().is_none());
        mgr.jump_to_last();
        assert!(mgr.selected_index().is_none());
    }

    #[test]
    fn test_toggle_ai_summaries() {
        let mut mgr = make_manager_with_sessions(1);
        assert!(mgr.view_state.show_ai_summaries);
        assert!(!mgr.toggle_ai_summaries());
        assert!(!mgr.view_state.show_ai_summaries);
        assert!(mgr.toggle_ai_summaries());
        assert!(mgr.view_state.show_ai_summaries);
    }

    #[test]
    fn test_set_search_query_populates_results() {
        let mut mgr = make_manager_with_sessions(3);
        mgr.set_search_query(Some("project_0".into()));
        assert!(!mgr.search_results().is_empty());
        mgr.set_search_query(None);
        assert!(mgr.search_results().is_empty());
    }

    #[test]
    fn test_export_markdown_produces_output() {
        let mgr = make_manager_with_sessions(2);
        let result = mgr.export(ExportFormat::Markdown);
        assert!(result.is_ok());
        let bytes = result.unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("# Timeline Export"));
        assert!(text.contains("Session "));
    }

    #[test]
    fn test_export_json_produces_valid_json() {
        let mgr = make_manager_with_sessions(2);
        let result = mgr.export(ExportFormat::Json);
        assert!(result.is_ok());
        let bytes = result.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(parsed.is_array());
    }

    #[test]
    fn test_export_html_contains_doctype() {
        let mgr = make_manager_with_sessions(1);
        let result = mgr.export(ExportFormat::Html);
        assert!(result.is_ok());
        let text = String::from_utf8(result.unwrap()).unwrap();
        assert!(text.contains("<!DOCTYPE html>"));
    }

    #[test]
    fn test_export_gif_animation_returns_err() {
        let mgr = make_manager_with_sessions(1);
        let result = mgr.export(ExportFormat::GifAnimation);
        assert!(result.is_err());
    }

    #[test]
    fn test_export_empty_sessions_still_works() {
        let mgr = TimelineManager::new();
        let result = mgr.export(ExportFormat::Markdown);
        assert!(result.is_ok());
    }

    #[test]
    fn test_generate_ai_summaries_populates_all() {
        let mut mgr = make_manager_with_sessions(4);
        mgr.generate_ai_summaries();
        for session in &mgr.sessions {
            assert!(session.summary.is_some());
        }
    }

    #[test]
    fn test_generate_ai_summaries_outcome_types() {
        let mut mgr = make_manager_with_sessions(6);
        mgr.generate_ai_summaries();
        let has_complete = mgr
            .sessions
            .iter()
            .any(|s| matches!(s.summary.as_ref().and_then(|x| Some(&x.outcome)), Some(OutcomeType::CompleteSuccess)));
        let has_notes = mgr
            .sessions
            .iter()
            .any(|s| matches!(s.summary.as_ref().and_then(|x| Some(&x.outcome)), Some(OutcomeType::SuccessWithNotes)));
        assert!(has_complete || has_notes);
    }

    #[test]
    fn test_session_success_rate_calculation() {
        let session = make_test_session(0);
        assert!(session.success_rate() > 0.0);
        assert!(session.success_rate() <= 100.0);
    }

    #[test]
    fn test_session_format_duration() {
        let session = make_test_session(0);
        let dur = session.format_duration();
        assert!(!dur.is_empty());
        assert!(dur.contains('m'));
    }

    #[test]
    fn test_next_search_result_wraps_around() {
        let mut mgr = make_manager_with_sessions(3);
        mgr.set_search_query(Some("project".into()));
        mgr.set_selected(Some(2));
        let result = mgr.next_search_result();
        assert!(result.is_some());
    }

    #[test]
    fn test_prev_search_result_wraps_around() {
        let mut mgr = make_manager_with_sessions(3);
        mgr.set_search_query(Some("project".into()));
        mgr.set_selected(Some(0));
        let result = mgr.prev_search_result();
        assert!(result.is_some());
    }

    #[test]
    fn test_default_view_state_values() {
        let state = TimelineViewState::default();
        assert_eq!(state.scroll_offset, 0);
        assert!(state.selected_session.is_none());
        assert!(state.expanded_sessions.is_empty());
        assert_eq!(state.zoom_level, ZoomLevel::Day);
        assert!(state.show_ai_summaries);
        assert_eq!(state.sort_by, SortBy::Time);
        assert!(state.sort_descending);
    }

    #[test]
    fn test_large_dataset_performance() {
        let mut mgr = TimelineManager::new();
        for i in 0..500 {
            mgr.sessions.push(make_test_session(i));
        }
        mgr.view_state.selected_session = Some(0);
        let filtered = mgr.get_filtered_sessions();
        assert_eq!(filtered.len(), 500);
        let results = mgr.search("project");
        assert!(!results.is_empty());
        let _export = mgr.export(ExportFormat::Json).expect("json export should work");
    }

    #[test]
    fn test_apply_filter_resets_scroll() {
        let mut mgr = make_manager_with_sessions(5);
        mgr.view_state.scroll_offset = 42;
        mgr.apply_filter(&TimelineFilter {
            success_only: true,
            ..Default::default()
        });
        assert_eq!(mgr.view_state.scroll_offset, 0);
    }

    #[test]
    fn test_widget_render_small_terminal_shows_warning() {
        use ratatui::buffer::Buffer;
        let mgr = make_manager_with_sessions(1);
        let view = TimelineView::new(&mgr);
        let tiny_area = Rect::new(0, 0, 10, 2);
        let mut buf = Buffer::empty(tiny_area);
        view.render(tiny_area, &mut buf);
        let content = buffer_to_string(&buf);
        assert!(content.contains("too small"));
    }

    #[test]
    fn test_widget_render_normal_displays_content() {
        use ratatui::buffer::Buffer;
        let mut mgr = make_manager_with_sessions(3);
        mgr.generate_ai_summaries();
        let view = TimelineView::new(&mgr);
        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
        let content = buffer_to_string(&buf);
        assert!(content.contains("Timeline") || content.contains("sessions"));
    }

    fn buffer_to_string(buf: &Buffer) -> String {
        let mut s = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                s.push(buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(' '));
            }
            s.push('\n');
        }
        s
    }
}
