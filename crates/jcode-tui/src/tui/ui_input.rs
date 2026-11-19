#[path = "ui_input/command_suggestions.rs"]
mod command_suggestions;
use command_suggestions::*;
#[path = "ui_input_status_rendering.rs"]
mod status_rendering;
pub(super) use status_rendering::build_notification_spans;
pub(super) use status_rendering::draw_input;
pub(super) use status_rendering::draw_status;

use super::inline_interactive_ui::format_elapsed;
use super::selection_highlight::highlight_line_selection;
use super::tools_ui::{get_tool_activity_detail, summarize_batch_running_tools_compact};
use super::visual_debug::{self, FrameCaptureBuilder};
use super::{
    ProcessingStatus, TuiState, accent_color, ai_color, asap_color, dim_color, pending_color,
    queued_color, rainbow_prompt_color, user_color,
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
    } else if app::has_safe_slash_command_token(input) {
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

/// Draw the Ctrl+R reverse prompt-history search overlay. Reuses the
/// command-palette positioning: floats below (or above) the input without
/// reserving layout height. Shows the query line plus the match list with the
/// selected row highlighted.
pub(super) fn draw_prompt_history_search_overlay(
    frame: &mut Frame,
    app: &dyn TuiState,
    area: Rect,
) {
    let Some(view) = app.prompt_history_search() else {
        return;
    };
    const VISIBLE_LIMIT: usize = 8;

    let accent = Style::default().fg(rgb(255, 213, 128));
    let dim = Style::default().fg(dim_color());
    let normal = Style::default().fg(rgb(128, 203, 196));

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("(history search) ", dim),
        Span::styled(view.query.clone(), accent),
        Span::styled("█", accent),
        Span::styled(
            "  ↑↓ select · ↵ insert · Esc cancel",
            Style::default().fg(dim_color()),
        ),
    ]));

    if view.matches.is_empty() {
        if view.query.trim().is_empty() {
            lines.push(Line::from(Span::styled("  type to search history", dim)));
        } else {
            lines.push(Line::from(Span::styled("  no matches", dim)));
        }
    } else {
        // Keep the selected row inside the visible window.
        let window_start = view
            .selected
            .saturating_sub(VISIBLE_LIMIT.saturating_sub(1))
            .min(
                view.matches
                    .len()
                    .saturating_sub(VISIBLE_LIMIT.min(view.matches.len())),
            );
        let visible = view
            .matches
            .iter()
            .enumerate()
            .skip(window_start)
            .take(VISIBLE_LIMIT);
        for (index, preview) in visible {
            let is_selected = index == view.selected;
            let marker = if is_selected { "▸ " } else { "  " };
            let style = if is_selected { accent } else { normal };
            let mut spans = vec![
                Span::styled(marker.to_string(), style),
                Span::styled(preview.clone(), style),
            ];
            if index == window_start + VISIBLE_LIMIT - 1
                && view.matches.len() > window_start + VISIBLE_LIMIT
            {
                spans.push(Span::styled(
                    format!(
                        "  +{} more",
                        view.matches.len() - (window_start + VISIBLE_LIMIT)
                    ),
                    dim,
                ));
            }
            lines.push(Line::from(spans));
        }
    }

    let Some(rect) = command_suggestions_overlay_rect(area, lines.len() as u16, frame.area())
    else {
        return;
    };
    lines.truncate(rect.height as usize);
    frame.render_widget(ratatui::widgets::Clear, rect);
    frame.render_widget(Paragraph::new(lines), rect);
}

