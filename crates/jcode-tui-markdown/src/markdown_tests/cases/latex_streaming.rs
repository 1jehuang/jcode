fn exact_multiline_latex_response() -> &'static str {
    concat!(
        "\\[\n\\boxed{\ne^{i\\pi}+1=0\n}\n\\]\n\n",
        "\\[\n\\int_{-\\infty}^{\\infty} e^{-x^2}\\,dx=\\sqrt{\\pi}\n\\]\n\n",
        "\\[\nx=\\frac{-b\\pm\\sqrt{b^2-4ac}}{2a}\n\\]\n\n",
        "\\[\n\\nabla\\cdot\\mathbf{E}=\\frac{\\rho}{\\varepsilon_0}\n\\]\n\n",
        "\\[\n\\frac{\\partial \\psi}{\\partial t}\n=\n",
        "\\alpha\\frac{\\partial^2\\psi}{\\partial x^2}\n\\]",
    )
}

#[test]
fn latex_foreground_is_white_and_styles_inline_math() {
    assert_eq!(MATH_FOREGROUND, (255, 255, 255));
    assert_eq!(MATH_INLINE_FOREGROUND, (255, 255, 255));

    let lines = with_streaming_render_context(|| render_markdown("Inline $x^2$ math."));
    let math_spans: Vec<_> = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .filter(|span| span.content.contains("x²"))
        .collect();
    assert_eq!(math_spans.len(), 1);
    assert_eq!(math_spans[0].style.fg, Some(crate::math_inline_fg()));
}

#[test]
fn exact_multiline_response_renders_all_five_equations() {
    let mut renderer = IncrementalMarkdownRenderer::new(Some(90));
    let rendered = lines_to_string(&renderer.update(exact_multiline_latex_response()));

    assert_eq!(rendered.matches("┌─ math").count(), 5, "{rendered}");
    assert!(!rendered.contains("$$"), "{rendered}");
    assert!(!rendered.contains(r"\partial"), "{rendered}");
    assert!(rendered.contains('∂'), "{rendered}");
    assert!(rendered.contains('α'), "{rendered}");
}

#[test]
fn every_streaming_prefix_converges_to_the_full_math_render() {
    let response = exact_multiline_latex_response();
    let mut renderer = IncrementalMarkdownRenderer::new(Some(90));

    for end in response
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(response.len()))
    {
        let _ = renderer.update(&response[..end]);
    }

    let incremental = renderer.update(response);
    let full = with_streaming_render_context(|| render_markdown_with_width(response, Some(90)));
    assert_eq!(incremental, full);
    assert_eq!(lines_to_string(&incremental).matches("┌─ math").count(), 5);
}

/// Regression for #735: no markdown render, streaming or completed, may run
/// the TeX toolchain on the calling (draw) thread. A blocking toolchain run
/// there starves the TUI event loop and makes Esc/interrupt look ignored.
#[test]
fn math_rendering_never_runs_the_toolchain_on_the_render_thread() {
    let before = latex_image::test_toolchain_runs();
    let mut renderer = IncrementalMarkdownRenderer::new(Some(90));
    let _ = renderer.update(exact_multiline_latex_response());
    let _ = render_markdown(r"$$x^2$$");
    assert_eq!(
        latex_image::test_toolchain_runs(),
        before,
        "markdown rendering must defer LaTeX toolchain work to the background worker"
    );
}

#[test]
fn inline_math_stays_inline_in_image_mode_and_skips_the_image_toolchain() {
    latex_image::reset_test_render_attempts();
    // Non-streaming render with Image mode active: inline math is part of a
    // sentence and must never become a block-level image panel.
    let lines =
        render_markdown_with_width("where $\\mathbf{u}$ is velocity, $p$ pressure.", Some(90));
    assert_eq!(
        latex_image::test_render_attempts(),
        0,
        "inline math must not invoke the image toolchain"
    );
    let rendered = lines_to_string(&lines);
    let sentence_lines: Vec<_> = rendered
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(sentence_lines.len(), 1, "{rendered}");
    assert!(sentence_lines[0].contains("where"), "{rendered}");
    assert!(sentence_lines[0].contains("is velocity"), "{rendered}");
    assert!(sentence_lines[0].contains("pressure"), "{rendered}");
    assert!(!rendered.contains("math"), "{rendered}");
}

#[test]
fn display_math_nested_in_a_list_uses_graphical_rendering() {
    let source = concat!(
        "- Gaussian integral:\n\n",
        "  \\[\n",
        "  \\int_{-\\infty}^{\\infty} e^{-x^2}\\,dx=\\sqrt{\\pi}\n",
        "  \\]\n",
    );

    latex_image::reset_test_render_attempts();
    let lines = render_markdown_with_width(source, Some(90));
    assert!(
        latex_image::test_render_attempts() > 0,
        "list-nested display math should enter the graphical LaTeX pipeline: {}",
        lines_to_string(&lines)
    );
}

#[test]
fn multiline_relations_survive_blockquotes_and_promoted_delimiters() {
    let source = concat!(
        "> Blockquote display:\n> \\[\n> x^2\n> =\n> y^2\n> \\]\n\n",
        "Standalone spelling:\n\\(\nx\n=\ny\n\\)",
    );
    let rendered = with_streaming_render_context(|| {
        lines_to_string(&render_markdown_with_width(source, Some(90)))
    });

    assert_eq!(rendered.matches("┌─ math").count(), 2, "{rendered}");
    assert!(rendered.contains("x² = y²"), "{rendered}");
    assert!(rendered.contains("x = y"), "{rendered}");
    assert!(!rendered.contains("{}="), "{rendered}");
    assert!(!rendered.contains("$$"), "{rendered}");
}
