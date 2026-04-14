use super::*;

pub(super) fn render_memory_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(info) = &data.memory_info else {
        return Vec::new();
    };
    if inner.width == 0 || inner.height == 0 {
        return Vec::new();
    }
    if info.total_count == 0 && info.activity.is_none() && info.sidecar_model.is_none() {
        return Vec::new();
    }

    let mut lines: Vec<Line> = Vec::new();
    let max_width = inner.width as usize;
    let activity = info.activity.as_ref();

    lines.push(render_memory_header_line(info, activity, max_width));

    if lines.len() < inner.height as usize
        && let Some(count_line) = render_memory_count_line(info, max_width)
    {
        lines.push(count_line);
    }

    if let Some(activity) = activity {
        if lines.len() < inner.height as usize {
            lines.push(render_memory_status_line(activity, max_width));
        }

        if lines.len() < inner.height as usize
            && let Some(model_line) = render_memory_model_line(info, max_width)
        {
            lines.push(model_line);
        }

        if memory_should_render_pipeline(activity) {
            for line in render_memory_pipeline_display_lines(activity, max_width) {
                if lines.len() >= inner.height as usize {
                    break;
                }
                lines.push(line);
            }
        }

        if lines.len() < inner.height as usize
            && let Some(trace_line) = render_memory_last_trace_line(activity, max_width)
        {
            lines.push(trace_line);
        }
    } else if lines.len() < inner.height as usize
        && let Some(model_line) = render_memory_model_line(info, max_width)
    {
        lines.push(model_line);
    }

    lines.truncate(inner.height as usize);
    lines
}

fn render_memory_header_line(
    _info: &MemoryInfo,
    activity: Option<&MemoryActivity>,
    max_width: usize,
) -> Line<'static> {
    let title = "Memory".to_string();
    let (badge, badge_color) = memory_status_badge(activity);
    let badge_text = format!(" {} ", badge);
    let title_width = UnicodeWidthStr::width(title.as_str());
    let badge_width = UnicodeWidthStr::width(badge_text.as_str());
    let available_title = if max_width > badge_width + 2 {
        max_width.saturating_sub(badge_width + 2)
    } else {
        max_width
    };

    let mut spans = vec![
        Span::styled("🧠 ", Style::default().fg(rgb(200, 150, 255))),
        Span::styled(
            truncate_with_ellipsis(&title, available_title.max(6)),
            Style::default().fg(rgb(210, 210, 220)).bold(),
        ),
    ];

    if max_width >= title_width + badge_width + 2 {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            badge_text,
            Style::default().fg(badge_color).bg(rgb(32, 32, 40)).bold(),
        ));
    }

    Line::from(spans)
}

fn render_memory_count_line(info: &MemoryInfo, max_width: usize) -> Option<Line<'static>> {
    if info.total_count == 0 {
        return None;
    }

    Some(Line::from(vec![Span::styled(
        truncate_with_ellipsis(&memory_count_label(info.total_count), max_width.max(8)),
        Style::default().fg(rgb(160, 160, 170)).bold(),
    )]))
}

fn memory_count_label(total_count: usize) -> String {
    if total_count == 1 {
        "1 memory".to_string()
    } else {
        format!("{total_count} memories")
    }
}

fn memory_recent_done(activity: &MemoryActivity) -> bool {
    matches!(activity.state, MemoryState::Idle)
        && activity
            .pipeline
            .as_ref()
            .map(PipelineState::is_complete)
            .unwrap_or(false)
        && activity.state_since.elapsed() <= Duration::from_secs(5)
}

fn memory_should_render_pipeline(activity: &MemoryActivity) -> bool {
    if activity.pipeline.is_some() {
        return !matches!(activity.state, MemoryState::Idle) || memory_recent_done(activity);
    }
    activity.is_processing()
}

