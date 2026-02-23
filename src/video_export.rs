use anyhow::{Context, Result};
use base64::Engine;
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
                family = line
                    .strip_prefix("font_family ")
                    .unwrap_or("")
                    .trim()
                    .to_string();
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
    crate::tui::mermaid::set_video_export_mode(true);
    let app = crate::tui::App::new_for_replay(session.clone()).await;

    let (font_family, font_size) = get_terminal_font();
    eprintln!(
        "  Rendering at {}x{}, {}fps, {:.1}x speed (font: {} {}pt)...",
        width, height, fps, speed, font_family, font_size
    );

    let frames = app
        .run_headless_replay(timeline, speed, width, height, fps)
        .await?;

    crate::tui::mermaid::set_video_export_mode(false);

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

    // Render unique SVGs and convert to PNG (parallel with concurrency limit)
    let png_dir = tmp_dir.join("png");
    std::fs::create_dir_all(&png_dir)?;

    // Write all SVGs first
    let mut svg_paths = Vec::new();
    for (i, (_, buf)) in unique_frames.iter().enumerate() {
        let svg = buffer_to_svg(buf, font_family, font_size, cell_w, cell_h);
        let svg_path = tmp_dir.join(format!("frame_{:06}.svg", i));
        std::fs::write(&svg_path, &svg)?;
        svg_paths.push(svg_path);
    }

    // Convert SVG→PNG in parallel batches
    let concurrency = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8);
    let total_unique = unique_frames.len();
    let rendered = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    for chunk in svg_paths.chunks(concurrency) {
        let mut handles = Vec::new();
        for (chunk_offset, svg_path) in chunk.iter().enumerate() {
            let idx = rendered.load(std::sync::atomic::Ordering::Relaxed) + chunk_offset;
            let png_path = png_dir.join(format!("unique_{:06}.png", idx));
            let rsvg = rsvg.clone();
            let svg_path = svg_path.clone();
            let img_w = img_w;
            let img_h = img_h;
            handles.push(tokio::spawn(async move {
                let status = tokio::process::Command::new(&rsvg)
                    .arg("--width")
                    .arg(img_w.to_string())
                    .arg("--height")
                    .arg(img_h.to_string())
                    .arg("--output")
                    .arg(&png_path)
                    .arg(&svg_path)
                    .status()
                    .await;
                let _ = std::fs::remove_file(&svg_path);
                status
            }));
        }
        for handle in handles {
            let status = handle.await?.context("Failed to run rsvg-convert")?;
            if !status.success() {
                anyhow::bail!("rsvg-convert failed");
            }
            let done = rendered.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if done % 20 == 0 || done == total_unique {
                eprint!("\r  Rendering SVG... {}/{}", done, total_unique);
            }
        }
    }
    eprintln!();

    // Create symlinks for the full frame sequence (ffmpeg needs sequential numbering)
    let seq_dir = tmp_dir.join("seq");
    std::fs::create_dir_all(&seq_dir)?;

    for (frame_num, &unique_idx) in frame_indices.iter().enumerate() {
        let src = png_dir.join(format!("unique_{:06}.png", unique_idx));
        let dst = seq_dir.join(format!("frame_{:06}.png", frame_num));
        crate::platform::symlink_or_copy(&src, &dst)?;
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
        .arg("slow")
        .arg("-tune")
        .arg("animation")
        .arg("-r")
        .arg(fps.to_string())
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
            if ca.symbol() != cb.symbol()
                || ca.fg != cb.fg
                || ca.bg != cb.bg
                || ca.modifier != cb.modifier
            {
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
        0 => "#000000",
        1 => "#cd3131",
        2 => "#0dbc79",
        3 => "#e5e510",
        4 => "#2472c8",
        5 => "#bc3fbc",
        6 => "#11a8cd",
        7 => "#e5e5e5",
        8 => "#666666",
        9 => "#f14c4c",
        10 => "#23d18b",
        11 => "#f5f543",
        12 => "#3b8eea",
        13 => "#d670d6",
        14 => "#29b8db",
        15 => "#ffffff",
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

/// A mermaid image region found in the buffer
#[allow(dead_code)]
struct MermaidRegion {
    /// Row where the marker is
    start_row: u16,
    /// Number of rows the image occupies (marker + empty rows)
    height: u16,
    /// The mermaid content hash
    hash: u64,
    /// Path to the cached PNG
    png_path: PathBuf,
    /// Image pixel width
    img_width: u32,
    /// Image pixel height
    img_height: u32,
    /// Column offset where the border indicator starts
    x_offset: u16,
}

/// Scan a buffer for mermaid image placeholder markers.
/// Detects both inline markers (\x00MERMAID_IMAGE:hash\x00) and
/// video export markers (JMERMAID:hash:END).
fn find_mermaid_regions(buf: &Buffer) -> Vec<MermaidRegion> {
    let width = buf.area.width;
    let height = buf.area.height;
    let mut regions = Vec::new();

    for y in 0..height {
        // Build row text while tracking byte-offset-to-column mapping
        let mut row_text = String::new();
        let mut byte_to_col: Vec<u16> = Vec::new();
        for x in 0..width {
            let sym = buf[(x, y)].symbol();
            for _ in 0..sym.len() {
                byte_to_col.push(x);
            }
            row_text.push_str(sym);
        }

        // Try both marker formats
        let (hash, marker_byte_pos) = if let Some(start) = row_text.find("\x00MERMAID_IMAGE:") {
            let after = start + "\x00MERMAID_IMAGE:".len();
            let h = row_text[after..]
                .find('\x00')
                .and_then(|end| u64::from_str_radix(&row_text[after..after + end], 16).ok());
            (h, Some(start))
        } else if let Some(start) = row_text.find("JMERMAID:") {
            let after = start + "JMERMAID:".len();
            let h = row_text[after..]
                .find(":END")
                .and_then(|end| u64::from_str_radix(&row_text[after..after + end], 16).ok());
            (h, Some(start))
        } else {
            (None, None)
        };

        if let Some(hash) = hash {
            // Convert byte offset to cell column using the mapping
            let marker_x = marker_byte_pos
                .and_then(|bp| byte_to_col.get(bp).copied())
                .unwrap_or(0);

            // Determine the right boundary of the region.
            // For JMERMAID markers, find the end of the marker text to infer the pane width.
            // The marker is written into the inner area of a bordered block, so the region
            // extends from marker_x to approximately the right border (which has non-space chars).
            // We find the last non-space character on the marker row as the boundary.
            let region_right = {
                let mut rx = width;
                // Scan backwards to find the inner boundary (skip border chars)
                while rx > marker_x + 1 {
                    rx -= 1;
                    let s = buf[(rx, y)].symbol();
                    if s != " " && !s.is_empty() && !s.starts_with("JMERMAID") {
                        // This is likely a border char - the inner region is to the left of it
                        break;
                    }
                }
                rx // right boundary (exclusive) — the border column
            };

            // Count consecutive empty rows below for image height
            let mut region_height = 1u16;
            for y2 in (y + 1)..height {
                let mut empty = true;
                for x in marker_x..region_right {
                    let s = buf[(x, y2)].symbol();
                    if s != " " && !s.is_empty() {
                        empty = false;
                        break;
                    }
                }
                if empty {
                    region_height += 1;
                } else {
                    break;
                }
            }

            // Look up cached PNG
            if let Some((png_path, img_w, img_h)) = crate::tui::mermaid::get_cached_png(hash) {
                regions.push(MermaidRegion {
                    start_row: y,
                    height: region_height,
                    hash,
                    png_path,
                    img_width: img_w,
                    img_height: img_h,
                    x_offset: marker_x,
                });
            }
        }
    }
    regions
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

    // Find mermaid image regions
    let mermaid_regions = find_mermaid_regions(buf);
    // Track which cell ranges to skip (row -> (start_x, end_x))
    let mut skip_ranges: std::collections::HashMap<u16, Vec<(u16, u16)>> =
        std::collections::HashMap::new();
    for region in &mermaid_regions {
        for r in region.start_row..(region.start_row + region.height) {
            skip_ranges
                .entry(r)
                .or_default()
                .push((region.x_offset, width));
        }
    }

    let mut svg = String::with_capacity(img_w as usize * img_h as usize / 4);
    svg.push_str(&format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{}" height="{}" viewBox="0 0 {} {}">"##,
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
        // Check if this row has mermaid skip ranges
        let skip = skip_ranges.get(&y);
        let should_skip_cell = |x: u16| -> bool {
            if let Some(ranges) = skip {
                ranges.iter().any(|(sx, ex)| x >= *sx && x < *ex)
            } else {
                false
            }
        };

        // Background rectangles (batch runs of same bg color)
        let mut x = 0u16;
        while x < width {
            if should_skip_cell(x) {
                x += 1;
                continue;
            }
            let cell = &buf[(x, y)];
            let bg = color_to_bg_hex(cell.bg);
            if bg == "#000000" {
                x += 1;
                continue;
            }
            let start_x = x;
            while x < width && !should_skip_cell(x) && color_to_bg_hex(buf[(x, y)].bg) == bg {
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

        // Text and box-drawing characters
        x = 0;
        while x < width {
            if should_skip_cell(x) {
                x += 1;
                continue;
            }
            let cell = &buf[(x, y)];
            let sym = cell.symbol();
            if sym == " " || sym.is_empty() {
                x += 1;
                continue;
            }
            if sym.contains('\x00') {
                x += 1;
                continue;
            }

            let first_char = sym.chars().next().unwrap_or(' ');
            if is_box_drawing(first_char) {
                let fg = color_to_hex(cell.fg);

                // Batch consecutive horizontal line chars (─, ━) into single lines
                if first_char == '─' || first_char == '━' {
                    let start_x = x;
                    let thick = first_char == '━';
                    while x < width && !should_skip_cell(x) {
                        let c = buf[(x, y)].symbol().chars().next().unwrap_or(' ');
                        if c != first_char || color_to_hex(buf[(x, y)].fg) != fg {
                            break;
                        }
                        x += 1;
                    }
                    let stroke_w = if thick { 2.5 } else { 1.5 };
                    let cy = y as u32 * cell_h + cell_h / 2;
                    svg.push_str(&format!(
                        r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="{}"/>"#,
                        start_x as u32 * cell_w,
                        cy,
                        x as u32 * cell_w,
                        cy,
                        fg,
                        stroke_w
                    ));
                    continue;
                }

                if let Some(fragment) = box_drawing_to_svg(
                    first_char,
                    x as u32 * cell_w,
                    y as u32 * cell_h,
                    cell_w,
                    cell_h,
                    &fg,
                ) {
                    svg.push_str(&fragment);
                }
                x += 1;
                continue;
            }

            let fg = color_to_hex(cell.fg);
            let bold = cell.modifier.contains(ratatui::style::Modifier::BOLD);

            // Batch consecutive non-box-drawing chars with same style
            let start_x = x;
            let mut text_run = String::new();
            while x < width && !should_skip_cell(x) {
                let c = &buf[(x, y)];
                let s = c.symbol();
                if s.is_empty() || s.contains('\x00') {
                    x += 1;
                    continue;
                }
                // Stop batching if we hit a box-drawing char
                let ch = s.chars().next().unwrap_or(' ');
                if is_box_drawing(ch) {
                    break;
                }
                if color_to_hex(c.fg) != fg
                    || c.modifier.contains(ratatui::style::Modifier::BOLD) != bold
                {
                    break;
                }
                text_run.push_str(s);
                x += 1;
            }

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

    // Embed mermaid PNG images
    for region in &mermaid_regions {
        if let Ok(png_data) = std::fs::read(&region.png_path) {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&png_data);

            // Calculate image placement within the region
            let region_x = region.x_offset as u32 * cell_w;
            let region_y = region.start_row as u32 * cell_h;
            let region_w = (width as u32 - region.x_offset as u32) * cell_w;
            let region_h = region.height as u32 * cell_h;

            // Scale image to fit within the region while preserving aspect ratio
            let aspect = region.img_width as f64 / region.img_height as f64;
            let (draw_w, draw_h) = if region_w as f64 / region_h as f64 > aspect {
                // Region is wider than image aspect — fit by height
                let h = region_h;
                let w = (h as f64 * aspect) as u32;
                (w, h)
            } else {
                // Region is taller than image aspect — fit by width
                let w = region_w;
                let h = (w as f64 / aspect) as u32;
                (w, h)
            };

            // Center the image within the region
            let draw_x = region_x + (region_w.saturating_sub(draw_w)) / 2;
            let draw_y = region_y + (region_h.saturating_sub(draw_h)) / 2;

            svg.push_str(&format!(
                r#"<image x="{}" y="{}" width="{}" height="{}" href="data:image/png;base64,{}"/>"#,
                draw_x, draw_y, draw_w, draw_h, b64
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

fn is_box_drawing(ch: char) -> bool {
    ('\u{2500}'..='\u{257F}').contains(&ch) || ('\u{2580}'..='\u{259F}').contains(&ch)
    // block elements
}

/// Render a single box-drawing character as SVG path/line elements.
/// Returns Some(svg_fragment) if the character is handled, None otherwise.
fn box_drawing_to_svg(
    ch: char,
    px: u32,
    py: u32,
    cw: u32,
    ch_h: u32,
    color: &str,
) -> Option<String> {
    let cx = px + cw / 2;
    let cy = py + ch_h / 2;
    let b = py + ch_h;
    let right = px + cw;

    // Line thickness
    let t = 1.5_f64;
    let t2 = 2.5_f64; // thick/double

    // Helper: horizontal and vertical line segments
    // For each box-drawing char, we draw lines from center to edges
    // L=left, R=right, U=up, D=down
    let (left, right_seg, up, down, thick) = match ch {
        // Light lines
        '─' => (true, true, false, false, false),
        '│' => (false, false, true, true, false),
        '┌' => (false, true, false, true, false),
        '┐' => (true, false, false, true, false),
        '└' => (false, true, true, false, false),
        '┘' => (true, false, true, false, false),
        '├' => (false, true, true, true, false),
        '┤' => (true, false, true, true, false),
        '┬' => (true, true, false, true, false),
        '┴' => (true, true, true, false, false),
        '┼' => (true, true, true, true, false),
        // Rounded corners — quarter-circle arcs connecting to adjacent ─ and │ cells
        // Uses SVG arc (A) for perfect quarter circles
        // Each corner draws: straight segment → arc → straight segment
        '╭' => {
            // Top-left: goes right and down
            let r = cw.min(ch_h) / 2;
            return Some(format!(
                r#"<path d="M {right},{cy} L {arcx},{cy} A {r},{r} 0 0 0 {cx},{arcy} L {cx},{b}" fill="none" stroke="{color}" stroke-width="{t}" stroke-linecap="round"/>"#,
                right = right,
                cy = cy,
                arcx = cx + r,
                r = r,
                cx = cx,
                arcy = cy + r,
                b = b,
                color = color,
                t = t
            ));
        }
        '╮' => {
            // Top-right: goes left and down
            let r = cw.min(ch_h) / 2;
            return Some(format!(
                r#"<path d="M {px},{cy} L {arcx},{cy} A {r},{r} 0 0 1 {cx},{arcy} L {cx},{b}" fill="none" stroke="{color}" stroke-width="{t}" stroke-linecap="round"/>"#,
                px = px,
                cy = cy,
                arcx = cx - r,
                r = r,
                cx = cx,
                arcy = cy + r,
                b = b,
                color = color,
                t = t
            ));
        }
        '╰' => {
            // Bottom-left: goes up and right
            let r = cw.min(ch_h) / 2;
            return Some(format!(
                r#"<path d="M {cx},{py} L {cx},{arcy} A {r},{r} 0 0 0 {arcx},{cy} L {right},{cy}" fill="none" stroke="{color}" stroke-width="{t}" stroke-linecap="round"/>"#,
                cx = cx,
                py = py,
                arcy = cy - r,
                r = r,
                arcx = cx + r,
                cy = cy,
                right = right,
                color = color,
                t = t
            ));
        }
        '╯' => {
            // Bottom-right: goes up and left
            let r = cw.min(ch_h) / 2;
            return Some(format!(
                r#"<path d="M {cx},{py} L {cx},{arcy} A {r},{r} 0 0 1 {arcx},{cy} L {px},{cy}" fill="none" stroke="{color}" stroke-width="{t}" stroke-linecap="round"/>"#,
                cx = cx,
                py = py,
                arcy = cy - r,
                r = r,
                arcx = cx - r,
                cy = cy,
                px = px,
                color = color,
                t = t
            ));
        }
        // Heavy lines
        '━' => (true, true, false, false, true),
        '┃' => (false, false, true, true, true),
        '┏' => (false, true, false, true, true),
        '┓' => (true, false, false, true, true),
        '┗' => (false, true, true, false, true),
        '┛' => (true, false, true, false, true),
        '┣' => (false, true, true, true, true),
        '┫' => (true, false, true, true, true),
        '┳' => (true, true, false, true, true),
        '┻' => (true, true, true, false, true),
        '╋' => (true, true, true, true, true),
        // Double lines
        '═' => {
            let g = 1u32;
            return Some(format!(
                concat!(
                    r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="{}"/>"#,
                    r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="{}"/>"#,
                ),
                px,
                cy - g,
                right,
                cy - g,
                color,
                t,
                px,
                cy + g,
                right,
                cy + g,
                color,
                t,
            ));
        }
        '║' => {
            let g = 1u32;
            return Some(format!(
                concat!(
                    r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="{}"/>"#,
                    r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="{}"/>"#,
                ),
                cx - g,
                py,
                cx - g,
                b,
                color,
                t,
                cx + g,
                py,
                cx + g,
                b,
                color,
                t,
            ));
        }
        // Block elements
        '█' => {
            return Some(format!(
                r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}"/>"#,
                px, py, cw, ch_h, color
            ));
        }
        '▀' => {
            return Some(format!(
                r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}"/>"#,
                px,
                py,
                cw,
                ch_h / 2,
                color
            ));
        }
        '▄' => {
            return Some(format!(
                r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}"/>"#,
                px,
                py + ch_h / 2,
                cw,
                ch_h / 2,
                color
            ));
        }
        '▌' => {
            return Some(format!(
                r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}"/>"#,
                px,
                py,
                cw / 2,
                ch_h,
                color
            ));
        }
        '▐' => {
            return Some(format!(
                r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}"/>"#,
                px + cw / 2,
                py,
                cw / 2,
                ch_h,
                color
            ));
        }
        '░' | '▒' | '▓' => {
            let opacity = match ch {
                '░' => 0.25,
                '▒' => 0.50,
                '▓' => 0.75,
                _ => 0.5,
            };
            return Some(format!(
                r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}" opacity="{}"/>"#,
                px, py, cw, ch_h, color, opacity
            ));
        }
        _ => return None,
    };

    let stroke_w = if thick { t2 } else { t };
    let mut svg = String::new();
    if left {
        svg.push_str(&format!(
            r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="{}" stroke-linecap="round"/>"#,
            px, cy, cx, cy, color, stroke_w
        ));
    }
    if right_seg {
        svg.push_str(&format!(
            r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="{}" stroke-linecap="round"/>"#,
            cx, cy, right, cy, color, stroke_w
        ));
    }
    if up {
        svg.push_str(&format!(
            r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="{}" stroke-linecap="round"/>"#,
            cx, py, cx, cy, color, stroke_w
        ));
    }
    if down {
        svg.push_str(&format!(
            r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="{}" stroke-linecap="round"/>"#,
            cx, cy, cx, b, color, stroke_w
        ));
    }
    Some(svg)
}
