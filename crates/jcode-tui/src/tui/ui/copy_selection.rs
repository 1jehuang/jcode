use unicode_width::UnicodeWidthStr;

use super::display_width::{
    clamp_display_col, display_col_slice, display_col_slice_copy, line_display_width,
};
use super::url_regex_support::link_target_for_display_column;
use super::{CopyViewportData, CopyViewportSnapshot};

pub(super) fn copy_point_from_snapshot(
    snapshot: &CopyViewportSnapshot,
    column: u16,
    row: u16,
) -> Option<crate::tui::CopySelectionPoint> {
    let area = snapshot.content_area;
    if row < area.y
        || row >= area.y.saturating_add(area.height)
        || column < area.x
        || column >= area.x.saturating_add(area.width)
    {
        return None;
    }

    let rel_row = row.saturating_sub(area.y) as usize;
    let abs_line = snapshot.scroll.saturating_add(rel_row);
    if abs_line >= snapshot.visible_end || abs_line >= snapshot.wrapped_plain_line_count() {
        return None;
    }

    let left_margin = snapshot.left_margins.get(rel_row).copied().unwrap_or(0);
    let content_x = area.x.saturating_add(left_margin);
    let rel_col = column.saturating_sub(content_x) as usize;
    let text = snapshot.wrapped_plain_line(abs_line)?;
    let copy_start = snapshot.wrapped_copy_offset(abs_line).unwrap_or(0);
    Some(crate::tui::CopySelectionPoint {
        pane: snapshot.pane,
        abs_line,
        column: clamp_display_col(text, rel_col).max(copy_start),
    })
}

#[derive(Clone, Copy, Debug)]
struct RawSelectionPoint {
    raw_line: usize,
    column: usize,
}

#[cfg(test)]
pub(super) fn copy_selection_text_from_raw_lines(
    snapshot: &CopyViewportSnapshot,
    start: crate::tui::CopySelectionPoint,
    end: crate::tui::CopySelectionPoint,
) -> Option<String> {
    if let Some(text) = copy_selection_text_with_math_targets(snapshot, start, end) {
        return Some(text);
    }
    copy_selection_text_from_raw_lines_base(snapshot, start, end)
}

/// Full copy text for a selection: semantic math substitution first, then the
/// raw logical-line path, then the wrapped display-line fallback (used when a
/// selection starts on chrome such as the session header that has no raw map).
pub(super) fn copy_selection_text_any(
    snapshot: &CopyViewportSnapshot,
    start: crate::tui::CopySelectionPoint,
    end: crate::tui::CopySelectionPoint,
) -> Option<String> {
    if let Some(text) = copy_selection_text_with_math_targets(snapshot, start, end) {
        return Some(text);
    }
    copy_segment_text(snapshot, start, end)
}

fn copy_segment_text(
    snapshot: &CopyViewportSnapshot,
    start: crate::tui::CopySelectionPoint,
    end: crate::tui::CopySelectionPoint,
) -> Option<String> {
    copy_selection_text_from_raw_lines_base(snapshot, start, end)
        .or_else(|| copy_selection_text_from_wrapped_lines(snapshot, start, end))
}

/// Display-line copy path. Mirrors the raw path but slices wrapped lines and
/// honors per-line copy offsets that skip gutter chrome.
pub(super) fn copy_selection_text_from_wrapped_lines(
    snapshot: &CopyViewportSnapshot,
    start: crate::tui::CopySelectionPoint,
    end: crate::tui::CopySelectionPoint,
) -> Option<String> {
    let mut out = String::new();
    for abs_line in start.abs_line..=end.abs_line {
        if abs_line > start.abs_line {
            out.push('\n');
        }
        let text = snapshot.wrapped_plain_line(abs_line)?;
        let copy_start = snapshot.wrapped_copy_offset(abs_line).unwrap_or(0);
        let start_col = if abs_line == start.abs_line {
            clamp_display_col(text, start.column).max(copy_start)
        } else {
            copy_start
        };
        let end_col = if abs_line == end.abs_line {
            clamp_display_col(text, end.column).max(copy_start)
        } else {
            line_display_width(text)
        };
        if end_col < start_col {
            continue;
        }
        out.push_str(&semantic_slice(text, start_col, end_col));
    }
    Some(out)
}

