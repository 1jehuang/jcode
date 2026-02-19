use anyhow::{Context, Result};
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::replay::TimelineEvent;

fn find_command(name: &str) -> Option<PathBuf> {
    std::process::Command::new("which")
        .arg(name)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim().to_string()))
        .or_else(|| {
            let cargo_bin = dirs::home_dir()?.join(".cargo/bin").join(name);
            cargo_bin.exists().then_some(cargo_bin)
        })
}

pub async fn export_video(
    session: &crate::session::Session,
    timeline: &[TimelineEvent],
    speed: f64,
    output_path: &Path,
    width: u16,
    height: u16,
    fps: u32,
) -> Result<()> {
    let asciicast_path = output_path.with_extension("cast");

    let app = crate::tui::App::new_for_replay(session.clone()).await;

    eprintln!(
        "  Rendering at {}x{}, {}fps, {:.1}x speed...",
        width, height, fps, speed
    );

    let cast_events = app
        .run_headless_replay(timeline, speed, width, height, fps)
        .await?;

    write_asciicast(&asciicast_path, width, height, &cast_events)?;
    eprintln!(
        "  Wrote asciicast ({} frames): {}",
        cast_events.len(),
        asciicast_path.display()
    );

    let ext = output_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp4");

    if ext == "cast" {
        return Ok(());
    }

    if ext == "gif" {
        convert_asciicast_to_gif(&asciicast_path, output_path, 14).await?;
        let _ = std::fs::remove_file(&asciicast_path);
        return Ok(());
    }

    let gif_path = output_path.with_extension("gif");
    convert_asciicast_to_gif(&asciicast_path, &gif_path, 14).await?;
    convert_gif_to_video(&gif_path, output_path).await?;
    let _ = std::fs::remove_file(&asciicast_path);
    let _ = std::fs::remove_file(&gif_path);

    Ok(())
}

fn write_asciicast(
    path: &Path,
    width: u16,
    height: u16,
    events: &[(f64, String)],
) -> Result<()> {
    let mut file = std::fs::File::create(path)
        .with_context(|| format!("Failed to create {}", path.display()))?;

    let header = serde_json::json!({
        "version": 2,
        "width": width,
        "height": height,
        "timestamp": chrono::Utc::now().timestamp(),
        "env": {
            "SHELL": "/bin/bash",
            "TERM": "xterm-256color"
        },
        "title": "jcode replay"
    });
    writeln!(file, "{}", serde_json::to_string(&header)?)?;

    let mut prev_output = String::new();
    for (time, output) in events {
        if *output == prev_output {
            continue;
        }

        let reset_and_home = "\x1b[H\x1b[2J";
        let full_output = format!("{}{}", reset_and_home, output);

        let event = serde_json::json!([time, "o", full_output]);
        writeln!(file, "{}", serde_json::to_string(&event)?)?;

        prev_output = output.clone();
    }

    Ok(())
}

pub fn buffer_to_ansi(buf: &Buffer) -> String {
    let mut output = String::new();
    let width = buf.area.width;
    let height = buf.area.height;

    for y in 0..height {
        if y > 0 {
            output.push_str("\r\n");
        }

        let mut prev_fg: Option<Color> = None;
        let mut prev_bg: Option<Color> = None;
        let mut prev_bold = false;
        let mut prev_dim = false;

        for x in 0..width {
            let cell = &buf[(x, y)];
            let fg = cell.fg;
            let bg = cell.bg;
            let bold = cell.modifier.contains(ratatui::style::Modifier::BOLD);
            let dim = cell.modifier.contains(ratatui::style::Modifier::DIM);

            let need_style =
                prev_fg != Some(fg) || prev_bg != Some(bg) || prev_bold != bold || prev_dim != dim;

            if need_style {
                output.push_str("\x1b[0m");

                if bold {
                    output.push_str("\x1b[1m");
                }
                if dim {
                    output.push_str("\x1b[2m");
                }

                push_fg_ansi(&mut output, fg);
                push_bg_ansi(&mut output, bg);

                prev_fg = Some(fg);
                prev_bg = Some(bg);
                prev_bold = bold;
                prev_dim = dim;
            }

            output.push_str(cell.symbol());
        }
    }

    output.push_str("\x1b[0m");
    output
}

