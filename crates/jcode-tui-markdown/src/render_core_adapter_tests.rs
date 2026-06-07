//! Parity checks: the shared-core adapter vs. the legacy renderer.
//!
//! The two renderers differ in spacing details and some decorative styling, so
//! these tests assert *content* parity (the visible text, modulo blank-line
//! padding and decorative markers) plus key styling invariants, rather than
//! byte-identical `Line` equality. The goal is to prove the shared core
//! reproduces the legacy renderer's meaning before any switchover.

use crate::{render_markdown, render_markdown_via_core};
use ratatui::text::Line;

/// Visible text of each non-blank line, trimmed, for loose comparison.
fn nonblank_texts(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Concatenated visible text with whitespace collapsed, for content-equality
/// checks that ignore layout/spacing differences.
fn flattened(lines: &[Line<'static>]) -> String {
    let joined = nonblank_texts(lines).join(" ");
    joined.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn assert_content_parity(md: &str) {
    let legacy = render_markdown(md);
    let core = render_markdown_via_core(md);
    assert_eq!(
        flattened(&core),
        flattened(&legacy),
        "content mismatch for input:\n{md}\n--- legacy ---\n{:?}\n--- core ---\n{:?}",
        nonblank_texts(&legacy),
        nonblank_texts(&core),
    );
}

#[test]
fn parity_plain_paragraph() {
    assert_content_parity("Hello world, this is a paragraph.");
}

#[test]
fn parity_heading_and_paragraph() {
    assert_content_parity("# Title\n\nSome body text here.");
}

#[test]
fn parity_emphasis() {
    assert_content_parity("This is *italic* and **bold** and `code`.");
}

#[test]
fn parity_unordered_list() {
    assert_content_parity("- alpha\n- beta\n- gamma");
}

#[test]
fn parity_ordered_list() {
    assert_content_parity("1. first\n2. second\n3. third");
}

#[test]
fn parity_code_block() {
    assert_content_parity("```rust\nfn main() {\n    println!(\"hi\");\n}\n```");
}

#[test]
fn parity_blockquote() {
    assert_content_parity("> a quoted line");
}

#[test]
fn parity_mixed_document() {
    let md = "\
# Heading

Intro paragraph with **bold** and a `snippet`.

- one
- two

Closing line.";
    assert_content_parity(md);
}

#[test]
fn core_marks_bold_and_code_styling() {
    let core = render_markdown_via_core("text **bold** and `code`");
    let spans: Vec<_> = core.iter().flat_map(|l| l.spans.iter()).collect();
    assert!(
        spans.iter().any(|s| s.content.contains("bold")
            && s.style
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD)),
        "bold word should carry BOLD modifier"
    );
    assert!(
        spans
            .iter()
            .any(|s| s.content.contains("code") && s.style.bg.is_some()),
        "inline code should carry a background fill"
    );
}

#[test]
fn parity_table() {
    let md = "\
| A | B |
|---|---|
| 1 | 2 |";
    assert_content_parity(md);
}

#[test]
fn core_renders_table_borders() {
    let core = render_markdown_via_core("| A | B |\n|---|---|\n| 1 | 2 |");
    let text: String = core
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(
        text.contains('A') && text.contains('1'),
        "table cells present: {text}"
    );
}

#[test]
fn core_renders_inline_math() {
    let core = render_markdown_via_core("an equation $x^2$ here");
    let spans: Vec<_> = core.iter().flat_map(|l| l.spans.iter()).collect();
    assert!(
        spans.iter().any(|s| s.content.contains("$x^2$")),
        "inline math should be wrapped in dollar signs"
    );
}

#[test]
fn core_renders_display_math_frame() {
    let core = render_markdown_via_core("$$\nx^2 + y^2\n$$");
    let texts = nonblank_texts(&core);
    assert!(
        texts.iter().any(|t| t.starts_with("┌─ math")),
        "display math should be framed: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("x^2 + y^2")),
        "display math content present: {texts:?}"
    );
}

#[test]
fn parity_currency_dollars() {
    assert_content_parity("It costs $35 and then $5.99 total.");
}

#[test]
fn parity_two_currency_amounts_not_math() {
    // Legacy escapes $-then-digit so it is NOT treated as inline math.
    assert_content_parity("Spend $5 here and $10 there for $15.");
}

#[test]
fn probe_math_divergence() {
    let math = crate::math_fg();
    for input in [
        "$5x$ and more",
        "price $5$ each",
        "$5+$3 = $8",
        "a $1 and $2 b",
        "x$5$y",
        "buy $5 sell $9 net $4",
        "$$\nx=5\n$$",
        "inline $a+b$ ok",
    ] {
        let cm: Vec<String> = render_markdown_via_core(input)
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.style.fg == Some(math))
            .map(|s| s.content.to_string())
            .collect();
        let lm: Vec<String> = render_markdown(input)
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.style.fg == Some(math))
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(cm, lm, "math styling mismatch for {input:?}");
    }
}

