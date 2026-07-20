use super::inline_interactive_ui::format_elapsed;
use super::selection_highlight::highlight_line_selection;
use super::tools_ui::{get_tool_activity_detail, summarize_batch_running_tools_compact};
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
use crate::tui::session_facts;
use ratatui::{prelude::*, style::Modifier, widgets::Paragraph};

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

/// Where the command-suggestion popover should render, given the input chunk.
///
/// Suggestions are an overlay: they never participate in the vertical layout,
/// so opening the palette never moves the transcript, the status line, or the
/// pinned footer. Preferred position is directly below the input (over blank
/// rows / the pinned footer); when the input sits at the bottom of the screen
/// the popover flips above the input, covering the last transcript rows
/// instead of reflowing them.
fn command_suggestions_overlay_rect(
    input_area: Rect,
    line_count: u16,
    frame_area: Rect,
) -> Option<Rect> {
    if line_count == 0 || input_area.width == 0 {
        return None;
    }
    let below_start = input_area.y.saturating_add(input_area.height);
    let space_below = frame_area.bottom().saturating_sub(below_start);
    if space_below >= line_count {
        return Some(Rect::new(
            input_area.x,
            below_start,
            input_area.width,
            line_count,
        ));
    }
    let space_above = input_area.y.saturating_sub(frame_area.y);
    let above_rows = line_count.min(space_above);
    if above_rows >= line_count.min(space_below.max(1)) && above_rows > 0 {
        return Some(Rect::new(
            input_area.x,
            input_area.y - above_rows,
            input_area.width,
            above_rows,
        ));
    }
    if space_below > 0 {
        return Some(Rect::new(
            input_area.x,
            below_start,
            input_area.width,
            space_below,
        ));
    }
    None
}

