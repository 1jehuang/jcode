use super::*;
#[path = "ui_messages_cache.rs"]
mod cache_support;
use crate::message::{
    ParsedBackgroundTaskNotification, ParsedBackgroundTaskProgressNotification,
    parse_background_task_notification_markdown,
    parse_background_task_progress_notification_markdown, strip_ansi_escape_sequences,
};
pub(super) use cache_support::get_cached_message_lines;
use cache_support::{centered_wrap_width, left_pad_lines_for_centered_mode};
use std::borrow::Cow;
use unicode_width::UnicodeWidthStr;

const MAX_INLINE_DIFF_LINES: usize = 12;
const MAX_DISCOVERY_DETAIL_LINES: usize = 2;
const MAX_DISCOVERY_SETUP_LINES: usize = 3;
const MAX_DISCOVERY_LISTING_ENTRIES: usize = 4;

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
    let sanitized = strip_ansi_escape_sequences(content);
    if !prefer_width_stable_system_glyphs() {
        return Cow::Owned(sanitized);
    }

    let normalized = sanitized
        .replace("⚡ ", "! ")
        .replace("⏳ ", "... ")
        .replace("⏰ ", "* ");
    Cow::Owned(normalized)
}

fn render_single_line_system_notice(
    msg: &DisplayMessage,
    width: u16,
) -> Option<Vec<Line<'static>>> {
    let is_launch_hotkey_notice = msg.title.as_deref() == Some("Launch hotkeys");
    let is_update_divergence_notice = msg.content.starts_with("Update diverged. Press ")
        || msg.content.starts_with("Local and upstream have diverged");
    let is_cold_cache_notice = msg.content.starts_with("🧊 Prompt cache went cold");
    if !is_cold_cache_notice && !is_launch_hotkey_notice && !is_update_divergence_notice {
        return None;
    }

    let centered = markdown::center_code_blocks();
    let line_width = centered_wrap_width(width.saturating_sub(4), centered, 96);
    let display_content = normalize_system_content_for_display(&msg.content);
    let compact = display_content
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let line = Line::from(Span::styled(compact, system_message_color()));
    let mut lines = vec![super::truncate_line_with_ellipsis_to_width(
        &line, line_width,
    )];
    if centered {
        left_pad_lines_for_centered_mode(&mut lines, width);
    }
    for span in &mut lines[0].spans {
        span.style.fg = Some(system_message_color());
    }
    Some(lines)
}

pub(crate) fn render_assistant_message(
    msg: &DisplayMessage,
    width: u16,
    _diff_mode: crate::config::DiffDisplayMode,
) -> Vec<Line<'static>> {
    let centered = markdown::center_code_blocks();
    let wrap_width = centered_wrap_width(width, centered, 96);
    let mut lines = if let Some(segments) = split_plan_segments(&msg.content) {
        render_assistant_segments(&segments, width, wrap_width)
    } else {
        markdown::render_markdown_with_width(&msg.content, Some(wrap_width))
    };
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

/// One piece of an assistant message that contains ```plan fenced blocks:
/// either ordinary markdown text or the inner markdown of a plan block.
#[derive(Debug, PartialEq, Eq)]
enum AssistantSegment {
    Markdown(String),
    Plan(String),
}

/// Split assistant content into markdown/plan segments when it contains at
/// least one ```plan fenced block. Returns `None` when there is no plan block
/// so the common path stays on the plain markdown renderer.
fn split_plan_segments(content: &str) -> Option<Vec<AssistantSegment>> {
    if !content.contains("```plan") {
        return None;
    }

    let mut segments: Vec<AssistantSegment> = Vec::new();
    let mut current = String::new();
    let mut plan_body: Option<String> = None;
    let mut plan_nested_fence = false;
    let mut in_other_fence = false;
    let mut saw_plan = false;

    for line in content.split('\n') {
        let trimmed = line.trim_start();
        if let Some(body) = plan_body.as_mut() {
            let is_fence_line = trimmed.starts_with("```");
            let is_bare_fence = is_fence_line && trimmed.trim_end() == "```";
            if is_bare_fence && !plan_nested_fence {
                let body = plan_body.take().unwrap_or_default();
                segments.push(AssistantSegment::Plan(body));
            } else {
                if is_fence_line {
                    // A nested fenced block inside the plan (e.g. ```bash ...
                    // ```). Its closing bare fence must not end the plan.
                    plan_nested_fence = !plan_nested_fence;
                }
                if !body.is_empty() {
                    body.push('\n');
                }
                body.push_str(line);
            }
            continue;
        }

        if !in_other_fence
            && trimmed
                .strip_prefix("```plan")
                .is_some_and(|rest| rest.trim().is_empty())
        {
            saw_plan = true;
            plan_nested_fence = false;
            if !current.trim().is_empty() {
                segments.push(AssistantSegment::Markdown(std::mem::take(&mut current)));
            } else {
                current.clear();
            }
            plan_body = Some(String::new());
            continue;
        }

        if trimmed.starts_with("```") {
            in_other_fence = !in_other_fence;
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }

    // Unterminated plan fence (e.g. mid-stream): render what we have as a card.
    if let Some(body) = plan_body.take() {
        segments.push(AssistantSegment::Plan(body));
    }
    if !current.trim().is_empty() {
        segments.push(AssistantSegment::Markdown(current));
    }

    saw_plan.then_some(segments)
}

fn render_assistant_segments(
    segments: &[AssistantSegment],
    width: u16,
    wrap_width: usize,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    for segment in segments {
        match segment {
            AssistantSegment::Markdown(text) => {
                if !lines.is_empty() {
                    lines.push(Line::from(""));
                }
                lines.extend(markdown::render_markdown_with_width(text, Some(wrap_width)));
            }
            AssistantSegment::Plan(body) => {
                if !lines.is_empty() {
                    lines.push(Line::from(""));
                }
                lines.extend(render_plan_card(body, width));
            }
        }
    }
    lines
}

/// Render the inner markdown of a ```plan block as a bordered plan card.
fn render_plan_card(body: &str, width: u16) -> Vec<Line<'static>> {
    let border_style = Style::default().fg(rgb(158, 135, 255));
    let max_box_width = (width.saturating_sub(4) as usize).clamp(28, 100);
    let inner_width = max_box_width.saturating_sub(4).max(8);

    let title = plan_card_title(body);
    let body_without_title = plan_card_body_without_title(body, &title);

    // `render_markdown_with_width` sizes block elements (code, tables, rules)
    // but does not hard-wrap paragraph text; the normal message path wraps
    // later in the pipeline. The card boxes its content immediately and
    // `render_rounded_box` truncates over-long lines, so wrap here to avoid
    // cutting plan text off at the border.
    let rendered = markdown::render_markdown_with_width(&body_without_title, Some(inner_width));
    let mut content: Vec<Line<'static>> = markdown::wrap_lines(rendered, inner_width);
    // Trim leading/trailing blank rows inside the card.
    while content.first().is_some_and(|line| line.width() == 0) {
        content.remove(0);
    }
    while content.last().is_some_and(|line| line.width() == 0) {
        content.pop();
    }
    if content.is_empty() {
        content.push(Line::from(Span::styled(
            "(empty plan)",
            Style::default().fg(dim_color()),
        )));
    }

    render_rounded_box(&title, content, max_box_width, border_style)
}

/// Title for the plan card: the first markdown heading in the body, else "Plan".
fn plan_card_title(body: &str) -> String {
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed
            .strip_prefix("# ")
            .or_else(|| trimmed.strip_prefix("## "))
            .or_else(|| trimmed.strip_prefix("### "))
        {
            let heading = heading.trim();
            if !heading.is_empty() {
                return format!("⛭ {}", heading);
            }
        }
        if !trimmed.is_empty() {
            break;
        }
    }
    "⛭ Plan".to_string()
}

