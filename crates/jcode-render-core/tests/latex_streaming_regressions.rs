use jcode_render_core::{BlockKind, normalize_latex_math, parse_markdown};

fn math_display_count(markdown: &str) -> usize {
    parse_markdown(markdown)
        .blocks
        .iter()
        .filter(|block| block.kind == BlockKind::MathDisplay)
        .count()
}

#[test]
fn escaped_short_prose_lists_do_not_render_as_math() {
    for source in [
        r#"after encountering “\[old, new\]”. This leads to a third side effect"#,
        r"\[old, new\]",
        r"compare \(old, new\) before editing",
        r"choose \[yes, no, off\] here",
    ] {
        assert_eq!(normalize_latex_math(source), source);
        assert_eq!(math_display_count(source), 0, "{source:?}");
        let visible = parse_markdown(source)
            .blocks
            .iter()
            .flat_map(|block| &block.lines)
            .map(|line| line.plain_text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!visible.contains('\\'), "{visible:?}");
    }

    for source in [
        r"\[x, y\]",
        r"\[ab, cd\]",
        r"\[old, new = x\]",
        r"\[\mathrm{old}, \mathrm{new}\]",
        "\\[\nold, new\n\\]",
        "$$old, new$$",
    ] {
        assert_eq!(math_display_count(source), 1, "{source:?}");
    }
}

#[test]
fn parses_the_exact_multiline_equation_response_from_the_tui() {
    let response = concat!(
        "\\[\n\\boxed{\ne^{i\\pi}+1=0\n}\n\\]\n\n",
        "\\[\n\\int_{-\\infty}^{\\infty} e^{-x^2}\\,dx=\\sqrt{\\pi}\n\\]\n\n",
        "\\[\nx=\\frac{-b\\pm\\sqrt{b^2-4ac}}{2a}\n\\]\n\n",
        "\\[\n\\nabla\\cdot\\mathbf{E}=\\frac{\\rho}{\\varepsilon_0}\n\\]\n\n",
        "\\[\n\\frac{\\partial \\psi}{\\partial t}\n=\n",
        "\\alpha\\frac{\\partial^2\\psi}{\\partial x^2}\n\\]",
    );

    let normalized = normalize_latex_math(response);
    let parsed = parse_markdown(response);
    assert_eq!(
        math_display_count(response),
        5,
        "normalized={normalized:?} parsed={parsed:#?}"
    );
    assert_eq!(normalized.matches("$$").count(), 10);
}

#[test]
fn every_streaming_prefix_is_deterministic_and_the_complete_response_is_math() {
    let equation = concat!(
        "Result:\n\n\\[\n",
        "\\frac{\\partial \\psi}{\\partial t}\n=\n",
        "\\alpha\\frac{\\partial^2\\psi}{\\partial x^2}\n\\]",
    );

    for end in equation
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(equation.len()))
    {
        let prefix = &equation[..end];
        let first = parse_markdown(prefix);
        let second = parse_markdown(prefix);
        assert_eq!(
            first, second,
            "nondeterministic prefix ending at byte {end}"
        );
    }

    assert_eq!(math_display_count(equation), 1);
}

#[test]
fn display_math_survives_common_markdown_containers() {
    let cases = [
        ("> \\[\n> x^2\n> =\n> y^2\n> \\]", "blockquote"),
        ("- \\[\n  x^2\n  =\n  y^2\n  \\]", "bullet list"),
        ("1. \\[\n   x^2\n   =\n   y^2\n   \\]", "ordered list"),
    ];

    for (source, label) in cases {
        assert!(
            math_display_count(source) >= 1,
            "{label} lost display math after normalization: {:?}",
            normalize_latex_math(source)
        );
    }
}

#[test]
fn standalone_display_math_resists_markdown_block_interruptors() {
    let bodies = [
        "x\n=\ny",
        "x\n---\ny",
        "x\n\ny",
        "x\n# not a heading\ny",
        "x\n> not a quote\ny",
        "x\n- not a list\ny",
        "x\n1. not a list\ny",
        "x\n``` not a fence\ny",
    ];

    for body in bodies {
        for source in [format!("$$\n{body}\n$$"), format!("\\[\n{body}\n\\]")] {
            assert_eq!(
                math_display_count(&source),
                1,
                "block syntax escaped display math: {source:?}; normalized={:?}",
                normalize_latex_math(&source)
            );
        }
    }
}

#[test]
fn crlf_and_adjacent_display_blocks_remain_balanced_and_idempotent() {
    let source = "\\[\r\nx\r\n=\r\ny\r\n\\]\r\n\r\n$$\r\na+b\r\n$$";
    let normalized = normalize_latex_math(source);
    assert_eq!(math_display_count(source), 2, "{normalized:?}");
    assert_eq!(normalize_latex_math(&normalized), normalized);
    assert_eq!(normalized.matches("$$").count(), 4);
}

#[test]
fn stabilization_preserves_newlines_comments_and_is_idempotent() {
    let source = "\\[\na % this comment must still end at the newline\nb\n=\nc\n\\]";
    let normalized = normalize_latex_math(source);

    assert_eq!(
        normalized,
        "$$\na % this comment must still end at the newline\nb\n{}=\nc\n$$"
    );
    assert_eq!(normalize_latex_math(&normalized), normalized);
    assert_eq!(math_display_count(source), 1);
}

#[test]
fn nested_quote_list_display_math_keeps_its_container() {
    let source = "> - \\[\n>   x\n>   =\n>   y\n>   \\]";
    let normalized = normalize_latex_math(source);

    assert!(normalized.contains(">   {}="), "{normalized:?}");
    assert_eq!(math_display_count(source), 1, "{normalized:?}");
}

#[test]
fn escaped_and_literal_delimiters_never_become_math() {
    let cases = [
        r"\\[escaped\\]",
        r"`\\[inline code\\]`",
        "```text\n\\[fenced code\\]\n```",
        "    \\[indented code\\]",
        r"\\[missing close",
    ];

    for source in cases {
        assert_eq!(
            math_display_count(source),
            0,
            "literal case parsed as math: {source:?}"
        );
    }

    let unclosed_display = "$$\nx\n=\ny";
    assert_eq!(normalize_latex_math(unclosed_display), unclosed_display);
}

#[test]
fn standalone_multiline_inline_delimiters_promote_without_losing_content() {
    for source in [
        "\\(\nx\n=\ny\n\\)",
        "$\nx\n=\ny\n$",
        "> $\n> x\n> =\n> y\n> $",
        "- $\n  x\n  =\n  y\n  $",
        "$\r\nx\r\n=\r\ny\r\n$",
    ] {
        let normalized = normalize_latex_math(source);
        let visible = parse_markdown(source)
            .blocks
            .iter()
            .flat_map(|block| &block.lines)
            .map(|line| line.plain_text())
            .collect::<Vec<_>>()
            .join("\n");
        for expected in ["x", "=", "y"] {
            assert!(visible.contains(expected), "{source:?} => {visible:?}");
        }
        assert_eq!(
            math_display_count(source),
            1,
            "{source:?} => {normalized:?}"
        );
        assert_eq!(normalize_latex_math(&normalized), normalized);
    }

    let unclosed = "$\nx\n=\ny";
    assert_eq!(normalize_latex_math(unclosed), unclosed);

    let fenced = "```text\n$\nx\n$\n```";
    assert_eq!(normalize_latex_math(fenced), fenced);
    assert_eq!(math_display_count(fenced), 0);
}
