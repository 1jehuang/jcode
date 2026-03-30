use super::*;
use crate::message::parse_background_task_notification_markdown;
use std::collections::{HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MessageCacheKey {
    width: u16,
    diff_mode: crate::config::DiffDisplayMode,
    message_hash: u64,
    content_len: usize,
    diagram_mode: crate::config::DiagramDisplayMode,
    centered: bool,
}

#[derive(Default)]
struct MessageCacheState {
    entries: HashMap<MessageCacheKey, Arc<Vec<Line<'static>>>>,
    order: VecDeque<MessageCacheKey>,
}

impl MessageCacheState {
    fn get(&self, key: &MessageCacheKey) -> Option<Vec<Line<'static>>> {
        self.entries.get(key).map(|arc| arc.as_ref().clone())
    }

    fn insert(&mut self, key: MessageCacheKey, lines: Vec<Line<'static>>) {
        let arc = Arc::new(lines);
        if self.entries.contains_key(&key) {
            self.entries.insert(key, arc);
            return;
        }

        self.entries.insert(key.clone(), arc);
        self.order.push_back(key);

        while self.order.len() > MESSAGE_CACHE_LIMIT {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }
}

static MESSAGE_CACHE: OnceLock<Mutex<MessageCacheState>> = OnceLock::new();

fn message_cache() -> &'static Mutex<MessageCacheState> {
    MESSAGE_CACHE.get_or_init(|| Mutex::new(MessageCacheState::default()))
}

const MESSAGE_CACHE_LIMIT: usize = 2048;

