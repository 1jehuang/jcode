//! Local TALA -> production compiler/rasterizer/PNG-cache probe, without a model.
//!
//! `JCODE_TEST_D2=/opt/homebrew/bin/d2 cargo run -p jcode-tui-mermaid --example d2_tala -- --png-only`
//! The printed PNG can be inspected directly or displayed by the production TUI.
//! This probe does not depend on any terminal transport implementation.
use jcode_tui_mermaid::{D2Layout, D2Options, RenderResult, render_d2_sized_with_options};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = D2Options {
        binary: std::env::var_os("JCODE_TEST_D2")
            .map(Into::into)
            .unwrap_or_else(|| "d2".into()),
        layout: D2Layout::Tala,
        ..Default::default()
    };
    let source = "direction: right\nclient: Terminal\ncompiler: Local D2\nraster: PNG cache\nclient -> compiler: D2\ncompiler -> raster: SVG";
    match render_d2_sized_with_options(source, Some(100), options, false) {
        RenderResult::Image {
            hash,
            path,
            width,
            height,
        } => {
            println!(
                "TALA {width}x{height} hash={hash:016x} PNG={}",
                path.display()
            );
            Ok(())
        }
        RenderResult::Error(error) => Err(error.into()),
    }
}