/// Draw the command-suggestion popover as a late overlay pass.
///
/// Called after the chunked layout (and info widgets) have rendered so the
/// palette floats over existing rows instead of reserving layout height.
pub(super) fn draw_command_suggestions_overlay(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    if let Some(picker) = app.inline_interactive_state()
        && picker.kind == crate::tui::PickerKind::Model
    {
        // Use the command palette surface, not a separate bordered picker.
        // Keep it above the composer and size the window before building rows
        // so the selected model remains visible even on short terminals.
        let height = area.y.saturating_sub(frame.area().y);
        let lines = super::inline_interactive_ui::model_suggestion_lines(picker, height as usize);
        if !lines.is_empty() && area.width > 0 {
            let rect = Rect::new(
                area.x,
                area.y - lines.len() as u16,
                area.width,
                lines.len() as u16,
            );
            frame.render_widget(ratatui::widgets::Clear, rect);
            frame.render_widget(Paragraph::new(lines), rect);
        }
        return;
    }
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

/// Extract the slash-command portion of the typed input that should be matched
/// against suggestion command tokens for highlighting purposes.
fn command_suggestion_needle(input: &str) -> Option<String> {
    let trimmed = input.trim_start();
    if !trimmed.starts_with('/') {
        return None;
    }
    Some(trimmed.to_string())
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
    if app.login_input_hint().is_some() || app.voice_composer_preview().is_some() {
        return 1;
    }
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
    if app.login_input_hint().is_some() {
        return (" login> ", accent_color());
    }
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
    app.compacted_hidden_user_prompts()
        .saturating_add(app.display_user_message_count())
        .saturating_add(1)
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
    let preview = app.voice_composer_preview();
    let (lines, _, _) = wrap_input_text_with_ghost(
        app.input(),
        app.cursor_pos(),
        line_width,
        &num_str,
        prompt_char,
        caret_color,
        prompt_len,
        &|number| app.input_image_attached(number),
        voice_ghost(preview.as_ref()),
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
    app.queued_messages()
        .iter()
        .filter(|msg| !crate::todo::is_auto_poke_message(msg))
        .count()
        + app.queued_commands().len()
        + pending_count
        + if interleave { 1 } else { 0 }
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
    for msg in app.queued_commands() {
        let normalized = normalize_repaint_sensitive_notice_text(msg);
        previews.push(format!(
            "⏳ {}",
            normalized.chars().take(100).collect::<String>()
        ));
    }
    for msg in app
        .queued_messages()
        .iter()
        .filter(|msg| !crate::todo::is_auto_poke_message(msg))
    {
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
    for msg in app.queued_commands() {
        items.push((QueuedMsgType::Queued, msg.as_str()));
    }
    for msg in app
        .queued_messages()
        .iter()
        .filter(|msg| !crate::todo::is_auto_poke_message(msg))
    {
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
        ConnectionPhase::SendingRequest => "sending context".to_string(),
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

/// Shared by the open session status line and its lightweight todo-lane observer.
/// No provider or worker state is inferred here: absent telemetry stays absent.
pub(super) fn live_processing_status_line(
    status: &crate::protocol::SwarmLiveStatus,
    stale_secs: Option<f32>,
    spinner: &'static str,
) -> Line<'static> {
    let single_line = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let elapsed = format_elapsed(status.elapsed_ms as f32 / 1000.0);
    let transport = collect_transport_context_labels(
        status
            .status_detail
            .as_deref()
            .and_then(normalize_status_detail),
        status
            .connection_type
            .as_deref()
            .map(display_connection_type),
        None,
    );
    let tokens = match (status.input_tokens, status.output_tokens) {
        (None, None) => None,
        (input, output) => Some(format!(
            "↑{} ↓{}",
            input
                .map(format_stream_tokens)
                .unwrap_or_else(|| "?".into()),
            output
                .map(format_stream_tokens)
                .unwrap_or_else(|| "?".into())
        )),
    };
    let mut spans = match status.phase.as_str() {
        "running" => {
            let mut spans = vec![
                Span::styled(spinner, Style::default().fg(ai_color())),
                Span::styled(
                    format!(" working… {elapsed}"),
                    Style::default().fg(dim_color()),
                ),
            ];
            if let Some(activity) = &status.tool_intent {
                spans.push(Span::styled(
                    format!(" · {}", single_line(activity)),
                    Style::default().fg(ai_color()),
                ));
            }
            spans
        }
        "running_tool" => running_tool_header_spans(
            spinner,
            &single_line(status.tool_name.as_deref().unwrap_or("tool")),
            status.tool_intent.as_deref().map(single_line).as_deref(),
            ai_color(),
        ),
        "streaming" => {
            let mut label =
                streaming_liveness_label(elapsed.clone(), stale_secs, status.stream_message_ended);
            if let Some(tps) = status.output_tps_milli {
                label.push_str(&format!(" · {:.1} tps", tps as f64 / 1000.0));
            }
            streaming_status_spans(spinner, label, status.stream_message_ended, false, "")
        }
        "sending" | "thinking" | "connecting" => {
            let label = if status.phase == "connecting" {
                match status.connection_phase.as_deref().unwrap_or("connecting") {
                    "authenticating" => "refreshing auth",
                    "sending_request" => "sending context",
                    "waiting_for_response" => "waiting for response",
                    phase => phase,
                }
            } else {
                status.phase.as_str()
            };
            vec![
                Span::styled(spinner, Style::default().fg(ai_color())),
                Span::styled(
                    format!(" {}… {elapsed}", single_line(label)),
                    Style::default().fg(dim_color()),
                ),
            ]
        }
        _ => {
            let reason = status
                .stop_reason
                .as_deref()
                .or(status.status_detail.as_deref())
                .unwrap_or(match status.phase.as_str() {
                    "completed" | "done" => "completed",
                    "failed" | "crashed" => "failed",
                    "stopped" => "stopped",
                    "cancelled" => "cancelled",
                    "waiting" => "waiting",
                    "blocked" => "blocked",
                    "waiting_network" => "network disconnected, waiting to retry",
                    "ready" => "ready",
                    "queued" => "queued",
                    _ => "idle",
                });
            let glyph = match status.phase.as_str() {
                "completed" | "done" => "✓",
                "failed" | "crashed" => "✗",
                "waiting" => "◷",
                "blocked" => "⊘",
                _ => "◼",
            };
            return Line::from(vec![Span::styled(
                format!("{glyph} {} · {elapsed}", single_line(reason)),
                Style::default().fg(rgb(225, 165, 90)),
            )]);
        }
    };
    if let Some(tokens) = tokens {
        spans.push(Span::styled(
            format!(" · {tokens}"),
            Style::default().fg(dim_color()),
        ));
    }
    for label in transport {
        spans.push(Span::styled(
            format!(" · {}", single_line(&label)),
            Style::default().fg(dim_color()),
        ));
    }
    if status.phase == "running_tool" {
        spans.push(Span::styled(
            format!(" · {elapsed}"),
            Style::default().fg(dim_color()),
        ));
    }
    Line::from(spans)
}

fn live_status_snapshot(app: &dyn TuiState, elapsed: f32) -> crate::protocol::SwarmLiveStatus {
    let (input, output) = app.streaming_tokens();
    crate::protocol::SwarmLiveStatus {
        elapsed_ms: (elapsed * 1000.0) as u64,
        input_tokens: Some(input),
        output_tokens: Some(output),
        output_tps_milli: app.output_tps().map(|tps| (tps * 1000.0) as u64),
        connection_type: app.connection_type(),
        status_detail: app.status_detail(),
        stream_message_ended: app.stream_message_ended(),
        ..Default::default()
    }
}

/// Background launches finish their tool call immediately, but the work and
/// its intent must remain visible while the agent moves on to another tool.
fn status_with_background_tasks(
    foreground: Line<'static>,
    tasks: &[crate::tui::BackgroundTaskRow],
    width: usize,
    processing: bool,
) -> Line<'static> {
    use crate::tui::BackgroundTaskRowStatus;
    use jcode_tui_render::truncate_line_with_ellipsis_to_width as fit;

    let running: Vec<_> = tasks
        .iter()
        .filter(|task| task.status == BackgroundTaskRowStatus::Running)
        .collect();
    if running.is_empty() {
        return fit(&foreground, width);
    }
    let labels = running
        .iter()
        .map(|task| task.label.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join(" · ");
    let background = Line::from(Span::styled(
        format!("bg {}: {}", running.len(), labels),
        Style::default().fg(ai_color()),
    ));
    // Idle tips must not displace actual work. While processing, reserve space
    // for both activities rather than letting a long foreground intent hide bg.
    if !processing || foreground.width() == 0 || width < 24 {
        return fit(&background, width);
    }
    let background_width = background.width().min(width / 2);
    let mut line = fit(&foreground, width.saturating_sub(background_width + 3));
    line.spans
        .push(Span::styled(" · ", Style::default().fg(dim_color())));
    line.spans.extend(fit(&background, background_width).spans);
    line
}

fn running_tool_header_spans(
    spinner: &'static str,
    name: &str,
    detail: Option<&str>,
    anim_color: Color,
) -> Vec<Span<'static>> {
    let mut spans = vec![
        Span::styled(spinner, Style::default().fg(anim_color)),
        Span::styled(
            format!(" running {}", name),
            Style::default().fg(dim_color()),
        ),
    ];
    if let Some(detail) = detail {
        spans.push(Span::styled(
            format!(" · {}", detail),
            Style::default().fg(anim_color).bold(),
        ));
    }
    spans
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
#[path = "ui_input/status_tests.rs"]
mod tests;

pub(crate) fn openai_reset_status_hint(
    auth_method: super::info_widget::AuthMethod,
    usage: &crate::usage::OpenAIUsageData,
    account_label: Option<&str>,
) -> Option<String> {
    if auth_method != super::info_widget::AuthMethod::OpenAIOAuth
        || !usage.banked_reset_available_for_account(account_label)
    {
        return None;
    }
    let credits = usage.openai_reset_credits.as_ref()?;
    let count = credits.available_count;
    let noun = if count == 1 { "reset" } else { "resets" };
    let mut expiries = Vec::new();
    for expiry in credits
        .available_expirations
        .iter()
        .take(count.try_into().unwrap_or(usize::MAX))
    {
        if let Some(expiry) = expiry
            .as_deref()
            .and_then(|expiry| chrono::DateTime::parse_from_rfc3339(expiry).ok())
        {
            expiries.push(expiry);
        }
    }
    let detail = expiries.into_iter().min().map_or_else(
        || "expiry unknown".to_string(),
        |expiry| {
            format!(
                "expires in {}",
                crate::usage::format_reset_time(&expiry.to_rfc3339())
            )
        },
    );
    Some(format!("{count} {noun} available · {detail} · /reset"))
}

fn caption_line(row: &crate::tui::JarvisCaption, width: u16) -> Line<'static> {
    let (prefix, color) = match row.score {
        None => ("Heard: ".to_owned(), Color::DarkGray),
        Some((p, accepted)) => {
            let label = if accepted { "Trigger" } else { "Heard" };
            let color = if accepted {
                Color::Green
            } else if p >= 0.5 {
                Color::Yellow
            } else {
                Color::DarkGray
            };
            (format!("{:.0}% {label}: ", p * 100.0), color)
        }
    };
    Line::styled(
        truncate_cells(&format!("{prefix}{}", row.text), width as usize),
        Style::default().fg(color),
    )
}

#[cfg(test)]
#[test]
fn caption_cells_show_score_and_do_not_wrap() {
    use ratatui::{Terminal, backend::TestBackend};
    let mut terminal = Terminal::new(TestBackend::new(18, 2)).unwrap();
    for (score, expected, prefix) in [
        (None, Color::DarkGray, "Heard:"),
        (Some((0.12, false)), Color::DarkGray, "12% Heard:"),
        (Some((0.55, false)), Color::Yellow, "55% Heard:"),
        (Some((0.94, false)), Color::Yellow, "94% Heard:"),
        (Some((0.95, true)), Color::Green, "95% Trigger:"),
    ] {
        let row = crate::tui::JarvisCaption {
            id: 1,
            text: "界".repeat(40),
            score,
        };
        terminal
            .draw(|f| f.render_widget(Paragraph::new(caption_line(&row, 18)), f.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        for (x, c) in prefix.chars().enumerate() {
            assert_eq!(buffer[(x as u16, 0)].symbol(), c.to_string());
            assert_eq!(buffer[(x as u16, 0)].fg, expected);
        }
        assert!((0..18).all(|x| buffer[(x, 1)].symbol() == " "));
    }
}

fn notification_lines(app: &dyn TuiState, width: u16) -> Vec<Line<'static>> {
    let spans = build_notification_spans(app);
    if width == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    for row in app.jarvis_transcript().into_iter().flatten() {
        lines.push(caption_line(row, width));
    }
    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }
    lines
        .into_iter()
        .map(|line| {
            if app.centered_mode() {
                line.alignment(Alignment::Center)
            } else {
                line
            }
        })
        .collect()
}

pub(super) fn notification_height(app: &dyn TuiState, width: u16) -> u16 {
    if app.openai_reset_hint().is_some() {
        notification_lines(app, width)
            .len()
            .try_into()
            .unwrap_or(u16::MAX)
    } else {
        u16::from(app.has_notification())
            + app.jarvis_transcript().map_or(0, |rows| rows.len() as u16)
    }
}

pub(super) fn draw_notification(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    frame.render_widget(Paragraph::new(notification_lines(app, area.width)), area);
}

/// Draw the session status line, pinned directly below the input. Shows the
/// directory, branch, git status, context usage, access method, provider, and
/// model, compacting facts step by step as the terminal narrows.
pub(super) fn draw_overscroll_status(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let data = app.info_widget_data();

    let model = data
        .model
        .clone()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| app.provider_model());
    let provider = data
        .provider_name
        .clone()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| app.provider_name());
    let facts = OverscrollFacts {
        machine: app.remote_machine(),
        dir: overscroll_directory_transition(
            data.original_working_dir.as_deref(),
            app.working_dir().as_deref(),
        ),
        branch: overscroll_git_branch(&data),
        git: overscroll_git_info(&data).cloned(),
        context: overscroll_context_usage(&data),
        account_remaining: overscroll_account_remaining(&data),
        auth: overscroll_auth_label(data.auth_method),
        provider: (!provider.is_empty() && !overscroll_is_runtime_placeholder(&provider))
            .then(|| overscroll_provider_display(&provider)),
        model: (!model.is_empty() && !overscroll_is_placeholder(&model))
            .then(|| session_facts::pretty_model(&model)),
        effort: data
            .reasoning_effort
            .as_deref()
            .and_then(overscroll_short_reasoning)
            .map(str::to_string),
    };

    let alignment = if app.centered_mode() {
        Alignment::Center
    } else {
        Alignment::Right
    };
    let (spans, _) = overscroll_fit_facts(&facts, area.width as usize);
    if spans.is_empty() {
        return;
    }
    let line = Line::from(spans).alignment(alignment);
    frame.render_widget(Paragraph::new(line), area);
}

/// Render only the supplied session facts. Unlike the live adapter this never
/// falls back to the current App, process directory, SSH host or credentials.
#[cfg(test)]
pub(crate) fn preview_overscroll_line(
    data: &crate::tui::info_widget::InfoWidgetData,
    width: u16,
) -> Line<'static> {
    preview_overscroll_line_for_host(data, width, None)
}

