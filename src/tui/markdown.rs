use pulldown_cmark::{Event, Parser, Tag, TagEnd, CodeBlockKind};
use ratatui::prelude::*;
use syntect::highlighting::{ThemeSet, Style as SynStyle};
use syntect::parsing::SyntaxSet;
use syntect::easy::HighlightLines;
use std::sync::LazyLock;

// Syntax highlighting resources (loaded once)
static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(|| SyntaxSet::load_defaults_newlines());
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

// Colors matching ui.rs palette
const CODE_BG: Color = Color::Rgb(40, 40, 40);
const CODE_FG: Color = Color::Rgb(200, 200, 200);
const BOLD_COLOR: Color = Color::Rgb(255, 255, 255);
const ITALIC_COLOR: Color = Color::Rgb(180, 180, 180);
const LINK_COLOR: Color = Color::Rgb(138, 180, 248);

/// Render markdown text to styled ratatui Lines
pub fn render_markdown(text: &str) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();

    // Style stack for nested formatting
    let mut bold = false;
    let mut italic = false;
    let mut in_code_span = false;
    let mut in_code_block = false;
    let mut code_block_lang: Option<String> = None;
    let mut code_block_content = String::new();

    let parser = Parser::new(text);

    for event in parser {
        match event {
            Event::Start(Tag::Strong) => bold = true,
            Event::End(TagEnd::Strong) => bold = false,

            Event::Start(Tag::Emphasis) => italic = true,
            Event::End(TagEnd::Emphasis) => italic = false,

            Event::Start(Tag::CodeBlock(kind)) => {
                // Flush current line before code block
                if !current_spans.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                }
                in_code_block = true;
                code_block_lang = match kind {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => Some(lang.to_string()),
                    _ => None,
                };
                code_block_content.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                // Render code block with syntax highlighting
                let highlighted = highlight_code(&code_block_content, code_block_lang.as_deref());
                lines.extend(highlighted);
                in_code_block = false;
                code_block_lang = None;
                code_block_content.clear();
            }

            Event::Code(code) => {
                // Inline code
                current_spans.push(Span::styled(
                    format!(" {} ", code),
                    Style::default().fg(CODE_FG).bg(CODE_BG),
                ));
            }

            Event::Text(text) => {
                if in_code_block {
                    code_block_content.push_str(&text);
                } else {
                    let style = match (bold, italic) {
                        (true, true) => Style::default().fg(BOLD_COLOR).bold().italic(),
                        (true, false) => Style::default().fg(BOLD_COLOR).bold(),
                        (false, true) => Style::default().fg(ITALIC_COLOR).italic(),
                        (false, false) => Style::default(),
                    };
                    current_spans.push(Span::styled(text.to_string(), style));
                }
            }

            Event::SoftBreak | Event::HardBreak => {
                if !in_code_block {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                }
            }

            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                if !current_spans.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                }
            }

            Event::Start(Tag::Link { dest_url, .. }) => {
                // Just show link text in link color
            }
            Event::End(TagEnd::Link) => {}

            _ => {}
        }
    }

    // Flush remaining spans
    if !current_spans.is_empty() {
        lines.push(Line::from(current_spans));
    }

    lines
}

/// Highlight a code block with syntax highlighting
fn highlight_code(code: &str, lang: Option<&str>) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Try to find syntax for the language
    let syntax = lang
        .and_then(|l| SYNTAX_SET.find_syntax_by_token(l))
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());

    let theme = &THEME_SET.themes["base16-ocean.dark"];
    let mut highlighter = HighlightLines::new(syntax, theme);

    for line in code.lines() {
        let highlighted = highlighter.highlight_line(line, &SYNTAX_SET);

        match highlighted {
            Ok(ranges) => {
                let spans: Vec<Span<'static>> = ranges
                    .into_iter()
                    .map(|(style, text)| {
                        Span::styled(
                            text.to_string(),
                            syntect_to_ratatui_style(style),
                        )
                    })
                    .collect();
                lines.push(Line::from(spans));
            }
            Err(_) => {
                // Fallback to plain text
                lines.push(Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(CODE_FG),
                )));
            }
        }
    }

    lines
}

/// Convert syntect style to ratatui style
fn syntect_to_ratatui_style(style: SynStyle) -> Style {
    let fg = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
    Style::default().fg(fg)
}

/// Create a progress bar string
pub fn progress_bar(progress: f32, width: usize) -> String {
    let filled = (progress * width as f32) as usize;
    let empty = width.saturating_sub(filled);

    let bar: String = std::iter::repeat('█')
        .take(filled)
        .chain(std::iter::repeat('░').take(empty))
        .collect();

    bar
}

/// Create a styled progress bar line
pub fn progress_line(label: &str, progress: f32, width: usize) -> Line<'static> {
    let bar = progress_bar(progress, width.saturating_sub(label.len() + 3));
    let pct = (progress * 100.0) as u8;

    Line::from(vec![
        Span::styled(label.to_string(), Style::default().dim()),
        Span::raw(" "),
        Span::styled(bar, Style::default().fg(Color::Rgb(129, 199, 132))),
        Span::styled(format!(" {}%", pct), Style::default().dim()),
    ])
}

#[cfg(test)]
mod tests {
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
    fn test_progress_bar() {
        let bar = progress_bar(0.5, 10);
        assert_eq!(bar.chars().count(), 10);
    }
}
