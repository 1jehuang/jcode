use super::{
    DEFAULT_RENDER_HEIGHT, DEFAULT_RENDER_WIDTH, RENDER_SUPERSAMPLE, RENDER_WIDTH_BUCKET_CELLS,
    RenderConfig, SVG_FONT_DB, Theme, get_font_size,
};
use std::path::Path;

/// Count nodes and edges in mermaid content (rough estimate)
pub(super) fn estimate_diagram_size(content: &str) -> (usize, usize) {
    let mut nodes = 0;
    let mut edges = 0;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("%%") {
            continue;
        }
        if trimmed.contains("-->")
            || trimmed.contains("-.->")
            || trimmed.contains("==>")
            || trimmed.contains("---")
            || trimmed.contains("-.-")
        {
            edges += 1;
        }
        if (trimmed.contains('[') && trimmed.contains(']'))
            || (trimmed.contains('{') && trimmed.contains('}'))
            || (trimmed.contains('(') && trimmed.contains(')'))
        {
            nodes += 1;
        }
    }

    (nodes.max(2), edges.max(1))
}

/// Calculate optimal PNG dimensions based on terminal and diagram complexity
pub(super) fn calculate_render_size(
    node_count: usize,
    edge_count: usize,
    terminal_width: Option<u16>,
) -> (f64, f64) {
    let base_width = if let Some(term_width) = terminal_width {
        let font_width = get_font_size().map(|(w, _)| w).unwrap_or(8) as f64;
        let pixel_width = term_width as f64 * font_width;
        pixel_width.clamp(400.0, DEFAULT_RENDER_WIDTH as f64)
    } else {
        1200.0
    };

    let complexity = node_count + edge_count;
    let complexity_factor = match complexity {
        0..=5 => 0.6,
        6..=15 => 0.8,
        16..=30 => 1.0,
        _ => 1.1,
    };

    let raw_width = (base_width * complexity_factor * RENDER_SUPERSAMPLE)
        .clamp(400.0, DEFAULT_RENDER_WIDTH as f64);
    let width = normalize_render_target_width(raw_width) as f64;
    let height = (width * 0.75).clamp(300.0, DEFAULT_RENDER_HEIGHT as f64);

    (width, height)
}

pub(super) fn normalize_render_target_width(width: f64) -> u32 {
    let width = width.max(1.0).round() as u32;
    let font_width = get_font_size()
        .map(|(w, _)| u32::from(w))
        .unwrap_or(8)
        .max(1);
    let bucket = font_width
        .saturating_mul(RENDER_WIDTH_BUCKET_CELLS)
        .max(font_width);
    let rounded = ((width + (bucket / 2)) / bucket).saturating_mul(bucket);
    rounded.clamp(400, DEFAULT_RENDER_WIDTH)
}

pub(super) fn extract_xml_attribute<'a>(tag: &'a str, attr: &str) -> Option<&'a str> {
    let pattern = format!(" {attr}=\"");
    let start = tag.find(&pattern)? + pattern.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

pub(super) fn parse_svg_length(value: &str) -> Option<f32> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.ends_with('%') {
        return None;
    }
    let normalized = trimmed.strip_suffix("px").unwrap_or(trimmed);
    let parsed = normalized.parse::<f32>().ok()?;
    if parsed.is_finite() && parsed > 0.0 {
        Some(parsed)
    } else {
        None
    }
}

pub(super) fn parse_svg_viewbox_size(tag: &str) -> Option<(f32, f32)> {
    let viewbox = extract_xml_attribute(tag, "viewBox")?;
    let mut parts = viewbox.split_whitespace();
    let _min_x = parts.next()?.parse::<f32>().ok()?;
    let _min_y = parts.next()?.parse::<f32>().ok()?;
    let width = parts.next()?.parse::<f32>().ok()?;
    let height = parts.next()?.parse::<f32>().ok()?;
    if width.is_finite() && width > 0.0 && height.is_finite() && height > 0.0 {
        Some((width, height))
    } else {
        None
    }
}

pub(super) fn parse_svg_explicit_size(tag: &str) -> Option<(f32, f32)> {
    let width = parse_svg_length(extract_xml_attribute(tag, "width")?)?;
    let height = parse_svg_length(extract_xml_attribute(tag, "height")?)?;
    Some((width, height))
}

fn format_svg_length(value: f32) -> String {
    let mut out = format!("{:.3}", value.max(1.0));
    while out.ends_with('0') {
        out.pop();
    }
    if out.ends_with('.') {
        out.pop();
    }
    out
}

