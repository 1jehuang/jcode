//! Compact single-line status bar at the bottom of the TUI.
//!
//! Shows workspace name, repo name, active agent count, and pending task count.
//! Hidden when terminal width < 80 columns.

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use super::color_support::rgb;
use super::TuiState;

/// Minimum terminal width to show the status bar.
const MIN_WIDTH: u16 = 80;

/// Data snapshot for one frame, extracted from TuiState.
pub(crate) struct StatusBarData {
    pub workspace_name: Option<String>,
    pub repo_name: Option<String>,
    pub active_agents: u32,
    pub pending_tasks: u32,
    pub session_name: Option<String>,
}

impl StatusBarData {
    /// Extract status bar data from the current TuiState.
    pub fn from_state(app: &dyn TuiState) -> Self {
        let workspace_name = extract_workspace_name(app);
        let repo_name = extract_repo_name(app);
        let stats = app.swarm_stats();
        let active_agents = stats.active;
        // Pending tasks = queued messages count
        let pending_tasks = app.queued_messages().len() as u32;
        let session_name = app.session_display_name();

        Self {
            workspace_name,
            repo_name,
            active_agents,
            pending_tasks,
            session_name,
        }
    }
}

/// Render the compact status bar into `area`.
///
/// Returns `true` if the bar was rendered, `false` if the area was too narrow
/// or the data was empty.
pub(crate) fn render_status_bar(frame: &mut Frame, app: &dyn TuiState, area: Rect) -> bool {
    if area.width < MIN_WIDTH {
        return false;
    }

    let data = StatusBarData::from_state(app);
    let line = build_status_line(&data, area.width as usize);
    frame.render_widget(Paragraph::new(line), area);
    true
}

/// Build the single-line content for the status bar.
fn build_status_line(data: &StatusBarData, width: usize) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let dim = jcode_tui_style::theme::dim_color();
    let separator_color = rgb(60, 60, 60);

    // Left side: workspace + repo
    if let Some(ref ws) = data.workspace_name {
        spans.push(Span::styled(
            format!(" {} ", ws),
            Style::default().fg(rgb(100, 160, 220)),
        ));
    }

    if let Some(ref repo) = data.repo_name {
        if spans.is_empty() {
            // No workspace, just show repo
            spans.push(Span::styled(
                format!(" {} ", repo),
                Style::default().fg(rgb(100, 160, 220)),
            ));
        } else {
            spans.push(Span::styled(" \u{2502} ", Style::default().fg(separator_color)));
            spans.push(Span::styled(
                format!("{} ", repo),
                Style::default().fg(dim),
            ));
        }
    }

    // Center: session name
    if let Some(ref session) = data.session_name {
        if spans.is_empty() {
            spans.push(Span::styled(
                format!(" {} ", session),
                Style::default().fg(dim),
            ));
        }
    }

    // Right side: agent stats
    let mut right_spans: Vec<Span<'static>> = Vec::new();

    if data.active_agents > 0 {
        if !right_spans.is_empty() {
            right_spans.push(Span::styled(" ", Style::default()));
        }
        right_spans.push(Span::styled(
            format!("\u{25b8} {} active", data.active_agents),
            Style::default().fg(rgb(0, 200, 0)),
        ));
    }

    if data.pending_tasks > 0 {
        if !right_spans.is_empty() {
            right_spans.push(Span::styled(" ", Style::default()));
        }
        right_spans.push(Span::styled(
            format!("\u{25cf} {} pending", data.pending_tasks),
            Style::default().fg(rgb(255, 193, 7)),
        ));
    }

    // If nothing to show, return empty
    if spans.is_empty() && right_spans.is_empty() {
        return Line::from("");
    }

    // Pad to fill width
    let left_width: usize = spans.iter().map(|s| s.content.len()).sum();
    let right_width: usize = right_spans.iter().map(|s| s.content.len()).sum();
    let used_width = left_width + right_width;

    if right_spans.is_empty() {
        // Left-aligned only
        Line::from(spans)
    } else if left_width == 0 {
        // Right-aligned only
        let padding = width.saturating_sub(right_width);
        let mut result = Vec::new();
        if padding > 0 {
            result.push(Span::raw(" ".repeat(padding)));
        }
        result.extend(right_spans);
        Line::from(result)
    } else {
        // Both sides: pad the middle
        let padding = width.saturating_sub(used_width);
        let mut result = spans;
        if padding > 0 {
            result.push(Span::raw(" ".repeat(padding)));
        }
        result.extend(right_spans);
        Line::from(result)
    }
}

/// Extract a short workspace name from the TuiState.
///
/// Prefers the workspace map's current workspace number; falls back to the
/// working directory basename when workspace mode is off.
fn extract_workspace_name(app: &dyn TuiState) -> Option<String> {
    if app.workspace_mode_enabled() {
        let rows = app.workspace_map_rows();
        if let Some(current) = rows.iter().find(|r| r.is_current) {
            return Some(format!("ws:{}", current.workspace));
        }
        if let Some(first) = rows.first() {
            return Some(format!("ws:{}", first.workspace));
        }
    }
    // Fallback: derive from working directory
    app.working_dir().and_then(|dir| {
        std::path::Path::new(&dir)
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
    })
}

/// Extract a short repo name from the working directory.
///
/// Returns the basename of the working directory as a shorthand repo label.
fn extract_repo_name(app: &dyn TuiState) -> Option<String> {
    app.working_dir().and_then(|dir| {
        let path = std::path::Path::new(&dir);
        path.file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
    })
}