fn memory_compact_summary(info: &MemoryInfo) -> String {
    if let Some(activity) = info.activity.as_ref() {
        if activity.is_processing() {
            return memory_active_summary(&activity.state)
                .or_else(|| {
                    activity
                        .pipeline
                        .as_ref()
                        .map(memory_pipeline_progress_summary)
                })
                .or_else(|| memory_last_trace_summary(activity))
                .unwrap_or_else(|| "working".to_string());
        }

        if memory_recent_done(activity) {
            return "done".to_string();
        }

        return "idle".to_string();
    }

    if info.total_count > 0 {
        "idle".to_string()
    } else {
        info.sidecar_model
            .as_deref()
            .map(compact_memory_model_label)
            .unwrap_or_else(|| "idle".to_string())
    }
}

fn memory_status_badge(activity: Option<&MemoryActivity>) -> (String, Color) {
    let Some(activity) = activity else {
        return ("IDLE".to_string(), rgb(120, 120, 130));
    };

    if let Some(pipeline) = &activity.pipeline {
        let live_step = [
            ("SEARCH", &pipeline.search, rgb(140, 180, 255)),
            ("VERIFY", &pipeline.verify, rgb(255, 200, 100)),
            ("INJECT", &pipeline.inject, rgb(200, 150, 255)),
            ("UPDATE", &pipeline.maintain, rgb(120, 220, 180)),
        ]
        .into_iter()
        .find(|(_, status, _)| matches!(status, StepStatus::Running | StepStatus::Error));

        if let Some((label, status, color)) = live_step {
            return (
                if matches!(status, StepStatus::Error) {
                    "FAILED".to_string()
                } else {
                    label.to_string()
                },
                if matches!(status, StepStatus::Error) {
                    rgb(255, 100, 100)
                } else {
                    color
                },
            );
        }

        if memory_recent_done(activity) {
            return ("DONE".to_string(), rgb(100, 200, 100));
        }
    }

    match &activity.state {
        MemoryState::Idle => ("IDLE".to_string(), rgb(120, 120, 130)),
        MemoryState::Embedding => ("SEARCH".to_string(), rgb(140, 180, 255)),
        MemoryState::SidecarChecking { .. } => ("VERIFY".to_string(), rgb(255, 200, 100)),
        MemoryState::FoundRelevant { .. } => ("READY".to_string(), rgb(100, 200, 100)),
        MemoryState::Extracting { .. } => ("SAVE".to_string(), rgb(200, 150, 255)),
        MemoryState::Maintaining { .. } => ("UPDATE".to_string(), rgb(120, 220, 180)),
        MemoryState::ToolAction { .. } => ("TOOL".to_string(), rgb(140, 200, 255)),
    }
}

fn render_memory_model_line(info: &MemoryInfo, max_width: usize) -> Option<Line<'static>> {
    let model = info.sidecar_model.as_deref()?.trim();
    if model.is_empty() {
        return None;
    }

    let available = max_width.saturating_sub(7);
    Some(Line::from(vec![
        Span::styled("Model: ", Style::default().fg(rgb(120, 120, 130))),
        Span::styled(
            truncate_with_ellipsis(model, available),
            Style::default().fg(rgb(140, 200, 255)).bold(),
        ),
    ]))
}

