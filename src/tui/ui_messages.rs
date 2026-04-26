use super::*;
#[path = "ui_messages_cache.rs"]
mod cache_support;
use crate::message::{
    ParsedBackgroundTaskProgressNotification, parse_background_task_notification_markdown,
    parse_background_task_progress_notification_markdown,
};
pub(super) use cache_support::get_cached_message_lines;
use cache_support::{centered_wrap_width, left_pad_lines_for_centered_mode};
use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use unicode_width::UnicodeWidthStr;

fn prefer_width_stable_system_glyphs() -> bool {
    std::env::var("TERM_PROGRAM")
        .ok()
        .map(|value| value.eq_ignore_ascii_case("kitty"))
        .unwrap_or(false)
        || std::env::var("TERM")
            .ok()
            .map(|value| value.to_ascii_lowercase().contains("kitty"))
            .unwrap_or(false)
}

fn width_stable_system_title<'a>(normal: &'a str, stable: &'a str) -> &'a str {
    if prefer_width_stable_system_glyphs() {
        stable
    } else {
        normal
    }
}

fn normalize_system_content_for_display(content: &str) -> Cow<'_, str> {
    if !prefer_width_stable_system_glyphs() {
        return Cow::Borrowed(content);
    }

    let normalized = content
        .replace("⚡ ", "! ")
        .replace("⏳ ", "... ")
        .replace("⏰ ", "* ");
    Cow::Owned(normalized)
}

pub(crate) fn render_assistant_message(
    msg: &DisplayMessage,
    width: u16,
    _diff_mode: crate::config::DiffDisplayMode,
) -> Vec<Line<'static>> {
    let centered = markdown::center_code_blocks();
    let wrap_width = centered_wrap_width(width, centered, 96);
    let mut lines = markdown::render_markdown_with_width(&msg.content, Some(wrap_width));
    if centered {
        markdown::recenter_structured_blocks_for_display(&mut lines, width as usize);
    }
    if !msg.tool_calls.is_empty() {
        if lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| !span.content.trim().is_empty())
        }) {
            lines.push(Line::default().alignment(ratatui::layout::Alignment::Left));
        }
        lines.extend(render_assistant_tool_call_lines(
            &msg.tool_calls,
            wrap_width,
            centered,
        ));
    }
    lines
}

fn render_assistant_tool_call_lines(
    tool_calls: &[String],
    width: usize,
    centered: bool,
) -> Vec<Line<'static>> {
    if tool_calls.is_empty() {
        return Vec::new();
    }

    const TOOL_SEPARATOR: &str = " · ";

    let label = if tool_calls.len() == 1 {
        "tool:"
    } else {
        "tools:"
    };
    let prefix = format!("  {} ", label);
    let prefix_width = prefix.width();
    let available_width = width.max(prefix_width.saturating_add(1));

    let prefix_style = Style::default().fg(tool_color()).dim();
    let separator_style = Style::default().fg(dim_color()).dim();
    let name_style = Style::default().fg(accent_color()).dim();

    let max_width = available_width.saturating_sub(1).max(prefix_width + 1);
    let mut spans = vec![Span::styled(prefix.clone(), prefix_style)];
    let mut current_width = prefix_width;
    let mut shown = 0usize;

    for (idx, tool_name) in tool_calls.iter().enumerate() {
        let separator_width = if shown == 0 {
            0
        } else {
            TOOL_SEPARATOR.width()
        };
        let more_remaining = tool_calls.len().saturating_sub(idx + 1);
        let more_label = if more_remaining > 0 {
            format!("{}+{} more", TOOL_SEPARATOR, more_remaining)
        } else {
            String::new()
        };
        let required = separator_width + tool_name.width() + more_label.width();

        if current_width.saturating_add(required) <= max_width {
            if shown > 0 {
                spans.push(Span::styled(TOOL_SEPARATOR, separator_style));
                current_width = current_width.saturating_add(separator_width);
            }
            spans.push(Span::styled(tool_name.clone(), name_style));
            current_width = current_width.saturating_add(tool_name.width());
            shown += 1;
        } else {
            break;
        }
    }

    if shown < tool_calls.len() {
        let remaining = tool_calls.len() - shown;
        let more_text = if shown == 0 {
            format!("+{} more", remaining)
        } else {
            format!("{}+{} more", TOOL_SEPARATOR, remaining)
        };
        spans.push(Span::styled(more_text, separator_style));
    }

    let mut lines = vec![super::truncate_line_with_ellipsis_to_width(
        &Line::from(spans),
        max_width,
    )];

    if centered {
        left_pad_lines_for_centered_mode(&mut lines, width as u16);
        if let Some(line) = lines.first_mut() {
            *line = super::truncate_line_with_ellipsis_to_width(line, max_width);
        }
    }

    lines
}

pub(crate) fn render_system_message(
    msg: &DisplayMessage,
    width: u16,
    _diff_mode: crate::config::DiffDisplayMode,
) -> Vec<Line<'static>> {
    if let Some(title) = msg.title.as_deref() {
        if title == "Reload" {
            return render_reload_system_message(msg, width);
        }
        if title == "Connection" {
            return render_connection_system_message(msg, width);
        }
    }

    if let Some(lines) = render_scheduled_session_message(msg, width) {
        return lines;
    }

    let centered = markdown::center_code_blocks();
    let wrap_width = centered_wrap_width(width.saturating_sub(4), centered, 96);
    let display_content = normalize_system_content_for_display(&msg.content);
    let mut lines = markdown::render_markdown_with_width(&display_content, Some(wrap_width));
    if lines.iter().any(|line| line.width() > wrap_width) {
        lines = display_content
            .lines()
            .flat_map(|line| {
                if line.is_empty() {
                    vec![Line::from("")]
                } else {
                    split_by_display_width(line, wrap_width)
                        .into_iter()
                        .map(Line::from)
                        .collect::<Vec<_>>()
                }
            })
            .collect();
    }
    if centered {
        left_pad_lines_for_centered_mode(&mut lines, width);
    }
    for line in &mut lines {
        for span in &mut line.spans {
            span.style.fg = Some(system_message_color());
        }
    }
    lines
}