/// The host belongs to the selected preview, never the attached live session.
#[cfg(test)]
pub(crate) fn preview_overscroll_line_for_host(
    data: &crate::tui::info_widget::InfoWidgetData,
    width: u16,
    host: Option<&str>,
) -> Line<'static> {
    let facts = OverscrollFacts {
        machine: host
            .filter(|host| !host.trim().is_empty())
            .map(str::to_owned),
        dir: overscroll_directory_transition(
            data.original_working_dir.as_deref(),
            data.working_dir.as_deref(),
        ),
        branch: overscroll_git_branch(data),
        git: overscroll_git_info(data).cloned(),
        context: overscroll_context_usage(data),
        account_remaining: overscroll_account_remaining(data),
        auth: overscroll_auth_label(data.auth_method),
        provider: data
            .provider_name
            .as_ref()
            .filter(|name| !name.is_empty() && !overscroll_is_runtime_placeholder(name))
            .map(|name| overscroll_provider_display(name)),
        model: data
            .model
            .as_ref()
            .filter(|name| !name.is_empty() && !overscroll_is_placeholder(name))
            .map(|name| session_facts::pretty_model(name)),
        effort: data
            .reasoning_effort
            .as_deref()
            .and_then(overscroll_short_reasoning)
            .map(str::to_string),
    };
    let label = if data.historical_context {
        "saved · "
    } else {
        ""
    };
    let label_width = unicode_width::UnicodeWidthStr::width(label);
    let mut spans = overscroll_fit_facts(&facts, (width as usize).saturating_sub(label_width)).0;
    if !label.is_empty() && width as usize > label_width {
        spans.insert(0, Span::styled(label, Style::default().fg(dim_color())));
    }
    Line::from(spans).alignment(Alignment::Right)
}

