use super::*;
use ratatui::widgets::Wrap;

impl SessionPicker {
    pub(super) fn crash_reason_line(session: &SessionInfo) -> Option<Line<'static>> {
        let reason = match &session.status {
            SessionStatus::Crashed { message } => message
                .as_deref()
                .unwrap_or("Unexpected termination (no additional details)"),
            _ => return None,
        };

        let reason_display = if reason.chars().count() > 54 {
            format!("{}...", safe_truncate(reason, 51))
        } else {
            reason.to_string()
        };

        Some(Line::from(vec![
            Span::styled("     ", Style::default()),
            Span::styled(
                format!("reason: {}", reason_display),
                Style::default().fg(rgb(220, 120, 120)),
            ),
        ]))
    }

    fn render_session_item(&self, session: &SessionInfo, is_selected: bool) -> ListItem<'static> {
        let dim: Color = rgb(100, 100, 100);
        let dimmer: Color = rgb(70, 70, 70);
        let user_clr: Color = rgb(138, 180, 248);
        let accent: Color = rgb(186, 139, 255);
        let batch_restore: Color = rgb(255, 140, 140);
        let batch_row_bg: Color = rgb(36, 18, 18);

        let created_ago = format_time_ago(session.created_at);
        let in_batch_restore = self.crashed_session_ids.contains(&session.id);
        let is_marked = self.selected_session_ids.contains(&session.id);

        let name_style = if is_selected {
            Style::default()
                .fg(rgb(140, 220, 160))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let canary_marker = if session.is_canary { " 🔬" } else { "" };
        let debug_marker = if session.is_debug { " 🧪" } else { "" };
        let saved_marker = if session.saved { " 📌" } else { "" };
        let selection_marker = if is_marked { "[x] " } else { "[ ] " };
        let selection_style = if is_marked {
            Style::default()
                .fg(rgb(140, 220, 160))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(rgb(90, 90, 90))
        };

        let time_ago = format_time_ago(session.last_message_time);
        let (status_icon, status_color, time_label) = match &session.status {
            SessionStatus::Active => ("▶", rgb(100, 200, 100), "active".to_string()),
            SessionStatus::Closed => ("✓", dim, format!("closed {}", time_ago)),
            SessionStatus::Crashed { .. } => {
                ("💥", rgb(220, 100, 100), format!("crashed {}", time_ago))
            }
            SessionStatus::Reloaded => ("🔄", user_clr, format!("reloaded {}", time_ago)),
            SessionStatus::Compacted => ("📦", rgb(255, 193, 7), format!("compacted {}", time_ago)),
            SessionStatus::RateLimited => ("⏳", accent, format!("rate-limited {}", time_ago)),
            SessionStatus::Error { .. } => {
                ("❌", rgb(220, 100, 100), format!("errored {}", time_ago))
            }
        };

        let mut line1_spans = vec![
            Span::styled(selection_marker, selection_style),
            Span::styled(
                format!("{} ", session.icon),
                Style::default().fg(rgb(110, 210, 255)),
            ),
            Span::styled(session.short_name.clone(), name_style),
            Span::styled(canary_marker, Style::default().fg(rgb(255, 193, 7))),
            Span::styled(debug_marker, Style::default().fg(rgb(180, 180, 180))),
            Span::styled(saved_marker, Style::default().fg(rgb(255, 180, 100))),
            Span::styled(
                format!(" {}", status_icon),
                Style::default().fg(status_color),
            ),
            Span::styled(format!("  {}", time_label), Style::default().fg(dim)),
        ];
        if let Some(ref label) = session.save_label {
            line1_spans.push(Span::styled(
                format!("  \"{}\"", label),
                Style::default().fg(rgb(255, 200, 140)),
            ));
        }
        if let Some(source_badge) = session.source.badge() {
            line1_spans.push(Span::styled(
                format!("  {}", source_badge),
                Style::default()
                    .fg(rgb(120, 210, 255))
                    .add_modifier(Modifier::BOLD),
            ));
        }
        if in_batch_restore {
            line1_spans.push(Span::styled(
                "  [BATCH]",
                Style::default()
                    .fg(batch_restore)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        let line1 = Line::from(line1_spans);

        let title_display = if session.title.chars().count() > 42 {
            format!("{}...", safe_truncate(&session.title, 39))
        } else {
            session.title.clone()
        };
        let line2 = Line::from(vec![
            Span::styled("     ", Style::default()),
            Span::styled(title_display, Style::default().fg(rgb(180, 180, 180))),
        ]);

        let tokens_display = if session.estimated_tokens >= 1000 {
            format!("~{}k tok", session.estimated_tokens / 1000)
        } else {
            format!("~{} tok", session.estimated_tokens)
        };
        let line3 = Line::from(vec![
            Span::styled("     ", Style::default()),
            Span::styled(
                format!("{}", session.user_message_count),
                Style::default().fg(user_clr),
            ),
            Span::styled(" user", Style::default().fg(dimmer)),
            Span::styled(" · ", Style::default().fg(dimmer)),
            Span::styled(
                format!("{}", session.assistant_message_count),
                Style::default().fg(rgb(129, 199, 132)),
            ),
            Span::styled(" assistant", Style::default().fg(dimmer)),
            Span::styled(" · ", Style::default().fg(dimmer)),
            Span::styled(tokens_display, Style::default().fg(dimmer)),
        ]);

        let dir_part = if let Some(ref dir) = session.working_dir {
            let dir_display = if dir.chars().count() > 28 {
                let chars: Vec<char> = dir.chars().collect();
                let suffix: String = chars
                    .iter()
                    .rev()
                    .take(25)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                format!("...{}", suffix)
            } else {
                dir.clone()
            };
            format!("  📁 {}", dir_display)
        } else {
            String::new()
        };
        let line4 = Line::from(vec![
            Span::styled("     ", Style::default()),
            Span::styled(
                format!("created: {}", created_ago),
                Style::default().fg(dimmer),
            ),
            Span::styled(dir_part, Style::default().fg(dimmer)),
        ]);

        let mut rows = vec![line1, line2, line3, line4];
        if let Some(reason_line) = Self::crash_reason_line(session) {
            rows.push(reason_line);
        }
        rows.push(Line::from(""));

        let mut item = ListItem::new(rows);
        if in_batch_restore && !is_selected {
            item = item.style(Style::default().bg(batch_row_bg));
        }
        item
    }

    pub(super) fn render_session_list(&mut self, frame: &mut Frame, area: Rect) {
        let server_color: Color = rgb(255, 200, 100);
        let dim: Color = rgb(100, 100, 100);

        let items: Vec<ListItem> = self
            .items
            .iter()
            .enumerate()
            .map(|(idx, item)| {
                let is_selected = self.list_state.selected() == Some(idx);

                match item {
                    PickerItem::ServerHeader {
                        name,
                        icon,
                        version,
                        session_count,
                    } => {
                        let line1 = Line::from(vec![
                            Span::styled(format!("{} ", icon), Style::default().fg(server_color)),
                            Span::styled(
                                name.clone(),
                                Style::default()
                                    .fg(server_color)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("  {} · {} sessions", version, session_count),
                                Style::default().fg(dim),
                            ),
                        ]);
                        ListItem::new(vec![line1])
                    }
                    PickerItem::OrphanHeader { session_count } => {
                        let line1 = Line::from(vec![
                            Span::styled("📦 ", Style::default().fg(dim)),
                            Span::styled(
                                "Other sessions",
                                Style::default().fg(dim).add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("  {} sessions", session_count),
                                Style::default().fg(dim),
                            ),
                        ]);
                        ListItem::new(vec![line1])
                    }
                    PickerItem::SavedHeader { session_count } => {
                        let saved_color: Color = rgb(255, 180, 100);
                        let line1 = Line::from(vec![
                            Span::styled("📌 ", Style::default().fg(saved_color)),
                            Span::styled(
                                "Saved",
                                Style::default()
                                    .fg(saved_color)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(format!("  {}", session_count), Style::default().fg(dim)),
                        ]);
                        ListItem::new(vec![line1])
                    }
                    PickerItem::Session => self
                        .item_to_session
                        .get(idx)
                        .and_then(|session_idx| {
                            session_idx
                                .and_then(|i| self.visible_sessions.get(i).copied())
                                .and_then(|session_ref| self.session_by_ref(session_ref))
                        })
                        .map(|session| self.render_session_item(session, is_selected))
                        .unwrap_or_else(|| ListItem::new(Line::from(""))),
                }
            })
            .collect();

        let mut title_parts: Vec<Span> = Vec::new();
        title_parts.push(Span::styled(
            format!(" {} ", self.visible_sessions.len()),
            Style::default()
                .fg(rgb(200, 200, 200))
                .add_modifier(Modifier::BOLD),
        ));
        title_parts.push(Span::styled(
            "sessions",
            Style::default().fg(rgb(120, 120, 120)),
        ));

        if let Some(label) = self.filter_mode.label() {
            title_parts.push(Span::styled(
                format!("  {}", label),
                Style::default().fg(rgb(255, 180, 100)),
            ));
        }

        if self.hidden_test_count > 0 {
            title_parts.push(Span::styled(
                format!(" (+{} hidden)", self.hidden_test_count),
                Style::default().fg(rgb(80, 80, 80)),
            ));
        }

        if !self.search_query.is_empty() {
            title_parts.push(Span::styled(
                format!("  🔍 \"{}\"", self.search_query),
                Style::default().fg(rgb(186, 139, 255)),
            ));
        }

        if self.selection_count() > 0 {
            title_parts.push(Span::styled(
                format!("  ✓ {} selected", self.selection_count()),
                Style::default().fg(rgb(140, 220, 160)),
            ));
        }

        title_parts.push(Span::styled(" ", Style::default()));

        let help = if self.search_active {
            " type to filter, Esc cancel "
        } else {
            " Space select · Enter resume · s next filter · S prev · d debug · / search · h/l focus · ↑↓ · q "
        };

        let border_dim: Color = rgb(70, 70, 70);
        let border_focus: Color = rgb(130, 130, 160);
        let border_color = if self.focus == PaneFocus::Sessions {
            border_focus
        } else {
            border_dim
        };

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(Line::from(title_parts))
                    .title_bottom(Line::from(Span::styled(
                        help,
                        Style::default().fg(rgb(80, 80, 80)),
                    )))
                    .border_style(Style::default().fg(border_color)),
            )
            .highlight_style(
                Style::default()
                    .bg(rgb(40, 44, 52))
                    .add_modifier(Modifier::BOLD),
            );

        frame.render_stateful_widget(list, area, &mut self.list_state);
    }

    pub(super) fn render_crash_banner(&self, frame: &mut Frame, area: Rect) {
        let Some(info) = &self.crashed_sessions else {
            return;
        };

        let title = if info.session_ids.len() == 1 {
            " crashed session detected "
        } else {
            " crashed sessions detected "
        };
        let names = info.display_names.join(", ");
        let body = vec![
            Line::from(vec![
                Span::styled("💥 ", Style::default().fg(rgb(255, 140, 140))),
                Span::styled(names, Style::default().fg(Color::White)),
            ]),
            Line::from(vec![Span::styled(
                "Press B to restore all from the last crash window.",
                Style::default().fg(rgb(180, 180, 180)),
            )]),
        ];

        let block = Paragraph::new(body)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(rgb(255, 140, 140))),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(block, area);
    }
}
