use super::*;

pub struct IncrementalMarkdownRenderer {
    /// Previously rendered lines
    rendered_lines: Vec<Line<'static>>,
    /// Text that was rendered (for comparison)
    rendered_text: String,
    /// Position of last safe checkpoint (after complete block)
    last_checkpoint: usize,
    /// Number of lines at last checkpoint
    lines_at_checkpoint: usize,
    /// Whether a blank separator should be preserved at the checkpoint boundary
    checkpoint_needs_separator: bool,
    /// Width constraint
    max_width: Option<usize>,
}

impl IncrementalMarkdownRenderer {
    pub fn new(max_width: Option<usize>) -> Self {
        Self {
            rendered_lines: Vec::new(),
            rendered_text: String::new(),
            last_checkpoint: 0,
            lines_at_checkpoint: 0,
            checkpoint_needs_separator: false,
            max_width,
        }
    }

    /// Update with new text, returns rendered lines
    ///
    /// This method efficiently handles streaming by:
    /// 1. Detecting if text was only appended (common case)
    /// 2. Finding safe re-render points (after complete blocks)
    /// 3. Only re-rendering from the last safe point
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
            "last_checkpoint": self.last_checkpoint,
            "lines_at_checkpoint": self.lines_at_checkpoint,
            "total_estimate_bytes": rendered_lines_estimate_bytes + rendered_text_bytes,
        })
    }

    fn update_internal(&mut self, full_text: &str) -> Vec<Line<'static>> {
        // Fast path: text unchanged
        if full_text == self.rendered_text {
            return self.rendered_lines.clone();
        }

        // Full re-render required.
        //
        // We previously tried to splice newly-appended markdown from a saved checkpoint,
        // but markdown block separators and list continuity make that unsafe without
        // carrying richer parser state across updates. In practice this caused transient
        // streaming artifacts like duplicated/misaligned content. Favor correctness here.
        self.rendered_lines = render_markdown_with_width(full_text, self.max_width);
        self.rendered_text = full_text.to_string();

        // Find checkpoint for next incremental update
        self.refresh_checkpoint(full_text, true);

        self.rendered_lines.clone()
    }

    /// Find the last complete block in text
    #[cfg(test)]
    pub(crate) fn find_last_complete_block(&self, text: &str) -> Option<usize> {
        self.find_last_complete_block_checkpoint(text)
            .map(|checkpoint| checkpoint.offset)
    }

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

    /// Refresh checkpoint metadata from the latest rendered text.
    ///
    /// `force = true` recomputes prefix line counts even when checkpoint byte position is unchanged.
    fn refresh_checkpoint(&mut self, full_text: &str, force: bool) {
        let checkpoint = self.find_last_complete_block_checkpoint(full_text);
        let new_checkpoint = checkpoint.map(|cp| cp.offset).unwrap_or(0);
        let new_checkpoint_needs_separator =
            checkpoint.map(|cp| cp.needs_separator).unwrap_or(false);
        if !force
            && new_checkpoint == self.last_checkpoint
            && new_checkpoint_needs_separator == self.checkpoint_needs_separator
        {
            return;
        }

        self.last_checkpoint = new_checkpoint;
        self.checkpoint_needs_separator = new_checkpoint_needs_separator;
        if new_checkpoint == 0 {
            self.lines_at_checkpoint = 0;
        } else {
            let prefix_lines =
                render_markdown_with_width(&full_text[..new_checkpoint], self.max_width);
            self.lines_at_checkpoint = prefix_lines.len();
        }
    }

    /// Reset the renderer state
    pub fn reset(&mut self) {
        self.rendered_lines.clear();
        self.rendered_text.clear();
        self.last_checkpoint = 0;
        self.lines_at_checkpoint = 0;
        self.checkpoint_needs_separator = false;
    }

    /// Update width constraint, resets if changed
    pub fn set_width(&mut self, max_width: Option<usize>) {
        if self.max_width != max_width {
            self.max_width = max_width;
            self.reset();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompleteBlockCheckpoint {
    offset: usize,
    needs_separator: bool,
}

pub(crate) fn is_heading_line(line: &str) -> bool {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    hashes > 0 && hashes <= 6 && line.chars().nth(hashes) == Some(' ')
}

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

pub(crate) fn looks_like_ordered_list_item(line: &str) -> bool {
    let trimmed = line.trim_start();
    let digit_count = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
    digit_count > 0
        && matches!(trimmed.chars().nth(digit_count), Some('.' | ')'))
        && matches!(trimmed.chars().nth(digit_count + 1), Some(' ' | '\t'))
}

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
