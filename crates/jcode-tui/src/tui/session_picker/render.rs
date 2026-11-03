use super::*;
use ratatui::widgets::Wrap;

impl SessionPicker {
    pub(super) fn todo_dot_spans(session: &SessionInfo, width: usize) -> Vec<Span<'static>> {
        let count = session.todo_statuses.len();
        let visible = count.min(width);
        (0..visible)
            .map(|index| {
                if index + 1 == visible && count > visible {
                    return Span::styled("…", Style::default().fg(rgb(140, 140, 140)));
                }
                let color = match session.todo_statuses[index].as_str() {
                    "completed" => rgb(129, 199, 132),
                    "in_progress" => rgb(255, 213, 79),
                    "cancelled" => rgb(80, 80, 80),
                    _ => rgb(140, 140, 140),
                };
                Span::styled("●", Style::default().fg(color))
            })
            .collect()
    }

    fn running_spinner_frame() -> usize {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .saturating_div(u128::from(
                jcode_tui_render::swarm_gallery::STRIP_SPINNER_FRAME_MS,
            )) as usize
    }

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

    #[cfg(test)]
    pub(super) fn format_estimated_tokens(tokens: usize) -> String {
        if tokens < 1_000 {
            return format!("~{} tok", tokens);
        }

        const UNITS: &[(f64, &str)] = &[
            (1.0, ""),
            (1_000.0, "k"),
            (1_000_000.0, "M"),
            (1_000_000_000.0, "B"),
            (1_000_000_000_000.0, "T"),
            (1_000_000_000_000_000.0, "P"),
            (1_000_000_000_000_000_000.0, "E"),
        ];

        let tokens = tokens as f64;
        let mut unit_idx = 0;
        while unit_idx + 1 < UNITS.len() && tokens >= UNITS[unit_idx + 1].0 {
            unit_idx += 1;
        }

        loop {
            let value = tokens / UNITS[unit_idx].0;
            let decimals = if value < 10.0 { 1 } else { 0 };
            let rounded = if decimals == 1 {
                (value * 10.0).round() / 10.0
            } else {
                value.round()
            };

            if rounded >= 1000.0 && unit_idx + 1 < UNITS.len() {
                unit_idx += 1;
                continue;
            }

            let value_display = if decimals == 1 && (rounded.fract()).abs() > f64::EPSILON {
                format!("{rounded:.1}")
            } else {
                format!("{rounded:.0}")
            };
            return format!("~{}{} tok", value_display, UNITS[unit_idx].1);
        }
    }

    /// Normalized search tokens used for highlighting, or an empty vec when there
    /// is no active search. Mirrors the matcher's tokenization so highlighting and
    /// filtering agree on what counts as a match.
    pub(super) fn active_highlight_tokens(&self) -> Vec<String> {
        super::loading::search_query_tokens(&self.search_query)
    }

    /// Split `text` into spans, applying `base` to non-matching segments and a
    /// distinct highlight style to case-insensitive occurrences of any of the
    /// `tokens`. Tokens are matched independently (logical OR for highlighting),
    /// matching the AND-token filter's notion of "this word matched". Overlapping
    /// or adjacent matches are merged via a per-character highlight mask.
    pub(super) fn highlight_spans(
        text: &str,
        tokens: &[String],
        base: Style,
    ) -> Vec<Span<'static>> {
        if tokens.is_empty() || text.is_empty() {
            return vec![Span::styled(text.to_string(), base)];
        }

        let chars: Vec<char> = text.chars().collect();
        let lower: String = text.to_lowercase();
        // Map lowercased byte offsets back to char indices so multi-byte and
        // case-folding-width changes can't desync the mask.
        let lower_chars: Vec<char> = lower.chars().collect();

        let mut mask = vec![false; lower_chars.len()];
        let mut any = false;
        for token in tokens {
            if token.is_empty() || token.chars().count() > lower_chars.len() {
                continue;
            }
            let needle: Vec<char> = token.chars().collect();
            let mut i = 0;
            while i + needle.len() <= lower_chars.len() {
                if lower_chars[i..i + needle.len()] == needle[..] {
                    for slot in mask.iter_mut().skip(i).take(needle.len()) {
                        *slot = true;
                    }
                    any = true;
                    i += needle.len();
                } else {
                    i += 1;
                }
            }
        }

        if !any {
            return vec![Span::styled(text.to_string(), base)];
        }

        // The lowercase char count can differ from the original char count when
        // case folding changes length (rare); fall back to no highlight rather
        // than risk a slice mismatch.
        if mask.len() != chars.len() {
            return vec![Span::styled(text.to_string(), base)];
        }

        let highlight = base.fg(rgb(255, 214, 90)).add_modifier(Modifier::BOLD);
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut idx = 0;
        while idx < chars.len() {
            let hot = mask[idx];
            let start = idx;
            while idx < chars.len() && mask[idx] == hot {
                idx += 1;
            }
            let segment: String = chars[start..idx].iter().collect();
            spans.push(Span::styled(segment, if hot { highlight } else { base }));
        }
        spans
    }

    fn primary_title_display(session: &SessionInfo) -> String {
        let title = session.title.trim();
        let short_name = session.short_name.trim();
        let primary = if title.is_empty() { short_name } else { title };
        if primary.chars().count() > 54 {
            format!("{}...", safe_truncate(primary, 51))
        } else {
            primary.to_string()
        }
    }

    #[cfg(test)]
    pub(super) fn render_session_item_lines(
        &self,
        session: &SessionInfo,
        is_selected: bool,
    ) -> Vec<Line<'static>> {
        self.render_session_item_lines_at_frame(session, is_selected, Self::running_spinner_frame())
    }

    pub(super) fn render_session_item_lines_at_frame(
        &self,
        session: &SessionInfo,
        is_selected: bool,
        _spinner_frame: usize,
    ) -> Vec<Line<'static>> {
        let dim: Color = rgb(100, 100, 100);
        let dimmer: Color = rgb(70, 70, 70);
        let batch_restore: Color = rgb(255, 140, 140);

        // The list is ordered by last activity, so show that instead of the
        // creation time (a day-old session active a minute ago sits on top).
        let active_ago =
            format_time_ago(session.last_active_at.unwrap_or(session.last_message_time));
        let in_batch_restore = self.crashed_session_ids.contains(&session.id);
        let is_marked = self.selected_session_ids.contains(&session.id);
        let same_dir = self.session_in_current_dir(session);
        let same_dir_clr: Color = rgb(120, 200, 140);
        let highlight_tokens = self.active_highlight_tokens();

        let recently_closed = self.session_recently_closed(session);
        let name_style = if is_selected {
            Style::default()
                .fg(rgb(140, 220, 160))
                .add_modifier(Modifier::BOLD)
        } else if recently_closed {
            Style::default()
                .fg(rgb(255, 193, 7))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let debug_marker = if session.is_debug { " 🧪" } else { "" };
        let saved_marker = if session.saved { " 📌" } else { "" };
        let selection_marker = if is_marked { "● " } else { "○ " };
        let selection_style = if is_marked {
            Style::default()
                .fg(rgb(140, 220, 160))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(rgb(90, 90, 90))
        };

        let is_current = self.session_is_current(session);

        let primary_title = Self::primary_title_display(session);
        let mut line1_spans = vec![
            Span::styled(selection_marker, selection_style),
            Span::styled(
                format!("{} ", session.icon),
                Style::default().fg(rgb(110, 210, 255)),
            ),
        ];
        line1_spans.extend(Self::highlight_spans(
            &primary_title,
            &highlight_tokens,
            name_style,
        ));
        line1_spans.extend([
            Span::styled(debug_marker, Style::default().fg(rgb(180, 180, 180))),
            Span::styled(saved_marker, Style::default().fg(rgb(255, 180, 100))),
        ]);
        if let Some(ref label) = session.save_label {
            let label_style = Style::default().fg(rgb(255, 200, 140));
            line1_spans.push(Span::styled("  \"".to_string(), label_style));
            line1_spans.extend(Self::highlight_spans(label, &highlight_tokens, label_style));
            line1_spans.push(Span::styled("\"".to_string(), label_style));
        }
        if let Some(source_badge) = session.source.badge() {
            line1_spans.push(Span::styled(
                format!("  {}", source_badge),
                Style::default()
                    .fg(rgb(120, 210, 255))
                    .add_modifier(Modifier::BOLD),
            ));
        }
        if session.server_icon.as_deref() == Some("☁")
            && let Some(host) = session.server_name.as_deref()
        {
            line1_spans.push(Span::styled(
                format!("  ☁ {host}"),
                Style::default().fg(rgb(120, 210, 255)),
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
        if is_current {
            line1_spans.push(Span::styled(
                "  ◀ current",
                Style::default()
                    .fg(rgb(110, 210, 255))
                    .add_modifier(Modifier::BOLD),
            ));
        }
        line1_spans.extend(Self::todo_dot_spans(session, 64));
        let line1 = Line::from(line1_spans);

        // Identity and todo dots above, directory and last activity below.
        // Trailing indicators are reserved before truncating each row's left side.
        let mut detail_spans = vec![Span::raw("  ")];
        if let Some(dir) = session.working_dir.as_deref() {
            let directory_icon = if session.server_icon.as_deref() == Some("☁") {
                "☁"
            } else {
                "📁"
            };
            detail_spans.extend(Self::highlight_spans(
                &format!("{directory_icon} {}", compact_dir(dir)),
                &highlight_tokens,
                Style::default().fg(if same_dir { same_dir_clr } else { dim }),
            ));
        } else {
            detail_spans.push(Span::styled("No directory", Style::default().fg(dimmer)));
        }
        if same_dir {
            detail_spans.push(Span::styled(
                "  ▸ here",
                Style::default()
                    .fg(same_dir_clr)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        let activity = format!("  {active_ago}");
        detail_spans.push(Span::styled(activity, Style::default().fg(dim)));
        let mut rows = vec![line1, Line::from(detail_spans)];
        if let Some(reason_line) = Self::crash_reason_line(session) {
            rows.push(reason_line);
        }

        rows
    }

    fn render_session_item(
        &self,
        session: &SessionInfo,
        is_selected: bool,
        spinner_frame: usize,
        striped: bool,
        width: usize,
    ) -> ListItem<'static> {
        let batch_row_bg: Color = rgb(36, 18, 18);
        let in_batch_restore = self.crashed_session_ids.contains(&session.id);
        let rows: Vec<_> = self
            .render_session_item_lines_at_frame(session, is_selected, spinner_frame)
            .into_iter()
            .enumerate()
            .map(|(index, mut line)| {
                if index == 0 {
                    // Keep at least half the row for identity, even for huge plans.
                    line.spans
                        .truncate(line.spans.len() - session.todo_statuses.len().min(64));
                    let dots = Self::todo_dot_spans(session, width.saturating_sub(8) / 2);
                    if dots.is_empty() {
                        return jcode_tui_render::truncate_line_with_ellipsis_to_width(
                            &line, width,
                        );
                    }
                    let left_width = width.saturating_sub(dots.len() + 2);
                    let mut left =
                        jcode_tui_render::truncate_line_with_ellipsis_to_width(&line, left_width);
                    left.spans
                        .push(Span::raw(" ".repeat(width - dots.len() - left.width())));
                    left.spans.extend(dots);
                    return left;
                }
                if index < 2 {
                    let Some(suffix) = line.spans.pop() else {
                        return line;
                    };
                    // Tiny terminals prioritize a truncated identity/directory.
                    if suffix.width() + 8 <= width {
                        let left_width = width - suffix.width();
                        let mut left = jcode_tui_render::truncate_line_with_ellipsis_to_width(
                            &line, left_width,
                        );
                        left.spans
                            .push(Span::raw(" ".repeat(left_width - left.width())));
                        left.spans.push(suffix);
                        return left;
                    }
                    line.spans.push(suffix);
                }
                jcode_tui_render::truncate_line_with_ellipsis_to_width(&line, width)
            })
            .collect();
        let mut item = ListItem::new(rows);
        if in_batch_restore && !is_selected {
            item = item.style(Style::default().bg(batch_row_bg));
        } else if striped {
            item = item.style(Style::default().bg(rgb(28, 30, 34)));
        }
        item
    }

    pub(super) fn render_session_list(&mut self, frame: &mut Frame, area: Rect) {
        self.render_session_list_impl(frame, area, true);
    }

    fn list_session_at(&self, index: usize) -> Option<&SessionInfo> {
        self.item_to_session
            .get(index)
            .copied()
            .flatten()
            .and_then(|index| self.visible_sessions.get(index).copied())
            .and_then(|session_ref| self.session_by_ref(session_ref))
    }

    pub(super) fn render_session_list_impl(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        virtualize: bool,
    ) {
        self.poll_transcript_search();
        // List rows never wrap. Determine exact heights without formatting the
        // offscreen titles, prompts, paths, timestamps, or highlighted spans.
        let mut item_heights: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                if !matches!(item, PickerItem::Session) {
                    return 1;
                }
                self.list_session_at(index).map_or(1, |session| {
                    2 + usize::from(matches!(session.status, SessionStatus::Crashed { .. }))
                })
            })
            .collect();
        let inner_height = area.height.saturating_sub(2) as usize;
        let virtualize = virtualize && self.loading_message.is_none() && !self.items.is_empty();
        let visible = if virtualize {
            performance::visible_item_bounds(
                &item_heights,
                self.list_state.selected(),
                self.list_state.offset(),
                inner_height,
            )
        } else {
            0..self.items.len()
        };
        let server_color: Color = rgb(255, 200, 100);
        let dim: Color = rgb(100, 100, 100);
        let spinner_frame = Self::running_spinner_frame();
        let spinner = jcode_tui_render::swarm_gallery::STRIP_SPINNER_FRAMES
            [spinner_frame % jcode_tui_render::swarm_gallery::STRIP_SPINNER_FRAMES.len()];

        let items: Vec<ListItem> = if let Some(message) = self.loading_message.as_deref() {
            vec![
                ListItem::new(Line::from(vec![
                    Span::styled("  ⏳ ", Style::default().fg(rgb(255, 200, 100))),
                    Span::styled(
                        message.to_string(),
                        Style::default()
                            .fg(rgb(220, 220, 220))
                            .add_modifier(Modifier::BOLD),
                    ),
                ])),
                ListItem::new(Line::from(vec![Span::styled(
                    "     Scanning local, imported, and running sessions…",
                    Style::default().fg(dim),
                )])),
            ]
        } else if self.items.is_empty()
            && self.filter_mode == jcode_tui_session_picker::SessionFilterMode::Active
        {
            vec![
                ListItem::new(Line::from(vec![Span::styled(
                    "  No other active sessions",
                    Style::default()
                        .fg(rgb(220, 220, 220))
                        .add_modifier(Modifier::BOLD),
                )])),
                ListItem::new(Line::from(vec![Span::styled(
                    "     s cycles to all sessions · Esc closes",
                    Style::default().fg(dim),
                )])),
            ]
        } else {
            self.items
                .iter()
                .enumerate()
                .skip(visible.start)
                .take(visible.len())
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
                                Span::styled(
                                    format!("{} ", icon),
                                    Style::default().fg(server_color),
                                ),
                                Span::styled(
                                    name.clone(),
                                    Style::default()
                                        .fg(server_color)
                                        .add_modifier(Modifier::BOLD),
                                ),
                                Span::styled(
                                    if version.is_empty() {
                                        format!("  {} sessions", session_count)
                                    } else {
                                        format!("  {} · {} sessions", version, session_count)
                                    },
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
                                Span::styled(
                                    format!("  {}", session_count),
                                    Style::default().fg(dim),
                                ),
                            ]);
                            ListItem::new(vec![line1])
                        }
                        PickerItem::Session => self
                            .list_session_at(idx)
                            .map(|session| {
                                // Use the filtered session ordinal, not the viewport row,
                                // so headers and scrolling do not change stripe parity.
                                let striped =
                                    self.item_to_session[idx].is_some_and(|index| index % 2 == 1);
                                self.render_session_item(
                                    session,
                                    is_selected,
                                    spinner_frame,
                                    striped,
                                    area.width.saturating_sub(3) as usize,
                                )
                            })
                            .unwrap_or_else(|| ListItem::new(Line::from(""))),
                    }
                })
                .collect()
        };

        let mut title_parts: Vec<Span> = Vec::new();
        if self.loading_message.is_some() {
            title_parts.push(Span::styled(
                if self.search_active {
                    " loading "
                } else {
                    " loading sessions "
                },
                Style::default()
                    .fg(rgb(255, 200, 100))
                    .add_modifier(Modifier::BOLD),
            ));
        } else if self.filter_mode == jcode_tui_session_picker::SessionFilterMode::Active {
            // The Active view breaks the count down into "still working" vs
            // "ready for input" so the user can triage at a glance. Counts are
            // derived from the visible rows so search/debug filters stay honest.
            let (working, ready) =
                self.visible_session_iter()
                    .fold((0usize, 0usize), |(working, ready), session| {
                        if self.session_is_streaming(session) {
                            (working + 1, ready)
                        } else {
                            (working, ready + 1)
                        }
                    });
            title_parts.push(Span::styled(
                format!(" {} active ", working + ready),
                Style::default()
                    .fg(rgb(200, 200, 200))
                    .add_modifier(Modifier::BOLD),
            ));
            title_parts.push(Span::styled(
                format!("{}{} working", spinner, working),
                Style::default().fg(rgb(255, 193, 7)),
            ));
            title_parts.push(Span::styled(" · ", Style::default().fg(rgb(80, 80, 80))));
            title_parts.push(Span::styled(
                format!("●{} ready", ready),
                Style::default().fg(rgb(100, 220, 130)),
            ));
        } else {
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
        }

        let filter_label = self.filter_mode.label().unwrap_or("all");
        title_parts.push(Span::styled(
            format!("  {}", filter_label),
            Style::default().fg(rgb(255, 180, 100)),
        ));
        // These shortcuts insert search text while editing, so do not advertise
        // them there or let their hint push the query out of a narrow header.
        if !self.search_active {
            title_parts.push(Span::styled(
                " (s/S filter)",
                Style::default().fg(rgb(80, 80, 80)),
            ));
        }

        if self.hidden_test_count > 0 && !self.search_active {
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

        let mut help = if self.search_active && self.loading_message.is_some() {
            " type to filter · loading… · Esc clear · Ctrl+C cancel ".to_string()
        } else if self.loading_message.is_some() {
            " / search · s/S filter · Esc cancel ".to_string()
        } else if self.transcript_search.pending() {
            " searching full transcripts… · type to refine · Ctrl+C cancel ".to_string()
        } else if self.search_active {
            " type to filter · Ctrl+J/K or ↑↓ nav · Ctrl+W word-del · Esc cancel ".to_string()
        } else {
            match crate::config::config().keybindings.session_picker_enter {
                crate::config::SessionPickerResumeAction::CurrentTerminal => {
                    " Space select · Enter in place · Ctrl+Enter new terminal · d debug · / search · h/l focus · ↑↓ · q ".to_string()
                }
                crate::config::SessionPickerResumeAction::NewTerminal => {
                    " Space select · Enter new terminal · Ctrl+Enter in place · d debug · / search · h/l focus · ↑↓ · q ".to_string()
                }
            }
        };
        if self.selected_live_claude_target().is_some() && !self.search_active {
            help = format!(" T take over live Claude ·{}", help);
        }

        let border_dim: Color = rgb(70, 70, 70);
        let border_focus: Color = rgb(130, 130, 160);
        let border_color = if self.focus == PaneFocus::Sessions {
            border_focus
        } else {
            border_dim
        };

        // Measure total rendered rows and per-item heights so we can show a
        // native scrollbar when the list overflows. `List` renders each item at
        // its own line count (no wrapping), so summing item heights gives the
        // exact content height, and a prefix sum maps the scroll offset (first
        // visible item index) to a rendered-row offset for the scrollbar thumb.
        if !virtualize {
            item_heights = items.iter().map(|item| item.height().max(1)).collect();
        }
        let total_item_rows: usize = item_heights.iter().sum();
        let show_scrollbar =
            super::super::ui::native_scrollbar_visible(true, total_item_rows, inner_height);

        let list = List::new(items)
            // Reserve the rail even on unselected rows so moving focus never
            // shifts text. A quiet green-gray fill blends with the zebra rows.
            .highlight_symbol("▌")
            .repeat_highlight_symbol(true)
            .highlight_spacing(ratatui::widgets::HighlightSpacing::Always)
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
            .highlight_style(if self.onboarding_start_new_highlighted() {
                // Focus is on the onboarding "Start a new session" row; dim the
                // list selection so only one row reads as active.
                Style::default().fg(rgb(150, 150, 160))
            } else {
                Style::default()
                    .fg(rgb(140, 220, 160))
                    .bg(rgb(36, 43, 42))
                    .add_modifier(Modifier::BOLD)
            });

        if virtualize {
            let mut visible_state = ListState::default().with_selected(
                self.list_state
                    .selected()
                    .filter(|index| visible.contains(index))
                    .map(|index| index - visible.start),
            );
            frame.render_stateful_widget(list, area, &mut visible_state);
            if inner_height > 0 && area.width > 2 {
                *self.list_state.offset_mut() = visible.start + visible_state.offset();
            }
        } else {
            frame.render_stateful_widget(list, area, &mut self.list_state);
        }

        // Draw the scrollbar inside the right border, after the list has updated
        // its scroll offset for this frame. Translate the first-visible item index
        // to a rendered-row offset so the thumb tracks long, multi-line items.
        if show_scrollbar && area.width > 2 {
            let offset_item = self.list_state.offset().min(item_heights.len());
            let row_offset: usize = item_heights[..offset_item].iter().sum();
            let scrollbar_area = Rect {
                x: area.x + area.width.saturating_sub(1),
                y: area.y + 1,
                width: 1,
                height: area.height.saturating_sub(2),
            };
            super::super::ui::render_native_scrollbar(
                frame,
                scrollbar_area,
                row_offset,
                total_item_rows,
                inner_height,
                self.focus == PaneFocus::Sessions,
            );
        }
    }

    pub(super) fn render_crash_banner(&self, frame: &mut Frame, area: Rect) {
        let Some(info) = &self.crashed_sessions else {
            return;
        };

        let omitted = if info.omitted_crashed_count > 0 {
            format!(" · {} older skipped", info.omitted_crashed_count)
        } else {
            String::new()
        };
        let title = format!(
            " R restore shown group · {} relevant crashed session(s) detected{} ",
            info.session_ids.len(),
            omitted
        );
        let names = info.display_names.join(", ");
        let body = vec![
            Line::from(vec![
                Span::styled("💥 ", Style::default().fg(rgb(255, 140, 140))),
                Span::styled(names, Style::default().fg(Color::White)),
            ]),
            Line::from(vec![Span::styled(
                "Press R (or B) to restore only this guessed recent group.",
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

/// Short, recognizable form of a session directory: `~` for home and, when
/// still long, the trailing path components that fit (never a cut-off name).
pub(super) fn compact_dir(dir: &str) -> String {
    const MAX: usize = 28;
    let dir = dir.trim_end_matches('/');
    let dir = match dirs::home_dir() {
        Some(home) => {
            let home = home.to_string_lossy();
            match dir.strip_prefix(home.as_ref()) {
                Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
                _ => dir.to_string(),
            }
        }
        None => dir.to_string(),
    };
    if dir.chars().count() <= MAX {
        return dir;
    }
    let mut tail: Vec<&str> = Vec::new();
    let mut len = 2; // leading "…/"
    for component in dir.rsplit('/').filter(|c| !c.is_empty()) {
        let extra = component.chars().count() + usize::from(!tail.is_empty());
        if !tail.is_empty() && len + extra > MAX {
            break;
        }
        tail.push(component);
        len += extra;
    }
    tail.reverse();
    let joined = tail.join("/");
    if joined.chars().count() + 2 > MAX {
        let head: String = joined.chars().take(MAX - 3).collect();
        return format!("…/{head}…");
    }
    format!("…/{joined}")
}
