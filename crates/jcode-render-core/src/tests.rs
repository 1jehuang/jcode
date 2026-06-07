use crate::{
    Alignment, BlockKind, ColumnWidth, FillRole, StyleRole, StyledLine, StyledSpan, WidthMeasure,
    parse_markdown, wrap_line,
};

fn block_kinds(doc: &crate::Document) -> Vec<&BlockKind> {
    doc.blocks.iter().map(|b| &b.kind).collect()
}

#[test]
fn parses_heading() {
    let doc = parse_markdown("# Hello world");
    assert_eq!(doc.blocks.len(), 1);
    assert_eq!(doc.blocks[0].kind, BlockKind::Heading { level: 1 });
    let line = &doc.blocks[0].lines[0];
    assert_eq!(line.plain_text(), "Hello world");
    assert!(line.spans.iter().all(|s| s.attrs.bold));
    assert!(line.spans.iter().all(|s| s.role == StyleRole::Strong));
}

#[test]
fn parses_paragraph_with_emphasis() {
    let doc = parse_markdown("plain *italic* **bold**");
    assert_eq!(doc.blocks.len(), 1);
    assert_eq!(doc.blocks[0].kind, BlockKind::Paragraph);
    let spans = &doc.blocks[0].lines[0].spans;
    let italic = spans.iter().find(|s| s.text == "italic").unwrap();
    assert!(italic.attrs.italic && !italic.attrs.bold);
    let bold = spans.iter().find(|s| s.text == "bold").unwrap();
    assert!(bold.attrs.bold);
    assert_eq!(bold.role, StyleRole::Strong);
}

#[test]
fn parses_inline_code() {
    let doc = parse_markdown("use `cargo build` now");
    let spans = &doc.blocks[0].lines[0].spans;
    let code = spans.iter().find(|s| s.text == "cargo build").unwrap();
    assert_eq!(code.role, StyleRole::Code);
    assert_eq!(code.fill, FillRole::Code);
}

#[test]
fn parses_fenced_code_block() {
    let md = "```rust\nfn main() {}\nlet x = 1;\n```";
    let doc = parse_markdown(md);
    assert_eq!(
        block_kinds(&doc),
        vec![&BlockKind::CodeBlock {
            language: Some("rust".to_string())
        }]
    );
    let lines: Vec<String> = doc.blocks[0].lines.iter().map(|l| l.plain_text()).collect();
    assert_eq!(lines, vec!["fn main() {}", "let x = 1;"]);
    assert!(
        doc.blocks[0].lines[0]
            .spans
            .iter()
            .all(|s| s.fill == FillRole::Code)
    );
}

#[test]
fn parses_unordered_list_with_markers() {
    let doc = parse_markdown("- one\n- two");
    let items: Vec<_> = doc
        .blocks
        .iter()
        .filter(|b| matches!(b.kind, BlockKind::ListItem { .. }))
        .collect();
    assert_eq!(items.len(), 2);
    assert!(items[0].lines[0].plain_text().starts_with("• "));
    assert!(items[0].lines[0].plain_text().contains("one"));
}

#[test]
fn parses_ordered_list_numbers() {
    let doc = parse_markdown("1. first\n2. second");
    let items: Vec<_> = doc
        .blocks
        .iter()
        .filter(|b| matches!(b.kind, BlockKind::ListItem { ordered: true, .. }))
        .collect();
    assert_eq!(items.len(), 2);
    assert!(items[0].lines[0].plain_text().starts_with("1. "));
    assert!(items[1].lines[0].plain_text().starts_with("2. "));
}

#[test]
fn parses_nested_list_depth() {
    let doc = parse_markdown("- top\n  - nested");
    let depths: Vec<usize> = doc
        .blocks
        .iter()
        .filter_map(|b| match b.kind {
            BlockKind::ListItem { depth, .. } => Some(depth),
            _ => None,
        })
        .collect();
    assert!(depths.contains(&0));
    assert!(depths.contains(&1));
}

#[test]
fn parses_blockquote() {
    let doc = parse_markdown("> quoted text");
    assert!(
        doc.blocks
            .iter()
            .any(|b| b.kind == BlockKind::BlockQuote && b.lines[0].plain_text().contains("quoted"))
    );
}

#[test]
fn parses_thematic_break() {
    let doc = parse_markdown("a\n\n---\n\nb");
    assert!(
        doc.blocks
            .iter()
            .any(|b| b.kind == BlockKind::ThematicBreak)
    );
}

#[test]
fn wrap_preserves_styling_and_width() {
    let line = StyledLine::from_spans(vec![
        StyledSpan::new("hello ", StyleRole::Text),
        StyledSpan::new("beautiful", StyleRole::Strong).bold(),
        StyledSpan::new(" world", StyleRole::Text),
    ]);
    let wrapped = wrap_line(&line, 8, &ColumnWidth);
    assert!(wrapped.len() > 1);
    // Every produced line is within the width budget.
    for l in &wrapped {
        assert!(
            ColumnWidth.measure(&l.plain_text()) <= 8,
            "line too wide: {:?}",
            l.plain_text()
        );
    }
    // The bold word retains its styling across the wrap (it may be split).
    let has_bold = wrapped
        .iter()
        .flat_map(|l| l.spans.iter())
        .any(|s| s.attrs.bold && s.role == StyleRole::Strong && !s.text.trim().is_empty());
    assert!(has_bold);
}

#[test]
fn wrap_hard_splits_overlong_word() {
    let line = StyledLine::from_spans(vec![StyledSpan::plain("abcdefghij")]);
    let wrapped = wrap_line(&line, 4, &ColumnWidth);
    assert!(wrapped.len() >= 3);
    for l in &wrapped {
        assert!(ColumnWidth.measure(&l.plain_text()) <= 4);
    }
    let joined: String = wrapped.iter().map(|l| l.plain_text()).collect();
    assert_eq!(joined, "abcdefghij");
}

#[test]
fn wrap_keeps_alignment() {
    let line = StyledLine::aligned(
        vec![StyledSpan::plain("one two three four five")],
        Alignment::Center,
    );
    let wrapped = wrap_line(&line, 7, &ColumnWidth);
    assert!(wrapped.iter().all(|l| l.alignment == Alignment::Center));
}

#[test]
fn wrap_noop_when_fits() {
    let line = StyledLine::from_spans(vec![StyledSpan::plain("short")]);
    let wrapped = wrap_line(&line, 80, &ColumnWidth);
    assert_eq!(wrapped.len(), 1);
    assert_eq!(wrapped[0].plain_text(), "short");
}
