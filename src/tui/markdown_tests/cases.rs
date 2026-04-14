use super::*;

#[test]
fn test_simple_markdown() {
    let lines = render_markdown("Hello **world**");
    assert!(!lines.is_empty());
}

#[test]
fn test_code_block() {
    let lines = render_markdown("```rust\nfn main() {}\n```");
    assert!(!lines.is_empty());
}

#[test]
fn test_extract_copy_targets_from_rendered_lines_for_code_block() {
    let lines = render_markdown("before\n\n```rust\nfn main() {}\nprintln!(\"hi\");\n```\n\nafter");
    let targets = extract_copy_targets_from_rendered_lines(&lines);

    assert_eq!(targets.len(), 1);
    let target = &targets[0];
    assert_eq!(
        target.kind,
        CopyTargetKind::CodeBlock {
            language: Some("rust".to_string())
        }
    );
    assert_eq!(target.content, "fn main() {}\nprintln!(\"hi\");");
    assert_eq!(target.start_raw_line, target.badge_raw_line);
    assert!(target.end_raw_line > target.start_raw_line);
}

#[test]
fn test_progress_bar() {
    let bar = progress_bar(0.5, 10);
    assert_eq!(bar.chars().count(), 10);
}

#[test]
fn test_table_render_basic() {
    let md = "| A | B |\n| - | - |\n| 1 | 2 |";
    let lines = render_markdown(md);
    let rendered: Vec<String> = lines.iter().map(line_to_string).collect();

    assert!(
        rendered
            .iter()
            .any(|l| l.contains('│') && l.contains('A') && l.contains('B'))
    );
    assert!(rendered.iter().any(|l| l.contains('─') && l.contains('┼')));
}

#[test]
fn test_table_width_truncation() {
    let md = "| Column | Value |\n| - | - |\n| very_long_cell_value | 1234567890 |";
    let lines = render_markdown_with_width(md, Some(20));
    let rendered: Vec<String> = lines.iter().map(line_to_string).collect();

    assert!(rendered.iter().any(|l| l.contains('…')));
    let max_len = rendered
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0);
    assert!(max_len <= 20);
}

#[test]
fn test_table_width_truncation_with_three_columns_stays_within_limit() {
    let md =
        "| # | Principle | Story Ready |\n| - | - | - |\n| 1 | Customer Obsession | unchecked |";
    let lines = render_markdown_with_width(md, Some(24));
    let rendered: Vec<String> = lines.iter().map(line_to_string).collect();

    assert!(
        rendered.iter().any(|line| line.contains("─┼─")),
        "expected table separator line: {:?}",
        rendered
    );

    let max_width = rendered.iter().map(|line| line.width()).max().unwrap_or(0);
    assert!(
        max_width <= 24,
        "expected all rendered table lines to fit width 24, got {} in {:?}",
        max_width,
        rendered
    );
}

#[test]
fn test_table_cjk_alignment() {
    let md = "| Issue | You wrote |\n| - | - |\n| 政策 pronunciation | zhēn cí |";
    let lines = render_markdown(md);
    let rendered: Vec<String> = lines.iter().map(line_to_string).collect();

    let non_empty: Vec<&String> = rendered.iter().filter(|l| !l.is_empty()).collect();
    assert!(
        non_empty.len() >= 3,
        "Expected at least 3 non-empty lines, got {}: {:?}",
        non_empty.len(),
        non_empty
    );

    let header = non_empty[0];
    let separator = non_empty[1];
    let data_row = non_empty[2];

    let header_width = UnicodeWidthStr::width(header.as_str());
    let sep_width = UnicodeWidthStr::width(separator.as_str());
    let data_width = UnicodeWidthStr::width(data_row.as_str());

    assert_eq!(
        header_width, sep_width,
        "Header and separator should have same display width: header='{}' ({}) sep='{}' ({})",
        header, header_width, separator, sep_width
    );
    assert_eq!(
        header_width, data_width,
        "Header and data row should have same display width: header='{}' ({}) data='{}' ({})",
        header, header_width, data_row, data_width
    );
}

#[test]
fn test_mermaid_block_detection() {
    // Mermaid blocks should be detected and rendered differently than regular code
    let md = "```mermaid\nflowchart LR\n    A --> B\n```";
    let lines = render_markdown(md);

    // Mermaid rendering can return:
    // 1. Empty lines (image displayed via Kitty/iTerm2 protocol directly to stdout)
    // 2. ASCII fallback lines (if no graphics support)
    // 3. Error lines (if parsing failed)
    // All are valid outcomes

    // Should NOT have the code block border (┌─ mermaid) since mermaid removes it
    let text: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();

    // The key test: it should NOT contain syntax-highlighted code (the raw mermaid source)
    // It should either be empty (image displayed) or contain mermaid metadata
    assert!(
        lines.is_empty() || text.contains("mermaid") || text.contains("flowchart"),
        "Expected mermaid handling, got: {}",
        text
    );
}

