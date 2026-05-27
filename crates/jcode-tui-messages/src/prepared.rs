use crate::WrappedLineMap;
use jcode_tui_markdown::CopyTargetKind;
use ratatui::text::Line;
use std::sync::Arc;

/// Pre-computed image region from line scanning.
#[derive(Clone, Copy)]
pub struct ImageRegion {
    /// Absolute line index in wrapped_lines.
    pub abs_line_idx: usize,
    /// Absolute exclusive end line of the image placeholder region.
    pub end_line: usize,
    /// Hash of the mermaid content for cache lookup.
    pub hash: u64,
    /// Total height of the image placeholder in lines.
    pub height: u16,
}

#[derive(Clone, Debug)]
pub struct CopyTarget {
    pub kind: CopyTargetKind,
    pub content: String,
    pub start_line: usize,
    pub end_line: usize,
    pub badge_line: usize,
}

#[derive(Clone, Debug)]
pub struct EditToolRange {
    pub edit_index: usize,
    pub msg_index: usize,
    pub file_path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub expandable: bool,
}

#[derive(Clone)]
pub struct PreparedMessages {
    pub wrapped_lines: Vec<Line<'static>>,
    pub wrapped_plain_lines: Arc<Vec<String>>,
    pub wrapped_copy_offsets: Arc<Vec<usize>>,
    pub raw_plain_lines: Arc<Vec<String>>,
    pub wrapped_line_map: Arc<Vec<WrappedLineMap>>,
    pub wrapped_user_indices: Vec<usize>,
    /// Wrapped line indices where a user prompt line starts.
    pub wrapped_user_prompt_starts: Vec<usize>,
    /// Wrapped line indices where a user prompt line ends, exclusive.
    pub wrapped_user_prompt_ends: Vec<usize>,
    /// Flattened user prompt text in display order, used by prompt preview without
    /// scanning display_messages on every frame.
    pub user_prompt_texts: Vec<String>,
    /// Pre-scanned image regions computed once, not every frame.
    pub image_regions: Vec<ImageRegion>,
    /// Line ranges for edit tool messages.
    pub edit_tool_ranges: Vec<EditToolRange>,
    pub copy_targets: Vec<CopyTarget>,
}

#[derive(Clone)]
pub struct PreparedSection {
    pub kind: PreparedSectionKind,
    pub prepared: Arc<PreparedMessages>,
    pub line_start: usize,
    pub raw_start: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreparedSectionKind {
    Body,
    Header,
    BatchProgress,
    Streaming,
}

#[derive(Clone)]
pub struct PreparedChatFrame {
    pub sections: Vec<PreparedSection>,
    pub total_wrapped_lines: usize,
    pub total_raw_lines: usize,
    pub wrapped_user_indices: Vec<usize>,
    pub wrapped_user_prompt_starts: Vec<usize>,
    pub wrapped_user_prompt_ends: Vec<usize>,
    pub user_prompt_texts: Vec<String>,
    pub image_regions: Vec<ImageRegion>,
    pub edit_tool_ranges: Vec<EditToolRange>,
    pub copy_targets: Vec<CopyTarget>,
}

impl PreparedChatFrame {
    pub fn from_single(prepared: Arc<PreparedMessages>) -> Self {
        Self::from_sections(vec![(PreparedSectionKind::Body, prepared)])
    }