/// Slice `text` between display columns, replacing every fully selected
/// inline-math span with its `$source$` LaTeX form. A span that is only partly
/// highlighted copies the highlighted glyphs, like any other text.
fn semantic_slice(text: &str, start_col: usize, end_col: usize) -> std::borrow::Cow<'_, str> {
    let Some(spans) = jcode_tui_markdown::inline_math_spans_for_plain_line(text) else {
        return std::borrow::Cow::Borrowed(display_col_slice_copy(text, start_col, end_col));
    };
    let mut out = String::new();
    let mut col = start_col;
    let mut replaced = false;
    for span in spans.iter() {
        if span.start_col < start_col || span.end_col > end_col {
            continue;
        }
        if span.start_col > col {
            out.push_str(display_col_slice(text, col, span.start_col));
        }
        out.push('$');
        out.push_str(&span.source);
        out.push('$');
        replaced = true;
        col = span.end_col.max(col);
    }
    if !replaced {
        return std::borrow::Cow::Borrowed(display_col_slice_copy(text, start_col, end_col));
    }
    if col < end_col {
        out.push_str(display_col_slice_copy(text, col, end_col));
    }
    std::borrow::Cow::Owned(out)
}

/// How frame chrome on one raw line is treated when it is part of a selection.
#[derive(Clone, Copy)]
enum FrameChrome {
    /// A `│ ` gutter of this display width precedes the content.
    Gutter(usize),
    /// A `┌─ label` / `└─` border row with no content of its own.
    Border,
}

/// Frame chrome for the raw lines a selection touches, keyed by raw line.
///
/// Code blocks, quotes and text math frames are drawn with a `│ ` gutter and
/// (for frames) `┌─` / `└─` border rows. None of that is content, so a
/// selection copies the text inside the frame, the same text its copy badge
/// would give for the whole block.
fn frame_chrome_for_selection(
    snapshot: &CopyViewportSnapshot,
    start: crate::tui::CopySelectionPoint,
    end: crate::tui::CopySelectionPoint,
) -> Vec<(usize, FrameChrome)> {
    use jcode_tui_markdown::CopyTargetKind;
    let CopyViewportData::ChatFrame { prepared } = &snapshot.data else {
        return Vec::new();
    };
    let mut chrome = Vec::new();
    for target in prepared.copy_targets.iter() {
        let framed = match &target.kind {
            CopyTargetKind::CodeBlock { .. } | CopyTargetKind::Math { .. } => true,
            CopyTargetKind::Blockquote => false,
            CopyTargetKind::Error | CopyTargetKind::ToolOutput => continue,
        };
        let first = target.start_line.max(start.abs_line);
        let last = target.end_line.min(end.abs_line.saturating_add(1));
        for abs_line in first..last {
            let Some(map) = snapshot.wrapped_line_map(abs_line) else {
                continue;
            };
            if chrome.last().is_some_and(|(raw, _)| *raw == map.raw_line) {
                continue;
            }
            let Some(text) = snapshot.raw_plain_line(map.raw_line) else {
                continue;
            };
            let body = text.trim_start();
            let entry = if framed && (body.starts_with("┌─") || body.starts_with("└─")) {
                FrameChrome::Border
            } else {
                let mut rest = body;
                while let Some(next) = rest.strip_prefix("│ ") {
                    rest = next;
                    if framed {
                        break;
                    }
                }
                if rest == "│" {
                    rest = "";
                }
                if rest.len() == body.len() {
                    continue;
                }
                FrameChrome::Gutter(line_display_width(text) - line_display_width(rest))
            };
            chrome.push((map.raw_line, entry));
        }
    }
    chrome.sort_by_key(|(raw, _)| *raw);
    chrome
}

fn frame_chrome_at(chrome: &[(usize, FrameChrome)], raw_line: usize) -> Option<FrameChrome> {
    chrome
        .binary_search_by_key(&raw_line, |(raw, _)| *raw)
        .ok()
        .map(|idx| chrome[idx].1)
}

