use super::*;

/// Border width for mermaid diagrams (left bar + space)
pub(super) const BORDER_WIDTH: u16 = 2;

fn rect_contains_point(rect: Rect, x: u16, y: u16) -> bool {
    let right = rect.x.saturating_add(rect.width);
    let bottom = rect.y.saturating_add(rect.height);
    x >= rect.x && x < right && y >= rect.y && y < bottom
}

pub(super) fn set_cell_if_visible(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    ch: char,
    style: Option<Style>,
) {
    let bounds = *buf.area();
    if !rect_contains_point(bounds, x, y) {
        return;
    }
    let cell = &mut buf[(x, y)];
    cell.set_char(ch);
    if let Some(style) = style {
        cell.set_style(style);
    }
}

pub(super) fn draw_left_border(buf: &mut Buffer, area: Rect) {
    let clamped = area.intersection(*buf.area());
    if clamped.width == 0 || clamped.height == 0 {
        return;
    }
    let border_style = Style::default().fg(rgb(100, 100, 100)); // DIM_COLOR
    let y_end = clamped.y.saturating_add(clamped.height);
    for row in clamped.y..y_end {
        set_cell_if_visible(buf, clamped.x, row, '│', Some(border_style));
        if clamped.width > 1 {
            let spacer_x = clamped.x.saturating_add(1);
            set_cell_if_visible(buf, spacer_x, row, ' ', None);
        }
    }
}

pub(super) fn render_stateful_image_safe(
    hash: u64,
    area: Rect,
    buf: &mut Buffer,
    protocol: &mut StatefulProtocol,
    resize: Resize,
) -> bool {
    let widget = StatefulImage::default().resize(resize);
    match panic::catch_unwind(panic::AssertUnwindSafe(|| {
        widget.render(area, buf, protocol);
    })) {
        Ok(()) => true,
        Err(payload) => {
            crate::log_warn(&format!(
                "Recovered image render panic for diagram {:016x}: {}",
                hash,
                crate::panic_payload_to_string(payload.as_ref())
            ));
            clear_image_area(area, buf);
            false
        }
    }
}