/// Remove the first heading line when it was promoted to the card title.
fn plan_card_body_without_title(body: &str, title: &str) -> String {
    if title == "⛭ Plan" {
        return body.to_string();
    }
    let mut removed = false;
    body.lines()
        .filter(|line| {
            if removed {
                return true;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return true;
            }
            if trimmed.starts_with('#') {
                removed = true;
                return false;
            }
            removed = true;
            true
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render a collapsed/collapsing reasoning trace ("current" mode). The content is
/// sentinel-wrapped dim+italic markup (reasoning lines and/or a `▸ thought for Xs`
/// summary), so it reuses the standard markdown path that styles those runs dim.
pub(crate) fn render_reasoning_message(
    msg: &DisplayMessage,
    width: u16,
    _diff_mode: crate::config::DiffDisplayMode,
) -> Vec<Line<'static>> {
    let centered = markdown::center_code_blocks();
    let wrap_width = centered_wrap_width(width, centered, 96);
    let mut lines = markdown::render_markdown_with_width(&msg.content, Some(wrap_width));
    if centered {
        left_pad_lines_for_centered_mode(&mut lines, width);
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

/// Wrap plaintext content into lines without any markdown interpretation.
///
/// System messages are status/notice text and must render verbatim (no bold,
/// headings, list bullets, code fences, etc.). This word-wraps on whitespace
/// and hard-splits tokens that are wider than `wrap_width`, preserving the
/// author's own line breaks and leading indentation. Wrapped continuation
/// lines are hang-indented to match the original line's indentation so that
/// authored plaintext layout (indented commands, aligned blocks) survives.
fn render_plaintext_lines(content: &str, wrap_width: usize) -> Vec<Line<'static>> {
    let wrap_width = wrap_width.max(1);
    let mut lines: Vec<Line<'static>> = Vec::new();

    for raw_line in content.split('\n') {
        let raw_line = raw_line.trim_end_matches('\r');
        if raw_line.trim().is_empty() {
            lines.push(Line::from(String::new()));
            continue;
        }

        // Preserve the line's leading indentation (tabs normalized to spaces).
        // Wrapped continuation lines reuse this indent so authored plaintext
        // layout (indented commands, aligned blocks) survives. If the indent
        // leaves no room for content, fall back to no continuation indent.
        let indent: String = raw_line
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .map(|c| if c == '\t' { ' ' } else { c })
            .collect();
        let indent_width = indent.width();
        let cont_indent = if indent_width < wrap_width {
            indent.as_str()
        } else {
            ""
        };
        let body = raw_line.trim_start_matches([' ', '\t']);

        // Width available to content on each wrapped line.
        let avail = wrap_width.saturating_sub(cont_indent.width()).max(1);

        // `current` always begins with the active indent; `content_width`
        // tracks how much real content sits after that indent on this visual
        // line (so we know whether to insert a separating space and when to
        // wrap).
        let mut current = indent.clone();
        let mut content_width = 0usize;

        for word in body.split_whitespace() {
            let word_width = word.width();

            // Hard-split a token wider than the available content width.
            if word_width > avail {
                if content_width > 0 {
                    lines.push(Line::from(std::mem::take(&mut current)));
                }
                for chunk in split_by_display_width(word, avail) {
                    lines.push(Line::from(format!("{}{}", cont_indent, chunk)));
                }
                current = cont_indent.to_string();
                content_width = 0;
                continue;
            }

            let sep = if content_width > 0 { 1 } else { 0 };
            if content_width > 0 && content_width + sep + word_width > avail {
                lines.push(Line::from(std::mem::take(&mut current)));
                current = cont_indent.to_string();
                content_width = 0;
            }
            if content_width > 0 {
                current.push(' ');
                content_width += 1;
            }
            current.push_str(word);
            content_width += word_width;
        }

        lines.push(Line::from(current));
    }

    if lines.is_empty() {
        lines.push(Line::from(String::new()));
    }
    lines
}

/// Render the full agentgrep tool output inline beneath the tool summary line.
/// Each output line is prefixed with a dim left border and indented so it reads
/// as a nested block. Long lines are hard-split to the available width and the
/// block is capped so a giant search result cannot flood the transcript.
fn render_agentgrep_output_body(content: &str, row_width: usize) -> Vec<Line<'static>> {
    const MAX_BODY_LINES: usize = 400;
    let border = "    │ ";
    let border_width = UnicodeWidthStr::width(border);
    let avail = row_width.saturating_sub(border_width).max(1);

    let mut out: Vec<Line<'static>> = Vec::new();
    let source_lines: Vec<&str> = content.split('\n').collect();
    let total = source_lines.len();
    let mut truncated_extra = 0usize;

    for raw_line in source_lines {
        if out.len() >= MAX_BODY_LINES {
            truncated_extra = total.saturating_sub(out.len());
            break;
        }
        let raw_line = raw_line.trim_end_matches('\r');
        if raw_line.is_empty() {
            out.push(Line::from(Span::styled(
                border.to_string(),
                Style::default().fg(dim_color()),
            )));
            continue;
        }
        if UnicodeWidthStr::width(raw_line) <= avail {
            out.push(Line::from(vec![
                Span::styled(border.to_string(), Style::default().fg(dim_color())),
                Span::styled(raw_line.to_string(), Style::default().fg(dim_color())),
            ]));
        } else {
            for chunk in split_by_display_width(raw_line, avail) {
                if out.len() >= MAX_BODY_LINES {
                    break;
                }
                out.push(Line::from(vec![
                    Span::styled(border.to_string(), Style::default().fg(dim_color())),
                    Span::styled(chunk, Style::default().fg(dim_color())),
                ]));
            }
        }
    }

    if truncated_extra > 0 {
        out.push(Line::from(Span::styled(
            format!("    │ … {} more lines …", truncated_extra),
            Style::default().fg(dim_color()),
        )));
    }

    out
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

    if msg
        .content
        .starts_with("⚡ Server reload in progress - waiting for handoff")
        || msg.content.starts_with("⚡ Connection lost - retrying")
    {
        return render_connection_system_message(msg, width);
    }

    // Compact notices are transient status summaries. Keep them scannable and
    // prevent a narrow viewport from turning one notice into a paragraph.
    if let Some(lines) = render_single_line_system_notice(msg, width) {
        return lines;
    }

    if let Some(lines) = render_scheduled_session_message(msg, width) {
        return lines;
    }

    let centered = markdown::center_code_blocks();
    let wrap_width = centered_wrap_width(width.saturating_sub(4), centered, 96);
    let display_content = normalize_system_content_for_display(&msg.content);
    // Authored summaries that use markdown (bold/lists/headings/links) render as
    // markdown so they read cleanly. Plain status/help text keeps the original
    // line-oriented plaintext path, which preserves authored indentation and
    // wraps long lines to width: markdown parsing would otherwise strip leading
    // indentation and leave long paragraphs unwrapped (stretching edge to edge).
    // Either way, color is forced to the system color so output stays distinct.
    let mut lines = if content_has_markdown_formatting(&display_content) {
        // Keep single newlines as hard breaks rather than letting markdown
        // collapse them into one paragraph, then wrap to width so long lines
        // still respect the layout/gutters.
        let hard_broken = preserve_hard_line_breaks_for_markdown(&display_content);
        let rendered = markdown::render_markdown_with_width(&hard_broken, Some(wrap_width));
        markdown::wrap_lines(rendered, wrap_width)
    } else {
        render_plaintext_lines(&display_content, wrap_width)
    };
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

/// Heuristic: does authored system content use markdown formatting that is
/// worth rendering (bold/italic, inline code, headings, lists, links,
/// blockquotes, fenced code, tables)?
///
/// Plain status/help text (no markdown) keeps the original plaintext path so
/// authored indentation is preserved and long lines wrap to width. We only opt
/// into markdown when a marker is actually present, which avoids regressing
/// indented/aligned output that markdown parsing would otherwise flatten.
fn content_has_markdown_formatting(content: &str) -> bool {
    // Inline markers that can appear anywhere on a line.
    if content.contains("**")
        || content.contains("__")
        || content.contains('`')
        || content.contains("](")
    {
        return true;
    }
    // Block markers only count at the start of a (trimmed) line.
    content.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("# ")
            || trimmed.starts_with("## ")
            || trimmed.starts_with("### ")
            || trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed.starts_with("+ ")
            || trimmed.starts_with("> ")
            || trimmed.starts_with("```")
            || trimmed.starts_with("~~~")
            || trimmed.starts_with('|')
            || trimmed.split_once('.').is_some_and(|(num, rest)| {
                !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) && rest.starts_with(' ')
            })
    })
}

/// Convert single newlines in authored system content into markdown hard line
/// breaks (a trailing `  ` before the newline) so the renderer keeps each
/// source line on its own row instead of reflowing them into one paragraph.
///
/// Blank-line paragraph boundaries are left untouched, and lines that already
/// belong to block constructs (list items, headings, fenced code, blockquotes,
/// tables) are not given a hard break since markdown already breaks on them.
fn preserve_hard_line_breaks_for_markdown(content: &str) -> Cow<'_, str> {
    if !content.contains('\n') {
        return Cow::Borrowed(content);
    }

    let lines: Vec<&str> = content.split('\n').collect();
    let mut out = String::with_capacity(content.len() + lines.len() * 2);
    let mut in_fence = false;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
        }
        out.push_str(line);

        let is_last = idx + 1 == lines.len();
        if is_last {
            continue;
        }
        let next = lines[idx + 1];
        let next_trimmed = next.trim_start();
        // Don't touch paragraph breaks (blank line follows), fenced code, or the
        // current/next line being a markdown block construct that already forces
        // its own line.
        let current_blank = line.trim().is_empty();
        let next_blank = next.trim().is_empty();
        let next_is_block = next_trimmed.starts_with('#')
            || next_trimmed.starts_with("- ")
            || next_trimmed.starts_with("* ")
            || next_trimmed.starts_with("+ ")
            || next_trimmed.starts_with("> ")
            || next_trimmed.starts_with('|')
            || next_trimmed
                .split_once('.')
                .is_some_and(|(num, _)| !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()));
        if in_fence || current_blank || next_blank || next_is_block {
            out.push('\n');
        } else {
            // Hard break: two trailing spaces before the newline.
            out.push_str("  \n");
        }
    }
    Cow::Owned(out)
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

pub(crate) fn render_overnight_message(
    msg: &DisplayMessage,
    width: u16,
    diff_mode: crate::config::DiffDisplayMode,
) -> Vec<Line<'static>> {
    let Ok(card) = serde_json::from_str::<crate::overnight::OvernightProgressCard>(&msg.content)
    else {
        return render_system_message(msg, width, diff_mode);
    };

    let centered = markdown::center_code_blocks();
    let (icon, border_color, status_color, text_color) = match card.status.as_str() {
        "completed" => (
            "✓",
            rgb(90, 190, 120),
            rgb(130, 225, 155),
            rgb(220, 246, 226),
        ),
        "failed" => (
            "✗",
            rgb(220, 100, 100),
            rgb(255, 150, 150),
            rgb(255, 225, 225),
        ),
        "cancel requested" | "cancelling" => (
            "◌",
            rgb(255, 193, 94),
            rgb(255, 214, 120),
            rgb(255, 241, 214),
        ),
        _ => (
            "◌",
            rgb(158, 135, 255),
            rgb(198, 184, 255),
            rgb(232, 228, 255),
        ),
    };
    let border_style = Style::default().fg(border_color);
    let status_style = Style::default().fg(status_color).bold();
    let text_style = Style::default().fg(text_color);
    let label_style = Style::default().fg(dim_color());
    let dim_style = Style::default().fg(dim_color()).dim();
    let filled_style = Style::default().fg(status_color);
    let empty_style = Style::default().fg(rgb(70, 68, 95));

    let max_box_width = if centered {
        (width.saturating_sub(4) as usize).min(120)
    } else {
        (width.saturating_sub(2) as usize).min(100)
    }
    .max(28);
    let inner_width = max_box_width.saturating_sub(4).max(1);
    let short_run_id = compact_run_id(&card.run_id);
    let title = format!("{} overnight · {} · {}", icon, card.phase, short_run_id);

    let mut box_content = vec![render_overnight_progress_line(
        &card,
        inner_width,
        filled_style,
        empty_style,
        label_style,
        status_style,
    )];

    push_wrapped_kv_line(
        &mut box_content,
        "Target",
        &format!("{} · {}", card.time_relation, card.target_wake_at),
        inner_width,
        label_style,
        text_style,
    );
    push_wrapped_kv_line(
        &mut box_content,
        "Coordinator",
        &format!(
            "{} ({})",
            card.coordinator_session_id, card.coordinator_session_name
        ),
        inner_width,
        label_style,
        text_style,
    );
    push_wrapped_kv_line(
        &mut box_content,
        "Last activity",
        &format!(
            "{} · next: {}",
            card.last_activity_label, card.next_prompt_label
        ),
        inner_width,
        label_style,
        text_style,
    );
    push_wrapped_kv_line(
        &mut box_content,
        "Tasks",
        &format_overnight_task_counts(&card),
        inner_width,
        label_style,
        text_style,
    );
    if let Some(active) = card
        .active_task_title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        push_wrapped_kv_line(
            &mut box_content,
            "Current",
            active,
            inner_width,
            label_style,
            text_style,
        );
    }
    push_wrapped_kv_line(
        &mut box_content,
        "Usage",
        &format!(
            "{} risk, {} confidence · {}",
            card.usage_risk, card.usage_confidence, card.usage_projection
        ),
        inner_width,
        label_style,
        text_style,
    );
    push_wrapped_kv_line(
        &mut box_content,
        "Resources",
        &card.resources_summary,
        inner_width,
        label_style,
        text_style,
    );
    if let Some(summary) = card
        .latest_event_summary
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        let latest = card
            .latest_event_kind
            .as_deref()
            .map(|kind| format!("{}: {}", kind, summary))
            .unwrap_or_else(|| summary.to_string());
        push_wrapped_kv_line(
            &mut box_content,
            "Latest",
            &latest,
            inner_width,
            label_style,
            text_style,
        );
    }
    push_wrapped_kv_line(
        &mut box_content,
        "Review",
        &format!("{} · log: {}", card.review_path, card.log_path),
        inner_width,
        label_style,
        dim_style,
    );

    let mut lines = render_rounded_box(&title, box_content, max_box_width, border_style);
    if centered {
        left_pad_lines_for_centered_mode(&mut lines, width);
    }
    lines
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum TodoCardPayload {
    Current {
        #[serde(default)]
        todos: Vec<crate::todo::TodoItem>,
        #[serde(default)]
        goals: Vec<crate::todo::TodoGoal>,
    },
    Legacy(Vec<crate::todo::TodoItem>),
}