fn push_fg_ansi(out: &mut String, color: Color) {
    match color {
        Color::Reset => {}
        Color::Black => out.push_str("\x1b[30m"),
        Color::Red => out.push_str("\x1b[31m"),
        Color::Green => out.push_str("\x1b[32m"),
        Color::Yellow => out.push_str("\x1b[33m"),
        Color::Blue => out.push_str("\x1b[34m"),
        Color::Magenta => out.push_str("\x1b[35m"),
        Color::Cyan => out.push_str("\x1b[36m"),
        Color::Gray => out.push_str("\x1b[37m"),
        Color::DarkGray => out.push_str("\x1b[90m"),
        Color::LightRed => out.push_str("\x1b[91m"),
        Color::LightGreen => out.push_str("\x1b[92m"),
        Color::LightYellow => out.push_str("\x1b[93m"),
        Color::LightBlue => out.push_str("\x1b[94m"),
        Color::LightMagenta => out.push_str("\x1b[95m"),
        Color::LightCyan => out.push_str("\x1b[96m"),
        Color::White => out.push_str("\x1b[97m"),
        Color::Rgb(r, g, b) => {
            out.push_str(&format!("\x1b[38;2;{};{};{}m", r, g, b));
        }
        Color::Indexed(i) => {
            out.push_str(&format!("\x1b[38;5;{}m", i));
        }
    }
}

fn push_bg_ansi(out: &mut String, color: Color) {
    match color {
        Color::Reset => {}
        Color::Black => out.push_str("\x1b[40m"),
        Color::Red => out.push_str("\x1b[41m"),
        Color::Green => out.push_str("\x1b[42m"),
        Color::Yellow => out.push_str("\x1b[43m"),
        Color::Blue => out.push_str("\x1b[44m"),
        Color::Magenta => out.push_str("\x1b[45m"),
        Color::Cyan => out.push_str("\x1b[46m"),
        Color::Gray => out.push_str("\x1b[47m"),
        Color::DarkGray => out.push_str("\x1b[100m"),
        Color::LightRed => out.push_str("\x1b[101m"),
        Color::LightGreen => out.push_str("\x1b[102m"),
        Color::LightYellow => out.push_str("\x1b[103m"),
        Color::LightBlue => out.push_str("\x1b[104m"),
        Color::LightMagenta => out.push_str("\x1b[105m"),
        Color::LightCyan => out.push_str("\x1b[106m"),
        Color::White => out.push_str("\x1b[107m"),
        Color::Rgb(r, g, b) => {
            out.push_str(&format!("\x1b[48;2;{};{};{}m", r, g, b));
        }
        Color::Indexed(i) => {
            out.push_str(&format!("\x1b[48;5;{}m", i));
        }
    }
}

async fn convert_asciicast_to_gif(
    asciicast_path: &Path,
    gif_path: &Path,
    font_size: u32,
) -> Result<()> {
    let agg_path = find_command("agg").context(
        "agg not found. Install with: cargo install --git https://github.com/asciinema/agg",
    )?;

    eprintln!("  Converting asciicast → GIF...");
    let status = tokio::process::Command::new(&agg_path)
        .arg(asciicast_path)
        .arg(gif_path)
        .arg("--font-size")
        .arg(font_size.to_string())
        .arg("--speed")
        .arg("1")
        .status()
        .await
        .context("Failed to run agg")?;

    if !status.success() {
        anyhow::bail!("agg exited with status: {}", status);
    }
    eprintln!("  Output: {}", gif_path.display());
    Ok(())
}

async fn convert_gif_to_video(gif_path: &Path, output_path: &Path) -> Result<()> {
    let ffmpeg_path =
        find_command("ffmpeg").context("ffmpeg not found")?;

    let ext = output_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp4");

    eprintln!("  Converting GIF → {}...", ext.to_uppercase());
    let status = tokio::process::Command::new(&ffmpeg_path)
        .arg("-y")
        .arg("-i")
        .arg(gif_path)
        .arg("-movflags")
        .arg("faststart")
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-vf")
        .arg("scale=trunc(iw/2)*2:trunc(ih/2)*2")
        .arg(output_path)
        .status()
        .await
        .context("Failed to run ffmpeg")?;

    if !status.success() {
        anyhow::bail!("ffmpeg exited with status: {}", status);
    }
    eprintln!("  Output: {}", output_path.display());
    Ok(())
}