/// Model name color on the overscroll status line.
const OVERSCROLL_MODEL_PINK: Color = Color::Rgb(255, 135, 200);

/// Raw facts for the overscroll status line, before width fitting.
struct OverscrollFacts {
    /// Remote machine this session runs on (cloud VM or SSH host).
    machine: Option<String>,
    dir: Option<String>,
    branch: Option<String>,
    git: Option<crate::tui::info_widget::GitInfo>,
    context: Option<(usize, usize)>,
    account_remaining: Option<u8>,
    auth: Option<(&'static str, Color)>,
    provider: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

/// Per-fact detail level. 0 is the fullest form. Higher levels are more
/// compact, and the last level of each optional fact hides it.
#[derive(Clone, Copy, Default)]
struct OverscrollLevels {
    machine: u8,
    dir: u8,
    branch: u8,
    git: u8,
    context: u8,
    account_bar: u8,
    context_bar: u8,
    auth: u8,
    provider: u8,
    model: u8,
}

/// Compaction ladder, applied one step at a time until the line fits.
///
/// Priority (most to least important): directory, model, context usage,
/// git branch, git status, provider, auth. Low-value facts are shortened or
/// hidden first. The directory, model, and context usage are never hidden,
/// only shortened, so the line always answers "where am I, what am I running,
/// how full is the context".
const OVERSCROLL_LADDER: &[fn(&mut OverscrollLevels)] = &[
    |l| l.account_bar = 1, // drop account bar before shortening other facts
    |l| l.context_bar = 1, // keep token counts when the context bar cannot fit
    |l| l.auth = 1,        // hide auth ("OAuth"/"API key")
    |l| l.git = 1,         // "~3 +1 ?2 ↑1" -> "±6 ↑1"
    |l| l.branch = 1,      // long branch -> 12 chars
    |l| l.provider = 1,    // hide provider
    |l| l.context = 1,     // "74k/256k 29%" -> "29%"
    |l| l.git = 2,         // hide git status
    |l| l.branch = 2,      // hide branch
    |l| l.model = 1,       // drop reasoning effort
    |l| l.machine = 1,     // "☁ jcode-cloud:" -> "☁ "
    |l| l.dir = 1,         // "~/…/jcode" -> "jcode"
];

fn overscroll_fact_spans(
    facts: &OverscrollFacts,
    levels: OverscrollLevels,
) -> Vec<Vec<Span<'static>>> {
    let muted = Style::default().fg(rgb(140, 140, 150));
    let mut out: Vec<Vec<Span<'static>>> = Vec::new();

    let machine_spans = |levels: OverscrollLevels| -> Vec<Span<'static>> {
        let Some(machine) = &facts.machine else {
            return Vec::new();
        };
        let cloud = Style::default().fg(rgb(170, 200, 255));
        let mut spans = vec![Span::styled("☁ ", cloud)];
        if levels.machine == 0 {
            spans.push(Span::styled(
                overscroll_truncate_label(machine, 24),
                cloud.bold(),
            ));
            if facts.dir.is_some() {
                spans.push(Span::styled(":", muted));
            }
        }
        spans
    };