/// Render an image at the given area using ratatui-image
/// If centered is true, the image will be horizontally centered within the area
/// If crop_top is true, clip from the top to show the bottom portion when partially visible
/// Returns the number of rows used
///
/// ## Optimizations
/// - Uses blocking locks for consistent rendering (no frame skipping)
/// - Skips render if area and settings unchanged from last frame
/// - Uses Fit mode for small terminals to scale instead of crop
/// - Only clears area if render fails
/// - Draws a left border (like code blocks) for visual consistency
pub fn render_image_widget(
    hash: u64,
    area: Rect,
    buf: &mut Buffer,
    centered: bool,
    crop_top: bool,
) -> u16 {
    // In video export mode, skip terminal image protocol rendering.
    // The placeholder marker stays in the buffer so the SVG pipeline
    // can detect it and embed the cached PNG directly.
    if VIDEO_EXPORT_MODE.load(Ordering::Relaxed) {
        return area.height;
    }

    let buf_area = *buf.area();
    let area = area.intersection(buf_area);

    if area.width == 0 || area.height == 0 {
        return 0;
    }

    // Skip if area is too small (need room for border + image)
    if area.width <= BORDER_WIDTH {
        return 0;
    }

    // Draw left border (vertical bar like code blocks)
    draw_left_border(buf, area);

    // Adjust area for image (after border)
    let image_area = Rect {
        x: area.x + BORDER_WIDTH,
        y: area.y,
        width: area.width - BORDER_WIDTH,
        height: area.height,
    };

    // Skip if image area is too small
    if image_area.width == 0 {
        return area.height;
    }

    let min_cached_width = PICKER
        .get()
        .and_then(|p| p.as_ref())
        .map(|picker| image_area.width as u32 * picker.font_size().0 as u32);
    let cached = get_cached_diagram(hash, min_cached_width);
    let (img_width, path) = if let Some(cached) = cached {
        (cached.width, Some(cached.path))
    } else {
        (0, None)
    };

    // Calculate the actual render area (potentially centered within image_area)
    let render_area = if centered && img_width > 0 {
        // Calculate actual rendered width in terminal cells
        let rendered_width = if let Some(Some(picker)) = PICKER.get() {
            let font_size = picker.font_size();
            let img_width_cells = (img_width as f32 / font_size.0 as f32).ceil() as u16;
            img_width_cells.min(image_area.width)
        } else {
            image_area.width
        };

        // Center horizontally within image_area
        let x_offset = (image_area.width.saturating_sub(rendered_width)) / 2;
        Rect {
            x: image_area.x + x_offset,
            y: image_area.y,
            width: rendered_width,
            height: image_area.height,
        }
    } else {
        image_area
    };

    // Try to render from existing state - single lock for the whole operation
    {
        let mut state = IMAGE_STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let needs_reset = state
            .get(&hash)
            .map(|s| {
                s.resize_mode != ResizeMode::Crop
                    || path
                        .as_ref()
                        .map(|p| s.source_path.as_path() != p.as_path())
                        .unwrap_or(false)
            })
            .unwrap_or(false);
        if needs_reset {
            state.remove(&hash);
        }
        if let Some(img_state) = state.get_mut(hash) {
            img_state.resize_mode = ResizeMode::Crop;
            img_state.last_viewport = None;
            // Always use Crop mode - no rescaling during scroll
            let crop_opts = CropOptions {
                clip_top: crop_top,
                clip_left: false,
            };

            // If crop direction changed, force a re-encode so we don't reuse stale data
            if img_state.last_crop_top != crop_top {
                img_state
                    .protocol
                    .resize_encode(&Resize::Crop(Some(crop_opts)), render_area);
                img_state.last_crop_top = crop_top;
            }

            // Track whether this is a geometry-identical frame (for skipped_renders stat).
            let same_area = img_state.last_area == Some(render_area);
            let state_key = LastRenderState {
                area: render_area,
                crop_top,
                resize_mode: ResizeMode::Crop,
            };
            {
                let last_same = LAST_RENDER
                    .lock()
                    .ok()
                    .and_then(|mut map| {
                        let prev = map.get(&hash).cloned();
                        map.insert(hash, state_key.clone());
                        prev
                    })
                    .map(|prev| prev == state_key)
                    .unwrap_or(false);
                if last_same
                    && same_area
                    && let Ok(mut dbg) = MERMAID_DEBUG.lock()
                {
                    dbg.stats.skipped_renders += 1;
                }
            }
            if let Ok(mut dbg) = MERMAID_DEBUG.lock() {
                dbg.stats.image_state_hits += 1;
            }
            if !render_stateful_image_safe(
                hash,
                render_area,
                buf,
                &mut img_state.protocol,
                Resize::Crop(Some(crop_opts)),
            ) {
                return 0;
            }
            img_state.last_area = Some(render_area);
            return area.height;
        }
    }

    // State miss - need to load image from cache
    if let Some(path) = path
        && let Some(Some(picker)) = PICKER.get()
        && let Ok(img) = image::open(&path)
    {
        if let Ok(mut dbg) = MERMAID_DEBUG.lock() {
            dbg.stats.image_state_misses += 1;
        }
        let protocol = picker.new_resize_protocol(img);

        let mut state = IMAGE_STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.insert(
            hash,
            ImageState {
                protocol,
                source_path: path.clone(),
                last_area: Some(render_area),
                resize_mode: ResizeMode::Crop,
                last_crop_top: false,
                last_viewport: None,
            },
        );

        if let Some(img_state) = state.get_mut(hash) {
            let crop_opts = CropOptions {
                clip_top: crop_top,
                clip_left: false,
            };
            img_state.last_crop_top = crop_top;
            if !render_stateful_image_safe(
                hash,
                render_area,
                buf,
                &mut img_state.protocol,
                Resize::Crop(Some(crop_opts)),
            ) {
                return 0;
            }
            return area.height;
        }
    }

    // Render failed - clear the area to avoid showing stale content
    let clr_area = area.intersection(buf_area);
    if clr_area.width > 0 && clr_area.height > 0 {
        jcode_tui_workspace::color_support::clear_buf(clr_area, buf);
    }

    0
}

/// Render an image using Fit mode (scales to fit the available area).
/// draw_border controls whether a left border is drawn like code blocks.
pub fn render_image_widget_fit(
    hash: u64,
    area: Rect,
    buf: &mut Buffer,
    centered: bool,
    draw_border: bool,
) -> u16 {
    render_image_widget_fit_inner(hash, area, buf, centered, draw_border, false)
}

pub fn render_image_widget_scale(
    hash: u64,
    area: Rect,
    buf: &mut Buffer,
    draw_border: bool,
) -> u16 {
    render_image_widget_fit_inner(hash, area, buf, false, draw_border, true)
}