// Todo cards sit directly on the terminal background, so the global
// `dim_color()` (RGB 80) is too faint for meaningful metadata. Keep a compact
// semantic palette here: cool colors describe structure/state, while amber is
// reserved for priority and blocked work.
fn todo_group_color() -> Color {
    rgb(190, 165, 235)
}

fn todo_label_color() -> Color {
    rgb(145, 155, 175)
}

fn todo_meta_color() -> Color {
    rgb(155, 165, 180)
}

fn todo_score_color() -> Color {
    rgb(105, 205, 165)
}

fn todo_confidence_color() -> Color {
    rgb(135, 155, 180)
}

impl TodoCardPayload {
    fn into_parts(self) -> (Vec<crate::todo::TodoItem>, Vec<crate::todo::TodoGoal>) {
        match self {
            Self::Current { todos, goals } => (todos, goals),
            Self::Legacy(todos) => (todos, Vec::new()),
        }
    }
}

struct ParsedTodoToolOutput {
    todos: Vec<crate::todo::TodoItem>,
    goals: Vec<crate::todo::TodoGoal>,
    goal_updates: Vec<crate::todo::TodoGoalChange>,
}

fn parse_todo_tool_output(content: &str) -> Option<ParsedTodoToolOutput> {
    // Remote display and timestamp injection can decorate tool results before
    // they reach this renderer. Keep that transport metadata outside the
    // structured payload parser so a valid todo result still renders as a card.
    let content = strip_todo_tool_output_headers(content);
    let mut todo_stream =
        serde_json::Deserializer::from_str(content).into_iter::<Vec<crate::todo::TodoItem>>();
    let todos = todo_stream.next()?.ok()?;
    let mut remainder = content.get(todo_stream.byte_offset()..)?.trim_start();
    let goals = if let Some(goal_json) = remainder.strip_prefix("Goals:") {
        let mut goal_stream = serde_json::Deserializer::from_str(goal_json.trim_start())
            .into_iter::<Vec<crate::todo::TodoGoal>>();
        let goals = goal_stream.next().and_then(Result::ok).unwrap_or_default();
        remainder = goal_json
            .trim_start()
            .get(goal_stream.byte_offset()..)
            .unwrap_or_default()
            .trim_start();
        goals
    } else {
        Vec::new()
    };
    let goal_updates = if let Some(update_json) = remainder.strip_prefix("Goal updates:") {
        let mut update_stream = serde_json::Deserializer::from_str(update_json.trim_start())
            .into_iter::<Vec<crate::todo::TodoGoalChange>>();
        update_stream
            .next()
            .and_then(Result::ok)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    Some(ParsedTodoToolOutput {
        todos,
        goals,
        goal_updates,
    })
}

fn strip_todo_tool_output_headers(content: &str) -> &str {
    let mut content = content.trim_start();
    // Remote clients prefix outputs with the tool name, while restored history
    // may independently prefix timing metadata. Accept either order without
    // weakening the JSON shape that follows.
    for _ in 0..3 {
        if let Some(rest) = strip_todo_tool_name_header(content) {
            content = rest;
            continue;
        }
        let rest = strip_tool_result_timestamp_header(content);
        if rest.len() < content.len() {
            content = rest;
            continue;
        }
        break;
    }
    content
}

fn strip_todo_tool_name_header(content: &str) -> Option<&str> {
    let after_open = content.strip_prefix('[')?;
    let header_end = after_open.find(']')?;
    let name = after_open[..header_end].trim();
    (tools_ui::canonical_tool_name(name) == "todo")
        .then(|| after_open[header_end + 1..].trim_start())
}

fn strip_tool_result_timestamp_header(content: &str) -> &str {
    let trimmed = content.trim_start();
    let Some(after_open) = trimmed.strip_prefix('[') else {
        return content;
    };
    let Some(header_end) = after_open.find(']') else {
        return content;
    };
    let header = &after_open[..header_end];
    let is_timing_header = header.starts_with("tool timing:");
    let is_timestamp_header = chrono::DateTime::parse_from_rfc3339(header).is_ok();
    if !is_timing_header && !is_timestamp_header {
        return content;
    }
    after_open[header_end + 1..].trim_start()
}

/// Render the inline todo-list card (`role == "todos"`). New payloads contain
/// both todo items and goal assessments; the legacy item-array shape remains
/// supported so older transcript entries keep rendering.
pub(crate) fn render_todos_message(
    msg: &DisplayMessage,
    width: u16,
    diff_mode: crate::config::DiffDisplayMode,
) -> Vec<Line<'static>> {
    let Ok(payload) = serde_json::from_str::<TodoCardPayload>(&msg.content) else {
        return render_system_message(msg, width, diff_mode);
    };
    let (todos, goals) = payload.into_parts();

    let centered = markdown::center_code_blocks();
    let meta_style = Style::default().fg(todo_meta_color());
    let card_width = if centered {
        (width.saturating_sub(4) as usize).min(120)
    } else {
        (width.saturating_sub(2) as usize).min(100)
    }
    .max(1);
    let base_indent = if centered { "" } else { "  " };
    let inner_width = card_width.saturating_sub(base_indent.width()).max(1);

    let mut lines = Vec::new();
    if todos.is_empty() {
        lines.push(todo_card_line(
            vec![Span::styled(
                "No tasks yet. The model populates them as work is planned.",
                meta_style,
            )],
            base_indent,
            inner_width,
        ));
    } else {
        // Partition into first-seen-order groups (ungrouped bucket last). When
        // no todo declares a group, keep a flat list without headers.
        let group_of = |todo: &crate::todo::TodoItem| -> Option<String> {
            todo.group
                .as_deref()
                .map(str::trim)
                .filter(|g| !g.is_empty())
                .map(str::to_string)
        };
        let has_groups = todos.iter().any(|t| group_of(t).is_some());
        if has_groups {
            let mut groups: Vec<(Option<String>, Vec<&crate::todo::TodoItem>)> = Vec::new();
            for todo in &todos {
                let key = group_of(todo);
                if let Some(entry) = groups.iter_mut().find(|(existing, _)| *existing == key) {
                    entry.1.push(todo);
                } else {
                    groups.push((key, vec![todo]));
                }
            }
            groups.sort_by_key(|(key, _)| key.is_none());
            for (group, items) in &groups {
                let label = group.as_deref().unwrap_or("other");
                let goal = todo_card_goal_for_group(&goals, group.as_deref());
                lines.push(render_todo_goal_header(
                    label,
                    items,
                    base_indent,
                    inner_width,
                ));
                push_todo_goal_details(&mut lines, goal, base_indent, inner_width);
                for todo in items {
                    lines.push(render_todo_card_item_line(todo, base_indent, inner_width));
                }
            }
        } else {
            let goal = todo_card_goal_for_group(&goals, None);
            lines.push(render_todo_status_header(
                todos.iter(),
                base_indent,
                inner_width,
            ));
            if goal.is_some() {
                push_todo_goal_details(&mut lines, goal, base_indent, inner_width);
            }
            for todo in &todos {
                lines.push(render_todo_card_item_line(todo, base_indent, inner_width));
            }
        }
    }

    if centered {
        left_pad_lines_for_centered_mode(&mut lines, width);
    }
    lines
}

fn todo_card_line(
    spans: Vec<Span<'static>>,
    base_indent: &str,
    inner_width: usize,
) -> Line<'static> {
    let mut prefixed = vec![Span::raw(base_indent.to_string())];
    prefixed.extend(spans);
    super::truncate_line_with_ellipsis_to_width(
        &Line::from(prefixed),
        inner_width.saturating_add(base_indent.width()),
    )
}

fn todo_card_goal_for_group<'a>(
    goals: &'a [crate::todo::TodoGoal],
    group: Option<&str>,
) -> Option<&'a crate::todo::TodoGoal> {
    let key = group.map(str::trim).filter(|value| !value.is_empty());
    goals.iter().find(|goal| {
        goal.group
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            == key
    })
}

fn todo_goal_score_spans(goal: Option<&crate::todo::TodoGoal>) -> Vec<Span<'static>> {
    let Some(goal) = goal else {
        return Vec::new();
    };
    let mut spans = Vec::new();
    for (label, score) in [
        ("User intention alignment", goal.user_intention_alignment),
        ("Hill climbability", goal.hill_climbability),
        ("Ownership", goal.end_to_end_ownership),
    ] {
        let Some(score) = score else {
            continue;
        };
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", Style::default().fg(dim_color())));
        }
        spans.push(Span::styled(
            format!("{} ", label),
            Style::default().fg(todo_label_color()),
        ));
        spans.push(Span::styled(
            format!("{}%", score),
            Style::default().fg(todo_score_color()),
        ));
    }
    spans
}

fn push_todo_status_pips<'a>(
    spans: &mut Vec<Span<'static>>,
    todos: impl IntoIterator<Item = &'a crate::todo::TodoItem>,
    max_pips: usize,
) {
    let (completed, in_progress, total) =
        todos
            .into_iter()
            .fold((0usize, 0usize, 0usize), |counts, todo| {
                (
                    counts.0 + usize::from(todo.status == "completed"),
                    counts.1 + usize::from(todo.status == "in_progress"),
                    counts.2 + 1,
                )
            });
    if total == 0 || max_pips == 0 {
        return;
    }

    let (done_pips, active_pips, open_pips) = if total <= max_pips.max(12) {
        (
            completed,
            in_progress,
            total.saturating_sub(completed + in_progress),
        )
    } else {
        let scale =
            |count: usize| ((count as f64 / total as f64) * max_pips as f64).round() as usize;
        let mut done = scale(completed);
        let mut active = scale(in_progress);
        if completed > 0 && done == 0 {
            done = 1;
        }
        if in_progress > 0 && active == 0 {
            active = 1;
        }
        done = done.min(max_pips);
        active = active.min(max_pips.saturating_sub(done));
        (done, active, max_pips.saturating_sub(done + active))
    };

    for _ in 0..done_pips {
        spans.push(Span::styled("●", Style::default().fg(rgb(100, 180, 100))));
    }
    for _ in 0..active_pips {
        spans.push(Span::styled("●", Style::default().fg(asap_color())));
    }
    for _ in 0..open_pips {
        spans.push(Span::styled("○", Style::default().fg(rgb(90, 90, 105))));
    }
}

fn render_todo_status_header<'a>(
    todos: impl IntoIterator<Item = &'a crate::todo::TodoItem>,
    base_indent: &str,
    inner_width: usize,
) -> Line<'static> {
    let mut spans = Vec::new();
    push_todo_status_pips(&mut spans, todos, inner_width);
    todo_card_line(spans, base_indent, inner_width)
}

fn render_todo_goal_header(
    label: &str,
    todos: &[&crate::todo::TodoItem],
    base_indent: &str,
    inner_width: usize,
) -> Line<'static> {
    let label_width = label.width();
    let mut spans = vec![Span::styled(
        label.to_string(),
        Style::default().fg(todo_group_color()).bold(),
    )];
    spans.push(Span::raw("  "));
    push_todo_status_pips(
        &mut spans,
        todos.iter().copied(),
        inner_width.saturating_sub(label_width + 2),
    );
    todo_card_line(spans, base_indent, inner_width)
}