fn copy_selection_text_from_raw_lines_base(
    snapshot: &CopyViewportSnapshot,
    start: crate::tui::CopySelectionPoint,
    end: crate::tui::CopySelectionPoint,
) -> Option<String> {
    if snapshot.raw_plain_line_count() == 0 || snapshot.wrapped_line_map(start.abs_line).is_none() {
        return None;
    }

    let chrome = frame_chrome_for_selection(snapshot, start, end);
    let start = raw_selection_point(snapshot, start)?;
    let end = raw_selection_point(snapshot, end)?;
    if start.raw_line >= snapshot.raw_plain_line_count()
        || end.raw_line >= snapshot.raw_plain_line_count()
    {
        return None;
    }
    let multi_line = start.raw_line != end.raw_line;

    let selected_lines = end
        .raw_line
        .saturating_sub(start.raw_line)
        .saturating_add(1);
    let mut out = String::new();
    let mut emitted = false;
    for raw_line in start.raw_line..=end.raw_line {
        let gutter = match frame_chrome_at(&chrome, raw_line) {
            // A border row is only copied when it is all that is selected.
            Some(FrameChrome::Border) if multi_line => continue,
            Some(FrameChrome::Gutter(width)) => width,
            _ => 0,
        };
        if emitted {
            out.push('\n');
        }
        emitted = true;
        let text = snapshot.raw_plain_line(raw_line)?;
        if raw_line != start.raw_line && raw_line != end.raw_line {
            if raw_line == start.raw_line + 1 {
                out.reserve(text.len().saturating_mul(selected_lines.min(8)));
            }
            out.push_str(&semantic_slice(text, gutter, line_display_width(text)));
            continue;
        }
        let line_width = line_display_width(text);
        let start_col = if raw_line == start.raw_line {
            clamp_display_col(text, start.column).max(gutter)
        } else {
            gutter
        };
        let end_col = if raw_line == end.raw_line {
            clamp_display_col(text, end.column)
        } else {
            line_width
        };

        if end_col < start_col {
            continue;
        }

        let slice = semantic_slice(text, start_col, end_col);
        if raw_line == start.raw_line {
            out.reserve(slice.len().saturating_mul(selected_lines.min(8)));
        }
        out.push_str(&slice);
    }

    Some(out)
}

/// Whether a selection should copy a display-math target as LaTeX source.
///
/// Image renders have no text under the cursor, so any overlap copies the
/// source. Text (Unicode) frames copy the source only when the whole formula
/// body is highlighted. A partial highlight copies the highlighted glyphs.
fn math_target_copies_source(
    snapshot: &CopyViewportSnapshot,
    target: &jcode_tui_messages::CopyTarget,
    start: crate::tui::CopySelectionPoint,
    end: crate::tui::CopySelectionPoint,
) -> bool {
    let mut first = None;
    let mut last = None;
    for line in target.start_line..target.end_line {
        let Some(text) = snapshot.wrapped_plain_line(line) else {
            continue;
        };
        if text.contains("\0IIMG:") {
            return true;
        }
        let trimmed = text.trim();
        if line == target.start_line || trimmed.is_empty() || trimmed.starts_with("└─") {
            continue;
        }
        first.get_or_insert(line);
        last = Some(line);
    }
    let (Some(first), Some(last)) = (first, last) else {
        return true;
    };
    // The formula body starts after the frame gutter, so a drag that begins
    // on the first glyph (not on the `│ ` chrome) still selects all of it.
    let first_col = snapshot
        .wrapped_plain_line(first)
        .map(|text| {
            let body = text.trim_start();
            let body = body.strip_prefix("│ ").unwrap_or(body).trim_start();
            line_display_width(text) - line_display_width(body)
        })
        .unwrap_or(0)
        .max(snapshot.wrapped_copy_offset(first).unwrap_or(0));
    let last_col = snapshot
        .wrapped_plain_line(last)
        .map(|text| line_display_width(text.trim_end()))
        .unwrap_or(0);
    (start.abs_line, start.column) <= (first, first_col)
        && (last, last_col) <= (end.abs_line, end.column)
}

