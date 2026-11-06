use super::*;
use unicode_width::UnicodeWidthStr;

const PHASE_WIDTH: usize = 12;
const ELAPSED_WIDTH: usize = 8;

fn status_single_line(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() || c.is_whitespace()
                || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            { ' ' } else { c }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn status_cell(text: &str, width: usize) -> String {
    let line = jcode_tui_render::truncate_line_with_ellipsis_to_width(
        &Line::from(status_single_line(text)),
        width,
    );
    let mut result = line
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect::<String>();
    result.push_str(&" ".repeat(width.saturating_sub(UnicodeWidthStr::width(result.as_str()))));
    result
}

pub(super) fn stable_status_elapsed(elapsed_ms: u64) -> String {
    let seconds = elapsed_ms / 1000;
    let hours = seconds / 3600;
    let value = if hours == 0 {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    } else if hours < 100 {
        format!("{hours}:{:02}:{:02}", seconds / 60 % 60, seconds % 60)
    } else if hours <= 999 {
        format!("{hours}h{:02}m", seconds / 60 % 60)
    } else if hours <= 999_999 {
        format!("{hours}h")
    } else {
        ">999999h".into()
    };
    status_cell(&value, ELAPSED_WIDTH)
}

/// Values never resize a telemetry slot, including when telemetry is absent.
pub(super) fn stable_status_telemetry(status: &crate::protocol::SwarmLiveStatus) -> [String; 3] {
    let token = |value: Option<u64>| {
        value
            .map(format_stream_tokens)
            .unwrap_or_else(|| "-".into())
    };
    [
        format!("↑{}", status_cell(&token(status.input_tokens), 6)),
        format!("↓{}", status_cell(&token(status.output_tokens), 6)),
        status_cell(
            &status
                .output_tps_milli
                .map(|v| format!("{:.1}/s", v as f64 / 1000.0))
                .unwrap_or_else(|| "-/s".into()),
            8,
        ),
    ]
}

#[derive(Default)]
struct StatusBadges {
    queued: usize,
    background: usize,
}

impl StatusBadges {
    fn text(&self, width: usize) -> String {
        let count = |n: usize| if n > 99 { "99+".into() } else { n.to_string() };
        let queued = if self.queued > 0 {
            if width >= 22 {
                format!("[+{} queued]", count(self.queued))
            } else {
                format!("q:{}", count(self.queued))
            }
        } else {
            String::new()
        };
        let background = if self.background > 0 {
            if width >= 22 {
                format!("[bg {}]", count(self.background))
            } else {
                format!("bg:{}", count(self.background))
            }
        } else {
            String::new()
        };
        let (queue_width, background_width) = match width {
            22.. => (13, 8),
            12.. => (5, 6),
            _ => (3, 4),
        };
        format!(
            "{} {}",
            status_cell(&queued, queue_width),
            status_cell(&background, background_width)
        )
    }
}

#[cfg(test)]
pub(super) fn stable_status_line(
    glyph: &str,
    phase: &str,
    elapsed_ms: u64,
    activity: &str,
    telemetry: &[String],
    width: usize,
) -> Line<'static> {
    compose_status(
        glyph,
        phase,
        elapsed_ms,
        activity,
        telemetry,
        &StatusBadges::default(),
        width,
    )
}

fn compose_status(
    glyph: &str,
    phase: &str,
    elapsed_ms: u64,
    activity: &str,
    telemetry: &[String],
    badges: &StatusBadges,
    width: usize,
) -> Line<'static> {
    let clock = if phase == "idle" {
        " ".repeat(ELAPSED_WIDTH)
    } else {
        stable_status_elapsed(elapsed_ms)
    };
    let prefix = format!(" {} {} │ ", status_cell(phase, PHASE_WIDTH), clock);
    let prefix_width = 1 + UnicodeWidthStr::width(prefix.as_str());
    // These reservations depend only on terminal width, never on label/count length.
    let badge_width = match width {
        100.. => 22,
        64.. => 12,
        40.. => 8,
        _ => 0,
    };
    let mut count = telemetry.len();
    while count > 0
        && prefix_width
            + 8
            + badge_width
            + 3
            + telemetry[..count]
                .iter()
                .map(|s| UnicodeWidthStr::width(s.as_str()) + 1)
                .sum::<usize>()
            > width
    {
        count -= 1;
    }
    let right = telemetry[..count].join(" ");
    let right_width = UnicodeWidthStr::width(right.as_str()) + if count > 0 { 3 } else { 0 };
    let activity_width = width.saturating_sub(prefix_width + badge_width + right_width);
    let warning = matches!(
        phase,
        "building" | "rate limited" | "offline" | "blocked" | "failed" | "crashed"
    );
    let warning_color = rgb(255, 193, 7);
    let activity_warning = warning
        || activity.contains('⚠')
        || activity.starts_with("stalled")
        || activity.starts_with("no tokens");
    let mut spans = vec![
        Span::styled(
            status_cell(glyph, 1),
            Style::default().fg(if warning { warning_color } else { ai_color() }),
        ),
        Span::styled(
            prefix,
            Style::default().fg(if warning { warning_color } else { dim_color() }),
        ),
        Span::styled(
            status_cell(activity, activity_width),
            Style::default().fg(if activity_warning {
                warning_color
            } else {
                ai_color()
            }),
        ),
    ];
    if badge_width > 0 {
        spans.push(Span::styled(
            badges.text(badge_width),
            Style::default().fg(queued_color()),
        ));
    }
    if count > 0 {
        spans.push(Span::styled(
            format!(" │ {right}"),
            Style::default().fg(dim_color()),
        ));
    }
    jcode_tui_render::truncate_line_with_ellipsis_to_width(&Line::from(spans), width)
}

