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
fn test_table_width_wraps_without_truncation() {
    let md = "| Column | Value |\n| - | - |\n| very_long_cell_value | 1234567890 |";
    let lines = render_markdown_with_width(md, Some(20));
    let rendered: Vec<String> = lines.iter().map(line_to_string).collect();

    assert!(!rendered.iter().any(|l| l.contains('…')), "{rendered:?}");
    assert!(
        rendered.iter().any(|l| l.contains("very_long")),
        "{rendered:?}"
    );
    assert!(
        rendered.iter().any(|l| l.contains("_cell_val")),
        "{rendered:?}"
    );
    assert!(rendered.iter().any(|l| l.contains("ue")), "{rendered:?}");
    let max_len = rendered
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0);
    assert!(max_len <= 20);
}

#[test]
fn test_table_width_wrapping_with_three_columns_stays_within_limit() {
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
fn test_table_width_wrap_keeps_apostrophe_words_intact_when_possible() {
    let md = "| Movie | Notes |\n| - | - |\n| Test | Anderson's thriller shouldn't truncate |";
    let lines = render_markdown_with_width(md, Some(32));
    let rendered: Vec<String> = lines.iter().map(line_to_string).collect();

    assert!(!rendered.iter().any(|l| l.contains('…')), "{rendered:?}");
    assert!(
        rendered.iter().any(|l| l.contains("Anderson’s")),
        "expected apostrophe word to remain intact when it fits: {rendered:?}"
    );

    let max_width = rendered.iter().map(|line| line.width()).max().unwrap_or(0);
    assert!(max_width <= 32, "{max_width}: {rendered:?}");
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
    // Mermaid rendering is temporarily disabled by default, so Mermaid fences
    // should safely fall back to normal code blocks unless explicitly opted in.
    let md = "```mermaid\nflowchart LR\n    A --> B\n```";
    let lines = render_markdown(md);
    let text: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();

    assert!(
        text.contains("mermaid"),
        "Expected code block header: {text}"
    );
    assert!(
        text.contains("flowchart LR"),
        "Expected raw Mermaid source: {text}"
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
    let body_text = body
        .iter()
        .map(|line| line.trim_start_matches("│ "))
        .collect::<String>();
    assert!(
        body_text.contains("render_native_scrollbar"),
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
fn test_reasoning_line_stays_dim_italic_across_apostrophes() {
    // Smart-punctuation splits a reasoning line into several text events at
    // apostrophes. The whole line (every span) must remain dim + italic, since
    // only the first event carries the REASONING_SENTINEL.
    assert_reasoning_line_fully_dim_italic("so it's composite and don't stop.");
}

/// Render a single reasoning line through the real `reasoning_line_markup`
/// helper + markdown renderer and assert the invariant the user cares about:
/// every visible span is dim + italic, the sentinel never leaks into visible
/// text, and the original (escaped) characters all survive. This is the
/// programmatic check that replaces eyeballing the TUI: any input that breaks
/// the dim/italic styling fails here.
fn assert_reasoning_line_fully_dim_italic(line: &str) {
    let sentinel = crate::REASONING_SENTINEL;
    let md = crate::reasoning_line_markup(line);
    let lines = render_markdown(&md);
    let dim = md_dim_color();

    let mut visible = String::new();
    for rendered in &lines {
        for span in &rendered.spans {
            assert!(
                !span.content.contains(sentinel),
                "sentinel leaked into visible text for {line:?}: {:?}",
                span.content
            );
            if span.content.trim().is_empty() {
                continue;
            }
            assert_eq!(
                span.style.fg,
                Some(dim),
                "reasoning span not dim for {line:?}: {:?}",
                span.content
            );
            assert!(
                span.style.add_modifier.contains(Modifier::ITALIC),
                "reasoning span not italic for {line:?}: {:?}",
                span.content
            );
            visible.push_str(&span.content);
        }
    }

    // Styling must not have eaten any characters. Smart-punctuation may pretty
    // up the text (straight quotes/dashes become typographic ones), so compare
    // after normalizing those equivalences; the point is that no content is lost
    // and nothing reverts to non-reasoning styling. Leading/trailing whitespace
    // is not visually meaningful (the renderer may collapse it), so compare the
    // trimmed bodies.
    assert_eq!(
        normalize_smart_punctuation(visible.trim()),
        normalize_smart_punctuation(line.trim()),
        "reasoning text content lost during render for {line:?}"
    );
}

/// Fold the typographic characters pulldown-cmark's smart-punctuation produces
/// back to their ASCII equivalents so styling tests can compare text content
/// without caring about quote/dash prettification.
fn normalize_smart_punctuation(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            '\u{2013}' | '\u{2014}' => '-',
            other => other,
        })
        .collect::<String>()
        .replace('\u{2026}', "...")
}

#[test]
fn test_reasoning_line_survives_embedded_markdown() {
    // Adversarial corpus: each of these contains markdown the renderer would
    // normally act on (emphasis, code, links, headings, lists, math, html).
    // After wrapping with reasoning_line_markup they must all render as a single
    // dim+italic run with no characters lost.
    let cases = [
        "plain reasoning with no markup",
        "it's a contraction with an apostrophe",
        "emphasis *star* and _under_ score",
        "double **bold** and __strong__ markers",
        "inline `code span` should stay literal",
        "a [link](https://example.com) reference",
        "an ![image](pic.png) embed",
        "trailing single asterisk like 5 * 3 = 15",
        "ratio a/b and pipe a | b in a table-ish line",
        "tilde ~~strike~~ and lone ~ tilde",
        "angle <tag> and ampersand a & b",
        "leading # not a heading and - not a list",
        "math $x^2$ and $$y$$ dollar spans",
        "backslash path C:\\\\Users\\\\name",
        "unbalanced *open emphasis that never closes",
        "unbalanced `open code that never closes",
        "mixed it's **bold** with `code` and a [link](x)",
        "🤔 unicode and emoji with it's apostrophe",
        // Whitespace edges: CommonMark emphasis flanking rules reject a closing
        // `*` preceded by whitespace (or an opening `*` followed by whitespace),
        // so lines that start/end with whitespace must still stay dim+italic.
        "trailing spaces break the closing star   ",
        "   leading spaces before the text",
        "  surrounded by spaces on both sides  ",
        "trailing tab breaks it\t",
        "ends right at an asterisk *",
        "* starts at an asterisk",
        "   ",
        "\t",
        "ends with escaped backslash and space \\ ",
    ];
    for case in cases {
        assert_reasoning_line_fully_dim_italic(case);
    }
}

#[test]
fn test_multiline_reasoning_renders_one_visual_line_per_thought() {
    // Regression: consecutive reasoning lines must NOT collapse into one run-on
    // paragraph. Each `reasoning_line_markup` line ends in a CommonMark hard
    // break so the model's line structure is preserved (one visual row per
    // thought) instead of softbreaks rendering as spaces.
    let thoughts = [
        "First I will analyze the problem.",
        "Then I will consider the options.",
        "Finally I will decide.",
    ];
    let mut md = String::new();
    for thought in thoughts {
        md.push_str(&crate::reasoning_line_markup(thought));
    }
    let lines = render_markdown(&md);

    // One rendered line per thought (ignoring any trailing blank lines).
    let non_blank: Vec<String> = lines
        .iter()
        .map(line_to_string)
        .filter(|t| !t.trim().is_empty())
        .collect();
    assert_eq!(
        non_blank.len(),
        thoughts.len(),
        "each reasoning thought should be its own line, got: {non_blank:?}"
    );
    for (rendered, expected) in non_blank.iter().zip(thoughts) {
        assert_eq!(
            rendered.trim(),
            expected,
            "reasoning line content mismatch: {non_blank:?}"
        );
    }

    // And every visible span must still be dim + italic.
    let dim = md_dim_color();
    for line in &lines {
        for span in &line.spans {
            if span.content.trim().is_empty() {
                continue;
            }
            assert_eq!(
                span.style.fg,
                Some(dim),
                "reasoning span not dim: {:?}",
                span.content
            );
            assert!(
                span.style.add_modifier.contains(Modifier::ITALIC),
                "reasoning span not italic: {:?}",
                span.content
            );
        }
    }
}

#[test]
fn test_reasoning_emphasis_does_not_leak_into_following_text() {
    // After the reasoning emphasis closes, normal paragraph text must not be
    // styled as reasoning (dim/italic).
    let md = format!(
        "{}\n{}",
        crate::reasoning_line_markup("it's **thinking** with `code`."),
        "Normal answer text."
    );
    let lines = render_markdown(&md);

    let dim = md_dim_color();
    let answer_line = lines
        .iter()
        .find(|l| line_to_string(l).contains("Normal answer text."))
        .expect("answer line present");
    for span in &answer_line.spans {
        if span.content.trim().is_empty() {
            continue;
        }
        assert_ne!(
            span.style.fg,
            Some(dim),
            "answer span must not be dim: {:?}",
            span.content
        );
        assert!(
            !span.style.add_modifier.contains(Modifier::ITALIC),
            "answer span must not be italic: {:?}",
            span.content
        );
    }
}

#[test]
fn test_reasoning_summary_line_markup_folds_to_single_dim_italic_trace() {
    let sentinel = crate::REASONING_SENTINEL;

    // Pluralized count for multi-line blocks.
    let many = crate::reasoning_summary_line_markup(3);
    assert!(
        many.contains(&format!("*{0}▸ thought (3 lines){0}*", sentinel)),
        "expected pluralized summary markup, got: {many:?}"
    );

    // Single/zero-line blocks omit the count.
    let one = crate::reasoning_summary_line_markup(1);
    assert!(
        one.contains(&format!("*{0}▸ thought{0}*", sentinel)) && !one.contains("lines"),
        "expected bare summary markup, got: {one:?}"
    );
    let none = crate::reasoning_summary_line_markup(0);
    assert!(none.contains(&format!("*{0}▸ thought{0}*", sentinel)), "{none:?}");

    // The summary line renders dim + italic with no sentinel leaking into text.
    let lines = render_markdown(&many);
    let dim = md_dim_color();
    let mut saw_marker = false;
    for rendered in &lines {
        for span in &rendered.spans {
            assert!(
                !span.content.contains(sentinel),
                "sentinel leaked into visible summary: {:?}",
                span.content
            );
            if span.content.trim().is_empty() {
                continue;
            }
            if span.content.contains('▸') {
                saw_marker = true;
            }
            assert_eq!(span.style.fg, Some(dim), "summary span not dim: {:?}", span.content);
            assert!(
                span.style.add_modifier.contains(Modifier::ITALIC),
                "summary span not italic: {:?}",
                span.content
            );
        }
    }
    assert!(saw_marker, "summary marker '▸' must be visible: {lines:?}");
}
