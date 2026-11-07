use super::{
    accent_color, ai_color, ai_text, asap_color, clear_area, dim_color, get_grouped_changelog,
    header_icon_color, header_name_color, header_session_color, pending_color, queued_color,
    record_chat_overlay_copy_snapshot, rgb, tool_color, user_bg, user_color, user_text,
};
use crate::tui::TuiState;
use crate::tui::info_widget::WidgetPlacement;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};

use super::selection_highlight::highlight_line_selection;

pub(super) fn draw_changelog_overlay(
    frame: &mut Frame,
    area: Rect,
    scroll: usize,
    app: &dyn TuiState,
) {
    clear_area(frame, area);

    let groups = get_grouped_changelog();
    let mut lines: Vec<Line<'static>> = Vec::new();

    if groups.is_empty() {
        lines.push(Line::from(Span::styled(
            "No changelog entries available.",
            Style::default().fg(dim_color()),
        )));
    } else {
        for group in &groups {
            let heading = match &group.released_at {
                Some(released_at) => format!("  {} · {}", group.version, released_at),
                None => format!("  {}", group.version),
            };
            lines.push(Line::from(Span::styled(
                heading,
                Style::default()
                    .fg(rgb(200, 200, 220))
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            for entry in &group.entries {
                lines.push(Line::from(vec![
                    Span::styled("    • ", Style::default().fg(dim_color())),
                    Span::styled(entry.clone(), Style::default().fg(rgb(170, 170, 185))),
                ]));
            }
            lines.push(Line::from(""));
        }
    }

    let total_lines = lines.len();
    let visible_height = area.height.saturating_sub(2) as usize;
    let max_scroll = total_lines.saturating_sub(visible_height);
    let scroll = scroll.min(max_scroll);

    let scroll_info = if total_lines > visible_height {
        let pct = if max_scroll > 0 {
            (scroll * 100) / max_scroll
        } else {
            100
        };
        format!(" {}% ", pct)
    } else {
        String::new()
    };

    let title = format!(" Changelog {} ", scroll_info);
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(rgb(200, 200, 220))
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::from(Span::styled(
            " Esc to close · drag to select, release to copy · wheel/j/k scroll ",
            Style::default().fg(dim_color()),
        )))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(dim_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let visible_end = scroll
        .saturating_add(inner.height as usize)
        .min(total_lines);

    // Register the rendered lines so the shared copy-selection machinery can map
    // mouse drags to text and highlight + copy the selection, exactly like the
    // chat viewport. Without this, mouse capture would block native terminal
    // selection and there would be no way to copy from the overlay.
    record_chat_overlay_copy_snapshot(&lines, scroll, visible_end, inner);

    let mut visible_lines: Vec<Line<'static>> =
        lines.get(scroll..visible_end).unwrap_or(&[]).to_vec();

    if let Some(range) = app.copy_selection_range().filter(|range| {
        range.start.pane == crate::tui::CopySelectionPane::Chat
            && range.end.pane == crate::tui::CopySelectionPane::Chat
    }) {
        let (start, end) = if (range.start.abs_line, range.start.column)
            <= (range.end.abs_line, range.end.column)
        {
            (range.start, range.end)
        } else {
            (range.end, range.start)
        };
        for abs_idx in start.abs_line.max(scroll)..=end.abs_line.min(visible_end.saturating_sub(1))
        {
            let rel_idx = abs_idx.saturating_sub(scroll);
            if let Some(line) = visible_lines.get_mut(rel_idx) {
                let start_col = if abs_idx == start.abs_line {
                    start.column
                } else {
                    0
                };
                let end_col = if abs_idx == end.abs_line {
                    end.column
                } else {
                    line.width()
                };
                *line = highlight_line_selection(line, start_col, end_col);
            }
        }
    }

    frame.render_widget(Paragraph::new(visible_lines), inner);
}

pub(super) fn draw_commit_log_overlay(
    frame: &mut Frame,
    area: Rect,
    overlay: &crate::tui::app::CommitLogOverlay,
) {
    clear_area(frame, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Commit log: {} ", overlay.title))
        .title_bottom(Line::from(Span::styled(
            " j/k move, d/u page, g/G top/bottom, c copy hash, q/Esc close ",
            Style::default().fg(dim_color()),
        )));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    if let Some(error) = &overlay.error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {error}"),
                Style::default().fg(rgb(240, 110, 110)),
            ))),
            inner,
        );
        return;
    }
    if overlay.entries.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " No commits yet.",
                Style::default().fg(dim_color()),
            ))),
            inner,
        );
        return;
    }

    let height = inner.height as usize;
    let selected = overlay.scroll.min(overlay.entries.len() - 1);
    // Keep the selected row roughly centered once it moves past the middle.
    let start = selected
        .saturating_sub(height / 2)
        .min(overlay.entries.len().saturating_sub(height));

    let hash_style = Style::default().fg(rgb(200, 180, 120));
    let date_style = Style::default().fg(dim_color());
    let author_style = Style::default().fg(rgb(130, 170, 230));
    let refs_style = Style::default()
        .fg(accent_color())
        .add_modifier(Modifier::BOLD);
    let subject_style = Style::default().fg(rgb(220, 220, 230));
    let selected_bg = rgb(45, 50, 65);

    let lines: Vec<Line<'static>> = overlay
        .entries
        .iter()
        .enumerate()
        .skip(start)
        .take(height)
        .map(|(index, entry)| {
            let mut spans = vec![
                Span::styled(format!(" {} ", entry.hash), hash_style),
                Span::styled(format!("{} ", entry.date), date_style),
                Span::styled(format!("{} ", entry.author), author_style),
            ];
            if !entry.refs.is_empty() {
                spans.push(Span::styled(format!("({}) ", entry.refs), refs_style));
            }
            spans.push(Span::styled(entry.subject.clone(), subject_style));
            let line = Line::from(spans);
            if index == selected {
                line.style(Style::default().bg(selected_bg))
            } else {
                line
            }
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

#[derive(Clone, Copy)]
struct CalculatorPalette {
    background: Color,
    text: Color,
    muted: Color,
    border: Color,
    surface: Color,
    function: Color,
    action: Color,
    accent: Color,
    accent_text: Color,
    warning: Color,
}

impl CalculatorPalette {
    fn for_theme(theme: crate::tui::app::CalculatorTheme) -> Self {
        use crate::tui::app::CalculatorTheme;

        match theme {
            CalculatorTheme::Dark => Self {
                background: rgb(18, 22, 31),
                text: rgb(231, 236, 245),
                muted: rgb(137, 151, 170),
                border: rgb(74, 91, 116),
                surface: rgb(37, 46, 61),
                function: rgb(38, 55, 69),
                action: rgb(73, 48, 57),
                accent: rgb(100, 91, 225),
                accent_text: rgb(255, 255, 255),
                warning: rgb(255, 173, 160),
            },
            CalculatorTheme::Light => Self {
                background: rgb(255, 255, 255),
                text: rgb(31, 41, 55),
                muted: rgb(94, 105, 121),
                border: rgb(205, 213, 224),
                surface: rgb(246, 248, 251),
                function: rgb(238, 244, 250),
                action: rgb(255, 237, 237),
                accent: rgb(47, 93, 186),
                accent_text: rgb(255, 255, 255),
                warning: rgb(169, 54, 57),
            },
        }
    }
}

fn draw_calculator_buttons(
    frame: &mut Frame,
    area: Rect,
    labels: &[&str],
    palette: CalculatorPalette,
    selected: Option<usize>,
    function_row: bool,
) {
    if labels.is_empty() || area.height == 0 {
        return;
    }
    let columns = Layout::horizontal(vec![
        Constraint::Ratio(1, labels.len() as u32);
        labels.len()
    ])
    .split(area);

    for (index, (label, cell)) in labels.iter().zip(columns.iter()).enumerate() {
        let is_selected = selected == Some(index);
        let (foreground, background) = if is_selected || *label == "=" {
            (palette.accent_text, palette.accent)
        } else if matches!(*label, "AC" | "⌫") {
            (palette.warning, palette.action)
        } else if function_row {
            (palette.text, palette.function)
        } else {
            (palette.text, palette.surface)
        };
        let button = Rect::new(
            cell.x.saturating_add(1),
            cell.y,
            cell.width.saturating_sub(2),
            cell.height,
        );
        frame.render_widget(
            Paragraph::new(*label)
                .alignment(Alignment::Center)
                .style(Style::default().fg(foreground).bg(background)),
            button,
        );
    }
}

fn draw_calculator_history(
    frame: &mut Frame,
    area: Rect,
    overlay: &crate::tui::app::CalculatorOverlay,
    palette: CalculatorPalette,
) {
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);
    let query = if overlay.history_query.is_empty() {
        "type an expression or result to search"
    } else {
        overlay.history_query.as_str()
    };
    frame.render_widget(
        Paragraph::new(jcode_tui_render::truncate_line_with_ellipsis_to_width(
            &Line::from(vec![
                Span::styled("SEARCH  ", Style::default().fg(palette.accent)),
                Span::styled(query.to_string(), Style::default().fg(palette.text)),
            ]),
            rows[0].width as usize,
        ))
        .style(Style::default().bg(palette.surface)),
        rows[0],
    );

    let matches = overlay.filtered_history_indices();
    let mut lines = Vec::new();
    if matches.is_empty() {
        let message = if overlay.history.is_empty() {
            "No calculations yet. Evaluate an expression to start history."
        } else {
            "No matching calculations."
        };
        lines.push(Line::from(Span::styled(
            message,
            Style::default().fg(palette.muted),
        )));
    } else {
        let visible = rows[2].height as usize;
        let start = overlay
            .history_cursor
            .saturating_sub(visible.saturating_sub(1));
        for (row, index) in matches.iter().skip(start).take(visible).enumerate() {
            let entry = &overlay.history[*index];
            let selected = start + row == overlay.history_cursor;
            let line = Line::from(vec![
                Span::styled(
                    if selected { "› " } else { "  " },
                    Style::default().fg(palette.accent),
                ),
                Span::styled(entry.expression.clone(), Style::default().fg(palette.text)),
                Span::styled("  =  ", Style::default().fg(palette.muted)),
                Span::styled(entry.result.clone(), Style::default().fg(palette.accent)),
            ]);
            lines.push(if selected {
                line.style(Style::default().bg(palette.surface))
            } else {
                line
            });
        }
    }
    let lines = lines
        .iter()
        .map(|line| {
            jcode_tui_render::truncate_line_with_ellipsis_to_width(line, rows[2].width as usize)
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), rows[2]);
    frame.render_widget(
        Paragraph::new(jcode_tui_render::truncate_line_with_ellipsis_to_width(
            &Line::from("Type to search · ↑/↓ select · Enter reuse · F9 back"),
            rows[3].width as usize,
        ))
        .style(Style::default().fg(palette.muted)),
        rows[3],
    );
}

pub(super) fn draw_calculator_overlay(
    frame: &mut Frame,
    area: Rect,
    overlay: &crate::tui::app::CalculatorOverlay,
) {
    let width = area.width.min(78);
    let mode = overlay.mode;
    let expanded = area.height
        >= if mode == crate::tui::app::CalculatorMode::Scientific {
            17
        } else {
            20
        };
    let height = area.height.min(match (expanded, mode) {
        (true, crate::tui::app::CalculatorMode::Scientific) => 17,
        (true, crate::tui::app::CalculatorMode::Programmer) => 20,
        (false, _) => 17,
    });
    if width < 20 || height < 8 {
        return;
    }

    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    clear_area(frame, popup);

    let palette = CalculatorPalette::for_theme(overlay.theme);
    let theme_name = match overlay.theme {
        crate::tui::app::CalculatorTheme::Dark => "DARK",
        crate::tui::app::CalculatorTheme::Light => "WHITE",
    };
    let mode_name = match mode {
        crate::tui::app::CalculatorMode::Scientific => "SCIENTIFIC",
        crate::tui::app::CalculatorMode::Programmer => "PROGRAMMER",
    };
    let title = if overlay.history_visible {
        format!(" ◇ CALC  ·  HISTORY / SEARCH  ·  {theme_name} [M] ")
    } else {
        format!(" ◇ CALC  ·  {mode_name}  ·  {theme_name} [M] ")
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(palette.border))
        .style(Style::default().bg(palette.background))
        .title(jcode_tui_render::truncate_line_with_ellipsis_to_width(
            &Line::from(title),
            width.saturating_sub(2) as usize,
        ));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    if overlay.history_visible {
        draw_calculator_history(frame, inner, overlay, palette);
        return;
    }

    let scientific_rows = [
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ];
    let programmer_rows = [
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ];
    let compact_rows = [
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(6),
        Constraint::Length(1),
    ];
    let rows = match (expanded, mode) {
        (true, crate::tui::app::CalculatorMode::Scientific) => {
            Layout::vertical(scientific_rows).split(inner)
        }
        (true, crate::tui::app::CalculatorMode::Programmer) => {
            Layout::vertical(programmer_rows).split(inner)
        }
        (false, _) => Layout::vertical(compact_rows).split(inner),
    };

    let angle = if overlay.degrees {
        "DEG · d"
    } else {
        "RAD · d"
    };
    let mode_hint = match mode {
        crate::tui::app::CalculatorMode::Scientific => "SCIENTIFIC · Tab: Programmer",
        crate::tui::app::CalculatorMode::Programmer => "PROGRAMMER · Tab: Scientific",
    };
    let header = Line::from(vec![
        Span::styled(mode_hint, Style::default().fg(palette.muted)),
        Span::styled("         ", Style::default()),
        Span::styled(angle, Style::default().fg(palette.muted)),
    ]);
    frame.render_widget(
        Paragraph::new(jcode_tui_render::truncate_line_with_ellipsis_to_width(
            &header,
            rows[0].width as usize,
        )),
        rows[0],
    );

    let expression = if overlay.expression.is_empty() {
        "Try 2^8, sqrt(81), or sin(30)"
    } else {
        &overlay.expression
    };
    let expression_line = Line::from(vec![
        Span::styled("fx  ", Style::default().fg(palette.accent)),
        Span::styled(expression.to_string(), Style::default().fg(palette.text)),
    ]);
    frame.render_widget(
        Paragraph::new(jcode_tui_render::truncate_line_with_ellipsis_to_width(
            &expression_line,
            rows[1].width as usize,
        ))
        .style(Style::default().bg(palette.surface)),
        rows[1],
    );

    let result_text = overlay.formatted_result().unwrap_or_else(|| {
        if overlay.error.is_some() {
            "Error".to_string()
        } else if overlay.expression.is_empty() {
            "0".to_string()
        } else {
            "…".to_string()
        }
    });
    let result_block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(palette.border))
        .title(Span::styled(" RESULT ", Style::default().fg(palette.muted)))
        .style(Style::default().bg(palette.background));
    let result_inner = result_block.inner(rows[2]);
    frame.render_widget(result_block, rows[2]);
    let result_line = Line::from(vec![
        Span::styled("=  ", Style::default().fg(palette.muted)),
        Span::styled(
            result_text,
            Style::default()
                .fg(if overlay.error.is_some() {
                    palette.warning
                } else {
                    palette.accent
                })
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(jcode_tui_render::truncate_line_with_ellipsis_to_width(
            &result_line,
            result_inner.width as usize,
        ))
        .style(Style::default().bg(palette.background)),
        result_inner,
    );

    let conversion = overlay
        .error
        .clone()
        .or_else(|| overlay.conversion_summary());
    if let Some(conversion) = conversion {
        frame.render_widget(
            Paragraph::new(jcode_tui_render::truncate_line_with_ellipsis_to_width(
                &Line::from(conversion),
                rows[3].width as usize,
            ))
            .style(Style::default().fg(if overlay.error.is_some() {
                palette.warning
            } else {
                palette.muted
            })),
            rows[3],
        );
    }

    let bases = [
        ("BIN · F1", crate::tui::app::CalculatorBase::Binary),
        ("OCT · F2", crate::tui::app::CalculatorBase::Octal),
        ("DEC · F3", crate::tui::app::CalculatorBase::Decimal),
        ("HEX · F4", crate::tui::app::CalculatorBase::Hexadecimal),
    ];
    let selected_base = bases.iter().position(|(_, base)| *base == overlay.base);
    let base_labels = bases.map(|(label, _)| label);
    let scientific_keys = [
        "sin · s", "cos · c", "tan · t", "ln · l", "log · g", "√ · r",
    ];
    let number_keys = [
        ["7", "8", "9", "÷ /", "(", "AC"],
        ["4", "5", "6", "× *", ")", "⌫"],
        ["1", "2", "3", "− -", "^", "π · p"],
        ["0", ".", "=", "+", "%", "e"],
    ];
    let programmer_keys = ["&", "|", "xor · y", "NOT · ~", "<<", ">>"];
    if expanded && mode == crate::tui::app::CalculatorMode::Scientific {
        let section_style = Style::default()
            .fg(palette.muted)
            .add_modifier(Modifier::BOLD);
        for (index, label) in [(4, "SCIENTIFIC"), (7, "NUMBER KEYPAD")] {
            frame.render_widget(Paragraph::new(label).style(section_style), rows[index]);
        }
        draw_calculator_buttons(frame, rows[5], &scientific_keys, palette, None, true);
        for (labels, row_index) in number_keys.iter().zip(8..12) {
            draw_calculator_buttons(frame, rows[row_index], labels, palette, None, false);
        }
    } else if expanded {
        let section_style = Style::default()
            .fg(palette.muted)
            .add_modifier(Modifier::BOLD);
        for (index, label) in [(4, "BASES"), (7, "NUMBER KEYPAD"), (13, "PROGRAMMER")] {
            frame.render_widget(Paragraph::new(label).style(section_style), rows[index]);
        }
        draw_calculator_buttons(frame, rows[5], &base_labels, palette, selected_base, true);
        for (labels, row_index) in number_keys.iter().zip(8..12) {
            draw_calculator_buttons(frame, rows[row_index], labels, palette, None, false);
        }
        draw_calculator_buttons(frame, rows[14], &programmer_keys, palette, None, true);
    } else if mode == crate::tui::app::CalculatorMode::Scientific {
        draw_calculator_buttons(frame, rows[4], &scientific_keys, palette, None, true);
        let keypad_rows = Layout::vertical([Constraint::Length(1); 6]).split(rows[5]);
        for (labels, row_area) in number_keys.iter().zip(keypad_rows.iter()) {
            draw_calculator_buttons(frame, *row_area, labels, palette, None, false);
        }
    } else {
        draw_calculator_buttons(frame, rows[4], &base_labels, palette, selected_base, true);
        let keypad_rows = Layout::vertical([Constraint::Length(1); 6]).split(rows[5]);
        for (labels, row_area) in number_keys.iter().zip(keypad_rows.iter()) {
            draw_calculator_buttons(frame, *row_area, labels, palette, None, false);
        }
        draw_calculator_buttons(frame, keypad_rows[5], &programmer_keys, palette, None, true);
    }

    let footer = match (width < 50, mode) {
        (true, crate::tui::app::CalculatorMode::Scientific) => {
            "F9 history · Tab programmer · s/c/t trig"
        }
        (true, crate::tui::app::CalculatorMode::Programmer) => {
            "F9 history · Tab scientific · F1-F4 bases"
        }
        (false, crate::tui::app::CalculatorMode::Scientific) => {
            "F9 history · Tab programmer · s/c/t trig · m theme · Esc close"
        }
        (false, crate::tui::app::CalculatorMode::Programmer) => {
            "F9 history · Tab scientific · F1-F4 bases · m theme · Esc close"
        }
    };
    let footer_row = match (expanded, mode) {
        (true, crate::tui::app::CalculatorMode::Scientific) => 12,
        (true, crate::tui::app::CalculatorMode::Programmer) => 15,
        (false, _) => 6,
    };
    frame.render_widget(
        Paragraph::new(jcode_tui_render::truncate_line_with_ellipsis_to_width(
            &Line::from(footer),
            rows[footer_row].width as usize,
        ))
        .style(Style::default().fg(palette.muted)),
        rows[footer_row],
    );
}

pub(super) fn draw_help_overlay(frame: &mut Frame, area: Rect, scroll: usize, app: &dyn TuiState) {
    clear_area(frame, area);

    let section_style = Style::default()
        .fg(accent_color())
        .add_modifier(Modifier::BOLD);
    let cmd_style = Style::default().fg(rgb(230, 230, 240));
    let desc_style = Style::default().fg(rgb(150, 150, 165));
    let key_style = Style::default().fg(rgb(200, 180, 120));
    let sep_style = Style::default().fg(rgb(50, 50, 55));

    let mut lines: Vec<Line<'static>> = Vec::new();

    let separator = || -> Line<'static> {
        Line::from(Span::styled(
            "  ─────────────────────────────────────────────────",
            sep_style,
        ))
    };

    let help_entry = |cmd: &str, desc: &str| -> Line<'static> {
        Line::from(vec![
            Span::styled("    ", Style::default()),
            Span::styled(cmd.to_string(), cmd_style),
            Span::styled("  ", Style::default()),
            Span::styled(desc.to_string(), desc_style),
        ])
    };

    let alt = jcode_tui_core::keybind::alt_chord;
    let key_entry = |key: &str, desc: &str| -> Line<'static> {
        Line::from(vec![
            Span::styled("    ", Style::default()),
            Span::styled(format!("{:<22}", key), key_style),
            Span::styled(desc.to_string(), desc_style),
        ])
    };

    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled("  Commands", section_style)));
    lines.push(Line::from(""));
    lines.push(help_entry("/help", "Show this help overlay"));
    lines.push(help_entry(
        "/help <command>",
        "Show details for one command",
    ));
    lines.push(help_entry("/model", "List or switch models"));
    lines.push(help_entry("/model <name>", "Switch to a different model"));
    lines.push(help_entry(
        "/provider-test-coverage",
        "Show live-test evidence for the current provider/model",
    ));
    lines.push(help_entry("/agents", "Configure models for agent roles"));
    lines.push(help_entry(
        "/swarm-prompt",
        "Open the active swarm routing prompt in your editor",
    ));
    lines.push(help_entry(
        "/effort <level>",
        "Set effort (none|minimal|low|medium|high|xhigh|max|swarm)",
    ));
    lines.push(help_entry(
        "/fast [on|off|status|default ...]",
        "Toggle fast mode",
    ));
    lines.push(help_entry(
        "/transport <mode>",
        "Set connection transport (auto|https|websocket)",
    ));
    lines.push(help_entry(
        "/alignment [status|centered|left]",
        "Show or persist text alignment preference",
    ));
    lines.push(help_entry(
        "/compact-notifications [status|on|off]",
        "Collapse swarm/file-activity notifications to one line",
    ));
    lines.push(help_entry(
        "/show-agentgrep-output [status|on|off]",
        "Render full agentgrep search output inline in chat",
    ));
    lines.push(help_entry("/config", "Show active configuration"));
    lines.push(help_entry("/config init", "Create default config file"));
    lines.push(help_entry("/config edit", "Open config in $EDITOR"));
    lines.push(help_entry(
        "/voice",
        "Voice input: speak, then send (Ctrl+Space)",
    ));
    lines.push(help_entry("/dictate", "Run configured external dictation"));
    lines.push(help_entry(
        "/git [status]",
        "Show branch and working tree status for the repo",
    ));
    lines.push(help_entry(
        "/context",
        "Show the full session context snapshot",
    ));
    lines.push(help_entry(
        "/skills",
        "Show loaded skills and jcode-endorsed recommendations",
    ));
    lines.push(help_entry("/info", "Show session info and token usage"));
    lines.push(help_entry(
        "/keys",
        "Show keybinding conflicts with your terminal/OS",
    ));
    lines.push(help_entry(
        "/reset usage limits openai",
        "Review a banked reset, then confirm or cancel",
    ));
    lines.push(help_entry(
        "/reset usage limits claude",
        "Review the Claude session-limit reset, then confirm or cancel",
    ));
    lines.push(help_entry("/usage", "Show connected provider usage limits"));
    lines.push(help_entry(
        "/support",
        "Email support with diagnostics prefilled",
    ));
    lines.push(help_entry("/version", "Show version and build details"));
    lines.push(help_entry(
        "/changelog",
        "Show recent changes in this build",
    ));

    lines.push(Line::from(""));
    lines.push(separator());
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled("  Session", section_style)));
    lines.push(Line::from(""));
    lines.push(help_entry("/clear", "Clear conversation and start fresh"));
    lines.push(help_entry(
        "/compact",
        "Summarize old messages to free context",
    ));
    lines.push(help_entry(
        "/rewind",
        "Show numbered history, /rewind N to rewind",
    ));
    lines.push(help_entry(
        "/fix",
        "Attempt recovery when model cannot continue",
    ));
    lines.push(help_entry(
        "/poke",
        "Poke model to resume with incomplete todos (on/off/status)",
    ));
    lines.push(help_entry(
        "/plan [goal]",
        "Draft a plan-only proposal as a plan card (no edits)",
    ));
    lines.push(help_entry(
        "/improve",
        "Autonomously improve the repo until returns diminish",
    ));
    lines.push(help_entry(
        "/improve resume",
        "Resume the last saved improve loop/plan",
    ));
    lines.push(help_entry(
        "/refactor",
        "Run a safe refactor loop with independent review",
    ));
    lines.push(help_entry(
        "/refactor resume",
        "Resume the last saved refactor loop/plan",
    ));
    lines.push(help_entry(
        "/splitview [on|off|status]",
        "Mirror the current chat in the side panel",
    ));
    lines.push(help_entry(
        "/fork [prompt]",
        "Fork session into a new window (alias: /split)",
    ));
    lines.push(help_entry(
        "/transfer",
        "Open a fresh session with only compacted context + copied todos",
    ));
    lines.push(help_entry(
        "/workspace [status|on|off|add]",
        "Enable and manage the Niri-style session workspace",
    ));
    lines.push(help_entry(
        "/catchup [next|list]",
        "Jump to finished sessions and open a Catch Up brief",
    ));
    lines.push(help_entry(
        "/back",
        "Return to the previous Catch Up source session",
    ));
    lines.push(help_entry("/resume", "Browse and resume previous sessions"));
    lines.push(help_entry(
        "/active",
        "Manage live sessions: see which are working vs ready",
    ));
    lines.push(help_entry(
        "/catchup [next]",
        "Jump into finished sessions with a side-panel brief",
    ));
    lines.push(help_entry(
        "/back",
        "Return to the previous Catch Up session",
    ));
    lines.push(help_entry("/save [label]", "Bookmark session for /resume"));
    lines.push(help_entry(
        "/rename <name>|--clear",
        "Set or clear current session name",
    ));
    lines.push(help_entry(
        "/unsave",
        "Remove bookmark from current session",
    ));

    lines.push(Line::from(""));
    lines.push(separator());
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled("  Memory & Swarm", section_style)));
    lines.push(Line::from(""));
    lines.push(help_entry("/memory [on|off]", "Toggle memory features"));
    lines.push(help_entry(
        "/test [claim]",
        "Run layered verification and produce proof",
    ));
    lines.push(help_entry(
        "/initiatives",
        "Open initiatives overview / resume an initiative",
    ));
    lines.push(help_entry("/swarm [on|off]", "Toggle swarm features"));

    lines.push(Line::from(""));
    lines.push(separator());
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled("  Auth & Accounts", section_style)));
    lines.push(Line::from(""));
    lines.push(help_entry("/auth", "Show authentication status"));
    lines.push(help_entry(
        "/login [provider]",
        "Interactive or direct login",
    ));
    lines.push(help_entry(
        "/account",
        "Open combined Claude/OpenAI account picker",
    ));
    lines.push(help_entry(
        "/subscription",
        "Inspect jcode subscription scaffold",
    ));
    lines.push(help_entry(
        "/subscribe",
        "Why and how to subscribe to jcode",
    ));

    lines.push(Line::from(""));
    lines.push(separator());
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled("  System", section_style)));
    lines.push(Line::from(""));
    lines.push(help_entry("/reload", "Reload to newer binary if available"));
    lines.push(help_entry(
        "/restart",
        "Restart with current binary (no build)",
    ));
    lines.push(help_entry(
        "/rebuild",
        "Full update (git pull + build + tests)",
    ));
    if app.is_remote_mode() {
        lines.push(help_entry("/client-reload", "Force reload client binary"));
        lines.push(help_entry("/server-reload", "Force reload server binary"));
        lines.push(help_entry(
            "/continue",
            "Continue every interrupted live session that would auto-resume",
        ));
    }
    lines.push(help_entry(
        "/debug-visual",
        "Enable visual debugging for TUI issues",
    ));
    lines.push(help_entry("/quit", "Exit jcode"));

    // The sections above are hand-curated for ordering, but they drift as
    // commands are added. Anything registered and not already shown gets listed
    // here so no working command is invisible in /help.
    let shown: std::collections::HashSet<String> = lines
        .iter()
        .filter_map(|line| line.spans.get(1).map(|span| span.content.to_string()))
        .map(|cmd| {
            cmd.split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let uncovered: Vec<(&str, &str)> = crate::tui::app::registered_command_entries()
        .filter(|(name, _)| !shown.contains(*name))
        .collect();
    if !uncovered.is_empty() {
        lines.push(Line::from(""));
        lines.push(separator());
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("  More commands", section_style)));
        lines.push(Line::from(""));
        for (name, desc) in uncovered {
            lines.push(help_entry(name, desc));
        }
    }

    let skills = app.available_skills();
    if !skills.is_empty() {
        lines.push(Line::from(""));
        lines.push(separator());
        lines.push(Line::from(""));

        lines.push(Line::from(Span::styled("  Skills", section_style)));
        lines.push(Line::from(""));
        for skill in &skills {
            lines.push(help_entry(&format!("/{}", skill), "Activate skill"));
        }
    }

    lines.push(Line::from(""));
    lines.push(separator());
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled("  Navigation", section_style)));
    lines.push(Line::from(""));
    lines.push(key_entry("PageUp / PageDown", "Scroll history"));
    lines.push(key_entry("Up / Down", "Scroll history (when input empty)"));
    lines.push(key_entry(
        "Ctrl+J / Ctrl+K",
        "Jump to next / previous user prompt (also Ctrl+] / Ctrl+[)",
    ));
    lines.push(key_entry(
        "Ctrl+Shift+J / Ctrl+Shift+K",
        "Scroll history down / up one line",
    ));
    lines.push(key_entry(
        "Cmd/Super+K / J",
        "Jump to previous / next user prompt (macOS, if forwarded)",
    ));
    lines.push(key_entry("Ctrl+1..4", "Resize side panel to 25/50/75/100%"));
    lines.push(key_entry(
        "Ctrl+5..9",
        "Jump by recency (5 = 5th most recent)",
    ));

    lines.push(Line::from(""));
    lines.push(separator());
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled(
        "  Diagrams & Diffs",
        section_style,
    )));
    lines.push(Line::from(""));
    lines.push(key_entry(
        &crate::tui::keybind::side_panel_toggle_key_label(),
        "Cycle side panel: split, fullscreen, hidden",
    ));
    lines.push(key_entry(
        &crate::tui::keybind::diagram_pane_visibility_key_label(),
        "Show/hide diagram pane",
    ));
    lines.push(key_entry(&alt("T"), "Toggle diagram position (side/top)"));
    lines.push(key_entry(
        &alt("Shift+I"),
        "Show/hide inline images (persists)",
    ));
    lines.push(key_entry("Ctrl+H / Ctrl+L", "Focus chat / diagram / diffs"));
    lines.push(key_entry(
        "Ctrl+L / Cmd+L",
        "Clear screen, history stays above (no pane focused)",
    ));
    lines.push(key_entry(
        "Ctrl+Left / Right",
        "Cycle diagrams (when diagram focused)",
    ));
    lines.push(key_entry("h/j/k/l / arrows", "Pan diagram (when focused)"));
    lines.push(key_entry("[ / ]", "Zoom diagram (when focused)"));
    lines.push(key_entry("+ / -", "Resize diagram pane"));
    lines.push(key_entry(
        "/diff",
        "Cycle diff mode (Off/Inline/Pinned/File)",
    ));
    lines.push(key_entry(&alt("G"), "Show the project's git commit log"));
    lines.push(key_entry("Shift+Tab", "Cycle favorited models"));
    lines.push(key_entry("Ctrl+O", "Set default model (in /model picker)"));
    lines.push(key_entry(
        "Ctrl+N",
        "Toggle favorite model (in /model picker)",
    ));

    lines.push(Line::from(""));
    lines.push(separator());
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled("  Input & Editing", section_style)));
    lines.push(Line::from(""));
    lines.push(key_entry(
        "Ctrl+C / Ctrl+D",
        "Quit (press twice to confirm)",
    ));
    lines.push(key_entry("Ctrl+X", "Cut entire input line to clipboard"));
    lines.push(key_entry(
        "Ctrl+A",
        "Copy visible chat viewport plus nearby context",
    ));
    lines.push(key_entry("Ctrl+U", "Clear input line"));
    lines.push(key_entry("Ctrl+K", "Delete to end of input"));
    lines.push(key_entry(
        &format!("{} / {}", alt("Backspace"), alt("Delete")),
        "Delete previous word in input",
    ));
    lines.push(key_entry(
        "Cmd/Super+Backspace / Delete",
        "Delete previous word in input",
    ));
    if cfg!(target_os = "macos") {
        // On macOS, Cmd+Left/Right default to effort cycling; Home/End and
        // Cmd+A/E still jump to the start/end of the input.
        lines.push(key_entry("Home / End", "Move to start / end of input"));
    } else {
        lines.push(key_entry(
            "Cmd/Super+Left / Right",
            "Move to start / end of input",
        ));
    }
    lines.push(key_entry("Cmd/Super+Z", "Undo input edit"));
    lines.push(key_entry("Cmd/Super+X / V", "Cut input / paste clipboard"));
    lines.push(key_entry("Ctrl+S", "Stash / pop input (save for later)"));
    lines.push(key_entry("Ctrl+Backspace", "Delete previous word in input"));
    lines.push(key_entry("Ctrl+B / Ctrl+F", "Move by word left / right"));
    lines.push(key_entry("Ctrl+Left / Right", "Move by word left / right"));
    lines.push(key_entry(
        &format!("Shift+Enter / {}", alt("Enter")),
        "Insert newline in input",
    ));
    lines.push(key_entry(
        "Trailing \\ then Enter",
        "Insert newline (fallback; run /terminal-setup to fix Shift+Enter)",
    ));
    lines.push(key_entry(
        "Ctrl+Enter / Cmd+Enter",
        "Use opposite send mode while processing",
    ));
    lines.push(key_entry("Ctrl+Up", "Retrieve pending message for editing"));
    lines.push(key_entry("Ctrl+Tab / Ctrl+T", "Toggle queue mode"));
    lines.push(key_entry(
        "Ctrl+R",
        "Search prompt history (across sessions)",
    ));
    lines.push(key_entry(
        &format!("Ctrl+V / {}", alt("V")),
        "Paste clipboard (text or image)",
    ));
    lines.push(key_entry(
        &alt("A"),
        "Quick-copy visible chat viewport plus nearby context",
    ));
    lines.push(key_entry(&alt("Y"), "Toggle chat selection/copy mode"));
    lines.push(key_entry(&alt("S"), "Toggle typing scroll lock"));
    lines.push(key_entry("Ctrl+P", "Toggle auto-poke for incomplete todos"));
    lines.push(key_entry(&alt("X"), "Show/dismiss todo list card in chat"));
    lines.push(key_entry(
        &crate::tui::keybind::effort_switch_keys_label(),
        "Cycle effort (reasoning + swarm)",
    ));
    if cfg!(target_os = "macos") {
        lines.push(key_entry(
            &alt("Left / Right"),
            &format!("Move by word in input (also {} / {})", alt("B"), alt("F")),
        ));
    }
    if let Some(label) = app.dictation_key_label() {
        lines.push(key_entry(&label, "Run configured dictation"));
    }
    if let Some(label) = crate::tui::keybind::load_open_resume_key().label {
        lines.push(key_entry(&label, "Open the /resume session picker"));
    }
    if let Some(label) = crate::tui::keybind::load_new_terminal_key().label {
        lines.push(key_entry(
            &label,
            "Spawn new jcode session in a new terminal",
        ));
    }

    lines.push(Line::from(""));

    let total_lines = lines.len();
    let visible_height = area.height.saturating_sub(2) as usize;
    let max_scroll = total_lines.saturating_sub(visible_height);
    let scroll = scroll.min(max_scroll);

    let scroll_info = if total_lines > visible_height {
        let pct = if max_scroll > 0 {
            (scroll * 100) / max_scroll
        } else {
            100
        };
        format!(" {}% ", pct)
    } else {
        String::new()
    };

    let title = format!(" Help {} ", scroll_info);
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(rgb(200, 200, 220))
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::from(Span::styled(
            " Esc to close · mouse wheel/j/k scroll · Space/PageUp page · /help <cmd> for details ",
            Style::default().fg(dim_color()),
        )))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(dim_color()));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .scroll((scroll as u16, 0));

    frame.render_widget(paragraph, area);
}

