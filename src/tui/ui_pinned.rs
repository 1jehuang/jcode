use super::*;
use crate::tui::mermaid;

/// Format tokens compactly (1.2M, 45K, 123)
fn format_tokens_compact(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.0}K", tokens as f64 / 1_000.0)
    } else {
        format!("{}", tokens)
    }
}

fn format_usage_line(tokens_str: String, cache_status: Option<String>) -> String {
    let mut parts = Vec::new();
    if !tokens_str.is_empty() {
        parts.push(tokens_str);
    }
    if let Some(cache) = cache_status {
        parts.push(cache);
    }
    if parts.is_empty() {
        String::new()
    } else {
        parts.join(" • ")
    }
}

fn format_cache_status(
    cache_read_tokens: Option<u64>,
    cache_creation_tokens: Option<u64>,
) -> Option<String> {
    match (cache_read_tokens, cache_creation_tokens) {
        (Some(read), _) if read > 0 => {
            let k = read / 1000;
            if k > 0 {
                Some(format!("⚡{}k cached", k))
            } else {
                Some(format!("⚡{} cached", read))
            }
        }
        (_, Some(created)) if created > 0 => {
            let k = created / 1000;
            if k > 0 {
                Some(format!("💾{}k stored", k))
            } else {
                Some(format!("💾{} stored", created))
            }
        }
        _ => None,
    }
}

enum PinnedContentEntry {
    Diff {
        file_path: String,
        lines: Vec<ParsedDiffLine>,
        additions: usize,
        deletions: usize,
    },
    Image {
        file_path: String,
        hash: u64,
        width: u32,
        height: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PinnedCacheKey {
    messages_version: u64,
    collect_diffs: bool,
    collect_images: bool,
}

#[derive(Default)]
struct PinnedCacheState {
    key: Option<PinnedCacheKey>,
    entries: Vec<PinnedContentEntry>,
    rendered_lines: Option<PinnedRenderedCache>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SidePanelRenderKey {
    page_id: String,
    updated_at_ms: u64,
    inner_width: u16,
    inner_height: u16,
    has_protocol: bool,
}

#[derive(Default)]
struct SidePanelRenderCacheState {
    key: Option<SidePanelRenderKey>,
    rendered: Option<PinnedRenderedCache>,
}

#[derive(Clone)]
struct PinnedRenderedCache {
    inner_width: u16,
    line_wrap: bool,
    lines: Vec<Line<'static>>,
    image_placements: Vec<PinnedImagePlacement>,
}

#[derive(Clone)]
struct PinnedImagePlacement {
    after_text_line: usize,
    hash: u64,
    rows: u16,
}

static PINNED_CACHE: OnceLock<Mutex<PinnedCacheState>> = OnceLock::new();
static SIDE_PANEL_RENDER_CACHE: OnceLock<Mutex<SidePanelRenderCacheState>> = OnceLock::new();

fn pinned_cache() -> &'static Mutex<PinnedCacheState> {
    PINNED_CACHE.get_or_init(|| Mutex::new(PinnedCacheState::default()))
}

fn side_panel_render_cache() -> &'static Mutex<SidePanelRenderCacheState> {
    SIDE_PANEL_RENDER_CACHE.get_or_init(|| Mutex::new(SidePanelRenderCacheState::default()))
}

pub(super) fn collect_pinned_content_cached(
    messages: &[DisplayMessage],
    collect_diffs: bool,
    collect_images: bool,
    messages_version: u64,
) -> bool {
    let key = PinnedCacheKey {
        messages_version,
        collect_diffs,
        collect_images,
    };

    let mut cache = match pinned_cache().lock() {
        Ok(c) => c,
        Err(poisoned) => poisoned.into_inner(),
    };

    if cache.key.as_ref() == Some(&key) {
        return !cache.entries.is_empty();
    }

    let entries = collect_pinned_content(messages, collect_diffs, collect_images);
    let has_entries = !entries.is_empty();
    cache.key = Some(key);
    cache.entries = entries;
    cache.rendered_lines = None;
    has_entries
}

fn collect_pinned_content(
    messages: &[DisplayMessage],
    collect_diffs: bool,
    collect_images: bool,
) -> Vec<PinnedContentEntry> {
    let mut entries = Vec::new();
    for msg in messages {
        if msg.role != "tool" {
            continue;
        }
        let Some(ref tc) = msg.tool_data else {
            continue;
        };

        if collect_images && matches!(tc.name.as_str(), "read" | "Read") {
            let file_path = tc
                .input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let path = std::path::Path::new(&file_path);
            if is_supported_image_ext(path) && path.exists() {
                if let Some((w, h)) = get_image_dimensions_from_path(path) {
                    let hash = mermaid::register_external_image(path, w, h);
                    entries.push(PinnedContentEntry::Image {
                        file_path,
                        hash,
                        width: w,
                        height: h,
                    });
                }
            }
            continue;
        }

        if !collect_diffs {
            continue;
        }

        if !matches!(
            tc.name.as_str(),
            "edit"
                | "Edit"
                | "write"
                | "multiedit"
                | "patch"
                | "Patch"
                | "apply_patch"
                | "ApplyPatch"
        ) {
            continue;
        }

        let file_path = tc
            .input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| {
                tc.input
                    .get("patch_text")
                    .and_then(|v| v.as_str())
                    .and_then(|patch_text| match tc.name.as_str() {
                        "apply_patch" | "ApplyPatch" => {
                            tools_ui::extract_apply_patch_primary_file(patch_text)
                        }
                        "patch" | "Patch" => {
                            tools_ui::extract_unified_patch_primary_file(patch_text)
                        }
                        _ => None,
                    })
            })
            .unwrap_or_else(|| "unknown".to_string());

        let change_lines = {
            let from_content = collect_diff_lines(&msg.content);
            if !from_content.is_empty() {
                from_content
            } else {
                generate_diff_lines_from_tool_input(tc)
            }
        };
        if change_lines.is_empty() {
            continue;
        }

        let additions = change_lines
            .iter()
            .filter(|l| l.kind == DiffLineKind::Add)
            .count();
        let deletions = change_lines
            .iter()
            .filter(|l| l.kind == DiffLineKind::Del)
            .count();

        entries.push(PinnedContentEntry::Diff {
            file_path,
            lines: change_lines,
            additions,
            deletions,
        });
    }
    entries
}

