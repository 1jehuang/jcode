use super::*;
use ratatui::widgets::{Block, BorderType, Borders};
use unicode_width::UnicodeWidthStr;

fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn truncate_display(text: &str, max_width: usize) -> String {
    if display_width(text) <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }

    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width + 1 > max_width {
            break;
        }
        out.push(ch);
        used += ch_width;
    }
    out.push('…');
    out
}

fn pad_left_display(text: &str, width: usize) -> String {
    let truncated = truncate_display(text, width);
    let padding = width.saturating_sub(display_width(truncated.as_str()));
    format!("{}{}", truncated, " ".repeat(padding))
}

fn pad_center_display(text: &str, width: usize) -> String {
    let truncated = truncate_display(text, width);
    let rendered = display_width(truncated.as_str());
    let total_padding = width.saturating_sub(rendered);
    let left_padding = total_padding / 2;
    let right_padding = total_padding.saturating_sub(left_padding);
    format!(
        "{}{}{}",
        " ".repeat(left_padding),
        truncated,
        " ".repeat(right_padding)
    )
}

fn picker_entry_display_name(entry: &crate::tui::PickerEntry) -> String {
    let default_marker = if entry.is_default { " ⚙" } else { "" };
    let suffix = if entry.recommended && !entry.is_current {
        format!(" ★{}", default_marker)
    } else if entry.old && !entry.is_current {
        if let Some(ref date) = entry.created_date {
            format!(" {}{}", date, default_marker)
        } else {
            format!(" old{}", default_marker)
        }
    } else if let Some(ref date) = entry.created_date {
        if !entry.is_current {
            format!(" {}{}", date, default_marker)
        } else {
            default_marker.to_string()
        }
    } else {
        default_marker.to_string()
    };

    format!("{}{}", entry.name, suffix)
}

fn picker_row_marker(is_row_selected: bool, unavailable: bool) -> &'static str {
    if unavailable {
        "×"
    } else if is_row_selected {
        "▸"
    } else {
        " "
    }
}

fn route_detail_display_text(detail: &str, unavailable: bool) -> Option<String> {
    let trimmed = detail.trim();
    if unavailable {
        if trimmed.is_empty() {
            Some("unavailable".to_string())
        } else {
            Some(format!("unavailable · {}", trimmed))
        }
    } else if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn account_picker_shows_provider_badge(picker: &crate::tui::InlineInteractiveState) -> bool {
    let mut providers: Vec<&str> = Vec::new();
    for &fi in &picker.filtered {
        let entry = &picker.entries[fi];
        if let Some(route) = entry.options.get(entry.selected_option) {
            let provider = route.provider.trim();
            if !provider.is_empty()
                && !providers
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(provider))
            {
                providers.push(provider);
                if providers.len() > 1 {
                    return true;
                }
            }
        }
    }
    false
}

fn account_picker_entry_title(
    entry: &crate::tui::PickerEntry,
    show_provider_badge: bool,
) -> (String, usize) {
    let display_name = picker_entry_display_name(entry);
    let provider_prefix = if show_provider_badge {
        entry
            .options
            .get(entry.selected_option)
            .map(|route| format!("{} · ", route.provider))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let prefix_chars = provider_prefix.chars().count();
    (format!("{}{}", provider_prefix, display_name), prefix_chars)
}

fn account_inline_interactive_state_label(entry: &crate::tui::PickerEntry) -> &'static str {
    entry.account_state_label().unwrap_or("—")
}