fn render_image_widget_fit_inner(
    hash: u64,
    area: Rect,
    buf: &mut Buffer,
    centered: bool,
    draw_border: bool,
    scale_up: bool,
) -> u16 {
    if VIDEO_EXPORT_MODE.load(Ordering::Relaxed) {
        return area.height;
    }

    let buf_area = *buf.area();
    let area = area.intersection(buf_area);

    if area.width == 0 || area.height == 0 {
        return 0;
    }

    let border_width = if draw_border { BORDER_WIDTH } else { 0 };
    if area.width <= border_width {
        return 0;
    }

    if draw_border {
        draw_left_border(buf, area);
    }

    let image_area = Rect {
        x: area.x + border_width,
        y: area.y,
        width: area.width - border_width,
        height: area.height,
    };

    if image_area.width == 0 {
        return area.height;
    }

    let min_cached_width = if scale_up {
        None
    } else {
        PICKER
            .get()
            .and_then(|p| p.as_ref())
            .map(|picker| image_area.width as u32 * picker.font_size().0 as u32)
    };
    let cached = get_cached_diagram(hash, min_cached_width);
    let (img_width, path) = if let Some(cached) = cached {
        (cached.width, Some(cached.path))
    } else {
        (0, None)
    };

    let render_area = if centered && img_width > 0 {
        let rendered_width = if let Some(Some(picker)) = PICKER.get() {
            let font_size = picker.font_size();
            let img_width_cells = (img_width as f32 / font_size.0 as f32).ceil() as u16;
            img_width_cells.min(image_area.width)
        } else {
            image_area.width
        };
        let x_offset = (image_area.width.saturating_sub(rendered_width)) / 2;
        Rect {
            x: image_area.x + x_offset,
            y: image_area.y,
            width: rendered_width,
            height: image_area.height,
        }
    } else {
        image_area
    };

    {
        let mut state = IMAGE_STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let target_mode = if scale_up {
            ResizeMode::Scale
        } else {
            ResizeMode::Fit
        };
        let resize = if scale_up {
            Resize::Scale(None)
        } else {
            Resize::Fit(None)
        };
        let needs_reset = state
            .get(&hash)
            .map(|s| {
                s.resize_mode != target_mode
                    || path
                        .as_ref()
                        .map(|p| s.source_path.as_path() != p.as_path())
                        .unwrap_or(false)
            })
            .unwrap_or(false);
        if needs_reset {
            state.remove(&hash);
        }
        if let Some(img_state) = state.get_mut(hash) {
            img_state.resize_mode = target_mode;
            img_state.last_viewport = None;
            // Track identical-geometry frames for skipped_renders stat.
            let same_area = img_state.last_area == Some(render_area);
            let state_key = LastRenderState {
                area: render_area,
                crop_top: false,
                resize_mode: target_mode,
            };
            {
                let last_same = LAST_RENDER
                    .lock()
                    .ok()
                    .and_then(|mut map| {
                        let prev = map.get(&hash).cloned();
                        map.insert(hash, state_key.clone());
                        prev
                    })
                    .map(|prev| prev == state_key)
                    .unwrap_or(false);
                if last_same
                    && same_area
                    && let Ok(mut dbg) = MERMAID_DEBUG.lock()
                {
                    dbg.stats.skipped_renders += 1;
                }
            }
            if let Ok(mut dbg) = MERMAID_DEBUG.lock() {
                dbg.stats.image_state_hits += 1;
                dbg.stats.fit_state_reuse_hits += 1;
            }
            if !render_stateful_image_safe(hash, render_area, buf, &mut img_state.protocol, resize)
            {
                return 0;
            }
            img_state.last_area = Some(render_area);
            return area.height;
        }
    }

    if let Some(path) = path
        && let Some(Some(picker)) = PICKER.get()
        && let Ok(img) = image::open(&path)
    {
        if let Ok(mut dbg) = MERMAID_DEBUG.lock() {
            dbg.stats.image_state_misses += 1;
            dbg.stats.fit_protocol_rebuilds += 1;
        }
        let target_mode = if scale_up {
            ResizeMode::Scale
        } else {
            ResizeMode::Fit
        };
        let resize = if scale_up {
            Resize::Scale(None)
        } else {
            Resize::Fit(None)
        };
        let protocol = picker.new_resize_protocol(img);

        let mut state = IMAGE_STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.insert(
            hash,
            ImageState {
                protocol,
                source_path: path.clone(),
                last_area: Some(render_area),
                resize_mode: target_mode,
                last_crop_top: false,
                last_viewport: None,
            },
        );

        if let Some(img_state) = state.get_mut(hash) {
            if !render_stateful_image_safe(hash, render_area, buf, &mut img_state.protocol, resize)
            {
                return 0;
            }
            return area.height;
        }
    }

    let clr_area = area.intersection(buf_area);
    if clr_area.width > 0 && clr_area.height > 0 {
        jcode_tui_workspace::color_support::clear_buf(clr_area, buf);
    }

    0
}
