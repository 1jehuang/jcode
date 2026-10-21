use super::{
    DEFAULT_RENDER_HEIGHT, DEFAULT_RENDER_WIDTH, RENDER_SUPERSAMPLE, RENDER_WIDTH_BUCKET_CELLS,
    get_font_size,
};
#[cfg(feature = "renderer")]
use super::{RenderConfig, SVG_FONT_DB, Theme};
#[cfg(feature = "renderer")]
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

#[cfg(all(
    feature = "renderer",
    not(all(feature = "mmdr-size-api", mmdr_size_api_available))
))]
pub(super) fn extract_xml_attribute<'a>(tag: &'a str, attr: &str) -> Option<&'a str> {
    let pattern = format!(" {attr}=\"");
    let start = tag.find(&pattern)? + pattern.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

#[cfg(all(
    feature = "renderer",
    not(all(feature = "mmdr-size-api", mmdr_size_api_available))
))]
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

#[cfg(all(
    feature = "renderer",
    not(all(feature = "mmdr-size-api", mmdr_size_api_available))
))]
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

#[cfg(all(
    feature = "renderer",
    not(all(feature = "mmdr-size-api", mmdr_size_api_available))
))]
pub(super) fn parse_svg_explicit_size(tag: &str) -> Option<(f32, f32)> {
    let width = parse_svg_length(extract_xml_attribute(tag, "width")?)?;
    let height = parse_svg_length(extract_xml_attribute(tag, "height")?)?;
    Some((width, height))
}

#[cfg(all(
    feature = "renderer",
    not(all(feature = "mmdr-size-api", mmdr_size_api_available))
))]
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

#[cfg(all(
    feature = "renderer",
    not(all(feature = "mmdr-size-api", mmdr_size_api_available))
))]
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

#[cfg(all(
    feature = "renderer",
    not(all(feature = "mmdr-size-api", mmdr_size_api_available))
))]
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

#[cfg(feature = "renderer")]
fn primary_font_family(fonts: &str) -> String {
    fonts
        .split(',')
        .map(|s| s.trim().trim_matches('"'))
        .find(|s| !s.is_empty())
        .unwrap_or("Inter")
        .to_string()
}

/// Pick the first family in the comma-separated `fonts` list that is actually
/// present in `db`, so usvg's default-font fallback resolves to an installed
/// face. Generic CSS keywords (sans-serif, etc.) are skipped. Falls back to the
/// first listed family when none are installed (preserving prior behavior).
#[cfg(feature = "renderer")]
fn primary_font_family_in_db(fonts: &str, db: &usvg::fontdb::Database) -> String {
    const GENERIC: [&str; 6] = [
        "ui-sans-serif",
        "system-ui",
        "-apple-system",
        "sans-serif",
        "serif",
        "monospace",
    ];
    let mut first_specific: Option<String> = None;
    for family in fonts.split(',').map(|s| s.trim().trim_matches('"')) {
        if family.is_empty() || GENERIC.contains(&family) {
            continue;
        }
        if first_specific.is_none() {
            first_specific = Some(family.to_string());
        }
        let query = usvg::fontdb::Query {
            families: &[usvg::fontdb::Family::Name(family)],
            ..usvg::fontdb::Query::default()
        };
        if db.query(&query).is_some() {
            return family.to_string();
        }
    }
    first_specific.unwrap_or_else(|| primary_font_family(fonts))
}

#[cfg(feature = "renderer")]
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

#[cfg(feature = "renderer")]
pub(super) fn write_output_png_cached_fonts(
    svg: &str,
    output: &Path,
    render_cfg: &RenderConfig,
    theme: &Theme,
) -> anyhow::Result<()> {
    write_svg_png(
        svg,
        output,
        SvgRasterOptions {
            font_family: primary_font_family_in_db(&theme.font_family, &SVG_FONT_DB),
            default_size: (render_cfg.width, render_cfg.height),
            background: theme.background.clone(),
            target_width: None,
            deny_images: false,
        },
    )
    .map(|_| ())
}

#[cfg(feature = "renderer")]
struct SvgRasterOptions {
    font_family: String,
    default_size: (f32, f32),
    background: String,
    target_width: Option<u32>,
    deny_images: bool,
}

#[cfg(feature = "renderer")]
pub(super) fn write_d2_png(
    svg: &str,
    output: &Path,
    target_width: u32,
) -> anyhow::Result<(u32, u32)> {
    super::d2::validate_d2_svg(svg).map_err(anyhow::Error::msg)?;
    write_svg_png(
        svg,
        output,
        SvgRasterOptions {
            font_family: primary_font_family_in_db(
                "Inter, DejaVu Sans, Liberation Sans, sans-serif",
                &SVG_FONT_DB,
            ),
            default_size: (800.0, 600.0),
            background: "#00000000".into(),
            target_width: Some(target_width),
            deny_images: true,
        },
    )
}

#[cfg(feature = "renderer")]
fn write_svg_png(
    svg: &str,
    output: &Path,
    options: SvgRasterOptions,
) -> anyhow::Result<(u32, u32)> {
    let mut opt = usvg::Options {
        font_family: options.font_family,
        default_size: usvg::Size::from_wh(options.default_size.0, options.default_size.1)
            .or_else(|| usvg::Size::from_wh(800.0, 600.0))
            .ok_or_else(|| anyhow::anyhow!("invalid diagram render size"))?,
        fontdb: SVG_FONT_DB.clone(),
        ..Default::default()
    };
    if options.deny_images {
        // resources_dir=None alone still permits absolute local file reads.
        opt.image_href_resolver = usvg::ImageHrefResolver {
            resolve_string: Box::new(|_, _| None),
            resolve_data: Box::new(|_, _, _| None),
        };
    }
    let tree = usvg::Tree::from_str(svg, &opt)?;
    let natural = tree.size();
    let (width, height, scale) = if let Some(target_width) = options.target_width {
        let width = f64::from(natural.width());
        let height = f64::from(natural.height());
        anyhow::ensure!(
            width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0,
            "invalid D2 SVG geometry"
        );
        let scale = (f64::from(target_width.max(1)) / width)
            .min(4096.0 / width)
            .min(4096.0 / height);
        let output_width = (width * scale).round().max(1.0) as u32;
        let output_height = (height * scale).round().max(1.0) as u32;
        anyhow::ensure!(
            output_width <= 4096
                && output_height <= 4096
                && u64::from(output_width) * u64::from(output_height) <= 16 * 1024 * 1024,
            "D2 raster exceeds pixel budget"
        );
        (output_width, output_height, scale as f32)
    } else {
        let size = natural.to_int_size();
        (size.width(), size.height(), 1.0)
    };
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| anyhow::anyhow!("Failed to allocate pixmap"))?;
    if let Some(color) = parse_hex_color_for_png(&options.background) {
        pixmap.fill(color);
    }
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.save_png(output)?;
    Ok((width, height))
}