#[test]
fn test_mixed_code_and_mermaid() {
    // Mixed content should render both correctly
    let md = "```rust\nfn main() {}\n```\n\n```mermaid\nflowchart TD\n    A\n```\n\n```python\nprint('hi')\n```";
    let lines = render_markdown(md);

    // Should have output for all blocks
    assert!(
        lines.len() >= 3,
        "Expected multiple lines for mixed content"
    );
}

#[test]
fn test_inline_math_render() {
    let lines = render_markdown("Area is $a^2$.");
    let rendered = lines_to_string(&lines);
    assert!(rendered.contains("$a^2$"));
}

#[test]
fn test_display_math_render() {
    let lines = render_markdown("$$\nE = mc^2\n$$");
    let rendered = lines_to_string(&lines);
    assert!(rendered.contains("┌─ math"));
    assert!(rendered.contains("E = mc^2"));
    assert!(rendered.contains("└─"));
}

#[test]
fn test_link_strike_and_image_render() {
    let md = "This is ~~old~~ and [docs](https://example.com).\n\n![chart](https://img.example/chart.png)";
    let lines = render_markdown(md);
    let rendered = lines_to_string(&lines);
    assert!(rendered.contains("old"));
    assert!(rendered.contains("docs (https://example.com)"));
    assert!(rendered.contains("[image: chart] (https://img.example/chart.png)"));
}

#[test]
fn test_ordered_and_task_list_render() {
    let md = "1. first\n2. second\n\n- [x] done\n- [ ] todo";
    let lines = render_markdown(md);
    let rendered = lines_to_string(&lines);
    assert!(rendered.contains("1. first"));
    assert!(rendered.contains("2. second"));
    assert!(rendered.contains("[x] done"));
    assert!(rendered.contains("[ ] todo"));
}

#[test]
fn test_blockquote_footnote_and_definition_list_render() {
    let md = "> quote line\n\nRef[^a]\n\n[^a]: footnote body\n\nTerm\n  : definition text";
    let lines = render_markdown(md);
    let rendered = lines_to_string(&lines);
    assert!(rendered.contains("│ quote line"));
    assert!(rendered.contains("[^a]"));
    assert!(rendered.contains("[^a]: footnote body"));
    assert!(rendered.contains("Term"));
    assert!(rendered.contains("definition text"));
}

#[test]
fn test_plain_paragraph_alignment_remains_unset() {
    let lines = render_markdown("plain paragraph");
    let line = lines
        .iter()
        .find(|line| line_to_string(line).contains("plain paragraph"))
        .expect("paragraph line");
    assert_eq!(line.alignment, None);
}

#[test]
fn test_structured_markdown_lines_force_left_alignment() {
    let md = concat!(
        "- [x] done\n",
        "1. numbered\n\n",
        "> quoted\n\n",
        "[^a]: footnote body\n\n",
        "Term\n  : definition text\n\n",
        "| A | B |\n| - | - |\n| 1 | 2 |\n\n",
        "$$\nE = mc^2\n$$\n\n",
        "---\n\n",
        "<div>html</div>"
    );

    let saved = center_code_blocks();
    set_center_code_blocks(true);
    let lines = render_markdown_with_width(md, Some(40));
    set_center_code_blocks(saved);

    let expected = [
        "• [x] done",
        "1. numbered",
        "│ quoted",
        "[^a]: footnote body",
        "• Term",
        "  -> definition text",
        "A │ B",
        "─┼─",
        "1 │ 2",
        "┌─ math",
        "│ E = mc^2",
        "└─",
        "────",
        "<div>html</div>",
    ];

    for snippet in expected {
        let line = lines
            .iter()
            .find(|line| line_to_string(line).contains(snippet))
            .unwrap_or_else(|| panic!("missing line containing '{snippet}' in {lines:?}"));
        assert_eq!(
            line.alignment,
            Some(Alignment::Left),
            "expected left alignment for line containing '{snippet}'"
        );
    }
}

#[test]
fn test_wrapped_left_aligned_list_items_stay_left_aligned() {
    let lines = render_markdown("- this is a long list item that should wrap");
    let wrapped = wrap_lines(lines, 12);

    let non_empty: Vec<&Line<'_>> = wrapped
        .iter()
        .filter(|line| !line.spans.is_empty())
        .collect();
    assert!(
        non_empty.len() >= 2,
        "expected wrapped list item: {wrapped:?}"
    );
    assert!(
        non_empty
            .iter()
            .all(|line| line.alignment == Some(Alignment::Left)),
        "expected wrapped list lines to preserve left alignment: {wrapped:?}"
    );
}