pub(crate) fn render_usage_message(
    msg: &DisplayMessage,
    width: u16,
    _diff_mode: crate::config::DiffDisplayMode,
) -> Vec<Line<'static>> {
    let border_style = Style::default().fg(rgb(120, 140, 190));
    let title = msg.title.as_deref().unwrap_or("Usage");
    let inner_width = width.saturating_sub(8).max(24) as usize;
    let content_width = inner_width.min(96);

    let mut content = Vec::new();
    for raw_line in msg.content.lines() {
        if raw_line.is_empty() {
            content.push(Line::from(""));
            continue;
        }

        let (text, style) = if let Some(rest) = raw_line.strip_prefix("! ") {
            (rest, Style::default().fg(Color::Red))
        } else if let Some(rest) = raw_line.strip_prefix("~ ") {
            (rest, Style::default().fg(rgb(255, 200, 100)))
        } else if let Some(rest) = raw_line.strip_prefix("+ ") {
            (rest, Style::default().fg(rgb(100, 220, 170)))
        } else if let Some(rest) = raw_line.strip_prefix("# ") {
            (rest, Style::default().fg(Color::White).bold())
        } else {
            (raw_line, Style::default().fg(dim_color()))
        };

        let chunks = split_by_display_width(text, content_width);
        if chunks.is_empty() {
            content.push(Line::from(""));
        } else {
            for chunk in chunks {
                content.push(Line::from(Span::styled(chunk, style)));
            }
        }
    }

    if content.is_empty() {
        content.push(Line::from(Span::styled(
            "No usage data available.",
            Style::default().fg(dim_color()),
        )));
    }

    render_rounded_box(
        title,
        content,
        width.saturating_sub(4) as usize,
        border_style,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedScheduledSessionMessage {
    task: String,
    working_dir: Option<String>,
    relevant_files: Option<String>,
    branch: Option<String>,
    background: Option<String>,
    success_criteria: Option<String>,
    scheduled_by_session: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedScheduledToolMessage {
    task: String,
    when: String,
    id: Option<String>,
    working_dir: Option<String>,
    relevant_files: Option<String>,
    target: Option<String>,
}

fn parse_prefixed_value(line: &str, prefix: &str) -> Option<String> {
    line.trim()
        .strip_prefix(prefix)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn push_card_section(
    content: &mut Vec<Line<'static>>,
    label: &str,
    value: Option<&str>,
    inner_width: usize,
    label_style: Style,
    body_style: Style,
) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };

    if !content.is_empty() {
        content.push(Line::from(""));
    }
    content.push(Line::from(Span::styled(label.to_string(), label_style)));
    for chunk in split_by_display_width(value, inner_width) {
        content.push(Line::from(Span::styled(chunk, body_style)));
    }
}

fn parse_scheduled_session_message(content: &str) -> Option<ParsedScheduledSessionMessage> {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = normalized.lines().map(str::trim);
    if lines.next()? != "[Scheduled task]" {
        return None;
    }
    let due_line = lines.next()?.trim();
    if !due_line.starts_with("A scheduled task for this session is now due.") {
        return None;
    }

    let mut parsed = ParsedScheduledSessionMessage {
        task: String::new(),
        working_dir: None,
        relevant_files: None,
        branch: None,
        background: None,
        success_criteria: None,
        scheduled_by_session: None,
    };

    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some(value) = parse_prefixed_value(line, "Task: ") {
            parsed.task = value;
        } else if let Some(value) = parse_prefixed_value(line, "Working directory: ") {
            parsed.working_dir = Some(value);
        } else if let Some(value) = parse_prefixed_value(line, "Relevant files: ") {
            parsed.relevant_files = Some(value);
        } else if let Some(value) = parse_prefixed_value(line, "Branch: ") {
            parsed.branch = Some(value);
        } else if let Some(value) = parse_prefixed_value(line, "Background: ") {
            parsed.background = Some(value);
        } else if let Some(value) = parse_prefixed_value(line, "Success criteria: ") {
            parsed.success_criteria = Some(value);
        } else if let Some(value) = parse_prefixed_value(line, "Scheduled by session: ") {
            parsed.scheduled_by_session = Some(value);
        }
    }

    if parsed.task.is_empty() {
        return None;
    }

    Some(parsed)
}

fn render_scheduled_session_message(
    msg: &DisplayMessage,
    width: u16,
) -> Option<Vec<Line<'static>>> {
    let parsed = parse_scheduled_session_message(&msg.content)?;
    let centered = markdown::center_code_blocks();
    let max_box_width = if centered {
        (width.saturating_sub(4) as usize).min(96)
    } else {
        (width.saturating_sub(2) as usize).min(88)
    }
    .max(20);
    let inner_width = max_box_width.saturating_sub(4).max(1);

    let border_style = Style::default().fg(rgb(120, 180, 255));
    let status_style = Style::default().fg(rgb(186, 220, 255)).bold();
    let label_style = Style::default().fg(dim_color());
    let body_style = Style::default().fg(rgb(225, 232, 245));
    let meta_style = Style::default().fg(rgb(170, 200, 255));

    let mut box_content = vec![Line::from(Span::styled(
        "This scheduled task is now active in this session.",
        status_style,
    ))];
    push_card_section(
        &mut box_content,
        "Task",
        Some(&parsed.task),
        inner_width,
        label_style,
        body_style,
    );
    push_card_section(
        &mut box_content,
        "Working directory",
        parsed.working_dir.as_deref(),
        inner_width,
        label_style,
        body_style,
    );
    push_card_section(
        &mut box_content,
        "Relevant files",
        parsed.relevant_files.as_deref(),
        inner_width,
        label_style,
        body_style,
    );
    push_card_section(
        &mut box_content,
        "Branch",
        parsed.branch.as_deref(),
        inner_width,
        label_style,
        body_style,
    );
    push_card_section(
        &mut box_content,
        "Background",
        parsed.background.as_deref(),
        inner_width,
        label_style,
        body_style,
    );
    push_card_section(
        &mut box_content,
        "Success criteria",
        parsed.success_criteria.as_deref(),
        inner_width,
        label_style,
        body_style,
    );
    push_card_section(
        &mut box_content,
        "Created by",
        parsed.scheduled_by_session.as_deref(),
        inner_width,
        label_style,
        meta_style,
    );

    let mut lines = render_rounded_box(
        width_stable_system_title("⏰ scheduled task due", "scheduled task due"),
        box_content,
        max_box_width,
        border_style,
    );
    if centered {
        left_pad_lines_for_centered_mode(&mut lines, width);
    }
    Some(lines)
}

