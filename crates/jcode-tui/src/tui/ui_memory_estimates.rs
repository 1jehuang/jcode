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
        .map(|span| match &span.content {
            std::borrow::Cow::Owned(text) => text.capacity(),
            std::borrow::Cow::Borrowed(_) => 0,
        })
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
        "accounting": "component capacities before Arc deduplication; not additive when plain-line vectors alias",
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
        "wrapped_user_indices_bytes": estimate_usize_vec_bytes(&prepared.wrapped_user_indices),
        "wrapped_user_prompt_starts_bytes": estimate_usize_vec_bytes(&prepared.wrapped_user_prompt_starts),
        "wrapped_user_prompt_ends_bytes": estimate_usize_vec_bytes(&prepared.wrapped_user_prompt_ends),
        "image_regions_bytes": estimate_image_regions_bytes(&prepared.image_regions),
        "edit_tool_ranges_bytes": estimate_edit_tool_ranges_bytes(&prepared.edit_tool_ranges),
        "message_boundaries_bytes": prepared.message_boundaries.capacity() * std::mem::size_of::<super::MessageBoundary>(),
    })
}

// Diagnostic-only accounting. Do not use this walker for cache admission: those
// estimates deliberately have their own fast-path/memoization semantics.
#[derive(Default)]
struct DiagnosticAllocations {
    seen: std::collections::HashSet<usize>,
}

pub(super) fn diagnostic_aux_profile<'a>(
    values: impl Iterator<Item = (usize, usize, &'a Vec<String>)>,
    order_entries_count: usize,
) -> serde_json::Value {
    let mut seen = std::collections::HashSet::new();
    let mut entries_count = 0;
    let mut bytes = 0;
    for (pointer, inline_bytes, lines) in values {
        entries_count += 1;
        if seen.insert(pointer) {
            bytes +=
                2 * std::mem::size_of::<usize>() + inline_bytes + estimate_string_vec_bytes(lines);
        }
    }
    serde_json::json!({"entries_count": entries_count, "unique_values_count": seen.len(),
        "order_entries_count": order_entries_count, "unique_prepared_bytes": bytes})
}

impl DiagnosticAllocations {
    fn arc<T>(&mut self, value: &Arc<T>, payload: impl FnOnce(&mut Self, &T) -> usize) -> usize {
        if !self.seen.insert(Arc::as_ptr(value) as usize) {
            return 0;
        }
        // Arc's two reference counters and inline value, excluding allocator
        // rounding/metadata. All other vector helpers count only heap buffers.
        2 * std::mem::size_of::<usize>() + std::mem::size_of::<T>() + payload(self, value)
    }

    fn messages(&mut self, value: &Arc<PreparedMessages>) -> usize {
        self.arc(value, |seen, p| {
            p.wrapped_lines.capacity() * std::mem::size_of::<Line<'static>>()
                + p.wrapped_lines
                    .iter()
                    .map(|line| {
                        line.spans.capacity() * std::mem::size_of::<Span<'static>>()
                            + line
                                .spans
                                .iter()
                                .map(|span| match &span.content {
                                    std::borrow::Cow::Owned(text) => text.capacity(),
                                    std::borrow::Cow::Borrowed(_) => 0,
                                })
                                .sum::<usize>()
                    })
                    .sum::<usize>()
                + seen.arc(&p.wrapped_plain_lines, |_, v| estimate_string_vec_bytes(v))
                + seen.arc(&p.raw_plain_lines, |_, v| estimate_string_vec_bytes(v))
                + seen.arc(&p.wrapped_copy_offsets, |_, v| estimate_usize_vec_bytes(v))
                + seen.arc(&p.wrapped_line_map, |_, v| {
                    v.capacity() * std::mem::size_of::<WrappedLineMap>()
                })
                + estimate_usize_vec_bytes(&p.wrapped_user_indices)
                + estimate_usize_vec_bytes(&p.wrapped_user_prompt_starts)
                + estimate_usize_vec_bytes(&p.wrapped_user_prompt_ends)
                + estimate_string_vec_bytes(&p.user_prompt_texts)
                + estimate_image_regions_bytes(&p.image_regions)
                + estimate_edit_tool_ranges_bytes(&p.edit_tool_ranges)
                + estimate_copy_targets_bytes(&p.copy_targets)
                + p.message_boundaries.capacity() * std::mem::size_of::<super::MessageBoundary>()
        })
    }

