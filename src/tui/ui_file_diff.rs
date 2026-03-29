use super::*;

fn selection_bg_for(base_bg: Option<Color>) -> Color {
    let fallback = rgb(32, 38, 48);
    blend_color(base_bg.unwrap_or(fallback), accent_color(), 0.34)
}

fn selection_fg_for(base_fg: Option<Color>) -> Option<Color> {
    base_fg.map(|fg| blend_color(fg, Color::White, 0.15))
}

fn highlight_line_selection(
    line: &Line<'static>,
    start_col: usize,
    end_col: usize,
) -> Line<'static> {
    if end_col <= start_col {
        return line.clone();
    }

    let mut rebuilt: Vec<Span<'static>> = Vec::new();
    let mut current_text = String::new();
    let mut current_style: Option<Style> = None;
    let mut col = 0usize;

    let flush = |rebuilt: &mut Vec<Span<'static>>, text: &mut String, style: &mut Option<Style>| {
        if !text.is_empty() {
            let span = match style.take() {
                Some(style) => Span::styled(std::mem::take(text), style),
                None => Span::raw(std::mem::take(text)),
            };
            rebuilt.push(span);
        }
    };

    for span in &line.spans {
        for ch in span.content.chars() {
            let width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            let selected = if width == 0 {
                col > start_col && col <= end_col
            } else {
                col < end_col && col.saturating_add(width) > start_col
            };

            let mut style = span.style;
            if selected {
                style = style.bg(selection_bg_for(style.bg));
                if let Some(fg) = selection_fg_for(style.fg) {
                    style = style.fg(fg);
                }
            }

            if current_style == Some(style) {
                current_text.push(ch);
            } else {
                flush(&mut rebuilt, &mut current_text, &mut current_style);
                current_text.push(ch);
                current_style = Some(style);
            }

            col = col.saturating_add(width);
        }
    }

    flush(&mut rebuilt, &mut current_text, &mut current_style);

    Line {
        spans: rebuilt,
        style: line.style,
        alignment: line.alignment,
    }
}

