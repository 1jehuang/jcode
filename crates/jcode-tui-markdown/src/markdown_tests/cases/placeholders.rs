// Placeholder-preservation tests: image/diagram placeholder bodies are blank
// lines by design, and block-separator normalization must never collapse them.

#[cfg(feature = "mermaid-renderer")]
#[test]
fn test_blockquote_separators_preserve_image_fill_rows() {
    let mut lines = vec![Line::from("│ before"), Line::default()];
    lines.extend(jcode_tui_mermaid::inline_image_placeholder_lines(0xabcdef, 4, 40));
    lines.push(Line::default());
    lines.push(Line::from("│ after"));
    lines.push(Line::default());
    fill_blockquote_separators(&mut lines, 1);
    assert_eq!(line_to_string(&lines[1]), "│ ");
    assert!(lines[3..6].iter().all(line_is_blank));
    assert_eq!(line_to_string(&lines[6]), "│ ");
    assert!(line_is_blank(lines.last().unwrap()));
}

#[cfg(feature = "mermaid-renderer")]
#[test]
fn test_normalize_block_separators_preserves_inline_image_placeholder_body() {
    let rows = 12u16;
    let mut lines = vec![Line::from("before")];
    lines.push(Line::from(""));
    lines.extend(jcode_tui_mermaid::inline_image_placeholder_lines(
        0xabcdef, rows, 40,
    ));
    lines.push(Line::from(""));
    lines.push(Line::from("after"));

    normalize_block_separators(&mut lines);

    let marker_idx = lines
        .iter()
        .position(|line| jcode_tui_mermaid::parse_inline_image_placeholder(line).is_some())
        .expect("placeholder marker must survive normalization");
    let blank_run = lines[marker_idx + 1..]
        .iter()
        .take_while(|line| line_is_blank(line))
        .count();
    assert!(
        blank_run >= (rows - 1) as usize,
        "placeholder body must keep its {} blank fill lines, found {} (lines: {:?})",
        rows - 1,
        blank_run,
        lines.iter().map(line_to_string).collect::<Vec<_>>()
    );
    assert_eq!(
        line_to_string(lines.last().expect("content after placeholder")),
        "after",
        "content after the placeholder must remain"
    );
}

#[cfg(feature = "mermaid-renderer")]
#[test]
fn test_normalize_block_separators_keeps_trailing_placeholder_body() {
    // A diagram at the very end of a message: trailing-blank trimming must not
    // eat the placeholder's blank fill lines.
    let rows = 8u16;
    let mut lines = vec![Line::from("intro")];
    lines.push(Line::from(""));
    lines.extend(jcode_tui_mermaid::inline_image_placeholder_lines(
        0x123456, rows, 30,
    ));

    normalize_block_separators(&mut lines);

    let marker_idx = lines
        .iter()
        .position(|line| jcode_tui_mermaid::parse_inline_image_placeholder(line).is_some())
        .expect("placeholder marker must survive normalization");
    assert_eq!(
        lines.len() - marker_idx,
        rows as usize,
        "trailing placeholder must keep marker + {} blank rows (lines: {:?})",
        rows - 1,
        lines.iter().map(line_to_string).collect::<Vec<_>>()
    );
}

#[cfg(feature = "mermaid-renderer")]
#[test]
fn test_normalize_block_separators_still_collapses_ordinary_blank_runs() {
    let mut lines = vec![
        Line::from("a"),
        Line::from(""),
        Line::from(""),
        Line::from(""),
        Line::from("b"),
        Line::from(""),
    ];

    normalize_block_separators(&mut lines);

    let rendered: Vec<String> = lines.iter().map(line_to_string).collect();
    assert_eq!(rendered, vec!["a", "", "b"]);
}
