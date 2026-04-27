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

pub(super) fn estimate_prepared_chat_frame_bytes(prepared: &PreparedChatFrame) -> usize {
    prepared.sections.capacity() * std::mem::size_of::<PreparedSection>()
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

pub(crate) fn debug_memory_profile() -> serde_json::Value {
    use std::collections::HashSet;

    let (body_entries_count, body_msg_count_sum, body_unique_prepared_bytes) = {
        let cache = body_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        (cache.entries.len(), msg_count_sum, unique_bytes)
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