fn parse_scheduled_tool_message(msg: &DisplayMessage) -> Option<ParsedScheduledToolMessage> {
    let task = msg
        .title
        .as_deref()?
        .strip_prefix("scheduled: ")?
        .trim()
        .to_string();
    if task.is_empty() {
        return None;
    }

    let normalized = msg.content.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = normalized.lines().map(str::trim);
    let first_line = lines.next()?.trim();

    let (when, id) = if let Some(rest) = first_line.strip_prefix("Scheduled task '") {
        let (_task_in_line, when_part) = rest.split_once("' for ")?;
        if let Some((when, id_part)) = when_part.rsplit_once(" (id: ") {
            (
                when.trim().to_string(),
                id_part.strip_suffix(')').map(str::trim).map(str::to_string),
            )
        } else {
            (when_part.trim().to_string(), None)
        }
    } else if let Some(rest) = first_line.strip_prefix("Scheduled ambient task ") {
        let (id, when) = rest.split_once(" for ")?;
        (when.trim().to_string(), Some(id.trim().to_string()))
    } else {
        return None;
    };

    let mut working_dir = None;
    let mut relevant_files = None;
    let mut target = None;
    for line in lines {
        if let Some(value) = parse_prefixed_value(line, "Working directory: ") {
            working_dir = Some(value);
        } else if let Some(value) = parse_prefixed_value(line, "Relevant files: ") {
            relevant_files = Some(value);
        } else if let Some(value) = parse_prefixed_value(line, "Target: ") {
            target = Some(value);
        }
    }

    Some(ParsedScheduledToolMessage {
        task,
        when,
        id,
        working_dir,
        relevant_files,
        target,
    })
}

fn render_scheduled_tool_message(msg: &DisplayMessage, width: u16) -> Option<Vec<Line<'static>>> {
    let parsed = parse_scheduled_tool_message(msg)?;
    let centered = markdown::center_code_blocks();
    let max_box_width = if centered {
        (width.saturating_sub(4) as usize).min(96)
    } else {
        (width.saturating_sub(2) as usize).min(88)
    }
    .max(20);
    let inner_width = max_box_width.saturating_sub(4).max(1);

    let border_style = Style::default().fg(rgb(140, 180, 255));
    let status_style = Style::default().fg(rgb(186, 220, 255)).bold();
    let label_style = Style::default().fg(dim_color());
    let body_style = Style::default().fg(rgb(225, 232, 245));
    let meta_style = Style::default().fg(rgb(170, 200, 255));

    let mut box_content = vec![Line::from(Span::styled(
        format!("Will run {}.", parsed.when),
        status_style,
    ))];
    push_card_section(
        &mut box_content,
        "Task",
        Some(&parsed.task),
        inner_width,
        label_style,
        body_style,
    );
    push_card_section(
        &mut box_content,
        "Target",
        parsed.target.as_deref(),
        inner_width,
        label_style,
        body_style,
    );
    push_card_section(
        &mut box_content,
        "Working directory",
        parsed.working_dir.as_deref(),
        inner_width,
        label_style,
        body_style,
    );
    push_card_section(
        &mut box_content,
        "Relevant files",
        parsed.relevant_files.as_deref(),
        inner_width,
        label_style,
        body_style,
    );
    push_card_section(
        &mut box_content,
        "Task id",
        parsed.id.as_deref(),
        inner_width,
        label_style,
        meta_style,
    );

    let mut lines = render_rounded_box(
        width_stable_system_title("⏰ scheduled", "scheduled"),
        box_content,
        max_box_width,
        border_style,
    );
    if centered {
        left_pad_lines_for_centered_mode(&mut lines, width);
    }
    Some(lines)
}

fn render_reload_system_message(msg: &DisplayMessage, width: u16) -> Vec<Line<'static>> {
    let centered = markdown::center_code_blocks();
    let border_style = Style::default().fg(rgb(120, 180, 255));
    let label_style = Style::default().fg(dim_color());
    let text_style = Style::default().fg(rgb(220, 236, 255));
    let max_box_width = if centered {
        (width.saturating_sub(4) as usize).min(96)
    } else {
        (width.saturating_sub(2) as usize).min(88)
    }
    .max(20);
    let inner_width = max_box_width.saturating_sub(4).max(1);

    let mut box_content = Vec::new();
    let mut non_empty_lines = msg
        .content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .peekable();

    if non_empty_lines.peek().is_none() {
        box_content.push(Line::from(Span::styled("No reload details.", label_style)));
    } else {
        for (idx, line) in non_empty_lines.enumerate() {
            if idx > 0 {
                box_content.push(Line::from(""));
            }
            for chunk in split_by_display_width(line, inner_width) {
                box_content.push(Line::from(Span::styled(chunk, text_style)));
            }
        }
    }

    let mut lines = render_rounded_box(
        width_stable_system_title("⚡ reload", "reload"),
        box_content,
        max_box_width,
        border_style,
    );
    if centered {
        left_pad_lines_for_centered_mode(&mut lines, width);
    }
    lines
}

fn split_resume_hint(detail: &str) -> (&str, Option<&str>) {
    if let Some((main, hint)) = detail.split_once(" · resume: ") {
        (main.trim(), Some(hint.trim()))
    } else {
        (detail.trim(), None)
    }
}

