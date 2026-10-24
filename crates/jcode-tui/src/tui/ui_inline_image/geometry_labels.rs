use super::{ImageExpandLevel, InlineImageItem, MAX_VIEWPORT_FRACTION_PERCENT, MIN_IMAGE_ROWS};
use crate::tui::mermaid;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

fn fit_geometry_with_cap(width: u32, height: u32, chat_width: u16, cap_rows: u16) -> (u16, u16) {
    // Single source of truth for inline-fit placeholder geometry, shared with
    // the mermaid crate so diagrams and raster images stay in lockstep with
    // the draw-time fit math.
    mermaid::inline_fit_geometry(width, height, chat_width, cap_rows)
}

/// Compute `(rows, cols)` for an inline image at `chat_width`, given a viewport
/// height to cap against.
pub(super) fn fit_geometry(
    width: u32,
    height: u32,
    chat_width: u16,
    viewport_height: u16,
) -> (u16, u16) {
    let cap_rows = ((viewport_height as u32 * MAX_VIEWPORT_FRACTION_PERCENT as u32) / 100)
        .clamp(MIN_IMAGE_ROWS as u32, u16::MAX as u32) as u16;
    fit_geometry_with_cap(width, height, chat_width, cap_rows)
}

/// Compute `(rows, cols)` for an image anchored inside the transcript body at a
/// given expand level. Uses a fixed (viewport-independent) row cap so the body's
/// prepared lines stay valid across resizes (the body cache is keyed by width
/// only); the expand level only swaps which fixed cap applies.
pub(crate) fn fit_geometry_anchored(
    width: u32,
    height: u32,
    chat_width: u16,
    level: ImageExpandLevel,
) -> (u16, u16) {
    match level {
        ImageExpandLevel::Fit => {
            fit_geometry_with_cap(width, height, chat_width, level.anchored_cap_rows())
        }
        ImageExpandLevel::Large | ImageExpandLevel::Full => mermaid::inline_fit_geometry_upscaled(
            width,
            height,
            chat_width,
            level.anchored_cap_rows(),
        ),
    }
}

pub(super) fn register_level_geometries(item: &InlineImageItem, width: u16, fit: (u16, u16)) {
    mermaid::register_inline_level_geometries(
        item.id,
        [
            fit,
            mermaid::inline_fit_geometry_upscaled(
                item.width,
                item.height,
                width,
                ImageExpandLevel::Large.anchored_cap_rows(),
            ),
            mermaid::inline_fit_geometry_upscaled(
                item.width,
                item.height,
                width,
                ImageExpandLevel::Full.anchored_cap_rows(),
            ),
        ],
    );
}

/// Compute how many rows an inline image should occupy at `chat_width`, given a
/// viewport height to cap against.
#[cfg(test)]
pub(super) fn fit_rows(width: u32, height: u32, chat_width: u16, viewport_height: u16) -> u16 {
    fit_geometry(width, height, chat_width, viewport_height).0
}

/// Build the dim label line shown above an inline image, e.g.
/// `  screenshot.png  1920×1080`, with a trailing show/hide badge
/// (`[Alt⇧I] hide` / `[Alt⇧I] show image`) so the toggle is
/// discoverable right where the image renders. The line is kept deliberately
/// short so it fits on one row; there is no visible expand badge, but clicking
/// the rendered image body still cycles the per-image size
/// (see `inline_image_body_target_from_screen`).
pub(crate) fn image_label_line(
    item: &InlineImageItem,
    width: u16,
    images_visible: bool,
    _level: ImageExpandLevel,
) -> Line<'static> {
    let dims = format!("{}×{}", item.width, item.height);
    let label = if item.label.is_empty() {
        dims
    } else {
        format!("{}  {}", item.label, dims)
    };
    let dim = Style::default().add_modifier(Modifier::DIM);
    let prefix = Line::from(vec![Span::styled("  ", dim), Span::styled(label, dim)]);
    if item.collapsed_by_default && !images_visible {
        return jcode_tui_render::truncate_line_preserving_suffix_to_width(
            &prefix,
            &Line::from(Span::styled(" · click to show image", dim)),
            width as usize,
        );
    }
    let mut suffix = vec![Span::raw("  ")];
    suffix.extend(crate::tui::ui::viewport::shortcut_badge_spans(
        'I', dim, dim, dim,
    ));
    suffix.push(Span::raw(" "));
    if images_visible {
        suffix.push(Span::styled("hide", dim));
    } else {
        suffix.push(Span::styled(
            "show image",
            Style::default().add_modifier(Modifier::DIM | Modifier::ITALIC),
        ));
    }
    jcode_tui_render::truncate_line_preserving_suffix_to_width(
        &prefix,
        &Line::from(suffix),
        width as usize,
    )
}