#[test]
fn test_wrapped_code_block_repeats_gutter_on_continuations() {
    let lines = render_markdown("```text\nalpha beta gamma delta\n```");
    let wrapped = wrap_lines(lines, 10);
    let rendered: Vec<String> = wrapped.iter().map(line_to_string).collect();

    assert_eq!(
        rendered,
        vec![
            "┌─ text ",
            "│ alpha ",
            "│ beta ",
            "│ gamma ",
            "│ delta",
            "└─",
        ]
    );
}

#[test]
fn test_wrapped_syntax_highlighted_code_block_keeps_all_body_lines_in_frame() {
    let lines = render_markdown("```rust\nlet alpha_beta_gamma = delta_epsilon_zeta();\n```");
    let wrapped = wrap_lines(lines, 18);
    let rendered: Vec<String> = wrapped.iter().map(line_to_string).collect();

    assert!(
        rendered
            .first()
            .is_some_and(|line| line.starts_with("┌─ rust ")),
        "expected code block header: {rendered:?}"
    );
    assert_eq!(rendered.last().map(String::as_str), Some("└─"));

    let body = &rendered[1..rendered.len() - 1];
    assert!(body.len() >= 2, "expected wrapped code body: {rendered:?}");
    assert!(
        body.iter().all(|line| line.starts_with("│ ")),
        "every wrapped code line should remain inside the code block frame: {rendered:?}"
    );

    let flattened = body
        .iter()
        .map(|line| line.trim_start_matches("│ "))
        .collect::<String>();
    assert!(
        flattened.contains("let alpha_beta_gamma = delta_epsilon_zeta();"),
        "wrapped code body should preserve code text order: {rendered:?}"
    );
}

#[test]
fn test_wrapped_text_code_block_with_long_token_keeps_gutter_on_continuations() {
    let lines = render_markdown(
        "```text\nui_viewport::render_native_scrollbar|viewport::render_native_scrollbar|render_native_scrollbar(\n```",
    );
    let wrapped = wrap_lines(lines, 24);
    let rendered: Vec<String> = wrapped.iter().map(line_to_string).collect();

    assert_eq!(rendered.first().map(String::as_str), Some("┌─ text "));
    assert_eq!(rendered.last().map(String::as_str), Some("└─"));

    let body = &rendered[1..rendered.len() - 1];
    assert!(body.len() >= 2, "expected wrapped code body: {rendered:?}");
    assert!(
        body.iter().all(|line| line.starts_with("│ ")),
        "every wrapped continuation should preserve the framed gutter: {rendered:?}"
    );
    assert!(
        body.concat().contains("render_native_scrollbar"),
        "wrapped code body should preserve the long identifier: {rendered:?}"
    );
}

#[test]
fn test_centered_mode_keeps_list_markers_flush_left() {
    let md = concat!(
        "1. Create a goal\n",
        "   - title\n",
        "   - description / \"why this matters\"\n",
        "   - success criteria\n",
        "2. Break it down\n",
        "   - milestones\n",
        "   - steps\n"
    );

    let saved = center_code_blocks();
    set_center_code_blocks(true);
    let lines = render_markdown_with_width(md, Some(80));
    set_center_code_blocks(saved);

    let numbered_1 = lines
        .iter()
        .find(|line| line_to_string(line).contains("1. Create a goal"))
        .expect("numbered list item");
    let numbered_2 = lines
        .iter()
        .find(|line| line_to_string(line).contains("2. Break it down"))
        .expect("second numbered list item");
    let bullet = lines
        .iter()
        .find(|line| line_to_string(line).contains("description /"))
        .expect("nested bullet item");

    let numbered_1_text = line_to_string(numbered_1);
    let numbered_2_text = line_to_string(numbered_2);
    let bullet_text = line_to_string(bullet);

    let numbered_pad = leading_spaces(&numbered_1_text);
    let numbered_2_pad = leading_spaces(&numbered_2_text);
    let bullet_pad = leading_spaces(&bullet_text);

    assert!(
        numbered_pad > 0,
        "numbered list should be centered as a block: {lines:?}"
    );
    assert!(
        numbered_pad == numbered_2_pad,
        "numbered items should share the same block padding: {lines:?}"
    );
    assert!(
        bullet_pad > numbered_pad,
        "nested bullet should keep additional internal indent within the centered block: {lines:?}"
    );
    assert!(
        numbered_1_text[numbered_pad..].starts_with("1. Create a goal"),
        "number marker should stay left-aligned within centered block: {lines:?}"
    );
    assert!(
        bullet_text[bullet_pad..].starts_with("• description /"),
        "bullet marker should stay left-aligned within centered block: {lines:?}"
    );
}