fn is_supported_image_ext(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| {
            matches!(
                ext.to_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
            )
        })
        .unwrap_or(false)
}

fn get_image_dimensions_from_path(path: &std::path::Path) -> Option<(u32, u32)> {
    let data = std::fs::read(path).ok()?;
    if data.len() > 24 && &data[0..8] == b"\x89PNG\r\n\x1a\n" {
        let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
        let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
        return Some((w, h));
    }
    if data.len() > 2 && data[0] == 0xFF && data[1] == 0xD8 {
        let mut i = 2;
        while i + 9 < data.len() {
            if data[i] == 0xFF {
                let marker = data[i + 1];
                if marker == 0xC0 || marker == 0xC2 {
                    let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                    let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                    return Some((w, h));
                }
                if marker == 0xD9 || marker == 0xDA {
                    break;
                }
                let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
                i += 2 + len;
            } else {
                i += 1;
            }
        }
    }
    if data.len() > 10 && (&data[0..4] == b"GIF8") {
        let w = u16::from_le_bytes([data[6], data[7]]) as u32;
        let h = u16::from_le_bytes([data[8], data[9]]) as u32;
        return Some((w, h));
    }
    None
}

pub(super) fn draw_pinned_content_cached(
    frame: &mut Frame,
    area: Rect,
    scroll: usize,
    line_wrap: bool,
    focused: bool,
) {
    use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

    if area.width < 10 || area.height < 3 {
        return;
    }

    let mut cache = match pinned_cache().lock() {
        Ok(c) => c,
        Err(poisoned) => poisoned.into_inner(),
    };

    if cache.entries.is_empty() {
        return;
    }

    let entries = &cache.entries;
    let total_diffs = entries
        .iter()
        .filter(|e| matches!(e, PinnedContentEntry::Diff { .. }))
        .count();
    let total_images = entries
        .iter()
        .filter(|e| matches!(e, PinnedContentEntry::Image { .. }))
        .count();
    let total_additions: usize = entries
        .iter()
        .map(|e| match e {
            PinnedContentEntry::Diff { additions, .. } => *additions,
            _ => 0,
        })
        .sum();
    let total_deletions: usize = entries
        .iter()
        .map(|e| match e {
            PinnedContentEntry::Diff { deletions, .. } => *deletions,
            _ => 0,
        })
        .sum();

    let mut title_parts = vec![Span::styled(" pinned ", Style::default().fg(tool_color()))];
    if total_diffs > 0 {
        title_parts.push(Span::styled(
            format!("+{}", total_additions),
            Style::default().fg(diff_add_color()),
        ));
        title_parts.push(Span::styled(" ", Style::default().fg(dim_color())));
        title_parts.push(Span::styled(
            format!("-{}", total_deletions),
            Style::default().fg(diff_del_color()),
        ));
        title_parts.push(Span::styled(
            format!(" {}f", total_diffs),
            Style::default().fg(dim_color()),
        ));
    }
    if total_images > 0 {
        if total_diffs > 0 {
            title_parts.push(Span::styled(" ", Style::default().fg(dim_color())));
        }
        title_parts.push(Span::styled(
            format!("📷{}", total_images),
            Style::default().fg(dim_color()),
        ));
    }
    title_parts.push(Span::styled(" ", Style::default().fg(dim_color())));

    let border_color = if focused { tool_color() } else { dim_color() };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(Line::from(title_parts));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let needs_rebuild = match &cache.rendered_lines {
        Some(rendered) => rendered.inner_width != inner.width || rendered.line_wrap != line_wrap,
        None => true,
    };

    if needs_rebuild {
        let has_protocol = mermaid::protocol_type().is_some();
        let mut text_lines: Vec<Line<'static>> = Vec::new();
        let mut image_placements: Vec<PinnedImagePlacement> = Vec::new();

        for (i, entry) in entries.iter().enumerate() {
            if i > 0 {
                text_lines.push(Line::from(""));
            }

            match entry {
                PinnedContentEntry::Diff {
                    file_path,
                    lines: diff_lines,
                    additions,
                    deletions,
                } => {
                    let short_path = file_path
                        .rsplit('/')
                        .take(2)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<Vec<_>>()
                        .join("/");

                    let file_ext = std::path::Path::new(file_path)
                        .extension()
                        .and_then(|e| e.to_str());

                    text_lines.push(Line::from(vec![
                        Span::styled("── ", Style::default().fg(dim_color())),
                        Span::styled(
                            short_path,
                            Style::default()
                                .fg(rgb(180, 200, 255))
                                .add_modifier(ratatui::style::Modifier::BOLD),
                        ),
                        Span::styled(" (", Style::default().fg(dim_color())),
                        Span::styled(
                            format!("+{}", additions),
                            Style::default().fg(diff_add_color()),
                        ),
                        Span::styled(" ", Style::default().fg(dim_color())),
                        Span::styled(
                            format!("-{}", deletions),
                            Style::default().fg(diff_del_color()),
                        ),
                        Span::styled(")", Style::default().fg(dim_color())),
                    ]));

                    for line in diff_lines {
                        let base_color = if line.kind == DiffLineKind::Add {
                            diff_add_color()
                        } else {
                            diff_del_color()
                        };

                        let mut spans: Vec<Span<'static>> = vec![Span::styled(
                            line.prefix.clone(),
                            Style::default().fg(base_color),
                        )];

                        if !line.content.is_empty() {
                            let highlighted =
                                markdown::highlight_line(line.content.as_str(), file_ext);
                            for span in highlighted {
                                let tinted = tint_span_with_diff_color(span, base_color);
                                spans.push(tinted);
                            }
                        }

                        text_lines.push(Line::from(spans));
                    }
                }
                PinnedContentEntry::Image {
                    file_path,
                    hash,
                    width: img_w,
                    height: img_h,
                } => {
                    let short_path = file_path
                        .rsplit('/')
                        .take(2)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<Vec<_>>()
                        .join("/");

                    text_lines.push(Line::from(vec![
                        Span::styled("── 📷 ", Style::default().fg(dim_color())),
                        Span::styled(
                            short_path,
                            Style::default()
                                .fg(rgb(180, 200, 255))
                                .add_modifier(ratatui::style::Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(" {}×{}", img_w, img_h),
                            Style::default().fg(dim_color()),
                        ),
                    ]));

                    if has_protocol {
                        let img_rows = inner.height.min(12).max(4);
                        image_placements.push(PinnedImagePlacement {
                            after_text_line: text_lines.len(),
                            hash: *hash,
                            rows: img_rows,
                        });
                        for _ in 0..img_rows {
                            text_lines.push(Line::from(""));
                        }
                    }
                }
            }
        }

        if text_lines.is_empty() {
            text_lines.push(Line::from(Span::styled(
                "No content yet",
                Style::default().fg(dim_color()),
            )));
        }

        cache.rendered_lines = Some(PinnedRenderedCache {
            inner_width: inner.width,
            line_wrap,
            lines: text_lines,
            image_placements,
        });
    }

    let Some(rendered) = cache.rendered_lines.as_ref() else {
        return;
    };
    let total_lines = rendered.lines.len();
    PINNED_PANE_TOTAL_LINES.store(total_lines, Ordering::Relaxed);

    let max_scroll = total_lines.saturating_sub(inner.height as usize);
    let clamped_scroll = scroll.min(max_scroll);
    LAST_DIFF_PANE_EFFECTIVE_SCROLL.store(clamped_scroll, Ordering::Relaxed);

    let visible_lines: Vec<Line<'static>> = rendered
        .lines
        .iter()
        .skip(clamped_scroll)
        .take(inner.height as usize)
        .cloned()
        .collect();

    let paragraph = if line_wrap {
        Paragraph::new(visible_lines).wrap(Wrap { trim: false })
    } else {
        Paragraph::new(visible_lines)
    };
    frame.render_widget(paragraph, inner);

    let has_protocol = mermaid::protocol_type().is_some();
    if has_protocol {
        for placement in &rendered.image_placements {
            let text_y = placement.after_text_line as u16;
            if text_y < clamped_scroll as u16 {
                continue;
            }
            let y_in_inner = text_y.saturating_sub(clamped_scroll as u16);
            if y_in_inner >= inner.height {
                continue;
            }
            let avail_rows = inner.height.saturating_sub(y_in_inner).min(placement.rows);
            if avail_rows < 2 {
                continue;
            }
            let img_area = Rect {
                x: inner.x,
                y: inner.y + y_in_inner,
                width: inner.width,
                height: avail_rows,
            };
            mermaid::render_image_widget_fit(
                placement.hash,
                img_area,
                frame.buffer_mut(),
                false,
                false,
            );
        }
    }
}