fn picker_render_width(picker: &crate::tui::InlineInteractiveState, max_width: usize) -> usize {
    let marker_width = 3usize;
    let is_preview = picker.preview;
    const WIDTH_SCAN_LIMIT: usize = 200;

    if picker.uses_compact_navigation() {
        let show_provider_badge = account_picker_shows_provider_badge(picker);
        let mut max_title_len = display_width("ACCOUNT");
        let mut max_state_len = display_width("STATE");

        for &fi in &picker.filtered {
            let entry = &picker.entries[fi];
            let (title, _) = account_picker_entry_title(entry, show_provider_badge);
            max_title_len = max_title_len.max(display_width(title.as_str()));
            max_state_len =
                max_state_len.max(display_width(account_inline_interactive_state_label(entry)));
        }

        let state_width = (max_state_len + 1).clamp(7, 10);
        let min_title_width = max_title_len.clamp(8, 10);
        let title_cap = if show_provider_badge { 42 } else { 34 };
        let budget = max_width.saturating_sub(marker_width + state_width);
        let title_width = max_title_len
            .min(title_cap)
            .min(budget.max(min_title_width.min(budget)));

        return marker_width + title_width + state_width;
    }

    let mut max_model_len = display_width(picker.primary_label());
    let mut max_provider_len = display_width(picker.secondary_label(is_preview));
    let mut max_via_len = display_width(picker.tertiary_label());

    for &fi in picker.filtered.iter().take(WIDTH_SCAN_LIMIT) {
        let entry = &picker.entries[fi];
        max_model_len = max_model_len.max(display_width(picker_entry_display_name(entry).as_str()));
        if let Some(route) = entry.active_option() {
            let provider_label = if entry.option_count() > 1 {
                format!("{} ({})", route.provider, entry.option_count())
            } else {
                route.provider.clone()
            };
            max_provider_len = max_provider_len.max(display_width(provider_label.as_str()));
            max_via_len = max_via_len.max(display_width(route.api_method.as_str()));
        }
    }

    let mut provider_width = (max_provider_len + 1).min(if is_preview { 16 } else { 20 });
    let mut via_width = (max_via_len + 1).min(12);
    let model_cap = if is_preview { 42 } else { 56 };
    let min_model_width = max_model_len.clamp(6, 8);

    let budget = max_width.saturating_sub(marker_width);
    if provider_width + via_width + min_model_width > budget {
        let provider_floor = 8usize.min(provider_width);
        let via_floor = 4usize.min(via_width);

        let provider_reduction = (provider_width + via_width + min_model_width)
            .saturating_sub(budget)
            .min(provider_width.saturating_sub(provider_floor));
        provider_width = provider_width.saturating_sub(provider_reduction);

        let via_reduction = (provider_width + via_width + min_model_width)
            .saturating_sub(budget)
            .min(via_width.saturating_sub(via_floor));
        via_width = via_width.saturating_sub(via_reduction);
    }

    let model_budget = budget.saturating_sub(provider_width + via_width);
    let model_width = max_model_len
        .min(model_cap)
        .min(model_budget.max(min_model_width.min(model_budget)));

    marker_width + provider_width + via_width + model_width
}

pub(super) fn format_elapsed(secs: f32) -> String {
    if secs >= 3600.0 {
        let hours = (secs / 3600.0) as u32;
        let mins = ((secs % 3600.0) / 60.0) as u32;
        format!("{}h {}m", hours, mins)
    } else if secs >= 60.0 {
        let mins = (secs / 60.0) as u32;
        let s = (secs % 60.0) as u32;
        format!("{}m {}s", mins, s)
    } else {
        format!("{:.1}s", secs)
    }
}

fn fuzzy_match_positions(pattern: &str, text: &str) -> Vec<usize> {
    let pat: Vec<char> = pattern
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if pat.is_empty() {
        return Vec::new();
    }
    let txt: Vec<char> = text.to_lowercase().chars().collect();
    let mut pi = 0;
    let mut positions = Vec::new();
    for (ti, &tc) in txt.iter().enumerate() {
        if pi < pat.len() && tc == pat[pi] {
            positions.push(ti);
            pi += 1;
        }
    }
    if pi == pat.len() {
        positions
    } else {
        Vec::new()
    }
}

