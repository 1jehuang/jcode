use super::*;
use std::collections::{HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MessageCacheKey {
    width: u16,
    diff_mode: crate::config::DiffDisplayMode,
    message_hash: u64,
    content_len: usize,
    diagram_mode: crate::config::DiagramDisplayMode,
    centered: bool,
}

#[derive(Default)]
struct MessageCacheState {
    entries: HashMap<MessageCacheKey, Arc<Vec<Line<'static>>>>,
    order: VecDeque<MessageCacheKey>,
}

impl MessageCacheState {
    fn get(&self, key: &MessageCacheKey) -> Option<Vec<Line<'static>>> {
        self.entries.get(key).map(|arc| arc.as_ref().clone())
    }

    fn insert(&mut self, key: MessageCacheKey, lines: Vec<Line<'static>>) {
        let arc = Arc::new(lines);
        if self.entries.contains_key(&key) {
            self.entries.insert(key, arc);
            return;
        }

        self.entries.insert(key.clone(), arc);
        self.order.push_back(key);

        while self.order.len() > MESSAGE_CACHE_LIMIT {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }
}

static MESSAGE_CACHE: OnceLock<Mutex<MessageCacheState>> = OnceLock::new();

fn message_cache() -> &'static Mutex<MessageCacheState> {
    MESSAGE_CACHE.get_or_init(|| Mutex::new(MessageCacheState::default()))
}

const MESSAGE_CACHE_LIMIT: usize = 2048;

pub(super) fn get_cached_message_lines<F>(
    msg: &DisplayMessage,
    width: u16,
    diff_mode: crate::config::DiffDisplayMode,
    render: F,
) -> Vec<Line<'static>>
where
    F: FnOnce(&DisplayMessage, u16, crate::config::DiffDisplayMode) -> Vec<Line<'static>>,
{
    let key = MessageCacheKey {
        width,
        diff_mode,
        message_hash: hash_display_message(msg),
        content_len: msg.content.len(),
        diagram_mode: crate::config::config().display.diagram_mode,
        centered: markdown::center_code_blocks(),
    };

    let mut cache = match message_cache().lock() {
        Ok(c) => c,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(lines) = cache.get(&key) {
        return lines;
    }

    let lines = render(msg, width, diff_mode);
    cache.insert(key, lines.clone());
    lines
}

pub(crate) fn render_assistant_message(
    msg: &DisplayMessage,
    width: u16,
    _diff_mode: crate::config::DiffDisplayMode,
) -> Vec<Line<'static>> {
    let content_width = width as usize;
    let mut lines = markdown::render_markdown_with_width(&msg.content, Some(content_width));
    if !msg.tool_calls.is_empty() {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                msg.tool_calls.join(" "),
                Style::default().fg(accent_color()).dim(),
            ),
        ]));
    }
    lines
}