fn wrap_todo_detail(value: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut chunks = Vec::new();
    let mut current = String::new();

    for word in value.split_whitespace() {
        let word_width = word.width();
        if !current.is_empty() && current.width() + 1 + word_width <= width {
            current.push(' ');
            current.push_str(word);
            continue;
        }
        if current.is_empty() && word_width <= width {
            current.push_str(word);
            continue;
        }
        if !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
        }
        if word_width <= width {
            current.push_str(word);
            continue;
        }
        let mut word_chunks = split_by_display_width(word, width).into_iter().peekable();
        while let Some(chunk) = word_chunks.next() {
            if word_chunks.peek().is_some() {
                chunks.push(chunk);
            } else {
                current = chunk;
            }
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn push_todo_goal_details(
    lines: &mut Vec<Line<'static>>,
    goal: Option<&crate::todo::TodoGoal>,
    base_indent: &str,
    inner_width: usize,
) {
    let Some(goal) = goal else {
        return;
    };
    let scores = todo_goal_score_spans(Some(goal));
    if !scores.is_empty() {
        let score_width = Line::from(scores.clone()).width();
        let score_count = [
            goal.user_intention_alignment,
            goal.hill_climbability,
            goal.end_to_end_ownership,
        ]
        .into_iter()
        .flatten()
        .count();
        if score_width > inner_width.saturating_sub(2) && score_count > 1 {
            for (label, score) in [
                ("User intention alignment", goal.user_intention_alignment),
                ("Hill climbability", goal.hill_climbability),
                ("Ownership", goal.end_to_end_ownership),
            ] {
                let Some(score) = score else {
                    continue;
                };
                let mut spans = vec![Span::raw("  ")];
                spans.push(Span::styled(
                    format!("{} ", label),
                    Style::default().fg(todo_label_color()),
                ));
                spans.push(Span::styled(
                    format!("{}%", score),
                    Style::default().fg(todo_score_color()),
                ));
                lines.push(todo_card_line(spans, base_indent, inner_width));
            }
        } else {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(scores);
            lines.push(todo_card_line(spans, base_indent, inner_width));
        }
    }
    for (label, value) in [
        ("User intention", goal.user_intention.as_deref()),
        ("Objective", goal.objective.as_deref()),
        ("Feedback", goal.feedback_loop.as_deref()),
    ] {
        let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
            continue;
        };
        let prefix = format!("  {} · ", label);
        let prefix_width = prefix.width();
        let available = inner_width.saturating_sub(prefix_width).max(1);
        for (index, chunk) in wrap_todo_detail(value, available).into_iter().enumerate() {
            lines.push(todo_card_line(
                vec![
                    Span::styled(
                        if index == 0 {
                            prefix.clone()
                        } else {
                            " ".repeat(prefix_width)
                        },
                        Style::default().fg(todo_label_color()),
                    ),
                    Span::styled(chunk, Style::default().fg(todo_meta_color())),
                ],
                base_indent,
                inner_width,
            ));
        }
    }
}

fn render_todo_goal_updates(
    updates: &[crate::todo::TodoGoalChange],
    width: u16,
) -> Vec<Line<'static>> {
    let centered = markdown::center_code_blocks();
    let card_width = if centered {
        (width.saturating_sub(4) as usize).min(120)
    } else {
        (width.saturating_sub(2) as usize).min(100)
    }
    .max(1);
    let base_indent = if centered { "" } else { "  " };
    let inner_width = card_width.saturating_sub(base_indent.width()).max(1);
    let mut lines = Vec::new();

    for update in updates {
        let goal = update.after.as_ref().or(update.before.as_ref());
        let label = goal
            .and_then(|goal| goal.group.as_deref())
            .map(str::trim)
            .filter(|group| !group.is_empty())
            .unwrap_or("Goal");
        lines.push(todo_card_line(
            vec![
                Span::styled(
                    label.to_string(),
                    Style::default().fg(todo_group_color()).bold(),
                ),
                Span::styled("  updated", Style::default().fg(todo_meta_color())),
            ],
            base_indent,
            inner_width,
        ));

        for field in &update.fields {
            match field {
                crate::todo::TodoGoalField::UserIntentionAlignment => push_todo_score_update(
                    &mut lines,
                    "User intention alignment",
                    update
                        .before
                        .as_ref()
                        .and_then(|goal| goal.user_intention_alignment),
                    update
                        .after
                        .as_ref()
                        .and_then(|goal| goal.user_intention_alignment),
                    base_indent,
                    inner_width,
                ),
                crate::todo::TodoGoalField::HillClimbability => push_todo_score_update(
                    &mut lines,
                    "Hill climbability",
                    update
                        .before
                        .as_ref()
                        .and_then(|goal| goal.hill_climbability),
                    update
                        .after
                        .as_ref()
                        .and_then(|goal| goal.hill_climbability),
                    base_indent,
                    inner_width,
                ),
                crate::todo::TodoGoalField::EndToEndOwnership => push_todo_score_update(
                    &mut lines,
                    "Ownership",
                    update
                        .before
                        .as_ref()
                        .and_then(|goal| goal.end_to_end_ownership),
                    update
                        .after
                        .as_ref()
                        .and_then(|goal| goal.end_to_end_ownership),
                    base_indent,
                    inner_width,
                ),
                crate::todo::TodoGoalField::UserIntention => push_todo_text_update(
                    &mut lines,
                    "User intention",
                    update
                        .after
                        .as_ref()
                        .and_then(|goal| goal.user_intention.as_deref()),
                    base_indent,
                    inner_width,
                ),
                crate::todo::TodoGoalField::Objective => push_todo_text_update(
                    &mut lines,
                    "Objective",
                    update
                        .after
                        .as_ref()
                        .and_then(|goal| goal.objective.as_deref()),
                    base_indent,
                    inner_width,
                ),
                crate::todo::TodoGoalField::FeedbackLoop => push_todo_text_update(
                    &mut lines,
                    "Feedback",
                    update
                        .after
                        .as_ref()
                        .and_then(|goal| goal.feedback_loop.as_deref()),
                    base_indent,
                    inner_width,
                ),
            }
        }
    }

    if centered {
        left_pad_lines_for_centered_mode(&mut lines, width);
    }
    lines
}

fn push_todo_score_update(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    before: Option<u8>,
    after: Option<u8>,
    base_indent: &str,
    inner_width: usize,
) {
    let mut spans = vec![
        Span::raw("  "),
        Span::styled(
            format!("{} ", label),
            Style::default().fg(todo_label_color()),
        ),
    ];
    match (before, after) {
        (Some(before), Some(after)) => {
            spans.push(Span::styled(
                format!("{}%", before),
                Style::default().fg(todo_meta_color()),
            ));
            spans.push(Span::styled(" → ", Style::default().fg(todo_label_color())));
            spans.push(Span::styled(
                format!("{}%", after),
                Style::default().fg(todo_score_color()),
            ));
        }
        (None, Some(after)) => spans.push(Span::styled(
            format!("{}%", after),
            Style::default().fg(todo_score_color()),
        )),
        (_, None) => spans.push(Span::styled(
            "cleared",
            Style::default().fg(todo_meta_color()),
        )),
    }
    lines.push(todo_card_line(spans, base_indent, inner_width));
}

fn push_todo_text_update(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    after: Option<&str>,
    base_indent: &str,
    inner_width: usize,
) {
    let value = after.map(str::trim).filter(|value| !value.is_empty());
    let prefix = format!("  {} · ", label);
    let prefix_width = prefix.width();
    let available = inner_width.saturating_sub(prefix_width).max(1);
    let chunks = value
        .map(|value| wrap_todo_detail(value, available))
        .filter(|chunks| !chunks.is_empty())
        .unwrap_or_else(|| vec!["cleared".to_string()]);
    for (index, chunk) in chunks.into_iter().enumerate() {
        lines.push(todo_card_line(
            vec![
                Span::styled(
                    if index == 0 {
                        prefix.clone()
                    } else {
                        " ".repeat(prefix_width)
                    },
                    Style::default().fg(todo_label_color()),
                ),
                Span::styled(chunk, Style::default().fg(todo_meta_color())),
            ],
            base_indent,
            inner_width,
        ));
    }
}

fn todo_card_confidence_label(todo: &crate::todo::TodoItem) -> Option<String> {
    if todo.status == "completed"
        && let (Some(planning), Some(completed)) = (todo.confidence, todo.completion_confidence)
        && planning != completed
    {
        return Some(format!("{}→{}%", planning, completed));
    }
    let score = if todo.status == "completed" {
        todo.completion_confidence.or(todo.confidence)
    } else {
        todo.confidence
    };
    score.map(|score| format!("{}%", score))
}

fn render_todo_card_item_line(
    todo: &crate::todo::TodoItem,
    base_indent: &str,
    inner_width: usize,
) -> Line<'static> {
    let blocked = !todo.blocked_by.is_empty() && todo.status != "completed";
    let (glyph, glyph_color) = if blocked {
        ("⊳", rgb(225, 165, 90))
    } else {
        match todo.status.as_str() {
            "completed" => ("✓", rgb(105, 190, 125)),
            "in_progress" => ("●", asap_color()),
            "cancelled" => ("✗", rgb(190, 105, 115)),
            _ => ("○", rgb(135, 145, 160)),
        }
    };
    let text_color = match todo.status.as_str() {
        "completed" => rgb(135, 150, 145),
        "cancelled" => rgb(145, 130, 135),
        "in_progress" => rgb(225, 232, 240),
        _ => rgb(195, 202, 212),
    };
    let mut spans = vec![
        Span::raw("  "),
        Span::styled(format!("{} ", glyph), Style::default().fg(glyph_color)),
        Span::styled(todo.content.clone(), Style::default().fg(text_color)),
    ];
    if let Some(label) = todo_card_confidence_label(todo) {
        spans.push(Span::styled(
            format!(" · {}", label),
            Style::default().fg(todo_confidence_color()),
        ));
    }
    todo_card_line(spans, base_indent, inner_width)
}

fn compact_run_id(run_id: &str) -> String {
    if run_id.width() <= 22 {
        run_id.to_string()
    } else {
        let prefix: String = run_id.chars().take(18).collect();
        format!("{}…", prefix)
    }
}

fn render_overnight_progress_line(
    card: &crate::overnight::OvernightProgressCard,
    inner_width: usize,
    filled_style: Style,
    empty_style: Style,
    label_style: Style,
    text_style: Style,
) -> Line<'static> {
    let percent = card.progress_percent.clamp(0.0, 100.0);
    let label = format!("{:>3}%", percent.round() as u32);
    let summary = format!("{} / {}", card.elapsed_label, card.target_duration_label);
    let separator = " · ";
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
        Span::styled(summary, text_style),
    ]);
    super::truncate_line_with_ellipsis_to_width(&line, inner_width)
}