fn parse_connection_retry_message(content: &str) -> Option<(String, String, Option<String>)> {
    let rest = content.strip_prefix("⚡ Connection lost — retrying (attempt ")?;
    let (attempt_and_elapsed, detail) = rest.split_once(") — ")?;
    let (attempt, elapsed) = attempt_and_elapsed.split_once(", ")?;
    let (detail, hint) = split_resume_hint(detail);
    Some((
        format!("Retrying · attempt {} · {}", attempt.trim(), elapsed.trim()),
        detail.to_string(),
        hint.map(str::to_string),
    ))
}

fn parse_connection_waiting_message(content: &str) -> Option<(String, String, Option<String>)> {
    let rest = content.strip_prefix("⚡ Server reload in progress — waiting for handoff (")?;
    let (elapsed, detail) = rest.split_once(") — ")?;
    let (detail, hint) = split_resume_hint(detail);
    Some((
        format!("Waiting for handoff · {}", elapsed.trim()),
        detail.to_string(),
        hint.map(str::to_string),
    ))
}

fn render_connection_system_message(msg: &DisplayMessage, width: u16) -> Vec<Line<'static>> {
    let centered = markdown::center_code_blocks();
    let content = msg.content.trim();
    let max_box_width = if centered {
        (width.saturating_sub(4) as usize).min(96)
    } else {
        (width.saturating_sub(2) as usize).min(88)
    }
    .max(20);
    let inner_width = max_box_width.saturating_sub(4).max(1);

    let (title, border_color, status_color, status_line, detail, hint) =
        if let Some((status_line, detail, hint)) = parse_connection_retry_message(content) {
            (
                width_stable_system_title("⚡ reconnecting", "reconnecting"),
                rgb(255, 193, 94),
                rgb(255, 220, 140),
                status_line,
                Some(detail),
                hint,
            )
        } else if let Some((status_line, detail, hint)) = parse_connection_waiting_message(content)
        {
            (
                width_stable_system_title("⚡ waiting for reload", "waiting for reload"),
                rgb(120, 180, 255),
                rgb(180, 215, 255),
                status_line,
                Some(detail),
                hint,
            )
        } else if content.starts_with("⏳ Starting server") {
            (
                width_stable_system_title("⏳ starting server", "starting server"),
                rgb(255, 193, 94),
                rgb(255, 220, 140),
                "Starting shared server".to_string(),
                None,
                None,
            )
        } else {
            let display_content = normalize_system_content_for_display(content);
            let mut lines =
                markdown::render_markdown_with_width(&display_content, Some(inner_width));
            if centered {
                left_pad_lines_for_centered_mode(&mut lines, width);
            }
            for line in &mut lines {
                for span in &mut line.spans {
                    span.style.fg = Some(system_message_color());
                }
            }
            return lines;
        };

    let border_style = Style::default().fg(border_color);
    let status_style = Style::default().fg(status_color).bold();
    let label_style = Style::default().fg(dim_color());
    let body_style = Style::default().fg(rgb(225, 232, 245));
    let hint_style = Style::default().fg(rgb(170, 200, 255));
    let mut box_content = vec![Line::from(Span::styled(status_line, status_style))];

    if let Some(detail) = detail.filter(|detail| !detail.is_empty()) {
        box_content.push(Line::from(""));
        box_content.push(Line::from(Span::styled("Detail", label_style)));
        for chunk in split_by_display_width(&detail, inner_width) {
            box_content.push(Line::from(Span::styled(chunk, body_style)));
        }
    }

    if let Some(hint) = hint.filter(|hint| !hint.is_empty()) {
        box_content.push(Line::from(""));
        box_content.push(Line::from(Span::styled("Resume", label_style)));
        for chunk in split_by_display_width(&hint, inner_width) {
            box_content.push(Line::from(Span::styled(chunk, hint_style)));
        }
    }

    let mut lines = render_rounded_box(title, box_content, max_box_width, border_style);
    if centered {
        left_pad_lines_for_centered_mode(&mut lines, width);
    }
    lines
}

pub(crate) fn render_background_task_message(
    msg: &DisplayMessage,
    width: u16,
    diff_mode: crate::config::DiffDisplayMode,
) -> Vec<Line<'static>> {
    if let Some(progress) = parse_background_task_progress_notification_markdown(&msg.content) {
        return render_background_task_progress_message(&progress, width);
    }

    let Some(parsed) = parse_background_task_notification_markdown(&msg.content) else {
        return render_system_message(msg, width, diff_mode);
    };

    let centered = markdown::center_code_blocks();
    let task_label = crate::message::background_task_display_label(
        &parsed.tool_name,
        parsed.display_name.as_deref(),
    );
    let (title, border_color, status_color, preview_color) = if parsed.status.starts_with('✓') {
        (
            format!("✓ bg {} completed · {}", task_label, parsed.task_id),
            rgb(100, 180, 100),
            rgb(120, 210, 140),
            rgb(214, 240, 220),
        )
    } else if parsed.status.starts_with('✗') {
        (
            format!("✗ bg {} failed · {}", task_label, parsed.task_id),
            rgb(220, 100, 100),
            rgb(255, 150, 150),
            rgb(255, 225, 225),
        )
    } else {
        (
            format!("◌ bg {} running · {}", task_label, parsed.task_id),
            rgb(255, 193, 94),
            rgb(255, 214, 120),
            rgb(255, 241, 214),
        )
    };

    let border_style = Style::default().fg(border_color);
    let label_style = Style::default().fg(dim_color());
    let status_style = Style::default().fg(status_color).bold();
    let preview_style = Style::default().fg(preview_color);

    let max_box_width = if centered {
        (width.saturating_sub(4) as usize).min(120)
    } else {
        (width.saturating_sub(2) as usize).min(96)
    }
    .max(16);
    let inner_width = max_box_width.saturating_sub(4).max(1);

    let mut box_content: Vec<Line<'static>> = vec![Line::from(vec![
        Span::styled(parsed.exit_label.clone(), status_style),
        Span::styled(" · ", label_style),
        Span::styled(parsed.duration.clone(), label_style),
    ])];

    if let Some(failure_summary) = parsed
        .failure_summary
        .as_deref()
        .filter(|summary| !summary.is_empty())
    {
        box_content.push(Line::from(""));
        box_content.push(Line::from(Span::styled("Failure", label_style)));
        for chunk in split_by_display_width(failure_summary, inner_width) {
            box_content.push(Line::from(Span::styled(chunk, status_style)));
        }
    }

    box_content.push(Line::from(""));

    match parsed.preview.as_deref() {
        Some(preview) => {
            let preview_lines: Vec<&str> = preview.lines().collect();
            let shown_lines = preview_lines.len().min(4);
            for line in preview_lines.iter().take(shown_lines) {
                if line.is_empty() {
                    box_content.push(Line::from(""));
                    continue;
                }
                for chunk in split_by_display_width(line, inner_width) {
                    box_content.push(Line::from(Span::styled(chunk, preview_style)));
                }
            }
            if preview_lines.len() > shown_lines {
                let remaining = preview_lines.len() - shown_lines;
                box_content.push(Line::from(Span::styled(
                    format!(
                        "… +{} more line{}",
                        remaining,
                        if remaining == 1 { "" } else { "s" }
                    ),
                    label_style,
                )));
            }
        }
        None => {
            box_content.push(Line::from(Span::styled("No output captured.", label_style)));
        }
    }

    let mut lines = render_rounded_box(&title, box_content, max_box_width, border_style);
    if centered {
        left_pad_lines_for_centered_mode(&mut lines, width);
    }
    lines
}