pub(super) fn draw_side_panel_markdown(
    frame: &mut Frame,
    area: Rect,
    snapshot: &crate::side_panel::SidePanelSnapshot,
    scroll: usize,
    focused: bool,
) {
    use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

    if area.width < 10 || area.height < 3 {
        return;
    }

    let Some(page) = snapshot.focused_page() else {
        return;
    };

    let page_index = snapshot
        .pages
        .iter()
        .position(|candidate| candidate.id == page.id)
        .map(|idx| idx + 1)
        .unwrap_or(1);
    let page_count = snapshot.pages.len();

    let mut title_parts = vec![Span::styled(" side ", Style::default().fg(tool_color()))];
    title_parts.push(Span::styled(
        page.title.clone(),
        Style::default()
            .fg(rgb(180, 200, 255))
            .add_modifier(ratatui::style::Modifier::BOLD),
    ));
    title_parts.push(Span::styled(
        format!(" {}/{} ", page_index, page_count),
        Style::default().fg(dim_color()),
    ));

    let border_color = if focused { tool_color() } else { dim_color() };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(Line::from(title_parts));

    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let has_protocol = mermaid::protocol_type().is_some();
    let rendered = render_side_panel_markdown_cached(page, inner, has_protocol);

    PINNED_PANE_TOTAL_LINES.store(rendered.lines.len(), Ordering::Relaxed);
    let max_scroll = rendered.lines.len().saturating_sub(inner.height as usize);
    let clamped_scroll = scroll.min(max_scroll);
    LAST_DIFF_PANE_EFFECTIVE_SCROLL.store(clamped_scroll, Ordering::Relaxed);

    let visible_lines: Vec<Line<'static>> = rendered
        .lines
        .iter()
        .skip(clamped_scroll)
        .take(inner.height as usize)
        .cloned()
        .collect();
    frame.render_widget(Paragraph::new(visible_lines), inner);

    if has_protocol {
        for placement in &rendered.image_placements {
            let text_y = placement.after_text_line as u16;
            if text_y < clamped_scroll as u16 {
                continue;
            }
            let y_in_inner = text_y.saturating_sub(clamped_scroll as u16);
            if y_in_inner >= inner.height {
                continue;
            }
            let avail_rows = inner.height.saturating_sub(y_in_inner).min(placement.rows);
            if avail_rows < 2 {
                continue;
            }
            let img_area = Rect {
                x: inner.x,
                y: inner.y + y_in_inner,
                width: inner.width,
                height: avail_rows,
            };
            mermaid::render_image_widget_fit(
                placement.hash,
                img_area,
                frame.buffer_mut(),
                false,
                false,
            );
        }
    }
}

