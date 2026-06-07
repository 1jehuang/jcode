use super::inline_interactive_ui::format_elapsed;
use super::tools_ui::{get_tool_summary, summarize_batch_running_tools_compact};
use super::visual_debug::{self, FrameCaptureBuilder};
use super::{
    ProcessingStatus, TuiState, accent_color, ai_color, animated_tool_color, asap_color, dim_color,
    pending_color, queued_color, rainbow_prompt_color, user_color,
};
use crate::message::ConnectionPhase;
use crate::tui::app;
use crate::tui::color_support::rgb;
use crate::tui::detect_kv_cache_problem;
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

fn command_suggestion_hint_line_count(suggestions: &[(String, &'static str)]) -> u16 {
    if suggestions.is_empty() {
        return 0;
    }

    if suggestions.len() == 1 {
        1
    } else {
        suggestions.len().min(app::COMMAND_SUGGESTION_VISIBLE_LIMIT) as u16
    }
}

fn command_suggestion_window_start(selected: usize, suggestion_count: usize) -> usize {
    if suggestion_count <= app::COMMAND_SUGGESTION_VISIBLE_LIMIT {
        0
    } else {
        selected
            .saturating_add(1)
            .saturating_sub(app::COMMAND_SUGGESTION_VISIBLE_LIMIT)
            .min(suggestion_count.saturating_sub(app::COMMAND_SUGGESTION_VISIBLE_LIMIT))
    }
}

fn should_render_suggestions_below_input(
    input_area: Rect,
    input_line_count: usize,
    suggestion_line_count: usize,
    terminal_height: u16,
) -> bool {
    suggestion_line_count > 0
        && input_area.y.saturating_add(input_area.height) < terminal_height
        && input_area
            .y
            .saturating_add(input_line_count as u16)
            .saturating_add(suggestion_line_count as u16)
            <= terminal_height
}

fn command_suggestion_lines(
    app: &dyn TuiState,
    suggestions: &[(String, &'static str)],
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if suggestions.len() == 1 {
        let (cmd, desc) = &suggestions[0];
        lines.push(Line::from(vec![
            Span::styled(cmd.to_string(), Style::default().fg(rgb(255, 213, 128))),
            Span::styled(
                format!("  {}", desc),
                Style::default().fg(rgb(255, 213, 128)),
            ),
        ]));
    } else if !suggestions.is_empty() {
        let selected = app
            .command_suggestion_selected()
            .min(suggestions.len().saturating_sub(1));
        let window_start = command_suggestion_window_start(selected, suggestions.len());
        let limited: Vec<_> = suggestions
            .iter()
            .skip(window_start)
            .take(app::COMMAND_SUGGESTION_VISIBLE_LIMIT)
            .collect();
        let window_end = window_start + limited.len();
        let more_count = suggestions.len().saturating_sub(window_end);
        let selected_visible = selected.saturating_sub(window_start);

        for (i, (cmd, desc)) in limited.iter().enumerate() {
            let is_selected = i == selected_visible;
            let description_style = if is_selected {
                Style::default().fg(rgb(255, 213, 128))
            } else {
                Style::default().fg(dim_color())
            };
            let command_style = if is_selected {
                Style::default().fg(rgb(255, 213, 128))
            } else {
                Style::default().fg(rgb(128, 203, 196))
            };
            let mut spans = Vec::new();
            spans.push(Span::styled(cmd.to_string(), command_style));
            spans.push(Span::styled(format!("  {}", desc), description_style));
            if i == 0 && window_start > 0 {
                spans.push(Span::styled(
                    format!("  ↑{}", window_start),
                    Style::default().fg(dim_color()),
                ));
            }
            if i + 1 == limited.len() && more_count > 0 {
                spans.push(Span::styled(
                    format!("  +{} more", more_count),
                    Style::default().fg(dim_color()),
                ));
            }
            lines.push(Line::from(spans));
        }
    }
    lines
}

pub(super) fn input_hint_line_height(app: &dyn TuiState) -> u16 {
    let suggestions = app.command_suggestions();
    let mode = composer_mode(app.input(), app.is_remote_mode());
    let has_suggestions = !suggestions.is_empty()
        && matches!(mode, ComposerMode::SlashCommand | ComposerMode::Chat)
        && (matches!(mode, ComposerMode::SlashCommand) || !app.is_processing());

    if has_suggestions {
        return command_suggestion_hint_line_count(&suggestions);
    }

    u16::from(
        shell_mode_hint(mode).is_some()
            || app.next_prompt_new_session_armed()
            || (app.is_processing() && !app.input().is_empty()),
    )
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

fn occasional_session_history_warning(
    total_tokens: u64,
    compaction_count: usize,
    context_limit: Option<usize>,
    width: usize,
    elapsed_secs: u64,
) -> Option<String> {
    let context_limit = context_limit.and_then(|limit| u64::try_from(limit).ok());
    let token_threshold = context_limit.unwrap_or(250_000).max(1);

    if total_tokens < token_threshold || width < 64 {
        return None;
    }

    // This is not current context usage, so keep it an occasional nudge rather
    // than a persistent warning. The first reminder is never shown before the
    // session has processed a full model context window, then it reappears at
    // context-window-sized token intervals as the session grows. Keep the
    // existing time gate as a short visibility window within each interval.
    let token_interval = token_threshold;
    let token_window = (token_interval / 20).clamp(10_000, 100_000);
    if (total_tokens - token_threshold) % token_interval >= token_window {
        return None;
    }
    if elapsed_secs % 300 >= 10 {
        return None;
    }

    let tokens = format_stream_tokens(total_tokens);
    let compactions = if compaction_count > 0 {
        format!(
            " and {compaction_count} compact{}",
            if compaction_count == 1 { "" } else { "s" }
        )
    } else {
        String::new()
    };

    Some(format!(
        "Session history: {tokens} tokens processed{compactions}; /clear starts fresh context"
    ))
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
    let (streaming_input_tokens, _) = app.streaming_tokens();
    let provider_name = app.provider_name();
    let upstream_provider = app.upstream_provider();
    let cache_ttl = app.cache_ttl_status();
    let kv_cache_problem = detect_kv_cache_problem(
        &provider_name,
        upstream_provider.as_deref(),
        user_turn_count,
        streaming_input_tokens,
        cache_read,
        cache_creation,
        cache_ttl.as_ref(),
    );

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
                push_queued_suffix(&mut spans, &queued_suffix);
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
                push_queued_suffix(&mut spans, &queued_suffix);
                Line::from(spans)
            }
            ProcessingStatus::Thinking(_start) => {
                let mut label = format!(" thinking… {}", format_elapsed(elapsed));
                append_transport_context(&mut label, app);
                let mut spans = vec![
                    Span::styled(spinner, Style::default().fg(ai_color())),
                    Span::styled(label, Style::default().fg(dim_color())),
                ];
                push_queued_suffix(&mut spans, &queued_suffix);
                Line::from(spans)
            }
            ProcessingStatus::Streaming => {
                let time_str = format_elapsed(elapsed);
                let (input_tokens, output_tokens) = app.streaming_tokens();
                let stream_message_ended = app.stream_message_ended();
                let mut status_text =
                    streaming_liveness_label(time_str, stale_secs, stream_message_ended);
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
                if let Some(problem) = kv_cache_problem {
                    let miss_tokens = problem.affected_tokens.unwrap_or(0);
                    let miss_str = if miss_tokens >= 1000 {
                        format!("{}k", miss_tokens / 1000)
                    } else if miss_tokens > 0 {
                        format!("{}", miss_tokens)
                    } else {
                        "kv".to_string()
                    };
                    status_text = format!("⚠ {} cache miss · {}", miss_str, status_text);
                }
                let spans = streaming_status_spans(
                    spinner,
                    status_text,
                    stream_message_ended,
                    kv_cache_problem.is_some(),
                    &queued_suffix,
                );
                Line::from(spans)
            }
            ProcessingStatus::WaitingForNetwork { listener } => {
                let mut spans = vec![
                    Span::styled("↻ ", Style::default().fg(rgb(255, 193, 7))),
                    Span::styled(
                        format!(
                            "network disconnected, waiting to retry · {} · {}",
                            listener,
                            format_elapsed(elapsed)
                        ),
                        Style::default().fg(rgb(255, 193, 7)),
                    ),
                ];
                push_queued_suffix(&mut spans, &queued_suffix);
                Line::from(spans)
            }
            ProcessingStatus::RunningTool(ref name) => {
                let half_width = 3;
                let decorative = crate::perf::tui_policy().enable_decorative_animations;
                // When decorative animations are disabled we still nudge the bar
                // forward at a slow "liveness" rate so a long-running tool (e.g.
                // bash) reads as alive instead of frozen.
                let bar_speed = if decorative {
                    2.0
                } else {
                    jcode_tui_style::theme::LIVENESS_INDICATOR_FPS / half_width as f32
                };
                let progress = elapsed * bar_speed % 1.0;
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
                let experimental_notice = app.active_experimental_feature_notice();
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

                if let Some(notice) = experimental_notice {
                    spans.push(Span::styled(
                        format!(" · ⚠ {}", notice),
                        Style::default().fg(rgb(255, 193, 7)).bold(),
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

                if let Some(problem) = kv_cache_problem {
                    let miss_tokens = problem.affected_tokens.unwrap_or(0);
                    let miss_str = if miss_tokens >= 1000 {
                        format!("{}k", miss_tokens / 1000)
                    } else if miss_tokens > 0 {
                        format!("{}", miss_tokens)
                    } else {
                        "kv".to_string()
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

                push_queued_suffix(&mut spans, &queued_suffix);
                Line::from(spans)
            }
        }
    } else if let Some((total_in, total_out)) = app.total_session_tokens() {
        let total = total_in + total_out;
        if let Some(warning) = occasional_session_history_warning(
            total,
            app.session_compaction_count(),
            app.context_limit(),
            area.width as usize,
            app.animation_elapsed() as u64,
        ) {
            let severe_token_threshold = app
                .context_limit()
                .and_then(|limit| u64::try_from(limit).ok())
                .map(|limit| limit.saturating_mul(3))
                .unwrap_or(1_000_000);
            let warning_color =
                if total >= severe_token_threshold || app.session_compaction_count() >= 3 {
                    rgb(255, 100, 100)
                } else {
                    rgb(255, 193, 7)
                };
            Line::from(vec![
                Span::styled("⚠ ", Style::default().fg(warning_color)),
                Span::styled(warning, Style::default().fg(warning_color)),
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

/// Append the "+N queued" suffix span (in the queued accent color) when there
/// are queued follow-up messages. Centralizes the repeated check/styling shared
/// by every processing-status branch in `draw_status`.
fn push_queued_suffix(spans: &mut Vec<Span<'static>>, queued_suffix: &str) {
    if !queued_suffix.is_empty() {
        spans.push(Span::styled(
            queued_suffix.to_string(),
            Style::default().fg(queued_color()),
        ));
    }
}

fn streaming_status_spans(
    spinner: &'static str,
    status_text: String,
    _stream_message_ended: bool,
    has_warning: bool,
    queued_suffix: &str,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    spans.push(Span::styled(spinner, Style::default().fg(ai_color())));
    spans.push(Span::styled(
        format!(" {}", status_text),
        Style::default().fg(if has_warning {
            rgb(255, 193, 7)
        } else {
            dim_color()
        }),
    ));
    push_queued_suffix(&mut spans, queued_suffix);
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    #[test]
    fn session_history_warning_is_clear_and_occasional() {
        assert!(occasional_session_history_warning(249_999, 0, None, 100, 0).is_none());
        assert!(occasional_session_history_warning(300_000, 0, None, 63, 0).is_none());
        assert!(occasional_session_history_warning(300_000, 0, None, 100, 10).is_none());
        assert!(occasional_session_history_warning(199_999, 0, Some(200_000), 100, 0).is_none());
        assert!(occasional_session_history_warning(300_000, 0, Some(400_000), 100, 0).is_none());
        assert!(occasional_session_history_warning(450_000, 0, Some(400_000), 100, 0).is_none());

        let warning = occasional_session_history_warning(2_500_000, 4, Some(500_000), 100, 0)
            .expect("large sessions should get a brief reminder");
        assert!(warning.contains("Session history: 2.5M tokens processed and 4 compacts"));
        assert!(warning.contains("/clear starts fresh context"));
        assert!(!warning.contains("Context usage"));
    }

    #[test]
    fn command_suggestion_hint_line_count_reserves_vertical_rows() {
        let suggestions = vec![
            ("/help".to_string(), "Show help"),
            ("/history".to_string(), "Show history"),
            ("/handoff".to_string(), "Prepare handoff"),
            ("/health".to_string(), "Show health"),
            ("/hide".to_string(), "Hide panel"),
            ("/hello".to_string(), "Say hello"),
            ("/hold".to_string(), "Hold state"),
            ("/home".to_string(), "Go home"),
            ("/hover".to_string(), "Show hover"),
        ];

        assert_eq!(
            command_suggestion_hint_line_count(&suggestions),
            app::COMMAND_SUGGESTION_VISIBLE_LIMIT as u16
        );
        assert_eq!(
            command_suggestion_hint_line_count(&suggestions),
            app::COMMAND_SUGGESTION_VISIBLE_LIMIT as u16
        );
        assert_eq!(command_suggestion_hint_line_count(&suggestions[..1]), 1);
    }

    #[test]
    fn command_suggestion_window_start_scrolls_after_visible_limit() {
        let limit = app::COMMAND_SUGGESTION_VISIBLE_LIMIT;
        assert_eq!(command_suggestion_window_start(0, limit + 3), 0);
        assert_eq!(command_suggestion_window_start(limit - 1, limit + 3), 0);
        assert_eq!(command_suggestion_window_start(limit, limit + 3), 1);
        assert_eq!(command_suggestion_window_start(limit + 2, limit + 3), 3);
    }

    #[test]
    fn command_suggestions_render_below_when_terminal_space_remains() {
        let input_area = Rect::new(0, 10, 80, 4);

        assert!(should_render_suggestions_below_input(input_area, 1, 3, 20));
    }

    #[test]
    fn command_suggestions_render_above_at_terminal_bottom() {
        let input_area = Rect::new(0, 16, 80, 4);

        assert!(!should_render_suggestions_below_input(input_area, 1, 3, 20));
    }

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
                    thought_signature: None,
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
                        thought_signature: None,
                    },
                    crate::message::ToolCall {
                        id: "batch-1-bash".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({"command": "cargo build --release --workspace"}),
                        intent: None,
                        thought_signature: None,
                    },
                    crate::message::ToolCall {
                        id: "batch-3-read".to_string(),
                        name: "read".to_string(),
                        input: serde_json::json!({"file_path": "README.md"}),
                        intent: None,
                        thought_signature: None,
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
    fn streaming_status_spans_keep_spinner_while_finalizing() {
        let spans = streaming_status_spans("⠋", "4.2s".to_string(), false, false, " · +1 queued");

        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].content.as_ref(), "⠋");
        assert_eq!(spans[1].content.as_ref(), " 4.2s");
        assert_eq!(spans[2].content.as_ref(), " · +1 queued");
    }

    #[test]
    fn streaming_status_spans_keep_spinner_after_message_end_while_finalizing() {
        let spans = streaming_status_spans("⠋", "finalizing".to_string(), true, false, "");

        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].content.as_ref(), "⠋");
        assert_eq!(spans[1].content.as_ref(), " finalizing");
    }

    #[test]
    fn push_queued_suffix_appends_only_when_present() {
        let mut spans: Vec<Span<'static>> = Vec::new();
        push_queued_suffix(&mut spans, "");
        assert!(spans.is_empty(), "empty suffix should add no span");

        push_queued_suffix(&mut spans, " · +2 queued");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content.as_ref(), " · +2 queued");
        assert_eq!(spans[0].style.fg, Some(queued_color()));
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

    if let Some(flicker_notice) = super::recent_flicker_ui_notice() {
        let copy_badge_ui = app.copy_badge_ui();
        let copy_badge_now = std::time::Instant::now();
        let key = super::FLICKER_NOTICE_COPY_KEY;
        let alt_style = if copy_badge_ui.alt_active {
            Style::default().fg(accent_color()).bold()
        } else {
            Style::default().fg(dim_color())
        };
        let shift_style = if copy_badge_ui.shift_active {
            Style::default().fg(accent_color()).bold()
        } else {
            Style::default().fg(dim_color())
        };
        let key_style = if copy_badge_ui.key_is_active(key, copy_badge_now) {
            Style::default().fg(accent_color()).bold()
        } else {
            Style::default().fg(dim_color())
        };

        push_sep(&mut spans);
        spans.push(Span::styled(
            flicker_notice.summary,
            Style::default().fg(rgb(255, 193, 7)),
        ));
        push_sep(&mut spans);
        spans.push(Span::styled(
            flicker_notice.hint,
            Style::default().fg(rgb(140, 180, 255)),
        ));
        spans.push(Span::raw(" "));
        if let Some(success) = copy_badge_ui.feedback_for_key(key, copy_badge_now) {
            let feedback_style = if success {
                Style::default().fg(ai_color()).bold()
            } else {
                Style::default().fg(Color::Red).bold()
            };
            let feedback_text = if success {
                "✓ Copied! "
            } else {
                "✗ Copy failed "
            };
            spans.push(Span::styled(feedback_text, feedback_style));
        }
        spans.push(Span::styled(
            super::viewport::copy_badge_alt_badge(),
            alt_style,
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled("[⇧]", shift_style));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!("[{}]", key.to_ascii_uppercase()),
            key_style,
        ));
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

/// Draw the elastic overscroll status line, revealed below the input when the
/// user scrolls past the bottom of the transcript. Shows model, provider,
/// access method, reasoning level, and context usage percentage.
pub(super) fn draw_overscroll_status(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let data = app.info_widget_data();

    let sep = || Span::styled(" · ", Style::default().fg(rgb(100, 100, 110)));
    let mut spans: Vec<Span> = Vec::new();

    // Model
    let model = data
        .model
        .clone()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| app.provider_model());
    if !model.is_empty() && !overscroll_is_placeholder(&model) {
        spans.push(Span::styled(
            model,
            Style::default().fg(rgb(255, 150, 200)).bold(),
        ));
        // Reasoning level shown inline next to the model, e.g. " high".
        if let Some(effort) = data
            .reasoning_effort
            .as_deref()
            .and_then(overscroll_short_reasoning)
        {
            spans.push(Span::styled(
                format!(" {}", effort),
                Style::default().fg(rgb(140, 140, 150)),
            ));
        }
    }

    // Provider
    let provider = data
        .provider_name
        .clone()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| app.provider_name());
    if !provider.is_empty() && !overscroll_is_runtime_placeholder(&provider) {
        if !spans.is_empty() {
            spans.push(sep());
        }
        spans.push(Span::styled(
            overscroll_provider_display(&provider),
            Style::default().fg(rgb(140, 180, 255)),
        ));
    }

    // Access method (auth)
    if let Some((label, color)) = overscroll_auth_label(data.auth_method) {
        if !spans.is_empty() {
            spans.push(sep());
        }
        spans.push(Span::styled(label.to_string(), Style::default().fg(color)));
    }

    // Context usage as a rounded bar
    if let Some((used, limit)) = overscroll_context_usage(&data) {
        if !spans.is_empty() {
            spans.push(sep());
        }
        spans.push(Span::styled(
            format!(
                "{}/{} ",
                overscroll_format_tokens(used),
                overscroll_format_tokens(limit)
            ),
            Style::default().fg(rgb(140, 140, 150)),
        ));
        spans.extend(overscroll_context_bar(used, limit, 10));
    }

    // Working directory last (right side), shown as a home-relative path.
    if let Some(dir) = app.working_dir().and_then(|d| overscroll_dir_label(&d)) {
        if !spans.is_empty() {
            spans.push(sep());
        }
        spans.push(Span::styled(" ", Style::default().fg(rgb(140, 180, 255))));
        spans.push(Span::styled(dir, Style::default().fg(rgb(140, 140, 150))));
    }

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

/// Format a working dir path home-relative (~/foo/bar), keeping the last 2 segments.
fn overscroll_dir_label(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let display = if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy();
        if !home.is_empty() && (trimmed == home || trimmed.starts_with(&format!("{home}/"))) {
            format!("~{}", &trimmed[home.len()..])
        } else {
            trimmed.to_string()
        }
    } else {
        trimmed.to_string()
    };
    // Keep it short: show at most the last two path segments.
    let segs: Vec<&str> = display.split('/').filter(|s| !s.is_empty()).collect();
    let short = if display.starts_with('~') {
        if segs.len() <= 2 {
            display.clone()
        } else {
            format!("~/…/{}", segs[segs.len() - 1])
        }
    } else if segs.len() <= 2 {
        format!("/{}", segs.join("/"))
    } else {
        format!("…/{}/{}", segs[segs.len() - 2], segs[segs.len() - 1])
    };
    Some(short)
}

/// Placeholder header strings used during remote startup; not real model names.
fn overscroll_is_placeholder(model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    m.starts_with("connecting")
        || m.starts_with("loading")
        || m == "connected"
        || m.contains("connecting to server")
}

/// Inert runtime provider labels (remote/replay/test-harness) shown before the
/// real provider name is known; not real provider names.
fn overscroll_is_runtime_placeholder(provider: &str) -> bool {
    matches!(
        provider.trim().to_ascii_lowercase().as_str(),
        "remote" | "replay" | "test-harness"
    )
}

fn overscroll_provider_display(provider: &str) -> String {
    match provider.to_ascii_lowercase().as_str() {
        "claude" => "Claude OAuth".to_string(),
        "anthropic" => "Anthropic API".to_string(),
        "openai" => "OpenAI".to_string(),
        "openrouter" => "OpenRouter".to_string(),
        "opencode" => "OpenCode".to_string(),
        "gemini" => "Gemini".to_string(),
        "copilot" => "GitHub Copilot".to_string(),
        "cursor" => "Cursor".to_string(),
        "bedrock" => "AWS Bedrock".to_string(),
        "antigravity" => "Antigravity".to_string(),
        _ => provider.to_string(),
    }
}

fn overscroll_auth_label(
    method: crate::tui::info_widget::AuthMethod,
) -> Option<(&'static str, Color)> {
    use crate::tui::info_widget::AuthMethod;
    match method {
        AuthMethod::Unknown => None,
        AuthMethod::ApiKey | AuthMethod::AnthropicApiKey | AuthMethod::OpenAIApiKey => {
            Some(("API key", rgb(180, 180, 190)))
        }
        AuthMethod::OpenRouterApiKey | AuthMethod::OpenCodeApiKey => {
            Some(("API key", rgb(140, 180, 255)))
        }
        AuthMethod::AnthropicOAuth => Some(("OAuth", rgb(255, 160, 100))),
        AuthMethod::OpenAIOAuth => Some(("OAuth", rgb(100, 200, 180))),
        AuthMethod::CopilotOAuth => Some(("OAuth", rgb(110, 200, 140))),
        AuthMethod::GeminiOAuth => Some(("OAuth", rgb(120, 190, 255))),
    }
}

fn overscroll_short_reasoning(effort: &str) -> Option<&str> {
    let effort = effort.trim();
    if effort.is_empty() {
        return None;
    }
    Some(match effort {
        "xhigh" => "xhigh",
        "high" => "high",
        "medium" => "medium",
        "low" => "low",
        "none" => "none",
        other => other,
    })
}

fn overscroll_context_usage(
    data: &crate::tui::info_widget::InfoWidgetData,
) -> Option<(usize, usize)> {
    let used_tokens = if let Some(observed) = data.observed_context_tokens {
        observed as usize
    } else {
        let info = data.context_info.as_ref()?;
        if info.total_chars == 0 {
            return None;
        }
        info.estimated_tokens()
    };
    let limit = data
        .context_limit
        .unwrap_or(crate::provider::DEFAULT_CONTEXT_LIMIT)
        .max(1);
    Some((used_tokens, limit))
}

/// Format a token count compactly: 1234 -> "1.2k", 200000 -> "200k", 1_500_000 -> "1.5M".
fn overscroll_format_tokens(tokens: usize) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 10_000 {
        format!("{}k", tokens / 1000)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1000.0)
    } else {
        tokens.to_string()
    }
}

/// Render a compact rounded progress bar (◖████░░◗) plus a percentage label.
fn overscroll_context_bar(used: usize, limit: usize, cells: usize) -> Vec<Span<'static>> {
    let limit = limit.max(1);
    let ratio = (used as f64 / limit as f64).clamp(0.0, 1.0);
    let pct = (ratio * 100.0).round() as u16;
    let filled = (ratio * cells as f64).round() as usize;
    let filled = filled.min(cells);

    // Match the info widget usage bar palette (based on remaining context).
    let left_pct = 100u16.saturating_sub(pct);
    let fill_color = if left_pct <= 20 {
        rgb(255, 100, 100)
    } else if left_pct <= 50 {
        rgb(255, 200, 100)
    } else {
        rgb(100, 200, 100)
    };
    let track_color = rgb(50, 50, 60);

    let mut spans = Vec::with_capacity(cells + 2);
    // Slim segmented pill (▰ filled / ▱ empty) reads thinner than full blocks.
    spans.push(Span::styled(
        "▰".repeat(filled),
        Style::default().fg(fill_color),
    ));
    spans.push(Span::styled(
        "▱".repeat(cells.saturating_sub(filled)),
        Style::default().fg(track_color),
    ));
    spans.push(Span::styled(
        format!(" {}%", pct),
        Style::default().fg(fill_color).bold(),
    ));
    spans
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
    let mut suggestion_lines: Vec<Line> = Vec::new();
    if has_suggestions {
        suggestion_lines = command_suggestion_lines(app, &suggestions);
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

    let render_suggestions_below = should_render_suggestions_below_input(
        area,
        all_lines.len().min(10),
        suggestion_lines.len(),
        frame.area().height,
    );

    if has_suggestions && !render_suggestions_below {
        lines.extend(suggestion_lines.iter().cloned());
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

    if has_suggestions && render_suggestions_below {
        for line in suggestion_lines {
            if lines.len() >= visible_height {
                break;
            }
            lines.push(line);
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
