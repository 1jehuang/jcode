use anyhow::{Context, Result};
use ratatui::buffer::Buffer;
use ratatui::style::Color;
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

    let font_px = font_size * 96.0 / 72.0;
    let cell_w = (font_px * 0.6).ceil() as u32;
    let cell_h = (font_px * 1.2).ceil() as u32;

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
    .await
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
        Color::Reset => "#000000".to_string(),
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
        232.. => {
            let v = 8 + (idx - 232) * 10;
            return format!("#{:02x}{:02x}{:02x}", v, v, v);
        }
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
        r##"<rect width="{}" height="{}" fill="#000000"/>"##,
        img_w, img_h
    ));

    let font_px = font_size * 96.0 / 72.0;
    svg.push_str(&format!(
        r##"<style>text {{ font-family: "{}", monospace; font-size: {:.1}px; dominant-baseline: text-before-edge; }}</style>"##,
        xml_escape(font_family),
        font_px
    ));

    // Render cells: batch adjacent cells with same bg color into rectangles,
    // then render text on top
    for y in 0..height {
        // Background rectangles (batch runs of same bg color)
        let mut x = 0u16;
        while x < width {
            let cell = &buf[(x, y)];
            let bg = color_to_bg_hex(cell.bg);
            if bg == "#000000" {
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


