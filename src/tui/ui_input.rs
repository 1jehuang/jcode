use super::inline_interactive_ui::format_elapsed;
use super::tools_ui::{get_tool_summary, summarize_batch_running_tools_compact};
use super::visual_debug::{self, FrameCaptureBuilder};
use super::{
    ProcessingStatus, TuiState, accent_color, ai_color, animated_tool_color, asap_color, dim_color,
    is_unexpected_cache_miss, pending_color, queued_color, rainbow_prompt_color, user_color,
};
use crate::message::ConnectionPhase;
use crate::tui::app;
use crate::tui::color_support::rgb;
use crate::tui::info_widget::occasional_status_tip;
use crate::tui::layout_utils;
use ratatui::{prelude::*, widgets::Paragraph};

fn shell_mode_color() -> Color {
    rgb(110, 214, 151)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComposerMode {
    Chat,
    SlashCommand,
    ShellLocal,
    ShellRemote,
}

impl ComposerMode {
    fn is_shell(self) -> bool {
        matches!(self, Self::ShellLocal | Self::ShellRemote)
    }
}

fn composer_mode(input: &str, is_remote_mode: bool) -> ComposerMode {
    if app::extract_input_shell_command(input).is_some() {
        if is_remote_mode {
            ComposerMode::ShellRemote
        } else {
            ComposerMode::ShellLocal
        }
    } else if input.trim_start().starts_with('/') {
        ComposerMode::SlashCommand
    } else {
        ComposerMode::Chat
    }
}

fn shell_mode_hint(mode: ComposerMode) -> Option<&'static str> {
    match mode {
        ComposerMode::ShellLocal => Some("  shell mode · Enter runs locally"),
        ComposerMode::ShellRemote => Some("  shell mode · Enter runs on server"),
        _ => None,
    }
}

fn normalize_repaint_sensitive_notice_text(text: &str) -> String {
    text.replace("⚠️", "⚠")
}

pub(super) fn input_hint_line_height(app: &dyn TuiState) -> u16 {
    let suggestions = app.command_suggestions();
    let mode = composer_mode(app.input(), app.is_remote_mode());
    let has_suggestions = !suggestions.is_empty()
        && matches!(mode, ComposerMode::SlashCommand | ComposerMode::Chat)
        && (matches!(mode, ComposerMode::SlashCommand) || !app.is_processing());

    if has_suggestions
        || shell_mode_hint(mode).is_some()
        || app.next_prompt_new_session_armed()
        || (app.is_processing() && !app.input().is_empty())
    {
        1
    } else {
        0
    }
}

pub(super) fn send_mode_reserved_width(app: &dyn TuiState) -> usize {
    let (icon, _) = send_mode_indicator(app);
    if icon.is_empty() { 0 } else { icon.len() + 1 }
}

pub(super) fn input_prompt(app: &dyn TuiState) -> (&'static str, Color) {
    let mode = composer_mode(app.input(), app.is_remote_mode());
    if mode.is_shell() {
        ("$ ", shell_mode_color())
    } else if app.is_processing() {
        ("… ", queued_color())
    } else if app.active_skill().is_some() {
        ("» ", accent_color())
    } else {
        ("> ", user_color())
    }
}

pub(crate) fn input_prompt_len(app: &dyn TuiState, next_prompt: usize) -> usize {
    let (prompt_char, _) = input_prompt(app);
    next_prompt.to_string().chars().count() + prompt_char.chars().count()
}

pub(crate) fn next_input_prompt_number(app: &dyn TuiState) -> usize {
    app.display_user_message_count() + 1
}

pub(super) fn wrapped_input_line_count(
    app: &dyn TuiState,
    area_width: u16,
    next_prompt: usize,
) -> usize {
    let reserved_width = send_mode_reserved_width(app);
    let prompt_len = input_prompt_len(app, next_prompt);
    let line_width = (area_width as usize).saturating_sub(prompt_len + reserved_width);
    if line_width == 0 {
        return 1;
    }

    let num_str = next_prompt.to_string();
    let (prompt_char, caret_color) = input_prompt(app);
    let (lines, _, _) = wrap_input_text(
        app.input(),
        app.cursor_pos(),
        line_width,
        &num_str,
        prompt_char,
        caret_color,
        prompt_len,
    );
    lines.len().max(1)
}

pub(super) fn pending_prompt_count(app: &dyn TuiState) -> usize {
    let pending_count = if app.is_processing() {
        app.pending_soft_interrupts().len()
    } else {
        0
    };
    let interleave = app.is_processing()
        && app
            .interleave_message()
            .map(|msg| !msg.is_empty())
            .unwrap_or(false);
    app.queued_messages().len() + pending_count + if interleave { 1 } else { 0 }
}

pub(super) fn pending_queue_preview(app: &dyn TuiState) -> Vec<String> {
    let mut previews = Vec::new();
    if app.is_processing() {
        for msg in app.pending_soft_interrupts() {
            if !msg.is_empty() {
                let normalized = normalize_repaint_sensitive_notice_text(msg);
                previews.push(format!(
                    "↻ {}",
                    normalized.chars().take(100).collect::<String>()
                ));
            }
        }
        if let Some(msg) = app.interleave_message()
            && !msg.is_empty()
        {
            let normalized = normalize_repaint_sensitive_notice_text(msg);
            previews.push(format!(
                "⚡ {}",
                normalized.chars().take(100).collect::<String>()
            ));
        }
    }
    for msg in app.queued_messages() {
        let normalized = normalize_repaint_sensitive_notice_text(msg);
        previews.push(format!(
            "⏳ {}",
            normalized.chars().take(100).collect::<String>()
        ));
    }
    previews
}