pub(super) fn draw_model_status_overlay(
    frame: &mut Frame,
    area: Rect,
    scroll: usize,
    content: &str,
) {
    clear_area(frame, area);

    let title_style = Style::default()
        .fg(accent_color())
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(rgb(210, 210, 220));
    let dim_style = Style::default().fg(dim_color());

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(Span::styled("  Model Status", title_style)));
    lines.push(Line::from(Span::styled(
        "  Live verification evidence for provider/model behavior in jcode",
        dim_style,
    )));
    lines.push(Line::from(""));

    for raw in content.lines() {
        if let Some(title) = raw.strip_prefix("# ") {
            lines.push(Line::from(Span::styled(format!("  {title}"), title_style)));
        } else if let Some(title) = raw.strip_prefix("## ") {
            lines.push(Line::from(Span::styled(format!("  {title}"), title_style)));
        } else if raw.trim().is_empty() {
            lines.push(Line::from(""));
        } else {
            lines.push(Line::from(Span::styled(
                format!("  {raw}"),
                model_status_line_style(raw, text_style),
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  ↑/↓ scroll, PgUp/PgDn page, c copy report, q/Esc close",
        dim_style,
    )));

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" /provider-test-coverage "),
        )
        .scroll((scroll.min(u16::MAX as usize) as u16, 0));
    frame.render_widget(paragraph, area);
}

