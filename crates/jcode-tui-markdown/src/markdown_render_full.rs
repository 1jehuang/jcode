use super::render_support::highlight_code;
use super::*;

pub fn render_markdown_with_width(text: &str, max_width: Option<usize>) -> Vec<Line<'static>> {
    let render_start = Instant::now();
    let text = escape_currency_dollars(text);
    let text = preserve_line_oriented_softbreaks(&text);
    let text = text.as_str();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let side_only = diagram_side_only();
    let streaming_mode = streaming_render_context_enabled();
    let deferred_mermaid_mode = deferred_mermaid_render_context_enabled();
    let spacing_mode = effective_markdown_spacing_mode();

    // Style stack for nested formatting
    let mut bold = false;
    let mut italic = false;
    let mut strike = false;
    let mut in_code_block = false;
    let mut code_block_lang: Option<String> = None;
    let mut code_block_content = String::new();
    let mut heading_level: Option<u8> = None;
    let mut blockquote_depth = 0usize;
    let mut list_stack: Vec<ListRenderState> = Vec::new();
    let mut link_targets: Vec<String> = Vec::new();
    let mut in_image = false;
    let mut image_url: Option<String> = None;
    let mut image_alt = String::new();
    let mut in_definition_list = false;
    let mut in_definition_item = false;
    let mut in_footnote_definition = false;
    let mut centered_blocks = CenteredStructuredBlockState::default();

    // Table state
    let mut in_table = false;
    let mut table_row: Vec<String> = Vec::new();
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut current_cell = String::new();
    let mut _is_header_row = false;

    // Enable table parsing
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_MATH);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_GFM);
    options.insert(Options::ENABLE_DEFINITION_LIST);
    options.insert(Options::ENABLE_SMART_PUNCTUATION);
    let parser = Parser::new_ext(text, options);

    // Debug counters
    let mut dbg_headings = 0usize;
    let mut dbg_code_blocks = 0usize;
    let mut dbg_mermaid_blocks = 0usize;
    let mut dbg_tables = 0usize;
    let mut dbg_list_items = 0usize;
    let mut dbg_blockquotes = 0usize;

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                dbg_headings += 1;
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                heading_level = Some(level as u8);
            }
            Event::End(TagEnd::Heading(_)) => {
                if !current_spans.is_empty() {
                    // Choose color based on heading level
                    let color = match heading_level {
                        Some(1) => heading_h1_color(),
                        Some(2) => heading_h2_color(),
                        Some(3) => heading_h3_color(),
                        _ => heading_color(),
                    };

                    let heading_spans: Vec<Span<'static>> = current_spans
                        .drain(..)
                        .map(|s| {
                            Span::styled(s.content.to_string(), Style::default().fg(color).bold())
                        })
                        .collect();
                    lines.push(Line::from(heading_spans));
                    push_block_separator(&mut lines, MarkdownBlockKind::Heading, spacing_mode);
                }
                heading_level = None;
            }

            Event::Start(Tag::Strong) => bold = true,
            Event::End(TagEnd::Strong) => bold = false,

            Event::Start(Tag::Emphasis) => italic = true,
            Event::End(TagEnd::Emphasis) => italic = false,

            Event::Start(Tag::Strikethrough) => strike = true,
            Event::End(TagEnd::Strikethrough) => strike = false,

            Event::Start(Tag::BlockQuote(_)) => {
                dbg_blockquotes += 1;
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                enter_centered_structured_block(&mut centered_blocks, lines.len());
                blockquote_depth += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                blockquote_depth = blockquote_depth.saturating_sub(1);
                exit_centered_structured_block(&mut centered_blocks, lines.len());
                if blockquote_depth == 0
                    && list_stack.is_empty()
                    && !in_definition_list
                    && !in_footnote_definition
                {
                    push_block_separator(&mut lines, MarkdownBlockKind::BlockQuote, spacing_mode);
                }
            }

            Event::Start(Tag::List(start)) => {
                enter_centered_structured_block(&mut centered_blocks, lines.len());
                let start_index = start.unwrap_or(1);
                let state = ListRenderState {
                    ordered: start.is_some(),
                    next_index: start_index,
                    item_line_starts: Vec::new(),
                    max_marker_digits: start_index.to_string().len(),
                };
                list_stack.push(state);
            }
            Event::End(TagEnd::List(_)) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                if let Some(state) = list_stack.pop()
                    && center_code_blocks()
                    && state.ordered
                {
                    align_ordered_list_markers(
                        &mut lines,
                        &state.item_line_starts,
                        state.max_marker_digits,
                    );
                }
                exit_centered_structured_block(&mut centered_blocks, lines.len());
                if blockquote_depth == 0
                    && list_stack.is_empty()
                    && !in_definition_list
                    && !in_footnote_definition
                {
                    push_block_separator(&mut lines, MarkdownBlockKind::List, spacing_mode);
                }
            }

            Event::Start(Tag::Link { dest_url, .. }) => {
                link_targets.push(dest_url.to_string());
            }
            Event::End(TagEnd::Link) => {
                if let Some(url) = link_targets.pop()
                    && !url.is_empty()
                {
                    current_spans.push(Span::styled(
                        format!(" ({})", url),
                        Style::default().fg(md_dim_color()),
                    ));
                }
            }

            Event::Start(Tag::Image { dest_url, .. }) => {
                in_image = true;
                image_url = Some(dest_url.to_string());
                image_alt.clear();
            }
            Event::End(TagEnd::Image) => {
                let alt = if image_alt.trim().is_empty() {
                    "image".to_string()
                } else {
                    image_alt.trim().to_string()
                };
                let label = if let Some(url) = image_url.take() {
                    format!("[image: {}] ({})", alt, url)
                } else {
                    format!("[image: {}]", alt)
                };
                if in_table {
                    current_cell.push_str(&label);
                } else {
                    ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                    current_spans.push(Span::styled(label, Style::default().fg(md_dim_color())));
                }
                in_image = false;
                image_alt.clear();
            }

            Event::Start(Tag::FootnoteDefinition(label)) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                enter_centered_structured_block(&mut centered_blocks, lines.len());
                in_footnote_definition = true;
                ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                current_spans.push(Span::styled(
                    format!("[^{}]: ", label),
                    Style::default().fg(md_dim_color()),
                ));
            }
            Event::End(TagEnd::FootnoteDefinition) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                exit_centered_structured_block(&mut centered_blocks, lines.len());
                in_footnote_definition = false;
            }

            Event::Start(Tag::DefinitionList) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                enter_centered_structured_block(&mut centered_blocks, lines.len());
                in_definition_list = true;
            }
            Event::End(TagEnd::DefinitionList) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                exit_centered_structured_block(&mut centered_blocks, lines.len());
                in_definition_list = false;
                if blockquote_depth == 0 && list_stack.is_empty() && !in_footnote_definition {
                    push_block_separator(
                        &mut lines,
                        MarkdownBlockKind::DefinitionList,
                        spacing_mode,
                    );
                }
            }
            Event::Start(Tag::DefinitionListTitle) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                current_spans.push(Span::styled("• ", Style::default().fg(md_dim_color())));
            }
            Event::End(TagEnd::DefinitionListTitle) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
            }
            Event::Start(Tag::DefinitionListDefinition) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                current_spans.push(Span::styled("  -> ", Style::default().fg(md_dim_color())));
                in_definition_item = true;
            }
            Event::End(TagEnd::DefinitionListDefinition) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                in_definition_item = false;
            }

            Event::Start(Tag::CodeBlock(kind)) => {
                dbg_code_blocks += 1;
                // Flush current line before code block
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                enter_centered_structured_block(&mut centered_blocks, lines.len());
                in_code_block = true;
                code_block_lang = match kind {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => Some(lang.to_string()),
                    _ => None,
                };
                // Don't add header here - we'll add it at the end when we know the block width
                code_block_content.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                // Check if this is a mermaid diagram
                let is_mermaid = mermaid_rendering_enabled()
                    && code_block_lang
                        .as_ref()
                        .map(|l| mermaid::is_mermaid_lang(l))
                        .unwrap_or(false);

                if is_mermaid {
                    dbg_mermaid_blocks += 1;
                    // Render mermaid diagram.
                    // In streaming mode this updates only the ephemeral preview entry.
                    let terminal_width = max_width.and_then(|w| u16::try_from(w).ok());
                    if !streaming_mode
                        && !mermaid_should_register_active()
                        && !mermaid::image_protocol_available()
                    {
                        lines.push(mermaid_sidebar_placeholder(
                            "↗ mermaid diagram (image protocols unavailable)",
                        ));
                        continue;
                    }
                    let result = if streaming_mode {
                        mermaid::render_mermaid_deferred_with_stream_scope(
                            &code_block_content,
                            terminal_width,
                            dbg_mermaid_blocks as u64,
                        )
                    } else if deferred_mermaid_mode {
                        mermaid::render_mermaid_deferred_with_registration(
                            &code_block_content,
                            terminal_width,
                            mermaid_should_register_active(),
                        )
                    } else if !mermaid_should_register_active() {
                        Some(mermaid::render_mermaid_untracked(
                            &code_block_content,
                            terminal_width,
                        ))
                    } else {
                        Some(mermaid::render_mermaid_sized(
                            &code_block_content,
                            terminal_width,
                        ))
                    };
                    match result {
                        Some(result) => {
                            if streaming_mode
                                && let mermaid::RenderResult::Image {
                                    hash,
                                    width,
                                    height,
                                    ..
                                } = &result
                            {
                                mermaid::set_streaming_preview_diagram(
                                    *hash, *width, *height, None,
                                );
                            }
                            match result {
                                mermaid::RenderResult::Image { .. } if side_only => {
                                    lines.push(mermaid_sidebar_placeholder(
                                        "↗ mermaid diagram (sidebar)",
                                    ));
                                }
                                other => {
                                    let mermaid_lines = mermaid::result_to_lines(other, max_width);
                                    lines.extend(mermaid_lines);
                                }
                            }
                        }
                        None => {
                            lines.push(mermaid_sidebar_placeholder(if side_only {
                                "↻ mermaid diagram rendering in sidebar..."
                            } else {
                                "↻ rendering mermaid diagram..."
                            }));
                        }
                    }
                } else {
                    // Render code block with syntax highlighting (cached)
                    let highlighted =
                        highlight_code_cached(&code_block_content, code_block_lang.as_deref());

                    let lang_label = code_block_lang.as_deref().unwrap_or("");
                    // Add header
                    lines.push(
                        Line::from(Span::styled(
                            format!("┌─ {} ", lang_label),
                            Style::default().fg(md_dim_color()),
                        ))
                        .left_aligned(),
                    );

                    // Add code lines
                    for hl_line in highlighted {
                        let mut spans =
                            vec![Span::styled("│ ", Style::default().fg(md_dim_color()))];
                        spans.extend(hl_line.spans);
                        lines.push(Line::from(spans).left_aligned());
                    }

                    // Add footer
                    lines.push(
                        Line::from(Span::styled("└─", Style::default().fg(md_dim_color())))
                            .left_aligned(),
                    );
                }
                exit_centered_structured_block(&mut centered_blocks, lines.len());
                in_code_block = false;
                code_block_lang = None;
                code_block_content.clear();
                if blockquote_depth == 0
                    && list_stack.is_empty()
                    && !in_definition_list
                    && !in_footnote_definition
                {
                    push_block_separator(&mut lines, MarkdownBlockKind::CodeBlock, spacing_mode);
                }
            }

            Event::Code(code) => {
                if in_image {
                    image_alt.push_str(&code);
                    continue;
                }
                // Inline code - handle differently in tables vs regular text
                if in_table {
                    current_cell.push_str(&code);
                } else {
                    ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                    current_spans.push(Span::styled(
                        code.to_string(),
                        apply_inline_decorations(
                            Style::default().fg(code_fg()).bg(code_bg()),
                            strike,
                            !link_targets.is_empty(),
                        ),
                    ));
                }
            }

            Event::InlineMath(math) => {
                if in_image {
                    image_alt.push('$');
                    image_alt.push_str(&math);
                    image_alt.push('$');
                    continue;
                }
                if in_table {
                    current_cell.push('$');
                    current_cell.push_str(&math);
                    current_cell.push('$');
                } else {
                    ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                    current_spans.push(math_inline_span(&math));
                }
            }

            Event::DisplayMath(math) => {
                if in_image {
                    image_alt.push_str("$$");
                    image_alt.push_str(&math);
                    image_alt.push_str("$$");
                    continue;
                }
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                if in_table {
                    current_cell.push_str("$$");
                    current_cell.push_str(&math);
                    current_cell.push_str("$$");
                } else {
                    let block_start = lines.len();
                    for line in math_display_lines(&math) {
                        lines.push(with_blockquote_prefix(line, blockquote_depth));
                    }
                    record_centered_independent_block(
                        &mut centered_blocks,
                        block_start,
                        lines.len(),
                    );
                    if blockquote_depth == 0
                        && list_stack.is_empty()
                        && !in_definition_list
                        && !in_footnote_definition
                    {
                        push_block_separator(
                            &mut lines,
                            MarkdownBlockKind::DisplayMath,
                            spacing_mode,
                        );
                    }
                }
            }

            Event::Text(text) => {
                if in_code_block {
                    code_block_content.push_str(&text);
                } else if in_image {
                    image_alt.push_str(&text);
                } else if in_table {
                    current_cell.push_str(&text);
                } else {
                    // Check for "Thought for X.Xs" pattern and render dimmed
                    let is_thinking_duration =
                        text.starts_with("Thought for ") && text.ends_with('s');
                    let mut style = if is_thinking_duration {
                        Style::default().fg(md_dim_color()).italic()
                    } else {
                        match (bold, italic) {
                            (true, true) => Style::default().fg(bold_color()).bold().italic(),
                            (true, false) => Style::default().fg(bold_color()).bold(),
                            (false, true) => Style::default().fg(text_color()).italic(),
                            (false, false) => Style::default().fg(text_color()),
                        }
                    };
                    style = apply_inline_decorations(style, strike, !link_targets.is_empty());
                    ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                    current_spans.push(Span::styled(text.to_string(), style));
                }
            }

            Event::SoftBreak => {
                if in_image {
                    image_alt.push(' ');
                } else if !in_code_block {
                    if blockquote_depth > 0 {
                        flush_current_line_with_alignment(
                            &mut lines,
                            &mut current_spans,
                            structured_markdown_alignment(
                                blockquote_depth,
                                &list_stack,
                                in_definition_list,
                                in_footnote_definition,
                            ),
                        );
                    } else {
                        current_spans.push(Span::raw(" "));
                    }
                }
            }
            Event::HardBreak => {
                if in_image {
                    image_alt.push(' ');
                } else if !in_code_block {
                    flush_current_line_with_alignment(
                        &mut lines,
                        &mut current_spans,
                        structured_markdown_alignment(
                            blockquote_depth,
                            &list_stack,
                            in_definition_list,
                            in_footnote_definition,
                        ),
                    );
                }
            }

            Event::Rule => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                let block_start = lines.len();
                let width = rendered_rule_width(max_width);
                let rule = Span::styled("─".repeat(width), Style::default().fg(md_dim_color()));
                lines.push(with_blockquote_prefix(
                    Line::from(rule).left_aligned(),
                    blockquote_depth,
                ));
                record_centered_independent_block(&mut centered_blocks, block_start, lines.len());
                if blockquote_depth == 0
                    && list_stack.is_empty()
                    && !in_definition_list
                    && !in_footnote_definition
                {
                    push_block_separator(&mut lines, MarkdownBlockKind::Rule, spacing_mode);
                }
            }

            Event::Html(html) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                let block_start = lines.len();
                for raw in html.lines() {
                    let span =
                        Span::styled(raw.to_string(), Style::default().fg(html_fg()).italic());
                    lines.push(with_blockquote_prefix(
                        Line::from(span).left_aligned(),
                        blockquote_depth,
                    ));
                }
                record_centered_independent_block(&mut centered_blocks, block_start, lines.len());
                if blockquote_depth == 0
                    && list_stack.is_empty()
                    && !in_definition_list
                    && !in_footnote_definition
                {
                    push_block_separator(&mut lines, MarkdownBlockKind::HtmlBlock, spacing_mode);
                }
            }

            Event::InlineHtml(html) => {
                if in_image {
                    image_alt.push_str(&html);
                } else if in_table {
                    current_cell.push_str(&html);
                } else {
                    ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                    current_spans.push(Span::styled(
                        html.to_string(),
                        Style::default().fg(html_fg()).italic(),
                    ));
                }
            }

            Event::FootnoteReference(label) => {
                if in_image {
                    image_alt.push_str(&format!("[^{}]", label));
                } else if in_table {
                    current_cell.push_str(&format!("[^{}]", label));
                } else {
                    ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                    current_spans.push(Span::styled(
                        format!("[^{}]", label),
                        Style::default().fg(md_dim_color()),
                    ));
                }
            }

            Event::TaskListMarker(checked) => {
                if in_table {
                    current_cell.push_str(if checked { "[x] " } else { "[ ] " });
                } else {
                    ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                    current_spans.push(Span::styled(
                        if checked { "[x] " } else { "[ ] " },
                        Style::default().fg(md_dim_color()),
                    ));
                }
            }

            Event::Start(Tag::Paragraph) => {
                ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                if in_definition_item && current_spans.is_empty() {
                    current_spans.push(Span::styled("  ", Style::default().fg(md_dim_color())));
                }
            }
            Event::End(TagEnd::Paragraph) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                push_block_separator(&mut lines, MarkdownBlockKind::Paragraph, spacing_mode);
            }

            Event::Start(Tag::Item) => {
                dbg_list_items += 1;
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                let item_line_start = lines.len();
                let depth = list_stack.len().saturating_sub(1);
                let indent = "  ".repeat(depth);
                let marker = if let Some(state) = list_stack.last_mut() {
                    if state.ordered {
                        let idx = state.next_index;
                        state.next_index = state.next_index.saturating_add(1);
                        state.max_marker_digits =
                            state.max_marker_digits.max(idx.to_string().len());
                        state.item_line_starts.push(item_line_start);
                        format!("{}{}. ", indent, idx)
                    } else {
                        format!("{}• ", indent)
                    }
                } else {
                    "• ".to_string()
                };
                current_spans.push(Span::styled(marker, Style::default().fg(md_dim_color())));
            }
            Event::End(TagEnd::Item) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
            }

            // Table handling
            Event::Start(Tag::Table(_)) => {
                dbg_tables += 1;
                // Flush any pending content
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                enter_centered_structured_block(&mut centered_blocks, lines.len());
                in_table = true;
                table_rows.clear();
            }
            Event::End(TagEnd::Table) => {
                // Render the collected table
                if !table_rows.is_empty() {
                    let rendered = render_table(&table_rows, max_width);
                    lines.extend(rendered);
                    exit_centered_structured_block(&mut centered_blocks, lines.len());
                    if blockquote_depth == 0
                        && list_stack.is_empty()
                        && !in_definition_list
                        && !in_footnote_definition
                    {
                        push_block_separator(&mut lines, MarkdownBlockKind::Table, spacing_mode);
                    }
                } else {
                    exit_centered_structured_block(&mut centered_blocks, lines.len());
                }
                in_table = false;
                table_rows.clear();
            }
            Event::Start(Tag::TableHead) => {
                _is_header_row = true;
                table_row.clear();
            }
            Event::End(TagEnd::TableHead) => {
                if !table_row.is_empty() {
                    table_rows.push(table_row.clone());
                }
                table_row.clear();
                _is_header_row = false;
            }
            Event::Start(Tag::TableRow) => {
                table_row.clear();
            }
            Event::End(TagEnd::TableRow) => {
                if !table_row.is_empty() {
                    table_rows.push(table_row.clone());
                }
                table_row.clear();
            }
            Event::Start(Tag::TableCell) => {
                current_cell.clear();
            }
            Event::End(TagEnd::TableCell) => {
                table_row.push(current_cell.trim().to_string());
                current_cell.clear();
            }

            Event::Start(Tag::MetadataBlock(_)) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
            }
            Event::End(TagEnd::MetadataBlock(_)) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
            }

            _ => {}
        }
    }

    // Handle incomplete code block (streaming case)
    // If we're still inside a code block, render what we have so far
    if in_code_block && !code_block_content.is_empty() {
        let is_mermaid = code_block_lang
            .as_ref()
            .map(|l| mermaid::is_mermaid_lang(l))
            .unwrap_or(false);

        if is_mermaid {
            if side_only {
                lines.push(mermaid_sidebar_placeholder(
                    "↗ mermaid diagram (sidebar, streaming...)",
                ));
            } else {
                // For mermaid, show "rendering..." placeholder while streaming
                let dim = Style::default().fg(md_dim_color());
                lines.push(Line::from(Span::styled("┌─ mermaid (streaming...) ", dim)));
                // Show first few lines of the diagram source
                for source_line in code_block_content.lines().take(5) {
                    lines.push(Line::from(vec![
                        Span::styled("│ ", dim),
                        Span::styled(source_line.to_string(), Style::default().fg(code_fg())),
                    ]));
                }
                if code_block_content.lines().count() > 5 {
                    lines.push(Line::from(Span::styled("│ ...", dim)));
                }
                lines.push(Line::from(Span::styled("└─", dim)));
            }
        } else {
            // Regular code block - render what we have
            let lang_str = code_block_lang.as_deref().unwrap_or("");
            let header = format!(
                "┌─ {} (streaming...)",
                if lang_str.is_empty() {
                    "code"
                } else {
                    lang_str
                }
            );
            lines.push(Line::from(Span::styled(
                header,
                Style::default().fg(md_dim_color()),
            )));

            // Render code with syntax highlighting
            let highlighted = highlight_code(&code_block_content, code_block_lang.as_deref());
            for line in highlighted {
                let mut prefixed = vec![Span::styled("│ ", Style::default().fg(md_dim_color()))];
                prefixed.extend(line.spans);
                lines.push(Line::from(prefixed));
            }
            // Show cursor to indicate more content coming
            lines.push(Line::from(Span::styled(
                "│ ▌",
                Style::default().fg(md_dim_color()),
            )));
            lines.push(Line::from(Span::styled(
                "└─",
                Style::default().fg(md_dim_color()),
            )));
        }
    }

    // Flush remaining spans
    flush_current_line_with_alignment(
        &mut lines,
        &mut current_spans,
        structured_markdown_alignment(
            blockquote_depth,
            &list_stack,
            in_definition_list,
            in_footnote_definition,
        ),
    );

    finalize_centered_structured_blocks(&mut centered_blocks, lines.len());

    normalize_block_separators(&mut lines);

    if center_code_blocks()
        && let Some(width) = max_width
    {
        center_structured_block_ranges(&mut lines, width, &centered_blocks.ranges);
    }

    if let Ok(mut state) = MARKDOWN_DEBUG.lock() {
        state.stats.total_renders += 1;
        state.stats.last_render_ms = Some(render_start.elapsed().as_secs_f32() * 1000.0);
        state.stats.last_text_len = Some(text.len());
        state.stats.last_lines = Some(lines.len());
        state.stats.last_headings = dbg_headings;
        state.stats.last_code_blocks = dbg_code_blocks;
        state.stats.last_mermaid_blocks = dbg_mermaid_blocks;
        state.stats.last_tables = dbg_tables;
        state.stats.last_list_items = dbg_list_items;
        state.stats.last_blockquotes = dbg_blockquotes;
    }

    lines
}
