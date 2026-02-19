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

fn get_terminal_font() -> (String, f64) {
    if let Ok(conf) = std::fs::read_to_string(
        dirs::home_dir()
            .unwrap_or_default()
            .join(".config/kitty/kitty.conf"),
    ) {
        let mut family = String::new();
        let mut size: f64 = 11.0;
        for line in conf.lines() {
            let line = line.trim();
            if line.starts_with("font_family ") {
                family = line.strip_prefix("font_family ").unwrap_or("").trim().to_string();
            }
            if line.starts_with("font_size ") {
                if let Ok(s) = line.strip_prefix("font_size ").unwrap_or("").trim().parse() {
                    size = s;
                }
            }
        }
        if !family.is_empty() {
            return (family, size);
        }
    }
    ("JetBrains Mono".to_string(), 11.0)
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
    let app = crate::tui::App::new_for_replay(session.clone()).await;

    let (font_family, font_size) = get_terminal_font();
    eprintln!(
        "  Rendering at {}x{}, {}fps, {:.1}x speed (font: {} {}pt)...",
        width, height, fps, speed, font_family, font_size
    );

    let frames = app
        .run_headless_replay(timeline, speed, width, height, fps)
        .await?;

    let ext = output_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp4");

    if ext == "cast" {
        write_asciicast(
            output_path,
            width,
            height,
            &frames
                .iter()
                .map(|(t, buf)| (*t, buffer_to_ansi(buf)))
                .collect::<Vec<_>>(),
        )?;
        eprintln!("  Output: {}", output_path.display());
        return Ok(());
    }

    // SVG → PNG → ffmpeg pipeline (high quality, uses local font)
    let rsvg = find_command("rsvg-convert");
    let ffmpeg = find_command("ffmpeg");

    if ext == "gif" {
        // For GIF, use agg if available (it handles GIF optimization well)
        if let Some(_agg) = find_command("agg") {
            let cast_path = output_path.with_extension("cast");
            let ansi_frames: Vec<(f64, String)> = frames
                .iter()
                .map(|(t, buf)| (*t, buffer_to_ansi(buf)))
                .collect();
            write_asciicast(&cast_path, width, height, &ansi_frames)?;
            convert_asciicast_to_gif(&cast_path, output_path, &font_family, font_size).await?;
            let _ = std::fs::remove_file(&cast_path);
            return Ok(());
        }
    }

    // Primary pipeline: SVG frames → PNG → ffmpeg
    if rsvg.is_some() && ffmpeg.is_some() {
        let cell_w = (font_size * 0.6).ceil() as u32;
        let cell_h = (font_size * 1.4).ceil() as u32;

        render_svg_pipeline(
            &frames,
            output_path,
            width,
            height,
            fps,
            &font_family,
            font_size,
            cell_w,
            cell_h,
        )
        .await?;
        return Ok(());
    }

    // Fallback: asciicast → agg → ffmpeg
    if let Some(_agg) = find_command("agg") {
        let cast_path = output_path.with_extension("cast");
        let ansi_frames: Vec<(f64, String)> = frames
            .iter()
            .map(|(t, buf)| (*t, buffer_to_ansi(buf)))
            .collect();
        write_asciicast(&cast_path, width, height, &ansi_frames)?;
        let gif_path = output_path.with_extension("gif");
        convert_asciicast_to_gif(&cast_path, &gif_path, &font_family, font_size).await?;
        convert_gif_to_video(&gif_path, output_path).await?;
        let _ = std::fs::remove_file(&cast_path);
        let _ = std::fs::remove_file(&gif_path);
        return Ok(());
    }

    anyhow::bail!(
        "No rendering tools found. Install one of:\n  \
         - rsvg-convert + ffmpeg (best quality)\n  \
         - agg + ffmpeg (cargo install --git https://github.com/asciinema/agg)"
    );
}

