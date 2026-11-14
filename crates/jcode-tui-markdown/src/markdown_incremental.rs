use super::*;

#[cfg(test)]
#[test]
fn link_urls_setting_invalidates_unchanged_incremental_text() {
    let mut renderer = IncrementalMarkdownRenderer::new(Some(120));
    let markdown = "[docs](https://example.com)";
    renderer.update(markdown);
    let renders = thread_render_count();
    renderer.update(markdown);
    assert_eq!(thread_render_count(), renders);

    // Simulate cached lines from the opposite setting without changing the
    // process-wide config hook, which would race parallel renderer tests.
    renderer.rendered_show_link_urls = !config_snapshot().show_link_urls;
    renderer.update(markdown);
    assert_eq!(thread_render_count(), renders + 1);
    assert_eq!(
        renderer.rendered_show_link_urls,
        config_snapshot().show_link_urls
    );
}

pub struct IncrementalMarkdownRenderer {
    /// Previously rendered lines
    rendered_lines: Vec<Line<'static>>,
    /// Text that was rendered (for comparison)
    rendered_text: String,
    /// Whether `rendered_lines` contains a deferred-mermaid pending
    /// placeholder; when true the identical-text fast path must re-render
    /// once the deferred render epoch advances so the completed diagram
    /// replaces the placeholder.
    rendered_mermaid_pending: bool,
    /// Deferred-render epoch observed just before `rendered_lines` was
    /// rendered.
    rendered_mermaid_epoch: u64,
    /// Link destination visibility used for the cached lines.
    rendered_show_link_urls: bool,
    /// Width constraint
    max_width: Option<usize>,
    /// Byte offset into `rendered_text` where the re-rendered tail starts.
    /// Everything before it is the stable head; `0` means no split.
    stable_offset: usize,
    /// Rendered lines of the stable head, reused instead of re-rendered.
    stable_lines: Vec<Line<'static>>,
    /// Whether a blank separator line sits between the head and the tail.
    stable_separator: bool,
    /// Newline count of the tail when the split was last checked against a
    /// full render.
    verified_tail_newlines: usize,
    /// Bumped whenever `stable_lines` changes, so callers can key their own
    /// per-line caches on it.
    stable_generation: u64,
    /// Set once this text proved unsafe to split; cleared by `reset` or when
    /// the text stops being an extension of the previous one.
    split_poisoned: bool,
    /// Render context the stable head was rendered under.
    stable_centered: bool,
    stable_spacing: Option<MarkdownSpacingMode>,
}

/// What a scan of the re-rendered tail found.
enum StableSplit {
    /// No later split point yet.
    None,
    /// Tail-relative byte offset of a line that starts a new top-level block.
    At(usize),
    /// The tail holds a construct this scanner does not follow.
    Unsafe,
}

impl IncrementalMarkdownRenderer {
    pub fn new(max_width: Option<usize>) -> Self {
        Self {
            rendered_lines: Vec::new(),
            rendered_text: String::new(),
            rendered_mermaid_pending: false,
            rendered_mermaid_epoch: 0,
            rendered_show_link_urls: config_snapshot().show_link_urls,
            max_width,
            stable_offset: 0,
            stable_lines: Vec::new(),
            stable_separator: false,
            verified_tail_newlines: 0,
            stable_generation: 0,
            split_poisoned: false,
            stable_centered: false,
            stable_spacing: None,
        }
    }

    /// `(generation, line_count)` of the leading lines returned by the last
    /// `update` that came from the stable head. While the generation is
    /// unchanged those lines are identical to the ones returned before, so a
    /// caller may reuse work derived from them (wrapping, plain text).
    pub fn stable_prefix(&self) -> (u64, usize) {
        (self.stable_generation, self.stable_lines.len())
    }

    fn clear_split(&mut self) {
        if self.stable_offset != 0 || !self.stable_lines.is_empty() {
            self.stable_generation = next_stable_generation();
        }
        self.stable_offset = 0;
        self.stable_lines = Vec::new();
        self.stable_separator = false;
        self.verified_tail_newlines = 0;
    }