fn render_memory_status_line(activity: &MemoryActivity, max_width: usize) -> Line<'static> {
    let (_badge, badge_color) = memory_status_badge(Some(activity));
    let summary = memory_state_detail(&activity.state)
        .or_else(|| {
            if memory_should_render_pipeline(activity) {
                activity
                    .pipeline
                    .as_ref()
                    .map(memory_pipeline_progress_summary)
            } else {
                None
            }
        })
        .or_else(|| memory_last_trace_summary(activity))
        .unwrap_or_else(|| "idle".to_string());
    let prefix = if activity.is_processing() {
        "Now: "
    } else {
        "Last: "
    };
    let age = format_age(activity.state_since.elapsed());
    let prefix_width = UnicodeWidthStr::width(prefix);
    let age_width = UnicodeWidthStr::width(age.as_str()) + 3;
    let summary_width = UnicodeWidthStr::width(summary.as_str());
    let show_age = prefix_width + summary_width + age_width <= max_width;
    let available = if show_age {
        max_width.saturating_sub(prefix_width + age_width)
    } else {
        max_width.saturating_sub(prefix_width)
    };

    let mut spans = vec![
        Span::styled(prefix, Style::default().fg(rgb(120, 120, 130))),
        Span::styled(
            truncate_smart(&summary, available),
            Style::default().fg(badge_color).bold(),
        ),
    ];

    if show_age {
        spans.push(Span::styled(" · ", Style::default().fg(rgb(90, 90, 100))));
        spans.push(Span::styled(age, Style::default().fg(rgb(120, 120, 130))));
    }

    Line::from(spans)
}

fn render_memory_pipeline_lines(pipeline: &PipelineState, max_width: usize) -> Vec<Line<'static>> {
    vec![
        render_memory_step_line(
            "╭ ",
            "Find matches",
            &pipeline.search,
            memory_step_detail(
                "search",
                &pipeline.search,
                pipeline.search_result.as_ref(),
                None,
            ),
            max_width,
        ),
        render_memory_step_line(
            "├ ",
            "Check relevance",
            &pipeline.verify,
            memory_step_detail(
                "verify",
                &pipeline.verify,
                pipeline.verify_result.as_ref(),
                pipeline.verify_progress,
            ),
            max_width,
        ),
        render_memory_step_line(
            "├ ",
            "Inject context",
            &pipeline.inject,
            memory_step_detail(
                "inject",
                &pipeline.inject,
                pipeline.inject_result.as_ref(),
                None,
            ),
            max_width,
        ),
        render_memory_step_line(
            "╰ ",
            "Update memory",
            &pipeline.maintain,
            memory_step_detail(
                "maintain",
                &pipeline.maintain,
                pipeline.maintain_result.as_ref(),
                None,
            ),
            max_width,
        ),
    ]
}

fn render_memory_pipeline_display_lines(
    activity: &MemoryActivity,
    max_width: usize,
) -> Vec<Line<'static>> {
    if let Some(pipeline) = &activity.pipeline {
        return render_memory_pipeline_lines(pipeline, max_width);
    }

    let (search, verify, inject, maintain, verify_progress) =
        fallback_pipeline_statuses(&activity.state);

    vec![
        render_memory_step_line(
            "╭ ",
            "Find matches",
            &search,
            memory_step_detail("search", &search, None, None),
            max_width,
        ),
        render_memory_step_line(
            "├ ",
            "Check relevance",
            &verify,
            memory_step_detail("verify", &verify, None, verify_progress),
            max_width,
        ),
        render_memory_step_line(
            "├ ",
            "Inject context",
            &inject,
            memory_step_detail("inject", &inject, None, None),
            max_width,
        ),
        render_memory_step_line(
            "╰ ",
            "Update memory",
            &maintain,
            memory_step_detail("maintain", &maintain, None, None),
            max_width,
        ),
    ]
}

fn fallback_pipeline_statuses(
    state: &MemoryState,
) -> (
    StepStatus,
    StepStatus,
    StepStatus,
    StepStatus,
    Option<(usize, usize)>,
) {
    match state {
        MemoryState::Idle => (
            StepStatus::Pending,
            StepStatus::Pending,
            StepStatus::Pending,
            StepStatus::Pending,
            None,
        ),
        MemoryState::Embedding => (
            StepStatus::Running,
            StepStatus::Pending,
            StepStatus::Pending,
            StepStatus::Pending,
            None,
        ),
        MemoryState::SidecarChecking { count } => (
            StepStatus::Done,
            StepStatus::Running,
            StepStatus::Pending,
            StepStatus::Pending,
            Some((0, *count)),
        ),
        MemoryState::FoundRelevant { .. } => (
            StepStatus::Done,
            StepStatus::Done,
            StepStatus::Running,
            StepStatus::Pending,
            None,
        ),
        MemoryState::Extracting { .. } | MemoryState::Maintaining { .. } => (
            StepStatus::Done,
            StepStatus::Done,
            StepStatus::Done,
            StepStatus::Running,
            None,
        ),
        MemoryState::ToolAction { .. } => (
            StepStatus::Pending,
            StepStatus::Pending,
            StepStatus::Pending,
            StepStatus::Running,
            None,
        ),
    }
}