#[test]
fn fuzz_visible_text_parity() {
    // Differential corpus: visible (flattened) text must match the legacy
    // renderer across a wide variety of constructs.
    let corpus = [
        "# H1\n## H2\n### H3",
        "plain paragraph with words",
        "**bold** _italic_ ~~strike~~ `code`",
        "- a\n- b\n  - nested\n- c",
        "1. one\n2. two\n3. three",
        "> quote line one\n> quote line two",
        "```rust\nfn f() {}\n```",
        "| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |",
        "text $a+b$ inline and $$x=5$$ display",
        "money $5 and $9.99 here",
        "[link](http://example.com) text",
        "a\n\nb\n\nc",
        "* item with **bold** and `code`",
        "Term\n: definition",
        "Mixed: # not heading inline",
        "line with  \nhard break",
        "1. step\n   - sub a\n   - sub b\n2. step two",
        "> nested\n> > deeper quote",
        "Para with $35 cost then math $y=2$.",
        "---\nafter rule",
        "***\nstars rule",
        "nested **bold _italic_ end** tail",
        "`code with $5` outside $6",
        "1. a\n2. b\n   1. b1\n   2. b2\n3. c",
        "- [ ] todo\n- [x] done",
        "para one\n\n> quote\n\npara two",
        "| left | right |\n|:-----|------:|\n| a | b |",
        "text with ![alt](img.png) image",
        "> quote with **bold** and `code`",
        "## Heading with `code` and **bold**",
        "Mixed $$\\sum_i x_i$$ display in para",
        "emoji 😀 and CJK 中文 text",
        "trailing spaces   \nnext line",
        "$5.00, $6, and $a=b$ together",
        "footnote ref[^1]\n\n[^1]: the note",
        "auto link <http://example.com> here",
        "> - quoted list item\n> - second",
        "Heading\n=======\n\nbody",
        "Sub\n---\n\nbody",
        "a\tb\tc tabs",
        "line\\\nwith backslash break",
        "**unclosed bold and `code",
        "| a |\n|---|\n| $5 |\n| $x$ |",
    ];
    for md in corpus {
        assert_eq!(
            flattened(&render_markdown_via_core(md)),
            flattened(&render_markdown(md)),
            "visible-text mismatch for:\n{md}"
        );
    }
}

