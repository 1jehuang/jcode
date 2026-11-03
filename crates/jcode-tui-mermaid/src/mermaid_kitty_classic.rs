//! Classic Kitty placements for multiplexers that cannot render Unicode placeholders.
//!
//! Uploads stay in the existing bounded Kitty cache. A frame owns placements,
//! because unlike placeholders they survive text being scrolled or removed.
use super::*;
use std::fmt::Write as _;

type PixelCrop = (u32, u32, u32, u32);

struct Placement {
    image_id: u32,
    placement_id: u32,
    area: Rect,
    crop: PixelCrop,
    marker: String,
    transmit: Option<String>,
}

#[derive(Default)]
struct ImageFrame {
    pending: Vec<Placement>,
    displayed: Vec<(u32, u32)>,
    cleanup: String,
    generation: u64,
}

static FRAME: LazyLock<Mutex<ImageFrame>> = LazyLock::new(|| Mutex::new(ImageFrame::default()));

#[derive(Clone, Copy, Default)]
struct FrameComposition {
    offset: (u16, u16),
    depth: usize,
}

thread_local! {
    static COMPOSITION: std::cell::Cell<FrameComposition> = const {
        std::cell::Cell::new(FrameComposition { offset: (0, 0), depth: 0 })
    };
}

/// Compose an origin-local image buffer into an already-started outer frame.
/// Nested begin/finish calls are deferred to the outer frame, and image
/// destinations are translated by `(x, y)`. Keep the full local frame buffer
/// when blitting, not an intermediate backend's post-diff buffer, because its
/// internal image markers must survive until the outer finish call.
/// Offsets compose additively and the scope is restored even after a panic.
pub fn with_image_frame_offset<T>(x: u16, y: u16, render: impl FnOnce() -> T) -> T {
    struct Restore(FrameComposition);
    impl Drop for Restore {
        fn drop(&mut self) {
            COMPOSITION.with(|context| context.set(self.0));
        }
    }
    let previous = COMPOSITION.with(|context| {
        let previous = context.get();
        context.set(FrameComposition {
            offset: (
                previous.offset.0.saturating_add(x),
                previous.offset.1.saturating_add(y),
            ),
            depth: previous.depth.saturating_add(1),
        });
        previous
    });
    let _restore = Restore(previous);
    render()
}

pub(super) fn state_hash(hash: u64, fit: Option<(u16, u16)>, font: (u16, u16)) -> u64 {
    if !runtime::use_classic_kitty() {
        return hash;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    ("classic-kitty", fit, font).hash(&mut hasher);
    // Keep the source hash recoverable from cached geometry, so invalidating
    // one diagram can remove every variant without a second unbounded index.
    hash ^ hasher.finish()
}

fn delete_placement(payload: &mut String, image: u32, placement: u32) {
    // Lowercase d=i removes the placement, retaining cached pixel data.
    let _ = write!(payload, "\x1b_Ga=d,d=i,i={image},p={placement},q=2\x1b\\");
}

/// Begin an image frame before rendering widgets. Pair with
/// [`finish_terminal_image_frame`] after overlays and other text are complete.
/// Native Kitty placeholders and non-Kitty protocols need no frame bookkeeping.
pub fn begin_terminal_image_frame() {
    if COMPOSITION.with(|context| context.get().depth != 0) {
        return;
    }
    let Ok(mut frame) = FRAME.lock() else { return };
    let abandoned = !frame.pending.is_empty();
    frame.pending.clear();
    drop(frame);
    if abandoned {
        // A caller abandoned the preceding frame before finalizing it. Its
        // one-shot uploads were consumed but never written. Invalidate the
        // cache rather than silently reusing terminal image ids that do not exist.
        if let Ok(mut cache) = KITTY_VIEWPORT_STATE.lock() {
            cache.clear();
        }
    }
}

pub(super) fn clear_placements() {
    if let Ok(mut frame) = FRAME.lock() {
        for (image, placement) in std::mem::take(&mut frame.displayed) {
            delete_placement(&mut frame.cleanup, image, placement);
        }
        frame.pending.clear();
    }
}

pub(super) fn take_placement_cleanup() -> String {
    FRAME
        .lock()
        .map(|mut frame| std::mem::take(&mut frame.cleanup))
        .unwrap_or_default()
}

pub(super) fn render_crop(hash: u64, area: Rect, buf: &mut Buffer, crop: PixelCrop) -> bool {
    let clipped = area.intersection(*buf.area());
    if clipped.is_empty() || crop.2 == 0 || crop.3 == 0 {
        return true;
    }
    let mut cache = match KITTY_VIEWPORT_STATE.lock() {
        Ok(cache) => cache,
        Err(_) => return false,
    };
    let Some(state) = cache.get_mut(hash) else {
        return false;
    };
    // Clamp both source coordinates and dimensions. A partial final cell must
    // never ask the terminal to read past the uploaded PNG.
    let proportion =
        |cells: u16, pixels: u32, total: u16| (cells as u64 * pixels as u64 / total as u64) as u32;
    let x = crop
        .0
        .saturating_add(proportion(clipped.x - area.x, crop.2, area.width));
    let y = crop
        .1
        .saturating_add(proportion(clipped.y - area.y, crop.3, area.height));
    let w = proportion(clipped.width, crop.2, area.width).min(state.pixel_size.0.saturating_sub(x));
    let h =
        proportion(clipped.height, crop.3, area.height).min(state.pixel_size.1.saturating_sub(y));
    if w == 0 || h == 0 {
        return true;
    }
    // Take the frame lock before consuming the one-shot upload.
    let Ok(mut frame) = FRAME.lock() else {
        return false;
    };
    let Some((image_id, transmit)) = cache.take_pending_transmit(hash) else {
        return false;
    };
    drop(cache);
    let placement_id = frame.pending.len() as u32 + 1;
    // Internal markers are removed before the backend sees the frame. Checking
    // every cell prevents a later modal or recovered panic frame from being
    // covered by a classic placement rendered earlier in the frame.
    let marker = format!("\x1b]777;jcode-image={placement_id}\x07 ");
    for y in clipped.top()..clipped.bottom() {
        for x in clipped.left()..clipped.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(&marker).set_skip(false);
            }
        }
    }
    frame.pending.push(Placement {
        image_id,
        placement_id,
        area: COMPOSITION.with(|context| {
            let (x, y) = context.get().offset;
            Rect {
                x: clipped.x.saturating_add(x),
                y: clipped.y.saturating_add(y),
                ..clipped
            }
        }),
        crop: (x, y, w, h),
        marker,
        transmit,
    });
    true
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_cell_viewport(
    hash: u64,
    area: Rect,
    buf: &mut Buffer,
    scroll_x: u16,
    scroll_y: u16,
    visible_width: u16,
    visible_height: u16,
) -> bool {
    let font = KITTY_VIEWPORT_STATE
        .lock()
        .ok()
        .and_then(|mut cache| cache.get_mut(hash).map(|state| state.font_size));
    let Some((font_w, font_h)) = font else {
        return false;
    };
    render_crop(
        hash,
        Rect {
            width: area.width.min(visible_width),
            height: area.height.min(visible_height),
            ..area
        },
        buf,
        (
            scroll_x as u32 * font_w.max(1) as u32,
            scroll_y as u32 * font_h.max(1) as u32,
            visible_width as u32 * font_w.max(1) as u32,
            visible_height as u32 * font_h.max(1) as u32,
        ),
    )
}