    if let Some(dir) = &facts.dir {
        let label = if levels.dir == 0 {
            dir.clone()
        } else {
            dir.split(" → ")
                .map(|path| path.rsplit('/').find(|seg| !seg.is_empty()).unwrap_or(path))
                .collect::<Vec<_>>()
                .join(" → ")
        };
        let mut group = machine_spans(levels);
        if group.is_empty() {
            group.push(Span::styled(" ", Style::default().fg(rgb(140, 180, 255))));
        }
        group.push(Span::styled(label, muted));
        out.push(group);
    } else if facts.machine.is_some() {
        out.push(machine_spans(levels));
    }

    if let Some(branch) = &facts.branch {
        let label = match levels.branch {
            0 => Some(branch.clone()),
            1 => Some(overscroll_truncate_label(branch, 12)),
            _ => None,
        };
        if let Some(label) = label {
            out.push(vec![Span::styled(
                format!(" {label}"),
                Style::default().fg(rgb(150, 170, 140)),
            )]);
        }
    }

    if let Some(git) = &facts.git
        && let Some(spans) = overscroll_git_status_spans(git, levels.git)
    {
        out.push(spans);
    }

    if let Some((used, limit)) = facts.context {
        let mut spans = Vec::new();
        if levels.context == 0 {
            spans.push(Span::styled(
                format!(
                    "{}/{} ",
                    overscroll_format_tokens(used),
                    overscroll_format_tokens(limit)
                ),
                muted,
            ));
        }
        if levels.context == 0 && levels.context_bar == 0 {
            let pct = ((used as f64 / limit.max(1) as f64).clamp(0.0, 1.0) * 100.0).round() as u8;
            spans.extend(crate::tui::info_widget::render_inline_limit_bar(
                100 - pct,
                4,
                true,
            ));
        } else {
            spans.push(overscroll_context_percentage(used, limit));
        }
        out.push(spans);
    }

