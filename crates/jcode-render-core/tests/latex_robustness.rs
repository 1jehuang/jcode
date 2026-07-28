use jcode_render_core::{
    BlockKind, StyleRole, normalize_latex_math, parse_markdown, render_display_latex,
    render_inline_latex,
};
use unicode_width::UnicodeWidthStr;

fn assert_visible_and_deterministic(source: &str) {
    let inline = render_inline_latex(source);
    let display = render_display_latex(source);
    assert_eq!(inline, render_inline_latex(source), "inline: {source:?}");
    assert_eq!(display, render_display_latex(source), "display: {source:?}");
    if !source.trim().is_empty() {
        assert!(
            !inline.trim().is_empty(),
            "inline content vanished: {source:?}"
        );
        assert!(
            display.iter().any(|line| !line.trim().is_empty()),
            "display content vanished: {source:?}"
        );
    }
}

#[test]
fn renders_the_supported_symbol_vocabulary() {
    let cases = [
        (
            r"\alpha \beta \gamma \delta \epsilon \varepsilon",
            "α β γ δ ε ε",
        ),
        (
            r"\theta \vartheta \lambda \mu \pi \varpi \phi \varphi \omega",
            "θ ϑ λ μ π ϖ φ ϕ ω",
        ),
        (
            r"\Gamma \Delta \Theta \Lambda \Xi \Pi \Sigma \Phi \Psi \Omega",
            "Γ Δ Θ Λ Ξ Π Σ Φ Ψ Ω",
        ),
        (
            r"\sum \prod \coprod \int \iint \iiint \oint \partial \nabla \infty",
            "∑ ∏ ∐ ∫ ∬ ∭ ∮ ∂ ∇ ∞",
        ),
        (
            r"\times \div \cdot \circ \pm \mp \ast \star",
            "× ÷ · ∘ ± ∓ ∗ ⋆",
        ),
        (
            r"\le \leq \ge \geq \ne \neq \approx \sim \simeq \equiv \propto",
            "≤ ≤ ≥ ≥ ≠ ≠ ≈ ∼ ≃ ≡ ∝",
        ),
        (
            r"\in \notin \ni \subset \supset \subseteq \supseteq \cup \cap \setminus",
            "∈ ∉ ∋ ⊂ ⊃ ⊆ ⊇ ∪ ∩ ∖",
        ),
        (
            r"\forall \exists \nexists \neg \land \lor \oplus \otimes \vdash \models",
            "∀ ∃ ∄ ¬ ∧ ∨ ⊕ ⊗ ⊢ ⊨",
        ),
        (
            r"\to \leftarrow \leftrightarrow \Rightarrow \Leftarrow \Leftrightarrow \mapsto \uparrow \downarrow",
            "→ ← ↔ ⇒ ⇐ ⇔ ↦ ↑ ↓",
        ),
        (
            r"\ldots \cdots \vdots \ddots \angle \degree \prime \perp \parallel \mid",
            "… ⋯ ⋮ ⋱ ∠ ° ′ ⊥ ∥ ∣",
        ),
        (
            r"\langle x \rangle \lceil x \rceil \lfloor x \rfloor",
            "⟨ x ⟩ ⌈ x ⌉ ⌊ x ⌋",
        ),
        (r"\emptyset \varnothing \ell \hbar", "∅ ∅ ℓ ℏ"),
    ];
    for (source, expected) in cases {
        assert_eq!(render_inline_latex(source), expected, "{source}");
    }
}

#[test]
fn renders_all_supported_script_characters_and_falls_back_for_others() {
    assert_eq!(
        render_inline_latex("x^{0123456789+-=()ni}"),
        "x⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾ⁿⁱ"
    );
    assert_eq!(
        render_inline_latex("x_{0123456789+-=()aehijklmnoprstuvx}"),
        "x₀₁₂₃₄₅₆₇₈₉₊₋₌₍₎ₐₑₕᵢⱼₖₗₘₙₒₚᵣₛₜᵤᵥₓ"
    );
    assert_eq!(render_inline_latex("x^{ab}_{qy}"), "x_(qy)^(ab)");
    assert_eq!(render_inline_latex("x_1^2_3^4"), "x₁₃²⁴");
}