/// Finish a frame, removing placements that moved or disappeared and placing
/// visible images after all text. Call even when the frame has no images.
/// A later overlay covering any part of an image hides that placement for this
/// frame, rather than allowing terminal graphics to cover the overlay.
pub fn finish_terminal_image_frame(buf: &mut Buffer) -> bool {
    if COMPOSITION.with(|context| context.get().depth != 0) {
        return false;
    }
    if buf.area().is_empty() || buf.content.iter().all(|cell| cell.skip) {
        return false;
    }
    // Drain deletions before observing live ids. An eviction after this point
    // remains queued for the next frame, never before its deferred upload.
    let pending_deletes = viewport_render::take_kitty_delete_payloads();
    let live_ids: std::collections::HashSet<_> = KITTY_VIEWPORT_STATE
        .lock()
        .map(|cache| {
            cache
                .entries
                .values()
                .map(|state| state.unique_id)
                .collect()
        })
        .unwrap_or_default();
    let Ok(mut frame) = FRAME.lock() else {
        return false;
    };
    let mut payload = std::mem::take(&mut frame.cleanup);
    for (image, placement) in std::mem::take(&mut frame.displayed) {
        delete_placement(&mut payload, image, placement);
    }
    payload.push_str(&pending_deletes);
    let pending = std::mem::take(&mut frame.pending);
    // Upload even an occluded image: another placement may share this image id,
    // and the bounded cache has already consumed its one-shot transmission.
    for placement in &pending {
        if let Some(transmit) = &placement.transmit
            && live_ids.contains(&placement.image_id)
        {
            payload.push_str(transmit);
        }
    }
    for placement in pending {
        let area = placement.area;
        let bounds = *buf.area();
        // Translation may put a local image outside the composed destination.
        // Check widened endpoints instead of Rect's saturating u16 edges.
        let mut visible = live_ids.contains(&placement.image_id)
            && area.x >= bounds.x
            && area.y >= bounds.y
            && area.x as u32 + area.width as u32 <= bounds.x as u32 + bounds.width as u32
            && area.y as u32 + area.height as u32 <= bounds.y as u32 + bounds.height as u32;
        for y in placement.area.top()..placement.area.bottom() {
            for x in placement.area.left()..placement.area.right() {
                match buf.cell_mut((x, y)) {
                    Some(cell) if cell.symbol() == placement.marker => {
                        cell.set_symbol(" ").set_skip(false);
                    }
                    _ => visible = false,
                }
            }
        }
        if !visible {
            continue;
        }
        let (x, y, w, h) = placement.crop;
        let _ = write!(
            payload,
            "\x1b[{};{}H\x1b_Ga=p,i={},p={},x={x},y={y},w={w},h={h},c={},r={},C=1,q=2\x1b\\",
            area.y + 1,
            area.x + 1,
            placement.image_id,
            placement.placement_id,
            area.width,
            area.height,
        );
        frame
            .displayed
            .push((placement.image_id, placement.placement_id));
    }
    if payload.is_empty() {
        return false;
    }
    frame.generation = frame.generation.wrapping_add(1);
    // Use the final logical cell, not a wide character's continuation cell or a
    // skipped protocol cell. The backend emits buffer diffs in row-major order.
    let area = *buf.area();
    let mut carrier = None;
    for y in area.top()..area.bottom() {
        let mut x = area.left();
        while x < area.right() {
            let Some(cell) = buf.cell((x, y)) else { break };
            if !cell.skip {
                carrier = Some((x, y));
            }
            let width = if cell.skip || cell.symbol().contains('\x1b') {
                // Protocol symbols contain control bytes, not printable text.
                // Their neighboring skipped cells describe the occupied width.
                1
            } else {
                ratatui::text::Span::raw(cell.symbol()).width().max(1)
            };
            x = x.saturating_add(width.min(u16::MAX as usize) as u16);
        }
    }
    let Some(position) = carrier else {
        return false;
    };
    let cell = buf.cell_mut(position).expect("carrier belongs to buffer");
    let mut symbol = cell.symbol().to_string();
    symbol.push_str("\x1b[s");
    // Force a changed carrier on identical frames too: text repaint/ED2 can
    // erase classic placements even when the image geometry did not change.
    if frame.generation & 1 != 0 {
        symbol.push_str("\x1b[u\x1b[s");
    }
    symbol.push_str(&payload);
    symbol.push_str("\x1b[u");
    cell.set_symbol(&symbol).set_skip(false);
    true
}