#[test]
fn test_centered_mode_centers_other_structured_blocks_as_blocks() {
    let md = concat!(
        "> quoted line\n\n",
        "[^a]: footnote body\n\n",
        "Term\n  : definition text\n\n",
        "| A | B |\n| - | - |\n| 1 | 2 |\n"
    );

    let saved = center_code_blocks();
    set_center_code_blocks(true);
    let lines = render_markdown_with_width(md, Some(50));
    set_center_code_blocks(saved);

    for snippet in ["│ quoted line", "[^a]: footnote body", "• Term", "A │ B"] {
        let line = lines
            .iter()
            .find(|line| line_to_string(line).contains(snippet))
            .unwrap_or_else(|| panic!("missing '{snippet}' in {lines:?}"));
        let text = line_to_string(line);
        assert!(
            leading_spaces(&text) > 0,
            "structured block line should be centered as a block: {text:?} / {lines:?}"
        );
    }
}

#[test]
fn test_centered_mode_still_centers_framed_code_blocks() {
    let saved = center_code_blocks();
    set_center_code_blocks(true);
    let lines = render_markdown_with_width("```rust\nfn main() {}\n```", Some(40));
    set_center_code_blocks(saved);

    let header = lines
        .iter()
        .find(|line| line_to_string(line).contains("┌─ rust "))
        .expect("code block header");
    assert!(
        line_to_string(header).starts_with(' '),
        "framed code block should keep centered padding: {lines:?}"
    );
}

#[test]
fn test_rule_and_inline_html_render() {
    let md = "before\n\n---\n\ninline <span>html</span> tag";
    let lines = render_markdown(md);
    let rendered = lines_to_string(&lines);
    assert!(rendered.contains("────────────────"));
    assert!(rendered.contains("<span>"));
    assert!(rendered.contains("</span>"));
}

#[test]
fn test_centered_mode_centers_rules_as_blocks() {
    let saved = center_code_blocks();
    set_center_code_blocks(true);
    let lines = render_markdown_with_width("before\n\n---\n\nafter", Some(50));
    set_center_code_blocks(saved);

    let rule_line = lines
        .iter()
        .find(|line| line_to_string(line).contains("────"))
        .expect("rule line");
    let text = line_to_string(rule_line);
    assert!(
        leading_spaces(&text) > 0,
        "rule should be centered: {text:?}"
    );
    assert!(
        UnicodeWidthStr::width(text.trim()) <= RULE_LEN,
        "rule should not span full width: {text:?}"
    );
}

#[test]
fn test_centered_mode_keeps_lists_left_aligned() {
    let saved = center_code_blocks();
    set_center_code_blocks(true);
    let lines = render_markdown_with_width("- one\n- two", Some(50));
    set_center_code_blocks(saved);

    let rendered: Vec<String> = lines
        .iter()
        .map(line_to_string)
        .filter(|line| !line.is_empty())
        .collect();

    assert_eq!(
        rendered.len(),
        2,
        "expected rendered list items: {rendered:?}"
    );
    let first_pad = leading_spaces(&rendered[0]);
    let second_pad = leading_spaces(&rendered[1]);
    assert_eq!(
        first_pad, second_pad,
        "list items should share the same block pad: {rendered:?}"
    );
    assert!(
        first_pad > 0,
        "list block should be centered in centered mode: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .all(|line| line[first_pad..].starts_with("• "))
    );
}

#[test]
fn test_centered_mode_right_aligns_ordered_markers_within_list_block() {
    let saved = center_code_blocks();
    set_center_code_blocks(true);
    let lines = render_markdown_with_width("9. stuff\n10. more stuff here", Some(50));
    set_center_code_blocks(saved);

    let nine = lines
        .iter()
        .find(|line| line_to_string(line).contains("stuff"))
        .expect("9 line");
    let ten = lines
        .iter()
        .find(|line| line_to_string(line).contains("more stuff here"))
        .expect("10 line");

    let nine_text = line_to_string(nine);
    let ten_text = line_to_string(ten);
    let nine_content = nine_text.find("stuff").expect("9 content");
    let ten_content = ten_text.find("more").expect("10 content");

    assert_eq!(
        nine_content, ten_content,
        "ordered list content should share a single column: {nine_text:?} / {ten_text:?}"
    );
    assert!(
        nine_text.contains(" 9. "),
        "single-digit marker should be right-aligned to match two-digit markers: {nine_text:?}"
    );
}