fn push_wrapped_kv_line(
    content: &mut Vec<Line<'static>>,
    label: &str,
    value: &str,
    inner_width: usize,
    label_style: Style,
    value_style: Style,
) {
    let prefix = format!("{}: ", label);
    let prefix_width = prefix.width();
    let available = inner_width.saturating_sub(prefix_width).max(1);
    let chunks = split_by_display_width(value.trim(), available);
    if chunks.is_empty() {
        return;
    }
    for (idx, chunk) in chunks.into_iter().enumerate() {
        if idx == 0 {
            content.push(super::truncate_line_with_ellipsis_to_width(
                &Line::from(vec![
                    Span::styled(prefix.clone(), label_style),
                    Span::styled(chunk, value_style),
                ]),
                inner_width,
            ));
        } else {
            content.push(super::truncate_line_with_ellipsis_to_width(
                &Line::from(vec![
                    Span::styled(" ".repeat(prefix_width), label_style),
                    Span::styled(chunk, value_style),
                ]),
                inner_width,
            ));
        }
    }
}

fn format_overnight_task_counts(card: &crate::overnight::OvernightProgressCard) -> String {
    let counts = &card.task_summary.counts;
    format!(
        "{} complete, {} active, {} blocked, {} deferred · {} total, {} validated",
        counts.completed,
        counts.active,
        counts.blocked,
        counts.deferred,
        card.task_summary.total,
        card.task_summary.validated
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
    } else {
        let rest = first_line.strip_prefix("Scheduled ambient task ")?;
        let (id, when) = rest.split_once(" for ")?;
        (when.trim().to_string(), Some(id.trim().to_string()))
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

fn truncate_connection_line(input: &str, width: usize) -> String {
    if input.chars().count() <= width {
        return input.to_string();
    }
    if width <= 1 {
        return "…".to_string();
    }
    let mut out: String = input.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn parse_connection_retry_message(content: &str) -> Option<(String, String, Option<String>)> {
    let rest = content.strip_prefix("⚡ Connection lost - retrying (attempt ")?;
    let (attempt_and_elapsed, detail) = rest.split_once(") - ")?;
    let (attempt, elapsed) = attempt_and_elapsed
        .split_once(", ")
        .or_else(|| attempt_and_elapsed.split_once(", in "))?;
    let (detail, hint) = split_resume_hint(detail);
    Some((
        format!("Retrying · attempt {} · {}", attempt.trim(), elapsed.trim()),
        detail.to_string(),
        hint.map(str::to_string),
    ))
}

fn parse_connection_waiting_message(content: &str) -> Option<(String, String, Option<String>)> {
    let rest = content.strip_prefix("⚡ Server reload in progress - waiting for handoff (")?;
    let (elapsed, detail) = rest.split_once(") - ")?;
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
            // System messages render as plaintext, never markdown.
            let mut lines = render_plaintext_lines(&display_content, inner_width);
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
        let detail = truncate_connection_line(&detail.replace('\n', " "), inner_width);
        box_content.push(Line::from(vec![
            Span::styled("Detail ", label_style),
            Span::styled(detail, body_style),
        ]));
    }

    if let Some(hint) = hint.filter(|hint| !hint.is_empty()) {
        let hint = truncate_connection_line(&hint.replace('\n', " "), inner_width);
        box_content.push(Line::from(vec![
            Span::styled("Resume ", label_style),
            Span::styled(hint, hint_style),
        ]));
    }

    box_content.truncate(3);

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
    if msg
        .content
        .trim_start()
        .starts_with("🐝 **Swarm await finished**")
    {
        return render_compact_swarm_await(
            "🐝 Swarm await",
            &compact_swarm_await_summary(&msg.content),
            width,
        )
        .unwrap_or_default();
    }
    if let Some(progress) = parse_background_task_progress_notification_markdown(&msg.content) {
        if progress.tool_name == "swarm" {
            return render_compact_swarm_background_progress(&progress, width);
        }
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
    let is_swarm = parsed.tool_name == "swarm";
    if is_swarm {
        return render_compact_swarm_background_completion(&parsed, width);
    }
    let (title, border_color, status_color, preview_color) = if parsed.status.starts_with('✓') {
        (
            if is_swarm {
                format!("🐝 {} completed · {}", task_label, parsed.task_id)
            } else {
                format!("✓ bg {} completed · {}", task_label, parsed.task_id)
            },
            rgb(100, 180, 100),
            rgb(120, 210, 140),
            rgb(214, 240, 220),
        )
    } else if parsed.status.starts_with('✗') {
        (
            if is_swarm {
                format!("🐝 {} failed · {}", task_label, parsed.task_id)
            } else {
                format!("✗ bg {} failed · {}", task_label, parsed.task_id)
            },
            rgb(220, 100, 100),
            rgb(255, 150, 150),
            rgb(255, 225, 225),
        )
    } else {
        (
            if is_swarm {
                format!("🐝 {} running · {}", task_label, parsed.task_id)
            } else {
                format!("◌ bg {} running · {}", task_label, parsed.task_id)
            },
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
        let failure_summary = strip_ansi_escape_sequences(failure_summary);
        box_content.push(Line::from(""));
        box_content.push(Line::from(Span::styled("Failure", label_style)));
        for chunk in split_by_display_width(&failure_summary, inner_width) {
            box_content.push(Line::from(Span::styled(chunk, status_style)));
        }
    }

    box_content.push(Line::from(""));

    match parsed.preview.as_deref() {
        Some(preview) => {
            let preview = strip_ansi_escape_sequences(preview);
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

fn compact_swarm_operation_label(label: &str) -> String {
    label
        .split_once(" (")
        .map(|(label, _)| label)
        .unwrap_or(label)
        .replace('_', " ")
}

fn compact_swarm_progress_fraction(summary: &str) -> Option<&str> {
    summary.split_whitespace().find(|part| {
        let part = part.trim_matches(|ch: char| !ch.is_ascii_digit() && ch != '/');
        let Some((done, total)) = part.split_once('/') else {
            return false;
        };
        !done.is_empty()
            && !total.is_empty()
            && done.chars().all(|ch| ch.is_ascii_digit())
            && total.chars().all(|ch| ch.is_ascii_digit())
    })
}

fn render_compact_swarm_background_progress(
    progress: &ParsedBackgroundTaskProgressNotification,
    width: u16,
) -> Vec<Line<'static>> {
    let label = compact_swarm_operation_label(&crate::message::background_task_display_label(
        &progress.tool_name,
        progress.display_name.as_deref(),
    ));
    let fraction = compact_swarm_progress_fraction(&progress.summary)
        .map(|value| value.trim_matches(|ch: char| !ch.is_ascii_digit() && ch != '/'));
    let text = fraction
        .map(|fraction| format!("● {label} · {fraction}"))
        .unwrap_or_else(|| format!("● {label}"));
    render_compact_swarm_operation_line(&text, width, rgb(255, 214, 120))
}

fn render_compact_swarm_background_completion(
    parsed: &ParsedBackgroundTaskNotification,
    width: u16,
) -> Vec<Line<'static>> {
    let label = compact_swarm_operation_label(&crate::message::background_task_display_label(
        &parsed.tool_name,
        parsed.display_name.as_deref(),
    ));
    let (text, color) = if parsed.status.starts_with('✓') {
        (
            format!("✓ {label} · {}", parsed.duration),
            rgb(120, 210, 140),
        )
    } else if parsed.status.starts_with('✗') {
        (
            format!("✗ {label} · failed after {}", parsed.duration),
            rgb(255, 150, 150),
        )
    } else {
        (
            format!("● {label} · {}", parsed.duration),
            rgb(255, 214, 120),
        )
    };
    render_compact_swarm_operation_line(&text, width, color)
}

fn render_compact_swarm_operation_line(text: &str, width: u16, color: Color) -> Vec<Line<'static>> {
    let mut lines = vec![super::truncate_line_with_ellipsis_to_width(
        &Line::from(vec![
            Span::styled("🐝 ", Style::default().fg(rgb(255, 200, 100))),
            Span::styled(text.to_string(), Style::default().fg(color)),
        ]),
        width.max(1) as usize,
    )];
    if markdown::center_code_blocks() {
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
    let is_model_refresh =
        progress.task_id == "refresh-model-list" && progress.tool_name == "catalog";
    let title = if is_model_refresh {
        format!("◌ model refresh · {}", task_label)
    } else if progress.tool_name == "swarm" {
        format!("🐝 {} · {}", task_label, progress.task_id)
    } else {
        format!("◌ bg {} · {}", task_label, progress.task_id)
    };

    let mut box_content = vec![render_compact_progress_line(
        progress,
        inner_width,
        filled_style,
        empty_style,
        label_style,
        text_style,
    )];
    if !is_model_refresh {
        let hint = format!(
            "Latest status: bg action=\"status\" task_id=\"{}\"",
            progress.task_id
        );
        box_content.push(super::truncate_line_with_ellipsis_to_width(
            &Line::from(Span::styled(hint, label_style)),
            inner_width,
        ));
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
        // U+2261 IDENTICAL TO, not U+2630 TRIGRAM FOR HEAVEN: the trigram
        // changed from narrow to wide in Unicode 16, so terminals with newer
        // width tables (kitty >= 0.40) render it 2 cells wide while
        // unicode-width crates pinned to older Unicode call it 1. That one-cell
        // disagreement shears every row it appears on (issue seen 2026-07-02:
        // info-widget borders pushed off-screen). Stick to glyphs whose width
        // is stable across Unicode versions.
        t if t.starts_with("Plan") => ("≡", rgb(186, 139, 255), rgb(238, 228, 255)),
        _ => ("◦", rgb(160, 160, 180), rgb(225, 225, 235)),
    }
}

/// Trailing badge text appended to a collapsed swarm tldr line. Kept as
/// constants so click hit-testing and rendering stay in sync.
pub(crate) const SWARM_EXPAND_BADGE: &str = "▸ expand";
pub(crate) const SWARM_COLLAPSE_BADGE: &str = "▾ collapse";
pub(crate) const SWARM_DIFF_EXPAND_BADGE: &str = "▸ diff";
pub(crate) const SWARM_DIFF_COLLAPSE_BADGE: &str = "▾ hide";
pub(crate) const SWARM_AGENT_SNAPSHOT_TITLE: &str = "swarm-agent-snapshot";

pub(crate) fn compact_swarm_await_summary(message: &str) -> String {
    let body = message
        .trim()
        .strip_prefix("🐝 **Swarm await finished**")
        .map(str::trim)
        .unwrap_or_else(|| message.trim());
    let mut done = 0usize;
    let mut total = 0usize;
    let mut in_statuses = false;

    for line in body.lines() {
        let line = line.trim();
        if line == "Member statuses:" {
            in_statuses = true;
            continue;
        }
        if in_statuses && (line == "Completion reports:" || line.ends_with("reports:")) {
            break;
        }
        if !in_statuses || line.is_empty() {
            continue;
        }
        if line.starts_with('✓') {
            done += 1;
            total += 1;
        } else if line.starts_with('✗') {
            total += 1;
        }
    }

    if total > 0 {
        if done == total {
            format!("✓ {done}/{total}")
        } else {
            format!("{done}/{total} finished")
        }
    } else {
        "finished".to_string()
    }
}

pub(crate) fn encode_swarm_agent_snapshot(
    member: &crate::protocol::SwarmMemberStatus,
) -> Option<String> {
    serde_json::to_string(member).ok()
}

struct CompactSwarmNotification<'a> {
    sender: &'a str,
    marker: String,
    marker_before_icon: bool,
    text_color: Color,
    file_activity: bool,
}

fn compact_swarm_notification(title: &str) -> Option<CompactSwarmNotification<'_>> {
    let (sender, marker, marker_before_icon, text_color, file_activity) =
        if let Some(sender) = title.strip_prefix("DM from ") {
            (sender, String::new(), false, rgb(214, 232, 255), false)
        } else if let Some(sender) = title.strip_prefix("Task · ") {
            (sender, String::new(), false, rgb(220, 236, 255), false)
        } else if let Some((channel, sender)) = title
            .strip_prefix('#')
            .and_then(|rest| rest.rsplit_once(" · "))
        {
            (
                sender,
                format!("#{channel} · "),
                false,
                rgb(214, 247, 244),
                false,
            )
        } else if let Some(sender) = title.strip_prefix("Broadcast · ") {
            (sender, "📣 ".to_string(), false, rgb(255, 240, 214), false)
        } else if let Some(sender) = title.strip_prefix("Shared context · ") {
            (sender, "🧠 ".to_string(), false, rgb(221, 247, 232), false)
        } else if let Some(sender) = title.strip_prefix("File activity · ") {
            (sender, "✎ ".to_string(), false, rgb(255, 228, 214), true)
        } else if let Some(sender) = title.strip_prefix("File conflict · ") {
            (sender, "⚠ ".to_string(), true, rgb(255, 190, 150), true)
        } else {
            let sender = title.strip_prefix("Swarm · ")?;
            (sender, String::new(), false, rgb(225, 225, 235), false)
        };
    let sender = sender.trim();
    (!sender.is_empty()).then_some(CompactSwarmNotification {
        sender,
        marker,
        marker_before_icon,
        text_color,
        file_activity,
    })
}

fn render_compact_agent_notification(
    title: &str,
    content: &str,
    width: u16,
) -> Option<Vec<Line<'static>>> {
    let notification = compact_swarm_notification(title)?;
    let icon = crate::id::session_icon(notification.sender);
    let collapsible = jcode_tui_messages::parse_collapsible_swarm_content(content);
    let (body, badge) = match collapsible {
        Some(parsed) if parsed.expanded => (
            format!("{}\n{}", parsed.tldr, parsed.body.trim()),
            Some(if notification.file_activity {
                SWARM_DIFF_COLLAPSE_BADGE
            } else {
                SWARM_COLLAPSE_BADGE
            }),
        ),
        Some(parsed) => (
            parsed.tldr.to_string(),
            Some(if notification.file_activity {
                SWARM_DIFF_EXPAND_BADGE
            } else {
                SWARM_EXPAND_BADGE
            }),
        ),
        None => (content.trim().to_string(), None),
    };
    let body = if title.starts_with("Shared context · ") {
        body.replace(" = ", " · ")
    } else {
        body
    };
    let text_color = notification.text_color;
    let icon_style = Style::default().fg(rgb(255, 200, 100));
    let body_style = Style::default().fg(text_color);
    let max_width = width.max(1) as usize;
    let body_width = max_width.saturating_sub(3).max(1);
    let mut rendered_body = if body.is_empty() {
        vec![Line::from(Span::styled(String::new(), body_style))]
    } else {
        markdown::render_markdown_with_width(&body, Some(body_width))
    };
    if rendered_body.is_empty() {
        rendered_body.push(Line::from(Span::styled(body, body_style)));
    }

    let mut lines = Vec::new();
    for (index, mut line) in rendered_body.into_iter().enumerate() {
        for span in &mut line.spans {
            if span.style.fg.is_none() {
                span.style.fg = Some(text_color);
            }
        }
        let mut spans = vec![if index == 0 && notification.marker_before_icon {
            Span::styled(format!("{}{} ", notification.marker, icon), body_style)
        } else if index == 0 {
            Span::styled(format!("{} ", icon), icon_style)
        } else {
            Span::raw("   ")
        }];
        if index == 0 && !notification.marker.is_empty() && !notification.marker_before_icon {
            spans.push(Span::styled(notification.marker.clone(), body_style));
        }
        spans.extend(line.spans);
        if index == 0
            && let Some(badge) = badge
        {
            spans.push(Span::styled(
                format!("  {badge}"),
                Style::default().fg(text_color).dim(),
            ));
        }
        lines.push(Line::from(spans));
    }

    if markdown::center_code_blocks() {
        left_pad_lines_for_centered_mode(&mut lines, width);
    }
    Some(lines)
}

fn render_compact_swarm_await(
    title: &str,
    content: &str,
    width: u16,
) -> Option<Vec<Line<'static>>> {
    if title != "🐝 Swarm await" {
        return None;
    }
    let mut lines = vec![Line::from(vec![
        Span::styled("🐝 ", Style::default().fg(rgb(255, 200, 100))),
        Span::styled(
            content.trim().to_string(),
            Style::default().fg(rgb(225, 225, 235)),
        ),
    ])];
    if markdown::center_code_blocks() {
        left_pad_lines_for_centered_mode(&mut lines, width);
    }
    Some(lines)
}

fn render_compact_plan_graph(title: &str, content: &str, width: u16) -> Option<Vec<Line<'static>>> {
    let version = title.strip_prefix("Plan graph · ")?.trim();
    let centered = markdown::center_code_blocks();
    let body_width = width.saturating_sub(3).max(1) as usize;
    let mut lines = vec![Line::from(vec![
        Span::styled("🐝 ", Style::default().fg(rgb(255, 200, 100))),
        Span::styled("Plan", Style::default().fg(rgb(186, 139, 255)).bold()),
        Span::styled(
            format!(" · {version}"),
            Style::default().fg(rgb(150, 150, 160)),
        ),
    ])];
    let mut fill_rows = 0usize;
    for mut line in markdown::render_markdown_with_width(content.trim(), Some(body_width)) {
        if let Some((_, rows, _)) = mermaid::parse_inline_image_placeholder(&line) {
            fill_rows = rows.saturating_sub(1) as usize;
            lines.push(line);
            continue;
        }
        if fill_rows > 0 {
            fill_rows -= 1;
            lines.push(line);
            continue;
        }
        let mut spans = vec![Span::raw("   ")];
        spans.append(&mut line.spans);
        lines.push(Line::from(spans));
    }
    if centered {
        left_pad_lines_for_centered_mode(&mut lines, width);
    }
    Some(lines)
}

