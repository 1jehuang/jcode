use jcode_tui_markdown::{render_markdown_with_width, render_markdown_with_width_and_prose_color};
use ratatui::{
    style::{Color, Modifier, Style},
    text::Line,
};

fn style_of(lines: &[Line<'_>], text: &str) -> Style {
    lines
        .iter()
        .flat_map(|line| &line.spans)
        .find(|span| span.content == text)
        .unwrap_or_else(|| panic!("missing span {text:?}: {lines:?}"))
        .style
}

#[test]
fn prose_color_preserves_markdown_modifiers_and_structured_text() {
    let source = "normal **bold** *italic* ~~strike~~\n\n# Heading\n\n- item\n\n> quoted";
    let lines = render_markdown_with_width_and_prose_color(source, Some(80), Color::Magenta);
    for text in [
        "normal ", "bold", "italic", "strike", "Heading", "item", "quoted",
    ] {
        assert_eq!(style_of(&lines, text).fg, Some(Color::Magenta), "{text}");
    }
    for (text, modifier) in [
        ("bold", Modifier::BOLD),
        ("italic", Modifier::ITALIC),
        ("strike", Modifier::CROSSED_OUT),
        ("Heading", Modifier::BOLD | Modifier::UNDERLINED),
    ] {
        assert!(
            style_of(&lines, text).add_modifier.contains(modifier),
            "{text}"
        );
    }
}

#[test]
fn prose_color_preserves_code_links_images_and_diff_styles() {
    let source = "prose `inline` [link](https://example.com) ![alt](image.png)\n\n```rust\nfn main() {}\n```\n\n```diff\n-old\n+new\n```";
    let normal = render_markdown_with_width(source, Some(80));
    let tinted = render_markdown_with_width_and_prose_color(source, Some(80), Color::Magenta);
    assert_eq!(normal.len(), tinted.len());
    for (normal, tinted) in normal.iter().zip(&tinted) {
        assert_eq!(normal.style, tinted.style);
        assert_eq!(normal.spans.len(), tinted.spans.len());
        for (normal, tinted) in normal.spans.iter().zip(&tinted.spans) {
            assert_eq!(normal.content, tinted.content);
            if normal.content == "prose " || normal.content == " " {
                continue;
            }
            assert_eq!(normal.style, tinted.style, "{}", normal.content);
        }
    }
}

#[test]
fn prose_color_is_local_and_normal_rendering_is_unchanged() {
    let source = "normal **bold** *italic*\n\n# Heading\n\n```rust\nfn main() {}\n```";
    let normal = render_markdown_with_width(source, Some(80));
    let first = render_markdown_with_width_and_prose_color(source, Some(80), Color::Red);
    let second = render_markdown_with_width_and_prose_color(source, Some(80), Color::Blue);
    assert_eq!(style_of(&first, "normal ").fg, Some(Color::Red));
    assert_eq!(style_of(&second, "normal ").fg, Some(Color::Blue));
    assert_eq!(
        first,
        render_markdown_with_width_and_prose_color(source, Some(80), Color::Red)
    );
    assert_eq!(normal, render_markdown_with_width(source, Some(80)));
}

#[test]
fn prose_color_keeps_semantic_inline_styles_inside_headings() {
    let lines = render_markdown_with_width_and_prose_color(
        "# Heading `code` [link](https://example.com) ![alt](image.png)",
        Some(80),
        Color::Magenta,
    );
    assert_eq!(style_of(&lines, "Heading ").fg, Some(Color::Magenta));
    for text in ["code", "link", "[image: alt] (image.png)"] {
        assert_ne!(style_of(&lines, text).fg, Some(Color::Magenta));
    }
    assert!(style_of(&lines, "code").bg.is_some());
}
