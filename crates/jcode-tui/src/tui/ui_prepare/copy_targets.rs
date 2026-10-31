//! Image geometry and message copy-target preparation.
use super::*;

/// Build the image regions for an image/mermaid placeholder in `wrapped_lines`,
/// where each placeholder "owns" the run of blank lines that follow it.
///
/// Done in a single reverse pass that precomputes, for every line, the length
/// of the blank run starting at that line. The previous implementation scanned
/// forward through the trailing blanks for every placeholder, which is O(L^2)
/// when a message has many placeholders each followed by long blank runs.
pub(crate) fn compute_image_regions(
    wrapped_lines: &[ratatui::text::Line<'static>],
) -> Vec<ImageRegion> {
    fn is_blank_line(line: &ratatui::text::Line<'static>) -> bool {
        line.spans.is_empty() || (line.spans.len() == 1 && line.spans[0].content.is_empty())
    }

    let len = wrapped_lines.len();
    // blank_run[i] = number of consecutive blank lines starting at index i.
    let mut blank_run = vec![0usize; len + 1];
    for idx in (0..len).rev() {
        blank_run[idx] = if is_blank_line(&wrapped_lines[idx]) {
            blank_run[idx + 1] + 1
        } else {
            0
        };
    }

    let mut image_regions = Vec::new();
    for (idx, line) in wrapped_lines.iter().enumerate() {
        if let Some(hash) = super::super::mermaid::parse_image_placeholder(line) {
            // The placeholder line plus the blank run immediately after it.
            let height = (1 + blank_run[idx + 1]).min(u16::MAX as usize) as u16;
            image_regions.push(ImageRegion {
                abs_line_idx: idx,
                end_line: idx + height as usize,
                hash,
                height,
                // Mermaid crop regions don't know their rendered width here;
                // 0 = treat the rows as fully occupied for layout purposes.
                width: 0,
                render: jcode_tui_messages::ImageRegionRender::Crop,
            });
        } else if let Some((hash, rows, cols)) =
            super::super::mermaid::parse_inline_image_placeholder(line)
        {
            // Inline raster image anchored in the transcript body. The marker
            // encodes its exact geometry; clamp to the blank run that actually
            // follows so a wrapped/truncated placeholder can never claim
            // non-blank lines below it.
            let available = (1 + blank_run[idx + 1]).min(u16::MAX as usize) as u16;
            // Zero-height markers anchor collapsed tool-image labels for
            // click-to-expand without creating a drawable image region.
            let height = rows.min(available);
            image_regions.push(ImageRegion {
                abs_line_idx: idx,
                end_line: idx + height as usize,
                hash,
                height,
                width: cols,
                render: jcode_tui_messages::ImageRegionRender::Fit,
            });
        }
    }
    image_regions
}

pub(crate) fn error_copy_target(
    content: &str,
    rendered_line_count: usize,
) -> Option<RawCopyTarget> {
    copy_target_for_kind(CopyTargetKind::Error, content, rendered_line_count)
}

pub(crate) fn tool_output_copy_target(
    content: &str,
    rendered_line_count: usize,
) -> Option<RawCopyTarget> {
    copy_target_for_kind(CopyTargetKind::ToolOutput, content, rendered_line_count)
}

pub(crate) fn copy_target_for_kind(
    kind: CopyTargetKind,
    content: &str,
    rendered_line_count: usize,
) -> Option<RawCopyTarget> {
    let content = content.trim();
    if content.is_empty() {
        return None;
    }

    Some(RawCopyTarget {
        kind,
        content: content.to_string(),
        start_raw_line: 0,
        end_raw_line: rendered_line_count.max(1),
        badge_raw_line: 0,
    })
}

pub(crate) fn offset_copy_target(target: RawCopyTarget, line_offset: usize) -> RawCopyTarget {
    RawCopyTarget {
        kind: target.kind,
        content: target.content,
        start_raw_line: target.start_raw_line + line_offset,
        end_raw_line: target.end_raw_line + line_offset,
        badge_raw_line: target.badge_raw_line + line_offset,
    }
}

pub(crate) fn assistant_message_copy_targets(
    content: &str,
    rendered_lines: &[Line<'static>],
) -> Vec<RawCopyTarget> {
    if is_error_copy_content(content) {
        return error_copy_target(content, rendered_lines.len())
            .into_iter()
            .collect();
    }

    crate::tui::markdown::extract_copy_targets_from_rendered_lines(rendered_lines)
}

pub(crate) fn tool_message_copy_target(
    msg: &DisplayMessage,
    rendered_line_count: usize,
) -> Option<RawCopyTarget> {
    if is_error_copy_content(&msg.content) {
        return error_copy_target(&msg.content, rendered_line_count);
    }
    if tools_ui::tool_output_looks_failed(&msg.content) {
        return tool_output_copy_target(&msg.content, rendered_line_count);
    }
    None
}