fn left_pad_lines_for_centered_mode(lines: &mut [Line<'static>], width: u16) {
    let max_line_width = lines.iter().map(Line::width).max().unwrap_or(0);
    let pad = (width as usize).saturating_sub(max_line_width) / 2;
    if pad == 0 {
        return;
    }

    let pad_str = " ".repeat(pad);
    for line in lines {
        line.spans.insert(0, Span::raw(pad_str.clone()));
        line.alignment = Some(ratatui::layout::Alignment::Left);
    }
}

fn centered_wrap_width(width: u16, centered: bool, centered_max_width: usize) -> usize {
    let width = width as usize;
    if centered {
        width.min(centered_max_width).max(1)
    } else {
        width.max(1)
    }
}

pub(super) fn get_cached_message_lines<F>(
    msg: &DisplayMessage,
    width: u16,
    diff_mode: crate::config::DiffDisplayMode,
    render: F,
) -> Vec<Line<'static>>
where
    F: FnOnce(&DisplayMessage, u16, crate::config::DiffDisplayMode) -> Vec<Line<'static>>,
{
    if cfg!(test) {
        return render(msg, width, diff_mode);
    }

    let key = MessageCacheKey {
        width,
        diff_mode,
        message_hash: hash_display_message(msg),
        content_len: msg.content.len(),
        diagram_mode: crate::config::config().display.diagram_mode,
        centered: markdown::center_code_blocks(),
    };

    let mut cache = match message_cache().lock() {
        Ok(c) => c,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(lines) = cache.get(&key) {
        return lines;
    }

    let lines = render(msg, width, diff_mode);
    cache.insert(key, lines.clone());
    lines
}

pub(crate) fn render_assistant_message(
    msg: &DisplayMessage,
    width: u16,
    _diff_mode: crate::config::DiffDisplayMode,
) -> Vec<Line<'static>> {
    let content_width = width as usize;
    let centered = markdown::center_code_blocks();
    let mut lines = markdown::render_markdown_with_width(&msg.content, Some(content_width));
    if !msg.tool_calls.is_empty() {
        lines.extend(render_assistant_tool_call_lines(
            &msg.tool_calls,
            content_width,
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

    let centered = markdown::center_code_blocks();
    let wrap_width = centered_wrap_width(width.saturating_sub(4), centered, 96);
    let mut lines = markdown::render_markdown_with_width(&msg.content, Some(wrap_width));
    if lines.iter().any(|line| line.width() > wrap_width) {
        lines = msg
            .content
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

    let mut lines = render_rounded_box("⚡ reload", box_content, max_box_width, border_style);
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
                "⚡ reconnecting",
                rgb(255, 193, 94),
                rgb(255, 220, 140),
                status_line,
                Some(detail),
                hint,
            )
        } else if let Some((status_line, detail, hint)) = parse_connection_waiting_message(content)
        {
            (
                "⚡ waiting for reload",
                rgb(120, 180, 255),
                rgb(180, 215, 255),
                status_line,
                Some(detail),
                hint,
            )
        } else if content.starts_with("⏳ Starting server") {
            (
                "⏳ starting server",
                rgb(255, 193, 94),
                rgb(255, 220, 140),
                "Starting shared server".to_string(),
                None,
                None,
            )
        } else {
            let mut lines = markdown::render_markdown_with_width(content, Some(inner_width));
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

    let mut lines = render_rounded_box(&title, box_content, max_box_width, border_style);
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
    let Some(parsed) = parse_background_task_notification_markdown(&msg.content) else {
        return render_system_message(msg, width, diff_mode);
    };

    let centered = markdown::center_code_blocks();
    let (title, border_color, status_color, preview_color) = if parsed.status.starts_with('✓') {
        (
            format!("✓ bg {} completed · {}", parsed.tool_name, parsed.task_id),
            rgb(100, 180, 100),
            rgb(120, 210, 140),
            rgb(214, 240, 220),
        )
    } else if parsed.status.starts_with('✗') {
        (
            format!("✗ bg {} failed · {}", parsed.tool_name, parsed.task_id),
            rgb(220, 100, 100),
            rgb(255, 150, 150),
            rgb(255, 225, 225),
        )
    } else {
        (
            format!("◌ bg {} running · {}", parsed.tool_name, parsed.task_id),
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

    if centered {
        left_pad_lines_for_centered_mode(&mut lines, width);
    }

    lines
}

pub(crate) fn render_tool_message(
    msg: &DisplayMessage,
    width: u16,
    diff_mode: crate::config::DiffDisplayMode,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let Some(ref tc) = msg.tool_data else {
        return lines;
    };

    let centered = markdown::center_code_blocks();

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
        let title = format!("🧠 saved ({})", category);
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
        return lines;
    }

    if tools_ui::is_memory_recall_tool(tc) && !msg.content.starts_with("Error:") {
        let border_style = Style::default().fg(rgb(150, 180, 255));
        let text_style = Style::default().fg(dim_color());

        let mut entries: Vec<(String, String)> = Vec::new();
        for line in msg.content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("- [") {
                if let Some(rest) = trimmed.strip_prefix("- [") {
                    if let Some(bracket_end) = rest.find(']') {
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
            }
        }

        if !entries.is_empty() {
            let count = entries.len();
            let tiles = group_into_tiles(entries);
            let header_text = format!(
                "🧠 recalled {} memor{}",
                count,
                if count == 1 { "y" } else { "ies" }
            );
            let header = Line::from(Span::styled(header_text, border_style));
            let total_width = (width.saturating_sub(4) as usize).min(120);
            let tile_lines =
                render_memory_tiles(&tiles, total_width, border_style, text_style, Some(header));
            for line in tile_lines {
                lines.push(line);
            }
            return lines;
        }
    }

    let is_error = msg.content.starts_with("Error:")
        || msg.content.starts_with("error:")
        || msg.content.starts_with("Failed:");

    let (icon, icon_color) = if is_error {
        ("✗", rgb(220, 100, 100))
    } else {
        ("✓", rgb(100, 180, 100))
    };

    let row_width = width.saturating_sub(1) as usize;
    let reserved_summary_width = row_width.saturating_sub(UnicodeWidthStr::width(
        format!("  {} {} ", icon, tc.name).as_str(),
    ));

    let summary = if tc.name == "subagent" {
        msg.title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .map(|title| {
                super::line_plain_text(&super::truncate_line_with_ellipsis_to_width(
                    &Line::from(title.to_string()),
                    reserved_summary_width,
                ))
            })
            .unwrap_or_else(|| {
                tools_ui::get_tool_summary_with_budget(tc, 50, Some(reserved_summary_width))
            })
    } else {
        tools_ui::get_tool_summary_with_budget(tc, 50, Some(reserved_summary_width))
    };

    let is_edit_tool = matches!(
        tc.name.as_str(),
        "edit" | "Edit" | "write" | "multiedit" | "patch" | "Patch" | "apply_patch" | "ApplyPatch"
    );
    let (additions, deletions) = if is_edit_tool {
        diff_change_counts_for_tool(tc, &msg.content)
    } else {
        (0, 0)
    };

    let mut tool_line = vec![
        Span::styled(format!("  {} ", icon), Style::default().fg(icon_color)),
        Span::styled(tc.name.clone(), Style::default().fg(tool_color())),
        Span::styled(format!(" {}", summary), Style::default().fg(dim_color())),
    ];
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

    lines.push(super::truncate_line_with_ellipsis_to_width(
        &Line::from(tool_line),
        row_width,
    ));

    if tc.name == "batch" {
        if let Some(calls) = tc.input.get("tool_calls").and_then(|v| v.as_array()) {
            let sub_results = tools_ui::parse_batch_sub_results(&msg.content);

            for (i, call) in calls.iter().enumerate() {
                let raw_name = call
                    .get("tool")
                    .or_else(|| call.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let params = tools_ui::batch_subcall_params(call);

                let sub_tc = ToolCall {
                    id: String::new(),
                    name: tools_ui::resolve_display_tool_name(raw_name).to_string(),
                    input: params,
                    intent: None,
                };

                let sub_errored = sub_results.get(i).copied().unwrap_or(false);
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
                ));
            }
        }
    }

    if diff_mode.is_inline() && is_edit_tool {
        let file_path_for_ext = tc
            .input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| {
                tc.input
                    .get("patch_text")
                    .and_then(|v| v.as_str())
                    .and_then(|patch_text| match tc.name.as_str() {
                        "apply_patch" | "ApplyPatch" => {
                            tools_ui::extract_apply_patch_primary_file(patch_text)
                        }
                        "patch" | "Patch" => {
                            tools_ui::extract_unified_patch_primary_file(patch_text)
                        }
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

        let (display_lines, truncated): (Vec<&ParsedDiffLine>, bool) =
            if total_changes <= MAX_DIFF_LINES {
                (change_lines.iter().collect(), false)
            } else {
                let half = MAX_DIFF_LINES / 2;
                let mut result: Vec<&ParsedDiffLine> = change_lines.iter().take(half).collect();
                result.extend(change_lines.iter().skip(total_changes - half));
                (result, true)
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
        let half_point = if truncated {
            MAX_DIFF_LINES / 2
        } else {
            usize::MAX
        };

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
                if max_content_width > 1 && content_vis_width > max_content_width {
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
        left_pad_lines_for_centered_mode(&mut lines, width);
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_system_message_forces_system_color_on_all_spans() {
        let msg = DisplayMessage::system("**Reload complete** — continuing.");

        let lines = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off);

        assert!(!lines.is_empty(), "expected rendered system message lines");
        for line in lines {
            for span in line.spans {
                assert_eq!(span.style.fg, Some(system_message_color()));
            }
        }
    }

    #[test]
    fn render_system_message_centered_mode_left_aligns_with_padding() {
        let saved = crate::tui::markdown::center_code_blocks();
        crate::tui::markdown::set_center_code_blocks(true);
        let msg = DisplayMessage::system("Reload complete — continuing.");

        let lines = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off);

        assert!(!lines.is_empty(), "expected rendered system message lines");
        for line in &lines {
            assert_eq!(
                line.alignment,
                Some(ratatui::layout::Alignment::Left),
                "centered system lines should be left-aligned with padding"
            );
            assert!(
                line.spans
                    .first()
                    .is_some_and(|span| span.content.starts_with(' ')),
                "centered system lines should start with padding"
            );
        }
        crate::tui::markdown::set_center_code_blocks(saved);
    }

    #[test]
    fn render_background_task_message_uses_box_and_truncates_preview_lines() {
        let msg = DisplayMessage::background_task(
            "**Background task** `bg123` · `bash` · ✓ completed · 7.1s · exit 0\n\n```text\nline 1\nline 2\nline 3\nline 4\nline 5\n```\n\n_Full output:_ `bg action=\"output\" task_id=\"bg123\"`",
        );

        let lines = render_background_task_message(&msg, 80, crate::config::DiffDisplayMode::Off);
        let plain = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(plain.contains("✓ bg bash completed · bg123"));
        assert!(plain.contains("exit 0 · 7.1s"));
        assert!(plain.contains("line 1"));
        assert!(plain.contains("… +1 more line"));
        assert!(!plain.contains("task bg123 · bash"));
        assert!(!plain.contains("Preview"));
        assert!(!plain.contains("Full output"));
        assert!(!plain.contains("bg action=\"output\" task_id=\"bg123\""));
    }

    #[test]
    fn render_assistant_message_truncates_tool_calls_to_single_line() {
        let saved = crate::tui::markdown::center_code_blocks();
        crate::tui::markdown::set_center_code_blocks(false);
        let msg = DisplayMessage {
            role: "assistant".to_string(),
            content: "Done.".to_string(),
            tool_calls: vec![
                "read".to_string(),
                "grep".to_string(),
                "apply_patch".to_string(),
                "batch".to_string(),
            ],
            duration_secs: None,
            title: None,
            tool_data: None,
        };

        let lines = render_assistant_message(&msg, 20, crate::config::DiffDisplayMode::Off);
        let tool_lines: Vec<String> = lines
            .iter()
            .skip(1)
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();

        assert!(
            tool_lines.len() == 1,
            "expected single-line tool-call summary: {tool_lines:?}"
        );
        assert!(
            tool_lines[0].contains("tools:"),
            "expected tool summary label on first line: {tool_lines:?}"
        );
        assert!(
            tool_lines.iter().all(|line| line.width() <= 20),
            "tool-call summary line should respect available width: {tool_lines:?}"
        );
        crate::tui::markdown::set_center_code_blocks(saved);
    }

    #[test]
    fn render_assistant_message_centers_single_line_tool_summary() {
        let saved = crate::tui::markdown::center_code_blocks();
        crate::tui::markdown::set_center_code_blocks(true);
        let msg = DisplayMessage {
            role: "assistant".to_string(),
            content: "Done.".to_string(),
            tool_calls: vec![
                "read".to_string(),
                "grep".to_string(),
                "apply_patch".to_string(),
                "batch".to_string(),
            ],
            duration_secs: None,
            title: None,
            tool_data: None,
        };

        let lines = render_assistant_message(&msg, 28, crate::config::DiffDisplayMode::Off);
        let tool_lines: Vec<String> = lines
            .iter()
            .skip(1)
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();

        assert!(
            tool_lines.len() == 1,
            "expected single-line tool-call summary: {tool_lines:?}"
        );
        let first_pad = tool_lines[0].chars().take_while(|c| *c == ' ').count();
        assert!(
            first_pad > 0,
            "tool summary should still be padded/centered as a block: {tool_lines:?}"
        );

        crate::tui::markdown::set_center_code_blocks(saved);
    }

    #[test]
    fn render_system_message_centered_mode_caps_wrap_width_for_visible_gutters() {
        let saved = crate::tui::markdown::center_code_blocks();
        crate::tui::markdown::set_center_code_blocks(true);
        let msg = DisplayMessage::system(
            "This is a long centered-mode system notification that should keep visible side gutters instead of stretching nearly edge to edge in a wide terminal.",
        );

        let lines = render_system_message(&msg, 120, crate::config::DiffDisplayMode::Off);
        let rendered: Vec<String> = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();

        assert!(
            rendered.iter().all(|line| line.starts_with("          ")),
            "centered system message should retain visible left padding in wide layouts: {rendered:?}"
        );

        crate::tui::markdown::set_center_code_blocks(saved);
    }

    #[test]
    fn render_system_message_uses_reload_card_for_reload_title() {
        let msg =
            DisplayMessage::system("Reloading server with newer binary...").with_title("Reload");

        let lines = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off);
        let plain = lines
            .iter()
            .map(extract_line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            plain.contains("reload"),
            "expected reload card title: {plain}"
        );
        assert!(plain.contains("Reloading server with newer binary"));
    }

    #[test]
    fn render_system_message_uses_connection_card_for_reconnect_status() {
        let msg = DisplayMessage::system(
            "⚡ Connection lost — retrying (attempt 2, 7s) — connection reset by server · resume: jcode --resume koala",
        )
        .with_title("Connection");

        let lines = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off);
        let plain = lines
            .iter()
            .map(extract_line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            plain.contains("reconnecting"),
            "expected reconnect card title: {plain}"
        );
        assert!(plain.contains("Retrying · attempt 2 · 7s"));
        assert!(plain.contains("connection reset by server"));
        assert!(plain.contains("jcode --resume koala"));
    }

    #[test]
    fn render_swarm_message_centered_mode_caps_wrap_width_for_long_notifications() {
        let saved = crate::tui::markdown::center_code_blocks();
        crate::tui::markdown::set_center_code_blocks(true);
        let msg = DisplayMessage::swarm(
            "File activity",
            "/home/jeremy/jcode/src/tui/ui_messages.rs — moss just edited this file while you were working nearby, so the notification should still read as centered in wide layouts.",
        );

        let lines = render_swarm_message(&msg, 120, crate::config::DiffDisplayMode::Off);
        let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();
        let first_pad = rendered[0].chars().take_while(|c| *c == ' ').count();

        assert!(
            first_pad >= 8,
            "centered swarm notification should keep a clearly visible left gutter: {rendered:?}"
        );
        assert!(
            rendered
                .iter()
                .all(|line| line.is_empty() || line.starts_with(&" ".repeat(first_pad))),
            "centered swarm notification should share one left pad across wrapped lines: {rendered:?}"
        );

        crate::tui::markdown::set_center_code_blocks(saved);
    }

    #[test]
    fn render_tool_message_prefers_subagent_title_with_model() {
        let msg = DisplayMessage {
            role: "tool".to_string(),
            content: "done".to_string(),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: Some("Verify subagent model (general · gpt-5.4)".to_string()),
            tool_data: Some(crate::message::ToolCall {
                id: "call_1".to_string(),
                name: "subagent".to_string(),
                input: serde_json::json!({
                    "description": "Verify subagent model",
                    "subagent_type": "general"
                }),
                intent: None,
            }),
        };

        let lines = render_tool_message(&msg, 80, crate::config::DiffDisplayMode::Off);
        let rendered: String = lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();

        assert!(rendered.contains("subagent Verify subagent model (general · gpt-5.4)"));
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