    if levels.auth == 0
        && let Some((label, color)) = facts.auth
    {
        out.push(vec![Span::styled(
            label.to_string(),
            Style::default().fg(color),
        )]);
    }

    if levels.provider == 0
        && let Some(provider) = &facts.provider
    {
        let mut group = vec![Span::styled(provider.clone(), muted)];
        if let Some(remaining) = facts.account_remaining {
            let cells = if levels.account_bar == 0 { 4 } else { 0 };
            if cells > 0 {
                group.push(Span::raw(" "));
            }
            group.extend(crate::tui::info_widget::render_inline_limit_bar(
                remaining, cells, false,
            ));
        }
        out.push(group);
    }

    if let Some(model) = &facts.model {
        let mut group = vec![Span::styled(
            model.clone(),
            Style::default().fg(OVERSCROLL_MODEL_PINK).bold(),
        )];
        if levels.model == 0
            && let Some(effort) = &facts.effort
        {
            group.push(Span::styled(format!(" {effort}"), muted));
        }
        out.push(group);
    }

    out
}

/// Fit the facts into `max_width` by walking the compaction ladder. Returns
/// the spans and whether they fit without last-resort character truncation.
fn overscroll_fit_facts(facts: &OverscrollFacts, max_width: usize) -> (Vec<Span<'static>>, bool) {
    use unicode_width::UnicodeWidthStr;
    let render = |levels: OverscrollLevels| {
        let mut out: Vec<Span<'static>> = Vec::new();
        for group in overscroll_fact_spans(facts, levels) {
            if !out.is_empty() {
                out.push(Span::raw(" "));
            }
            out.extend(group);
        }
        out
    };
    let width = |spans: &[Span<'static>]| spans.iter().map(|s| s.content.width()).sum::<usize>();

    let mut levels = OverscrollLevels::default();
    let mut spans = render(levels);
    for step in OVERSCROLL_LADDER {
        if width(&spans) <= max_width {
            return (spans, true);
        }
        step(&mut levels);
        spans = render(levels);
    }
    if width(&spans) <= max_width {
        return (spans, true);
    }
    (overscroll_truncate_spans(spans, max_width), false)
}

fn overscroll_truncate_label(label: &str, max_chars: usize) -> String {
    if label.chars().count() <= max_chars {
        return label.to_string();
    }
    let mut out: String = label.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
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
/// Git facts for the status line. Cloud sessions keep their repository in
/// `dual_git_info.cloud`, so prefer that over the (absent) local `git_info`.
fn overscroll_git_info(
    data: &crate::tui::info_widget::InfoWidgetData,
) -> Option<&crate::tui::info_widget::GitInfo> {
    if let Some(dual) = &data.dual_git_info
        && let crate::tui::cloud_git::RepoState::Ready(info) = &dual.cloud
    {
        return Some(info);
    }
    data.git_info.as_ref()
}

/// Compact git branch label for the status line and fact stack. Truncated so
/// long branch names cannot crowd out the other facts.
fn overscroll_git_branch(data: &crate::tui::info_widget::InfoWidgetData) -> Option<String> {
    let branch = overscroll_git_info(data)?.branch.trim();
    if branch.is_empty() {
        return None;
    }
    let mut label: String = branch.chars().take(24).collect();
    if branch.chars().count() > 24 {
        label.push('…');
    }
    Some(label)
}

/// Git status counts using the git widget palette. Level 0 is the full
/// `~modified +staged ?untracked ↑ahead ↓behind` form, level 1 collapses the
/// local changes into a single `±N` count, and level 2 hides it. `None` when
/// the tree is clean and in sync.
fn overscroll_git_status_spans(
    info: &crate::tui::info_widget::GitInfo,
    level: u8,
) -> Option<Vec<Span<'static>>> {
    let parts: Vec<(usize, &str, Color)> = match level {
        0 => vec![
            (info.modified, "~", rgb(240, 200, 80)),
            (info.staged, "+", rgb(100, 200, 100)),
            (info.untracked, "?", rgb(140, 140, 150)),
            (info.ahead, "↑", rgb(100, 200, 100)),
            (info.behind, "↓", rgb(255, 140, 100)),
        ],
        1 => vec![
            (
                info.modified + info.staged + info.untracked,
                "±",
                rgb(240, 200, 80),
            ),
            (info.ahead, "↑", rgb(100, 200, 100)),
            (info.behind, "↓", rgb(255, 140, 100)),
        ],
        _ => return None,
    };
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (count, symbol, color) in parts {
        if count == 0 {
            continue;
        }
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            format!("{symbol}{count}"),
            Style::default().fg(color),
        ));
    }
    (!spans.is_empty()).then_some(spans)
}

