//! Export production TUI replay frames without a terminal or provider connection.
use anyhow::{Context, Result, ensure};
use clap::Parser;
use jcode_tui::tui::App;
use ratatui::style::{Color, Modifier};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

#[derive(Parser)]
struct Args {
    /// Saved real session ID or JSON path, used for original model metadata.
    #[arg(long)]
    session: String,
    /// Reviewed timeline containing original session content and edited timing.
    #[arg(long)]
    timeline: PathBuf,
    /// New private output directory. Existing directories are rejected.
    #[arg(long)]
    output: PathBuf,
    #[arg(long, default_value_t = 120)]
    cols: u16,
    #[arg(long, default_value_t = 30)]
    rows: u16,
    #[arg(long, default_value_t = 24)]
    fps: u32,
    #[arg(long)]
    duration_ms: Option<u64>,
    /// Export row-major cell data as well as the lossless ANSI representation.
    #[arg(long)]
    cells: bool,
}

fn color(value: Color) -> Value {
    let index = match value {
        Color::Reset => return Value::Null,
        Color::Rgb(r, g, b) => return json!({"rgb": [r, g, b]}),
        Color::Indexed(index) => index,
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
    };
    json!({"index": index})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_colors_preserve_reset_rgb_and_all_palette_entries() {
        assert_eq!(color(Color::Reset), Value::Null);
        assert_eq!(color(Color::Rgb(10, 20, 30)), json!({"rgb":[10,20,30]}));
        for index in 0..=255 {
            assert_eq!(color(Color::Indexed(index)), json!({"index":index}));
        }
        assert_eq!(color(Color::Black), json!({"index":0}));
        assert_eq!(color(Color::White), json!({"index":15}));
        assert_eq!(color(Color::LightBlue), json!({"index":12}));
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    ensure!((1..=60).contains(&args.fps), "fps must be between 1 and 60");
    ensure!(
        (1..=300).contains(&args.cols) && (1..=100).contains(&args.rows),
        "dimensions must be 1..300 columns and 1..100 rows"
    );
    let timeline: Vec<jcode_tui::replay::TimelineEvent> =
        serde_json::from_slice(&fs::read(&args.timeline)?)?;
    let session = jcode_tui::replay::load_session(&args.session)?;
    let duration = args.duration_ms.unwrap_or_else(|| {
        jcode_tui::replay::timeline_to_replay_events(&timeline)
            .iter()
            .map(|(delay, _)| *delay)
            .sum::<u64>()
            .saturating_add(1500)
    });
    ensure!(
        (1..=300_000).contains(&duration),
        "duration must be 1ms to 5min"
    );
    let count = duration.saturating_mul(u64::from(args.fps)).div_ceil(1000);
    ensure!(
        count
            .saturating_mul(u64::from(args.cols))
            .saturating_mul(u64::from(args.rows))
            <= 20_000_000,
        "export exceeds the 20-million-cell memory budget"
    );
    let positions: Vec<u64> = (0..count).map(|i| i * 1000 / u64::from(args.fps)).collect();
    // Reject an existing destination rather than overwrite prior footage.
    fs::create_dir(&args.output).context("create new output directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&args.output, fs::Permissions::from_mode(0o700))?;
    }
    let app = App::new_for_replay(session);
    let frames = app.render_replay_states(&timeline, &positions, args.cols, args.rows, &[])?;
    let mut manifest_frames = Vec::with_capacity(frames.len());
    let mut previous = None;
    let mut previous_paths = (String::new(), Value::Null);
    for (index, (time, frame)) in frames.into_iter().enumerate() {
        if previous.as_ref() != Some(&frame) {
            let ansi = format!("frame-{index:05}.ansi");
            fs::write(args.output.join(&ansi), App::replay_frame_ansi(&frame)?)?;
            let cells_path = if args.cells {
                let name = format!("frame-{index:05}.cells.json");
                let cells: Vec<Value> = frame
                    .content
                    .iter()
                    .map(|cell| {
                        json!({
                            "s": cell.symbol(), "fg": color(cell.fg), "bg": color(cell.bg),
                            "bold": cell.modifier.contains(Modifier::BOLD),
                            "italic": cell.modifier.contains(Modifier::ITALIC),
                            "dim": cell.modifier.contains(Modifier::DIM),
                            "reverse": cell.modifier.contains(Modifier::REVERSED),
                            "underline": cell.modifier.contains(Modifier::UNDERLINED),
                            "strike": cell.modifier.contains(Modifier::CROSSED_OUT),
                            "skip": cell.skip
                        })
                    })
                    .collect();
                fs::write(args.output.join(&name), serde_json::to_vec(&cells)?)?;
                json!(name)
            } else {
                Value::Null
            };
            previous_paths = (ansi, cells_path);
            previous = Some(frame);
        }
        manifest_frames.push(json!({"t_ms":time,"ansi":previous_paths.0,"cells":previous_paths.1}));
    }
    let manifest = json!({
        "version":1,"source":"jcode production TUI renderer",
        "cols":args.cols,"rows":args.rows,"fps":args.fps,
        "duration_ms":duration,"frames":manifest_frames
    });
    fs::write(
        args.output.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    eprintln!(
        "Exported {count} production frames to {}",
        args.output.display()
    );
    Ok(())
}
