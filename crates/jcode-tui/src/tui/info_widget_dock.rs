//! Docked info column: the info box as a fixed panel on the right of the chat.
//!
//! The floating layout (`info_widget_layout`) fits boxes into the gaps between
//! chat lines, so they move whenever the chat scrolls or streams and shrink to
//! whatever gap is free. The dock instead reserves a column: the transcript is
//! wrapped narrower so it never runs under the panel, and the panel is drawn at
//! a fixed position and width that depend only on the terminal size, never on
//! the scroll position, the chat text, or the panel's own content.

use super::*;

/// Narrowest docked column (borders included).
const DOCK_MIN_WIDTH: u16 = 34;
/// Docked column width (borders included) when the terminal has room. Wide
/// enough for every line the sections draw at full length (context and usage
/// bars, model and account lines, KV cache), so nothing is cut off, and fixed
/// so the box never shifts sideways when a line appears or grows.
const DOCK_WIDTH: u16 = 46;
/// Columns the chat keeps beside the dock. Below this the dock is not shown.
const DOCK_MIN_CHAT_WIDTH: u16 = 60;

#[cfg(test)]
thread_local! {
    static DOCK_DISABLED_FOR_TEST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Transcript-geometry tests measure how the chat wraps at a given terminal
/// width. The dock would take part of that width, so they turn it off.
#[cfg(test)]
pub fn set_dock_disabled_for_test(disabled: bool) {
    DOCK_DISABLED_FOR_TEST.with(|flag| flag.set(disabled));
}
/// Width used to check whether the overview has anything to show. Wide
/// enough that no line is shortened.
const MEASURE_WIDTH: u16 = 200;

/// Sections shown in the dock, top to bottom.
fn dock_sections(data: &InfoWidgetData) -> Vec<WidgetKind> {
    let mut kinds = Vec::new();
    if data.has_data_for(WidgetKind::Overview) || overview_has_content(data) {
        kinds.push(WidgetKind::Overview);
    }
    for kind in [
        WidgetKind::MemoryActivity,
        WidgetKind::SwarmStatus,
        WidgetKind::BackgroundTasks,
        WidgetKind::Compaction,
        WidgetKind::AmbientMode,
    ] {
        if data.has_data_for(kind) {
            kinds.push(kind);
        }
    }
    kinds
}

fn overview_has_content(data: &InfoWidgetData) -> bool {
    !overview_lines(data, Rect::new(0, 0, MEASURE_WIDTH, u16::MAX / 2)).is_empty()
}

/// Every overview line at `inner` width. Unlike the floating Overview there is
/// no paging: the dock shows all sections at once.
///
/// The floating Overview is a detail layer only (model, context, and branch
/// live on the status line). The dock is a fixed summary panel, so it leads
/// with those identity facts before the detail sections.
fn overview_lines(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let mut overview = data.clone();
    overview.memory_info = None;
    overview.diagrams.clear();
    let mut lines = identity_lines(data, inner.width);
    lines.extend(render_sections(
        &overview,
        inner,
        Some(InfoPageKind::TodosExpanded),
    ));
    lines
}

/// Model and effort, provider and auth, context bar, and branch.
fn identity_lines(data: &InfoWidgetData, width: u16) -> Vec<Line<'static>> {
    let max = usize::from(width);
    let mut lines = Vec::new();
    if let Some(model) = data.model.as_deref().filter(|m| !m.trim().is_empty()) {
        let mut spans = vec![Span::styled(
            truncate_smart(
                &crate::tui::session_facts::pretty_model(model),
                max.saturating_sub(8),
            ),
            Style::default().fg(rgb(255, 150, 200)).bold(),
        )];
        if let Some(effort) = data
            .reasoning_effort
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
        {
            spans.push(Span::styled(
                format!(" ({effort})"),
                Style::default().fg(rgb(255, 200, 100)),
            ));
        }
        lines.push(Line::from(spans));
    }

    let mut detail = Vec::new();
    if let Some(provider) = data
        .provider_name
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        detail.push(Span::styled(
            provider.to_lowercase(),
            Style::default().fg(rgb(140, 180, 255)),
        ));
    }
    if let Some(auth) = auth_label(data.auth_method) {
        if !detail.is_empty() {
            detail.push(Span::styled(" · ", Style::default().fg(rgb(80, 80, 90))));
        }
        detail.push(Span::styled(auth, Style::default().fg(rgb(140, 140, 150))));
    }
    if !detail.is_empty() {
        lines.push(Line::from(detail));
    }

    if let Some(line) = context_line(data, width) {
        lines.push(line);
    }

    if let Some(info) = data
        .git_info
        .as_ref()
        .filter(|g| !g.branch.trim().is_empty())
    {
        lines.push(branch_line(info, max));
    }
    lines
}