    fn frame(&mut self, value: &Arc<PreparedChatFrame>) -> usize {
        self.arc(value, |seen, p| {
            p.sections.capacity() * std::mem::size_of::<PreparedSection>()
                + p.sections
                    .iter()
                    .map(|section| seen.messages(&section.prepared))
                    .sum::<usize>()
                + estimate_usize_vec_bytes(&p.wrapped_user_indices)
                + estimate_usize_vec_bytes(&p.wrapped_user_prompt_starts)
                + estimate_usize_vec_bytes(&p.wrapped_user_prompt_ends)
                + estimate_string_vec_bytes(&p.user_prompt_texts)
                + estimate_image_regions_bytes(&p.image_regions)
                + estimate_edit_tool_ranges_bytes(&p.edit_tool_ranges)
                + estimate_copy_targets_bytes(&p.copy_targets)
        })
    }
}

pub(crate) fn debug_memory_profile() -> serde_json::Value {
    // Keep roots alive throughout the walk so pointer identities cannot be
    // recycled between the body and full-frame snapshots. Lock order matches
    // idle_render_caches_empty.
    let body = body_cache().lock().unwrap_or_else(|p| p.into_inner());
    let full = full_prep_cache().lock().unwrap_or_else(|p| p.into_inner());
    let mut profile = prepared_cache_profile(&body, &full);
    drop(full);
    drop(body);
    let aux = super::prepare::debug_assistant_aux_memory_profile();
    let aux_bytes = aux["unique_prepared_bytes"].as_u64().unwrap_or(0);
    let visible_bytes = {
        #[cfg(test)]
        {
            TEST_VISIBLE_COPY_TARGETS
                .with(|state| estimate_visible_copy_targets_bytes(&state.borrow()))
        }
        #[cfg(not(test))]
        {
            estimate_visible_copy_targets_bytes(
                &visible_copy_targets_state()
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()),
            )
        }
    };
    let total = profile["total_estimate_bytes"].as_u64().unwrap_or(0);
    profile["assistant_aux_cache"] = aux;
    profile["visible_copy_targets"] = serde_json::json!({"estimate_bytes": visible_bytes});
    profile["total_estimate_bytes"] = serde_json::json!(total + aux_bytes + visible_bytes as u64);
    profile
}

fn prepared_cache_profile(
    body: &super::BodyCacheState,
    full: &super::FullPrepCacheState,
) -> serde_json::Value {
    let mut allocations = DiagnosticAllocations::default();

    let (body_entries_count, body_msg_count_sum, body_unique_prepared_bytes, body_breakdown) = {
        let cache = body;
        let breakdown = cache
            .entries
            .iter()
            .chain(cache.oversized_entries.iter())
            .max_by_key(|entry| entry.msg_count)
            .map(|entry| prepared_messages_breakdown(&entry.prepared));
        let mut unique_bytes = 0usize;
        let mut msg_count_sum = 0usize;
        for entry in cache.entries.iter().chain(cache.oversized_entries.iter()) {
            msg_count_sum += entry.msg_count;
            unique_bytes += allocations.messages(&entry.prepared);
        }
        (
            cache.entries.len() + cache.oversized_entries.len(),
            msg_count_sum,
            unique_bytes,
            breakdown,
        )
    };

    let (full_prep_entries_count, full_prep_unique_prepared_bytes) = {
        let cache = full;
        let mut unique_bytes = 0usize;
        for entry in cache.entries.iter().chain(cache.oversized_entries.iter()) {
            unique_bytes += allocations.frame(&entry.prepared);
        }
        (
            cache.entries.len() + cache.oversized_entries.len(),
            unique_bytes,
        )
    };

    serde_json::json!({
        "accounting": "unique reachable prepared allocations, shared Arcs attributed body then full prep; excludes allocator rounding, cache container storage and other UI roots",
        "body_cache": {
            "entries_count": body_entries_count,
            "regular_entries_count": body.entries.len(),
            "oversized_entries_count": body.oversized_entries.len(),
            "messages_count_sum": body_msg_count_sum,
            "unique_prepared_bytes": body_unique_prepared_bytes,
            "largest_entry_breakdown": body_breakdown,
        },
        "full_prep_cache": {
            "entries_count": full_prep_entries_count,
            "regular_entries_count": full.entries.len(),
            "oversized_entries_count": full.oversized_entries.len(),
            "unique_prepared_bytes": full_prep_unique_prepared_bytes,
        },
        "total_estimate_bytes": body_unique_prepared_bytes
            + full_prep_unique_prepared_bytes,
    })
}

