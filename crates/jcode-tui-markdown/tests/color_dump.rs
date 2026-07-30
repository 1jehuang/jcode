//! Dev-only visual harness: dump rendered spans with fg colors as JSON so a
//! script can paint an approximation of the TUI output. Run with:
//! `cargo test -p jcode-tui-markdown --test color_dump -- --nocapture --ignored`
use ratatui::style::Color;

#[test]
#[ignore = "dev visual harness, run explicitly with --ignored"]
fn dump_navier_stokes_render() {
    let text = "The Navier-Stokes equations describe viscous fluid motion.\n\nFor an incompressible fluid:\n\n$$\\rho \\left( \\frac{\\partial \\mathbf{u}}{\\partial t} + \\mathbf{u} \\cdot \\nabla \\mathbf{u} \\right) = -\\nabla p + \\mu \\nabla^2 \\mathbf{u} + \\mathbf{f}$$\n\nwhere $\\mathbf{u}$ is velocity, $p$ pressure, $\\rho$ density, $\\mu$ viscosity, $\\mathbf{f}$ external forces (like gravity).\n\nTerm by term: the left side is fluid acceleration (including the nonlinear convection term $\\mathbf{u} \\cdot \\nabla \\mathbf{u}$, which makes turbulence possible).";
    let lines = jcode_tui_markdown::render_markdown_with_width(text, Some(110));
    let mut out = Vec::new();
    for line in &lines {
        let spans: Vec<serde_json::Value> = line
            .spans
            .iter()
            .map(|s| {
                let fg = match s.style.fg {
                    Some(Color::Rgb(r, g, b)) => format!("{r},{g},{b}"),
                    Some(other) => format!("{other:?}"),
                    None => "default".to_string(),
                };
                serde_json::json!({"t": s.content, "fg": fg})
            })
            .collect();
        out.push(spans);
    }
    println!(
        "COLOR_DUMP_BEGIN{}COLOR_DUMP_END",
        serde_json::to_string(&out).unwrap()
    );
}