/// Replace selected display math with its semantic LaTeX source. Normal spans
/// on either side still use the raw logical-line path, so wrapped prose
/// retains the same copy behavior it has without math.
fn copy_selection_text_with_math_targets(
    snapshot: &CopyViewportSnapshot,
    start: crate::tui::CopySelectionPoint,
    end: crate::tui::CopySelectionPoint,
) -> Option<String> {
    let prepared = match &snapshot.data {
        CopyViewportData::ChatFrame { prepared } => prepared,
        CopyViewportData::Dense { .. } => return None,
    };
    let math_targets: Vec<_> = prepared
        .copy_targets
        .iter()
        .filter(|target| {
            matches!(
                &target.kind,
                jcode_tui_markdown::CopyTargetKind::Math { .. }
            )
                // Selection points are half-open display positions. Ending at
                // column zero of the label does not select the image; dragging
                // into any part of its label/body does.
                && (target.start_line, 0) < (end.abs_line, end.column)
                && (start.abs_line, start.column) < (target.end_line, 0)
                && math_target_copies_source(snapshot, target, start, end)
        })
        .collect();
    if math_targets.is_empty() {
        return None;
    }

    let mut parts = Vec::with_capacity(math_targets.len().saturating_mul(2) + 1);
    let mut cursor = start;
    for target in math_targets {
        if cursor.abs_line < target.start_line {
            let last_line = target.start_line - 1;
            let last_col = snapshot
                .wrapped_plain_line(last_line)
                .map(line_display_width)
                .unwrap_or(0);
            let normal_end = crate::tui::CopySelectionPoint {
                pane: cursor.pane,
                abs_line: last_line,
                column: last_col,
            };
            parts.push(copy_segment_text(snapshot, cursor, normal_end)?);
        }
        parts.push(target.content.clone());
        cursor = crate::tui::CopySelectionPoint {
            pane: cursor.pane,
            abs_line: target.end_line,
            column: 0,
        };
    }

    if (cursor.abs_line, cursor.column) <= (end.abs_line, end.column)
        && cursor.abs_line < snapshot.wrapped_plain_line_count()
    {
        parts.push(copy_segment_text(snapshot, cursor, end)?);
    }
    Some(parts.join("\n"))
}

/// Selection metrics (character count and line count) for the raw-lines path,
/// computed without allocating the full joined selection string. Mirrors the
/// slicing in [`copy_selection_text_from_raw_lines`] exactly so the displayed
/// "N chars · M lines" matches what would actually be copied.
pub(super) fn copy_selection_metrics_from_raw_lines(
    snapshot: &CopyViewportSnapshot,
    start: crate::tui::CopySelectionPoint,
    end: crate::tui::CopySelectionPoint,
) -> Option<(usize, usize)> {
    if let Some(text) = copy_selection_text_with_math_targets(snapshot, start, end) {
        return Some((text.chars().count(), text.split('\n').count().max(1)));
    }
    if snapshot.raw_plain_line_count() == 0 || snapshot.wrapped_line_map(start.abs_line).is_none() {
        return None;
    }

    let chrome = frame_chrome_for_selection(snapshot, start, end);
    let start = raw_selection_point(snapshot, start)?;
    let end = raw_selection_point(snapshot, end)?;
    if start.raw_line >= snapshot.raw_plain_line_count()
        || end.raw_line >= snapshot.raw_plain_line_count()
    {
        return None;
    }
    let multi_line = start.raw_line != end.raw_line;

    let mut chars = 0usize;
    let mut lines = 0usize;
    for raw_line in start.raw_line..=end.raw_line {
        let gutter = match frame_chrome_at(&chrome, raw_line) {
            Some(FrameChrome::Border) if multi_line => continue,
            Some(FrameChrome::Gutter(width)) => width,
            _ => 0,
        };
        if lines > 0 {
            chars += 1; // the joining '\n'
        }
        lines += 1;
        let text = snapshot.raw_plain_line(raw_line)?;
        if raw_line != start.raw_line && raw_line != end.raw_line {
            chars += semantic_slice(text, gutter, line_display_width(text))
                .chars()
                .count();
            continue;
        }
        let line_width = line_display_width(text);
        let start_col = if raw_line == start.raw_line {
            clamp_display_col(text, start.column).max(gutter)
        } else {
            gutter
        };
        let end_col = if raw_line == end.raw_line {
            clamp_display_col(text, end.column)
        } else {
            line_width
        };
        if end_col < start_col {
            continue;
        }
        chars += semantic_slice(text, start_col, end_col).chars().count();
    }

    Some((chars, lines.max(1)))
}