    fn compose(
        head: &[Line<'static>],
        separator: bool,
        tail: Vec<Line<'static>>,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::with_capacity(head.len() + tail.len() + 1);
        lines.extend_from_slice(head);
        if separator {
            lines.push(Line::default());
        }
        lines.extend(tail);
        lines
    }

    /// The separator (`Some(blank?)`) under which `head ++ tail` reproduces
    /// `full` exactly, if any.
    fn matching_separator(
        full: &[Line<'static>],
        head: &[Line<'static>],
        tail: &[Line<'static>],
    ) -> Option<bool> {
        if tail.is_empty() || full.len() < head.len() || full[..head.len()] != *head {
            return None;
        }
        let rest = &full[head.len()..];
        if rest == tail {
            return Some(false);
        }
        (rest.len() == tail.len() + 1 && line_is_blank(&rest[0]) && rest[1..] == *tail)
            .then_some(true)
    }

    /// Render `full_text`, re-rendering only the text after the stable head
    /// when that is known to give the same lines as a full render.
    ///
    /// A streamed reply only ever grows at the end, so the blocks before the
    /// last blank-line boundary do not change. Splitting markdown there is not
    /// safe in general (lists, reference definitions and HTML blocks reach
    /// across blank lines), so the split is not trusted on its own: it is only
    /// taken at a line that starts a plain top-level paragraph, text with
    /// constructs that reach across blocks never splits, and every time the
    /// tail gains or loses a line the composed result is compared against a
    /// full render. A mismatch turns splitting off for the rest of the text.
    fn render_split(&mut self, full_text: &str) -> Vec<Line<'static>> {
        let centered = center_code_blocks();
        let spacing = effective_markdown_spacing_mode();
        if !self.rendered_text.is_empty() && !full_text.starts_with(self.rendered_text.as_str()) {
            // Not an append: nothing learned about the old text carries over.
            self.clear_split();
            self.split_poisoned = false;
        } else if self.stable_offset > 0
            && (self.stable_centered != centered || self.stable_spacing != Some(spacing))
        {
            self.clear_split();
        }
        if self.split_poisoned {
            return render_markdown_with_width(full_text, self.max_width);
        }

        let tail = &full_text[self.stable_offset..];
        let split = if split_blocked(tail) {
            StableSplit::Unsafe
        } else {
            find_stable_split(tail)
        };
        let candidate = match split {
            StableSplit::Unsafe => {
                self.clear_split();
                self.split_poisoned = true;
                return render_markdown_with_width(full_text, self.max_width);
            }
            StableSplit::At(offset) => Some(offset),
            StableSplit::None => None,
        };
        if self.stable_offset == 0 && candidate.is_none() {
            return render_markdown_with_width(full_text, self.max_width);
        }
        let tail_newlines = count_newlines(tail);
        if candidate.is_none() && tail_newlines == self.verified_tail_newlines {
            let tail_lines = render_markdown_with_width(tail, self.max_width);
            return Self::compose(&self.stable_lines, self.stable_separator, tail_lines);
        }

        // Check the split (the current one, or the one about to be adopted)
        // against a full render, and return the full render either way.
        let full = render_markdown_with_width(full_text, self.max_width);
        let (offset, head) = match candidate {
            Some(relative) => {
                let segment = render_markdown_with_width(&tail[..relative], self.max_width);
                let head = if self.stable_offset == 0 {
                    segment
                } else {
                    Self::compose(&self.stable_lines, self.stable_separator, segment)
                };
                (self.stable_offset + relative, head)
            }
            None => (self.stable_offset, std::mem::take(&mut self.stable_lines)),
        };
        let new_tail = &full_text[offset..];
        let tail_lines = render_markdown_with_width(new_tail, self.max_width);
        match Self::matching_separator(&full, &head, &tail_lines) {
            Some(separator) => {
                if candidate.is_some() {
                    self.stable_generation = next_stable_generation();
                }
                self.stable_offset = offset;
                self.stable_lines = head;
                self.stable_separator = separator;
                self.verified_tail_newlines = count_newlines(new_tail);
                self.stable_centered = centered;
                self.stable_spacing = Some(spacing);
            }
            None => {
                self.stable_lines = head;
                self.clear_split();
                self.split_poisoned = true;
            }
        }
        full
    }

    /// Update with new text, returns rendered lines
    ///
    /// Unchanged text returns the cached lines. Any other text is rendered
    /// in full, once.
    pub fn update(&mut self, full_text: &str) -> Vec<Line<'static>> {
        with_streaming_render_context(|| self.update_internal(full_text))
    }

    pub fn debug_memory_profile(&self) -> serde_json::Value {
        let rendered_lines_estimate_bytes = estimate_lines_bytes(&self.rendered_lines);
        let rendered_text_bytes = self.rendered_text.capacity();
        serde_json::json!({
            "rendered_lines_count": self.rendered_lines.len(),
            "rendered_lines_estimate_bytes": rendered_lines_estimate_bytes,
            "rendered_text_bytes": rendered_text_bytes,
            "total_estimate_bytes": rendered_lines_estimate_bytes + rendered_text_bytes,
        })
    }

    fn update_internal(&mut self, full_text: &str) -> Vec<Line<'static>> {
        let show_link_urls = config_snapshot().show_link_urls;
        // Fast path: text unchanged. Not taken while a deferred mermaid
        // placeholder is baked into the cached lines and the deferred render
        // epoch has advanced: the background render finished, so re-render to
        // pick up the completed diagram.
        if full_text == self.rendered_text
            && self.rendered_show_link_urls == show_link_urls
            && !(self.rendered_mermaid_pending
                && mermaid::deferred_render_epoch() != self.rendered_mermaid_epoch)
        {
            return self.rendered_lines.clone();
        }

        // Full re-render required.
        //
        // We previously tried to splice newly-appended markdown from a saved checkpoint,
        // but markdown block separators and list continuity make that unsafe without
        // carrying richer parser state across updates. In practice this caused transient
        // streaming artifacts like duplicated/misaligned content. Favor correctness here.
        //
        // The epoch is read *before* rendering: if a background diagram render
        // completes mid-render, the stamp is already older than the new epoch
        // and the next update re-renders instead of waiting forever.
        let mermaid_epoch_before = mermaid::deferred_render_epoch();
        if self.rendered_show_link_urls != show_link_urls {
            self.clear_split();
        }
        self.rendered_lines = self.render_split(full_text);
        self.rendered_text = full_text.to_string();
        self.rendered_mermaid_pending = self
            .rendered_lines
            .iter()
            .any(line_is_mermaid_pending_placeholder);
        self.rendered_mermaid_epoch = mermaid_epoch_before;
        self.rendered_show_link_urls = show_link_urls;

        // No checkpoint is kept: counting the lines of the completed prefix
        // meant rendering that prefix a second time on every streamed update,
        // and nothing read the count.
        self.rendered_lines.clone()
    }

    /// Find the last complete block in text
    #[cfg(test)]
    pub(crate) fn find_last_complete_block(&self, text: &str) -> Option<usize> {
        self.find_last_complete_block_checkpoint(text)
            .map(|checkpoint| checkpoint.offset)
    }

    #[cfg(test)]
    fn find_last_complete_block_checkpoint(&self, text: &str) -> Option<CompleteBlockCheckpoint> {
        let mut checkpoint = None;
        let mut line_start = 0usize;
        let mut fence_state: Option<(char, usize)> = None;
        let mut display_math_open = false;
        let mut last_nonblank_kind: Option<MarkdownBlockKind> = None;
        let spacing_mode = effective_markdown_spacing_mode();

        while line_start <= text.len() {
            let relative_end = text[line_start..].find('\n');
            let (line_end, line_ends_with_newline) = match relative_end {
                Some(end) => (line_start + end, true),
                None => (text.len(), false),
            };
            let line = &text[line_start..line_end];
            let line_end_including_newline = if line_ends_with_newline {
                line_end + 1
            } else {
                line_end
            };

            match fence_state {
                Some((fence_char, fence_len)) => {
                    if is_closing_fence(line, fence_char, fence_len) {
                        fence_state = None;
                        last_nonblank_kind = Some(MarkdownBlockKind::CodeBlock);
                        checkpoint = Some(CompleteBlockCheckpoint {
                            offset: line_end_including_newline,
                            needs_separator: spacing_separates_after(
                                MarkdownBlockKind::CodeBlock,
                                spacing_mode,
                            ),
                        });
                    }
                }
                None => {
                    if display_math_open {
                        let dd_count = count_unescaped_double_dollar(line);
                        if dd_count % 2 == 1 {
                            display_math_open = false;
                            last_nonblank_kind = Some(MarkdownBlockKind::DisplayMath);
                            checkpoint = Some(CompleteBlockCheckpoint {
                                offset: line_end_including_newline,
                                needs_separator: spacing_separates_after(
                                    MarkdownBlockKind::DisplayMath,
                                    spacing_mode,
                                ),
                            });
                        }
                    } else if let Some((fence_char, fence_len)) = parse_opening_fence(line) {
                        fence_state = Some((fence_char, fence_len));
                    } else {
                        let dd_count = count_unescaped_double_dollar(line);
                        if dd_count > 0 {
                            if dd_count % 2 == 1 {
                                display_math_open = true;
                            } else {
                                last_nonblank_kind = Some(MarkdownBlockKind::DisplayMath);
                                checkpoint = Some(CompleteBlockCheckpoint {
                                    offset: line_end_including_newline,
                                    needs_separator: spacing_separates_after(
                                        MarkdownBlockKind::DisplayMath,
                                        spacing_mode,
                                    ),
                                });
                            }
                        } else if line_ends_with_newline && is_heading_line(line.trim_start()) {
                            last_nonblank_kind = Some(MarkdownBlockKind::Heading);
                            checkpoint = Some(CompleteBlockCheckpoint {
                                offset: line_end_including_newline,
                                needs_separator: spacing_separates_after(
                                    MarkdownBlockKind::Heading,
                                    spacing_mode,
                                ),
                            });
                        } else if line.trim().is_empty() {
                            checkpoint = Some(CompleteBlockCheckpoint {
                                offset: line_end_including_newline,
                                needs_separator: last_nonblank_kind
                                    .map(|kind| spacing_separates_after(kind, spacing_mode))
                                    .unwrap_or(false),
                            });
                        } else {
                            last_nonblank_kind = Some(infer_markdown_line_kind(line));
                        }
                    }
                }
            }

            if !line_ends_with_newline {
                break;
            }
            line_start = line_end + 1;
        }

        checkpoint
    }

    /// Reset the renderer state
    pub fn reset(&mut self) {
        self.rendered_lines.clear();
        self.rendered_text.clear();
        self.rendered_mermaid_pending = false;
        self.rendered_mermaid_epoch = 0;
        self.clear_split();
        self.split_poisoned = false;
    }

    /// Update width constraint, resets if changed
    pub fn set_width(&mut self, max_width: Option<usize>) {
        if self.max_width != max_width {
            self.max_width = max_width;
            self.reset();
        }
    }
}

/// Generations are unique across renderers, so `(generation, line_count)`
/// names one specific run of head lines process-wide.
fn next_stable_generation() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn count_newlines(text: &str) -> usize {
    text.bytes().filter(|byte| *byte == b'\n').count()
}

/// Text that must be rendered whole: it holds something that reaches across
/// block boundaries, has render side effects, or is pre-processed with state
/// that a split would lose.
fn split_blocked(text: &str) -> bool {
    // Math is normalized by passes that pair delimiters across the whole text,
    // and rendering it can emit a once-per-render install hint.
    text.contains('$')
        || text.contains("\\(")
        || text.contains("\\[")
        || text.contains("\\begin")
        // Reference link and footnote definitions resolve uses anywhere.
        || text.contains("]:")
        // Images and diagrams register state while rendering.
        || text.contains("![")
        || text.contains("ermaid")
}

/// Find the last line in `tail` that starts a new top-level paragraph after a
/// blank line, outside any fenced block. `tail` must itself start at such a
/// line (or at the start of the text).
fn find_stable_split(tail: &str) -> StableSplit {
    let mut found = None;
    let mut fence: Option<(char, usize)> = None;
    let mut previous_blank = false;
    let mut offset = 0usize;
    for line in tail.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let logical = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = logical.trim_start();
        if let Some((marker, len)) = fence {
            if is_closing_fence(logical, marker, len) {
                fence = None;
            }
            previous_blank = false;
            continue;
        }
        if trimmed.is_empty() {
            previous_blank = true;
            continue;
        }
        // HTML blocks may run across blank lines.
        if trimmed.starts_with('<') {
            return StableSplit::Unsafe;
        }
        if logical.starts_with("```") || logical.starts_with("~~~") {
            fence = parse_opening_fence(logical);
        } else if logical.contains("```") || logical.contains("~~~") {
            // A fence that is indented or inside a list or quote: where it
            // ends depends on its container, which this scan does not track.
            let lead = logical.trim_start_matches(|c: char| {
                c.is_whitespace() || c.is_ascii_digit() || "->*+.)".contains(c)
            });
            if lead.starts_with("```") || lead.starts_with("~~~") {
                return StableSplit::Unsafe;
            }
        }
        if fence.is_none()
            && previous_blank
            && start > 0
            && logical.chars().next().is_some_and(char::is_alphabetic)
        {
            found = Some(start);
        }
        previous_blank = false;
    }
    match found {
        Some(start) => StableSplit::At(start),
        None => StableSplit::None,
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompleteBlockCheckpoint {
    offset: usize,
    needs_separator: bool,
}

#[cfg(test)]
pub(crate) fn is_heading_line(line: &str) -> bool {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    hashes > 0 && hashes <= 6 && line.chars().nth(hashes) == Some(' ')
}

#[cfg(test)]
pub(crate) fn is_thematic_break_line(line: &str) -> bool {
    let trimmed = line.trim();
    let mut marker: Option<char> = None;
    let mut count = 0usize;

    for ch in trimmed.chars() {
        if ch == ' ' || ch == '\t' {
            continue;
        }
        match marker {
            None if matches!(ch, '-' | '*' | '_') => {
                marker = Some(ch);
                count = 1;
            }
            Some(existing) if ch == existing => count += 1,
            _ => return false,
        }
    }

    count >= 3
}

#[cfg(test)]
pub(crate) fn looks_like_ordered_list_item(line: &str) -> bool {
    let trimmed = line.trim_start();
    let digit_count = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
    digit_count > 0
        && matches!(trimmed.chars().nth(digit_count), Some('.' | ')'))
        && matches!(trimmed.chars().nth(digit_count + 1), Some(' ' | '\t'))
}

#[cfg(test)]
pub(crate) fn infer_markdown_line_kind(line: &str) -> MarkdownBlockKind {
    let trimmed = line.trim_start();
    if is_heading_line(trimmed) {
        MarkdownBlockKind::Heading
    } else if is_thematic_break_line(trimmed) {
        MarkdownBlockKind::Rule
    } else if trimmed.starts_with('>') {
        MarkdownBlockKind::BlockQuote
    } else if trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("+ ")
        || looks_like_ordered_list_item(trimmed)
    {
        MarkdownBlockKind::List
    } else if trimmed.starts_with('<') {
        MarkdownBlockKind::HtmlBlock
    } else {
        MarkdownBlockKind::Paragraph
    }
}