fn overscroll_dir_label(path: &str) -> Option<String> {
    session_facts::dir_label_short(path)
}

fn overscroll_directory_transition(
    original: Option<&str>,
    current: Option<&str>,
) -> Option<String> {
    let current = current.filter(|path| !path.trim().is_empty())?;
    let label = overscroll_dir_label(current).unwrap_or_else(|| current.to_string());
    let Some(original) = original.filter(|path| !path.trim().is_empty()) else {
        return Some(label);
    };
    if std::path::Path::new(original) == std::path::Path::new(current) {
        return Some(label);
    }
    let origin_label = overscroll_dir_label(original).unwrap_or_else(|| original.to_string());
    // Elision can hide the differing ancestor. Keep both paths distinguishable.
    if origin_label == label {
        Some(format!("{original} → {current}"))
    } else {
        Some(format!("{origin_label} → {label}"))
    }
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
        "swarm" => crate::tui::app::effort_display_label(effort),
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

/// The tightest subscription window determines the remaining account allowance.
/// Never turn missing usage or an API-key cost total into a fictional quota.
fn overscroll_account_remaining(data: &crate::tui::info_widget::InfoWidgetData) -> Option<u8> {
    use crate::tui::info_widget::{AuthMethod, UsageProvider};
    let info = data.usage_info.as_ref()?;
    if !matches!(
        (data.auth_method, info.provider),
        (AuthMethod::OpenAIOAuth, UsageProvider::OpenAI)
            | (AuthMethod::AnthropicOAuth, UsageProvider::Anthropic)
    ) {
        return None;
    }
    if info.limit_reached {
        return Some(0);
    }
    if !info.available
        || !info.five_hour.is_finite()
        || !info.seven_day.is_finite()
        || info.spark.is_some_and(|value| !value.is_finite())
    {
        return None;
    }
    Some(100u8.saturating_sub(info.max_usage_pct()))
}

/// Format context counts compactly while retaining useful precision.
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

/// Render the colored percentage alongside the context token counts.
fn overscroll_context_percentage(used: usize, limit: usize) -> Span<'static> {
    let limit = limit.max(1);
    let ratio = (used as f64 / limit as f64).clamp(0.0, 1.0);
    let pct = (ratio * 100.0).round() as u16;

    // Match the info widget usage bar palette (based on remaining context).
    let left_pct = 100u16.saturating_sub(pct);
    let fill_color = if left_pct <= 20 {
        rgb(255, 100, 100)
    } else if left_pct <= 50 {
        rgb(255, 200, 100)
    } else {
        rgb(100, 200, 100)
    };
    Span::styled(format!("{}%", pct), Style::default().fg(fill_color).bold())
}