fn render_memory_last_trace_line(
    activity: &MemoryActivity,
    max_width: usize,
) -> Option<Line<'static>> {
    let event = activity
        .recent_events
        .iter()
        .find(|event| is_traceworthy_memory_event(event))?;
    let (icon, text, color) = format_event_for_expanded(event, max_width.saturating_sub(8));
    if text.is_empty() {
        return None;
    }

    Some(Line::from(vec![
        Span::styled("Trace: ", Style::default().fg(rgb(120, 120, 130))),
        Span::styled(format!("{} ", icon), Style::default().fg(color)),
        Span::styled(
            truncate_with_ellipsis(&text, max_width.saturating_sub(7)),
            Style::default().fg(rgb(160, 160, 170)),
        ),
    ]))
}

fn render_memory_step_line(
    prefix: &'static str,
    label: &'static str,
    status: &StepStatus,
    detail: Option<String>,
    max_width: usize,
) -> Line<'static> {
    let (marker, marker_color, label_color, detail_color, fallback) = match status {
        StepStatus::Pending => (
            "·",
            rgb(100, 100, 110),
            rgb(140, 140, 150),
            rgb(120, 120, 130),
            Some("waiting"),
        ),
        StepStatus::Running => (
            current_memory_spinner_frame(),
            rgb(255, 200, 100),
            rgb(220, 220, 230),
            rgb(255, 200, 100),
            Some("running"),
        ),
        StepStatus::Done => (
            "✓",
            rgb(100, 200, 100),
            rgb(180, 180, 190),
            rgb(160, 160, 170),
            Some("done"),
        ),
        StepStatus::Error => (
            "!",
            rgb(255, 100, 100),
            rgb(220, 180, 180),
            rgb(255, 140, 140),
            Some("failed"),
        ),
        StepStatus::Skipped => (
            "-",
            rgb(100, 100, 110),
            rgb(120, 120, 130),
            rgb(120, 120, 130),
            Some("skipped"),
        ),
    };

    let detail = detail
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| fallback.unwrap_or("").to_string());
    let available = max_width
        .saturating_sub(UnicodeWidthStr::width(prefix))
        .saturating_sub(UnicodeWidthStr::width(marker))
        .saturating_sub(label.chars().count() + 4);
    let rail_color = if matches!(status, StepStatus::Running) {
        rgb(255, 200, 100)
    } else {
        rgb(80, 80, 92)
    };

    Line::from(vec![
        Span::styled(prefix.to_string(), Style::default().fg(rail_color)),
        Span::styled(format!("{} ", marker), Style::default().fg(marker_color)),
        Span::styled(
            label.to_string(),
            if matches!(status, StepStatus::Running | StepStatus::Done) {
                Style::default().fg(label_color).bold()
            } else {
                Style::default().fg(label_color)
            },
        ),
        Span::styled("  ", Style::default().fg(rgb(100, 100, 110))),
        Span::styled(
            truncate_smart(&detail, available),
            Style::default().fg(detail_color),
        ),
    ])
}

fn current_memory_spinner_frame() -> &'static str {
    if !crate::perf::tui_policy().enable_decorative_animations {
        return "•";
    }

    const FRAMES: [&str; 4] = ["/", "-", "\\", "|"];
    let frame = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_millis() / 120) as usize)
        .unwrap_or(0);
    FRAMES[frame % FRAMES.len()]
}