pub(super) fn draw_queued(frame: &mut Frame, app: &dyn TuiState, area: Rect, start_num: usize) {
    let mut items: Vec<(QueuedMsgType, &str)> = Vec::new();
    if app.is_processing() {
        for msg in app.pending_soft_interrupts() {
            if !msg.is_empty() {
                items.push((QueuedMsgType::Pending, msg.as_str()));
            }
        }
        if let Some(msg) = app.interleave_message()
            && !msg.is_empty()
        {
            items.push((QueuedMsgType::Interleave, msg));
        }
    }
    for msg in app.queued_messages() {
        items.push((QueuedMsgType::Queued, msg.as_str()));
    }

    let pending_count = items.len();
    let lines: Vec<Line> = items
        .iter()
        .take(3)
        .enumerate()
        .map(|(i, (msg_type, msg))| {
            let normalized_msg = normalize_repaint_sensitive_notice_text(msg);
            let distance = pending_count.saturating_sub(i);
            let num_color = rainbow_prompt_color(distance);
            let (indicator, indicator_color, msg_color, dim) = match msg_type {
                QueuedMsgType::Pending => ("↻", pending_color(), pending_color(), false),
                QueuedMsgType::Interleave => ("⚡", asap_color(), asap_color(), false),
                QueuedMsgType::Queued => ("⏳", queued_color(), queued_color(), true),
            };
            let mut msg_style = Style::default().fg(msg_color);
            if dim {
                msg_style = msg_style.dim();
            }
            Line::from(vec![
                Span::styled(format!("{}", start_num + i), Style::default().fg(num_color)),
                Span::raw(" "),
                Span::styled(indicator, Style::default().fg(indicator_color)),
                Span::raw(" "),
                Span::styled(normalized_msg, msg_style),
            ])
        })
        .collect();

    let paragraph = if app.centered_mode() {
        Paragraph::new(
            lines
                .iter()
                .map(|line| line.clone().alignment(Alignment::Center))
                .collect::<Vec<_>>(),
        )
    } else {
        Paragraph::new(lines)
    };
    frame.render_widget(paragraph, area);
}

fn format_stream_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.0}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

fn connection_phase_label(phase: &ConnectionPhase) -> String {
    match phase {
        ConnectionPhase::Authenticating => "refreshing auth".to_string(),
        ConnectionPhase::Connecting => "connecting".to_string(),
        ConnectionPhase::WaitingForResponse => "waiting for response".to_string(),
        ConnectionPhase::Streaming => "streaming".to_string(),
        ConnectionPhase::Retrying { attempt, max } => format!("retrying {}/{}", attempt, max),
    }
}

fn display_connection_type(connection_type: &str) -> String {
    match connection_type.trim() {
        "https/sse" => "https".to_string(),
        "websocket/persistent-fresh" => "websocket".to_string(),
        "websocket/persistent-reuse" => "existing websocket".to_string(),
        other => other.to_string(),
    }
}

fn normalize_status_detail(detail: &str) -> Option<String> {
    let trimmed = detail.trim();
    if trimmed.is_empty() {
        return None;
    }

    Some(
        match trimmed {
            "fresh websocket" => "opening websocket",
            "reusing websocket" => "using existing websocket",
            "websocket healthcheck" => "verifying websocket",
            "https fallback" => "using https fallback",
            other => other,
        }
        .to_string(),
    )
}

fn transport_label_overlaps(left: &str, right: &str) -> bool {
    let left = left.trim().to_ascii_lowercase();
    let right = right.trim().to_ascii_lowercase();
    !left.is_empty()
        && !right.is_empty()
        && (left == right || left.contains(&right) || right.contains(&left))
}

fn collect_transport_context_labels(
    detail: Option<String>,
    connection: Option<String>,
    upstream: Option<String>,
) -> Vec<String> {
    let mut labels = Vec::new();

    if let Some(detail) = detail.filter(|detail| !detail.trim().is_empty()) {
        labels.push(detail);
    }

    if let Some(connection) = connection.filter(|conn| !conn.trim().is_empty()) {
        let overlaps_existing = labels
            .iter()
            .any(|existing| transport_label_overlaps(existing, &connection));
        if !overlaps_existing {
            labels.push(connection);
        }
    }

    if let Some(upstream) = upstream
        .map(|upstream| upstream.trim().to_string())
        .filter(|upstream| !upstream.is_empty())
    {
        labels.push(format!("via {}", upstream));
    }

    labels
}

fn transport_context_labels(app: &dyn TuiState) -> Vec<String> {
    collect_transport_context_labels(
        app.status_detail()
            .and_then(|detail| normalize_status_detail(&detail)),
        app.connection_type()
            .map(|conn| display_connection_type(&conn))
            .filter(|conn| !conn.is_empty()),
        app.upstream_provider(),
    )
}

fn append_transport_context(status_text: &mut String, app: &dyn TuiState) {
    for label in transport_context_labels(app) {
        status_text.push_str(&format!(" · {}", label));
    }
}

fn streaming_liveness_label(
    time_str: String,
    stale_secs: Option<f32>,
    stream_message_ended: bool,
) -> String {
    if stream_message_ended {
        return time_str;
    }
    match stale_secs {
        Some(s) if s > 10.0 => format!("(stalled {:.0}s) · {}", s, time_str),
        Some(s) if s > 2.0 => format!("(no tokens {:.0}s) · {}", s, time_str),
        _ => time_str,
    }
}

fn batch_progress_state(
    batch_prog: Option<crate::bus::BatchProgress>,
    initial_total: Option<usize>,
) -> (usize, usize, Option<String>) {
    match batch_prog {
        Some(progress) => (progress.completed, progress.total, progress.last_completed),
        None => (0, initial_total.unwrap_or(0), None),
    }
}

fn batch_running_summary(batch_prog: &crate::bus::BatchProgress) -> Option<String> {
    summarize_batch_running_tools_compact(&batch_prog.running)
}

fn append_batch_progress_spans(
    spans: &mut Vec<Span<'static>>,
    anim_color: Color,
    batch_prog: Option<crate::bus::BatchProgress>,
    initial_total: Option<usize>,
) {
    let running_summary = batch_prog.as_ref().and_then(batch_running_summary);
    let (completed, total, last_completed) = batch_progress_state(batch_prog, initial_total);

    if total > 0 {
        spans.push(Span::styled(
            format!(" · {}/{} done", completed, total),
            Style::default().fg(anim_color).bold(),
        ));
    }

    if let Some(running) = running_summary {
        spans.push(Span::styled(
            format!(" · running: {}", running),
            Style::default().fg(dim_color()),
        ));
    }

    if let Some(tool_name) = last_completed.filter(|_| completed < total) {
        spans.push(Span::styled(
            format!(" · last done: {}", tool_name),
            Style::default().fg(dim_color()),
        ));
    }
}