    pub fn from_sections(sections: Vec<(PreparedSectionKind, Arc<PreparedMessages>)>) -> Self {
        let mut prepared_sections = Vec::new();
        let mut line_start = 0usize;
        let mut raw_start = 0usize;
        let mut wrapped_user_indices = Vec::new();
        let mut wrapped_user_prompt_starts = Vec::new();
        let mut wrapped_user_prompt_ends = Vec::new();
        let mut user_prompt_texts = Vec::new();
        let mut image_regions = Vec::new();
        let mut edit_tool_ranges = Vec::new();
        let mut copy_targets = Vec::new();

        for (kind, prepared) in sections {
            if prepared.wrapped_lines.is_empty()
                && prepared.raw_plain_lines.is_empty()
                && prepared.image_regions.is_empty()
                && prepared.edit_tool_ranges.is_empty()
                && prepared.copy_targets.is_empty()
            {
                continue;
            }

            wrapped_user_indices.extend(
                prepared
                    .wrapped_user_indices
                    .iter()
                    .map(|idx| idx + line_start),
            );
            wrapped_user_prompt_starts.extend(
                prepared
                    .wrapped_user_prompt_starts
                    .iter()
                    .map(|idx| idx + line_start),
            );
            wrapped_user_prompt_ends.extend(
                prepared
                    .wrapped_user_prompt_ends
                    .iter()
                    .map(|idx| idx + line_start),
            );
            user_prompt_texts.extend(prepared.user_prompt_texts.iter().cloned());
            image_regions.extend(prepared.image_regions.iter().map(|region| ImageRegion {
                abs_line_idx: region.abs_line_idx + line_start,
                end_line: region.end_line + line_start,
                hash: region.hash,
                height: region.height,
            }));
            edit_tool_ranges.extend(prepared.edit_tool_ranges.iter().map(|range| EditToolRange {
                edit_index: range.edit_index,
                msg_index: range.msg_index,
                file_path: range.file_path.clone(),
                start_line: range.start_line + line_start,
                end_line: range.end_line + line_start,
                expandable: range.expandable,
            }));
            copy_targets.extend(prepared.copy_targets.iter().map(|target| CopyTarget {
                kind: target.kind.clone(),
                content: target.content.clone(),
                start_line: target.start_line + line_start,
                end_line: target.end_line + line_start,
                badge_line: target.badge_line + line_start,
            }));
            prepared_sections.push(PreparedSection {
                kind,
                prepared: prepared.clone(),
                line_start,
                raw_start,
            });
            line_start += prepared.wrapped_lines.len();
            raw_start += prepared.raw_plain_lines.len();
        }

        Self {
            sections: prepared_sections,
            total_wrapped_lines: line_start,
            total_raw_lines: raw_start,
            wrapped_user_indices,
            wrapped_user_prompt_starts,
            wrapped_user_prompt_ends,
            user_prompt_texts,
            image_regions,
            edit_tool_ranges,
            copy_targets,
        }
    }

    pub fn total_wrapped_lines(&self) -> usize {
        self.total_wrapped_lines
    }

    pub fn wrapped_plain_line_count(&self) -> usize {
        self.total_wrapped_lines
    }

    pub fn visible_intersects_section(
        &self,
        kind: PreparedSectionKind,
        start: usize,
        end: usize,
    ) -> bool {
        if end <= start {
            return false;
        }

        self.sections.iter().any(|section| {
            if section.kind != kind {
                return false;
            }
            let section_start = section.line_start;
            let section_end = section_start + section.prepared.wrapped_lines.len();
            section_start < end && start < section_end
        })
    }

    fn line_section(&self, abs_line: usize) -> Option<(&PreparedSection, usize)> {
        self.sections.iter().find_map(|section| {
            let local = abs_line.checked_sub(section.line_start)?;
            (local < section.prepared.wrapped_lines.len()).then_some((section, local))
        })
    }

    fn raw_section(&self, raw_line: usize) -> Option<(&PreparedSection, usize)> {
        self.sections.iter().find_map(|section| {
            let local = raw_line.checked_sub(section.raw_start)?;
            (local < section.prepared.raw_plain_lines.len()).then_some((section, local))
        })
    }

    pub fn wrapped_plain_line(&self, abs_line: usize) -> Option<&str> {
        let (section, local) = self.line_section(abs_line)?;
        section
            .prepared
            .wrapped_plain_lines
            .get(local)
            .map(String::as_str)
    }

    pub fn wrapped_copy_offset(&self, abs_line: usize) -> Option<usize> {
        let (section, local) = self.line_section(abs_line)?;
        section.prepared.wrapped_copy_offsets.get(local).copied()
    }

    pub fn raw_plain_line(&self, raw_line: usize) -> Option<&str> {
        let (section, local) = self.raw_section(raw_line)?;
        section
            .prepared
            .raw_plain_lines
            .get(local)
            .map(String::as_str)
    }

    pub fn wrapped_line_map(&self, abs_line: usize) -> Option<WrappedLineMap> {
        let (section, local) = self.line_section(abs_line)?;
        let map = section.prepared.wrapped_line_map.get(local)?;
        Some(WrappedLineMap {
            raw_line: map.raw_line + section.raw_start,
            start_col: map.start_col,
            end_col: map.end_col,
        })
    }

    pub fn materialize_line_slice(&self, start: usize, end: usize) -> Vec<Line<'static>> {
        let end = end.min(self.total_wrapped_lines);
        if start >= end {
            return Vec::new();
        }

        let mut lines = Vec::with_capacity(end - start);
        for section in &self.sections {
            let section_start = section.line_start;
            let section_end = section_start + section.prepared.wrapped_lines.len();
            let overlap_start = start.max(section_start);
            let overlap_end = end.min(section_end);
            if overlap_start >= overlap_end {
                continue;
            }
            let local_start = overlap_start - section_start;
            let local_end = overlap_end - section_start;
            lines.extend_from_slice(&section.prepared.wrapped_lines[local_start..local_end]);
        }
        lines
    }

    pub fn materialize_all_lines(&self) -> Vec<Line<'static>> {
        self.materialize_line_slice(0, self.total_wrapped_lines)
    }
}