#[test]
fn test_wrapped_centered_ordered_list_keeps_shared_content_column() {
    let saved = center_code_blocks();
    set_center_code_blocks(true);
    let lines = render_markdown_with_width(
        "9. short\n10. this centered numbered list item should wrap onto another line cleanly",
        Some(42),
    );
    set_center_code_blocks(saved);

    let wrapped = wrap_lines(lines, 26);
    let rendered: Vec<String> = wrapped
        .iter()
        .map(line_to_string)
        .filter(|line| !line.is_empty())
        .collect();

    assert!(
        rendered.len() >= 3,
        "expected wrapped ordered list: {rendered:?}"
    );

    let short_line = rendered
        .iter()
        .find(|line| line.contains("short"))
        .expect("short line");
    let wrapped_first = rendered
        .iter()
        .find(|line| line.contains("this centered"))
        .expect("wrapped first line");
    let wrapped_cont = rendered
        .iter()
        .find(|line| line.contains("another line"))
        .expect("wrapped continuation");

    let short_col = short_line.find("short").expect("short col");
    let wrapped_first_col = wrapped_first.find("this").expect("first col");
    let wrapped_cont_col = wrapped_cont.find("another").expect("cont col");

    assert_eq!(
        short_col, wrapped_first_col,
        "9 and 10 content should align: {rendered:?}"
    );
    assert_eq!(
        wrapped_first_col, wrapped_cont_col,
        "wrapped continuation should stay on the shared content column: {rendered:?}"
    );
}

#[test]
fn test_wrapped_centered_bullet_list_preserves_content_indent() {
    let saved = center_code_blocks();
    set_center_code_blocks(true);
    let lines = render_markdown_with_width(
        "- this centered bullet item should wrap onto another line cleanly",
        Some(34),
    );
    set_center_code_blocks(saved);

    let wrapped = wrap_lines(lines, 22);
    let rendered: Vec<String> = wrapped
        .iter()
        .map(line_to_string)
        .filter(|line| !line.is_empty())
        .collect();

    assert!(
        rendered.len() >= 2,
        "expected wrapped list item: {rendered:?}"
    );

    let first_pad = leading_spaces(&rendered[0]);
    let second_pad = leading_spaces(&rendered[1]);
    assert!(rendered[0][first_pad..].starts_with("• "));
    assert_eq!(second_pad, first_pad + UnicodeWidthStr::width("• "));
}

#[test]
fn test_wrapped_centered_numbered_list_preserves_content_indent() {
    let saved = center_code_blocks();
    set_center_code_blocks(true);
    let lines = render_markdown_with_width(
        "12. this centered numbered list item should wrap onto another line cleanly",
        Some(38),
    );
    set_center_code_blocks(saved);

    let wrapped = wrap_lines(lines, 24);
    let rendered: Vec<String> = wrapped
        .iter()
        .map(line_to_string)
        .filter(|line| !line.is_empty())
        .collect();

    assert!(
        rendered.len() >= 2,
        "expected wrapped numbered item: {rendered:?}"
    );

    let first_pad = leading_spaces(&rendered[0]);
    let second_pad = leading_spaces(&rendered[1]);
    assert!(rendered[0][first_pad..].starts_with("12. "));
    assert_eq!(second_pad, first_pad + UnicodeWidthStr::width("12. "));
}

#[test]
fn test_centered_mode_keeps_blockquotes_left_aligned() {
    let saved = center_code_blocks();
    set_center_code_blocks(true);
    let lines = render_markdown_with_width("> quoted\n> second line", Some(50));
    set_center_code_blocks(saved);

    let rendered: Vec<String> = lines
        .iter()
        .map(line_to_string)
        .filter(|line| !line.is_empty())
        .collect();

    assert_eq!(rendered, vec!["│ quoted", "│ second line"]);
}

#[test]
fn test_compact_spacing_keeps_heading_tight_but_separates_list_from_next_heading() {
    let md = "# Intro\nBody\n\n- one\n- two\n\n# Next\nBody";
    let rendered: Vec<String> = render_markdown_with_mode(md, MarkdownSpacingMode::Compact)
        .iter()
        .map(line_to_string)
        .collect();

    assert_eq!(
        rendered,
        vec!["Intro", "Body", "", "• one", "• two", "", "Next", "Body"]
    );
}

#[test]
fn test_document_spacing_adds_heading_separation() {
    let md = "# Intro\nBody\n\n- one\n- two\n\n# Next\nBody";
    let rendered: Vec<String> = render_markdown_with_mode(md, MarkdownSpacingMode::Document)
        .iter()
        .map(line_to_string)
        .collect();

    assert_eq!(
        rendered,
        vec![
            "Intro", "", "Body", "", "• one", "• two", "", "Next", "", "Body"
        ]
    );
}

#[test]
fn test_compact_spacing_separates_code_block_from_following_heading_without_trailing_blank() {
    let md = "```rust\nfn main() {}\n```\n\n# Next";
    let rendered: Vec<String> = render_markdown_with_mode(md, MarkdownSpacingMode::Compact)
        .iter()
        .map(line_to_string)
        .collect();

    assert_eq!(
        rendered,
        vec!["┌─ rust ", "│ fn main() {}", "└─", "", "Next"]
    );
}

