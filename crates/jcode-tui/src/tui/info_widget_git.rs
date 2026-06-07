use super::text::truncate_smart;
use super::{GitInfo, InfoWidgetData};
use crate::tui::color_support::rgb;
use ratatui::prelude::*;

pub(super) fn render_git_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(info) = &data.git_info else {
        return Vec::new();
    };
    if !info.is_interesting() {
        return Vec::new();
    }

    let w = inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();

    let mut parts: Vec<Span> = Vec::new();
    parts.push(Span::styled(" ", Style::default().fg(rgb(240, 160, 60))));

    let mut stats_len = 0usize;
    if info.ahead > 0 {
        stats_len += format!(" ↑{}", info.ahead).chars().count();
    }
    if info.behind > 0 {
        stats_len += format!(" ↓{}", info.behind).chars().count();
    }
    if info.modified > 0 {
        stats_len += format!(" ~{}", info.modified).chars().count();
    }
    if info.staged > 0 {
        stats_len += format!(" +{}", info.staged).chars().count();
    }
    if info.untracked > 0 {
        stats_len += format!(" ?{}", info.untracked).chars().count();
    }

    let branch_max = w.saturating_sub(2 + stats_len).max(4);
    let branch_display = truncate_smart(&info.branch, branch_max);
    parts.push(Span::styled(
        branch_display,
        Style::default()
            .fg(rgb(200, 200, 210))
            .add_modifier(Modifier::BOLD),
    ));

    if info.modified > 0 {
        parts.push(Span::styled(
            format!(" ~{}", info.modified),
            Style::default().fg(rgb(240, 200, 80)),
        ));
    }
    if info.staged > 0 {
        parts.push(Span::styled(
            format!(" +{}", info.staged),
            Style::default().fg(rgb(100, 200, 100)),
        ));
    }
    if info.untracked > 0 {
        parts.push(Span::styled(
            format!(" ?{}", info.untracked),
            Style::default().fg(rgb(140, 140, 150)),
        ));
    }
    if info.ahead > 0 {
        parts.push(Span::styled(
            format!(" ↑{}", info.ahead),
            Style::default().fg(rgb(100, 200, 100)),
        ));
    }
    if info.behind > 0 {
        parts.push(Span::styled(
            format!(" ↓{}", info.behind),
            Style::default().fg(rgb(255, 140, 100)),
        ));
    }

    lines.push(Line::from(parts));

    let max_files = inner.height.saturating_sub(lines.len() as u16).min(5) as usize;
    for file in info.dirty_files.iter().take(max_files) {
        let display = truncate_smart(file, w.saturating_sub(4));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(display, Style::default().fg(rgb(140, 140, 155))),
        ]));
    }
    if info.dirty_files.len() > max_files {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("+{} more", info.dirty_files.len() - max_files),
                Style::default().fg(rgb(100, 100, 115)),
            ),
        ]));
    }

    lines
}

pub(super) fn render_git_compact(info: &GitInfo, width: u16) -> Vec<Line<'static>> {
    let w = width as usize;
    let mut parts: Vec<Span> = Vec::new();

    let branch_display = truncate_smart(&info.branch, w.saturating_sub(12).max(6));
    parts.push(Span::styled(" ", Style::default().fg(rgb(240, 160, 60))));
    parts.push(Span::styled(
        branch_display,
        Style::default().fg(rgb(160, 160, 170)),
    ));

    if info.ahead > 0 {
        parts.push(Span::styled(
            format!(" ↑{}", info.ahead),
            Style::default().fg(rgb(100, 200, 100)),
        ));
    }
    if info.behind > 0 {
        parts.push(Span::styled(
            format!(" ↓{}", info.behind),
            Style::default().fg(rgb(255, 140, 100)),
        ));
    }
    if info.modified > 0 {
        parts.push(Span::styled(
            format!(" ~{}", info.modified),
            Style::default().fg(rgb(240, 200, 80)),
        ));
    }
    if info.staged > 0 {
        parts.push(Span::styled(
            format!(" +{}", info.staged),
            Style::default().fg(rgb(100, 200, 100)),
        ));
    }
    if info.untracked > 0 {
        parts.push(Span::styled(
            format!(" ?{}", info.untracked),
            Style::default().fg(rgb(140, 140, 150)),
        ));
    }

    vec![Line::from(parts)]
}