pub(super) fn draw_status(frame: &mut Frame, app: &dyn TuiState, area: Rect, pending_count: usize) {
    let elapsed = app.elapsed().map(|d| d.as_secs_f32()).unwrap_or(0.0);
    let stale_secs = app.time_since_activity().map(|d| d.as_secs_f32());
    let (cache_read, cache_creation) = app.streaming_cache_tokens();
    let user_turn_count = app.display_user_message_count();
    let unexpected_cache_miss =
        is_unexpected_cache_miss(user_turn_count, cache_read, cache_creation);

    let queued_suffix = if pending_count > 0 {
        format!(" · +{} queued", pending_count)
    } else {
        String::new()
    };

    let line = if let Some(build_progress) = crate::build::read_build_progress() {
        let spinner = super::activity_indicator(elapsed, 12.5);
        Line::from(vec![
            Span::styled(spinner, Style::default().fg(rgb(255, 193, 7))),
            Span::styled(
                format!(" {}", build_progress),
                Style::default().fg(rgb(255, 193, 7)),
            ),
        ])
    } else if let Some(remaining) = app.rate_limit_remaining() {
        let secs = remaining.as_secs();
        let spinner = super::activity_indicator(elapsed, 4.0);
        let time_str = if secs >= 3600 {
            let hours = secs / 3600;
            let mins = (secs % 3600) / 60;
            format!("{}h {}m", hours, mins)
        } else if secs >= 60 {
            let mins = secs / 60;
            let s = secs % 60;
            format!("{}m {}s", mins, s)
        } else {
            format!("{}s", secs)
        };
        Line::from(vec![
            Span::styled(spinner, Style::default().fg(rgb(255, 193, 7))),
            Span::styled(
                format!(
                    " Rate limited. Auto-retry in {}...{}",
                    time_str, queued_suffix
                ),
                Style::default().fg(rgb(255, 193, 7)),
            ),
        ])
    } else if app.is_processing() {
        let spinner = super::activity_indicator(elapsed, 12.5);

        match app.status() {
            ProcessingStatus::Idle => Line::from(""),
            ProcessingStatus::Sending => {
                let mut spans = vec![
                    Span::styled(spinner, Style::default().fg(ai_color())),
                    Span::styled(
                        format!(" sending… {}", format_elapsed(elapsed)),
                        Style::default().fg(dim_color()),
                    ),
                ];
                if !queued_suffix.is_empty() {
                    spans.push(Span::styled(
                        queued_suffix.clone(),
                        Style::default().fg(queued_color()),
                    ));
                }
                Line::from(spans)
            }
            ProcessingStatus::Connecting(ref phase) => {
                let mut label = format!(
                    " {}… {}",
                    connection_phase_label(phase),
                    format_elapsed(elapsed)
                );
                append_transport_context(&mut label, app);
                let label_color = match phase {
                    crate::message::ConnectionPhase::Retrying { .. } => rgb(255, 193, 7),
                    crate::message::ConnectionPhase::Authenticating if elapsed > 10.0 => {
                        rgb(255, 193, 7)
                    }
                    crate::message::ConnectionPhase::Connecting if elapsed > 10.0 => {
                        rgb(255, 193, 7)
                    }
                    _ => dim_color(),
                };
                let mut spans = vec![
                    Span::styled(spinner, Style::default().fg(ai_color())),
                    Span::styled(label, Style::default().fg(label_color)),
                ];
                if !queued_suffix.is_empty() {
                    spans.push(Span::styled(
                        queued_suffix.clone(),
                        Style::default().fg(queued_color()),
                    ));
                }
                Line::from(spans)
            }
            ProcessingStatus::Thinking(_start) => {
                let mut label = format!(" thinking… {:.1}s", elapsed);
                append_transport_context(&mut label, app);
                let mut spans = vec![
                    Span::styled(spinner, Style::default().fg(ai_color())),
                    Span::styled(label, Style::default().fg(dim_color())),
                ];
                if !queued_suffix.is_empty() {
                    spans.push(Span::styled(
                        queued_suffix.clone(),
                        Style::default().fg(queued_color()),
                    ));
                }
                Line::from(spans)
            }
            ProcessingStatus::Streaming => {
                let time_str = format_elapsed(elapsed);
                let (input_tokens, output_tokens) = app.streaming_tokens();
                let mut status_text =
                    streaming_liveness_label(time_str, stale_secs, app.stream_message_ended());
                if let Some(tps) = app.output_tps() {
                    status_text = format!("{} · {:.1} tps", status_text, tps);
                }
                if input_tokens > 0 || output_tokens > 0 {
                    status_text = format!(
                        "{} · ↑{} ↓{}",
                        status_text,
                        format_stream_tokens(input_tokens),
                        format_stream_tokens(output_tokens)
                    );
                }
                append_transport_context(&mut status_text, app);
                if unexpected_cache_miss {
                    let miss_tokens = cache_creation.unwrap_or(0);
                    let miss_str = if miss_tokens >= 1000 {
                        format!("{}k", miss_tokens / 1000)
                    } else {
                        format!("{}", miss_tokens)
                    };
                    status_text = format!("⚠ {} cache miss · {}", miss_str, status_text);
                }
                let mut spans = vec![
                    Span::styled(spinner, Style::default().fg(ai_color())),
                    Span::styled(
                        format!(" {}", status_text),
                        Style::default().fg(if unexpected_cache_miss {
                            rgb(255, 193, 7)
                        } else {
                            dim_color()
                        }),
                    ),
                ];
                if !queued_suffix.is_empty() {
                    spans.push(Span::styled(
                        queued_suffix.clone(),
                        Style::default().fg(queued_color()),
                    ));
                }
                Line::from(spans)
            }
            ProcessingStatus::RunningTool(ref name) => {
                let half_width = 3;
                let (left_bar, right_bar) =
                    if crate::perf::tui_policy().enable_decorative_animations {
                        let progress = elapsed * 2.0 % 1.0;
                        let filled_pos = ((progress * half_width as f32) as usize) % half_width;
                        let left_bar: String = (0..half_width)
                            .map(|i| if i == filled_pos { '●' } else { '·' })
                            .collect();
                        let right_bar: String = (0..half_width)
                            .map(|i| {
                                if i == (half_width - 1 - filled_pos) {
                                    '●'
                                } else {
                                    '·'
                                }
                            })
                            .collect();
                        (left_bar, right_bar)
                    } else {
                        ("···".to_string(), "···".to_string())
                    };

                let anim_color = animated_tool_color(elapsed);
                let batch_prog = app.batch_progress();
                let is_batch = name == "batch";
                // For batch: compute initial total from the streaming tool call input
                let batch_total_initial = if is_batch {
                    app.streaming_tool_calls()
                        .last()
                        .and_then(|tc| tc.input.get("tool_calls"))
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                } else {
                    None
                };
                let tool_detail = if is_batch {
                    None // batch always uses progress display
                } else {
                    app.streaming_tool_calls()
                        .last()
                        .map(get_tool_summary)
                        .filter(|s| !s.is_empty())
                };
                let subagent = app.subagent_status();

                let mut spans = vec![
                    Span::styled(left_bar, Style::default().fg(anim_color)),
                    Span::styled(" ", Style::default()),
                    Span::styled(name.to_string(), Style::default().fg(anim_color).bold()),
                    Span::styled(" ", Style::default()),
                    Span::styled(right_bar, Style::default().fg(anim_color)),
                ];

                // For batch tool: show "completed/total · last_tool" progress
                if is_batch {
                    append_batch_progress_spans(
                        &mut spans,
                        anim_color,
                        batch_prog,
                        batch_total_initial,
                    );
                } else if let Some(detail) = tool_detail {
                    spans.push(Span::styled(
                        format!(" · {}", detail),
                        Style::default().fg(dim_color()),
                    ));
                }

                if let Some(status) = subagent {
                    spans.push(Span::styled(
                        format!(" ({})", status),
                        Style::default().fg(dim_color()),
                    ));
                }
                for label in transport_context_labels(app) {
                    spans.push(Span::styled(
                        format!(" · {}", label),
                        Style::default().fg(dim_color()),
                    ));
                }
                spans.push(Span::styled(
                    format!(" · {}", format_elapsed(elapsed)),
                    Style::default().fg(dim_color()),
                ));

                if unexpected_cache_miss {
                    let miss_tokens = cache_creation.unwrap_or(0);
                    let miss_str = if miss_tokens >= 1000 {
                        format!("{}k", miss_tokens / 1000)
                    } else {
                        format!("{}", miss_tokens)
                    };
                    spans.push(Span::styled(
                        format!(" · ⚠ {} cache miss", miss_str),
                        Style::default().fg(rgb(255, 193, 7)),
                    ));
                }

                spans.push(Span::styled(
                    " · Alt+B bg",
                    Style::default().fg(rgb(100, 100, 100)),
                ));

                if !queued_suffix.is_empty() {
                    spans.push(Span::styled(
                        queued_suffix.clone(),
                        Style::default().fg(queued_color()),
                    ));
                }
                Line::from(spans)
            }
        }
    } else if let Some((total_in, total_out)) = app.total_session_tokens() {
        let total = total_in + total_out;
        if total > 100_000 {
            let warning_color = if total > 150_000 {
                rgb(255, 100, 100)
            } else {
                rgb(255, 193, 7)
            };
            Line::from(vec![
                Span::styled("⚠ ", Style::default().fg(warning_color)),
                Span::styled(
                    format!("Session: {}k tokens ", total / 1000),
                    Style::default().fg(warning_color),
                ),
                Span::styled(
                    "(consider /clear for fresh context)",
                    Style::default().fg(dim_color()),
                ),
            ])
        } else if let Some(tip) =
            occasional_status_tip(area.width as usize, app.animation_elapsed() as u64)
        {
            Line::from(vec![Span::styled(tip, Style::default().fg(dim_color()))])
        } else {
            Line::from("")
        }
    } else {
        if let Some(tip) =
            occasional_status_tip(area.width as usize, app.animation_elapsed() as u64)
        {
            Line::from(vec![Span::styled(tip, Style::default().fg(dim_color()))])
        } else {
            Line::from("")
        }
    };

    crate::memory::check_staleness();

    let aligned_line = if app.centered_mode() {
        line.alignment(Alignment::Center)
    } else {
        line
    };
    frame.render_widget(Paragraph::new(aligned_line), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    #[test]
    fn batch_progress_spans_use_batch_chroma_for_initial_count() {
        let mut spans = Vec::new();
        let anim_color = rgb(12, 34, 56);

        append_batch_progress_spans(&mut spans, anim_color, None, Some(3));

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content.as_ref(), " · 0/3 done");
        assert_eq!(spans[0].style.fg, Some(anim_color));
        assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn batch_progress_spans_make_last_completed_explicit() {
        let mut spans = Vec::new();

        append_batch_progress_spans(
            &mut spans,
            rgb(120, 130, 140),
            Some(crate::bus::BatchProgress {
                session_id: "s".to_string(),
                tool_call_id: "tc".to_string(),
                total: 3,
                completed: 1,
                last_completed: Some("read".to_string()),
                running: Vec::new(),
                subcalls: Vec::new(),
            }),
            Some(3),
        );

        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].content.as_ref(), " · 1/3 done");
        assert_eq!(spans[1].content.as_ref(), " · last done: read");
    }

    #[test]
    fn batch_progress_spans_hide_last_completed_when_batch_finished() {
        let mut spans = Vec::new();

        append_batch_progress_spans(
            &mut spans,
            rgb(120, 130, 140),
            Some(crate::bus::BatchProgress {
                session_id: "s".to_string(),
                tool_call_id: "tc".to_string(),
                total: 3,
                completed: 3,
                last_completed: Some("read".to_string()),
                running: Vec::new(),
                subcalls: Vec::new(),
            }),
            Some(3),
        );

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content.as_ref(), " · 3/3 done");
    }

    #[test]
    fn batch_progress_spans_show_running_subcall_detail() {
        let mut spans = Vec::new();

        append_batch_progress_spans(
            &mut spans,
            rgb(120, 130, 140),
            Some(crate::bus::BatchProgress {
                session_id: "s".to_string(),
                tool_call_id: "tc".to_string(),
                total: 2,
                completed: 0,
                last_completed: None,
                running: vec![crate::message::ToolCall {
                    id: "batch-1-bash".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "cargo test -p jcode"}),
                    intent: None,
                }],
                subcalls: Vec::new(),
            }),
            Some(2),
        );

        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].content.as_ref(), " · 0/2 done");
        assert_eq!(spans[1].content.as_ref(), " · running: #1 bash");
    }

    #[test]
    fn batch_progress_spans_show_multiple_running_subcalls() {
        let mut spans = Vec::new();

        append_batch_progress_spans(
            &mut spans,
            rgb(120, 130, 140),
            Some(crate::bus::BatchProgress {
                session_id: "s".to_string(),
                tool_call_id: "tc".to_string(),
                total: 3,
                completed: 0,
                last_completed: None,
                running: vec![
                    crate::message::ToolCall {
                        id: "batch-2-grep".to_string(),
                        name: "grep".to_string(),
                        input: serde_json::json!({"pattern": "foo", "path": "src"}),
                        intent: None,
                    },
                    crate::message::ToolCall {
                        id: "batch-1-bash".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({"command": "cargo build --release --workspace"}),
                        intent: None,
                    },
                    crate::message::ToolCall {
                        id: "batch-3-read".to_string(),
                        name: "read".to_string(),
                        input: serde_json::json!({"file_path": "README.md"}),
                        intent: None,
                    },
                ],
                subcalls: Vec::new(),
            }),
            Some(3),
        );

        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].content.as_ref(), " · 0/3 done");
        assert_eq!(spans[1].content.as_ref(), " · running: #1 bash +2");
    }

    #[test]
    fn connection_phase_waiting_label_is_generic_response_wait() {
        assert_eq!(
            connection_phase_label(&ConnectionPhase::WaitingForResponse),
            "waiting for response"
        );
    }

    #[test]
    fn streaming_liveness_label_shows_quiet_stream_warning_before_message_end() {
        assert_eq!(
            streaming_liveness_label("4.2s".to_string(), Some(3.4), false),
            "(no tokens 3s) · 4.2s"
        );
        assert_eq!(
            streaming_liveness_label("12.0s".to_string(), Some(12.1), false),
            "(stalled 12s) · 12.0s"
        );
    }

    #[test]
    fn streaming_liveness_label_suppresses_quiet_stream_warning_after_message_end() {
        assert_eq!(
            streaming_liveness_label("4.2s".to_string(), Some(3.4), true),
            "4.2s"
        );
        assert_eq!(
            streaming_liveness_label("12.0s".to_string(), Some(12.1), true),
            "12.0s"
        );
    }

    #[test]
    fn display_connection_type_uses_reader_friendly_labels() {
        assert_eq!(display_connection_type("https/sse"), "https");
        assert_eq!(
            display_connection_type("websocket/persistent-fresh"),
            "websocket"
        );
        assert_eq!(
            display_connection_type("websocket/persistent-reuse"),
            "existing websocket"
        );
    }

    #[test]
    fn normalize_status_detail_uses_reader_friendly_labels() {
        assert_eq!(
            normalize_status_detail("fresh websocket").as_deref(),
            Some("opening websocket")
        );
        assert_eq!(
            normalize_status_detail("reusing websocket").as_deref(),
            Some("using existing websocket")
        );
        assert_eq!(
            normalize_status_detail("websocket healthcheck").as_deref(),
            Some("verifying websocket")
        );
        assert_eq!(
            normalize_status_detail("https fallback").as_deref(),
            Some("using https fallback")
        );
    }

    #[test]
    fn collect_transport_context_labels_dedupes_overlapping_transport_text() {
        assert_eq!(
            collect_transport_context_labels(
                normalize_status_detail("reusing websocket"),
                Some(display_connection_type("websocket/persistent-reuse")),
                Some("OpenRouter".to_string())
            ),
            vec![
                "using existing websocket".to_string(),
                "via OpenRouter".to_string()
            ]
        );

        assert_eq!(
            collect_transport_context_labels(
                normalize_status_detail("https fallback"),
                Some(display_connection_type("https/sse")),
                None,
            ),
            vec!["using https fallback".to_string()]
        );
    }

    #[test]
    fn composer_mode_detects_shell_input_before_commands() {
        assert_eq!(
            composer_mode(" ! cargo test ", false),
            ComposerMode::ShellLocal
        );
        assert_eq!(
            composer_mode("! cargo test", true),
            ComposerMode::ShellRemote
        );
        assert_eq!(composer_mode(" /help", false), ComposerMode::SlashCommand);
        assert_eq!(composer_mode("hello", false), ComposerMode::Chat);
    }

    #[test]
    fn shell_mode_hint_reflects_execution_target() {
        assert_eq!(
            shell_mode_hint(ComposerMode::ShellLocal),
            Some("  shell mode · Enter runs locally")
        );
        assert_eq!(
            shell_mode_hint(ComposerMode::ShellRemote),
            Some("  shell mode · Enter runs on server")
        );
        assert_eq!(shell_mode_hint(ComposerMode::Chat), None);
    }

    #[test]
    fn shell_mode_color_is_distinct() {
        assert_eq!(shell_mode_color(), rgb(110, 214, 151));
    }

    #[test]
    fn normalize_repaint_sensitive_notice_text_drops_warning_variation_selector() {
        assert_eq!(
            normalize_repaint_sensitive_notice_text("⚠️ File activity: read lines 1-9"),
            "⚠ File activity: read lines 1-9"
        );
        assert_eq!(
            normalize_repaint_sensitive_notice_text("all clear"),
            "all clear"
        );
    }
}