async fn render_svg_pipeline(
    frames: &[(f64, Buffer)],
    output_path: &Path,
    width: u16,
    height: u16,
    fps: u32,
    font_family: &str,
    font_size: f64,
    cell_w: u32,
    cell_h: u32,
) -> Result<()> {
    let rsvg = find_command("rsvg-convert").context("rsvg-convert not found")?;
    let ffmpeg = find_command("ffmpeg").context("ffmpeg not found")?;

    let img_w = cell_w * width as u32;
    let img_h = cell_h * height as u32;

    let tmp_dir = PathBuf::from(format!("/tmp/jcode_video_{}", std::process::id()));
    if tmp_dir.exists() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
    std::fs::create_dir_all(&tmp_dir)?;

    // Deduplicate frames: only render unique buffers
    let mut unique_frames: Vec<(usize, &Buffer)> = Vec::new();
    let mut frame_indices: Vec<usize> = Vec::new(); // maps frame# → unique index

    for (_t, buf) in frames {
        let found = unique_frames
            .iter()
            .position(|(_, existing)| buffers_equal(existing, buf));
        match found {
            Some(idx) => frame_indices.push(idx),
            None => {
                let idx = unique_frames.len();
                unique_frames.push((idx, buf));
                frame_indices.push(idx);
            }
        }
    }

    eprintln!(
        "  Rendering {} unique frames as SVG → PNG ({} total)...",
        unique_frames.len(),
        frames.len()
    );

    // Render unique SVGs and convert to PNG
    let png_dir = tmp_dir.join("png");
    std::fs::create_dir_all(&png_dir)?;

    for (i, (_, buf)) in unique_frames.iter().enumerate() {
        let svg = buffer_to_svg(buf, font_family, font_size, cell_w, cell_h);
        let svg_path = tmp_dir.join(format!("frame_{:06}.svg", i));
        std::fs::write(&svg_path, &svg)?;

        let png_path = png_dir.join(format!("unique_{:06}.png", i));
        let status = tokio::process::Command::new(&rsvg)
            .arg("--width")
            .arg(img_w.to_string())
            .arg("--height")
            .arg(img_h.to_string())
            .arg("--output")
            .arg(&png_path)
            .arg(&svg_path)
            .status()
            .await
            .context("Failed to run rsvg-convert")?;

        if !status.success() {
            anyhow::bail!("rsvg-convert failed on frame {}", i);
        }

        let _ = std::fs::remove_file(&svg_path);

        if (i + 1) % 20 == 0 || i + 1 == unique_frames.len() {
            eprint!(
                "\r  Rendering SVG... {}/{}",
                i + 1,
                unique_frames.len()
            );
        }
    }
    eprintln!();

    // Create symlinks for the full frame sequence (ffmpeg needs sequential numbering)
    let seq_dir = tmp_dir.join("seq");
    std::fs::create_dir_all(&seq_dir)?;

    for (frame_num, &unique_idx) in frame_indices.iter().enumerate() {
        let src = png_dir.join(format!("unique_{:06}.png", unique_idx));
        let dst = seq_dir.join(format!("frame_{:06}.png", frame_num));
        #[cfg(unix)]
        std::os::unix::fs::symlink(&src, &dst)?;
        #[cfg(not(unix))]
        std::fs::copy(&src, &dst)?;
    }

    eprintln!("  Encoding video with ffmpeg...");
    let status = tokio::process::Command::new(&ffmpeg)
        .arg("-y")
        .arg("-framerate")
        .arg(fps.to_string())
        .arg("-i")
        .arg(seq_dir.join("frame_%06d.png"))
        .arg("-c:v")
        .arg("libx264")
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-crf")
        .arg("18")
        .arg("-preset")
        .arg("medium")
        .arg("-movflags")
        .arg("faststart")
        .arg("-vf")
        .arg("scale=trunc(iw/2)*2:trunc(ih/2)*2")
        .arg(output_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .context("Failed to run ffmpeg")?;

    if !status.success() {
        anyhow::bail!("ffmpeg encoding failed");
    }

    eprintln!("  Output: {}", output_path.display());
    if output_path.exists() {
        let size = std::fs::metadata(output_path)?.len();
        eprintln!("  Size: {:.1} MB", size as f64 / 1_048_576.0);
    }
    let _ = std::fs::remove_dir_all(&tmp_dir);
    Ok(())
}

fn buffers_equal(a: &Buffer, b: &Buffer) -> bool {
    if a.area != b.area {
        return false;
    }
    for y in 0..a.area.height {
        for x in 0..a.area.width {
            let ca = &a[(x, y)];
            let cb = &b[(x, y)];
            if ca.symbol() != cb.symbol() || ca.fg != cb.fg || ca.bg != cb.bg || ca.modifier != cb.modifier {
                return false;
            }
        }
    }
    true
}

fn color_to_hex(color: Color) -> String {
    match color {
        Color::Reset => "#d4d4d4".to_string(),
        Color::Black => "#000000".to_string(),
        Color::Red => "#cd3131".to_string(),
        Color::Green => "#0dbc79".to_string(),
        Color::Yellow => "#e5e510".to_string(),
        Color::Blue => "#2472c8".to_string(),
        Color::Magenta => "#bc3fbc".to_string(),
        Color::Cyan => "#11a8cd".to_string(),
        Color::Gray => "#808080".to_string(),
        Color::DarkGray => "#666666".to_string(),
        Color::LightRed => "#f14c4c".to_string(),
        Color::LightGreen => "#23d18b".to_string(),
        Color::LightYellow => "#f5f543".to_string(),
        Color::LightBlue => "#3b8eea".to_string(),
        Color::LightMagenta => "#d670d6".to_string(),
        Color::LightCyan => "#29b8db".to_string(),
        Color::White => "#e5e5e5".to_string(),
        Color::Rgb(r, g, b) => format!("#{:02x}{:02x}{:02x}", r, g, b),
        Color::Indexed(i) => indexed_color_to_hex(i),
    }
}

fn color_to_bg_hex(color: Color) -> String {
    match color {
        Color::Reset => "#1e1e2e".to_string(), // Dark background
        _ => color_to_hex(color),
    }
}

fn indexed_color_to_hex(idx: u8) -> String {
    match idx {
        0 => "#000000", 1 => "#cd3131", 2 => "#0dbc79", 3 => "#e5e510",
        4 => "#2472c8", 5 => "#bc3fbc", 6 => "#11a8cd", 7 => "#e5e5e5",
        8 => "#666666", 9 => "#f14c4c", 10 => "#23d18b", 11 => "#f5f543",
        12 => "#3b8eea", 13 => "#d670d6", 14 => "#29b8db", 15 => "#ffffff",
        16..=231 => {
            let idx = idx - 16;
            let r = (idx / 36) * 51;
            let g = ((idx % 36) / 6) * 51;
            let b = (idx % 6) * 51;
            return format!("#{:02x}{:02x}{:02x}", r, g, b);
        }
        232..=255 => {
            let v = 8 + (idx - 232) * 10;
            return format!("#{:02x}{:02x}{:02x}", v, v, v);
        }
        _ => "#d4d4d4",
    }
    .to_string()
}

fn buffer_to_svg(
    buf: &Buffer,
    font_family: &str,
    font_size: f64,
    cell_w: u32,
    cell_h: u32,
) -> String {
    let width = buf.area.width;
    let height = buf.area.height;
    let img_w = cell_w * width as u32;
    let img_h = cell_h * height as u32;

    let mut svg = String::with_capacity(img_w as usize * img_h as usize / 4);
    svg.push_str(&format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {} {}">"##,
        img_w, img_h, img_w, img_h
    ));

    // Background
    svg.push_str(&format!(
        r##"<rect width="{}" height="{}" fill="#1e1e2e"/>"##,
        img_w, img_h
    ));

    // Font style
    svg.push_str(&format!(
        r##"<style>text {{ font-family: "{}", monospace; font-size: {}px; dominant-baseline: text-before-edge; }}</style>"##,
        xml_escape(font_family),
        font_size
    ));

    // Render cells: batch adjacent cells with same bg color into rectangles,
    // then render text on top
    for y in 0..height {
        // Background rectangles (batch runs of same bg color)
        let mut x = 0u16;
        while x < width {
            let cell = &buf[(x, y)];
            let bg = color_to_bg_hex(cell.bg);
            if bg == "#1e1e2e" {
                x += 1;
                continue;
            }
            let start_x = x;
            while x < width && color_to_bg_hex(buf[(x, y)].bg) == bg {
                x += 1;
            }
            svg.push_str(&format!(
                r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}"/>"#,
                start_x as u32 * cell_w,
                y as u32 * cell_h,
                (x - start_x) as u32 * cell_w,
                cell_h,
                bg
            ));
        }

        // Text (batch runs of same fg/style)
        x = 0;
        while x < width {
            let cell = &buf[(x, y)];
            let sym = cell.symbol();
            if sym == " " || sym.is_empty() {
                x += 1;
                continue;
            }
            let fg = color_to_hex(cell.fg);
            let bold = cell.modifier.contains(ratatui::style::Modifier::BOLD);

            // Batch consecutive chars with same style
            let start_x = x;
            let mut text_run = String::new();
            while x < width {
                let c = &buf[(x, y)];
                if color_to_hex(c.fg) != fg
                    || c.modifier.contains(ratatui::style::Modifier::BOLD) != bold
                {
                    break;
                }
                let s = c.symbol();
                if s.is_empty() {
                    x += 1;
                    continue;
                }
                text_run.push_str(s);
                x += 1;
            }

            // Trim trailing spaces
            let trimmed = text_run.trim_end();
            if trimmed.is_empty() {
                continue;
            }

            let font_weight = if bold { r#" font-weight="bold""# } else { "" };
            let text_y = y as u32 * cell_h + (cell_h as f64 * 0.15) as u32;

            svg.push_str(&format!(
                r#"<text x="{}" y="{}" fill="{}"{} xml:space="preserve">{}</text>"#,
                start_x as u32 * cell_w,
                text_y,
                fg,
                font_weight,
                xml_escape(trimmed)
            ));
        }
    }

    svg.push_str("</svg>");
    svg
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// Keep buffer_to_ansi public for headless replay (used by run_headless_replay)
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
        Color::Rgb(r, g, b) => out.push_str(&format!("\x1b[38;2;{};{};{}m", r, g, b)),
        Color::Indexed(i) => out.push_str(&format!("\x1b[38;5;{}m", i)),
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
        Color::Rgb(r, g, b) => out.push_str(&format!("\x1b[48;2;{};{};{}m", r, g, b)),
        Color::Indexed(i) => out.push_str(&format!("\x1b[48;5;{}m", i)),
    }
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
        "env": { "SHELL": "/bin/bash", "TERM": "xterm-256color" },
        "title": "jcode replay"
    });
    writeln!(file, "{}", serde_json::to_string(&header)?)?;

    let mut prev_output = String::new();
    for (time, output) in events {
        if *output == prev_output {
            continue;
        }
        let full_output = format!("\x1b[H\x1b[2J{}", output);
        writeln!(file, "{}", serde_json::to_string(&serde_json::json!([time, "o", full_output]))?)?;
        prev_output = output.clone();
    }
    Ok(())
}

