use super::*;

pub(super) fn active_codemode(app: &dyn TuiState) -> Option<crate::message::ToolCall> {
    // Connection-phase and nested-tool events can replace RunningTool while
    // the parent call is still live. ToolDone removes it from this list, so
    // pending tool calls, not the transient status label, own its lifetime.
    if !app.is_processing() {
        return None;
    }
    app.streaming_tool_calls()
        .into_iter()
        .rev()
        .find(|tool| tools_ui::canonical_tool_name(&tool.name) == "codemode")
}

pub(super) fn active_batch_progress(app: &dyn TuiState) -> Option<crate::bus::BatchProgress> {
    match app.status() {
        ProcessingStatus::RunningTool(name) if name == "batch" => app.batch_progress(),
        _ => None,
    }
}

pub(in super::super) fn active_batch_progress_hash(app: &dyn TuiState) -> u64 {
    if let Some(tool) = active_codemode(app) {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        tool.id.hash(&mut hasher);
        tool.input.to_string().hash(&mut hasher);
        tool.intent.hash(&mut hasher);
        super::super::activity_indicator_frame_index(app.animation_elapsed(), 12.5)
            .hash(&mut hasher);
        return hasher.finish();
    }
    let Some(progress) = active_batch_progress(app) else {
        return 0;
    };

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if progress.completed < progress.total {
        super::super::activity_indicator_frame_index(app.animation_elapsed(), 12.5)
            .hash(&mut hasher);
    }
    progress.total.hash(&mut hasher);
    progress.completed.hash(&mut hasher);
    progress.last_completed.hash(&mut hasher);
    for subcall in &progress.subcalls {
        subcall.index.hash(&mut hasher);
        subcall.tool_call.id.hash(&mut hasher);
        subcall.tool_call.name.hash(&mut hasher);
        match subcall.state {
            crate::bus::BatchSubcallState::Running => 0u8,
            crate::bus::BatchSubcallState::Succeeded => 1u8,
            crate::bus::BatchSubcallState::Failed => 2u8,
        }
        .hash(&mut hasher);
        if let Ok(input) = serde_json::to_string(&subcall.tool_call.input) {
            input.hash(&mut hasher);
        }
    }
    hasher.finish()
}

pub(super) fn prepare_active_batch_progress(
    app: &dyn TuiState,
    width: u16,
    prefix_blank: bool,
) -> PreparedMessages {
    if let Some(tool) = active_codemode(app) {
        let block_width = if app.centered_mode() {
            super::super::centered_content_block_width(width, 96)
        } else {
            width as usize
        };
        let row_width = block_width.saturating_sub(1);
        let spinner = super::super::activity_indicator(app.animation_elapsed(), 12.5);
        let mut lines = Vec::new();
        if prefix_blank {
            lines.push(Line::from(""));
        }
        let mut header = vec![
            Span::styled(
                format!("  {spinner} "),
                Style::default().fg(rgb(255, 193, 94)),
            ),
            Span::styled("codemode", Style::default().fg(tool_color())),
        ];
        if let Some(intent) = tool
            .intent
            .as_deref()
            .filter(|intent| !intent.trim().is_empty())
        {
            header.push(Span::styled(" · ", Style::default().fg(dim_color())));
            header.push(Span::styled(
                intent.to_owned(),
                Style::default().fg(tool_color()),
            ));
        }
        lines.push(super::super::truncate_line_with_ellipsis_to_width(
            &Line::from(header),
            row_width,
        ));
        lines.extend(tools_ui::codemode_preview(&tool, row_width));
        if app.centered_mode() {
            super::super::left_pad_lines_to_block_width(&mut lines, width, block_width);
        }
        return wrap_lines_with_map(lines, &[], &[], &[], &[], &[], width, &[], &[], &[]);
    }
    let Some(progress) = active_batch_progress(app) else {
        return empty_prepared_messages();
    };

    let centered = app.centered_mode();
    let accent = rgb(255, 193, 94);
    let spinner = super::super::activity_indicator(app.animation_elapsed(), 12.5);
    let block_width = if centered {
        super::super::centered_content_block_width(width, 96)
    } else {
        width as usize
    };
    let row_width = block_width.saturating_sub(1);
    let mut lines: Vec<Line<'static>> = Vec::new();

    if prefix_blank {
        lines.push(Line::from(""));
    }

    let mut header = vec![
        Span::styled(format!("  {} ", spinner), Style::default().fg(accent)),
        Span::styled("batch", Style::default().fg(tool_color())),
        Span::styled(
            format!(" · {}/{} done", progress.completed, progress.total),
            Style::default().fg(dim_color()),
        ),
    ];
    if let Some(last) = progress
        .last_completed
        .as_ref()
        .filter(|_| progress.completed < progress.total)
    {
        header.push(Span::styled(
            format!(" · last done: {}", last),
            Style::default().fg(dim_color()),
        ));
    }
    lines.push(super::super::truncate_line_with_ellipsis_to_width(
        &Line::from(header),
        width.saturating_sub(1) as usize,
    ));

    let mut hidden_completed = 0usize;
    for subcall in &progress.subcalls {
        let (icon, icon_color) = match subcall.state {
            crate::bus::BatchSubcallState::Running => (spinner, accent),
            crate::bus::BatchSubcallState::Succeeded => {
                hidden_completed += 1;
                continue;
            }
            crate::bus::BatchSubcallState::Failed => ("✗", rgb(220, 100, 100)),
        };

        lines.push(tools_ui::render_batch_subcall_line(
            &subcall.tool_call,
            icon,
            icon_color,
            50,
            Some(row_width),
            None,
        ));
    }

    if hidden_completed > 0 && progress.completed < progress.total {
        lines.push(Line::from(Span::styled(
            format!("    … {} completed", hidden_completed),
            Style::default().fg(dim_color()),
        )));
    }

    if centered {
        super::super::left_pad_lines_to_block_width(&mut lines, width, block_width);
    }

    wrap_lines_with_map(lines, &[], &[], &[], &[], &[], width, &[], &[], &[])
}