/// Build the spans for the notification line. Returns empty vec when there is nothing to show.
/// This is the single source of truth for notification content - both the layout height
/// calculation (via `has_notification`) and the renderer call this.
pub(super) fn build_notification_spans(app: &dyn TuiState) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();

    let push_sep = |spans: &mut Vec<Span<'static>>| {
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", Style::default().fg(dim_color())));
        }
    };

    if let Some(selection) = app.copy_selection_status() {
        let pane_label = selection.pane.label();
        let label = if selection.has_action {
            if selection.selected_lines > 1 {
                format!(
                    "{} selection · {} chars · {} lines · Enter/Y copy · Esc exit",
                    pane_label, selection.selected_chars, selection.selected_lines
                )
            } else {
                format!(
                    "{} selection · {} chars · Enter/Y copy · Esc exit",
                    pane_label, selection.selected_chars
                )
            }
        } else if selection.dragging {
            format!(
                "{} selection · dragging… · Enter/Y copy · Esc exit",
                pane_label
            )
        } else {
            format!("{} selection · drag to copy", pane_label)
        };
        spans.push(Span::styled(label, Style::default().fg(rgb(140, 220, 200))));
    }

    if let Some(notice) = app.status_notice() {
        push_sep(&mut spans);
        spans.push(Span::styled(
            normalize_repaint_sensitive_notice_text(&notice),
            Style::default().fg(accent_color()),
        ));
    }

    if !app.is_processing() {
        let info = app.info_widget_data();
        if let Some(schedule_notice) =
            crate::tui::scheduled_notification_text(info.ambient_info.as_ref())
        {
            push_sep(&mut spans);
            spans.push(Span::styled(
                schedule_notice,
                Style::default().fg(rgb(140, 180, 255)),
            ));
        }

        if let Some(cache_info) = app.cache_ttl_status() {
            if cache_info.is_cold {
                let tokens_str = cache_info
                    .cached_tokens
                    .map(|t| {
                        if t >= 1_000_000 {
                            format!(" ({:.1}M tok)", t as f64 / 1_000_000.0)
                        } else if t >= 1_000 {
                            format!(" ({}K tok)", t / 1000)
                        } else {
                            format!(" ({} tok)", t)
                        }
                    })
                    .unwrap_or_default();
                push_sep(&mut spans);
                spans.push(Span::styled(
                    format!("🧊 cache cold{}", tokens_str),
                    Style::default().fg(rgb(140, 180, 255)),
                ));
            } else if cache_info.remaining_secs <= 60 {
                let tokens_str = cache_info
                    .cached_tokens
                    .map(|t| {
                        if t >= 1_000 {
                            format!(" {}K", t / 1000)
                        } else {
                            format!(" {}", t)
                        }
                    })
                    .unwrap_or_default();
                push_sep(&mut spans);
                spans.push(Span::styled(
                    format!("⏳ cache {}s{}", cache_info.remaining_secs, tokens_str),
                    Style::default().fg(rgb(255, 193, 7)),
                ));
            }
        }
    }

    if app.has_stashed_input() {
        push_sep(&mut spans);
        spans.push(Span::styled(
            "📋 stash",
            Style::default().fg(rgb(255, 193, 7)),
        ));
    }

    spans
}