/// The idle resume composer uses the live input wrapper, but never reads the
/// currently open session's draft, queue, selection, or cursor state.
#[cfg(test)]
pub(crate) fn draw_preview_input(frame: &mut Frame, area: Rect, next_prompt: usize) {
    let number = next_prompt.to_string();
    let prompt = "> ";
    let prompt_len = number.len() + prompt.len();
    let line_width = (area.width as usize).saturating_sub(prompt_len);
    if area.height == 0 || line_width == 0 {
        return;
    }
    let (lines, _, _) = wrap_input_text_with_images(
        "",
        0,
        line_width,
        &number,
        prompt,
        user_color(),
        prompt_len,
        &|_| false,
    );
    frame.render_widget(Paragraph::new(lines), area);
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

#[cfg(test)]
pub(crate) fn wrap_input_text<'a>(
    input: &str,
    cursor_pos: usize,
    line_width: usize,
    num_str: &str,
    prompt_char: &'a str,
    caret_color: Color,
    prompt_len: usize,
) -> (Vec<Line<'a>>, usize, usize) {
    wrap_input_text_with_images(
        input,
        cursor_pos,
        line_width,
        num_str,
        prompt_char,
        caret_color,
        prompt_len,
        &|_| false,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn wrap_input_text_with_images<'a>(
    input: &str,
    cursor_pos: usize,
    line_width: usize,
    num_str: &str,
    prompt_char: &'a str,
    caret_color: Color,
    prompt_len: usize,
    attached: &dyn Fn(usize) -> bool,
) -> (Vec<Line<'a>>, usize, usize) {
    wrap_input_text_with_ghost(
        input,
        cursor_pos,
        line_width,
        num_str,
        prompt_char,
        caret_color,
        prompt_len,
        attached,
        None,
    )
}

/// Like [`wrap_input_text_with_images`], with an optional live voice
/// transcript appended as ghost text. The ghost wraps with the input but is
/// never editable, so the cursor stays at its real position.
#[allow(clippy::too_many_arguments)]
fn wrap_input_text_with_ghost<'a>(
    input: &str,
    cursor_pos: usize,
    line_width: usize,
    num_str: &str,
    prompt_char: &'a str,
    caret_color: Color,
    prompt_len: usize,
    attached: &dyn Fn(usize) -> bool,
    ghost: Option<(&str, Style)>,
) -> (Vec<Line<'a>>, usize, usize) {
    let ghost_start = input.chars().count();
    let combined;
    let (input, ghost_style) = match ghost.filter(|(text, _)| !text.is_empty()) {
        Some((text, style)) => {
            let sep = if input.is_empty() || input.ends_with(char::is_whitespace) {
                ""
            } else {
                " "
            };
            combined = format!("{input}{sep}{text}");
            (combined.as_str(), Some(style))
        }
        None => (input, None),
    };
    let image_ranges: Vec<(std::ops::Range<usize>, bool)> = input
        .match_indices("[image ")
        .filter_map(|(start, _)| {
            let suffix = &input[start + 7..];
            let digits = suffix.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 || !suffix[digits..].starts_with(']') {
                return None;
            }
            let char_start = input[..start].chars().count();
            let present = suffix[..digits].parse::<usize>().is_ok_and(attached);
            Some((char_start..char_start + 8 + digits, present))
        })
        .collect();
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

        let mut spans = if idx == 0 {
            let num_color = rainbow_prompt_color(0);
            vec![
                Span::styled(num_str.to_string(), Style::default().fg(num_color)),
                Span::styled(prompt_char.to_string(), Style::default().fg(caret_color)),
            ]
        } else {
            vec![Span::raw(" ".repeat(prompt_len))]
        };
        // Apply styling after wrapping, preserving cursor and click geometry
        // even when an attachment tag crosses a terminal row boundary.
        let mut text = String::new();
        let mut previous_style = Style::default();
        for (offset, ch) in segment.text.chars().enumerate() {
            let position = segment.start_char + offset;
            let style = if let Some(ghost_style) = ghost_style.filter(|_| position >= ghost_start) {
                ghost_style
            } else if let Some((_, present)) = image_ranges
                .iter()
                .find(|(range, _)| range.contains(&position))
            {
                let color = if *present {
                    rgb(102, 204, 221)
                } else {
                    rgb(255, 193, 7)
                };
                Style::default().fg(color).bold()
            } else {
                Style::default()
            };
            if style != previous_style && !text.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut text), previous_style));
            }
            previous_style = style;
            text.push(ch);
        }
        if !text.is_empty() {
            spans.push(Span::styled(text, previous_style));
        }
        lines.push(Line::from(spans));
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

/// Ghost text for the live voice transcript: italic and dim while
/// recording, accent colored while the final pass runs.
#[path = "ui_input/voice_preview.rs"]
mod voice_preview;
use voice_preview::{truncate_cells, voice_ghost, voice_waveform_line};

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
    }
}

#[derive(Clone, Copy)]
enum QueuedMsgType {
    Pending,
    Interleave,
    Queued,
}

/// The usable text width of one composer row, i.e. the width used by
/// `wrap_input_segments` when the composer is rendered into `area_width`.
/// Returns `None` when there is no room for text.
pub(crate) fn composer_line_width(
    app: &dyn TuiState,
    area_width: u16,
    next_prompt: usize,
) -> Option<usize> {
    let prompt_len = input_prompt_len(app, next_prompt);
    let reserved = send_mode_reserved_width(app);
    let width = (area_width as usize).saturating_sub(prompt_len + reserved);
    (width > 0).then_some(width)
}

/// Move the cursor one *visual* (wrapped) row up (`delta = -1`) or down
/// (`delta = 1`) within the composer, keeping the display column.
///
/// Returns the new byte offset, or `None` when the cursor is already on the
/// first/last visual row (so callers can fall through to prompt history).
pub(crate) fn visual_line_move(
    input: &str,
    cursor_pos: usize,
    line_width: usize,
    delta: isize,
) -> Option<usize> {
    use unicode_width::UnicodeWidthChar;

    if line_width == 0 {
        return None;
    }
    let cursor_char_pos = crate::tui::core::byte_offset_to_char_index(input, cursor_pos);
    let segments = wrap_input_segments(input, line_width);
    if segments.len() < 2 {
        return None;
    }
    let current = segments
        .iter()
        .position(|s| cursor_char_pos >= s.start_char && cursor_char_pos <= s.end_char)?;
    let target_idx = current.checked_add_signed(delta)?;
    let target = segments.get(target_idx)?;
    let col = cursor_col_for_segment(&segments[current], cursor_char_pos);

    // Walk the target row to the same display column.
    let mut display = 0usize;
    let mut chars_in = 0usize;
    for ch in target.text.chars() {
        if display >= col {
            break;
        }
        display += ch.width().unwrap_or(0);
        chars_in += 1;
    }
    Some(crate::tui::core::char_index_to_byte_offset(
        input,
        target.start_char + chars_in,
    ))
}
