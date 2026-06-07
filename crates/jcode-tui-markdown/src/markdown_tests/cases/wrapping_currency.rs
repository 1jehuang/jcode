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

#[test]
fn wrap_keeps_word_intact_across_span_boundary() {
    // Regression: a word split across multiple styled spans (e.g. smart-quote
    // tokenization turning "there's" into "there" + "’" + "s") must not be
    // broken mid-word at the span boundary when wrapping left-aligned text.
    use ratatui::style::{Color, Style};
    let spans = vec![
        Span::styled("or if there".to_string(), Style::default()),
        Span::styled("\u{2019}".to_string(), Style::default().fg(Color::Red)),
        Span::styled("s something about the".to_string(), Style::default()),
    ];
    let line = Line::from(spans); // left aligned (default)
    let wrapped = wrap_line(line, 13);
    let rendered: Vec<String> = wrapped.iter().map(line_to_string).collect();

    // "there’s" must appear intact on a single rendered line.
    assert!(
        rendered.iter().any(|l| l.contains("there\u{2019}s")),
        "expected word to stay intact, got {rendered:?}"
    );
    // No line may end with the apostrophe while the next starts with the rest.
    for (i, l) in rendered.iter().enumerate() {
        if l.trim_end().ends_with('\u{2019}')
            && let Some(next) = rendered.get(i + 1)
        {
            assert!(
                !next.trim_start().starts_with('s'),
                "word 'there\u{2019}s' was split across lines: {rendered:?}"
            );
        }
    }
    // Each rendered line must respect the width budget.
    for l in &rendered {
        assert!(l.width() <= 13, "line exceeds width: {l:?} in {rendered:?}");
    }
}