pub(super) fn draw_notification(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    let spans = build_notification_spans(app);
    if spans.is_empty() {
        return;
    }
    let line = Line::from(spans);
    let aligned_line = if app.centered_mode() {
        line.alignment(Alignment::Center)
    } else {
        line
    };
    frame.render_widget(Paragraph::new(aligned_line), area);
}

pub(super) fn draw_input(
    frame: &mut Frame,
    app: &dyn TuiState,
    area: Rect,
    next_prompt: usize,
    debug_capture: &mut Option<FrameCaptureBuilder>,
) {
    let input_text = app.input();
    let cursor_pos = app.cursor_pos();

    let mode = composer_mode(input_text, app.is_remote_mode());
    let suggestions = app.command_suggestions();
    let has_suggestions = !suggestions.is_empty()
        && matches!(mode, ComposerMode::SlashCommand | ComposerMode::Chat)
        && (matches!(mode, ComposerMode::SlashCommand) || !app.is_processing());

    let (prompt_char, caret_color) = input_prompt(app);
    let num_str = format!("{}", next_prompt);
    let prompt_len = input_prompt_len(app, next_prompt);
    let reserved_width = send_mode_reserved_width(app);
    let line_width = (area.width as usize).saturating_sub(prompt_len + reserved_width);

    if line_width == 0 {
        return;
    }

    let (all_lines, cursor_line, cursor_col) = wrap_input_text(
        input_text,
        cursor_pos,
        line_width,
        &num_str,
        prompt_char,
        caret_color,
        prompt_len,
    );

    let mut lines: Vec<Line> = Vec::new();
    let mut hint_shown = false;
    let mut hint_line: Option<String> = None;
    if has_suggestions {
        let input_trimmed = input_text.trim();
        let exact_match = suggestions.iter().find(|(cmd, _)| cmd == input_trimmed);

        if suggestions.len() == 1 || exact_match.is_some() {
            let (cmd, desc) = exact_match.unwrap_or(&suggestions[0]);
            let mut spans = vec![
                Span::styled("  ", Style::default().fg(dim_color())),
                Span::styled(cmd.to_string(), Style::default().fg(rgb(138, 180, 248))),
                Span::styled(format!(" - {}", desc), Style::default().fg(dim_color())),
            ];
            if suggestions.len() > 1 {
                spans.push(Span::styled(
                    format!("  Tab: +{} more", suggestions.len() - 1),
                    Style::default().fg(dim_color()),
                ));
            }
            lines.push(Line::from(spans));
        } else {
            let max_suggestions = 5;
            let limited: Vec<_> = suggestions.iter().take(max_suggestions).collect();
            let more_count = suggestions.len().saturating_sub(max_suggestions);

            let mut spans = vec![Span::styled("  Tab: ", Style::default().fg(dim_color()))];
            for (i, (cmd, desc)) in limited.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled(" │ ", Style::default().fg(dim_color())));
                }
                spans.push(Span::styled(
                    cmd.to_string(),
                    Style::default().fg(rgb(138, 180, 248)),
                ));
                if i == 0 {
                    spans.push(Span::styled(
                        format!(" ({})", desc),
                        Style::default().fg(dim_color()),
                    ));
                }
            }
            if more_count > 0 {
                spans.push(Span::styled(
                    format!(" (+{})", more_count),
                    Style::default().fg(dim_color()),
                ));
            }
            lines.push(Line::from(spans));
        }
    } else if let Some(shell_hint) = shell_mode_hint(mode) {
        hint_shown = true;
        hint_line = Some(shell_hint.trim().to_string());
        lines.push(Line::from(Span::styled(
            shell_hint,
            Style::default().fg(shell_mode_color()),
        )));
    } else if app.next_prompt_new_session_armed() {
        hint_shown = true;
        let hint = "  ↗ Next prompt opens a new session";
        hint_line = Some(hint.trim().to_string());
        lines.push(Line::from(Span::styled(
            hint,
            Style::default().fg(rgb(120, 200, 255)),
        )));
    } else if app.is_processing() && !input_text.is_empty() {
        hint_shown = true;
        let hint = if app.queue_mode() {
            "  Ctrl+Enter to send now"
        } else {
            "  Ctrl+Enter to queue"
        };
        hint_line = Some(hint.trim().to_string());
        lines.push(Line::from(Span::styled(
            hint,
            Style::default().fg(dim_color()),
        )));
    }

    if let Some(capture) = debug_capture {
        capture.rendered_text.input_area = input_text.to_string();
        if let Some(hint) = &hint_line {
            capture.rendered_text.input_hint = Some(hint.clone());
        }
        visual_debug::check_shift_enter_anomaly(
            capture,
            app.is_processing(),
            input_text,
            hint_shown,
        );
    }

    let suggestions_offset = lines.len();
    let total_input_lines = all_lines.len();
    let visible_height = area.height as usize;

    let scroll_offset = if total_input_lines + suggestions_offset <= visible_height {
        0
    } else {
        let available_for_input = visible_height.saturating_sub(suggestions_offset);
        if cursor_line < available_for_input {
            0
        } else {
            cursor_line.saturating_sub(available_for_input.saturating_sub(1))
        }
    };

    for line in all_lines.into_iter().skip(scroll_offset) {
        lines.push(line);
        if lines.len() >= visible_height {
            break;
        }
    }

    let centered = app.centered_mode();
    let paragraph = if centered {
        Paragraph::new(
            lines
                .iter()
                .map(|l| l.clone().alignment(Alignment::Center))
                .collect::<Vec<_>>(),
        )
    } else {
        Paragraph::new(lines.clone())
    };
    frame.render_widget(paragraph, area);

    let cursor_screen_line = cursor_line.saturating_sub(scroll_offset) + suggestions_offset;
    let cursor_y = area.y + (cursor_screen_line as u16).min(area.height.saturating_sub(1));

    let cursor_x = if centered {
        let actual_line_width = lines
            .get(cursor_screen_line)
            .map(|l| l.width())
            .unwrap_or(prompt_len);
        let center_offset = (area.width as usize).saturating_sub(actual_line_width) / 2;
        let cursor_offset = prompt_len + cursor_col;
        area.x + center_offset as u16 + cursor_offset as u16
    } else {
        area.x + prompt_len as u16 + cursor_col as u16
    };

    frame.set_cursor_position(Position::new(cursor_x, cursor_y));
    draw_send_mode_indicator(frame, app, area);
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WrappedInputSegment {
    text: String,
    start_char: usize,
    end_char: usize,
    display_width: usize,
}