fn progress_summary_without_leading_percent(summary: &str) -> &str {
    if let Some((first, rest)) = summary.split_once(" · ") {
        let first = first.trim();
        if first
            .strip_suffix('%')
            .and_then(|value| value.parse::<f32>().ok())
            .is_some()
        {
            return rest.trim();
        }
    }
    summary.trim()
}

fn render_compact_progress_line(
    progress: &ParsedBackgroundTaskProgressNotification,
    inner_width: usize,
    filled_style: Style,
    empty_style: Style,
    label_style: Style,
    text_style: Style,
) -> Line<'static> {
    let Some(percent) = progress.percent else {
        return super::truncate_line_with_ellipsis_to_width(
            &Line::from(Span::styled(progress.summary.clone(), text_style)),
            inner_width,
        );
    };

    let percent = percent.clamp(0.0, 100.0);
    let label = format!("{:>3}%", percent.round() as u32);
    let separator = " · ";
    let summary = progress_summary_without_leading_percent(&progress.summary);
    let fixed_width = 1 + label.width() + separator.width();
    let bar_width = if inner_width >= 56 {
        18
    } else if inner_width >= 40 {
        14
    } else if inner_width >= 28 {
        10
    } else {
        6
    }
    .min(inner_width.saturating_sub(fixed_width).max(1));
    let filled = ((percent / 100.0) * bar_width as f32).round() as usize;
    let filled = filled.min(bar_width);
    let empty = bar_width.saturating_sub(filled);

    let line = Line::from(vec![
        Span::styled("█".repeat(filled), filled_style),
        Span::styled("░".repeat(empty), empty_style),
        Span::styled(" ", label_style),
        Span::styled(label, label_style),
        Span::styled(separator, label_style),
        Span::styled(summary.to_string(), text_style),
    ]);

    super::truncate_line_with_ellipsis_to_width(&line, inner_width)
}

fn render_background_task_progress_message(
    progress: &ParsedBackgroundTaskProgressNotification,
    width: u16,
) -> Vec<Line<'static>> {
    let centered = markdown::center_code_blocks();
    let border_color = rgb(255, 193, 94);
    let border_style = Style::default().fg(border_color);
    let label_style = Style::default().fg(dim_color());
    let text_style = Style::default().fg(rgb(255, 241, 214));
    let filled_style = Style::default().fg(rgb(255, 214, 120));
    let empty_style = Style::default().fg(rgb(94, 82, 62));

    let max_box_width = if centered {
        (width.saturating_sub(4) as usize).min(120)
    } else {
        (width.saturating_sub(2) as usize).min(96)
    }
    .max(16);
    let inner_width = max_box_width.saturating_sub(4).max(1);
    let task_label = crate::message::background_task_display_label(
        &progress.tool_name,
        progress.display_name.as_deref(),
    );
    let title = format!("◌ bg {} · {}", task_label, progress.task_id);

    let mut box_content = vec![render_compact_progress_line(
        progress,
        inner_width,
        filled_style,
        empty_style,
        label_style,
        text_style,
    )];
    let hint = format!(
        "Latest status: bg action=\"status\" task_id=\"{}\"",
        progress.task_id
    );
    box_content.push(super::truncate_line_with_ellipsis_to_width(
        &Line::from(Span::styled(hint, label_style)),
        inner_width,
    ));

    let mut lines = render_rounded_box(&title, box_content, max_box_width, border_style);
    if centered {
        left_pad_lines_for_centered_mode(&mut lines, width);
    }
    lines
}

fn swarm_notification_style(title: Option<&str>) -> (&'static str, Color, Color) {
    match title.unwrap_or_default() {
        t if t.starts_with("DM from ") => ("✉", rgb(120, 180, 255), rgb(214, 232, 255)),
        t if t.starts_with('#') => ("#", rgb(90, 210, 200), rgb(214, 247, 244)),
        t if t.starts_with("Broadcast") => ("📣", rgb(255, 193, 94), rgb(255, 240, 214)),
        t if t.starts_with("Shared context") => ("🧠", rgb(120, 210, 160), rgb(221, 247, 232)),
        t if t.starts_with("File activity") => ("⚠", rgb(255, 160, 120), rgb(255, 228, 214)),
        t if t.starts_with("Task") => ("⚑", rgb(130, 184, 255), rgb(220, 236, 255)),
        t if t.starts_with("Plan") => ("☰", rgb(186, 139, 255), rgb(238, 228, 255)),
        _ => ("◦", rgb(160, 160, 180), rgb(225, 225, 235)),
    }
}