/// Whether the command palette is active for the current composer state.
fn command_suggestions_active(app: &dyn TuiState, suggestions: &[(String, &'static str)]) -> bool {
    let mode = composer_mode(app.input(), app.is_remote_mode());
    !suggestions.is_empty()
        && matches!(mode, ComposerMode::SlashCommand | ComposerMode::Chat)
        && (matches!(mode, ComposerMode::SlashCommand) || !app.is_processing())
}

/// Draw the command-suggestion popover as a late overlay pass.
///
/// Called after the chunked layout (and info widgets) have rendered so the
/// palette floats over existing rows instead of reserving layout height.
pub(super) fn draw_command_suggestions_overlay(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    let suggestions = app.command_suggestions();
    if !command_suggestions_active(app, &suggestions) {
        return;
    }
    let mut lines = command_suggestion_lines(app, &suggestions);
    let Some(rect) = command_suggestions_overlay_rect(area, lines.len() as u16, frame.area())
    else {
        return;
    };
    lines.truncate(rect.height as usize);
    if app.centered_mode() {
        lines = lines
            .into_iter()
            .map(|l| l.alignment(Alignment::Center))
            .collect();
    }
    frame.render_widget(ratatui::widgets::Clear, rect);
    frame.render_widget(Paragraph::new(lines), rect);
}

fn command_suggestion_lines(
    app: &dyn TuiState,
    suggestions: &[(String, &'static str)],
) -> Vec<Line<'static>> {
    // Highlight the characters of each command that the typed query matched.
    // We only highlight the command token itself (the part before the first
    // space), matched against the corresponding leading token of the input.
    let needle = command_suggestion_needle(app.input());
    let highlight = |cmd: &str, base: Style| -> Vec<Span<'static>> {
        highlight_command_spans(cmd, needle.as_deref(), base)
    };

    let mut lines = Vec::new();
    if suggestions.len() == 1 {
        let (cmd, desc) = &suggestions[0];
        let base = Style::default().fg(rgb(255, 213, 128));
        let mut spans = highlight(cmd, base);
        spans.push(Span::styled(format!("  {}", desc), base));
        lines.push(Line::from(spans));
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
            let mut spans = highlight(cmd, command_style);
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

/// Extract the slash-command portion of the typed input that should be matched
/// against suggestion command tokens for highlighting purposes.
fn command_suggestion_needle(input: &str) -> Option<String> {
    let trimmed = input.trim_start();
    if !trimmed.starts_with('/') {
        return None;
    }
    Some(trimmed.to_string())
}

/// Build spans for a suggestion command, recoloring the characters that the
/// fuzzy matcher aligned with the typed query. Falls back to a single
/// unhighlighted span when there is nothing (useful) to highlight.
fn highlight_command_spans(cmd: &str, needle: Option<&str>, base: Style) -> Vec<Span<'static>> {
    let positions: Vec<usize> = match needle {
        Some(n) if !n.is_empty() && n != "/" => crate::tui::fuzzy::fuzzy_match_positions(n, cmd),
        _ => Vec::new(),
    };
    if positions.is_empty() {
        return vec![Span::styled(cmd.to_string(), base)];
    }

    // Recolor (rather than underline) the matched characters so they stay in
    // the command palette's own hue: matched chars are a brighter version of
    // the line's base color, while unmatched chars are dimmed so the match
    // visually pops.
    let highlight_style = base
        .fg(brighten_command_color(base.fg))
        .add_modifier(Modifier::BOLD);
    let rest_style = base.fg(dim_command_color(base.fg));
    let chars: Vec<char> = cmd.chars().collect();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run_start = 0usize;
    let mut run_is_match = !chars.is_empty() && positions.contains(&0);
    for i in 1..=chars.len() {
        let cur_is_match = i < chars.len() && positions.contains(&i);
        if cur_is_match != run_is_match || i == chars.len() {
            let chunk: String = chars[run_start..i].iter().collect();
            spans.push(Span::styled(
                chunk,
                if run_is_match {
                    highlight_style
                } else {
                    rest_style
                },
            ));
            run_start = i;
            run_is_match = cur_is_match;
        }
    }
    spans
}

/// Blend a palette color toward white to emphasize a matched character while
/// keeping its original hue.
pub(crate) fn brighten_command_color(color: Option<Color>) -> Color {
    match color {
        Some(Color::Rgb(r, g, b)) => {
            let lift = |c: u8| -> u8 { c.saturating_add((255 - c) / 2) };
            rgb(lift(r), lift(g), lift(b))
        }
        _ => rgb(255, 255, 255),
    }
}

/// Blend a palette color toward black so unmatched characters recede.
pub(crate) fn dim_command_color(color: Option<Color>) -> Color {
    match color {
        Some(Color::Rgb(r, g, b)) => rgb(r / 2, g / 2, b / 2),
        other => other.unwrap_or_else(dim_color),
    }
}

pub(super) fn input_hint_line_height(app: &dyn TuiState) -> u16 {
    // Command suggestions render as an overlay (see
    // `draw_command_suggestions_overlay`) and intentionally reserve no layout
    // height: opening the palette must not move the transcript or footer.
    if command_suggestions_active(app, &app.command_suggestions()) {
        return 0;
    }
    let mode = composer_mode(app.input(), app.is_remote_mode());

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
                // "Suspiciously long" is measured per connection attempt, not
                // across the whole turn, so later round-trips don't immediately
                // render yellow just because the turn has been running a while.
                let phase_elapsed = app
                    .connection_phase_elapsed()
                    .map_or(elapsed, |d| d.as_secs_f32());
                let label_color = match phase {
                    crate::message::ConnectionPhase::Retrying { .. } => rgb(255, 193, 7),
                    crate::message::ConnectionPhase::Authenticating if phase_elapsed > 10.0 => {
                        rgb(255, 193, 7)
                    }
                    crate::message::ConnectionPhase::Connecting if phase_elapsed > 10.0 => {
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
                        .map(get_tool_activity_detail)
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

    if app.centered_mode() {
        frame.render_widget(Paragraph::new(line.alignment(Alignment::Center)), area);
        return;
    }
    frame.render_widget(Paragraph::new(line), area);
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
    fn right_fact_stack_shifts_up_as_a_unit_when_bottom_row_is_occupied() {
        let area = Rect::new(0, 0, 40, 5);
        let mut buffer = ratatui::buffer::Buffer::empty(area);
        for x in 12..40 {
            buffer[(x, 4)].set_symbol("x");
        }
        let lines = ["oauth", "model", "dir", "context"]
            .into_iter()
            .map(|text| RightFactLine::new(vec![Span::raw(text)]).expect("fact line"))
            .collect();

        let placements = right_fact_placements(
            &buffer,
            lines,
            0,
            5,
            0,
            40,
            Rect::new(0, 0, 40, 3),
            false,
            None,
        );
        let rows = placements
            .iter()
            .map(|placement| placement.area.y)
            .collect::<Vec<_>>();
        assert_eq!(rows, vec![3, 2, 1, 0]);
        assert!(placements.iter().all(|placement| placement.area.y != 4));
    }

    #[test]
    fn right_fact_stack_treats_styled_blank_cells_as_occupied() {
        let area = Rect::new(0, 0, 32, 2);
        let mut buffer = ratatui::buffer::Buffer::empty(area);
        for x in 12..32 {
            buffer[(x, 1)].set_bg(Color::Blue);
        }
        let line = RightFactLine::new(vec![Span::raw("context")]).expect("fact line");
        let placements = right_fact_placements(
            &buffer,
            vec![line],
            0,
            2,
            0,
            32,
            Rect::new(0, 0, 32, 1),
            false,
            None,
        );
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].area.y, 0);
    }

    #[test]
    fn right_fact_stack_never_draws_over_the_input_cursor() {
        let area = Rect::new(0, 0, 32, 2);
        let buffer = ratatui::buffer::Buffer::empty(area);
        let line = RightFactLine::new(vec![Span::raw("context")]).expect("fact line");
        let placements = right_fact_placements(
            &buffer,
            vec![line],
            0,
            2,
            0,
            32,
            Rect::new(0, 0, 32, 1),
            false,
            Some(Position::new(28, 1)),
        );
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].area.y, 0);
    }

    #[test]
    fn overscroll_provider_display_is_credential_neutral() {
        // The credential (OAuth vs API key) is reported by the adjacent auth
        // chip from canonical resolution; the provider name must not bake in a
        // credential or the two can contradict (e.g. "Claude OAuth · API key").
        assert_eq!(overscroll_provider_display("claude"), "Claude");
        assert_eq!(overscroll_provider_display("anthropic"), "Anthropic");
        assert!(!overscroll_provider_display("claude").contains("OAuth"));
        assert!(!overscroll_provider_display("anthropic").contains("API"));
    }

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
    fn command_suggestion_window_start_scrolls_after_visible_limit() {
        let limit = app::COMMAND_SUGGESTION_VISIBLE_LIMIT;
        assert_eq!(command_suggestion_window_start(0, limit + 3), 0);
        assert_eq!(command_suggestion_window_start(limit - 1, limit + 3), 0);
        assert_eq!(command_suggestion_window_start(limit, limit + 3), 1);
        assert_eq!(command_suggestion_window_start(limit + 2, limit + 3), 3);
    }

    #[test]
    fn command_suggestions_overlay_prefers_space_below_input() {
        let frame = Rect::new(0, 0, 80, 20);
        let input_area = Rect::new(0, 10, 80, 1);

        assert_eq!(
            command_suggestions_overlay_rect(input_area, 3, frame),
            Some(Rect::new(0, 11, 80, 3))
        );
    }

    #[test]
    fn command_suggestions_overlay_flips_above_at_terminal_bottom() {
        let frame = Rect::new(0, 0, 80, 20);
        let input_area = Rect::new(0, 19, 80, 1);

        assert_eq!(
            command_suggestions_overlay_rect(input_area, 3, frame),
            Some(Rect::new(0, 16, 80, 3))
        );
    }

    #[test]
    fn command_suggestions_overlay_handles_zero_lines_and_no_space() {
        let frame = Rect::new(0, 0, 80, 20);
        let input_area = Rect::new(0, 10, 80, 1);
        assert_eq!(command_suggestions_overlay_rect(input_area, 0, frame), None);

        // Input fills the whole frame: nowhere to float, so no popover.
        let full = Rect::new(0, 0, 80, 20);
        assert_eq!(command_suggestions_overlay_rect(full, 3, frame), None);
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

    // Learned-keybinding nudge: distinct bright color + bold so the user reads
    // it as "the system noticed I'm not using a shortcut", not a normal status.
    if let Some(hint) = app.learn_hint() {
        push_sep(&mut spans);
        spans.push(Span::styled(
            normalize_repaint_sensitive_notice_text(&hint),
            Style::default().fg(rgb(214, 122, 255)).bold(),
        ));
    }

    // Inline hotkey feedback: what a rarely-used chord just did, or the nearest
    // binding for an unbound chord. Cool cyan so it reads as informational.
    if let Some(feedback) = app.hotkey_feedback() {
        push_sep(&mut spans);
        spans.push(Span::styled(
            normalize_repaint_sensitive_notice_text(&feedback),
            Style::default().fg(rgb(102, 204, 221)),
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
                // Small gray "how long ago it went cold" hint, e.g. `1h 1m`.
                spans.push(Span::styled(
                    format!(
                        " {}",
                        crate::tui::format_compact_age(cache_info.cold_for_secs)
                    ),
                    Style::default().fg(dim_color()),
                ));
            } else if cache_info.expiring_soon() {
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
                // Above a minute, a raw seconds count is hard to read at a
                // glance; show minutes granularity instead.
                let remaining = cache_info.remaining_secs;
                let time_str = if remaining > 60 {
                    format!("{}m", remaining.div_ceil(60))
                } else {
                    format!("{}s", remaining)
                };
                push_sep(&mut spans);
                spans.push(Span::styled(
                    format!("⏳ cache {}{}", time_str, tokens_str),
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
/// access method, reasoning level, and context usage percentage, with a live
/// `(overscroll x.x)` countdown pinned to the right so users can see the line
/// is temporary and rebounds away on its own.
pub(super) fn draw_overscroll_status(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let data = app.info_widget_data();

    let sep = || Span::styled(" · ", Style::default().fg(rgb(100, 100, 110)));

    // The countdown is the priority affordance: it explains the line exists and
    // is going away. Build it first so it always gets space on the right edge.
    let countdown: Option<Span> = app.chat_overscroll_remaining().map(|secs| {
        Span::styled(
            format!("(overscroll {:.1})", secs.max(0.0)),
            Style::default().fg(rgb(150, 150, 165)).italic(),
        )
    });

    let mut spans: Vec<Span> = Vec::new();

    // Model
    let model = data
        .model
        .clone()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| app.provider_model());
    if !model.is_empty() && !overscroll_is_placeholder(&model) {
        spans.push(Span::styled(
            session_facts::pretty_model(&model),
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

    // Working directory last, shown as a home-relative path.
    if let Some(dir) = app.working_dir().and_then(|d| overscroll_dir_label(&d)) {
        if !spans.is_empty() {
            spans.push(sep());
        }
        spans.push(Span::styled(" ", Style::default().fg(rgb(140, 180, 255))));
        spans.push(Span::styled(dir, Style::default().fg(rgb(140, 140, 150))));
    }

    let total_width = area.width as usize;

    // No countdown active: just render the info line (centered or not) as before.
    let Some(countdown) = countdown else {
        if spans.is_empty() {
            return;
        }
        let line = Line::from(overscroll_truncate_spans(spans, total_width));
        let aligned_line = if app.centered_mode() {
            line.alignment(Alignment::Center)
        } else {
            line
        };
        frame.render_widget(Paragraph::new(aligned_line), area);
        return;
    };

    let countdown_width = countdown.content.chars().count();

    // Tight width: if there is not even room for the countdown plus a single
    // space of breathing room, drop the info entirely and just show the
    // countdown (truncated as a last resort). The affordance survives.
    if total_width <= countdown_width + 1 {
        let countdown_line = Line::from(overscroll_truncate_spans(vec![countdown], total_width))
            .alignment(Alignment::Right);
        frame.render_widget(Paragraph::new(countdown_line), area);
        return;
    }

    // Reserve the countdown on the right; the info line gets the rest and is
    // truncated to fit so the two never collide.
    let gap = 1u16;
    let right_w = countdown_width as u16;
    let left_w = area.width.saturating_sub(right_w);
    let left_area = Rect {
        width: left_w.saturating_sub(gap),
        ..area
    };
    let right_area = Rect {
        x: area.x + left_w,
        width: right_w,
        ..area
    };

    if !spans.is_empty() {
        let avail = left_area.width as usize;
        let info_line = Line::from(overscroll_truncate_spans(spans, avail));
        let info_line = if app.centered_mode() {
            info_line.alignment(Alignment::Center)
        } else {
            info_line
        };
        frame.render_widget(Paragraph::new(info_line), left_area);
    }

    let countdown_line = Line::from(vec![countdown]).alignment(Alignment::Right);
    frame.render_widget(Paragraph::new(countdown_line), right_area);
}

/// Truncate a list of spans to at most `max_width` display columns, appending a
/// single-cell ellipsis when content is dropped. Preserves per-span styling.
fn overscroll_truncate_spans(spans: Vec<Span<'static>>, max_width: usize) -> Vec<Span<'static>> {
    use unicode_width::UnicodeWidthStr;
    if max_width == 0 {
        return Vec::new();
    }
    let total: usize = spans.iter().map(|s| s.content.width()).sum();
    if total <= max_width {
        return spans;
    }
    // Leave room for a trailing ellipsis.
    let budget = max_width.saturating_sub(1);
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for span in spans {
        let w = span.content.width();
        if used + w <= budget {
            used += w;
            out.push(span);
            continue;
        }
        // Partial: take as many chars as fit within the remaining budget.
        let remaining = budget - used;
        if remaining > 0 {
            let mut taken = String::new();
            let mut tw = 0usize;
            for ch in span.content.chars() {
                let cw = UnicodeWidthStr::width(ch.to_string().as_str());
                if tw + cw > remaining {
                    break;
                }
                tw += cw;
                taken.push(ch);
            }
            if !taken.is_empty() {
                out.push(Span::styled(taken, span.style));
            }
        }
        break;
    }
    out.push(Span::styled("…", Style::default().fg(rgb(100, 100, 110))));
    out
}

/// Format a working dir path home-relative (~/foo/bar), keeping the last 2 segments.
fn overscroll_dir_label(path: &str) -> Option<String> {
    session_facts::dir_label_short(path)
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
        // Keep provider labels credential-neutral: the adjacent auth chip
        // (`overscroll_auth_label`) reports OAuth vs API key from the canonical
        // credential resolution. Baking a credential into the provider name
        // used to produce contradictions like "Claude OAuth · API key" when
        // the Anthropic route was pinned to the API key.
        "claude" => "Claude".to_string(),
        "anthropic" => "Anthropic".to_string(),
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
        "max" => "max",
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

const RIGHT_FACT_CONTEXT_CELLS: usize = 6;
const RIGHT_FACT_GAP: u16 = 2;
const RIGHT_FACT_PAD: u16 = 1;
const RIGHT_FACT_TRANSCRIPT_ROWS: u16 = 4;

#[derive(Clone)]
struct RightFactLine {
    spans: Vec<Span<'static>>,
    width: u16,
}

impl RightFactLine {
    fn new(spans: Vec<Span<'static>>) -> Option<Self> {
        use unicode_width::UnicodeWidthStr;
        let width: usize = spans.iter().map(|span| span.content.width()).sum();
        let width = u16::try_from(width).ok()?;
        (width > 0).then_some(Self { spans, width })
    }
}

#[derive(Clone)]
struct RightFactPlacement {
    line: RightFactLine,
    area: Rect,
}

/// Draw session facts as a bottom-anchored stack in otherwise unused cells on
/// the right side of the composer chrome. When chrome rows are occupied, the
/// stack may climb into at most the last few transcript rows, but only where
/// the final rendered buffer has a genuinely blank suffix. It never reflows or
/// overwrites transcript, status, notification, inline UI, or input content.
pub(super) fn draw_right_fact_stack(
    frame: &mut Frame,
    app: &dyn TuiState,
    messages_area: Rect,
    input_area: Rect,
    transcript_scrollbar_visible: bool,
    input_cursor: Option<Position>,
) {
    // The legacy overscroll row owns these same facts while it is visible.
    // Standing down here avoids duplicates and keeps its elastic reveal from
    // changing transcript-tail overlays mid-gesture. Users with overscroll off
    // get the compact stack continuously.
    if app.chat_overscroll_active() || input_area.width == 0 || input_area.height == 0 {
        return;
    }

    let lines = right_fact_lines(app);
    if lines.is_empty() {
        return;
    }

    // Never composite into a live transcript while a turn is processing. Its
    // tail changes every frame, so even collision-safe facts could appear to
    // jump as streaming rows arrive. Chrome rows remain available.
    let transcript_rows = if app.auto_scroll_paused() || app.is_processing() {
        0
    } else {
        RIGHT_FACT_TRANSCRIPT_ROWS.min(messages_area.height)
    };
    let top = messages_area.bottom().saturating_sub(transcript_rows);
    let bottom = input_area.bottom();
    if top >= bottom {
        return;
    }

    let placements = {
        let buffer = frame.buffer_mut();
        right_fact_placements(
            buffer,
            lines,
            top,
            bottom,
            input_area.x,
            input_area.right(),
            messages_area,
            transcript_scrollbar_visible,
            input_cursor,
        )
    };

    for placement in placements {
        frame.render_widget(
            Paragraph::new(Line::from(placement.line.spans)),
            placement.area,
        );
    }
}

/// Build fact rows in their visual top-to-bottom order: provider/auth, model,
/// directory, then context usage. Placement walks this list in reverse so the
/// context row is anchored nearest the input whenever space permits.
fn right_fact_lines(app: &dyn TuiState) -> Vec<RightFactLine> {
    let data = app.info_widget_data();
    let sep = || Span::styled(" · ", Style::default().fg(rgb(100, 100, 110)));
    let mut lines = Vec::with_capacity(4);

    let mut access = Vec::new();
    let provider = data
        .provider_name
        .clone()
        .filter(|provider| !provider.trim().is_empty())
        .unwrap_or_else(|| app.provider_name());
    if !provider.is_empty() && !overscroll_is_runtime_placeholder(&provider) {
        access.push(Span::styled(
            overscroll_provider_display(&provider),
            Style::default().fg(rgb(140, 180, 255)),
        ));
    }
    if let Some((label, color)) = overscroll_auth_label(data.auth_method) {
        if !access.is_empty() {
            access.push(sep());
        }
        access.push(Span::styled(label.to_string(), Style::default().fg(color)));
    }
    if let Some(line) = RightFactLine::new(access) {
        lines.push(line);
    }

    let model = data
        .model
        .clone()
        .filter(|model| !model.trim().is_empty())
        .unwrap_or_else(|| app.provider_model());
    if !model.is_empty() && !overscroll_is_placeholder(&model) {
        let mut spans = vec![Span::styled(
            session_facts::pretty_model(&model),
            Style::default().fg(rgb(255, 150, 200)).bold(),
        )];
        if let Some(effort) = data
            .reasoning_effort
            .as_deref()
            .and_then(overscroll_short_reasoning)
        {
            spans.push(Span::styled(
                format!(" {effort}"),
                Style::default().fg(rgb(140, 140, 150)),
            ));
        }
        if let Some(line) = RightFactLine::new(spans) {
            lines.push(line);
        }
    }

    if let Some(dir) = app
        .working_dir()
        .and_then(|path| overscroll_dir_label(&path))
        && let Some(line) = RightFactLine::new(vec![Span::styled(
            dir,
            Style::default().fg(rgb(140, 140, 150)),
        )])
    {
        lines.push(line);
    }

    if let Some((used, limit)) = overscroll_context_usage(&data) {
        let mut spans = vec![Span::styled(
            format!(
                "{}/{} ",
                overscroll_format_tokens(used),
                overscroll_format_tokens(limit)
            ),
            Style::default().fg(rgb(140, 140, 150)),
        )];
        spans.extend(overscroll_context_bar(
            used,
            limit,
            RIGHT_FACT_CONTEXT_CELLS,
        ));
        if let Some(line) = RightFactLine::new(spans) {
            lines.push(line);
        }
    }

    lines
}

fn right_fact_placements(
    buffer: &ratatui::buffer::Buffer,
    lines: Vec<RightFactLine>,
    top: u16,
    bottom: u16,
    left: u16,
    right: u16,
    messages_area: Rect,
    transcript_scrollbar_visible: bool,
    protected_position: Option<Position>,
) -> Vec<RightFactPlacement> {
    if top >= bottom || left >= right {
        return Vec::new();
    }

    let mut placements = Vec::with_capacity(lines.len());
    let mut next_row = bottom.checked_sub(1);

    for line in lines.into_iter().rev() {
        let Some(mut row) = next_row else {
            break;
        };
        let mut placed = None;

        loop {
            if row < top {
                break;
            }
            let row_right = if transcript_scrollbar_visible
                && row >= messages_area.y
                && row < messages_area.bottom()
            {
                right.saturating_sub(1)
            } else {
                right
            };

            let required = line
                .width
                .saturating_add(RIGHT_FACT_GAP)
                .saturating_add(RIGHT_FACT_PAD);
            if row_right.saturating_sub(left) >= required {
                let fact_right = row_right.saturating_sub(RIGHT_FACT_PAD);
                let fact_left = fact_right.saturating_sub(line.width);
                let probe_left = fact_left.saturating_sub(RIGHT_FACT_GAP);
                if probe_left >= left
                    && (probe_left..row_right).all(|x| {
                        protected_position != Some(Position::new(x, row))
                            && right_fact_cell_is_blank(&buffer[(x, row)])
                    })
                {
                    placed = Some(Rect::new(fact_left, row, line.width, 1));
                    break;
                }
            }

            let Some(previous) = row.checked_sub(1) else {
                break;
            };
            row = previous;
        }

        if let Some(area) = placed {
            next_row = area.y.checked_sub(1);
            placements.push(RightFactPlacement { line, area });
        }
    }

    placements
}

fn right_fact_cell_is_blank(cell: &ratatui::buffer::Cell) -> bool {
    cell.symbol().trim().is_empty()
        && cell.bg == Color::Reset
        && cell.modifier.is_empty()
        && !cell.skip
}

pub(super) fn draw_input(
    frame: &mut Frame,
    app: &dyn TuiState,
    area: Rect,
    next_prompt: usize,
    debug_capture: &mut Option<FrameCaptureBuilder>,
) -> Option<Position> {
    let input_text = app.input();
    let cursor_pos = app.cursor_pos();

    let mode = composer_mode(input_text, app.is_remote_mode());
    // Command suggestions render later as an overlay pass
    // (`draw_command_suggestions_overlay`), never inline: they must not shift
    // the input rows or anything below them.
    let has_suggestions = command_suggestions_active(app, &app.command_suggestions());

    let (prompt_char, caret_color) = input_prompt(app);
    let num_str = format!("{}", next_prompt);
    let prompt_len = input_prompt_len(app, next_prompt);
    let reserved_width = send_mode_reserved_width(app);
    let line_width = (area.width as usize).saturating_sub(prompt_len + reserved_width);

    if line_width == 0 {
        return None;
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
        // Overlay pass owns the palette rows; no inline hint line competes
        // with it.
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
            "  Ctrl/Cmd+Enter to send now"
        } else {
            "  Ctrl/Cmd+Enter to queue"
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
    let visible_input_rows = lines.len().saturating_sub(suggestions_offset);

    let centered = app.centered_mode();

    // Register the composer's visible rows with the shared copy-selection
    // machinery so mouse drags can select and copy the text being typed
    // (issue #430). The prompt prefix / continuation indent is excluded via
    // per-row left margins, so hit-testing and the copied text only ever
    // cover the typed text, never the prompt decoration.
    if visible_input_rows > 0 {
        let (wrapped_plain, raw_plain, line_map) =
            input_copy_snapshot_parts(input_text, line_width);
        let left_margins: Vec<u16> = (0..visible_input_rows)
            .map(|rel| {
                let abs = scroll_offset + rel;
                let text_width = wrapped_plain
                    .get(abs)
                    .map(|text| unicode_width::UnicodeWidthStr::width(text.as_str()))
                    .unwrap_or(0);
                let mut margin = prompt_len;
                if centered {
                    margin += (area.width as usize).saturating_sub(prompt_len + text_width) / 2;
                }
                margin.min(area.width as usize) as u16
            })
            .collect();
        let input_rows_area = Rect::new(
            area.x,
            area.y.saturating_add(suggestions_offset as u16),
            area.width,
            visible_input_rows as u16,
        );
        super::record_input_copy_snapshot(
            wrapped_plain,
            raw_plain,
            line_map,
            scroll_offset,
            scroll_offset + visible_input_rows,
            input_rows_area,
            &left_margins,
        );
    }

    // Highlight an active copy selection over the composer text, mirroring the
    // chat/side-pane selection rendering. Columns are selection-space (typed
    // text only), so shift them right by the prompt width for display.
    if let Some(range) = app.copy_selection_range().filter(|range| {
        range.start.pane == crate::tui::CopySelectionPane::Input
            && range.end.pane == crate::tui::CopySelectionPane::Input
    }) {
        let (start, end) = if (range.start.abs_line, range.start.column)
            <= (range.end.abs_line, range.end.column)
        {
            (range.start, range.end)
        } else {
            (range.end, range.start)
        };
        for rel in 0..visible_input_rows {
            let abs = scroll_offset + rel;
            if abs < start.abs_line || abs > end.abs_line {
                continue;
            }
            if let Some(line) = lines.get_mut(suggestions_offset + rel) {
                let start_col = if abs == start.abs_line {
                    prompt_len + start.column
                } else {
                    prompt_len
                };
                let end_col = if abs == end.abs_line {
                    prompt_len + end.column
                } else {
                    line.width()
                };
                *line = highlight_line_selection(line, start_col, end_col);
            }
        }
    }

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

    let cursor = Position::new(cursor_x, cursor_y);
    frame.set_cursor_position(cursor);
    draw_send_mode_indicator(frame, app, area);
    Some(cursor)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WrappedInputSegment {
    text: String,
    start_char: usize,
    end_char: usize,
    display_width: usize,
}

/// Wrapped/raw text plus the wrapped-to-raw line map for the composer's
/// copy-selection snapshot. Wrapped rows mirror `wrap_input_segments` exactly
/// (the same function that lays out the rendered rows), while raw lines are
/// the logical `\n`-separated input lines, so copying a selection that spans
/// a soft wrap yields the original text without injected line breaks.
fn input_copy_snapshot_parts(
    input: &str,
    line_width: usize,
) -> (Vec<String>, Vec<String>, Vec<super::WrappedLineMap>) {
    use unicode_width::UnicodeWidthChar;

    let segments = wrap_input_segments(input, line_width);
    let raw_lines: Vec<String> = input.split('\n').map(str::to_owned).collect();

    // (raw_line, display_col) at each char boundary of the input.
    let mut boundaries = Vec::with_capacity(input.chars().count() + 1);
    let mut raw_line = 0usize;
    let mut col = 0usize;
    boundaries.push((raw_line, col));
    for ch in input.chars() {
        if ch == '\n' {
            raw_line += 1;
            col = 0;
        } else {
            col += ch.width().unwrap_or(0);
        }
        boundaries.push((raw_line, col));
    }

    let mut wrapped_plain = Vec::with_capacity(segments.len());
    let mut line_map = Vec::with_capacity(segments.len());
    for segment in &segments {
        let (raw_line, start_col) = boundaries
            .get(segment.start_char)
            .copied()
            .unwrap_or((0, 0));
        line_map.push(super::WrappedLineMap {
            raw_line,
            start_col,
            end_col: start_col + segment.display_width,
        });
        wrapped_plain.push(segment.text.clone());
    }
    (wrapped_plain, raw_lines, line_map)
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
        ("", asap_color())
    }
}

fn draw_send_mode_indicator(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let indicator_area = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(1),
        width: area.width,
        height: 1,
    };

    let (icon, color) = send_mode_indicator(app);
    if !icon.is_empty() {
        let line = Line::from(Span::styled(icon, Style::default().fg(color)));
        let paragraph = Paragraph::new(line).alignment(Alignment::Right);
        frame.render_widget(paragraph, indicator_area);
        return;
    }
}

#[derive(Clone, Copy)]
enum QueuedMsgType {
    Pending,
    Interleave,
    Queued,
}