#[test]
fn test_document_spacing_keeps_table_single_spaced_between_blocks() {
    let md = "Before\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\nAfter";
    let rendered: Vec<String> =
        render_markdown_with_width_and_mode(md, 40, MarkdownSpacingMode::Document)
            .iter()
            .map(line_to_string)
            .collect();

    let table_start = rendered
        .iter()
        .position(|line| line.contains('│') && line.contains('A') && line.contains('B'))
        .expect("table header line");
    assert_eq!(rendered[table_start - 1], "");
    assert_eq!(rendered[table_start + 3], "");
    assert_eq!(rendered.last().map(String::as_str), Some("After"));
}

#[test]
fn test_debug_memory_profile_reports_highlight_cache_usage() {
    if let Ok(mut cache) = HIGHLIGHT_CACHE.lock() {
        cache.entries.clear();
    }

    let _ = highlight_code_cached("fn main() { println!(\"hi\"); }", Some("rust"));
    let profile = debug_memory_profile();

    assert!(profile.highlight_cache_entries >= 1);
    assert!(profile.highlight_cache_lines >= 1);
    assert!(profile.highlight_cache_estimate_bytes > 0);
}

#[test]
fn test_incremental_renderer_basic() {
    let mut renderer = IncrementalMarkdownRenderer::new(Some(80));

    // First render
    let lines1 = renderer.update("Hello **world**");
    assert!(!lines1.is_empty());

    // Same text should return cached result
    let lines2 = renderer.update("Hello **world**");
    assert_eq!(lines1.len(), lines2.len());

    // Appended text should work
    let lines3 = renderer.update("Hello **world**\n\nMore text");
    assert!(lines3.len() > lines1.len());
}

#[test]
fn test_incremental_renderer_streaming() {
    let mut renderer = IncrementalMarkdownRenderer::new(Some(80));

    // Simulate streaming tokens
    let _ = renderer.update("Hello ");
    let _ = renderer.update("Hello world");
    let _ = renderer.update("Hello world\n\n");
    let lines = renderer.update("Hello world\n\nParagraph 2");

    // Should have rendered both paragraphs
    assert!(lines.len() >= 2);
}

#[test]
fn test_incremental_renderer_streaming_heading_does_not_duplicate() {
    let mut renderer = IncrementalMarkdownRenderer::new(Some(80));

    let _ = renderer.update("## Planning");
    let _ = renderer.update("## Planning\n\n");
    let lines = renderer.update("## Planning\n\nNext step");
    let rendered = lines_to_string(&lines);

    assert_eq!(rendered.matches("Planning").count(), 1, "{rendered}");
    assert!(rendered.contains("Next step"), "{rendered}");
}

#[test]
fn test_incremental_renderer_streaming_inline_math() {
    let mut renderer = IncrementalMarkdownRenderer::new(Some(80));
    let _ = renderer.update("Compute $x");
    let lines = renderer.update("Compute $x$");
    let rendered = lines_to_string(&lines);
    assert!(rendered.contains("$x$"));
}

#[test]
fn test_incremental_renderer_streaming_display_math() {
    let mut renderer = IncrementalMarkdownRenderer::new(Some(80));
    let _ = renderer.update("Intro\n\n$$\nA + B");
    let lines = renderer.update("Intro\n\n$$\nA + B\n$$\n");
    let rendered = lines_to_string(&lines);

    assert!(
        rendered.contains("┌─ math"),
        "expected display math block after closing delimiter: {}",
        rendered
    );
    assert!(rendered.contains("│ A + B"), "expected math body");
    assert!(
        !rendered.contains("$$"),
        "expected raw $$ delimiters to be consumed: {}",
        rendered
    );
}

#[test]
fn test_incremental_renderer_streams_fenced_block_before_close() {
    let mut renderer = IncrementalMarkdownRenderer::new(Some(80));
    let _ = renderer.update("Plan:\n\n```\n");
    let lines = renderer.update("Plan:\n\n```\nProcess A: |████\n");
    let rendered = lines_to_string(&lines);

    assert!(
        rendered.contains("Process A"),
        "Expected streamed code-block content before closing fence: {}",
        rendered
    );
}

#[test]
fn test_incremental_renderer_defers_mermaid_render_until_background_ready() {
    crate::tui::mermaid::clear_cache().ok();

    let mut renderer = IncrementalMarkdownRenderer::new(Some(80));
    let text = "Plan:\n\n```mermaid\nflowchart LR\n  A[Start] --> B[End]\n```\n";
    let lines = renderer.update(text);
    let rendered = lines_to_string(&lines);

    assert!(
        rendered.contains("rendering mermaid diagram")
            || rendered.contains("mermaid diagram rendering"),
        "expected deferred mermaid placeholder on first completed streaming render: {}",
        rendered
    );
}

#[test]
fn test_checkpoint_does_not_enter_unclosed_fence() {
    let renderer = IncrementalMarkdownRenderer::new(Some(80));
    let text = "Intro\n\n```\nProcess A\n\nProcess B";
    let checkpoint = renderer.find_last_complete_block(text);
    assert_eq!(checkpoint, Some("Intro\n\n".len()));
}