pub(super) fn link_target_from_snapshot(
    snapshot: &CopyViewportSnapshot,
    point: crate::tui::CopySelectionPoint,
) -> Option<String> {
    let raw_point = raw_selection_point(snapshot, point)?;
    let raw_text = snapshot.raw_plain_line(raw_point.raw_line)?;
    link_target_for_display_column(raw_text, raw_point.column)
}

/// `(raw_line, display_column)` of a selection point in the snapshot's
/// unwrapped (logical) lines. For the composer these are the `\n`-separated
/// lines of the input, so soft wraps and the prompt prefix are already gone.
pub(super) fn raw_line_column_from_snapshot(
    snapshot: &CopyViewportSnapshot,
    point: crate::tui::CopySelectionPoint,
) -> Option<(usize, usize)> {
    raw_selection_point(snapshot, point).map(|raw| (raw.raw_line, raw.column))
}

fn raw_selection_point(
    snapshot: &CopyViewportSnapshot,
    point: crate::tui::CopySelectionPoint,
) -> Option<RawSelectionPoint> {
    let wrapped_text = snapshot.wrapped_plain_line(point.abs_line)?;
    let map = snapshot.wrapped_line_map(point.abs_line)?;
    let display_copy_start = snapshot
        .wrapped_copy_offset(point.abs_line)
        .unwrap_or(0)
        .min(wrapped_text.width());
    let local_col = clamp_display_col(wrapped_text, point.column).max(display_copy_start);
    let segment_width = map.end_col.saturating_sub(map.start_col);
    Some(RawSelectionPoint {
        raw_line: map.raw_line,
        column: map.start_col
            + local_col
                .saturating_sub(display_copy_start)
                .min(segment_width),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_tui_markdown::CopyTargetKind;
    use jcode_tui_messages::{CopyTarget, PreparedChatFrame, PreparedMessages, WrappedLineMap};
    use ratatui::layout::Rect;
    use ratatui::text::Line;
    use std::sync::Arc;

    fn math_snapshot() -> CopyViewportSnapshot {
        math_snapshot_with(&[
            "before",
            "  math",
            "\0IIMG:000000001a7ec0de:0002:0014\0",
            "",
            "after",
        ])
    }

    /// A Unicode (text) display-math frame, as drawn without a TeX toolchain.
    fn unicode_math_snapshot() -> CopyViewportSnapshot {
        math_snapshot_with(&["before", "┌─ math", "│ x² + α", "└─", "after"])
    }

    fn math_snapshot_with(lines: &[&str]) -> CopyViewportSnapshot {
        let plain: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
        let wrapped_lines = plain.iter().cloned().map(Line::from).collect();
        let maps = plain
            .iter()
            .enumerate()
            .map(|(raw_line, text)| WrappedLineMap {
                raw_line,
                start_col: 0,
                end_col: line_display_width(text),
            })
            .collect();
        let prepared = PreparedMessages {
            wrapped_lines,
            wrapped_plain_lines: Arc::new(plain.clone()),
            wrapped_copy_offsets: Arc::new(vec![0; plain.len()]),
            raw_plain_lines: Arc::new(plain.clone()),
            wrapped_line_map: Arc::new(maps),
            wrapped_user_indices: Vec::new(),
            wrapped_user_prompt_starts: Vec::new(),
            wrapped_user_prompt_ends: Vec::new(),
            user_prompt_texts: Vec::new(),
            image_regions: Vec::new(),
            edit_tool_ranges: Vec::new(),
            copy_targets: vec![CopyTarget {
                kind: CopyTargetKind::Math { display: true },
                content: "$$\nx^2 + \\alpha\n$$".to_string(),
                start_line: 1,
                end_line: 4,
                badge_line: 1,
            }],
            message_boundaries: Vec::new(),
            mermaid_pending_epoch: None,
        };
        CopyViewportSnapshot {
            pane: crate::tui::CopySelectionPane::Chat,
            data: CopyViewportData::ChatFrame {
                prepared: Arc::new(PreparedChatFrame::from_single(Arc::new(prepared))),
            },
            scroll: 0,
            visible_end: plain.len(),
            content_area: Rect::new(0, 0, 80, plain.len() as u16),
            left_margins: vec![0; plain.len()],
        }
    }

    fn point(line: usize, column: usize) -> crate::tui::CopySelectionPoint {
        crate::tui::CopySelectionPoint {
            pane: crate::tui::CopySelectionPane::Chat,
            abs_line: line,
            column,
        }
    }

    #[test]
    fn selection_replaces_latex_image_rows_with_source_and_preserves_neighbors() {
        let snapshot = math_snapshot();
        let copied = copy_selection_text_from_raw_lines(&snapshot, point(0, 0), point(4, 5))
            .expect("selection should resolve");
        assert_eq!(copied, "before\n$$\nx^2 + \\alpha\n$$\nafter");
        assert!(!copied.contains("IIMG"));
        assert_eq!(
            copy_selection_metrics_from_raw_lines(&snapshot, point(0, 0), point(4, 5)),
            Some((copied.chars().count(), copied.split('\n').count()))
        );
    }

    #[test]
    fn selection_over_unicode_inline_math_copies_latex_source() {
        let rendered =
            jcode_tui_markdown::render_markdown(r"Identity $e^{i\pi_{7}} + 1 = 0$ holds");
        let line = rendered
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .find(|text| text.starts_with("Identity"))
            .expect("prose line");
        let width = line_display_width(&line);
        assert_eq!(
            semantic_slice(&line, 0, width),
            r"Identity $e^{i\pi_{7}} + 1 = 0$ holds"
        );
        // A partial drag inside the formula copies only the highlighted glyphs.
        assert_eq!(
            semantic_slice(&line, 10, 12),
            display_col_slice_copy(&line, 10, 12)
        );
        assert!(!semantic_slice(&line, 10, 12).contains('$'));
        assert_eq!(semantic_slice(&line, 0, 8), "Identity");
    }

    #[test]
    fn selection_inside_latex_image_copies_complete_source() {
        let snapshot = math_snapshot();
        assert_eq!(
            copy_selection_text_from_raw_lines(&snapshot, point(1, 0), point(3, 0)),
            Some("$$\nx^2 + \\alpha\n$$".to_string())
        );
    }

    #[test]
    fn partial_selection_inside_unicode_math_frame_copies_highlighted_glyphs() {
        let snapshot = unicode_math_snapshot();
        assert_eq!(
            copy_selection_text_from_raw_lines(&snapshot, point(2, 2), point(2, 4)),
            Some("x²".to_string())
        );
        assert_eq!(
            copy_selection_metrics_from_raw_lines(&snapshot, point(2, 2), point(2, 4)),
            Some((2, 1))
        );
    }

    #[test]
    fn full_selection_of_unicode_math_frame_copies_latex_source() {
        let snapshot = unicode_math_snapshot();
        let source = "$$\nx^2 + \\alpha\n$$".to_string();
        // From the first glyph, from the gutter, and from the frame label.
        for start in [point(2, 2), point(2, 0), point(1, 0)] {
            assert_eq!(
                copy_selection_text_from_raw_lines(&snapshot, start, point(2, 8)),
                Some(source.clone()),
                "{start:?}"
            );
        }
        assert_eq!(
            copy_selection_text_from_raw_lines(&snapshot, point(0, 0), point(4, 5)),
            Some(format!("before\n{source}\nafter"))
        );
    }
}