fn auth_label(method: AuthMethod) -> Option<&'static str> {
    match method {
        AuthMethod::Unknown => None,
        AuthMethod::AnthropicOAuth
        | AuthMethod::OpenAIOAuth
        | AuthMethod::CopilotOAuth
        | AuthMethod::GeminiOAuth => Some("🔐 OAuth"),
        AuthMethod::ApiKey
        | AuthMethod::AnthropicApiKey
        | AuthMethod::OpenAIApiKey
        | AuthMethod::OpenRouterApiKey
        | AuthMethod::OpenCodeApiKey => Some("🔑 API Key"),
    }
}

/// `Context 236k/1000k ▰▰▱▱…`, or `None` before any context is known.
fn context_line(data: &InfoWidgetData, width: u16) -> Option<Line<'static>> {
    let label = if data.is_compacting {
        "Context📦"
    } else {
        "Context"
    };
    if data.context_info_stale {
        return Some(Line::from(vec![
            Span::styled(format!("{label} "), Style::default().fg(rgb(140, 140, 150))),
            Span::styled("updating...", Style::default().fg(rgb(220, 180, 80))),
        ]));
    }
    let used = match (data.observed_context_tokens, data.context_info.as_ref()) {
        (Some(tokens), _) => tokens as usize,
        (None, Some(info)) if info.total_chars > 0 => info.estimated_tokens(),
        _ => return None,
    };
    let limit = data
        .context_limit
        .unwrap_or(crate::provider::DEFAULT_CONTEXT_LIMIT)
        .max(1);
    let k = |t: usize| {
        if t >= 1000 {
            format!("{}k", t / 1000)
        } else {
            t.to_string()
        }
    };
    let tokens = format!("{}/{}", k(used), k(limit));
    let used_pct = ((used as f64 / limit as f64) * 100.0)
        .round()
        .clamp(0.0, 100.0) as u8;
    let left_pct = 100u8.saturating_sub(used_pct);
    let color = if left_pct <= 20 {
        rgb(255, 100, 100)
    } else if left_pct <= 50 {
        rgb(255, 200, 100)
    } else {
        rgb(100, 200, 100)
    };
    let mut spans = vec![
        Span::styled(format!("{label} "), Style::default().fg(rgb(140, 140, 150))),
        Span::styled(format!("{tokens} "), Style::default().fg(color).bold()),
    ];
    let fixed = UnicodeWidthStr::width(label) + 1 + tokens.len() + 1;
    let bar = usize::from(width).saturating_sub(fixed).min(24);
    if bar >= 3 {
        let filled = ((used as f64 / limit as f64) * bar as f64).round() as usize;
        let filled = filled.min(bar);
        spans.push(Span::styled("▰".repeat(filled), Style::default().fg(color)));
        spans.push(Span::styled(
            "▱".repeat(bar - filled),
            Style::default().fg(rgb(50, 50, 60)),
        ));
    }
    Some(Line::from(spans))
}

/// Branch with ahead/behind and local change counts.
fn branch_line(info: &GitInfo, max: usize) -> Line<'static> {
    let mut spans = vec![
        Span::styled(" ", Style::default().fg(rgb(240, 160, 60))),
        Span::styled(
            truncate_smart(&info.branch, max.saturating_sub(24).max(6)),
            Style::default().fg(rgb(160, 160, 170)),
        ),
    ];
    for (count, sign, color) in [
        (info.ahead, "↑", rgb(100, 200, 100)),
        (info.behind, "↓", rgb(255, 140, 100)),
        (info.modified, "~", rgb(240, 200, 80)),
        (info.staged, "+", rgb(100, 200, 100)),
        (info.untracked, "?", rgb(140, 140, 150)),
    ] {
        if count > 0 {
            spans.push(Span::styled(
                format!(" {sign}{count}"),
                Style::default().fg(color),
            ));
        }
    }
    Line::from(spans)
}

fn section_lines(kind: WidgetKind, data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    match kind {
        WidgetKind::Overview => overview_lines(data, inner),
        other => inline_frame(render_widget_content(other, data, inner), inner.width),
    }
}