pub(crate) fn render_tool_message(
    msg: &DisplayMessage,
    width: u16,
    diff_mode: crate::config::DiffDisplayMode,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let Some(ref tc) = msg.tool_data else {
        return lines;
    };

    let centered = markdown::center_code_blocks();

    if tools_ui::is_memory_store_tool(tc) && !msg.content.starts_with("Error:") {
        let content = tc
            .input
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let category = tc
            .input
            .get("category")
            .and_then(|v| v.as_str())
            .or_else(|| tc.input.get("tag").and_then(|v| v.as_str()))
            .unwrap_or("fact");
        let title = format!("🧠 saved ({})", category);
        let border_style = Style::default().fg(rgb(255, 200, 100));
        let text_style = Style::default().fg(dim_color());
        let max_box = (width.saturating_sub(4) as usize).min(72);
        let inner_width = max_box.saturating_sub(4);

        let mut box_content: Vec<Line<'static>> = Vec::new();
        let text_display_width = unicode_width::UnicodeWidthStr::width(content);
        if text_display_width <= inner_width {
            box_content.push(Line::from(Span::styled(content.to_string(), text_style)));
        } else {
            for chunk in split_by_display_width(content, inner_width) {
                box_content.push(Line::from(Span::styled(chunk, text_style)));
            }
        }

        let box_lines = render_rounded_box(&title, box_content, max_box, border_style);
        for line in box_lines {
            lines.push(line);
        }
        return lines;
    }

    if tools_ui::is_memory_recall_tool(tc) && !msg.content.starts_with("Error:") {
        let border_style = Style::default().fg(rgb(150, 180, 255));
        let text_style = Style::default().fg(dim_color());

        let mut entries: Vec<(String, String)> = Vec::new();
        for line in msg.content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("- [") {
                if let Some(rest) = trimmed.strip_prefix("- [") {
                    if let Some(bracket_end) = rest.find(']') {
                        let cat = rest[..bracket_end].to_string();
                        let content = rest[bracket_end + 1..].trim();
                        let content = if let Some(tag_start) = content.rfind(" [") {
                            content[..tag_start].trim()
                        } else {
                            content
                        };
                        entries.push((cat, content.to_string()));
                    }
                }
            }
        }

        if !entries.is_empty() {
            let count = entries.len();
            let tiles = group_into_tiles(entries);
            let header_text = format!(
                "🧠 recalled {} memor{}",
                count,
                if count == 1 { "y" } else { "ies" }
            );
            let header = Line::from(Span::styled(header_text, border_style));
            let total_width = (width.saturating_sub(4) as usize).min(90);
            let tile_lines =
                render_memory_tiles(&tiles, total_width, border_style, text_style, Some(header));
            for line in tile_lines {
                lines.push(line);
            }
            return lines;
        }
    }

    let summary = tools_ui::get_tool_summary(tc);
    let is_error = msg.content.starts_with("Error:")
        || msg.content.starts_with("error:")
        || msg.content.starts_with("Failed:");

    let (icon, icon_color) = if is_error {
        ("✗", rgb(220, 100, 100))
    } else {
        ("✓", rgb(100, 180, 100))
    };

    let is_edit_tool = matches!(
        tc.name.as_str(),
        "edit" | "Edit" | "write" | "multiedit" | "patch" | "Patch" | "apply_patch" | "ApplyPatch"
    );
    let (additions, deletions) = if is_edit_tool {
        diff_change_counts_for_tool(tc, &msg.content)
    } else {
        (0, 0)
    };

    let mut tool_line = vec![
        Span::styled(format!("  {} ", icon), Style::default().fg(icon_color)),
        Span::styled(tc.name.clone(), Style::default().fg(tool_color())),
        Span::styled(format!(" {}", summary), Style::default().fg(dim_color())),
    ];
    if is_edit_tool {
        tool_line.push(Span::styled(" (", Style::default().fg(dim_color())));
        tool_line.push(Span::styled(
            format!("+{}", additions),
            Style::default().fg(diff_add_color()),
        ));
        tool_line.push(Span::styled(" ", Style::default().fg(dim_color())));
        tool_line.push(Span::styled(
            format!("-{}", deletions),
            Style::default().fg(diff_del_color()),
        ));
        tool_line.push(Span::styled(")", Style::default().fg(dim_color())));
    }

    lines.push(Line::from(tool_line));

    if tc.name == "batch" {
        if let Some(calls) = tc.input.get("tool_calls").and_then(|v| v.as_array()) {
            let sub_results = tools_ui::parse_batch_sub_results(&msg.content);

            for (i, call) in calls.iter().enumerate() {
                let raw_name = call
                    .get("tool")
                    .or_else(|| call.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let display_name = tools_ui::resolve_display_tool_name(raw_name);
                let params = tools_ui::batch_subcall_params(call);

                let sub_tc = ToolCall {
                    id: String::new(),
                    name: display_name.to_string(),
                    input: params,
                    intent: None,
                };
                let sub_summary = tools_ui::get_tool_summary(&sub_tc);

                let sub_errored = sub_results.get(i).copied().unwrap_or(false);
                let (sub_icon, sub_icon_color) = if sub_errored {
                    ("✗", rgb(220, 100, 100))
                } else {
                    ("✓", rgb(100, 180, 100))
                };

                lines.push(Line::from(vec![
                    Span::styled(
                        format!("    {} ", sub_icon),
                        Style::default().fg(sub_icon_color),
                    ),
                    Span::styled(display_name.to_string(), Style::default().fg(tool_color())),
                    Span::styled(
                        format!(" {}", sub_summary),
                        Style::default().fg(dim_color()),
                    ),
                ]));
            }
        }
    }

    if diff_mode.is_inline() && is_edit_tool {
        let file_path_for_ext = tc
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
            });
        let file_ext = file_path_for_ext
            .as_deref()
            .and_then(|p| std::path::Path::new(p).extension())
            .and_then(|e| e.to_str());

        let change_lines = {
            let from_content = collect_diff_lines(&msg.content);
            if !from_content.is_empty() {
                from_content
            } else {
                generate_diff_lines_from_tool_input(tc)
            }
        };

        const MAX_DIFF_LINES: usize = 12;
        let total_changes = change_lines.len();
        let additions = change_lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Add)
            .count();
        let deletions = change_lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Del)
            .count();

        let (display_lines, truncated): (Vec<&ParsedDiffLine>, bool) =
            if total_changes <= MAX_DIFF_LINES {
                (change_lines.iter().collect(), false)
            } else {
                let half = MAX_DIFF_LINES / 2;
                let mut result: Vec<&ParsedDiffLine> = change_lines.iter().take(half).collect();
                result.extend(change_lines.iter().skip(total_changes - half));
                (result, true)
            };

        let pad_str = "";

        lines.push(
            Line::from(Span::styled(
                format!("{}┌─ diff", pad_str),
                Style::default().fg(dim_color()),
            ))
            .alignment(ratatui::layout::Alignment::Left),
        );

        let mut shown_truncation = false;
        let half_point = if truncated {
            MAX_DIFF_LINES / 2
        } else {
            usize::MAX
        };

        for (i, line) in display_lines.iter().enumerate() {
            if truncated && !shown_truncation && i >= half_point {
                let skipped = total_changes - MAX_DIFF_LINES;
                lines.push(
                    Line::from(Span::styled(
                        format!("{}│ ... {} more changes ...", pad_str, skipped),
                        Style::default().fg(dim_color()),
                    ))
                    .alignment(ratatui::layout::Alignment::Left),
                );
                shown_truncation = true;
            }

            let base_color = if line.kind == DiffLineKind::Add {
                diff_add_color()
            } else {
                diff_del_color()
            };

            let border_prefix = format!("{}│ ", pad_str);
            let prefix_visual_width = unicode_width::UnicodeWidthStr::width(border_prefix.as_str())
                + unicode_width::UnicodeWidthStr::width(line.prefix.as_str());
            let max_content_width = (width as usize).saturating_sub(prefix_visual_width + 1);

            let mut spans: Vec<Span<'static>> = vec![
                Span::styled(border_prefix, Style::default().fg(dim_color())),
                Span::styled(line.prefix.clone(), Style::default().fg(base_color)),
            ];

            if !line.content.is_empty() {
                let content = &line.content;
                let content_vis_width = unicode_width::UnicodeWidthStr::width(content.as_str());
                if max_content_width > 1 && content_vis_width > max_content_width {
                    let mut end = 0;
                    let mut vis_w = 0;
                    let limit = max_content_width.saturating_sub(1);
                    for (i, ch) in content.char_indices() {
                        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                        if vis_w + cw > limit {
                            break;
                        }
                        vis_w += cw;
                        end = i + ch.len_utf8();
                    }
                    let truncated = &content[..end];
                    let highlighted = markdown::highlight_line(truncated, file_ext);
                    for span in highlighted {
                        spans.push(tint_span_with_diff_color(span, base_color));
                    }
                    spans.push(Span::styled("…", Style::default().fg(dim_color())));
                } else {
                    let highlighted = markdown::highlight_line(content.as_str(), file_ext);
                    for span in highlighted {
                        spans.push(tint_span_with_diff_color(span, base_color));
                    }
                }
            }

            lines.push(Line::from(spans).alignment(ratatui::layout::Alignment::Left));
        }

        let footer = if total_changes > 0 && truncated {
            format!("{}└─ (+{} -{} total)", pad_str, additions, deletions)
        } else {
            format!("{}└─", pad_str)
        };
        lines.push(
            Line::from(Span::styled(footer, Style::default().fg(dim_color())))
                .alignment(ratatui::layout::Alignment::Left),
        );
    }

    if centered {
        let max_line_width = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                    .sum::<usize>()
            })
            .max()
            .unwrap_or(0);
        let pad = (width as usize).saturating_sub(max_line_width) / 2;
        if pad > 0 {
            let pad_str: String = " ".repeat(pad);
            for line in &mut lines {
                line.spans.insert(0, Span::raw(pad_str.clone()));
                line.alignment = Some(ratatui::layout::Alignment::Left);
            }
        }
    }

    lines
}

