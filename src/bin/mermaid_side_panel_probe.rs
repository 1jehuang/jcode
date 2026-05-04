use anyhow::{Context, Result, anyhow};
use std::env;

fn usage() -> &'static str {
    "usage: cargo run --features dev-bins --bin mermaid_side_panel_probe -- <mermaid-file> [--pane-width N] [--pane-height N] [--font-width N] [--font-height N] [--left]"
}

fn parse_u16_arg(args: &mut std::vec::IntoIter<String>, flag: &str) -> Result<u16> {
    let value = args
        .next()
        .ok_or_else(|| anyhow!("missing value for {flag}"))?;
    value
        .parse::<u16>()
        .with_context(|| format!("invalid integer for {flag}: {value}"))
}

fn main() -> Result<()> {
    let mut args = env::args().skip(1).collect::<Vec<_>>().into_iter();
    let mut path: Option<String> = None;
    let mut pane_width: u16 = 36;
    let mut pane_height: u16 = 30;
    let mut font_width: u16 = 8;
    let mut font_height: u16 = 16;
    let mut centered = true;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--pane-width" => pane_width = parse_u16_arg(&mut args, "--pane-width")?,
            "--pane-height" => pane_height = parse_u16_arg(&mut args, "--pane-height")?,
            "--font-width" => font_width = parse_u16_arg(&mut args, "--font-width")?,
            "--font-height" => font_height = parse_u16_arg(&mut args, "--font-height")?,
            "--left" => centered = false,
            "--help" | "-h" => {
                println!("{}", usage());
                return Ok(());
            }
            value if value.starts_with('-') => {
                return Err(anyhow!("unknown flag: {value}\n{}", usage()));
            }
            value => {
                if path.is_some() {
                    return Err(anyhow!(
                        "only one mermaid file path is supported\n{}",
                        usage()
                    ));
                }
                path = Some(value.to_string());
            }
        }
    }

    let path = path.ok_or_else(|| anyhow!("missing mermaid file path\n{}", usage()))?;
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read mermaid file: {path}"))?;
    let probe = jcode::tui::debug_probe_side_panel_mermaid(
        &content,
        pane_width,
        pane_height,
        Some((font_width, font_height)),
        centered,
    )?;
    println!("{}", serde_json::to_string_pretty(&probe)?);
    Ok(())
}