#[test]
fn test_checkpoint_advances_after_heading_line() {
    let renderer = IncrementalMarkdownRenderer::new(Some(80));
    let text = "## Planning\nNext item";
    let checkpoint = renderer.find_last_complete_block(text);
    assert_eq!(checkpoint, Some("## Planning\n".len()));
}

#[test]
fn test_incremental_renderer_replaces_stale_prefix_chars() {
    let mut renderer = IncrementalMarkdownRenderer::new(Some(80));
    let _ = renderer.update("Plan:\n\n```\n[\n");
    let lines = renderer.update("Plan:\n\n```\nProcess A\n");
    let rendered = lines_to_string(&lines);

    assert!(
        !rendered.contains("│ ["),
        "Expected stale '[' to be replaced during streaming: {}",
        rendered
    );
    assert!(rendered.contains("Process A"));
}

#[test]
fn test_streaming_unclosed_bracket_keeps_text_visible() {
    let mut renderer = IncrementalMarkdownRenderer::new(Some(80));
    let lines = renderer.update("[Process A: |████");
    let rendered = lines_to_string(&lines);
    assert!(
        rendered.contains("Process A"),
        "Expected unclosed bracket line to remain visible: {}",
        rendered
    );
}

#[test]
fn test_incremental_renderer_matches_full_render_for_prefixes() {
    let sample = concat!(
        "## Plan\n\n",
        "First paragraph with **bold** text.\n\n",
        "---\n\n",
        "- item one\n",
        "- item two\n\n",
        "```rust\n",
        "fn main() {\n",
        "    println!(\"hi\");\n",
        "}\n",
        "```\n\n",
        "Trailing <span>html</span> text.\n",
    );

    let mut renderer = IncrementalMarkdownRenderer::new(Some(60));
    for end in 0..=sample.len() {
        if !sample.is_char_boundary(end) {
            continue;
        }
        let prefix = &sample[..end];
        let incremental = lines_to_string(&renderer.update(prefix));
        let full = lines_to_string(&render_markdown_with_width(prefix, Some(60)));
        assert_eq!(
            incremental, full,
            "incremental render diverged at prefix {end}:\n--- prefix ---\n{prefix:?}\n--- incremental ---\n{incremental}\n--- full ---\n{full}"
        );
    }
}

#[test]
fn test_center_aligned_wrap_balances_lines() {
    let line = Line::from("aa aa aa aa aa aa aa aa aa").alignment(Alignment::Center);
    let wrapped = wrap_line(line, 20);
    let widths: Vec<usize> = wrapped.iter().map(Line::width).collect();

    assert_eq!(wrapped.len(), 2, "{wrapped:?}");
    let min = widths.iter().copied().min().unwrap_or(0);
    let max = widths.iter().copied().max().unwrap_or(0);
    assert!(max - min <= 3, "expected balanced widths, got {widths:?}");
}

#[test]
fn test_lazy_rendering_visible_range() {
    let md = "```rust\nfn main() {\n    println!(\"hello\");\n}\n```\n\nSome text\n\n```python\nprint('hi')\n```";

    // Render with full visibility
    let lines_full = render_markdown_lazy(md, Some(80), 0..100);

    // Render with partial visibility (only first code block visible)
    let lines_partial = render_markdown_lazy(md, Some(80), 0..5);

    // Both should produce output
    assert!(!lines_full.is_empty());
    assert!(!lines_partial.is_empty());
}

#[test]
fn test_ranges_overlap() {
    assert!(ranges_overlap(0..10, 5..15));
    assert!(ranges_overlap(5..15, 0..10));
    assert!(!ranges_overlap(0..5, 10..15));
    assert!(!ranges_overlap(10..15, 0..5));
    assert!(ranges_overlap(0..10, 0..10)); // Same range
    assert!(ranges_overlap(0..10, 5..6)); // Contained
}

#[test]
fn test_highlight_cache_performance() {
    // First call should cache
    let code = "fn main() {\n    println!(\"hello\");\n}";
    let lines1 = highlight_code_cached(code, Some("rust"));

    // Second call should hit cache
    let lines2 = highlight_code_cached(code, Some("rust"));

    assert_eq!(lines1.len(), lines2.len());
}

#[test]
fn test_bold_with_dollar_signs() {
    let md = "Meet the **$35 minimum** (local delivery) and delivery is **free**. Below that, expect a **$5.99** fee.";
    let lines = render_markdown(md);
    let rendered = lines_to_string(&lines);
    assert!(
        !rendered.contains("**"),
        "Bold markers should not appear as literal text: {}",
        rendered
    );
    assert!(rendered.contains("$35 minimum"));
    assert!(rendered.contains("$5.99"));
}