fn live_row(
    status: &crate::protocol::SwarmLiveStatus,
    stale_secs: Option<f32>,
    spinner: &'static str,
    badges: &StatusBadges,
    width: usize,
) -> Line<'static> {
    let phase = match status.phase.as_str() {
        "running_tool" if status.tool_name.as_deref() == Some("batch") => "batch",
        "running_tool" => "tool",
        "running" => "working",
        "waiting_network" => "offline",
        other => other,
    };
    let terminal = matches!(
        phase,
        "completed"
            | "done"
            | "failed"
            | "crashed"
            | "stopped"
            | "cancelled"
            | "blocked"
            | "waiting"
            | "idle"
            | "ready"
            | "queued"
    );
    let mut activity = Vec::new();
    // Warnings have a stable home, ahead of potentially unbounded tool intent.
    if status.phase == "streaming"
        && !status.stream_message_ended
        && let Some(stale) = stale_secs.filter(|s| *s > 2.0)
    {
        activity.push(format!(
            "{} {:.0}s",
            if stale > 10.0 { "stalled" } else { "no tokens" },
            stale
        ));
    }
    if terminal {
        if let Some(reason) = status
            .stop_reason
            .as_ref()
            .or(status.status_detail.as_ref())
        {
            activity.push(reason.clone());
        }
    } else {
        // draw_status places important warnings at the start of this field.
        if let Some(intent) = &status.tool_intent
            && intent.contains('⚠')
        {
            activity.push(intent.clone());
        }
        if let Some(name) = &status.tool_name {
            activity.push(name.clone());
        }
        if let Some(intent) = &status.tool_intent
            && !intent.contains('⚠')
        {
            activity.push(intent.clone());
        }
        if let Some(connection) = &status.connection_phase {
            activity.push(match connection.as_str() {
                "authenticating" => "refreshing auth".into(),
                "sending_request" => "sending context".into(),
                "waiting_for_response" => "waiting for response".into(),
                other => other.into(),
            });
        }
        activity.extend(collect_transport_context_labels(
            status
                .status_detail
                .as_deref()
                .and_then(normalize_status_detail),
            status
                .connection_type
                .as_deref()
                .map(display_connection_type),
            None,
        ));
    }
    activity.retain(|detail| !detail.trim().is_empty());
    let glyph = match phase {
        "completed" | "done" => "✓",
        "failed" | "crashed" => "✗",
        "blocked" => "⊘",
        "waiting" | "offline" => "↻",
        "idle" | "ready" | "stopped" | "cancelled" => "◼",
        _ => spinner,
    };
    let telemetry = if terminal {
        [" ".repeat(7), " ".repeat(7), " ".repeat(8)]
    } else {
        stable_status_telemetry(status)
    };
    compose_status(
        glyph,
        phase,
        status.elapsed_ms,
        &activity.join(" · "),
        &telemetry,
        badges,
        width,
    )
}

#[cfg(test)]
pub(super) fn live_processing_status_line_at_width(
    status: &crate::protocol::SwarmLiveStatus,
    stale_secs: Option<f32>,
    spinner: &'static str,
    width: usize,
) -> Line<'static> {
    live_row(status, stale_secs, spinner, &StatusBadges::default(), width)
}