pub(crate) fn debug_side_panel_memory_profile() -> serde_json::Value {
    pinned_ui::debug_memory_profile()
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn diagnostic_aux_known_capacities_and_aliases() {
        let mut lines = Vec::with_capacity(7);
        lines.push(String::with_capacity(16384));
        let value = Arc::new(lines);
        let profile = diagnostic_aux_profile(
            [&value, &value].into_iter().map(|v| {
                (
                    Arc::as_ptr(v) as usize,
                    size_of::<Vec<String>>() + size_of::<usize>(),
                    v.as_ref(),
                )
            }),
            2,
        );
        assert_eq!(profile["entries_count"], 2);
        assert_eq!(profile["unique_values_count"], 1);
        assert_eq!(profile["order_entries_count"], 2);
        assert_eq!(
            profile["unique_prepared_bytes"],
            3 * size_of::<usize>() + size_of::<Vec<String>>() + 7 * size_of::<String>() + 16384
        );
        let empty = diagnostic_aux_profile(std::iter::empty(), 0);
        assert_eq!(empty["unique_prepared_bytes"], 0);
    }

    fn fixture() -> PreparedMessages {
        let mut text = String::with_capacity(8192);
        text.push_str("owned");
        let mut lines = Vec::with_capacity(7);
        lines.push(Line::from(vec![
            Span::raw(text),
            Span::raw("static borrowed"),
        ]));
        let mut strings = Vec::with_capacity(5);
        strings.push(String::with_capacity(4096));
        let strings = Arc::new(strings);
        PreparedMessages {
            wrapped_lines: lines,
            wrapped_plain_lines: strings.clone(),
            raw_plain_lines: strings,
            wrapped_copy_offsets: Arc::new(Vec::with_capacity(11)),
            wrapped_line_map: Arc::new(Vec::with_capacity(13)),
            wrapped_user_indices: Vec::new(),
            wrapped_user_prompt_starts: Vec::new(),
            wrapped_user_prompt_ends: Vec::new(),
            user_prompt_texts: Vec::new(),
            image_regions: Vec::new(),
            edit_tool_ranges: Vec::new(),
            copy_targets: Vec::new(),
            message_boundaries: Vec::with_capacity(17),
            mermaid_pending_epoch: None,
        }
    }

    #[test]
    fn diagnostic_known_capacities_and_nested_arc_aliases() {
        let p = Arc::new(fixture());
        let counters = 2 * size_of::<usize>();
        let expected = counters
            + size_of::<PreparedMessages>()
            + 7 * size_of::<Line<'static>>()
            + 2 * size_of::<Span<'static>>()
            + 8192
            + counters
            + size_of::<Vec<String>>()
            + 5 * size_of::<String>()
            + 4096
            + counters
            + size_of::<Vec<usize>>()
            + 11 * size_of::<usize>()
            + counters
            + size_of::<Vec<WrappedLineMap>>()
            + 13 * size_of::<WrappedLineMap>()
            + 17 * size_of::<super::super::MessageBoundary>();
        let mut seen = DiagnosticAllocations::default();
        assert_eq!(seen.messages(&p), expected);
        assert_eq!(seen.messages(&p.clone()), 0);
        let cloned = Arc::new((*p).clone());
        let mut independent = DiagnosticAllocations::default();
        let independent_bytes = independent.messages(&cloned);
        let shared_bytes = counters
            + size_of::<Vec<String>>()
            + 5 * size_of::<String>()
            + 4096
            + counters
            + size_of::<Vec<usize>>()
            + 11 * size_of::<usize>()
            + counters
            + size_of::<Vec<WrappedLineMap>>()
            + 13 * size_of::<WrappedLineMap>();
        assert_eq!(seen.messages(&cloned), independent_bytes - shared_bytes);
    }

    #[test]
    fn diagnostic_oversized_and_cross_cache_shared_frames() {
        use super::super::*;
        let p = Arc::new(fixture());
        let frame = Arc::new(PreparedChatFrame::from_single(p.clone()));
        let body_key = BodyCacheKey {
            width: 40,
            diff_mode: crate::config::DiffDisplayMode::Off,
            messages_version: 1,
            diagram_mode: crate::config::DiagramDisplayMode::Pinned,
            centered: false,
            compact_thinking: false,
            mermaid_aspect_bucket: None,
            pin_images: true,
            inline_images_visible: true,
            images_signature: (0, 0),
            expanded_images_version: 0,
            swarm_members_signature: 0,
            latex_rendering: crate::config::LatexRenderingMode::Image,
            show_link_urls: false,
        };
        let full_key = FullPrepCacheKey {
            width: 40,
            height: 20,
            diff_mode: crate::config::DiffDisplayMode::Off,
            messages_version: 1,
            diagram_mode: crate::config::DiagramDisplayMode::Pinned,
            centered: false,
            compact_thinking: false,
            mermaid_aspect_bucket: None,
            is_processing: false,
            streaming_text_len: 0,
            streaming_text_hash: 0,
            batch_progress_hash: 0,
            inline_images_signature: (0, 0),
            inline_images_visible: true,
            expanded_images_version: 0,
            swarm_members_signature: 0,
            latex_rendering: crate::config::LatexRenderingMode::Image,
            show_link_urls: false,
        };
        let entry = BodyCacheEntry {
            key: body_key,
            prepared: p.clone(),
            prepared_bytes: 0,
            msg_count: 3,
            prompt_offset: 0,
        };
        let mut body = BodyCacheState::default();
        body.entries.push_back(entry.clone());
        body.oversized_entries.push_back(entry);
        let entry = FullPrepCacheEntry {
            key: full_key,
            prepared: frame.clone(),
            prepared_bytes: 0,
        };
        let mut full = FullPrepCacheState::default();
        full.entries.push_back(entry.clone());
        full.oversized_entries.push_back(entry);
        let result = prepared_cache_profile(&body, &full);
        let mut seen = DiagnosticAllocations::default();
        let body_bytes = seen.messages(&p);
        let frame_bytes = seen.frame(&frame);
        assert_eq!(result["body_cache"]["entries_count"], 2);
        assert_eq!(result["body_cache"]["oversized_entries_count"], 1);
        assert_eq!(result["body_cache"]["messages_count_sum"], 6);
        assert_eq!(result["body_cache"]["unique_prepared_bytes"], body_bytes);
        assert_eq!(result["full_prep_cache"]["entries_count"], 2);
        assert_eq!(result["full_prep_cache"]["oversized_entries_count"], 1);
        assert_eq!(
            result["full_prep_cache"]["unique_prepared_bytes"],
            frame_bytes
        );
        assert_eq!(result["total_estimate_bytes"], body_bytes + frame_bytes);
        body.entries.clear();
        full.entries.clear();
        let oversized_only = prepared_cache_profile(&body, &full);
        assert_eq!(
            oversized_only["total_estimate_bytes"],
            body_bytes + frame_bytes
        );
        assert_eq!(oversized_only["body_cache"]["messages_count_sum"], 3);
    }
}
