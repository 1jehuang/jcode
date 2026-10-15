use super::*;

pub(super) fn gmail_draft_id(tool_output: &str) -> Option<&str> {
    tool_output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("Draft ID:")
            .map(str::trim)
            .filter(|id| !id.is_empty())
    })
}

pub(super) fn render_gmail_draft_card(
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