fn render_compact_plan_update(
    title: &str,
    content: &str,
    width: u16,
) -> Option<Vec<Line<'static>>> {
    title.strip_prefix("Plan · ")?;
    let mut lines = vec![super::truncate_line_with_ellipsis_to_width(
        &Line::from(vec![
            Span::styled("🐝 ", Style::default().fg(rgb(255, 200, 100))),
            Span::styled("Plan", Style::default().fg(rgb(186, 139, 255)).bold()),
            Span::styled(
                format!(" · {}", content.trim()),
                Style::default().fg(rgb(225, 225, 235)),
            ),
        ]),
        width.max(1) as usize,
    )];
    if markdown::center_code_blocks() {
        left_pad_lines_for_centered_mode(&mut lines, width);
    }
    Some(lines)
}

pub(crate) fn render_swarm_message(
    msg: &DisplayMessage,
    width: u16,
    _diff_mode: crate::config::DiffDisplayMode,
) -> Vec<Line<'static>> {
    if msg.title.as_deref() == Some(SWARM_AGENT_SNAPSHOT_TITLE)
        && let Ok(member) = serde_json::from_str::<crate::protocol::SwarmMemberStatus>(&msg.content)
    {
        return crate::tui::info_widget::swarm_gallery::render_swarm_chat_card_lines(
            &[member],
            width as usize,
        );
    }

    let centered = markdown::center_code_blocks();
    let title = msg.title.as_deref().unwrap_or("Swarm").trim();
    if let Some(lines) = render_compact_agent_notification(title, &msg.content, width) {
        return lines;
    }
    if let Some(lines) = render_compact_swarm_await(title, &msg.content, width) {
        return lines;
    }
    if let Some(lines) = render_compact_plan_graph(title, &msg.content, width) {
        return lines;
    }
    if let Some(lines) = render_compact_plan_update(title, &msg.content, width) {
        return lines;
    }
    let collapsible = jcode_tui_messages::parse_collapsible_swarm_content(&msg.content);
    let (content, tldr_line): (String, Option<(String, bool)>) = match collapsible {
        Some(parsed) if !parsed.expanded => (String::new(), Some((parsed.tldr.to_string(), false))),
        Some(parsed) => (
            parsed.body.trim().to_string(),
            Some((parsed.tldr.to_string(), true)),
        ),
        None => (msg.content.trim().to_string(), None),
    };
    let content = content.as_str();
    let (icon, rail_color, text_color) = swarm_notification_style(msg.title.as_deref());
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
    lines.push(Line::from(Span::styled(
        format!("{} {}", icon, title),
        header_style,
    )));

    // Collapsed/expanded tldr line with its toggle badge. The badge is a
    // click target (see `swarm_expand_target_from_screen`) and must stay the
    // trailing token of this line.
    if let Some((tldr, expanded)) = &tldr_line {
        let badge = if *expanded {
            SWARM_COLLAPSE_BADGE
        } else {
            SWARM_EXPAND_BADGE
        };
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(tldr.clone(), body_style),
            Span::styled(
                format!("  {}", badge),
                Style::default().fg(rail_color).dim(),
            ),
        ]));
    }

    let mut body_lines = if content.is_empty() {
        if tldr_line.is_some() {
            Vec::new()
        } else {
            vec![Line::from(Span::styled(String::new(), body_style))]
        }
    } else {
        markdown::render_markdown_with_width(content, Some(content_width))
    };

    if !content.is_empty() {
        // Mermaid/image placeholders must survive untouched: the marker has to
        // stay the first non-empty span (no rail prefix) and the blank fill
        // rows after it reserve the image's height, so they are exempt from
        // the blank-line cleanup below.
        let mut placeholder_fill_rows = 0usize;
        let placeholder_exempt: Vec<bool> = body_lines
            .iter()
            .map(|line| {
                if let Some((_, rows, _)) = mermaid::parse_inline_image_placeholder(line) {
                    placeholder_fill_rows = rows.saturating_sub(1) as usize;
                    true
                } else if placeholder_fill_rows > 0 {
                    placeholder_fill_rows -= 1;
                    true
                } else {
                    false
                }
            })
            .collect();
        let mut keep = placeholder_exempt.iter();
        body_lines.retain(|line| {
            *keep.next().unwrap_or(&false)
                || line
                    .spans
                    .iter()
                    .any(|span| !span.content.trim().is_empty())
        });
        if body_lines.is_empty() {
            body_lines.push(Line::from(Span::styled(content.to_string(), body_style)));
        }
    }

    let mut placeholder_fill_rows = 0usize;
    for line in body_lines {
        // Placeholder lines bypass the rail/color pass entirely.
        if let Some((_, rows, _)) = mermaid::parse_inline_image_placeholder(&line) {
            placeholder_fill_rows = rows.saturating_sub(1) as usize;
            lines.push(line);
            continue;
        }
        if placeholder_fill_rows > 0 {
            placeholder_fill_rows -= 1;
            lines.push(line);
            continue;
        }
        let mut line = line;
        if line.spans.is_empty() {
            line.spans.push(Span::styled(String::new(), body_style));
        }
        for span in &mut line.spans {
            if span.style.fg.is_none() {
                span.style.fg = Some(text_color);
            }
        }
        let mut spans = vec![Span::raw("  ")];
        spans.extend(line.spans);
        lines.push(Line::from(spans));
    }

    let mut wrapped_lines = Vec::new();
    let mut wrap_fill_rows = 0usize;
    for line in lines {
        if let Some((_, rows, _)) = mermaid::parse_inline_image_placeholder(&line) {
            wrap_fill_rows = rows.saturating_sub(1) as usize;
            wrapped_lines.push(line);
            continue;
        }
        if wrap_fill_rows > 0 {
            wrap_fill_rows -= 1;
            wrapped_lines.push(line);
            continue;
        }
        wrapped_lines.extend(markdown::wrap_line(line, block_wrap_width));
    }

    if centered {
        left_pad_lines_for_centered_mode(&mut wrapped_lines, width);
    }

    wrapped_lines
}