pub(crate) fn render_swarm_message(
    msg: &DisplayMessage,
    width: u16,
    _diff_mode: crate::config::DiffDisplayMode,
) -> Vec<Line<'static>> {
    let centered = markdown::center_code_blocks();
    let title = msg.title.as_deref().unwrap_or("Swarm").trim();
    let content = msg.content.trim();
    let (icon, rail_color, text_color) = swarm_notification_style(msg.title.as_deref());
    let rail_style = Style::default().fg(rail_color);
    let header_style = Style::default().fg(rail_color).bold();
    let body_style = Style::default().fg(text_color);

    let content_width = if centered {
        centered_wrap_width(width.saturating_sub(6), true, 96)
    } else {
        width.saturating_sub(4) as usize
    }
    .max(1);
    let block_wrap_width = if centered {
        content_width.saturating_add(2)
    } else {
        width.saturating_sub(1) as usize
    }
    .max(1);

    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("│ ", rail_style),
        Span::styled(format!("{} {}", icon, title), header_style),
    ]));

    let mut body_lines = if content.is_empty() {
        vec![Line::from(Span::styled(String::new(), body_style))]
    } else {
        markdown::render_markdown_with_width(content, Some(content_width))
    };

    if !content.is_empty() {
        body_lines.retain(|line| {
            line.spans
                .iter()
                .any(|span| !span.content.trim().is_empty())
        });
        if body_lines.is_empty() {
            body_lines.push(Line::from(Span::styled(content.to_string(), body_style)));
        }
    }

    for line in &mut body_lines {
        if line.spans.is_empty() {
            line.spans.push(Span::styled(String::new(), body_style));
        }
        for span in &mut line.spans {
            if span.style.fg.is_none() {
                span.style.fg = Some(text_color);
            }
        }
    }

    for line in body_lines {
        let mut spans = vec![Span::styled("│ ", rail_style)];
        spans.extend(line.spans);
        lines.push(Line::from(spans));
    }

    let mut wrapped_lines = Vec::new();
    for line in lines {
        wrapped_lines.extend(markdown::wrap_line(line, block_wrap_width));
    }

    if centered {
        left_pad_lines_for_centered_mode(&mut wrapped_lines, width);
    }

    wrapped_lines
}