// ------------------------------------------------------------------------
// Randomized differential fuzzing.
//
// A tiny xorshift PRNG drives a recursive markdown generator. Each generated
// document is rendered by both the shared core and the legacy renderer; their
// flattened visible text must match. Iteration count is controlled by the
// JCODE_MD_FUZZ_ITERS env var (default 5000) so CI stays fast while local deep
// runs can crank it up.

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
    fn chance(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const WORDS: &[&str] = &[
    "alpha",
    "beta",
    "gamma",
    "delta",
    "x",
    "y",
    "z",
    "the",
    "quick",
    "brown",
    "fox",
    "中文",
    "데이터",
    "emoji",
    "lorem",
    "ipsum",
    "a",
    "I",
    "we",
    "code",
];

fn gen_words(rng: &mut Rng, max: usize) -> String {
    let n = 1 + rng.below(max);
    (0..n)
        .map(|_| *rng.pick(WORDS))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Generate an inline fragment (no leading/trailing block structure).
fn gen_inline(rng: &mut Rng, depth: usize) -> String {
    match rng.below(if depth > 3 { 4 } else { 9 }) {
        0 => gen_words(rng, 4),
        1 => format!("**{}**", gen_words(rng, 3)),
        2 => format!("_{}_", gen_words(rng, 3)),
        3 => format!("`{}`", gen_words(rng, 2)),
        4 => format!("~~{}~~", gen_words(rng, 2)),
        5 => format!(
            "[{}](http://example.com/{})",
            gen_words(rng, 2),
            rng.below(99)
        ),
        6 => format!("${}+{}$", rng.pick(WORDS), rng.pick(WORDS)),
        7 => format!("${}", rng.below(999)), // currency
        _ => format!(
            "{} {} {}",
            gen_words(rng, 2),
            gen_inline(rng, depth + 1),
            gen_words(rng, 2)
        ),
    }
}

/// Generate a block-level fragment.
fn gen_block(rng: &mut Rng, depth: usize) -> String {
    match rng.below(if depth > 2 { 6 } else { 11 }) {
        0 => gen_inline(rng, 0),
        1 => {
            let level = 1 + rng.below(3);
            format!("{} {}", "#".repeat(level), gen_inline(rng, 0))
        }
        2 => {
            // unordered list
            let n = 1 + rng.below(3);
            (0..n)
                .map(|_| format!("- {}", gen_inline(rng, 0)))
                .collect::<Vec<_>>()
                .join("\n")
        }
        3 => {
            // ordered list
            let n = 1 + rng.below(3);
            (0..n)
                .map(|i| format!("{}. {}", i + 1, gen_inline(rng, 0)))
                .collect::<Vec<_>>()
                .join("\n")
        }
        4 => {
            // fenced code block
            let lang = if rng.chance(2) { "rust" } else { "" };
            let n = 1 + rng.below(3);
            let body = (0..n)
                .map(|_| format!("let {} = {};", rng.pick(WORDS), rng.below(99)))
                .collect::<Vec<_>>()
                .join("\n");
            format!("```{lang}\n{body}\n```")
        }
        5 => "---".to_string(),
        6 => {
            // table
            let cols = 1 + rng.below(3);
            let header: Vec<String> = (0..cols).map(|_| gen_words(rng, 1)).collect();
            let sep: Vec<&str> = (0..cols).map(|_| "---").collect();
            let rows = 1 + rng.below(3);
            let mut out = format!("| {} |\n| {} |", header.join(" | "), sep.join(" | "));
            for _ in 0..rows {
                let cells: Vec<String> = (0..cols).map(|_| gen_words(rng, 2)).collect();
                out.push_str(&format!("\n| {} |", cells.join(" | ")));
            }
            out
        }
        7 => {
            // blockquote (possibly nested / multiline)
            let n = 1 + rng.below(3);
            (0..n)
                .map(|_| {
                    let prefix = "> ".repeat(1 + rng.below(2));
                    format!("{}{}", prefix, gen_inline(rng, 0))
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        8 => {
            // task list
            let n = 1 + rng.below(3);
            (0..n)
                .map(|_| {
                    let mark = if rng.chance(2) { "x" } else { " " };
                    format!("- [{}] {}", mark, gen_inline(rng, 0))
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        9 => {
            // definition list
            format!("{}\n: {}", gen_words(rng, 2), gen_inline(rng, 0))
        }
        _ => {
            // footnote
            format!(
                "{}[^{n}]\n\n[^{n}]: {}",
                gen_inline(rng, 0),
                gen_inline(rng, 0),
                n = rng.below(50)
            )
        }
    }
}

fn gen_document(rng: &mut Rng) -> String {
    let n = 1 + rng.below(5);
    (0..n)
        .map(|_| gen_block(rng, 0))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[test]
fn fuzz_random_documents_parity() {
    let iters: u64 = std::env::var("JCODE_MD_FUZZ_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5000);
    let base_seed: u64 = std::env::var("JCODE_MD_FUZZ_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0x9E3779B97F4A7C15);

    let mut failures = Vec::new();
    for i in 0..iters {
        let mut rng = Rng::new(base_seed.wrapping_add(i.wrapping_mul(0x100000001B3)));
        let md = gen_document(&mut rng);
        let core = flattened(&render_markdown_via_core(&md));
        let legacy = flattened(&render_markdown(&md));
        if core != legacy {
            failures.push((i, md, core, legacy));
            if failures.len() >= 5 {
                break;
            }
        }
    }
    assert!(
        failures.is_empty(),
        "random-document parity mismatches ({} shown):\n{}",
        failures.len(),
        failures
            .iter()
            .map(|(i, md, core, legacy)| format!(
                "--- iter {i} ---\nINPUT:\n{md}\nCORE:   {core:?}\nLEGACY: {legacy:?}"
            ))
            .collect::<Vec<_>>()
            .join("\n\n")
    );
}

/// Stricter than `flattened`: compares the per-line visible text (trimmed,
/// blanks dropped) so line-structure/break divergences are caught, not just
/// whitespace-collapsed content.
#[test]
fn fuzz_random_documents_line_structure() {
    let iters: u64 = std::env::var("JCODE_MD_FUZZ_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5000);
    let base_seed: u64 = std::env::var("JCODE_MD_FUZZ_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0x1234_5678_9ABC_DEF0);

    let mut failures = Vec::new();
    for i in 0..iters {
        let mut rng = Rng::new(base_seed.wrapping_add(i.wrapping_mul(0x100000001B3)));
        let md = gen_document(&mut rng);
        let core = nonblank_texts(&render_markdown_via_core(&md));
        let legacy = nonblank_texts(&render_markdown(&md));
        if core != legacy {
            failures.push((i, md, core, legacy));
            if failures.len() >= 5 {
                break;
            }
        }
    }
    assert!(
        failures.is_empty(),
        "line-structure parity mismatches ({} shown):\n{}",
        failures.len(),
        failures
            .iter()
            .map(|(i, md, core, legacy)| format!(
                "--- iter {i} ---\nINPUT:\n{md}\nCORE:   {core:#?}\nLEGACY: {legacy:#?}"
            ))
            .collect::<Vec<_>>()
            .join("\n\n")
    );
}

#[test]
fn fuzz_random_documents_wrapped_parity() {
    use crate::render_markdown_via_core_wrapped;
    let iters: u64 = std::env::var("JCODE_MD_FUZZ_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000);
    let base_seed: u64 = std::env::var("JCODE_MD_FUZZ_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0xCAFE_F00D_DEAD_BEEF);

    let widths = [20usize, 40, 80];
    let mut failures = Vec::new();
    'outer: for i in 0..iters {
        for &w in &widths {
            let mut rng = Rng::new(base_seed.wrapping_add(i.wrapping_mul(0x100000001B3)));
            let md = gen_document(&mut rng);
            let core = nonblank_texts(&render_markdown_via_core_wrapped(&md, w));
            let legacy = nonblank_texts(&crate::wrap_lines(
                crate::render_markdown_with_width(&md, Some(w)),
                w,
            ));
            if core != legacy {
                failures.push((i, w, md, core, legacy));
                if failures.len() >= 5 {
                    break 'outer;
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "wrapped parity mismatches ({} shown):\n{}",
        failures.len(),
        failures
            .iter()
            .map(|(i, w, md, core, legacy)| format!(
                "--- iter {i} width {w} ---\nINPUT:\n{md}\nCORE:   {core:?}\nLEGACY: {legacy:?}"
            ))
            .collect::<Vec<_>>()
            .join("\n\n")
    );
}
