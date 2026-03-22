use super::*;

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

pub(super) fn draw_picker_line(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    let picker = match app.picker_state() {
        Some(p) => p,
        None => return,
    };

    let height = area.height as usize;
    let width = area.width as usize;
    if height == 0 {
        return;
    }

    let selected = picker.selected;
    let total = picker.models.len();
    let filtered_count = picker.filtered.len();
    let col = picker.column;
    let is_preview = picker.preview;
    let is_account_picker = picker.kind == crate::tui::PickerKind::Account;

    let col_focus_style = Style::default().fg(Color::White).bold().underlined();
    let col_dim_style = Style::default().fg(dim_color());
    let marker_width = 3usize;

    let mut max_provider_len = 0usize;
    let mut max_via_len = 0usize;
    for &fi in &picker.filtered {
        let entry = &picker.models[fi];
        let route = entry.routes.get(entry.selected_route);
        if let Some(r) = route {
            max_provider_len = max_provider_len.max(r.provider.len());
            max_via_len = max_via_len.max(r.api_method.len());
        }
    }
    max_provider_len = max_provider_len.max(8);
    max_via_len = max_via_len.max(3);

    let provider_width: usize;
    let via_width: usize;
    let model_width: usize;
    if is_preview {
        provider_width = (max_provider_len + 1).min(16);
        via_width = (max_via_len + 1).min(12);
        model_width = width.saturating_sub(marker_width + provider_width + via_width);
    } else {
        via_width = 12;
        provider_width = 20;
        model_width = width.saturating_sub(marker_width + provider_width + via_width);
    }

    let (col_widths, col_labels, col_logical): ([usize; 3], [&str; 3], [usize; 3]) =
        if is_account_picker {
            (
                [model_width, provider_width, via_width],
                ["ACCOUNT", "PROVIDER", "ACTION"],
                [0, 1, 2],
            )
        } else if is_preview {
            (
                [provider_width, model_width, via_width],
                ["PROVIDER", "MODEL", "VIA"],
                [1, 0, 2],
            )
        } else {
            (
                [model_width, provider_width, via_width],
                ["MODEL", "PROVIDER", "VIA"],
                [0, 1, 2],
            )
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

    let third_label = col_labels[2];
    let third_style = if col_logical[2] == col {
        col_focus_style
    } else {
        col_dim_style
    };
    header_spans.push(Span::styled(format!(" {}", third_label), third_style));

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
            if is_account_picker {
                "  ↵ select"
            } else {
                "  ↵ open"
            },
            Style::default().fg(rgb(60, 60, 80)).italic(),
        ));
    } else {
        header_spans.push(Span::styled(
            if is_account_picker {
                "  ↑↓/jk ↵ Esc"
            } else {
                "  ↑↓ ←→ ↵ Esc"
            },
            Style::default().fg(rgb(60, 60, 80)),
        ));
        if !is_account_picker {
            header_spans.push(Span::styled(
                "  ^D=default",
                Style::default().fg(rgb(60, 60, 80)).italic(),
            ));
        }
    }

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(header_spans));

    if picker.filtered.is_empty() {
        lines.push(Line::from(Span::styled(
            "   no matches",
            Style::default().fg(dim_color()).italic(),
        )));
        frame.render_widget(Paragraph::new(lines), area);
        return;
    }

    let list_height = height.saturating_sub(1);
    if list_height == 0 {
        frame.render_widget(Paragraph::new(lines), area);
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
        let entry = &picker.models[model_idx];
        let is_row_selected = vi == selected;
        let route = entry.routes.get(entry.selected_route);

        let marker = if is_row_selected { "▸" } else { " " };

        let mut spans: Vec<Span> = Vec::new();
        spans.push(Span::styled(
            format!(" {} ", marker),
            if is_row_selected {
                Style::default().fg(Color::White).bold()
            } else {
                Style::default().fg(dim_color())
            },
        ));

        let unavailable = route.map(|r| !r.available).unwrap_or(true);
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
        let display_name = format!("{}{}", entry.name, suffix);
        let padded_model: String = {
            let chars: Vec<char> = display_name.chars().collect();
            if chars.len() > model_width {
                chars[..model_width].iter().collect()
            } else if is_preview {
                format!("{:^w$}", display_name, w = model_width)
            } else {
                format!("{:<w$}", display_name, w = model_width)
            }
        };
        let account_action_color = match &entry.selection {
            crate::tui::PickerSelection::Account(crate::tui::AccountPickerSelection::Add {
                ..
            }) => Some(rgb(140, 220, 170)),
            crate::tui::PickerSelection::Account(crate::tui::AccountPickerSelection::Replace {
                ..
            }) => Some(rgb(240, 200, 120)),
            _ => None,
        };
        let model_style = if unavailable {
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

        let match_positions = if !picker.filter.is_empty() {
            let raw = fuzzy_match_positions(&picker.filter, &entry.name);
            if is_preview && !raw.is_empty() {
                let name_len = display_name.chars().count();
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
            vec![Span::styled(padded_model, model_style)]
        } else {
            let model_chars: Vec<char> = padded_model.chars().collect();
            let highlight_style = model_style.underlined();
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
                            model_style
                        },
                    ));
                    run_start = ci;
                    is_match_run = cur_is_match;
                }
            }
            result
        };

        let route_count = entry.routes.len();
        let provider_raw = route.map(|r| r.provider.as_str()).unwrap_or("—");
        let provider_label = if col == 0 && route_count > 1 {
            format!("{} ({})", provider_raw, route_count)
        } else {
            provider_raw.to_string()
        };
        let pw = provider_width.saturating_sub(1);
        let provider_display = {
            let chars: Vec<char> = provider_label.chars().collect();
            if chars.len() > pw {
                let truncated: String = chars[..pw].iter().collect();
                format!(" {:<w$}", truncated, w = pw)
            } else {
                format!(" {:<w$}", provider_label, w = pw)
            }
        };
        let provider_style = if unavailable {
            Style::default().fg(rgb(80, 80, 80))
        } else if is_row_selected && col == 1 {
            Style::default().fg(Color::White).bg(rgb(60, 60, 80)).bold()
        } else {
            Style::default().fg(rgb(140, 180, 255))
        };

        let via_raw = route.map(|r| r.api_method.as_str()).unwrap_or("—");
        let vw = via_width.saturating_sub(1);
        let via_display = {
            let chars: Vec<char> = via_raw.chars().collect();
            if chars.len() > vw {
                let truncated: String = chars[..vw].iter().collect();
                format!(" {:<w$}", truncated, w = vw)
            } else {
                format!(" {:<w$}", via_raw, w = vw)
            }
        };
        let via_style = if unavailable {
            Style::default().fg(rgb(80, 80, 80))
        } else if is_row_selected && col == 2 {
            Style::default().fg(Color::White).bg(rgb(60, 60, 80)).bold()
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

        if let Some(route) = route {
            if !route.detail.is_empty() {
                spans.push(Span::styled(
                    format!("  {}", route.detail),
                    if unavailable {
                        Style::default().fg(rgb(80, 80, 80))
                    } else {
                        Style::default().fg(dim_color())
                    },
                ));
            }
        }

        lines.push(Line::from(spans));
    }

    frame.render_widget(Paragraph::new(lines), area);
}