fn edit_tool_inline_diff_lines(tc: &ToolCall, content: &str) -> Option<Vec<ParsedDiffLine>> {
    let from_content = collect_diff_lines(content);
    let change_lines = if !from_content.is_empty() {
        from_content
    } else {
        generate_diff_lines_from_tool_input(tc)
    };
    (!change_lines.is_empty()).then_some(change_lines)
}

pub(super) fn edit_tool_inline_diff_is_expandable(
    tc: &ToolCall,
    content: &str,
    width: u16,
) -> bool {
    let Some(change_lines) = edit_tool_inline_diff_lines(tc, content) else {
        return false;
    };
    if change_lines.len() > MAX_INLINE_DIFF_LINES {
        return true;
    }

    change_lines.iter().any(|line| {
        let border_prefix_width = unicode_width::UnicodeWidthStr::width("│ ")
            + unicode_width::UnicodeWidthStr::width(line.prefix.as_str());
        let max_content_width = (width as usize).saturating_sub(border_prefix_width + 1);
        max_content_width > 1
            && unicode_width::UnicodeWidthStr::width(line.content.as_str()) > max_content_width
    })
}

fn gmail_draft_id(tool_output: &str) -> Option<&str> {
    tool_output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("Draft ID:")
            .map(str::trim)
            .filter(|id| !id.is_empty())
    })
}

fn render_gmail_draft_card(
    tool: &ToolCall,
    tool_output: &str,
    is_error: bool,
    available_width: usize,
) -> Option<Vec<Line<'static>>> {
    if tools_ui::canonical_tool_name(&tool.name) != "gmail"
        || tool.input.get("action").and_then(|value| value.as_str()) != Some("draft")
    {
        return None;
    }

    let max_box_width = available_width.min(88);
    if max_box_width < 10 {
        return None;
    }
    let inner_width = max_box_width.saturating_sub(4).max(1);
    let label_style = Style::default()
        .fg(tool_color())
        .add_modifier(Modifier::BOLD);
    let metadata_style = Style::default().fg(dim_color());
    let body_style = Style::default();
    let border_style = if is_error {
        Style::default().fg(rgb(220, 100, 100))
    } else {
        Style::default().fg(rgb(210, 105, 95))
    };

    let to = tool
        .input
        .get("to")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("(recipient missing)");
    let subject = tool
        .input
        .get("subject")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("(no subject)");

    let mut content: Vec<Line<'static>> = Vec::new();
    push_wrapped_kv_line(
        &mut content,
        "To",
        to,
        inner_width,
        label_style,
        metadata_style,
    );
    push_wrapped_kv_line(
        &mut content,
        "Subject",
        subject,
        inner_width,
        label_style,
        metadata_style,
    );

    if let Some(attachments) = tool
        .input
        .get("attachments")
        .and_then(|value| value.as_array())
    {
        let attachments = attachments
            .iter()
            .filter_map(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        if !attachments.is_empty() {
            push_wrapped_kv_line(
                &mut content,
                "Attachments",
                &attachments.join(", "),
                inner_width,
                label_style,
                metadata_style,
            );
        }
    }

    content.push(Line::from(""));
    content.push(Line::from(Span::styled("Body", label_style)));

    let body = tool
        .input
        .get("body")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let body = strip_ansi_escape_sequences(body)
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let body = if body.trim().is_empty() {
        "(empty body)".to_string()
    } else {
        body
    };
    for mut line in render_plaintext_lines(&body, inner_width) {
        for span in &mut line.spans {
            span.style = body_style;
        }
        content.push(line);
    }

    let title = if is_error {
        "Gmail draft failed".to_string()
    } else if tool_output
        .lines()
        .any(|line| line.trim() == "Draft created successfully.")
    {
        match gmail_draft_id(tool_output) {
            Some(id) => format!("Gmail draft created · {}", id),
            None => "Gmail draft created".to_string(),
        }
    } else {
        "Gmail draft".to_string()
    };

    Some(render_rounded_box(
        &title,
        content,
        max_box_width,
        border_style,
    ))
}

#[derive(Debug, Clone)]
struct DiscoveryListingEntry {
    name: String,
    blurb: String,
    url: Option<String>,
}

fn split_discovery_blurb_url(value: &str) -> (String, Option<String>) {
    let value = value.trim();
    if let Some((blurb, url)) = value.rsplit_once(" (")
        && let Some(url) = url.strip_suffix(')')
        && url.starts_with("http")
    {
        return (blurb.trim().to_string(), Some(url.to_string()));
    }
    (value.to_string(), None)
}

fn parse_discovery_listing_entries(output: &str) -> Vec<DiscoveryListingEntry> {
    output
        .lines()
        .filter_map(|line| {
            let (name, value) = line.trim().strip_prefix("- ")?.split_once(": ")?;
            let (blurb, url) = split_discovery_blurb_url(value);
            Some(DiscoveryListingEntry {
                name: name.trim().to_string(),
                blurb,
                url,
            })
        })
        .collect()
}

fn discovery_selected_details(output: &str, name: &str) -> (Option<String>, Option<String>) {
    let prefix = format!("{name}: ");
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix(&prefix))
        .map(split_discovery_blurb_url)
        .map(|(blurb, url)| (Some(blurb), url))
        .unwrap_or((None, None))
}

fn discovery_setup(output: &str) -> Option<String> {
    let (_, rest) = output.split_once("\n\nSetup: ")?;
    Some(
        rest.split("\n\nConsequential actions")
            .next()
            .unwrap_or(rest)
            .trim()
            .to_string(),
    )
    .filter(|value| !value.is_empty())
}

fn push_compact_discovery_kv(
    content: &mut Vec<Line<'static>>,
    label: &str,
    value: &str,
    available_width: usize,
    label_style: Style,
    value_style: Style,
    max_lines: usize,
) {
    let inner_width = available_width.saturating_sub(2).max(1);
    let mut wrapped = Vec::new();
    push_wrapped_kv_line(
        &mut wrapped,
        label,
        value,
        inner_width,
        label_style,
        value_style,
    );
    if wrapped.is_empty() {
        return;
    }

    let hidden = wrapped.len().saturating_sub(max_lines);
    let mut visible = wrapped.into_iter().take(max_lines).collect::<Vec<_>>();
    if hidden > 0
        && let Some(last) = visible.pop()
    {
        visible.push(super::truncate_line_preserving_suffix_to_width(
            &last,
            &Line::from(Span::styled(" …", value_style)),
            inner_width,
        ));
    }
    for mut line in visible {
        line.spans.insert(0, Span::raw("  "));
        content.push(line);
    }
}

fn push_compact_discovery_header(
    content: &mut Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    available_width: usize,
) {
    let mut line_spans = vec![Span::raw("  ")];
    line_spans.extend(spans);
    content.push(super::truncate_line_with_ellipsis_to_width(
        &Line::from(line_spans),
        available_width,
    ));
}