fn apply_side_selection_highlight(
    app: &dyn TuiState,
    visible_lines: &mut [Line<'static>],
    scroll: usize,
) {
    let Some(range) = app.copy_selection_range().filter(|range| {
        range.start.pane == crate::tui::CopySelectionPane::SidePane
            && range.end.pane == crate::tui::CopySelectionPane::SidePane
    }) else {
        return;
    };

    let (start, end) =
        if (range.start.abs_line, range.start.column) <= (range.end.abs_line, range.end.column) {
            (range.start, range.end)
        } else {
            (range.end, range.start)
        };

    let visible_end = scroll.saturating_add(visible_lines.len());
    for abs_idx in start.abs_line.max(scroll)..=end.abs_line.min(visible_end.saturating_sub(1)) {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FileContentSignature {
    len_bytes: u64,
    modified: Option<std::time::SystemTime>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct FileDiffCacheKey {
    pub(super) file_path: String,
    pub(super) msg_index: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum FileDiffDisplayRowKind {
    Normal,
    Add,
    Del,
    Placeholder,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FileDiffDisplayRow {
    pub(super) prefix: String,
    pub(super) text: String,
    pub(super) kind: FileDiffDisplayRowKind,
}

pub(super) struct FileDiffViewCacheEntry {
    pub(super) file_sig: Option<FileContentSignature>,
    pub(super) rows: Vec<FileDiffDisplayRow>,
    pub(super) rendered_rows: Vec<Option<Line<'static>>>,
    pub(super) first_change_line: usize,
    pub(super) additions: usize,
    pub(super) deletions: usize,
    pub(super) file_ext: Option<String>,
}

#[derive(Default)]
pub(super) struct FileDiffViewCacheState {
    pub(super) entries: HashMap<FileDiffCacheKey, FileDiffViewCacheEntry>,
    pub(super) order: VecDeque<FileDiffCacheKey>,
}

impl FileDiffViewCacheState {
    pub(super) fn insert(&mut self, key: FileDiffCacheKey, entry: FileDiffViewCacheEntry) {
        if !self.entries.contains_key(&key) {
            self.order.push_back(key.clone());
        }
        self.entries.insert(key, entry);

        while self.order.len() > FILE_DIFF_CACHE_LIMIT {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }
}

const FILE_DIFF_CACHE_LIMIT: usize = 8;

static FILE_DIFF_CACHE: OnceLock<Mutex<FileDiffViewCacheState>> = OnceLock::new();

pub(super) fn file_diff_cache() -> &'static Mutex<FileDiffViewCacheState> {
    FILE_DIFF_CACHE.get_or_init(|| Mutex::new(FileDiffViewCacheState::default()))
}

pub(super) fn file_content_signature(file_path: &str) -> Option<FileContentSignature> {
    let metadata = std::fs::metadata(file_path).ok()?;
    Some(FileContentSignature {
        len_bytes: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn render_file_diff_row(row: &FileDiffDisplayRow, file_ext: Option<&str>) -> Line<'static> {
    match row.kind {
        FileDiffDisplayRowKind::Placeholder => Line::from(Span::styled(
            row.text.clone(),
            Style::default().fg(dim_color()),
        )),
        FileDiffDisplayRowKind::Normal => {
            let mut spans = vec![Span::styled(
                row.prefix.clone(),
                Style::default().fg(dim_color()),
            )];
            spans.extend(markdown::highlight_line(&row.text, file_ext));
            Line::from(spans)
        }
        FileDiffDisplayRowKind::Add => {
            let mut spans = vec![Span::styled(
                row.prefix.clone(),
                Style::default().fg(diff_add_color()),
            )];
            for span in markdown::highlight_line(&row.text, file_ext) {
                spans.push(tint_span_with_diff_color(span, diff_add_color()));
            }
            Line::from(spans)
        }
        FileDiffDisplayRowKind::Del => {
            let mut spans = vec![Span::styled(
                row.prefix.clone(),
                Style::default().fg(diff_del_color()),
            )];
            for span in markdown::highlight_line(&row.text, file_ext) {
                spans.push(tint_span_with_diff_color(span, diff_del_color()));
            }
            Line::from(spans)
        }
    }
}

fn materialize_visible_file_diff_lines(
    cached: &mut FileDiffViewCacheEntry,
    start: usize,
    count: usize,
) -> Vec<Line<'static>> {
    if cached.rendered_rows.len() != cached.rows.len() {
        cached.rendered_rows.resize_with(cached.rows.len(), || None);
    }

    let end = start.saturating_add(count).min(cached.rows.len());
    let mut visible = Vec::with_capacity(end.saturating_sub(start));

    for idx in start..end {
        if cached.rendered_rows[idx].is_none() {
            let rendered = render_file_diff_row(&cached.rows[idx], cached.file_ext.as_deref());
            cached.rendered_rows[idx] = Some(rendered);
        }
        if let Some(line) = cached.rendered_rows[idx].as_ref() {
            visible.push(line.clone());
        }
    }

    visible
}

fn diff_lines_for_message(msg: Option<&DisplayMessage>) -> Vec<ParsedDiffLine> {
    let Some(msg) = msg else {
        return Vec::new();
    };
    let Some(tc) = msg.tool_data.as_ref() else {
        return Vec::new();
    };

    let from_content = collect_diff_lines(&msg.content);
    if !from_content.is_empty() {
        from_content
    } else {
        generate_diff_lines_from_tool_input(tc)
    }
}

fn build_file_diff_cache_entry(
    file_path: &str,
    msg: Option<&DisplayMessage>,
    file_sig: Option<FileContentSignature>,
) -> FileDiffViewCacheEntry {
    let diff_lines = diff_lines_for_message(msg);
    let file_content = std::fs::read_to_string(file_path).unwrap_or_default();
    let file_ext = std::path::Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_owned);

    struct DiffHunk {
        dels: Vec<String>,
        adds: Vec<String>,
    }

    let mut hunks: Vec<DiffHunk> = Vec::new();
    {
        let mut current_dels: Vec<String> = Vec::new();
        let mut current_adds: Vec<String> = Vec::new();
        for dl in &diff_lines {
            match dl.kind {
                DiffLineKind::Del => {
                    if !current_adds.is_empty() {
                        hunks.push(DiffHunk {
                            dels: current_dels,
                            adds: current_adds,
                        });
                        current_dels = Vec::new();
                        current_adds = Vec::new();
                    }
                    current_dels.push(dl.content.clone());
                }
                DiffLineKind::Add => {
                    current_adds.push(dl.content.clone());
                }
            }
        }
        if !current_dels.is_empty() || !current_adds.is_empty() {
            hunks.push(DiffHunk {
                dels: current_dels,
                adds: current_adds,
            });
        }
    }

    let mut add_to_dels: HashMap<usize, Vec<String>> = HashMap::new();
    let mut orphan_dels: Vec<String> = Vec::new();
    let file_lines_vec: Vec<&str> = file_content.lines().collect();
    let mut used_file_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();

    for hunk in &hunks {
        if hunk.adds.is_empty() {
            orphan_dels.extend(hunk.dels.clone());
            continue;
        }

        let first_add_trimmed = hunk.adds[0].trim();
        if first_add_trimmed.is_empty() {
            orphan_dels.extend(hunk.dels.clone());
            continue;
        }

        let mut found_idx = None;
        for (fi, fl) in file_lines_vec.iter().enumerate() {
            if !used_file_lines.contains(&fi) && fl.trim() == first_add_trimmed {
                found_idx = Some(fi);
                break;
            }
        }

        if let Some(idx) = found_idx {
            for (ai, _) in hunk.adds.iter().enumerate() {
                used_file_lines.insert(idx + ai);
            }
            if !hunk.dels.is_empty() {
                add_to_dels.insert(idx, hunk.dels.clone());
            }
        } else {
            orphan_dels.extend(hunk.dels.clone());
        }
    }

    let mut rows: Vec<FileDiffDisplayRow> = Vec::new();
    let mut first_change_line = usize::MAX;
    let mut del_count = 0usize;
    let mut add_count = 0usize;

    let line_num_width = file_lines_vec.len().to_string().len().max(3);
    let gutter_pad = " ".repeat(line_num_width);

    for (i, line_text) in file_lines_vec.iter().enumerate() {
        let line_num = i + 1;

        if let Some(dels) = add_to_dels.get(&i) {
            for del_text in dels {
                if first_change_line == usize::MAX {
                    first_change_line = rows.len();
                }
                del_count += 1;
                rows.push(FileDiffDisplayRow {
                    prefix: format!("{} │-", gutter_pad),
                    text: del_text.clone(),
                    kind: FileDiffDisplayRowKind::Del,
                });
            }
        }

        if used_file_lines.contains(&i) {
            if first_change_line == usize::MAX {
                first_change_line = rows.len();
            }
            add_count += 1;
            rows.push(FileDiffDisplayRow {
                prefix: format!("{:>width$} │+", line_num, width = line_num_width),
                text: (*line_text).to_string(),
                kind: FileDiffDisplayRowKind::Add,
            });
        } else {
            rows.push(FileDiffDisplayRow {
                prefix: format!("{:>width$} │ ", line_num, width = line_num_width),
                text: (*line_text).to_string(),
                kind: FileDiffDisplayRowKind::Normal,
            });
        }
    }

    for del_text in &orphan_dels {
        if first_change_line == usize::MAX {
            first_change_line = rows.len();
        }
        del_count += 1;
        rows.push(FileDiffDisplayRow {
            prefix: format!("{} │-", gutter_pad),
            text: del_text.clone(),
            kind: FileDiffDisplayRowKind::Del,
        });
    }

    if rows.is_empty() {
        rows.push(FileDiffDisplayRow {
            prefix: String::new(),
            text: "File not found or empty".to_string(),
            kind: FileDiffDisplayRowKind::Placeholder,
        });
    }

    let rendered_rows = vec![None; rows.len()];

    FileDiffViewCacheEntry {
        file_sig,
        rows,
        rendered_rows,
        first_change_line,
        additions: add_count,
        deletions: del_count,
        file_ext,
    }
}

fn find_visible_edit_tool<'a>(
    edit_ranges: &'a [EditToolRange],
    scroll: usize,
    visible_height: usize,
) -> Option<&'a EditToolRange> {
    if edit_ranges.is_empty() {
        return None;
    }

    let visible_start = scroll;
    let visible_end = scroll + visible_height;
    let visible_mid = scroll + visible_height / 2;
    let candidate_start = edit_ranges.partition_point(|range| range.end_line <= visible_start);
    let candidate_end = edit_ranges.partition_point(|range| range.start_line < visible_end);

    let mut best: Option<&EditToolRange> = None;
    let mut best_overlap = 0usize;
    let mut best_distance = usize::MAX;

    for range in &edit_ranges[candidate_start..candidate_end] {
        let overlap_start = range.start_line.max(visible_start);
        let overlap_end = range.end_line.min(visible_end);
        let overlap = if overlap_end > overlap_start {
            overlap_end - overlap_start
        } else {
            0
        };

        let range_mid = (range.start_line + range.end_line) / 2;
        let distance = if range_mid > visible_mid {
            range_mid - visible_mid
        } else {
            visible_mid - range_mid
        };

        if overlap > best_overlap || (overlap == best_overlap && distance < best_distance) {
            best = Some(range);
            best_overlap = overlap;
            best_distance = distance;
        }
    }

    if best.is_some() {
        return best;
    }

    // No overlapping edit range. Check the nearest neighbors around the insertion window
    // instead of rescanning the entire history.
    for idx in [candidate_start.checked_sub(1), Some(candidate_start)]
        .into_iter()
        .flatten()
    {
        if let Some(range) = edit_ranges.get(idx) {
            let range_mid = (range.start_line + range.end_line) / 2;
            let distance = range_mid.abs_diff(visible_mid);
            if best.is_none() || distance < best_distance {
                best = Some(range);
                best_distance = distance;
            }
        }
    }

    best
}

pub(super) fn active_file_diff_context(
    prepared: &PreparedMessages,
    scroll: usize,
    visible_height: usize,
) -> Option<ActiveFileDiffContext> {
    let range = find_visible_edit_tool(&prepared.edit_tool_ranges, scroll, visible_height)?;
    Some(ActiveFileDiffContext {
        edit_index: range.edit_index + 1,
        msg_index: range.msg_index,
        file_path: range.file_path.clone(),
        start_line: range.start_line,
        end_line: range.end_line,
    })
}

pub(super) fn draw_file_diff_view(
    frame: &mut Frame,
    area: Rect,
    app: &dyn TuiState,
    prepared: &PreparedMessages,
    pane_scroll: usize,
    focused: bool,
) {
    use ratatui::widgets::Paragraph;

    if area.width < 10 || area.height < 3 {
        return;
    }

    let scroll_offset = app.scroll_offset();
    let visible_height = area.height as usize;

    let scroll = if app.auto_scroll_paused() {
        scroll_offset
    } else {
        prepared.wrapped_lines.len().saturating_sub(visible_height)
    };

    let active_context = active_file_diff_context(prepared, scroll, visible_height);

    let Some(active_context) = active_context else {
        let Some(inner) = super::draw_right_rail_chrome(
            frame,
            area,
            Line::from(vec![Span::styled(
                " file ",
                Style::default().fg(tool_color()),
            )]),
            super::right_rail_border_style(false, tool_color()),
        ) else {
            return;
        };
        let msg = Paragraph::new(Line::from(Span::styled(
            "No edits visible",
            Style::default().fg(dim_color()),
        )));
        frame.render_widget(msg, inner);
        return;
    };

    let file_path = &active_context.file_path;
    let msg_index = active_context.msg_index;
    let cache_key = FileDiffCacheKey {
        file_path: file_path.clone(),
        msg_index,
    };
    let file_sig = file_content_signature(file_path);

    let needs_rebuild = {
        let cache = match file_diff_cache().lock() {
            Ok(c) => c,
            Err(poisoned) => poisoned.into_inner(),
        };
        cache
            .entries
            .get(&cache_key)
            .map(|cached| cached.file_sig != file_sig)
            .unwrap_or(true)
    };

    if needs_rebuild {
        let display_messages = app.display_messages();
        let msg = display_messages.get(msg_index);
        let entry = build_file_diff_cache_entry(file_path, msg, file_sig.clone());

        let mut cache = match file_diff_cache().lock() {
            Ok(c) => c,
            Err(poisoned) => poisoned.into_inner(),
        };
        cache.insert(cache_key.clone(), entry);
    }

    let (additions, deletions, total_lines, first_change_line) = {
        let cache = match file_diff_cache().lock() {
            Ok(c) => c,
            Err(poisoned) => poisoned.into_inner(),
        };
        let cached = cache
            .entries
            .get(&cache_key)
            .expect("file diff cache entry should exist after build");
        (
            cached.additions,
            cached.deletions,
            cached.rows.len(),
            cached.first_change_line,
        )
    };

    let short_path = file_path
        .rsplit('/')
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("/");

    let mut title_parts = vec![
        Span::styled(" ", Style::default().fg(dim_color())),
        Span::styled(
            short_path,
            Style::default()
                .fg(rgb(180, 200, 255))
                .add_modifier(ratatui::style::Modifier::BOLD),
        ),
    ];
    if additions > 0 || deletions > 0 {
        title_parts.push(Span::styled(" ", Style::default().fg(dim_color())));
        if additions > 0 {
            title_parts.push(Span::styled(
                format!("+{}", additions),
                Style::default().fg(diff_add_color()),
            ));
        }
        if deletions > 0 {
            if additions > 0 {
                title_parts.push(Span::styled(" ", Style::default().fg(dim_color())));
            }
            title_parts.push(Span::styled(
                format!("-{}", deletions),
                Style::default().fg(diff_del_color()),
            ));
        }
    }
    title_parts.push(Span::styled(
        format!(" {}L ", total_lines),
        Style::default().fg(dim_color()),
    ));
    title_parts.push(Span::styled(
        format!(" edit#{} ", active_context.edit_index),
        Style::default().fg(file_link_color()),
    ));

    let border_style = super::right_rail_border_style(focused, tool_color());
    let Some(inner) =
        super::draw_right_rail_chrome(frame, area, Line::from(title_parts), border_style)
    else {
        return;
    };

    super::set_pinned_pane_total_lines(total_lines);

    let max_scroll = total_lines.saturating_sub(inner.height as usize);

    let effective_scroll = if pane_scroll == usize::MAX && first_change_line != usize::MAX {
        let target = first_change_line.saturating_sub(inner.height as usize / 3);
        target.min(max_scroll)
    } else if pane_scroll == usize::MAX {
        max_scroll
    } else {
        pane_scroll.min(max_scroll)
    };
    super::set_last_diff_pane_effective_scroll(effective_scroll);

    let mut visible_lines = {
        let mut cache = match file_diff_cache().lock() {
            Ok(c) => c,
            Err(poisoned) => poisoned.into_inner(),
        };
        let cached = cache
            .entries
            .get_mut(&cache_key)
            .expect("file diff cache entry should exist after build");
        materialize_visible_file_diff_lines(cached, effective_scroll, inner.height as usize)
    };
    record_side_pane_snapshot(
        &visible_lines,
        effective_scroll,
        effective_scroll + visible_lines.len(),
        inner,
    );
    apply_side_selection_highlight(app, &mut visible_lines, effective_scroll);

    let paragraph = Paragraph::new(visible_lines);
    frame.render_widget(paragraph, inner);
}