fn model_status_line_style(raw: &str, default: Style) -> Style {
    // Reuse the same semantic classification the CLI uses so the TUI overlay
    // and `jcode provider-test-coverage` stay color-consistent.
    use crate::live_tests::CoverageLineStyle;
    match crate::live_tests::classify_provider_test_coverage_line(raw) {
        CoverageLineStyle::Title => Style::default()
            .fg(accent_color())
            .add_modifier(Modifier::BOLD),
        CoverageLineStyle::Pass => Style::default().fg(rgb(120, 220, 150)),
        CoverageLineStyle::Fail => Style::default().fg(rgb(240, 110, 110)),
        CoverageLineStyle::Warn => Style::default().fg(rgb(235, 190, 105)),
        CoverageLineStyle::Dim => Style::default().fg(dim_color()),
        CoverageLineStyle::Plain => default,
    }
}

pub(super) fn draw_debug_overlay(
    frame: &mut Frame,
    placements: &[WidgetPlacement],
    chunks: &[Rect],
) {
    if chunks.len() < 5 {
        return;
    }
    render_overlay_box(frame, chunks[0], "messages", Color::Red);
    render_overlay_box(frame, chunks[1], "queued", Color::Yellow);
    render_overlay_box(frame, chunks[2], "status", Color::Cyan);
    render_overlay_box(frame, chunks[3], "picker", Color::Magenta);
    render_overlay_box(frame, chunks[4], "input", Color::Green);
    if chunks.len() > 5 && chunks[5].height > 0 {
        render_overlay_box(frame, chunks[5], "donut", Color::Blue);
    }

    for placement in placements {
        let title = format!("widget:{}", placement.kind.as_str());
        render_overlay_box(frame, placement.rect, &title, Color::Magenta);
    }
}