pub(crate) fn render_tool_message(
    msg: &DisplayMessage,
    width: u16,
    diff_mode: crate::config::DiffDisplayMode,
) -> Vec<Line<'static>> {
    if let Some(lines) = render_scheduled_tool_message(msg, width) {
        return lines;
    }

    let mut lines: Vec<Line<'static>> = Vec::new();
    let Some(ref tc) = msg.tool_data else {
        return lines;
    };

    let centered = markdown::center_code_blocks();
    let token_badge = tool_output_token_badge(&msg.content);

    if tools_ui::is_memory_store_tool(tc) && !msg.content.starts_with("Error:") {
        let content = tc
            .input
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let category = tc
            .input
            .get("category")
            .and_then(|v| v.as_str())
            .or_else(|| tc.input.get("tag").and_then(|v| v.as_str()))
            .unwrap_or("fact");
        let title = format!("🧠 saved ({}) · {}", category, token_badge.label.as_str());
        let border_style = Style::default().fg(rgb(255, 200, 100));
        let text_style = Style::default().fg(dim_color());
        let max_box = (width.saturating_sub(4) as usize).min(72);
        let inner_width = max_box.saturating_sub(4);

        let mut box_content: Vec<Line<'static>> = Vec::new();
        let text_display_width = unicode_width::UnicodeWidthStr::width(content);
        if text_display_width <= inner_width {
            box_content.push(Line::from(Span::styled(content.to_string(), text_style)));
        } else {
            for chunk in split_by_display_width(content, inner_width) {
                box_content.push(Line::from(Span::styled(chunk, text_style)));
            }
        }

        let box_lines = render_rounded_box(&title, box_content, max_box, border_style);
        for line in box_lines {
            lines.push(line);
        }
        if centered {
            left_pad_lines_for_centered_mode(&mut lines, width);
        }
        return lines;
    }

    if tools_ui::is_memory_recall_tool(tc) && !msg.content.starts_with("Error:") {
        let border_style = Style::default().fg(rgb(150, 180, 255));
        let text_style = Style::default().fg(dim_color());

        let mut entries: Vec<(String, String)> = Vec::new();
        for line in msg.content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("- [")
                && let Some(rest) = trimmed.strip_prefix("- [")
                && let Some(bracket_end) = rest.find(']')
            {
                let cat = rest[..bracket_end].to_string();
                let content = rest[bracket_end + 1..].trim();
                let content = if let Some(tag_start) = content.rfind(" [") {
                    content[..tag_start].trim()
                } else {
                    content
                };
                entries.push((cat, content.to_string()));
            }
        }

        if !entries.is_empty() {
            let count = entries.len();
            let tiles = group_into_tiles(entries);
            let header_text = format!(
                "🧠 recalled {} memor{} · {}",
                count,
                if count == 1 { "y" } else { "ies" },
                token_badge.label.as_str()
            );
            let header = Line::from(Span::styled(header_text, border_style));
            let total_width = (width.saturating_sub(4) as usize).min(120);
            let tile_lines =
                render_memory_tiles(&tiles, total_width, border_style, text_style, Some(header));
            for line in tile_lines {
                lines.push(line);
            }
            if centered {
                left_pad_lines_for_centered_mode(&mut lines, width);
            }
            return lines;
        }
    }

    let batch_counts = if tc.name == "batch" {
        tools_ui::parse_batch_completion_counts(&msg.content)
    } else {
        None
    };
    let is_error = if let Some(counts) = batch_counts {
        counts.failed > 0 && counts.succeeded == 0
    } else {
        tools_ui::tool_output_looks_failed(&msg.content)
    };
    let is_partial_batch = batch_counts
        .map(|counts| counts.failed > 0 && counts.succeeded > 0)
        .unwrap_or(false);

    let (icon, icon_color) = if is_partial_batch {
        ("⚠", rgb(214, 184, 92))
    } else if is_error {
        ("✗", rgb(220, 100, 100))
    } else {
        ("✓", rgb(100, 180, 100))
    };

    let is_edit_tool = tools_ui::is_edit_tool_name(&tc.name);
    let (additions, deletions) = if is_edit_tool {
        diff_change_counts_for_tool(tc, &msg.content)
    } else {
        (0, 0)
    };

    let block_width = if centered {
        super::centered_content_block_width(width, 96)
    } else {
        width as usize
    };
    let row_width = block_width.saturating_sub(1);
    let display_name = tools_ui::resolve_display_tool_name(&tc.name).to_string();
    let base_prefix = format!("  {} {} ", icon, display_name);
    let token_suffix_width =
        UnicodeWidthStr::width(format!(" · {}", token_badge.label.as_str()).as_str());
    let edit_suffix_width = if is_edit_tool {
        UnicodeWidthStr::width(format!(" (+{} -{})", additions, deletions).as_str())
    } else {
        0
    };
    let reserved_summary_width = row_width
        .saturating_sub(UnicodeWidthStr::width(base_prefix.as_str()))
        .saturating_sub(token_suffix_width)
        .saturating_sub(edit_suffix_width);

    let intent = tc
        .intent
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let intent_reserved_width = intent
        .map(|intent| UnicodeWidthStr::width(intent).saturating_add(3))
        .unwrap_or(0)
        .min(reserved_summary_width.saturating_sub(8));
    let technical_summary_width = reserved_summary_width.saturating_sub(intent_reserved_width);

    let summary = if let Some(counts) = batch_counts {
        if counts.failed > 0 {
            if counts.succeeded == 0 {
                format!("{}/{} failed", counts.failed, counts.total())
            } else {
                format!("{}/{} succeeded", counts.succeeded, counts.total())
            }
        } else if counts.total() == 1 {
            "1 call".to_string()
        } else {
            format!("{} calls", counts.total())
        }
    } else if let Some(error_summary) = tools_ui::concise_tool_error_summary(&msg.content) {
        error_summary
    } else if tc.name == "subagent" {
        msg.title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .map(|title| {
                super::line_plain_text(&super::truncate_line_with_ellipsis_to_width(
                    &Line::from(title.to_string()),
                    technical_summary_width,
                ))
            })
            .unwrap_or_else(|| {
                tools_ui::get_tool_summary_with_budget(tc, 50, Some(technical_summary_width))
            })
    } else {
        tools_ui::get_tool_summary_with_budget(tc, 50, Some(technical_summary_width))
    };

    let mut tool_line = vec![
        Span::styled(format!("  {} ", icon), Style::default().fg(icon_color)),
        Span::styled(display_name, Style::default().fg(tool_color())),
    ];
    if let Some(intent) = intent {
        tool_line.push(Span::styled(" · ", Style::default().fg(dim_color())));
        tool_line.push(Span::styled(
            intent.to_string(),
            Style::default().fg(tool_color()),
        ));
        if !summary.is_empty() && summary != intent {
            tool_line.push(Span::styled(" · ", Style::default().fg(dim_color())));
            tool_line.push(Span::styled(summary, Style::default().fg(dim_color())));
        }
    } else if !summary.is_empty() {
        tool_line.push(Span::styled(
            format!(" {}", summary),
            Style::default().fg(dim_color()),
        ));
    }
    if is_edit_tool {
        tool_line.push(Span::styled(" (", Style::default().fg(dim_color())));
        tool_line.push(Span::styled(
            format!("+{}", additions),
            Style::default().fg(diff_add_color()),
        ));
        tool_line.push(Span::styled(" ", Style::default().fg(dim_color())));
        tool_line.push(Span::styled(
            format!("-{}", deletions),
            Style::default().fg(diff_del_color()),
        ));
        tool_line.push(Span::styled(")", Style::default().fg(dim_color())));
    }
    let token_suffix = Line::from(vec![
        Span::styled(" · ", Style::default().fg(dim_color())),
        Span::styled(token_badge.label, Style::default().fg(token_badge.color)),
    ]);

    lines.push(super::truncate_line_preserving_suffix_to_width(
        &Line::from(tool_line),
        &token_suffix,
        row_width,
    ));

    if tc.name == "batch"
        && let Some(calls) = tc.input.get("tool_calls").and_then(|v| v.as_array())
    {
        let sub_results = tools_ui::parse_batch_sub_outputs_by_index(&msg.content);

        for (i, call) in calls.iter().enumerate() {
            let raw_name = call
                .get("tool")
                .or_else(|| call.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let params = tools_ui::batch_subcall_params(call);

            let sub_tc = ToolCall {
                id: String::new(),
                name: tools_ui::resolve_display_tool_name(raw_name).to_string(),
                input: params,
                intent: call
                    .get("intent")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            };

            let sub_result = sub_results.get(&(i + 1));
            let sub_errored = sub_result.map(|result| result.errored).unwrap_or_else(|| {
                batch_counts.is_some_and(|counts| {
                    counts.failed > 0 && counts.succeeded == 0 && counts.total() == calls.len()
                })
            });
            let (sub_icon, sub_icon_color) = if sub_errored {
                ("✗", rgb(220, 100, 100))
            } else {
                ("✓", rgb(100, 180, 100))
            };

            lines.push(tools_ui::render_batch_subcall_line(
                &sub_tc,
                sub_icon,
                sub_icon_color,
                50,
                Some(row_width),
                sub_result.map(|result| result.content.as_str()),
            ));
        }
    }

    if diff_mode.is_inline() && is_edit_tool {
        let full_inline = diff_mode.is_full_inline();
        let file_path_for_ext = tc
            .input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| {
                tc.input
                    .get("patch_text")
                    .and_then(|v| v.as_str())
                    .and_then(|patch_text| match tools_ui::canonical_tool_name(&tc.name) {
                        "apply_patch" => tools_ui::extract_apply_patch_primary_file(patch_text),
                        "patch" => tools_ui::extract_unified_patch_primary_file(patch_text),
                        _ => None,
                    })
            });
        let file_ext = file_path_for_ext
            .as_deref()
            .and_then(|p| std::path::Path::new(p).extension())
            .and_then(|e| e.to_str());

        let change_lines = {
            let from_content = collect_diff_lines(&msg.content);
            if !from_content.is_empty() {
                from_content
            } else {
                generate_diff_lines_from_tool_input(tc)
            }
        };

        const MAX_DIFF_LINES: usize = 12;
        let total_changes = change_lines.len();
        let additions = change_lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Add)
            .count();
        let deletions = change_lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Del)
            .count();

        let (display_lines, truncated, half_point): (Vec<&ParsedDiffLine>, bool, usize) =
            if full_inline || total_changes <= MAX_DIFF_LINES {
                (change_lines.iter().collect(), false, usize::MAX)
            } else {
                let half = MAX_DIFF_LINES / 2;
                let mut result: Vec<&ParsedDiffLine> = change_lines.iter().take(half).collect();
                result.extend(change_lines.iter().skip(total_changes - half));
                (result, true, half)
            };

        let pad_str = "";

        lines.push(
            Line::from(Span::styled(
                format!("{}┌─ diff", pad_str),
                Style::default().fg(dim_color()),
            ))
            .alignment(ratatui::layout::Alignment::Left),
        );

        let mut shown_truncation = false;

        for (i, line) in display_lines.iter().enumerate() {
            if truncated && !shown_truncation && i >= half_point {
                let skipped = total_changes - MAX_DIFF_LINES;
                lines.push(
                    Line::from(Span::styled(
                        format!("{}│ ... {} more changes ...", pad_str, skipped),
                        Style::default().fg(dim_color()),
                    ))
                    .alignment(ratatui::layout::Alignment::Left),
                );
                shown_truncation = true;
            }

            let base_color = if line.kind == DiffLineKind::Add {
                diff_add_color()
            } else {
                diff_del_color()
            };

            let border_prefix = format!("{}│ ", pad_str);
            let prefix_visual_width = unicode_width::UnicodeWidthStr::width(border_prefix.as_str())
                + unicode_width::UnicodeWidthStr::width(line.prefix.as_str());
            let max_content_width = (width as usize).saturating_sub(prefix_visual_width + 1);

            let mut spans: Vec<Span<'static>> = vec![
                Span::styled(border_prefix, Style::default().fg(dim_color())),
                Span::styled(line.prefix.clone(), Style::default().fg(base_color)),
            ];

            if !line.content.is_empty() {
                let content = &line.content;
                let content_vis_width = unicode_width::UnicodeWidthStr::width(content.as_str());
                if !full_inline && max_content_width > 1 && content_vis_width > max_content_width {
                    let mut end = 0;
                    let mut vis_w = 0;
                    let limit = max_content_width.saturating_sub(1);
                    for (i, ch) in content.char_indices() {
                        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                        if vis_w + cw > limit {
                            break;
                        }
                        vis_w += cw;
                        end = i + ch.len_utf8();
                    }
                    let truncated = &content[..end];
                    let highlighted = markdown::highlight_line(truncated, file_ext);
                    for span in highlighted {
                        spans.push(tint_span_with_diff_color(span, base_color));
                    }
                    spans.push(Span::styled("…", Style::default().fg(dim_color())));
                } else {
                    let highlighted = markdown::highlight_line(content.as_str(), file_ext);
                    for span in highlighted {
                        spans.push(tint_span_with_diff_color(span, base_color));
                    }
                }
            }

            lines.push(Line::from(spans).alignment(ratatui::layout::Alignment::Left));
        }

        let footer = if total_changes > 0 && truncated {
            format!("{}└─ (+{} -{} total)", pad_str, additions, deletions)
        } else {
            format!("{}└─", pad_str)
        };
        lines.push(
            Line::from(Span::styled(footer, Style::default().fg(dim_color())))
                .alignment(ratatui::layout::Alignment::Left),
        );
    }

    if centered {
        super::left_pad_lines_to_block_width(&mut lines, width, block_width);
    }

    lines
}

