use super::*;

pub(super) fn status_single_line(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control()
                || c.is_whitespace()
                || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Main composer activity. Only the phase occupies a fixed ten-cell field.
pub(in crate::tui) fn composer_processing_status_line(
    status: &crate::protocol::SwarmLiveStatus,
    stale_secs: Option<f32>,
    spinner: &'static str,
) -> Line<'static> {
    if matches!(
        status.phase.as_str(),
        "stopped" | "failed" | "crashed" | "completed" | "done" | "cancelled"
    ) {
        let mut line = live_processing_status_line(status, stale_secs, spinner);
        for span in &mut line.spans {
            span.content = status_single_line(&span.content).into();
        }
        return line;
    }
    let connection = status.connection_phase.as_deref().unwrap_or("connecting");
    let phase = match status.phase.as_str() {
        "running_tool" if status.tool_name.as_deref() == Some("batch") => "batch",
        "running_tool" => "tool",
        "running" => "thinking",
        "waiting_network" => "offline",
        "connecting" if connection.starts_with("retry") => "retry",
        "sending" => "sending",
        "connecting" => "connecting",
        "thinking" => "thinking",
        "streaming" => "streaming",
        "building" => "building",
        "retry" => "retry",
        _ => "thinking",
    };
    let seconds = status.elapsed_ms / 1000;
    let elapsed = if seconds >= 3600 {
        format!("{}h {}m", seconds / 3600, seconds / 60 % 60)
    } else if seconds >= 60 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    };
    let warning = matches!(phase, "offline" | "retry" | "building");
    let color = if warning {
        rgb(255, 193, 7)
    } else {
        dim_color()
    };
    let mut spans = vec![
        Span::styled(
            spinner,
            Style::default().fg(if warning { color } else { ai_color() }),
        ),
        Span::styled(
            format!(" {phase:<10} {elapsed}"),
            Style::default().fg(color),
        ),
    ];
    let mut detail = |text: String| {
        let text = status_single_line(&text);
        if !text.is_empty() {
            spans.push(Span::styled(
                format!(" · {text}"),
                Style::default().fg(color),
            ));
        }
    };
    match status.phase.as_str() {
        "connecting" => {
            let label = match connection {
                "authenticating" => "refreshing auth",
                "sending_request" => "sending context",
                "waiting_for_response" => "waiting for response",
                other => other,
            };
            if label != phase {
                detail(label.into());
            }
        }
        "running_tool" => {
            if let Some(name) = status
                .tool_name
                .as_ref()
                .filter(|name| name.as_str() != "batch")
            {
                detail(name.clone());
            }
            if let Some(intent) = &status.tool_intent {
                detail(intent.clone());
            }
        }
        "streaming" if !status.stream_message_ended => match stale_secs {
            Some(s) if s > 10.0 => detail(format!("(stalled {s:.0}s)")),
            Some(s) if s > 2.0 => detail(format!("(no tokens {s:.0}s)")),
            _ => {}
        },
        "waiting_network" => detail("network disconnected, waiting to retry".into()),
        _ => {}
    }
    let tokens = [
        status
            .input_tokens
            .map(|input| format!("↑{}", format_stream_tokens(input))),
        status
            .output_tokens
            .map(|output| format!("↓{}", format_stream_tokens(output))),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    if !tokens.is_empty() {
        detail(tokens);
    }
    if let Some(tps) = status.output_tps_milli {
        detail(format!("{:.1} tps", tps as f64 / 1000.0));
    }
    for label in collect_transport_context_labels(
        status
            .status_detail
            .as_deref()
            .and_then(normalize_status_detail),
        status
            .connection_type
            .as_deref()
            .map(display_connection_type),
        None,
    ) {
        detail(label);
    }
    Line::from(spans)
}