pub(super) fn draw_inline_interactive(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    let picker = match app.inline_interactive_state() {
        Some(p) => p,
        None => return,
    };

    let height = area.height as usize;
    let width = area.width as usize;
    if height <= 2 || width <= 2 {
        return;
    }

    let selected = picker.selected;
    let total = picker.entries.len();
    let filtered_count = picker.filtered.len();
    let col = picker.column;
    let is_preview = picker.preview;
    let is_account_picker = picker.uses_compact_navigation();
    let is_usage_picker = picker.kind == crate::tui::PickerKind::Usage;

    let col_focus_style = Style::default().fg(accent_color()).bold();
    let col_dim_style = Style::default().fg(dim_color());
    let marker_width = 3usize;
    const WIDTH_SCAN_LIMIT: usize = 200;

    let show_account_provider_badge =
        is_account_picker && account_picker_shows_provider_badge(picker);
    let mut max_provider_len = 0usize;
    let mut max_via_len = 0usize;
    let mut max_account_title_len = display_width("ACCOUNT");
    let mut max_account_state_len = display_width("STATE");
    for &fi in picker.filtered.iter().take(WIDTH_SCAN_LIMIT) {
        let entry = &picker.entries[fi];
        let route = entry.active_option();
        if let Some(r) = route {
            max_provider_len = max_provider_len.max(display_width(r.provider.as_str()));
            max_via_len = max_via_len.max(display_width(r.api_method.as_str()));
        }
        if is_account_picker {
            let (title, _) = account_picker_entry_title(entry, show_account_provider_badge);
            max_account_title_len = max_account_title_len.max(display_width(title.as_str()));
            max_account_state_len = max_account_state_len
                .max(display_width(account_inline_interactive_state_label(entry)));
        }
    }
    max_provider_len = max_provider_len.max(8);
    max_via_len = max_via_len.max(3);

    let content_width = picker_render_width(picker, width.saturating_sub(2)).max(1);
    let outer_width = content_width.saturating_add(2).min(width);
    let horizontal_offset = if app.centered_mode() {
        area.width.saturating_sub(outer_width as u16) / 2
    } else {
        0
    };
    let render_area = Rect {
        x: area.x + horizontal_offset,
        y: area.y,
        width: outer_width as u16,
        height: area.height,
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(rgb(85, 85, 110)))
        .style(Style::default().bg(rgb(18, 18, 26)));
    frame.render_widget(block.clone(), render_area);

    let inner = block.inner(render_area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let height = inner.height as usize;
    let width = inner.width as usize;

    let provider_cap = if is_preview { 16 } else { 20 };
    let provider_width = (max_provider_len + 1).max(8).min(provider_cap);
    let via_width = (max_via_len + 1).clamp(4, 12);
    let account_state_width = (max_account_state_len + 1).clamp(7, 10);
    let account_title_width = width.saturating_sub(marker_width + account_state_width);
    let model_width = width.saturating_sub(marker_width + provider_width + via_width);

    let (col_labels, col_logical) = picker.header_layout(is_preview);
    let col_widths: [usize; 3] = if is_account_picker {
        [account_title_width, account_state_width, 0]
    } else if is_preview {
        [provider_width, model_width, via_width]
    } else {
        [model_width, provider_width, via_width]
    };

    let mut header_spans: Vec<Span> = Vec::new();

    let first_label = col_labels[0];
    let first_w = marker_width + col_widths[0];
    let first_style = if col_logical[0] == col {
        col_focus_style
    } else {
        col_dim_style
    };
    header_spans.push(Span::styled(
        format!(" {:<w$}", first_label, w = first_w.saturating_sub(1)),
        first_style,
    ));

    let second_label = col_labels[1];
    let second_w = col_widths[1];
    let second_style = if col_logical[1] == col {
        col_focus_style
    } else {
        col_dim_style
    };
    header_spans.push(Span::styled(
        if is_preview {
            format!("{:^w$}", second_label, w = second_w)
        } else {
            format!("{:<w$}", second_label, w = second_w)
        },
        second_style,
    ));

    if !is_account_picker {
        let third_label = col_labels[2];
        let third_style = if col_logical[2] == col {
            col_focus_style
        } else {
            col_dim_style
        };
        header_spans.push(Span::styled(format!(" {}", third_label), third_style));
    }

    let mut meta_parts = String::new();
    if !picker.filter.is_empty() {
        meta_parts.push_str(&format!("  \"{}\"", picker.filter));
    }
    let count_str = if filtered_count == total {
        format!(" ({})", total)
    } else {
        format!(" ({}/{})", filtered_count, total)
    };
    meta_parts.push_str(&count_str);
    header_spans.push(Span::styled(meta_parts, Style::default().fg(dim_color())));

    if is_preview {
        header_spans.push(Span::styled(
            picker.preview_submit_hint(),
            Style::default().fg(rgb(60, 60, 80)).italic(),
        ));
    } else {
        header_spans.push(Span::styled(
            picker.active_submit_hint(),
            Style::default().fg(rgb(60, 60, 80)),
        ));
        if picker.shows_default_shortcut_hint() {
            header_spans.push(Span::styled(
                "  ^D=default",
                Style::default().fg(rgb(60, 60, 80)).italic(),
            ));
        }
    }

    let row_base_width = if is_account_picker {
        marker_width + account_title_width + account_state_width
    } else {
        marker_width + provider_width + via_width + model_width
    };
    let detail_width = width.saturating_sub(row_base_width).saturating_sub(2);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(header_spans));

    if picker.filtered.is_empty() {
        lines.push(Line::from(Span::styled(
            "   no matches",
            Style::default().fg(dim_color()).italic(),
        )));
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }

    let list_height = height.saturating_sub(1);
    if list_height == 0 {
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }

    let half = list_height / 2;
    let start = if selected <= half {
        0
    } else if selected + list_height - half > filtered_count {
        filtered_count.saturating_sub(list_height)
    } else {
        selected - half
    };
    let end = (start + list_height).min(filtered_count);

    for vi in start..end {
        let model_idx = picker.filtered[vi];
        let entry = &picker.entries[model_idx];
        let is_row_selected = vi == selected;
        let route = entry.active_option();
        let unavailable = route.map(|r| !r.available).unwrap_or(true);

        let marker = picker_row_marker(is_row_selected, unavailable);

        let mut spans: Vec<Span> = Vec::new();
        spans.push(Span::styled(
            format!(" {} ", marker),
            if unavailable {
                Style::default().fg(rgb(180, 120, 120)).bold()
            } else if is_row_selected {
                Style::default().fg(Color::White).bold()
            } else {
                Style::default().fg(dim_color())
            },
        ));
        let display_name = picker_entry_display_name(entry);
        let account_action_color = match &entry.action {
            crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Add { .. }) => {
                Some(rgb(140, 220, 170))
            }
            crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Replace {
                ..
            }) => Some(rgb(240, 200, 120)),
            crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::OpenCenter {
                ..
            }) => Some(rgb(150, 190, 255)),
            _ => None,
        };
        let primary_style = if unavailable {
            Style::default().fg(rgb(80, 80, 80))
        } else if is_row_selected && col == 0 {
            Style::default().fg(Color::White).bg(rgb(60, 60, 80)).bold()
        } else if let Some(color) = account_action_color {
            Style::default().fg(color).bold()
        } else if entry.is_current {
            Style::default().fg(accent_color())
        } else if entry.recommended {
            Style::default().fg(rgb(255, 220, 120))
        } else if entry.old {
            Style::default().fg(rgb(120, 120, 130))
        } else {
            Style::default().fg(rgb(200, 200, 220))
        };

        if is_account_picker {
            let (title_text, title_prefix_chars) =
                account_picker_entry_title(entry, show_account_provider_badge);
            let padded_title = pad_left_display(title_text.as_str(), account_title_width);
            let state_label = account_inline_interactive_state_label(entry);
            let state_display = format!(
                " {}",
                pad_left_display(state_label, account_state_width.saturating_sub(1))
            );
            let match_positions = if !picker.filter.is_empty() {
                fuzzy_match_positions(&picker.filter, &entry.name)
                    .into_iter()
                    .map(|p| p + title_prefix_chars)
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            let title_spans: Vec<Span> = if match_positions.is_empty() || unavailable {
                vec![Span::styled(padded_title, primary_style)]
            } else {
                let title_chars: Vec<char> = padded_title.chars().collect();
                let highlight_style = primary_style.underlined();
                let mut result = Vec::new();
                let mut run_start = 0;
                let mut is_match_run = !title_chars.is_empty() && match_positions.contains(&0);
                for ci in 1..=title_chars.len() {
                    let cur_is_match = ci < title_chars.len() && match_positions.contains(&ci);
                    if cur_is_match != is_match_run || ci == title_chars.len() {
                        let chunk: String = title_chars[run_start..ci].iter().collect();
                        result.push(Span::styled(
                            chunk,
                            if is_match_run {
                                highlight_style
                            } else {
                                primary_style
                            },
                        ));
                        run_start = ci;
                        is_match_run = cur_is_match;
                    }
                }
                result
            };

            let state_style = if unavailable {
                Style::default().fg(rgb(80, 80, 80))
            } else if is_row_selected {
                Style::default().fg(Color::White).bg(rgb(60, 60, 80)).bold()
            } else if entry.is_current {
                Style::default().fg(accent_color()).bold()
            } else if let Some(color) = account_action_color {
                Style::default().fg(color)
            } else {
                Style::default().fg(dim_color())
            };

            spans.extend(title_spans);
            spans.push(Span::styled(state_display, state_style));
            if let Some(route) = route
                && let Some(detail_text) = route_detail_display_text(&route.detail, unavailable)
                && detail_width > 0
            {
                spans.push(Span::styled(
                    format!("  {}", truncate_display(detail_text.as_str(), detail_width)),
                    if unavailable {
                        Style::default().fg(rgb(180, 120, 120)).italic()
                    } else {
                        Style::default().fg(dim_color())
                    },
                ));
            }

            lines.push(Line::from(spans));
            continue;
        }

        let padded_model = if is_preview {
            pad_center_display(display_name.as_str(), model_width)
        } else {
            pad_left_display(display_name.as_str(), model_width)
        };

        let match_positions = if !picker.filter.is_empty() {
            let raw = fuzzy_match_positions(&picker.filter, &entry.name);
            if is_preview && !raw.is_empty() {
                let name_len = display_width(display_name.as_str());
                let pad = if name_len < model_width {
                    (model_width - name_len) / 2
                } else {
                    0
                };
                raw.into_iter().map(|p| p + pad).collect()
            } else {
                raw
            }
        } else {
            Vec::new()
        };
        let model_spans: Vec<Span> = if match_positions.is_empty() || unavailable {
            vec![Span::styled(padded_model, primary_style)]
        } else {
            let model_chars: Vec<char> = padded_model.chars().collect();
            let highlight_style = primary_style.underlined();
            let mut result = Vec::new();
            let mut run_start = 0;
            let mut is_match_run = !model_chars.is_empty() && match_positions.contains(&0);
            for ci in 1..=model_chars.len() {
                let cur_is_match = ci < model_chars.len() && match_positions.contains(&ci);
                if cur_is_match != is_match_run || ci == model_chars.len() {
                    let chunk: String = model_chars[run_start..ci].iter().collect();
                    result.push(Span::styled(
                        chunk,
                        if is_match_run {
                            highlight_style
                        } else {
                            primary_style
                        },
                    ));
                    run_start = ci;
                    is_match_run = cur_is_match;
                }
            }
            result
        };

        let route_count = entry.option_count();
        let provider_raw = route.map(|r| r.provider.as_str()).unwrap_or("—");
        let provider_label = if col == 0 && route_count > 1 {
            format!("{} ({})", provider_raw, route_count)
        } else {
            provider_raw.to_string()
        };
        let pw = provider_width.saturating_sub(1);
        let provider_display = format!(" {}", pad_left_display(provider_label.as_str(), pw));
        let provider_style = if unavailable {
            Style::default().fg(rgb(80, 80, 80))
        } else if is_row_selected && col == 1 {
            Style::default().fg(Color::White).bg(rgb(60, 60, 80)).bold()
        } else {
            Style::default().fg(rgb(140, 180, 255))
        };

        let via_raw = route.map(|r| r.api_method.as_str()).unwrap_or("—");
        let vw = via_width.saturating_sub(1);
        let via_display = format!(" {}", pad_left_display(via_raw, vw));
        let via_style = if unavailable {
            Style::default().fg(rgb(80, 80, 80))
        } else if is_row_selected && col == 2 {
            Style::default().fg(Color::White).bg(rgb(60, 60, 80)).bold()
        } else if is_usage_picker {
            Style::default().fg(rgb(196, 170, 255))
        } else {
            Style::default().fg(rgb(220, 190, 120))
        };

        if is_preview && !is_account_picker {
            spans.push(Span::styled(provider_display, provider_style));
            spans.extend(model_spans);
            spans.push(Span::styled(via_display, via_style));
        } else {
            spans.extend(model_spans);
            spans.push(Span::styled(provider_display, provider_style));
            spans.push(Span::styled(via_display, via_style));
        }

        if let Some(route) = route
            && let Some(detail_text) = route_detail_display_text(&route.detail, unavailable)
            && detail_width > 0
        {
            spans.push(Span::styled(
                format!("  {}", truncate_display(detail_text.as_str(), detail_width)),
                if unavailable {
                    Style::default().fg(rgb(180, 120, 120)).italic()
                } else {
                    Style::default().fg(dim_color())
                },
            ));
        }

        lines.push(Line::from(spans));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_picker() -> crate::tui::InlineInteractiveState {
        crate::tui::InlineInteractiveState {
            kind: crate::tui::PickerKind::Model,
            filtered: vec![0],
            selected: 0,
            column: 0,
            filter: String::new(),
            preview: false,
            entries: vec![crate::tui::PickerEntry {
                name: "gpt-5.4".to_string(),
                options: vec![crate::tui::PickerOption {
                    provider: "openai".to_string(),
                    api_method: "oauth".to_string(),
                    available: true,
                    detail: String::new(),
                    estimated_reference_cost_micros: None,
                }],
                action: crate::tui::PickerAction::Model,
                selected_option: 0,
                is_current: true,
                is_default: false,
                recommended: true,
                recommendation_rank: 0,
                old: false,
                created_date: None,
                effort: None,
            }],
        }
    }

    fn sample_account_picker(mixed_providers: bool) -> crate::tui::InlineInteractiveState {
        let mut models = vec![crate::tui::PickerEntry {
            name: "work".to_string(),
            options: vec![crate::tui::PickerOption {
                provider: "Claude".to_string(),
                api_method: "active".to_string(),
                available: true,
                detail: String::new(),
                estimated_reference_cost_micros: None,
            }],
            action: crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Switch {
                provider_id: "claude".to_string(),
                label: "work".to_string(),
            }),
            selected_option: 0,
            is_current: true,
            is_default: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            old: false,
            created_date: None,
            effort: None,
        }];

        if mixed_providers {
            models.push(crate::tui::PickerEntry {
                name: "personal".to_string(),
                options: vec![crate::tui::PickerOption {
                    provider: "OpenAI".to_string(),
                    api_method: "saved".to_string(),
                    available: true,
                    detail: String::new(),
                    estimated_reference_cost_micros: None,
                }],
                action: crate::tui::PickerAction::Account(
                    crate::tui::AccountPickerAction::Switch {
                        provider_id: "openai".to_string(),
                        label: "personal".to_string(),
                    },
                ),
                selected_option: 0,
                is_current: false,
                is_default: false,
                recommended: false,
                recommendation_rank: usize::MAX,
                old: false,
                created_date: None,
                effort: None,
            });
        }

        crate::tui::InlineInteractiveState {
            kind: crate::tui::PickerKind::Account,
            filtered: (0..models.len()).collect(),
            selected: 0,
            column: 0,
            filter: String::new(),
            preview: false,
            entries: models,
        }
    }

    fn sample_agent_target_picker() -> crate::tui::InlineInteractiveState {
        crate::tui::InlineInteractiveState {
            kind: crate::tui::PickerKind::Model,
            filtered: vec![0],
            selected: 0,
            column: 0,
            filter: String::new(),
            preview: false,
            entries: vec![crate::tui::PickerEntry {
                name: "Swarm / subagent".to_string(),
                options: vec![crate::tui::PickerOption {
                    provider: "gpt-5 default".to_string(),
                    api_method: "agents.swarm_model".to_string(),
                    available: true,
                    detail: "/agents swarm".to_string(),
                    estimated_reference_cost_micros: None,
                }],
                action: crate::tui::PickerAction::AgentTarget(crate::tui::AgentModelTarget::Swarm),
                selected_option: 0,
                is_current: false,
                is_default: false,
                recommended: false,
                recommendation_rank: usize::MAX,
                old: false,
                created_date: None,
                effort: None,
            }],
        }
    }

    #[test]
    fn picker_row_marker_uses_explicit_unavailable_marker() {
        assert_eq!(picker_row_marker(true, true), "×");
        assert_eq!(picker_row_marker(false, true), "×");
        assert_eq!(picker_row_marker(true, false), "▸");
        assert_eq!(picker_row_marker(false, false), " ");
    }

    #[test]
    fn route_detail_display_text_prefixes_unavailable_reason() {
        assert_eq!(
            route_detail_display_text("credentials expired", true).as_deref(),
            Some("unavailable · credentials expired")
        );
        assert_eq!(
            route_detail_display_text("", true).as_deref(),
            Some("unavailable")
        );
        assert_eq!(route_detail_display_text("", false), None);
        assert_eq!(
            route_detail_display_text("catalog still loading", false).as_deref(),
            Some("catalog still loading")
        );
    }

    #[test]
    fn picker_render_width_uses_intrinsic_content_width() {
        let picker = sample_picker();
        let width = picker_render_width(&picker, 120);
        assert!(width < 120, "picker should not expand to full width");
        assert!(width >= 20, "picker should remain wide enough for content");
    }

    #[test]
    fn picker_render_area_centers_in_centered_mode() {
        let picker = sample_picker();
        let width = picker_render_width(&picker, 80) as u16;
        let area = Rect::new(5, 3, 80, 2);
        let horizontal_offset = area.width.saturating_sub(width) / 2;
        let render_area = Rect {
            x: area.x + horizontal_offset,
            y: area.y,
            width,
            height: area.height,
        };

        assert!(render_area.x > area.x, "centered picker should shift right");
        assert_eq!(render_area.width, width);
    }

    #[test]
    fn account_picker_width_uses_compact_two_column_layout() {
        let picker = sample_account_picker(true);
        let width = picker_render_width(&picker, 120);
        assert!(width < 60, "account picker should stay compact");
        assert!(
            width >= 18,
            "account picker should still fit title and state"
        );
    }

    #[test]
    fn account_picker_only_shows_provider_badges_when_needed() {
        let mixed = sample_account_picker(true);
        let single = sample_account_picker(false);

        assert!(account_picker_shows_provider_badge(&mixed));
        assert!(!account_picker_shows_provider_badge(&single));

        let (mixed_title, _) = account_picker_entry_title(&mixed.entries[0], true);
        let (single_title, _) = account_picker_entry_title(&single.entries[0], false);
        assert!(mixed_title.starts_with("Claude · "));
        assert_eq!(single_title, "work");
    }

    #[test]
    fn agent_target_picker_uses_specific_column_labels() {
        let picker = sample_agent_target_picker();

        assert!(picker.is_agent_target_picker());
        assert_eq!(picker.primary_label(), "TARGET");
        assert_eq!(picker.secondary_label(false), "MODEL");
        assert_eq!(picker.tertiary_label(), "CONFIG");
        assert!(!picker.shows_default_shortcut_hint());
    }
}