/// Route the legacy crop/fit/scale widgets through the custom Kitty path too.
/// ratatui-image's Kitty implementation always emits unsupported U=1.
pub(super) fn render_widget(
    hash: u64,
    path: &Path,
    area: Rect,
    buf: &mut Buffer,
    resize: Resize,
) -> bool {
    let Some(picker) = PICKER.get().and_then(|picker| picker.as_ref()) else {
        return false;
    };
    let Some(source) = viewport_render::load_source_image(hash, path) else {
        return false;
    };
    let font = picker.font_size();
    let placement = match resize {
        Resize::Crop(options) => {
            let hash = state_hash(hash, None, font);
            if viewport_render::ensure_kitty_viewport_state(hash, path, &source, 100, font)
                .is_none()
            {
                return false;
            }
            let w = (area.width as u32 * font.0.max(1) as u32).min(source.width());
            let h = (area.height as u32 * font.1.max(1) as u32).min(source.height());
            let y = if options.is_some_and(|options| options.clip_top) {
                source.height() - h
            } else {
                0
            };
            let x = if options.is_some_and(|options| options.clip_left) {
                source.width() - w
            } else {
                0
            };
            return render_crop(
                hash,
                Rect {
                    width: w.div_ceil(font.0.max(1) as u32).min(area.width as u32) as u16,
                    height: h.div_ceil(font.1.max(1) as u32).min(area.height as u32) as u16,
                    ..area
                },
                buf,
                (x, y, w, h),
            );
        }
        Resize::Fit(_) | Resize::Scale(_) => {
            let scale = (area.width as f64 * font.0.max(1) as f64 / source.width().max(1) as f64)
                .min(area.height as f64 * font.1.max(1) as f64 / source.height().max(1) as f64);
            let scale = if matches!(resize, Resize::Fit(_)) {
                scale.min(1.0)
            } else {
                scale
            };
            let cols = ((source.width() as f64 * scale).ceil() as u32)
                .div_ceil(font.0.max(1) as u32) as u16;
            let rows = ((source.height() as f64 * scale).ceil() as u32)
                .div_ceil(font.1.max(1) as u32) as u16;
            let cols = cols.min(area.width);
            let rows = rows.min(area.height);
            let hash = state_hash(hash, Some((cols, rows)), font);
            let placement =
                viewport_render::ensure_kitty_fit_state(hash, path, &source, cols, rows, font);
            placement.map(|(_, cols, rows)| (hash, cols, rows))
        }
    };
    let Some((hash, cols, rows)) = placement else {
        return false;
    };
    render_cell_viewport(
        hash,
        area,
        buf,
        0,
        0,
        cols.min(area.width),
        rows.min(area.height),
    )
}

#[cfg(test)]
#[path = "mermaid_kitty_classic_tests.rs"]
mod tests;