fn hash_display_message(msg: &DisplayMessage) -> u64 {
    let mut hasher = DefaultHasher::new();
    msg.role.hash(&mut hasher);
    msg.content.hash(&mut hasher);
    msg.tool_calls.hash(&mut hasher);
    msg.title.hash(&mut hasher);
    if let Some(tool) = &msg.tool_data {
        tool.id.hash(&mut hasher);
        tool.name.hash(&mut hasher);
        hash_json_value(&tool.input, &mut hasher);
    }
    hasher.finish()
}

fn hash_json_value(value: &serde_json::Value, hasher: &mut DefaultHasher) {
    match value {
        serde_json::Value::Null => 0u8.hash(hasher),
        serde_json::Value::Bool(b) => {
            1u8.hash(hasher);
            b.hash(hasher);
        }
        serde_json::Value::Number(n) => {
            2u8.hash(hasher);
            n.hash(hasher);
        }
        serde_json::Value::String(s) => {
            3u8.hash(hasher);
            s.hash(hasher);
        }
        serde_json::Value::Array(arr) => {
            4u8.hash(hasher);
            arr.len().hash(hasher);
            for item in arr {
                hash_json_value(item, hasher);
            }
        }
        serde_json::Value::Object(map) => {
            5u8.hash(hasher);
            map.len().hash(hasher);
            for (k, v) in map {
                k.hash(hasher);
                hash_json_value(v, hasher);
            }
        }
    }
}