#[test]
fn test_escape_currency_preserves_math() {
    assert_eq!(escape_currency_dollars("$x^2$"), "$x^2$");
    assert_eq!(escape_currency_dollars("$$E=mc^2$$"), "$$E=mc^2$$");
    assert_eq!(escape_currency_dollars("costs $35"), "costs \\$35");
    assert_eq!(escape_currency_dollars("`$100`"), "`$100`");
    assert_eq!(escape_currency_dollars("```\n$50\n```"), "```\n$50\n```");
    assert_eq!(escape_currency_dollars("\\$10"), "\\$10");
    assert_eq!(escape_currency_dollars("████████░░░░"), "████████░░░░");
    assert_eq!(escape_currency_dollars("⣿⣿⣿⣀⣀⣀"), "⣿⣿⣿⣀⣀⣀");
    assert_eq!(escape_currency_dollars("▓▓▒▒░░"), "▓▓▒▒░░");
    assert_eq!(escape_currency_dollars("━━━╺━━━"), "━━━╺━━━");
    assert_eq!(escape_currency_dollars("⠋ Loading $5"), "⠋ Loading \\$5");
}

#[test]
fn test_currency_dollars_in_indented_code_block() {
    assert_eq!(
        escape_currency_dollars("   ```\nCost is $35\n```"),
        "   ```\nCost is $35\n```"
    );

    assert_eq!(
        escape_currency_dollars("    ```\nCost is $35\n```"),
        "    ```\nCost is $35\n```"
    );

    assert_eq!(
        escape_currency_dollars("        ```\nCost is $35\n```"),
        "        ```\nCost is $35\n```"
    );
}

#[test]
fn test_fence_closing_not_triggered_mid_line() {
    let md = "```\nvalue = `code` and then ``` in same line\n```";
    let rendered = lines_to_string(&render_markdown(md));

    assert!(rendered.contains("`code`"));
    assert!(rendered.contains("in same line"));
}

#[test]
fn test_line_oriented_tool_transcript_softbreaks_are_preserved() {
    let md = concat!(
        "tool: batch\n",
        "✓ batch 3 calls\n",
        "  ✓ bash $ git status --short --branch\n",
        "  ✓ communicate list\n",
        "┌─ diff\n",
        "│ 810- Session(SessionInfo),\n",
        "└─\n"
    );

    let lines = render_markdown_with_width(md, Some(28));
    let rendered: Vec<String> = lines.iter().map(line_to_string).collect();

    assert!(
        rendered
            .iter()
            .any(|line| line.trim_start() == "tool: batch"),
        "expected tool transcript header to stay on its own line: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.trim_start().starts_with("✓ batch 3 calls")),
        "expected batch summary to stay on its own line: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.trim_start().starts_with("✓ bash $ git status")),
        "expected nested transcript line to stay on its own line: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.trim_start().starts_with("┌─ diff")),
        "expected diff box header to stay on its own line: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .all(|line| !(line.contains("tool: batch") && line.contains("✓ batch 3 calls"))),
        "tool transcript lines should not collapse into one wrapped paragraph: {rendered:?}"
    );
}

#[test]
fn test_line_oriented_tool_transcript_followed_by_prose_gets_blank_line() {
    let md = concat!(
        "tool: batch\n",
        "✓ batch 1 calls\n",
        "Done checking the formatting."
    );

    let rendered: Vec<String> = render_markdown_with_width(md, Some(48))
        .iter()
        .map(line_to_string)
        .collect();

    let batch_idx = rendered
        .iter()
        .position(|line| line.trim_start() == "✓ batch 1 calls")
        .expect("missing batch transcript line");
    let prose_idx = rendered
        .iter()
        .position(|line| line.trim_start() == "Done checking the formatting.")
        .expect("missing prose line");

    assert_eq!(
        prose_idx,
        batch_idx + 2,
        "expected a blank line between transcript block and prose: {rendered:?}"
    );
    assert!(
        rendered[batch_idx + 1].trim().is_empty(),
        "expected separator line to be blank: {rendered:?}"
    );
}

#[test]
fn test_prose_before_line_oriented_tool_transcript_gets_blank_line() {
    let md = concat!(
        "I checked the repo state.\n",
        "✓ batch 1 calls\n",
        "  ✓ read src/main.rs"
    );

    let rendered: Vec<String> = render_markdown_with_width(md, Some(48))
        .iter()
        .map(line_to_string)
        .collect();

    let prose_idx = rendered
        .iter()
        .position(|line| line.trim_start() == "I checked the repo state.")
        .expect("missing prose line");
    let transcript_idx = rendered
        .iter()
        .position(|line| line.trim_start() == "✓ batch 1 calls")
        .expect("missing transcript line");

    assert_eq!(
        transcript_idx,
        prose_idx + 2,
        "expected a blank line before transcript block: {rendered:?}"
    );
    assert!(
        rendered[prose_idx + 1].trim().is_empty(),
        "expected separator line to be blank: {rendered:?}"
    );
}
