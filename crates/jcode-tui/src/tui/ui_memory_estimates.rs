use ratatui::text::{Line, Span};
use std::sync::Arc;

#[cfg(test)]
use super::TEST_VISIBLE_COPY_TARGETS;
#[cfg(not(test))]
use super::visible_copy_targets_state;
use super::{
    CopyTarget, CopyTargetKind, EditToolRange, ImageRegion, PreparedChatFrame, PreparedMessages,
    PreparedSection, VisibleCopyTarget, WrappedLineMap, body_cache, full_prep_cache, pinned_ui,
};

fn estimate_lines_bytes(lines: &[Line<'static>]) -> usize {
    lines
        .iter()
        .map(|line| {
            std::mem::size_of::<Line<'static>>()
                + line.spans.capacity() * std::mem::size_of::<Span<'static>>()
                + line
                    .spans
                    .iter()
                    .map(|span| span.content.len())
                    .sum::<usize>()
        })
        .sum()
}

fn estimate_arc_string_vec_bytes(values: &Arc<Vec<String>>) -> usize {
    std::mem::size_of::<Vec<String>>()
        + values.capacity() * std::mem::size_of::<String>()
        + values.iter().map(|value| value.capacity()).sum::<usize>()
}

fn estimate_arc_usize_vec_bytes(values: &Arc<Vec<usize>>) -> usize {
    std::mem::size_of::<Vec<usize>>() + values.capacity() * std::mem::size_of::<usize>()
}

fn estimate_arc_wrapped_line_map_bytes(values: &Arc<Vec<WrappedLineMap>>) -> usize {
    std::mem::size_of::<Vec<WrappedLineMap>>()
        + values.capacity() * std::mem::size_of::<WrappedLineMap>()
}

fn estimate_copy_target_kind_bytes(kind: &CopyTargetKind) -> usize {
    match kind {
        CopyTargetKind::CodeBlock { language } => {
            language.as_ref().map(|value| value.capacity()).unwrap_or(0)
        }
        CopyTargetKind::Blockquote => 0,
        CopyTargetKind::Math { .. } => 0,
        CopyTargetKind::Error => 0,
        CopyTargetKind::ToolOutput => 0,
    }
}

fn estimate_copy_targets_bytes(values: &Vec<CopyTarget>) -> usize {
    values
        .iter()
        .map(|target| estimate_copy_target_kind_bytes(&target.kind) + target.content.capacity())
        .sum::<usize>()
        + values.capacity() * std::mem::size_of::<CopyTarget>()
}

fn estimate_edit_tool_ranges_bytes(values: &Vec<EditToolRange>) -> usize {
    values
        .iter()
        .map(|range| range.file_path.capacity())
        .sum::<usize>()
        + values.capacity() * std::mem::size_of::<EditToolRange>()
}

fn estimate_string_vec_bytes(values: &Vec<String>) -> usize {
    values.iter().map(|value| value.capacity()).sum::<usize>()
        + values.capacity() * std::mem::size_of::<String>()
}

fn estimate_image_regions_bytes(values: &Vec<ImageRegion>) -> usize {
    values.capacity() * std::mem::size_of::<ImageRegion>()
}

fn estimate_usize_vec_bytes(values: &Vec<usize>) -> usize {
    values.capacity() * std::mem::size_of::<usize>()
}

pub(super) fn estimate_prepared_messages_bytes(prepared: &PreparedMessages) -> usize {
    estimate_lines_bytes(&prepared.wrapped_lines)
        + estimate_arc_string_vec_bytes(&prepared.wrapped_plain_lines)
        + estimate_arc_usize_vec_bytes(&prepared.wrapped_copy_offsets)
        + estimate_arc_string_vec_bytes(&prepared.raw_plain_lines)
        + estimate_arc_wrapped_line_map_bytes(&prepared.wrapped_line_map)
        + estimate_usize_vec_bytes(&prepared.wrapped_user_indices)
        + estimate_usize_vec_bytes(&prepared.wrapped_user_prompt_starts)
        + estimate_usize_vec_bytes(&prepared.wrapped_user_prompt_ends)
        + estimate_string_vec_bytes(&prepared.user_prompt_texts)
        + estimate_image_regions_bytes(&prepared.image_regions)
        + estimate_edit_tool_ranges_bytes(&prepared.edit_tool_ranges)
        + estimate_copy_targets_bytes(&prepared.copy_targets)
}

/// Sections share immutable `Arc<PreparedMessages>` bodies across frames.
/// Walking a large body (tens of thousands of lines) on every streaming frame
/// was ~10% of release-build client CPU, so remember the estimate per Arc.
/// `Weak` keeps the pointer from being reused for a different allocation.
fn estimate_shared_prepared_messages_bytes(prepared: &Arc<PreparedMessages>) -> usize {
    use std::cell::RefCell;
    use std::sync::Weak;
    const MEMO_CAP: usize = 8;
    thread_local! {
        static MEMO: RefCell<Vec<(Weak<PreparedMessages>, usize)>> = const { RefCell::new(Vec::new()) };
    }
    MEMO.with(|memo| {
        let mut memo = memo.borrow_mut();
        if let Some((_, bytes)) = memo
            .iter()
            .find(|(weak, _)| std::ptr::eq(weak.as_ptr(), Arc::as_ptr(prepared)))
        {
            return *bytes;
        }
        let bytes = estimate_prepared_messages_bytes(prepared);
        memo.retain(|(weak, _)| weak.strong_count() > 0);
        if memo.len() >= MEMO_CAP {
            memo.remove(0);
        }
        memo.push((Arc::downgrade(prepared), bytes));
        bytes
    })
}

pub(super) fn estimate_prepared_chat_frame_bytes(prepared: &PreparedChatFrame) -> usize {
    // Include each section's prepared content: the wrapped lines dominate a
    // frame's real memory footprint. Without this, multi-megabyte transcripts
    // estimate as a few hundred bytes, the oversized-entry path is dead code,
    // and the FULL_PREP_CACHE_MAX_BYTES budget cannot bound anything.
    prepared
        .sections
        .iter()
        .map(|section| estimate_shared_prepared_messages_bytes(&section.prepared))
        .sum::<usize>()
        + prepared.sections.capacity() * std::mem::size_of::<PreparedSection>()
        + estimate_usize_vec_bytes(&prepared.wrapped_user_indices)
        + estimate_usize_vec_bytes(&prepared.wrapped_user_prompt_starts)
        + estimate_usize_vec_bytes(&prepared.wrapped_user_prompt_ends)
        + estimate_string_vec_bytes(&prepared.user_prompt_texts)
        + estimate_image_regions_bytes(&prepared.image_regions)
        + estimate_edit_tool_ranges_bytes(&prepared.edit_tool_ranges)
        + estimate_copy_targets_bytes(&prepared.copy_targets)
}

fn estimate_visible_copy_targets_bytes(values: &Vec<VisibleCopyTarget>) -> usize {
    values
        .iter()
        .map(|target| {
            target.kind_label.capacity()
                + target.copied_notice.capacity()
                + target.content.capacity()
        })
        .sum::<usize>()
        + values.capacity() * std::mem::size_of::<VisibleCopyTarget>()
}

fn prepared_messages_breakdown(prepared: &PreparedMessages) -> serde_json::Value {
    let line_spans: usize = prepared
        .wrapped_lines
        .iter()
        .map(|line| line.spans.capacity() * std::mem::size_of::<Span<'static>>())
        .sum();
    let span_text: usize = prepared
        .wrapped_lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.len())
        .sum();
    let span_count: usize = prepared.wrapped_lines.iter().map(|l| l.spans.len()).sum();
    let span_capacity: usize = prepared
        .wrapped_lines
        .iter()
        .map(|l| l.spans.capacity())
        .sum();
    let max_spans_line = prepared
        .wrapped_lines
        .iter()
        .max_by_key(|l| l.spans.len())
        .map(|l| {
            l.spans
                .iter()
                .take(12)
                .map(|s| s.content.to_string())
                .collect::<Vec<_>>()
        });
    serde_json::json!({
        "wrapped_lines_count": prepared.wrapped_lines.len(),
        "span_count": span_count,
        "span_capacity": span_capacity,
        "max_spans_line_sample": max_spans_line,
        "max_spans_line_len": prepared.wrapped_lines.iter().map(|l| l.spans.len()).max(),
        "line_headers_bytes": prepared.wrapped_lines.capacity() * std::mem::size_of::<Line<'static>>(),
        "span_structs_bytes": line_spans,
        "span_text_bytes": span_text,
        "wrapped_plain_lines_bytes": estimate_arc_string_vec_bytes(&prepared.wrapped_plain_lines),
        "raw_plain_lines_bytes": estimate_arc_string_vec_bytes(&prepared.raw_plain_lines),
        "wrapped_copy_offsets_bytes": estimate_arc_usize_vec_bytes(&prepared.wrapped_copy_offsets),
        "wrapped_line_map_bytes": estimate_arc_wrapped_line_map_bytes(&prepared.wrapped_line_map),
        "copy_targets_bytes": estimate_copy_targets_bytes(&prepared.copy_targets),
        "user_prompt_texts_bytes": estimate_string_vec_bytes(&prepared.user_prompt_texts),
    })
}

pub(crate) fn debug_memory_profile() -> serde_json::Value {
    use std::collections::HashSet;

    let (body_entries_count, body_msg_count_sum, body_unique_prepared_bytes, body_breakdown) = {
        let cache = body_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let breakdown = cache
            .entries
            .iter()
            .max_by_key(|entry| entry.msg_count)
            .map(|entry| prepared_messages_breakdown(&entry.prepared));
        let mut seen = HashSet::new();
        let mut unique_bytes = 0usize;
        let mut msg_count_sum = 0usize;
        for entry in &cache.entries {
            msg_count_sum += entry.msg_count;
            let ptr = Arc::as_ptr(&entry.prepared) as usize;
            if seen.insert(ptr) {
                unique_bytes += estimate_prepared_messages_bytes(&entry.prepared);
            }
        }
        (cache.entries.len(), msg_count_sum, unique_bytes, breakdown)
    };

    let (full_prep_entries_count, full_prep_unique_prepared_bytes) = {
        let cache = full_prep_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut seen = HashSet::new();
        let mut unique_bytes = 0usize;
        for entry in &cache.entries {
            let ptr = Arc::as_ptr(&entry.prepared) as usize;
            if seen.insert(ptr) {
                unique_bytes += estimate_prepared_chat_frame_bytes(&entry.prepared);
            }
        }
        (cache.entries.len(), unique_bytes)
    };

    let visible_copy_targets_bytes = {
        #[cfg(test)]
        {
            TEST_VISIBLE_COPY_TARGETS
                .with(|state| estimate_visible_copy_targets_bytes(&state.borrow()))
        }
        #[cfg(not(test))]
        {
            let state = visible_copy_targets_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            estimate_visible_copy_targets_bytes(&state)
        }
    };

    serde_json::json!({
        "body_cache": {
            "entries_count": body_entries_count,
            "messages_count_sum": body_msg_count_sum,
            "unique_prepared_bytes": body_unique_prepared_bytes,
            "largest_entry_breakdown": body_breakdown,
        },
        "full_prep_cache": {
            "entries_count": full_prep_entries_count,
            "unique_prepared_bytes": full_prep_unique_prepared_bytes,
        },
        "visible_copy_targets": {
            "estimate_bytes": visible_copy_targets_bytes,
        },
        "total_estimate_bytes": body_unique_prepared_bytes
            + full_prep_unique_prepared_bytes
            + visible_copy_targets_bytes,
    })
}

pub(crate) fn debug_side_panel_memory_profile() -> serde_json::Value {
    pinned_ui::debug_memory_profile()
}