fn render_overlay_box(frame: &mut Frame, area: Rect, title: &str, color: Color) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color))
        .title(Span::styled(title.to_string(), Style::default().fg(color)));
    frame.render_widget(block, area);
}

pub(super) fn debug_palette_json() -> Option<serde_json::Value> {
    Some(serde_json::json!({
        "user_color": color_to_rgb(user_color()),
        "ai_color": color_to_rgb(ai_color()),
        "tool_color": color_to_rgb(tool_color()),
        "dim_color": color_to_rgb(dim_color()),
        "accent_color": color_to_rgb(accent_color()),
        "queued_color": color_to_rgb(queued_color()),
        "asap_color": color_to_rgb(asap_color()),
        "pending_color": color_to_rgb(pending_color()),
        "user_text": color_to_rgb(user_text()),
        "user_bg": color_to_rgb(user_bg()),
        "ai_text": color_to_rgb(ai_text()),
        "header_icon_color": color_to_rgb(header_icon_color()),
        "header_name_color": color_to_rgb(header_name_color()),
        "header_session_color": color_to_rgb(header_session_color()),
    }))
}

fn color_to_rgb(color: Color) -> Option<[u8; 3]> {
    match color {
        Color::Rgb(r, g, b) => Some([r, g, b]),
        Color::Indexed(n) if n >= 16 => {
            let (r, g, b) = crate::tui::color_support::indexed_to_rgb(n);
            Some([r, g, b])
        }
        _ => None,
    }
}
