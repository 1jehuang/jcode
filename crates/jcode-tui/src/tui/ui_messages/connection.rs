//! Compact connection state and retry notices.
use super::*;

pub(crate) fn split_resume_hint(detail: &str) -> (&str, Option<&str>) {
    if let Some((main, hint)) = detail.split_once(" · resume: ") {
        (main.trim(), Some(hint.trim()))
    } else {
        (detail.trim(), None)
    }
}

pub(crate) fn truncate_connection_line(input: &str, width: usize) -> String {
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

pub(crate) fn parse_connection_retry_message(
    content: &str,
) -> Option<(String, String, Option<String>)> {
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

pub(crate) fn parse_connection_waiting_message(
    content: &str,
) -> Option<(String, String, Option<String>)> {
    let rest = content.strip_prefix("⚡ Server reload in progress - waiting for handoff (")?;
    let (elapsed, detail) = rest.split_once(") - ")?;
    let (detail, hint) = split_resume_hint(detail);
    Some((
        format!("Waiting for handoff · {}", elapsed.trim()),
        detail.to_string(),
        hint.map(str::to_string),
    ))
}

pub(crate) fn render_connection_system_message(
    msg: &DisplayMessage,
    width: u16,
) -> Vec<Line<'static>> {
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