pub(super) fn draw_status(frame: &mut Frame, app: &dyn TuiState, area: Rect, pending_count: usize) {
    if app.status_notice().as_deref() == Some(crate::telemetry::OAUTH_ACCOUNT_NOTICE_TEXT) {
        crate::tui::app::oauth_telemetry::note_status_render(area);
        let notice = Line::from(status_single_line(
            crate::telemetry::OAUTH_ACCOUNT_NOTICE_TEXT,
        ));
        frame.render_widget(
            Paragraph::new(jcode_tui_render::truncate_line_with_ellipsis_to_width(
                &notice,
                area.width as usize,
            ))
            .style(Style::default().fg(accent_color())),
            area,
        );
        return;
    }
    let elapsed = app.elapsed().map(|d| d.as_secs_f32()).unwrap_or(0.0);
    let mut snapshot = live_status_snapshot(app, elapsed);
    snapshot.elapsed_ms = app
        .elapsed()
        .map(|d| d.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0);
    let mut activity = Vec::new();
    let build = crate::build::read_build_progress();
    snapshot.phase = if let Some(build) = build {
        activity.push(build.to_string());
        "building".into()
    } else if let Some(remaining) = app.rate_limit_remaining() {
        activity.push(format!(
            "auto-retry in {}",
            stable_status_elapsed(remaining.as_millis().min(u64::MAX as u128) as u64).trim()
        ));
        "rate limited".into()
    } else if app.is_processing() {
        match app.status() {
            ProcessingStatus::Idle => "idle".into(),
            ProcessingStatus::Sending => "sending".into(),
            ProcessingStatus::Thinking(_) => "thinking".into(),
            ProcessingStatus::Streaming => "streaming".into(),
            ProcessingStatus::Connecting(ref phase) => {
                snapshot.connection_phase = Some(connection_phase_label(phase));
                let phase_elapsed = app
                    .connection_phase_elapsed()
                    .map_or(elapsed, |d| d.as_secs_f32());
                if matches!(phase, crate::message::ConnectionPhase::Retrying { .. })
                    || (phase_elapsed > 10.0
                        && matches!(
                            phase,
                            crate::message::ConnectionPhase::Authenticating
                                | crate::message::ConnectionPhase::Connecting
                                | crate::message::ConnectionPhase::SendingRequest
                        ))
                {
                    activity.push("⚠ connection delayed".into());
                }
                "connecting".into()
            }
            ProcessingStatus::WaitingForNetwork { listener } => {
                activity.push(format!(
                    "network disconnected, waiting to retry · {listener}"
                ));
                "waiting_network".into()
            }
            ProcessingStatus::RunningTool(ref name) => {
                snapshot.tool_name = Some(name.clone());
                if name == "batch" {
                    let calls = app.streaming_tool_calls();
                    let initial = calls
                        .last()
                        .and_then(|tc| tc.input.get("tool_calls"))
                        .and_then(|v| v.as_array())
                        .map(|a| a.len());
                    let mut spans = Vec::new();
                    append_batch_progress_spans(
                        &mut spans,
                        ai_color(),
                        app.batch_progress(),
                        initial,
                    );
                    activity.push(
                        spans
                            .iter()
                            .map(|s| s.content.as_ref())
                            .collect::<String>()
                            .trim_start_matches(" · ")
                            .to_string(),
                    );
                } else if let Some(detail) = app
                    .streaming_tool_calls()
                    .last()
                    .map(get_tool_activity_detail)
                    .filter(|d| !d.is_empty())
                {
                    activity.push(detail);
                }
                "running_tool".into()
            }
        }
    } else {
        "idle".into()
    };
    if app.is_processing() {
        let mut warnings = Vec::new();
        if let Some(notice) = app.active_experimental_feature_notice() {
            warnings.push(format!("⚠ {notice}"));
        }
        if matches!(snapshot.phase.as_str(), "streaming" | "running_tool") {
            let (read, creation) = app.streaming_cache_tokens();
            if let Some(problem) = detect_kv_cache_problem(
                &app.provider_name(),
                app.upstream_provider().as_deref(),
                app.display_user_message_count(),
                app.streaming_tokens().0,
                read,
                creation,
                app.cache_ttl_status().as_ref(),
            ) {
                warnings.push(format!(
                    "⚠ {} cache miss",
                    problem
                        .affected_tokens
                        .map(format_stream_tokens)
                        .unwrap_or_else(|| "kv".into())
                ));
            }
        }
        warnings.append(&mut activity);
        activity = warnings;
        if let Some(subagent) = app.subagent_status() {
            activity.push(subagent);
        }
        if let Some(upstream) = app.upstream_provider() {
            activity.push(format!("via {upstream}"));
        }
    } else if snapshot.phase == "idle" {
        // Idle is steady except for actionable session-history warnings. No rotating tips.
        if let Some((input, output)) = app.total_session_tokens()
            && let Some(warning) = occasional_session_history_warning(
                input.saturating_add(output),
                app.session_compaction_count(),
                app.context_limit(),
                area.width as usize,
                app.animation_elapsed() as u64,
            )
        {
            activity.push(format!("⚠ {warning}"));
        }
        snapshot.status_detail = activity.first().cloned();
        snapshot.connection_type = None;
    }
    if snapshot.phase != "streaming" {
        snapshot.output_tps_milli = None;
    }
    snapshot.tool_intent = Some(activity.join(" · "));
    let badges = StatusBadges {
        queued: pending_count,
        background: app
            .background_task_rows()
            .iter()
            .filter(|t| t.status == crate::tui::BackgroundTaskRowStatus::Running)
            .count(),
    };
    let line = live_row(
        &snapshot,
        app.time_since_activity().map(|d| d.as_secs_f32()),
        super::super::activity_indicator(elapsed, 12.5),
        &badges,
        area.width as usize,
    );
    crate::memory::check_staleness();
    // Center the composer container, never the changing status text inside it.
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Left), area);
}