#[test]
fn formatting_commands_preserve_content_and_spacing() {
    for command in [
        "mathbf",
        "mathrm",
        "mathit",
        "mathsf",
        "mathtt",
        "mathcal",
        "mathbb",
        "boldsymbol",
        "displaystyle",
        "scriptstyle",
    ] {
        assert_eq!(
            render_inline_latex(&format!(r"\{command}{{x_2+y}}")),
            "x₂+y",
            "{command}"
        );
    }
    assert_eq!(
        render_inline_latex(r"a\,b\;c\:d\quad e\qquad f\!g"),
        "a b c d e  fg"
    );
    assert_eq!(
        render_inline_latex(r"\text{rate = 5 percent}"),
        "rate = 5 percent"
    );
    assert_eq!(
        render_inline_latex(r"\operatorname{arg max}_x f(x)"),
        "arg maxₓ f(x)"
    );
}

#[test]
fn accents_lines_and_delimiters_are_stable_unicode() {
    assert_eq!(
        render_inline_latex(r"\hat{x}\bar{y}\vec{v}\dot{x}\ddot{y}\tilde{z}"),
        "x̂ȳv⃗ẋÿz̃"
    );
    assert_eq!(render_inline_latex(r"\overline{ab}"), "a̅b̅");
    assert_eq!(render_inline_latex(r"\underline{ab}"), "a̲b̲");
    assert_eq!(render_inline_latex(r"\left( x \right)"), "( x )");
    assert_eq!(render_inline_latex(r"\left\{ x \right\}"), "{ x }");
    assert_eq!(render_inline_latex(r"\bigl\langle x \bigr\rangle"), "⟨ x ⟩");
    assert_eq!(render_inline_latex(r"\left. x \right|"), " x |");
}

#[test]
fn fraction_root_and_script_layouts_have_consistent_geometry() {
    let cases = [
        r"\frac{a}{b}",
        r"\frac{x+1}{y-z}",
        r"\frac{\frac{a}{b}}{\sqrt{x}}",
        r"x^{a+b}_{i-j}",
        r"\sqrt[3]{\frac{x}{y}}",
        r"A+\frac{界}{\alpha}+B",
    ];
    for source in cases {
        let lines = render_display_latex(source);
        assert!(!lines.is_empty(), "{source}");
        let width = lines.iter().map(|line| line.width()).max().unwrap();
        assert!(width > 0, "{source}: {lines:?}");
        assert!(
            lines.iter().all(|line| line.width() <= width),
            "{source}: {lines:?}"
        );
        assert!(
            lines.iter().all(|line| !line.contains('\t')),
            "{source}: {lines:?}"
        );
    }
}

#[test]
fn every_matrix_environment_has_the_expected_delimiter_family() {
    let cases = [
        ("matrix", "", ""),
        ("smallmatrix", "", ""),
        ("array", "", ""),
        ("pmatrix", "⎛", "⎞"),
        ("bmatrix", "⎡", "⎤"),
        ("Bmatrix", "⎧", "⎫"),
        ("vmatrix", "│", "│"),
        ("Vmatrix", "‖", "‖"),
        ("cases", "⎧", ""),
    ];
    for (environment, left, right) in cases {
        let source = if environment == "array" {
            r"\begin{array}{cc}a & bb \\ ccc & d\end{array}".to_string()
        } else {
            format!(r"\begin{{{environment}}}a & bb \\ ccc & d\end{{{environment}}}")
        };
        let lines = render_display_latex(&source);
        assert_eq!(lines.len(), 2, "{environment}: {lines:?}");
        if !left.is_empty() {
            assert!(lines[0].starts_with(left), "{environment}: {lines:?}");
        }
        if !right.is_empty() {
            assert!(lines[0].ends_with(right), "{environment}: {lines:?}");
        }
        assert_eq!(
            lines[0].width(),
            lines[1].width(),
            "{environment}: {lines:?}"
        );
    }
}

#[test]
fn matrices_handle_ragged_rows_nested_groups_and_row_spacing() {
    let ragged = render_display_latex(r"\begin{bmatrix}a & bb & ccc \\ d \\ e & ff\end{bmatrix}");
    assert_eq!(ragged.len(), 3);
    assert!(
        ragged.iter().all(|line| line.width() == ragged[0].width()),
        "{ragged:?}"
    );

    assert_eq!(
        render_inline_latex(r"\begin{matrix}{a & b} & \frac{c&d}{e} \\ x & y\end{matrix}"),
        "a & b, (c&d)⁄e; x, y"
    );
    assert_eq!(
        render_inline_latex(r"\begin{matrix}a\\[12pt]b\\[-2pt]c\\\end{matrix}"),
        "a; b; c"
    );
}

#[test]
fn ordinary_display_environments_render_their_body() {
    for environment in ["equation", "equation*", "displaymath"] {
        let source = format!(r"\begin{{{environment}}}\frac{{x}}{{y}}\end{{{environment}}}");
        let lines = render_display_latex(&source);
        assert!(
            lines.iter().any(|line| line.contains('─')),
            "{source}: {lines:?}"
        );
        assert!(
            !lines.iter().any(|line| line.contains("begin")),
            "{source}: {lines:?}"
        );
    }
}

