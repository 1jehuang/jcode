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

#[cfg(feature = "mermaid-renderer")]
#[test]
fn test_incremental_renderer_defers_mermaid_render_until_background_ready() {
    jcode_tui_mermaid::clear_cache().ok();

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