fn wrap_input_segments(input: &str, line_width: usize) -> Vec<WrappedInputSegment> {
    use unicode_width::UnicodeWidthChar;

    let chars: Vec<char> = input.chars().collect();
    if chars.is_empty() {
        return vec![WrappedInputSegment {
            text: String::new(),
            start_char: 0,
            end_char: 0,
            display_width: 0,
        }];
    }

    let mut segments = Vec::new();
    let mut pos = 0;
    let mut char_count = 0;

    while pos <= chars.len() {
        let newline_pos = chars[pos..].iter().position(|&c| c == '\n');
        let segment_end = match newline_pos {
            Some(rel_pos) => pos + rel_pos,
            None => chars.len(),
        };

        let segment = &chars[pos..segment_end];
        let mut seg_pos = 0;
        loop {
            let mut display_width = 0;
            let mut end = seg_pos;
            while end < segment.len() {
                let cw = segment[end].width().unwrap_or(0);
                if display_width + cw > line_width {
                    break;
                }
                display_width += cw;
                end += 1;
            }
            if end == seg_pos && seg_pos < segment.len() {
                end = seg_pos + 1;
                display_width = segment[seg_pos].width().unwrap_or(0);
            }

            let text: String = segment[seg_pos..end].iter().collect();
            let start_char = char_count;
            let end_char = char_count + (end - seg_pos);
            segments.push(WrappedInputSegment {
                text,
                start_char,
                end_char,
                display_width,
            });
            char_count = end_char;

            if end >= segment.len() {
                break;
            }
            seg_pos = end;
        }

        if newline_pos.is_some() {
            char_count += 1;
            pos = segment_end + 1;
        } else {
            break;
        }
    }

    segments
}