fn render_discovery_card(
    tool: &ToolCall,
    tool_output: &str,
    is_error: bool,
    available_width: usize,
    show_inline_disclosure: bool,
) -> Option<Vec<Line<'static>>> {
    if tools_ui::canonical_tool_name(&tool.name) != "discover_tools" {
        return None;
    }
    let block_width = available_width.min(96);
    if block_width < 12 {
        return None;
    }
    if tool_output.trim().is_empty() || tool_output.trim() == tool.name {
        return None;
    }

    let muted_style = Style::default().fg(dim_color());
    let label_style = muted_style;
    let name_style = muted_style;
    let mut content = Vec::new();

    if is_error {
        if show_inline_disclosure {
            push_compact_discovery_kv(
                &mut content,
                "about",
                crate::sponsors::DISCOVERY_DISCLOSURE_NOTICE,
                block_width,
                muted_style,
                muted_style,
                MAX_DISCOVERY_DETAIL_LINES,
            );
        }
        return (!content.is_empty()).then_some(content);
    }

    let category = tool
        .input
        .get("category")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("other");
    let explicit_action = tool
        .input
        .get("action")
        .and_then(|value| value.as_str())
        .map(str::trim);
    let action = explicit_action.unwrap_or_else(|| {
        if tool
            .input
            .get("tool")
            .and_then(|value| value.as_str())
            .is_some()
        {
            "select"
        } else {
            "browse"
        }
    });

    match action {
        "suggest" => {
            let kind = tool
                .input
                .get("suggestion_kind")
                .and_then(|value| value.as_str())
                .unwrap_or("capability_gap");
            let kind_label = match kind {
                "known_product" => tool
                    .input
                    .get("product_name")
                    .and_then(|value| value.as_str())
                    .map(|name| format!("Known product · {name}"))
                    .unwrap_or_else(|| "Known product".to_string()),
                _ => "Capability gap".to_string(),
            };
            let receipt = if tool_output.starts_with("Catalog suggestion already recorded.") {
                "suggestion already recorded"
            } else {
                "suggestion sent"
            };
            push_compact_discovery_header(
                &mut content,
                vec![
                    Span::styled(receipt.to_string(), muted_style),
                    Span::styled(" · ", muted_style),
                    Span::styled(kind_label, name_style),
                ],
                block_width,
            );

            if let Some(reason) = tool.input.get("reason").and_then(|value| value.as_str()) {
                push_compact_discovery_kv(
                    &mut content,
                    "gap",
                    reason,
                    block_width,
                    label_style,
                    muted_style,
                    MAX_DISCOVERY_DETAIL_LINES,
                );
            }
            if let Some(url) = tool
                .input
                .get("product_url")
                .and_then(|value| value.as_str())
            {
                push_compact_discovery_kv(
                    &mut content,
                    "url",
                    url,
                    block_width,
                    label_style,
                    muted_style,
                    1,
                );
            }
            if let Some(requirements) = tool
                .input
                .get("requirements")
                .and_then(|value| value.as_array())
            {
                let requirements = requirements
                    .iter()
                    .filter_map(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<_>>();
                if !requirements.is_empty() {
                    push_compact_discovery_kv(
                        &mut content,
                        "needs",
                        &requirements.join(" · "),
                        block_width,
                        label_style,
                        muted_style,
                        MAX_DISCOVERY_DETAIL_LINES,
                    );
                }
            }
            push_compact_discovery_kv(
                &mut content,
                "review",
                "Jcode maintainers only; not approval or availability",
                block_width,
                label_style,
                muted_style,
                MAX_DISCOVERY_DETAIL_LINES,
            );
        }
        "select" => {
            let name = tool
                .input
                .get("tool")
                .and_then(|value| value.as_str())
                .unwrap_or("selected tool");
            push_compact_discovery_header(
                &mut content,
                vec![
                    Span::styled("selected ", muted_style),
                    Span::styled(name.to_string(), name_style),
                ],
                block_width,
            );
            let (blurb, url) = discovery_selected_details(tool_output, name);
            let description = match (blurb.as_deref(), url.as_deref()) {
                (Some(blurb), Some(url)) => Some(format!("{blurb} · {url}")),
                (Some(blurb), None) => Some(blurb.to_string()),
                (None, Some(url)) => Some(url.to_string()),
                (None, None) => None,
            };
            if let Some(description) = description {
                push_compact_discovery_kv(
                    &mut content,
                    "details",
                    &description,
                    block_width,
                    label_style,
                    muted_style,
                    MAX_DISCOVERY_DETAIL_LINES,
                );
            }
            if let Some(reason) = tool.input.get("reason").and_then(|value| value.as_str()) {
                push_compact_discovery_kv(
                    &mut content,
                    "why",
                    reason,
                    block_width,
                    label_style,
                    muted_style,
                    MAX_DISCOVERY_DETAIL_LINES,
                );
            }
            if let Some(setup) = discovery_setup(tool_output) {
                push_compact_discovery_kv(
                    &mut content,
                    "setup",
                    &setup,
                    block_width,
                    label_style,
                    muted_style,
                    MAX_DISCOVERY_SETUP_LINES,
                );
            }
        }
        _ => {
            let entries = parse_discovery_listing_entries(tool_output);
            let result_label = if entries.len() == 1 {
                "result"
            } else {
                "results"
            };
            push_compact_discovery_header(
                &mut content,
                vec![
                    Span::styled(format!("{} {result_label}", entries.len()), muted_style),
                    Span::styled(" · ", muted_style),
                    Span::styled(category.to_string(), name_style),
                ],
                block_width,
            );
            if entries.is_empty() {
                push_compact_discovery_kv(
                    &mut content,
                    "catalog",
                    "no matching entries",
                    block_width,
                    label_style,
                    muted_style,
                    1,
                );
            } else {
                for entry in entries.iter().take(MAX_DISCOVERY_LISTING_ENTRIES) {
                    let details = match (&entry.blurb[..], entry.url.as_deref()) {
                        ("", Some(url)) => url.to_string(),
                        (blurb, Some(url)) => format!("{blurb} · {url}"),
                        (blurb, None) => blurb.to_string(),
                    };
                    push_compact_discovery_kv(
                        &mut content,
                        &entry.name,
                        &details,
                        block_width,
                        name_style,
                        muted_style,
                        MAX_DISCOVERY_DETAIL_LINES,
                    );
                }
                let hidden = entries.len().saturating_sub(MAX_DISCOVERY_LISTING_ENTRIES);
                if hidden > 0 {
                    push_compact_discovery_header(
                        &mut content,
                        vec![Span::styled(format!("+{hidden} more"), muted_style)],
                        block_width,
                    );
                }
            }
            if let Some(reason) = tool.input.get("reason").and_then(|value| value.as_str()) {
                push_compact_discovery_kv(
                    &mut content,
                    "why",
                    reason,
                    block_width,
                    label_style,
                    muted_style,
                    MAX_DISCOVERY_DETAIL_LINES,
                );
            }
        }
    }

    if show_inline_disclosure {
        push_compact_discovery_kv(
            &mut content,
            "about",
            crate::sponsors::DISCOVERY_DISCLOSURE_NOTICE,
            block_width,
            muted_style,
            muted_style,
            MAX_DISCOVERY_DETAIL_LINES,
        );
    }

    Some(content)
}

pub(crate) fn render_tool_message(
    msg: &DisplayMessage,
    width: u16,
    diff_mode: crate::config::DiffDisplayMode,
) -> Vec<Line<'static>> {
    if let Some(lines) = render_scheduled_tool_message(msg, width) {
        return lines;
    }

    let centered = markdown::center_code_blocks();
    let token_badge = tool_output_token_badge(&msg.content);

    // A restored or remotely mirrored transcript can occasionally retain the
    // todo result while losing its paired ToolCall metadata. Recognize the
    // structured todo payload itself so the full card never disappears merely
    // because that display-only association was unavailable.
    let is_todo_tool = msg
        .tool_data
        .as_ref()
        .is_none_or(|tc| tools_ui::canonical_tool_name(&tc.name) == "todo");
    if is_todo_tool
        && !tools_ui::tool_output_looks_failed(&msg.content)
        && let Some(parsed) = parse_todo_tool_output(&msg.content)
    {
        if !parsed.goal_updates.is_empty() {
            return render_todo_goal_updates(&parsed.goal_updates, width);
        }
        let payload =
            serde_json::json!({ "todos": parsed.todos, "goals": parsed.goals }).to_string();
        return render_todos_message(
            &DisplayMessage::todos(payload),
            width,
            crate::config::DiffDisplayMode::Off,
        );
    }

    let mut lines: Vec<Line<'static>> = Vec::new();
    let Some(ref tc) = msg.tool_data else {
        return lines;
    };

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
    let has_diff_changes = additions > 0 || deletions > 0;

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
    let edit_suffix_width = if is_edit_tool && has_diff_changes {
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
        // Error summaries always render so failures stay diagnosable even
        // when technical details are hidden.
        let show_detail = tools_ui::show_tool_call_details() || is_error || is_partial_batch;
        if show_detail && !summary.is_empty() && summary != intent {
            tool_line.push(Span::styled(" · ", Style::default().fg(dim_color())));
            tool_line.push(Span::styled(summary, Style::default().fg(dim_color())));
        }
    } else if !summary.is_empty() {
        tool_line.push(Span::styled(
            format!(" {}", summary),
            Style::default().fg(dim_color()),
        ));
    }
    if is_edit_tool && has_diff_changes {
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

    let rendered_tool_line = super::truncate_line_preserving_suffix_to_width(
        &Line::from(tool_line),
        &token_suffix,
        row_width,
    );
    let rendered_tool_line_text = super::line_plain_text(&rendered_tool_line);
    lines.push(rendered_tool_line);
    let mut show_inline_sponsor_disclosure =
        msg.title.as_deref() == Some(crate::sponsors::DISCOVERY_DISCLOSURE_TAG);

    if let Some(draft_lines) = render_gmail_draft_card(tc, &msg.content, is_error, row_width) {
        lines.extend(draft_lines);
    }
    if let Some(discovery_lines) = render_discovery_card(
        tc,
        &msg.content,
        is_error,
        row_width,
        show_inline_sponsor_disclosure,
    ) {
        show_inline_sponsor_disclosure = false;
        lines.extend(discovery_lines);
    }

    // Optionally render the full agentgrep search output inline in the
    // transcript. Gated behind `display.show_agentgrep_output` (default false)
    // so most users keep the compact one-line summary.
    if tools_ui::canonical_tool_name(&tc.name) == "agentgrep"
        && crate::config::config().display.show_agentgrep_output
        && !msg.content.trim().is_empty()
    {
        for line in render_agentgrep_output_body(&msg.content, row_width) {
            lines.push(line);
        }
    }

    // Fallback command preview on a second line only when the row has no
    // intent. With an intent present, the command summary is inline-only: it
    // shows on the tool row when it fits and is dropped otherwise, never
    // spilling onto a second line.
    if tools_ui::canonical_tool_name(&tc.name) == "bash"
        && intent.is_none()
        && !rendered_tool_line_text.contains('$')
        && let Some(command) = tc.input.get("command").and_then(|v| v.as_str())
    {
        let detail_width = row_width.saturating_sub(4).max(1);
        let command_detail = tools_ui::get_tool_summary_with_budget(tc, 80, Some(detail_width));
        if !command_detail.trim().is_empty() {
            let detail_line = Line::from(vec![
                Span::raw("    "),
                Span::styled(command_detail, Style::default().fg(dim_color())),
            ]);
            lines.push(super::truncate_line_with_ellipsis_to_width(
                &detail_line,
                row_width,
            ));
        } else if !command.trim().is_empty() {
            let fallback = format!("$ {}", command.trim());
            let detail_line = Line::from(vec![
                Span::raw("    "),
                Span::styled(fallback, Style::default().fg(dim_color())),
            ]);
            lines.push(super::truncate_line_with_ellipsis_to_width(
                &detail_line,
                row_width,
            ));
        }
    }

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
            let intent = tools_ui::batch_subcall_intent(call, &params);

            let sub_tc = ToolCall {
                id: String::new(),
                name: tools_ui::resolve_display_tool_name(raw_name).to_string(),
                input: params,
                intent,
                thought_signature: None,
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

            if let Some(result) = sub_result
                && let Some(mut draft_lines) = render_gmail_draft_card(
                    &sub_tc,
                    &result.content,
                    sub_errored,
                    row_width.saturating_sub(4),
                )
            {
                for line in &mut draft_lines {
                    line.spans.insert(0, Span::raw("    "));
                }
                lines.extend(draft_lines);
            }

            if let Some(result) = sub_result
                && let Some(mut discovery_lines) = render_discovery_card(
                    &sub_tc,
                    &result.content,
                    sub_errored,
                    row_width.saturating_sub(4),
                    show_inline_sponsor_disclosure,
                )
            {
                show_inline_sponsor_disclosure = false;
                for line in &mut discovery_lines {
                    line.spans.insert(0, Span::raw("    "));
                }
                lines.extend(discovery_lines);
            }

            if tools_ui::canonical_tool_name(&sub_tc.name) == "todo"
                && !sub_errored
                && let Some(result) = sub_result
                && let Some(parsed) = parse_todo_tool_output(&result.content)
            {
                let nested_width = row_width.saturating_sub(4).max(1).min(u16::MAX as usize) as u16;
                let mut todo_lines = if parsed.goal_updates.is_empty() {
                    let payload = serde_json::json!({
                        "todos": parsed.todos,
                        "goals": parsed.goals,
                    })
                    .to_string();
                    render_todos_message(
                        &DisplayMessage::todos(payload),
                        nested_width,
                        crate::config::DiffDisplayMode::Off,
                    )
                } else {
                    render_todo_goal_updates(&parsed.goal_updates, nested_width)
                };
                for line in &mut todo_lines {
                    line.spans.insert(0, Span::raw("    "));
                }
                lines.extend(todo_lines);
            }
        }
    }

    if diff_mode.is_inline()
        && is_edit_tool
        && let Some(change_lines) = edit_tool_inline_diff_lines(tc, &msg.content)
    {
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

        const MAX_DIFF_LINES: usize = MAX_INLINE_DIFF_LINES;
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

#[cfg(test)]
#[path = "ui_messages/tests.rs"]
mod tests;