pub(super) fn set_xml_attribute(tag: &str, attr: &str, value: &str) -> String {
    let pattern = format!(" {attr}=\"");
    if let Some(start) = tag.find(&pattern) {
        let value_start = start + pattern.len();
        if let Some(end_rel) = tag[value_start..].find('"') {
            let value_end = value_start + end_rel;
            let mut updated = String::with_capacity(tag.len() + value.len());
            updated.push_str(&tag[..value_start]);
            updated.push_str(value);
            updated.push_str(&tag[value_end..]);
            return updated;
        }
    }

    let insert_pos = tag.rfind('>').unwrap_or(tag.len());
    let mut updated = String::with_capacity(tag.len() + attr.len() + value.len() + 4);
    updated.push_str(&tag[..insert_pos]);
    updated.push_str(&format!(" {attr}=\"{value}\""));
    updated.push_str(&tag[insert_pos..]);
    updated
}

pub(super) fn retarget_svg_for_png(svg: &str, target_width: f64, target_height: f64) -> String {
    let Some(start) = svg.find("<svg") else {
        return svg.to_string();
    };
    let Some(end_rel) = svg[start..].find('>') else {
        return svg.to_string();
    };
    let end = start + end_rel;
    let root_tag = &svg[start..=end];

    let (resolved_width, resolved_height) = parse_svg_viewbox_size(root_tag)
        .or_else(|| parse_svg_explicit_size(root_tag))
        .map(|(width, height)| {
            let target_width = target_width.max(1.0) as f32;
            let target_height = target_height.max(1.0) as f32;
            let width_scale = target_width / width.max(1.0);
            let height_scale = target_height / height.max(1.0);
            let scale = width_scale.min(height_scale).max(0.0001);
            let output_width = (width * scale).max(1.0);
            let output_height = (height * scale).max(1.0);
            (output_width, output_height)
        })
        .unwrap_or_else(|| (target_width.max(1.0) as f32, target_height.max(1.0) as f32));

    let root_tag = set_xml_attribute(root_tag, "width", &format_svg_length(resolved_width));
    let root_tag = set_xml_attribute(&root_tag, "height", &format_svg_length(resolved_height));

    let mut updated = String::with_capacity(svg.len() - (end + 1 - start) + root_tag.len());
    updated.push_str(&svg[..start]);
    updated.push_str(&root_tag);
    updated.push_str(&svg[end + 1..]);
    updated
}

fn primary_font_family(fonts: &str) -> String {
    fonts
        .split(',')
        .map(|s| s.trim().trim_matches('"'))
        .find(|s| !s.is_empty())
        .unwrap_or("Inter")
        .to_string()
}

fn parse_hex_color_for_png(input: &str) -> Option<resvg::tiny_skia::Color> {
    let color = input.trim();
    let hex = color.strip_prefix('#')?;
    let (r, g, b, a) = match hex.len() {
        3 => {
            let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
            let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
            let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
            (r, g, b, 255)
        }
        4 => {
            let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
            let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
            let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
            let a = u8::from_str_radix(&hex[3..4].repeat(2), 16).ok()?;
            (r, g, b, a)
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            (r, g, b, 255)
        }
        8 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
            (r, g, b, a)
        }
        _ => return None,
    };
    resvg::tiny_skia::Color::from_rgba8(r, g, b, a).into()
}

pub(super) fn write_output_png_cached_fonts(
    svg: &str,
    output: &Path,
    render_cfg: &RenderConfig,
    theme: &Theme,
) -> anyhow::Result<()> {
    let opt = usvg::Options {
        font_family: primary_font_family(&theme.font_family),
        default_size: usvg::Size::from_wh(render_cfg.width, render_cfg.height)
            .or_else(|| usvg::Size::from_wh(800.0, 600.0))
            .ok_or_else(|| anyhow::anyhow!("invalid mermaid render size"))?,
        fontdb: SVG_FONT_DB.clone(),
        ..Default::default()
    };

    let tree = usvg::Tree::from_str(svg, &opt)?;
    let size = tree.size().to_int_size();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height())
        .ok_or_else(|| anyhow::anyhow!("Failed to allocate pixmap"))?;
    if let Some(color) = parse_hex_color_for_png(&theme.background) {
        pixmap.fill(color);
    }

    let mut pixmap_mut = pixmap.as_mut();
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::default(),
        &mut pixmap_mut,
    );
    pixmap.save_png(output)?;
    Ok(())
}