struct ToolOutputTokenBadge {
    label: String,
    color: Color,
}

fn tool_output_token_badge(content: &str) -> ToolOutputTokenBadge {
    let tokens = crate::util::estimate_tokens(content);
    let color = match crate::util::approx_tool_output_token_severity(tokens) {
        crate::util::ApproxTokenSeverity::Normal => rgb(118, 118, 118),
        crate::util::ApproxTokenSeverity::Warning => rgb(214, 184, 92),
        crate::util::ApproxTokenSeverity::Danger => rgb(224, 118, 118),
    };
    ToolOutputTokenBadge {
        label: crate::util::format_approx_token_count(tokens),
        color,
    }
}

fn hash_display_message(msg: &DisplayMessage) -> u64 {
    let mut hasher = DefaultHasher::new();
    msg.role.hash(&mut hasher);
    msg.content.hash(&mut hasher);
    msg.tool_calls.hash(&mut hasher);
    msg.title.hash(&mut hasher);
    if let Some(tool) = &msg.tool_data {
        tool.id.hash(&mut hasher);
        tool.name.hash(&mut hasher);
        hash_json_value(&tool.input, &mut hasher);
    }
    hasher.finish()
}

fn hash_json_value(value: &serde_json::Value, hasher: &mut DefaultHasher) {
    match value {
        serde_json::Value::Null => 0u8.hash(hasher),
        serde_json::Value::Bool(b) => {
            1u8.hash(hasher);
            b.hash(hasher);
        }
        serde_json::Value::Number(n) => {
            2u8.hash(hasher);
            n.hash(hasher);
        }
        serde_json::Value::String(s) => {
            3u8.hash(hasher);
            s.hash(hasher);
        }
        serde_json::Value::Array(arr) => {
            4u8.hash(hasher);
            arr.len().hash(hasher);
            for item in arr {
                hash_json_value(item, hasher);
            }
        }
        serde_json::Value::Object(map) => {
            5u8.hash(hasher);
            map.len().hash(hasher);
            for (k, v) in map {
                k.hash(hasher);
                hash_json_value(v, hasher);
            }
        }
    }
}

#[cfg(test)]
#[path = "ui_messages/tests.rs"]
mod tests;