fn render_side_panel_markdown_cached(
    page: &crate::side_panel::SidePanelPage,
    inner: Rect,
    has_protocol: bool,
) -> PinnedRenderedCache {
    let key = SidePanelRenderKey {
        page_id: page.id.clone(),
        updated_at_ms: page.updated_at_ms,
        inner_width: inner.width,
        inner_height: inner.height,
        has_protocol,
    };

    {
        let cache = match side_panel_render_cache().lock() {
            Ok(cache) => cache,
            Err(poisoned) => poisoned.into_inner(),
        };
        if cache.key.as_ref() == Some(&key) {
            if let Some(rendered) = &cache.rendered {
                return rendered.clone();
            }
        }
    }

    let saved_override = markdown::get_diagram_mode_override();
    let saved_centered = markdown::center_code_blocks();
    markdown::set_diagram_mode_override(Some(crate::config::DiagramDisplayMode::None));
    markdown::set_center_code_blocks(false);
    let rendered_markdown =
        markdown::render_markdown_with_width(&page.content, Some(inner.width as usize));
    markdown::set_center_code_blocks(saved_centered);
    markdown::set_diagram_mode_override(saved_override);

    let mut text_lines: Vec<Line<'static>> = Vec::new();
    let mut image_placements: Vec<PinnedImagePlacement> = Vec::new();
    for line in rendered_markdown {
        if has_protocol {
            if let Some(hash) = mermaid::parse_image_placeholder(&line) {
                let img_rows = inner.height.min(12).max(4);
                image_placements.push(PinnedImagePlacement {
                    after_text_line: text_lines.len(),
                    hash,
                    rows: img_rows,
                });
                for _ in 0..img_rows {
                    text_lines.push(Line::from(""));
                }
                continue;
            }
        }
        text_lines.push(line);
    }

    if text_lines.is_empty() {
        text_lines.push(Line::from(Span::styled(
            "No side panel content yet",
            Style::default().fg(dim_color()),
        )));
    }

    let rendered = PinnedRenderedCache {
        inner_width: inner.width,
        line_wrap: false,
        lines: text_lines,
        image_placements,
    };

    let mut cache = match side_panel_render_cache().lock() {
        Ok(cache) => cache,
        Err(poisoned) => poisoned.into_inner(),
    };
    cache.key = Some(key);
    cache.rendered = Some(rendered.clone());

    rendered
}