fn memory_step_detail(
    step: &str,
    status: &StepStatus,
    result: Option<&StepResult>,
    progress: Option<(usize, usize)>,
) -> Option<String> {
    match status {
        StepStatus::Running => progress.map(|(done, total)| format!("{done}/{total}")),
        StepStatus::Done | StepStatus::Error => result.and_then(|res| {
            let summary = res.summary.trim();
            if summary.is_empty() {
                None
            } else {
                Some(match step {
                    "search" if summary.ends_with("hits") => summary.replace("hits", "found"),
                    _ => summary.to_string(),
                })
            }
        }),
        StepStatus::Pending | StepStatus::Skipped => None,
    }
}

fn memory_pipeline_progress_summary(pipeline: &PipelineState) -> String {
    let completed = [
        &pipeline.search,
        &pipeline.verify,
        &pipeline.inject,
        &pipeline.maintain,
    ]
    .into_iter()
    .filter(|status| matches!(status, StepStatus::Done))
    .count();

    let active = [
        ("search", &pipeline.search, None),
        ("verify", &pipeline.verify, pipeline.verify_progress),
        ("inject", &pipeline.inject, None),
        ("update", &pipeline.maintain, None),
    ]
    .into_iter()
    .find_map(|(name, status, progress)| match status {
        StepStatus::Running => Some(if let Some((done, total)) = progress {
            format!("{} {}/{}", name, done, total)
        } else {
            name.to_string()
        }),
        StepStatus::Error => Some(format!("{} failed", name)),
        _ => None,
    });

    if let Some(active) = active {
        format!("{}/4 done · {}", completed, active)
    } else {
        format!("{}/4 done", completed)
    }
}

pub(super) fn render_memory_compact(info: &MemoryInfo, inner_width: u16) -> Vec<Line<'static>> {
    let max_width = inner_width.saturating_sub(2) as usize;
    let title = if info.total_count > 0 {
        memory_count_label(info.total_count)
    } else {
        "Memory".to_string()
    };
    let summary = memory_compact_summary(info);

    let title_width = UnicodeWidthStr::width(title.as_str());
    let summary_width = max_width.saturating_sub(title_width + 5);
    let accent = if let Some(activity) = info.activity.as_ref() {
        memory_status_badge(Some(activity)).1
    } else if info.total_count > 0 {
        rgb(160, 160, 170)
    } else {
        rgb(140, 200, 255)
    };

    vec![Line::from(vec![
        Span::styled("🧠 ", Style::default().fg(rgb(200, 150, 255))),
        Span::styled(title, Style::default().fg(rgb(180, 180, 190)).bold()),
        Span::styled(" · ", Style::default().fg(rgb(100, 100, 110))),
        Span::styled(
            truncate_with_ellipsis(&summary, summary_width.max(8)),
            Style::default().fg(accent),
        ),
    ])]
}

pub(super) fn render_memory_expanded(info: &MemoryInfo, inner: Rect) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();
    let max_width = inner.width.saturating_sub(2) as usize;

    lines.push(render_memory_header_line(
        info,
        info.activity.as_ref(),
        max_width,
    ));
    if let Some(count_line) = render_memory_count_line(info, max_width) {
        lines.push(count_line);
    }
    if let Some(activity) = &info.activity {
        lines.push(render_memory_status_line(activity, max_width));
    }
    if let Some(model_line) = render_memory_model_line(info, max_width) {
        lines.push(model_line);
    }

    if let Some(activity) = &info.activity {
        if memory_should_render_pipeline(activity) {
            lines.extend(render_memory_pipeline_display_lines(activity, max_width));
        }

        if let Some(last_line) = render_memory_last_trace_line(activity, max_width) {
            lines.push(last_line);
        }
    }

    lines.truncate(inner.height as usize);
    lines
}
fn format_age(duration: std::time::Duration) -> String {
    let secs = duration.as_secs();
    if secs < 2 {
        "now".to_string()
    } else if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}