fn cursor_col_for_segment(segment: &WrappedInputSegment, cursor_char_pos: usize) -> usize {
    use unicode_width::UnicodeWidthChar;

    let chars_before = cursor_char_pos.saturating_sub(segment.start_char);
    segment
        .text
        .chars()
        .take(chars_before)
        .map(|c| c.width().unwrap_or(0))
        .sum()
}

fn char_offset_for_clicked_column(text: &str, target_col: usize, display_width: usize) -> usize {
    use unicode_width::UnicodeWidthChar;

    if target_col >= display_width {
        return text.chars().count();
    }

    let mut display_col = 0;
    let mut chars_before = 0;
    for c in text.chars() {
        let cw = c.width().unwrap_or(0);
        if cw == 0 {
            chars_before += 1;
            continue;
        }
        if target_col < display_col + cw {
            if (target_col - display_col).saturating_mul(2) >= cw {
                chars_before += 1;
            }
            return chars_before;
        }
        display_col += cw;
        chars_before += 1;
    }

    chars_before
}

pub(crate) fn input_cursor_pos_from_screen(
    app: &dyn TuiState,
    area: Rect,
    next_prompt: usize,
    column: u16,
    row: u16,
) -> Option<usize> {
    if !layout_utils::point_in_rect(column, row, area) {
        return None;
    }

    let input_text = app.input();
    let reserved_width = send_mode_reserved_width(app);
    let prompt_len = input_prompt_len(app, next_prompt);
    let line_width = (area.width as usize).saturating_sub(prompt_len + reserved_width);
    if line_width == 0 {
        return Some(app.cursor_pos().min(input_text.len()));
    }

    let wrapped_lines = wrap_input_segments(input_text, line_width);
    let hint_lines = input_hint_line_height(app) as usize;
    let visible_height = area.height as usize;
    let total_input_lines = wrapped_lines.len().max(1);

    let scroll_offset = if total_input_lines + hint_lines <= visible_height {
        0
    } else {
        let available_for_input = visible_height.saturating_sub(hint_lines);
        let cursor_char_pos =
            crate::tui::core::byte_offset_to_char_index(input_text, app.cursor_pos());
        let cursor_line = wrapped_lines
            .iter()
            .position(|segment| {
                cursor_char_pos >= segment.start_char && cursor_char_pos <= segment.end_char
            })
            .unwrap_or_else(|| wrapped_lines.len().saturating_sub(1));
        if cursor_line < available_for_input {
            0
        } else {
            cursor_line.saturating_sub(available_for_input.saturating_sub(1))
        }
    };

    let screen_line = row.saturating_sub(area.y) as usize;
    if screen_line < hint_lines {
        return None;
    }

    let max_visible_input_lines = visible_height.saturating_sub(hint_lines).max(1);
    let input_screen_line = screen_line.saturating_sub(hint_lines);
    let line_index = (scroll_offset
        + input_screen_line.min(max_visible_input_lines.saturating_sub(1)))
    .min(wrapped_lines.len().saturating_sub(1));
    let segment = &wrapped_lines[line_index];

    let actual_line_width = prompt_len + segment.display_width;
    let text_start_x = if app.centered_mode() {
        let center_offset = (area.width as usize).saturating_sub(actual_line_width) / 2;
        area.x as usize + center_offset + prompt_len
    } else {
        area.x as usize + prompt_len
    };
    let target_col = column.saturating_sub(text_start_x as u16) as usize;
    let char_offset =
        char_offset_for_clicked_column(&segment.text, target_col, segment.display_width);
    let char_index = segment.start_char + char_offset;

    Some(crate::tui::core::char_index_to_byte_offset(
        input_text, char_index,
    ))
}