#[allow(dead_code)]
fn draw_pinned_content(
    frame: &mut Frame,
    area: Rect,
    entries: &[PinnedContentEntry],
    scroll: usize,
    line_wrap: bool,
    focused: bool,
) {
    use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

    if area.width < 10 || area.height < 3 {
        return;
    }

    let total_diffs = entries
        .iter()
        .filter(|e| matches!(e, PinnedContentEntry::Diff { .. }))
        .count();
    let total_images = entries
        .iter()
        .filter(|e| matches!(e, PinnedContentEntry::Image { .. }))
        .count();
    let total_additions: usize = entries
        .iter()
        .map(|e| match e {
            PinnedContentEntry::Diff { additions, .. } => *additions,
            _ => 0,
        })
        .sum();
    let total_deletions: usize = entries
        .iter()
        .map(|e| match e {
            PinnedContentEntry::Diff { deletions, .. } => *deletions,
            _ => 0,
        })
        .sum();

    let mut title_parts = vec![Span::styled(" pinned ", Style::default().fg(tool_color()))];
    if total_diffs > 0 {
        title_parts.push(Span::styled(
            format!("+{}", total_additions),
            Style::default().fg(diff_add_color()),
        ));
        title_parts.push(Span::styled(" ", Style::default().fg(dim_color())));
        title_parts.push(Span::styled(
            format!("-{}", total_deletions),
            Style::default().fg(diff_del_color()),
        ));
        title_parts.push(Span::styled(
            format!(" {}f", total_diffs),
            Style::default().fg(dim_color()),
        ));
    }
    if total_images > 0 {
        if total_diffs > 0 {
            title_parts.push(Span::styled(" ", Style::default().fg(dim_color())));
        }
        title_parts.push(Span::styled(
            format!("📷{}", total_images),
            Style::default().fg(dim_color()),
        ));
    }
    title_parts.push(Span::styled(" ", Style::default().fg(dim_color())));

    let border_color = if focused { tool_color() } else { dim_color() };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(Line::from(title_parts));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let mut text_lines: Vec<Line<'static>> = Vec::new();

    struct ImagePlacement {
        after_text_line: usize,
        hash: u64,
        rows: u16,
    }
    let mut image_placements: Vec<ImagePlacement> = Vec::new();

    let has_protocol = mermaid::protocol_type().is_some();

    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            text_lines.push(Line::from(""));
        }

        match entry {
            PinnedContentEntry::Diff {
                file_path,
                lines: diff_lines,
                additions,
                deletions,
            } => {
                let short_path = file_path
                    .rsplit('/')
                    .take(2)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("/");

                let file_ext = std::path::Path::new(file_path)
                    .extension()
                    .and_then(|e| e.to_str());

                text_lines.push(Line::from(vec![
                    Span::styled("── ", Style::default().fg(dim_color())),
                    Span::styled(
                        short_path,
                        Style::default()
                            .fg(rgb(180, 200, 255))
                            .add_modifier(ratatui::style::Modifier::BOLD),
                    ),
                    Span::styled(" (", Style::default().fg(dim_color())),
                    Span::styled(
                        format!("+{}", additions),
                        Style::default().fg(diff_add_color()),
                    ),
                    Span::styled(" ", Style::default().fg(dim_color())),
                    Span::styled(
                        format!("-{}", deletions),
                        Style::default().fg(diff_del_color()),
                    ),
                    Span::styled(")", Style::default().fg(dim_color())),
                ]));

                for line in diff_lines {
                    let base_color = if line.kind == DiffLineKind::Add {
                        diff_add_color()
                    } else {
                        diff_del_color()
                    };

                    let mut spans: Vec<Span<'static>> = vec![Span::styled(
                        line.prefix.clone(),
                        Style::default().fg(base_color),
                    )];

                    if !line.content.is_empty() {
                        let highlighted = markdown::highlight_line(line.content.as_str(), file_ext);
                        for span in highlighted {
                            let tinted = tint_span_with_diff_color(span, base_color);
                            spans.push(tinted);
                        }
                    }

                    text_lines.push(Line::from(spans));
                }
            }
            PinnedContentEntry::Image {
                file_path,
                hash,
                width: img_w,
                height: img_h,
            } => {
                let short_path = file_path
                    .rsplit('/')
                    .take(2)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("/");

                text_lines.push(Line::from(vec![
                    Span::styled("── 📷 ", Style::default().fg(dim_color())),
                    Span::styled(
                        short_path,
                        Style::default()
                            .fg(rgb(180, 200, 255))
                            .add_modifier(ratatui::style::Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" {}×{}", img_w, img_h),
                        Style::default().fg(dim_color()),
                    ),
                ]));

                if has_protocol {
                    let img_rows = inner.height.min(12).max(4);
                    image_placements.push(ImagePlacement {
                        after_text_line: text_lines.len(),
                        hash: *hash,
                        rows: img_rows,
                    });
                    for _ in 0..img_rows {
                        text_lines.push(Line::from(""));
                    }
                }
            }
        }
    }

    if text_lines.is_empty() {
        text_lines.push(Line::from(Span::styled(
            "No content yet",
            Style::default().fg(dim_color()),
        )));
    }

    let total_lines = text_lines.len();
    PINNED_PANE_TOTAL_LINES.store(total_lines, Ordering::Relaxed);

    let max_scroll = total_lines.saturating_sub(inner.height as usize);
    let clamped_scroll = scroll.min(max_scroll);
    LAST_DIFF_PANE_EFFECTIVE_SCROLL.store(clamped_scroll, Ordering::Relaxed);

    let visible_lines: Vec<Line<'static>> = text_lines.into_iter().skip(clamped_scroll).collect();

    let paragraph = if line_wrap {
        Paragraph::new(visible_lines).wrap(Wrap { trim: false })
    } else {
        Paragraph::new(visible_lines)
    };
    frame.render_widget(paragraph, inner);

    if has_protocol {
        for placement in &image_placements {
            let text_y = placement.after_text_line as u16;
            if text_y < clamped_scroll as u16 {
                continue;
            }
            let y_in_inner = text_y.saturating_sub(clamped_scroll as u16);
            if y_in_inner >= inner.height {
                continue;
            }
            let avail_rows = inner.height.saturating_sub(y_in_inner).min(placement.rows);
            if avail_rows < 2 {
                continue;
            }
            let img_area = Rect {
                x: inner.x,
                y: inner.y + y_in_inner,
                width: inner.width,
                height: avail_rows,
            };
            mermaid::render_image_widget_fit(
                placement.hash,
                img_area,
                frame.buffer_mut(),
                false,
                false,
            );
        }
    }
}