async fn convert_asciicast_to_gif(
    asciicast_path: &Path,
    gif_path: &Path,
    font_family: &str,
    font_size: f64,
) -> Result<()> {
    let agg_path = find_command("agg").context("agg not found")?;

    eprintln!("  Converting asciicast → GIF (font: {})...", font_family);
    let status = tokio::process::Command::new(&agg_path)
        .arg(asciicast_path)
        .arg(gif_path)
        .arg("--font-family")
        .arg(font_family)
        .arg("--font-size")
        .arg((font_size as u32).to_string())
        .arg("--speed")
        .arg("1")
        .status()
        .await
        .context("Failed to run agg")?;

    if !status.success() {
        anyhow::bail!("agg failed");
    }
    eprintln!("  Output: {}", gif_path.display());
    Ok(())
}

async fn convert_gif_to_video(gif_path: &Path, output_path: &Path) -> Result<()> {
    let ffmpeg = find_command("ffmpeg").context("ffmpeg not found")?;
    let ext = output_path.extension().and_then(|e| e.to_str()).unwrap_or("mp4");

    eprintln!("  Converting GIF → {}...", ext.to_uppercase());
    let status = tokio::process::Command::new(&ffmpeg)
        .arg("-y").arg("-i").arg(gif_path)
        .arg("-movflags").arg("faststart")
        .arg("-pix_fmt").arg("yuv420p")
        .arg("-vf").arg("scale=trunc(iw/2)*2:trunc(ih/2)*2")
        .arg(output_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status().await.context("Failed to run ffmpeg")?;

    if !status.success() {
        anyhow::bail!("ffmpeg failed");
    }
    eprintln!("  Output: {}", output_path.display());
    Ok(())
}