pub(crate) fn wrap_input_text<'a>(
    input: &str,
    cursor_pos: usize,
    line_width: usize,
    num_str: &str,
    prompt_char: &'a str,
    caret_color: Color,
    prompt_len: usize,
) -> (Vec<Line<'a>>, usize, usize) {
    let cursor_char_pos = crate::tui::core::byte_offset_to_char_index(input, cursor_pos);
    let wrapped_segments = wrap_input_segments(input, line_width);
    let mut lines: Vec<Line> = Vec::new();
    let mut cursor_line = 0;
    let mut cursor_col = 0;
    let mut found_cursor = false;

    for (idx, segment) in wrapped_segments.iter().enumerate() {
        if !found_cursor
            && cursor_char_pos >= segment.start_char
            && cursor_char_pos <= segment.end_char
        {
            cursor_line = idx;
            cursor_col = cursor_col_for_segment(segment, cursor_char_pos);
            found_cursor = true;
        }

        if idx == 0 {
            let num_color = rainbow_prompt_color(0);
            lines.push(Line::from(vec![
                Span::styled(num_str.to_string(), Style::default().fg(num_color)),
                Span::styled(prompt_char.to_string(), Style::default().fg(caret_color)),
                Span::raw(segment.text.clone()),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(prompt_len)),
                Span::raw(segment.text.clone()),
            ]));
        }
    }

    if !found_cursor {
        cursor_line = wrapped_segments.len().saturating_sub(1);
        cursor_col = wrapped_segments
            .last()
            .map(|segment| segment.display_width)
            .unwrap_or(0);
    }

    (lines, cursor_line, cursor_col)
}

fn send_mode_indicator(app: &dyn TuiState) -> (&'static str, Color) {
    let mode = composer_mode(app.input(), app.is_remote_mode());
    if mode.is_shell() {
        ("$", shell_mode_color())
    } else if app.next_prompt_new_session_armed() {
        ("↗", rgb(120, 200, 255))
    } else if app.queue_mode() {
        ("⏳", queued_color())
    } else if let Some(ref conn) = app.connection_type() {
        let lower = conn.to_lowercase();
        if lower.contains("websocket") {
            ("󰌘", rgb(100, 200, 180))
        } else if lower.contains("subprocess") || lower.contains("cli") {
            ("󰆍", rgb(180, 160, 220))
        } else {
            ("󰖟", rgb(140, 180, 255))
        }
    } else {
        ("⚡", asap_color())
    }
}

fn draw_send_mode_indicator(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    let (icon, color) = send_mode_indicator(app);
    if icon.is_empty() || area.width == 0 || area.height == 0 {
        return;
    }
    let indicator_area = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(1),
        width: area.width,
        height: 1,
    };
    let line = Line::from(Span::styled(icon, Style::default().fg(color)));
    let paragraph = Paragraph::new(line).alignment(Alignment::Right);
    frame.render_widget(paragraph, indicator_area);
}

#[derive(Clone, Copy)]
enum QueuedMsgType {
    Pending,
    Interleave,
    Queued,
}