/// The floating layout draws a widget's header, overflow counts, and legends
/// on its own border. Docked sections share one box, so that border text
/// becomes a header row above the body and a footer row below it.
fn inline_frame(framed: frame::Framed, width: u16) -> Vec<Line<'static>> {
    fn join(left: Option<Line<'static>>, right: Option<Line<'static>>) -> Option<Line<'static>> {
        match (left, right) {
            (None, None) => None,
            (Some(line), None) | (None, Some(line)) => Some(line),
            (Some(left), Some(right)) => {
                let mut spans = left.spans;
                spans.push(Span::raw(" "));
                spans.extend(right.spans);
                Some(Line::from(spans))
            }
        }
    }
    let width = usize::from(width);
    let fit = |line: Line<'static>| frame::fit(&line, width).unwrap_or(line);
    let mut lines = Vec::with_capacity(framed.lines.len() + 2);
    lines.extend(join(framed.title, framed.title_right).map(fit));
    lines.extend(framed.lines);
    lines.extend(join(framed.footer, framed.footer_right).map(fit));
    lines
}

/// Docked column width for this terminal and content, or `None` when the
/// chat would be too narrow to spare a column.
///
/// The workspace map and margin diagrams are drawn as pictures by the floating
/// layout. The dock only stacks text sections, so when either is present the
/// dock stands aside and the floating layout shows everything.
pub fn dock_width(data: &InfoWidgetData, chat_width: u16) -> Option<u16> {
    #[cfg(test)]
    if DOCK_DISABLED_FOR_TEST.with(|flag| flag.get()) {
        return None;
    }
    if data.has_data_for(WidgetKind::WorkspaceMap) || data.has_data_for(WidgetKind::Diagrams) {
        return None;
    }
    let available = chat_width.checked_sub(DOCK_MIN_CHAT_WIDTH)?;
    if available < DOCK_MIN_WIDTH {
        return None;
    }
    if dock_sections(data).is_empty() {
        return None;
    }
    // Depends on the terminal width only. Measuring the content made the box
    // one column wider (and its left edge jump) whenever a longer line such as
    // the KV cache line appeared after the first reply.
    Some(DOCK_WIDTH.min(available))
}

/// Every line of the docked box (sections separated by a dim rule) at the
/// given inner width.
fn dock_lines(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    for kind in dock_sections(data) {
        let section = section_lines(kind, data, inner);
        if section.is_empty() {
            continue;
        }
        if !lines.is_empty() {
            lines.push(Line::from(Span::styled(
                "─".repeat(inner.width as usize),
                Style::default().fg(rgb(55, 55, 65)),
            )));
        }
        lines.extend(section);
    }
    lines
}

/// Height of the docked box (borders included) at column `width`, or 0 when
/// there is nothing to show.
pub fn dock_height(data: &InfoWidgetData, width: u16) -> u16 {
    let inner = Rect::new(0, 0, width.saturating_sub(2), u16::MAX / 2);
    let lines = dock_lines(data, inner).len() as u16;
    if lines == 0 {
        0
    } else {
        lines.saturating_add(2)
    }
}

/// Where the docked box goes. Its top edge sits at a fixed row, a third of the
/// way down the chat column, computed from the column height alone (which only
/// changes with the terminal size). So the box never moves when the chat
/// scrolls or streams, when the input grows, or when its own content changes
/// height (usage limits appearing, todos): it grows and shrinks downward from
/// that row. Only if it would reach `bottom` (the status line) is it pulled
/// up, and clipped if the space is smaller than the box.
pub fn dock_rect(column: Rect, bottom: u16, box_height: u16) -> Option<Rect> {
    let usable = bottom.saturating_sub(column.y);
    if box_height == 0 || usable < 3 {
        return None;
    }
    let height = box_height.min(usable);
    let anchor = column.y + column.height / 3;
    let y = anchor.min(bottom - height).max(column.y);
    Some(Rect::new(column.x, y, column.width, height))
}

/// Draw the docked box into `rect` (from [`dock_rect`]).
pub fn render_dock(frame: &mut Frame, rect: Rect, data: &InfoWidgetData) {
    if rect.width < 3 || rect.height < 3 {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(rgb(70, 70, 80)).dim());
    let inner = block.inner(rect);
    let mut lines = dock_lines(data, inner);
    if lines.is_empty() {
        return;
    }
    lines.truncate(inner.height as usize);
    frame.render_widget(ratatui::widgets::Clear, rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Lines the dock would draw at `width` (borders included), for tests and
/// debug captures.
pub fn dock_text_lines(data: &InfoWidgetData, width: u16) -> Vec<String> {
    let inner = Rect::new(0, 0, width.saturating_sub(2), u16::MAX / 2);
    dock_sections(data)
        .into_iter()
        .flat_map(|kind| section_lines(kind, data, inner))
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}