#[test]
fn unknown_commands_and_environments_remain_debuggable() {
    let cases = [
        (r"\unknown", r"\unknown"),
        (r"\unknown value", r"\unknown value"),
        (r"\unknown{value}", r"\unknownvalue"),
        (
            r"\begin{mystery}x+y\end{mystery}",
            r"\begin{mystery}x+y\end{mystery}",
        ),
        (r"\end{orphan}", r"\endorphan"),
    ];
    for (source, expected) in cases {
        assert_eq!(render_inline_latex(source), expected, "{source}");
    }
}

#[test]
fn malformed_constructs_never_panic_and_keep_diagnostic_content() {
    let cases = [
        "{",
        "}",
        "[",
        "]",
        "^",
        "_",
        "\\",
        r"\frac",
        r"\frac{}",
        r"\frac{}{}",
        r"\sqrt",
        r"\sqrt[",
        r"\sqrt[]{}",
        r"\text",
        r"\left",
        r"\right",
        r"\begin",
        r"\begin{}",
        r"\begin{matrix}",
        r"\begin{matrix}a&b\end{pmatrix}",
        r"x_{",
        r"x^{}}",
        r"x^^__",
        "😀_{界",
        "\0\u{1b}[31m",
    ];
    for source in cases {
        assert_visible_and_deterministic(source);
    }
}

#[test]
fn generated_latex_grammar_corpus_is_total_and_deterministic() {
    let atoms = ["x", "7", "α", "界", "😀", r"\beta", r"\unknown"];
    let wrappers = [
        (r"{", "}"),
        (r"\frac{", "}{2}"),
        (r"\sqrt{", "}"),
        (r"\sqrt[3]{", "}"),
        (r"\mathbf{", "}"),
        (r"\left(", r"\right)"),
        (r"\begin{bmatrix}", r" & y \\ z & w\end{bmatrix}"),
    ];
    let scripts = ["", "_1", "^2", "_{i+j}", "^{n-1}", "_i^2"];
    for atom in atoms {
        for (open, close) in wrappers {
            for script in scripts {
                let source = format!("{open}{atom}{close}{script}");
                assert_visible_and_deterministic(&source);
            }
        }
    }
}

#[test]
fn long_flat_inputs_and_deep_commands_remain_bounded() {
    let flat = format!("{}{}", r"\alpha+".repeat(20_000), "x");
    let inline = render_inline_latex(&flat);
    assert!(inline.ends_with('x'));
    assert!(inline.len() < flat.len());

    let mut nested = "x".to_string();
    for _ in 0..2_000 {
        nested = format!(r"\sqrt{{{nested}}}");
    }
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || assert_visible_and_deterministic(&nested))
        .expect("spawn bounded stack test")
        .join()
        .expect("deep command rendering must not panic");
}

#[test]
fn normalizes_every_supported_math_fence_spelling() {
    for info in [
        "math",
        "MATH",
        "latex",
        "LaTeX",
        "tex",
        "TEX",
        "katex",
        "KaTeX",
        "{.math}",
        "{latex}",
        "math title=demo",
    ] {
        let source = format!("```{info}\n\\alpha_2\n```");
        assert_eq!(normalize_latex_math(&source), "$$\n\\alpha_2\n$$", "{info}");
    }
    assert_eq!(
        normalize_latex_math("  ~~~~latex\n\\[x^2\\]\n  ~~~~~"),
        "  $$\nx^2\n  $$"
    );
}

#[test]
fn normalization_protects_all_literal_code_forms() {
    let cases = [
        r"`\(x\)`",
        r"``code ` and \[x\]``",
        "```rust\n\\(x\\)\n```",
        "~~~text\n\\begin{matrix}x\\end{matrix}\n~~~",
        "    \\[indented code\\]",
        "\t\\(tab-indented code\\)",
    ];
    for source in cases {
        assert_eq!(normalize_latex_math(source), source, "{source:?}");
    }
}

#[test]
fn fence_like_content_does_not_end_a_generic_code_fence() {
    for source in [
        "```text\n```not a closing fence\n\\(literal\\)\n```",
        "~~~~text\n~~~~still content\n\\[literal\\]\n~~~~",
        "````rust\n``` shorter fence\n\\begin{matrix}x\\end{matrix}\n````",
    ] {
        assert_eq!(normalize_latex_math(source), source, "{source:?}");
    }

    assert_eq!(
        normalize_latex_math("```text\n\\(literal\\)\n```\n\n\\(real math\\)"),
        "```text\n\\(literal\\)\n```\n\n$real math$"
    );
}

