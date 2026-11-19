#[test]
fn code_highlighting_does_not_leak_line_comment_scope_into_following_lines() {
    // Highlighting is fed one terminator-less line at a time, so a `//` comment
    // must end with its line instead of coloring the rest of the block.
    let code = "package main

func main() {
// Simulate work.
println(\"hello\")
}";
    let comment = crate::highlight_line("// Simulate work.", Some("go"));
    let comment_fg = comment[0].style.fg;
    assert!(comment_fg.is_some());

    let highlighted = crate::render_support::highlight_code(code, Some("go"));
    assert_eq!(highlighted.len(), 6);
    let comment_line = &highlighted[3];
    assert!(comment_line.spans.iter().all(|s| s.style.fg == comment_fg));
    for line in &highlighted[4..] {
        assert!(
            line.spans.iter().any(|s| s.style.fg != comment_fg),
            "line kept comment color: {:?}",
            line_to_string(line)
        );
    }

    // Every single-line highlight must match what the multi-line pass produced.
    for (idx, text) in code.lines().enumerate().skip(4) {
        let alone = crate::highlight_line(text, Some("go"));
        let alone_fg: Vec<_> = alone.iter().map(|s| (s.content.as_ref(), s.style.fg)).collect();
        let block_fg: Vec<_> = highlighted[idx]
            .spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style.fg))
            .collect();
        assert_eq!(alone_fg, block_fg, "line {idx}");
    }
}

#[test]
fn highlight_file_lines_does_not_leak_line_comment_scope() {
    let source = "// first
x := 1
// second
y := 2
";
    let comment_fg = crate::highlight_line("// first", Some("go"))[0].style.fg;
    let rows = crate::highlight_file_lines(source, Some("go"), &[1, 2, 3, 4]);
    assert_eq!(rows.len(), 4);
    for (line_number, spans) in rows {
        let all_comment = spans.iter().all(|s| s.style.fg == comment_fg);
        assert_eq!(all_comment, line_number % 2 == 1, "line {line_number}");
    }
}

#[test]
fn markdown_go_fence_keeps_highlighting_after_a_line_comment() {
    let lines = render_markdown("```go
// Simulate work.
println(\"hello\")
```");
    let comment_fg = crate::highlight_line("// Simulate work.", Some("go"))[0].style.fg;
    let call = lines
        .iter()
        .find(|line| line_to_string(line).contains("println"))
        .expect("code line");
    assert!(
        call.spans
            .iter()
            .any(|s| s.content.contains("println") && s.style.fg != comment_fg),
        "println kept comment color"
    );
}

#[test]
fn markdown_code_fence_expands_tabs_to_four_cell_stops() {
    // Ratatui draws `\t` as nothing, so tab-indented code must arrive as spaces.
    // Stops count from the start of the code line, after wide characters too.
    let lines = render_markdown("```go\nfunc main() {\n\tif ok {\n\t\treturn\n\t}\n界\tx\nab\tc\n}\n```");
    let body: Vec<String> = lines
        .iter()
        .map(line_to_string)
        .filter_map(|text| text.trim_start().strip_prefix("│ ").map(str::to_string))
        .collect();
    assert_eq!(
        body,
        [
            "func main() {",
            "    if ok {",
            "        return",
            "    }",
            "界  x",
            "ab  c",
            "}",
        ]
    );
    assert!(lines.iter().all(|line| !line_to_string(line).contains('\t')));
}

#[test]
fn code_highlighting_keeps_multiline_state_and_ends_hash_comments() {
    // State that must span lines still does; line comments in other languages end.
    let block = crate::render_support::highlight_code("/* a
 b
*/
int x = 1;", Some("c"));
    let comment_fg = crate::highlight_line("/* a */", Some("c"))[0].style.fg;
    assert!(block[1].spans.iter().all(|s| s.style.fg == comment_fg));
    assert!(block[3].spans.iter().any(|s| s.style.fg != comment_fg));

    let hash_fg = crate::highlight_line("# note", Some("py"))[0].style.fg;
    let python = crate::render_support::highlight_code("# note
x = 1", Some("py"));
    assert!(python[0].spans.iter().all(|s| s.style.fg == hash_fg));
    assert!(python[1].spans.iter().any(|s| s.style.fg != hash_fg));
}
