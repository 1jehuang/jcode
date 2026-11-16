//! Bounded prepared previews and transcript fingerprints for the resume hot path.
use super::*;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};

// Additional retained previews, excluding the currently displayed preview. A
// large preview still renders normally but is not retained when navigating away.
pub(super) const RETAINED_PREVIEW_BYTES: usize = 16 * 1024 * 1024;
pub(super) const RETAINED_PREVIEW_ENTRIES: usize = 8;
const FINGERPRINT_ENTRIES: usize = 16;

#[derive(Default)]
pub(super) struct PreviewReuse {
    parked: VecDeque<(PreviewRenderCache, usize)>,
    bytes: usize,
    active_bytes: usize,
}

impl PreviewReuse {
    fn park(&mut self, cache: PreviewRenderCache, bytes: usize) {
        if bytes > RETAINED_PREVIEW_BYTES {
            return;
        }
        while self.bytes + bytes > RETAINED_PREVIEW_BYTES
            || self.parked.len() >= RETAINED_PREVIEW_ENTRIES
        {
            let Some((_, removed_bytes)) = self.parked.pop_front() else {
                break;
            };
            self.bytes -= removed_bytes;
        }
        self.bytes += bytes;
        self.parked.push_back((cache, bytes));
    }

    pub(super) fn retained_bytes(&self) -> usize {
        self.bytes
    }
    #[cfg(test)]
    pub(super) fn retained_entries(&self) -> usize {
        self.parked.len()
    }
}

impl PreviewRenderCache {
    pub(super) fn estimated_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.key.search_query.capacity()
            + self.wrapped_lines.capacity() * std::mem::size_of::<Line<'static>>()
            + self
                .wrapped_lines
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
            + self.image_regions.capacity() * std::mem::size_of::<jcode_tui_messages::ImageRegion>()
            + self.user_prompt_markers.capacity() * std::mem::size_of::<(usize, usize, String)>()
            + self
                .user_prompt_markers
                .iter()
                .map(|(_, _, text)| text.capacity())
                .sum::<usize>()
    }
}

impl SessionPicker {
    pub(super) fn reuse_preview(&mut self, key: &PreviewCacheKey) -> bool {
        let same_content = |cache: &PreviewRenderCache| {
            cache.key.content_hash == key.content_hash
                && cache.key.inner_width == key.inner_width
                && cache.key.inner_height == key.inner_height
                && cache.key.diff_mode == key.diff_mode
        };
        if self.preview_cache.as_ref().is_some_and(same_content) {
            self.refresh_preview_query(&key.search_query);
            return true;
        }
        let Some(index) = self
            .preview_reuse
            .parked
            .iter()
            .position(|(cache, _)| same_content(cache))
        else {
            return false;
        };
        let Some((cached, bytes)) = self.preview_reuse.parked.remove(index) else {
            return false;
        };
        self.preview_reuse.bytes -= bytes;
        self.install_sized_preview(cached, bytes);
        self.refresh_preview_query(&key.search_query);
        true
    }

    /// Search highlighting is applied to the visible buffer by the renderer.
    /// Changing the query only changes the jump target, not transcript content.
    fn refresh_preview_query(&mut self, query: &str) {
        let Some(cache) = self.preview_cache.as_mut() else {
            return;
        };
        if cache.key.search_query == query {
            return;
        }
        cache.key.search_query = query.to_string();
        let tokens = loading::search_query_tokens(query);
        cache.first_match_line = cache.wrapped_lines.iter().position(|line| {
            let text: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            let lower = text.to_lowercase();
            tokens.iter().any(|token| lower.contains(token.as_str()))
        });
    }

    pub(super) fn install_preview(&mut self, cache: PreviewRenderCache) {
        // Count owned allocations once on build, not on each zero-copy swap.
        let bytes = cache.estimated_bytes();
        self.install_sized_preview(cache, bytes);
    }

    fn install_sized_preview(&mut self, cache: PreviewRenderCache, bytes: usize) {
        let old_bytes = std::mem::replace(&mut self.preview_reuse.active_bytes, bytes);
        if let Some(old) = self.preview_cache.replace(cache) {
            self.preview_reuse.park(old, old_bytes);
        }
    }

    /// Production transcripts are private picker state. Mutable access drops
    /// the fingerprint, and reseeding clears all reference-indexed fingerprints.
    /// Dynamic header/config inputs are still hashed every frame.
    pub(super) fn selected_preview_content_hash(
        &self,
        session_ref: SessionRef,
        diff_mode: crate::config::DiffDisplayMode,
    ) -> Option<u64> {
        let session = self.session_by_ref(session_ref)?;
        let mut reuse = self.preview_fingerprints.borrow_mut();
        let cached = reuse
            .iter()
            .position(|(key, _)| *key == session_ref)
            .and_then(|index| reuse.remove(index));
        let digest = if let Some(entry) = cached {
            reuse.push_back(entry);
            entry.1
        } else {
            let digest = transcript_hash(session);
            if reuse.len() >= FINGERPRINT_ENTRIES {
                reuse.pop_front();
            }
            reuse.push_back((session_ref, digest));
            digest
        };
        Some(self.preview_content_hash_with_transcript(session, diff_mode, digest))
    }
}

pub(super) fn transcript_hash(session: &SessionInfo) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for msg in &session.messages_preview {
        msg.role.hash(&mut h);
        msg.content.hash(&mut h);
        msg.tool_calls.hash(&mut h);
        msg.duration_secs.map(f32::to_bits).hash(&mut h);
        msg.title.hash(&mut h);
        if let Some(tool) = &msg.tool_data {
            tool.id.hash(&mut h);
            tool.name.hash(&mut h);
            tool.input.to_string().hash(&mut h);
        }
    }
    h.finish()
}

/// The picker uses Ratatui's default top-to-bottom list, without scroll padding.
/// Resolve its whole-item viewport from cheap heights before formatting rows.
/// This preserves variable prompt/crash rows and selection scrolling exactly.
pub(super) fn visible_item_bounds(
    heights: &[usize],
    selected: Option<usize>,
    offset: usize,
    height: usize,
) -> std::ops::Range<usize> {
    if heights.is_empty() {
        return 0..0;
    }
    let offset = offset.min(heights.len() - 1);
    if height == 0 {
        return offset..offset;
    }
    let mut first = offset;
    let mut end = offset;
    let mut used = 0;
    for &rows in &heights[offset..] {
        if used + rows > height {
            break;
        }
        used += rows;
        end += 1;
    }
    let selected = selected.unwrap_or(offset).min(heights.len() - 1);
    while selected >= end {
        used += heights[end];
        end += 1;
        while used > height {
            used -= heights[first];
            first += 1;
        }
    }
    while selected < first {
        first -= 1;
        used += heights[first];
        while used > height {
            end -= 1;
            used -= heights[end];
        }
    }
    first..end
}