#[test]
fn normalizes_delimiters_only_when_balanced_and_unescaped() {
    let cases = [
        (r"before \(x^2\) after", r"before $x^2$ after"),
        (r"before \[x^2\] after", r"before $$x^2$$ after"),
        (r"\\(literal\\)", r"\\(literal\\)"),
        (r"\(missing", r"\(missing"),
        (r"missing\)", r"missing\)"),
        (r"\[label\](url)", r"\[label\](url)"),
        (r"$x + \(y\)$", r"$x + \(y\)$"),
        (r"$$x + \[y\]$$", r"$$x + \[y\]$$"),
    ];
    for (source, expected) in cases {
        assert_eq!(normalize_latex_math(source), expected, "{source}");
    }
}

#[test]
fn normalizes_all_standalone_display_environments() {
    let environments = [
        "equation",
        "equation*",
        "displaymath",
        "align",
        "align*",
        "aligned",
        "aligned*",
        "gather",
        "gather*",
        "gathered",
        "multline",
        "multline*",
        "split",
        "eqnarray",
        "eqnarray*",
        "matrix",
        "smallmatrix",
        "array",
        "pmatrix",
        "bmatrix",
        "Bmatrix",
        "vmatrix",
        "Vmatrix",
        "cases",
        "cases*",
    ];
    for environment in environments {
        let source = format!(r"prefix \begin{{{environment}}}x\end{{{environment}}} suffix");
        let normalized = normalize_latex_math(&source);
        assert!(normalized.contains("$$\n"), "{environment}: {normalized:?}");
        assert!(normalized.contains("\n$$"), "{environment}: {normalized:?}");
        assert!(normalized.contains(&format!(r"\begin{{{environment}}}x\end{{{environment}}}")));
    }
}

#[test]
fn environment_normalization_handles_nesting_and_mismatches() {
    let nested = r"\begin{align}a\begin{align}b\end{align}c\end{align}";
    let normalized = normalize_latex_math(nested);
    assert_eq!(normalized.matches("$$").count(), 2, "{normalized}");
    assert!(normalized.contains(nested));

    for source in [
        r"\begin{align}x",
        r"\begin{align}x\end{equation}",
        r"\begin{unknown}x\end{unknown}",
    ] {
        assert_eq!(normalize_latex_math(source), source, "{source}");
    }
}

#[test]
fn markdown_pipeline_preserves_structure_and_math_roles() {
    let markdown = concat!(
        "Price: $35.00 and inline \\(x_2 + \\alpha\\).\n\n",
        "\\[\\frac{x+1}{y}\\]\n\n",
        "```rust\nlet literal = r\"\\(x\\)\";\n```\n"
    );
    let doc = parse_markdown(markdown);
    assert!(
        doc.blocks
            .iter()
            .any(|block| block.kind == BlockKind::Paragraph)
    );
    let display = doc
        .blocks
        .iter()
        .find(|block| block.kind == BlockKind::MathDisplay)
        .expect("display math block");
    assert!(
        display
            .lines
            .iter()
            .all(|line| line.spans.iter().all(|span| span.role == StyleRole::Math))
    );
    assert!(
        display
            .lines
            .iter()
            .any(|line| line.plain_text().contains('─'))
    );

    let all_text = doc
        .blocks
        .iter()
        .flat_map(|block| &block.lines)
        .map(|line| line.plain_text())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all_text.contains("$35.00"), "{all_text}");
    assert!(all_text.contains("x₂ + α"), "{all_text}");
    assert!(all_text.contains(r"\(x\)"), "{all_text}");
}

#[test]
fn markdown_container_matrix_has_backend_neutral_parity() {
    let containers = [
        r"Inline \(\alpha_2 + x^2\).",
        r"\[\frac{x+1}{y}\]",
        "```math\n\\frac{x+1}{y}\n```",
        "~~~latex\n\\begin{bmatrix}a & b \\\\ c & d\\end{bmatrix}\n~~~",
        r"\begin{align*}x &= 1 \\ y &= 2\end{align*}",
    ];
    for markdown in containers {
        let first = parse_markdown(markdown);
        let second = parse_markdown(markdown);
        assert_eq!(first, second, "{markdown}");
        assert!(
            first
                .blocks
                .iter()
                .flat_map(|block| &block.lines)
                .flat_map(|line| &line.spans)
                .any(|span| span.role == StyleRole::Math),
            "{markdown}: {first:?}"
        );
    }
}
