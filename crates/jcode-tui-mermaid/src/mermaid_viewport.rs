use super::widget_render::set_cell_if_visible;
use super::*;

fn load_source_image(hash: u64, path: &Path) -> Option<Arc<DynamicImage>> {
    if let Ok(mut cache) = SOURCE_CACHE.lock()
        && let Some(img) = cache.get(hash, path)
    {
        return Some(img);
    }

    let img = image::open(path).ok()?;
    if let Ok(mut cache) = SOURCE_CACHE.lock() {
        return Some(cache.insert(hash, path.to_path_buf(), img));
    }
    Some(Arc::new(img))
}

/// Drop a decoded source once a stable Kitty fit transmission has been built.
/// The terminal retains the transmitted pixels and the on-disk cache retains
/// the source bytes, so keeping the full decoded image as well only adds tens of
/// MiB to long screenshot-heavy conversations.
fn release_source_image(hash: u64, path: &Path) {
    if let Ok(mut cache) = SOURCE_CACHE.lock() {
        cache.remove_if_path(hash, path);
    }
}

fn fitted_source_key(
    hash: u64,
    target_cols: u16,
    target_rows: u16,
    font_size: (u16, u16),
) -> FittedSourceKey {
    FittedSourceKey {
        hash,
        target_cols,
        target_rows,
        font_size,
    }
}

/// A fitted source is draw-ready when either its scaled variant is cached or
/// the decoded original already fits the target and therefore needs no resize.
fn fitted_source_is_ready(
    hash: u64,
    source_path: &Path,
    target_cols: u16,
    target_rows: u16,
    font_size: (u16, u16),
) -> bool {
    let key = fitted_source_key(hash, target_cols, target_rows, font_size);
    if let Ok(mut cache) = FITTED_SOURCE_CACHE.lock()
        && cache.get(key, source_path).is_some()
    {
        return true;
    }

    let max_w_px = (target_cols as u32).saturating_mul(font_size.0.max(1) as u32);
    let max_h_px = (target_rows as u32).saturating_mul(font_size.1.max(1) as u32);
    SOURCE_CACHE
        .lock()
        .ok()
        .and_then(|cache| {
            cache.entries.get(&hash).map(|entry| {
                entry.path == source_path
                    && entry.image.width() <= max_w_px
                    && entry.image.height() <= max_h_px
            })
        })
        .unwrap_or(false)
}

fn load_fitted_source(
    hash: u64,
    source_path: &Path,
    target_cols: u16,
    target_rows: u16,
    font_size: (u16, u16),
) -> Option<Arc<DynamicImage>> {
    let key = fitted_source_key(hash, target_cols, target_rows, font_size);
    if let Ok(mut cache) = FITTED_SOURCE_CACHE.lock()
        && let Some(image) = cache.get(key, source_path)
    {
        return Some(image);
    }

    let source = load_source_image(hash, source_path)?;
    let scaled = match scale_to_fit_box(source.as_ref(), target_cols, target_rows, font_size) {
        Cow::Borrowed(_) => return Some(source),
        Cow::Owned(image) => image,
    };
    let fitted = FITTED_SOURCE_CACHE
        .lock()
        .ok()
        .map(|mut cache| cache.insert(key, source_path.to_path_buf(), scaled))?;
    drop(source);
    // The fitted variant now owns the pixels needed for scrolling. Keeping the
    // full decoded original as well would double the resident cost.
    release_source_image(hash, source_path);
    Some(fitted)
}

pub(super) fn viewport_crop_should_scale_to_area(
    crop_w: u32,
    crop_h: u32,
    view_w_px: u32,
    view_h_px: u32,
) -> bool {
    crop_w == view_w_px && crop_h == view_h_px
}

fn kitty_viewport_unique_id(hash: u64) -> u32 {
    let _ = hash;
    loop {
        let sequence = NEXT_KITTY_IMAGE_ID.fetch_add(1, Ordering::Relaxed) & 0x00FF_FFFF;
        if sequence != 0 {
            // Reserve the 0x4Axxxxxx namespace ('J') for jcode's custom virtual
            // placements so these ids do not collide with ratatui-image's Kitty
            // protocol states in the same terminal process.
            return 0x4A00_0000 | sequence;
        }
    }
}

fn kitty_is_tmux() -> bool {
    std::env::var("TERM").is_ok_and(|term| term.starts_with("tmux"))
        || std::env::var("TERM_PROGRAM").is_ok_and(|term_program| term_program == "tmux")
}

fn kitty_transmit_payload(bytes: &[u8], id: u32, format: &str) -> String {
    use std::fmt::Write as _;

    let (start, escape, end) = Parser::escape_tmux(kitty_is_tmux());
    let chunks = bytes.chunks(4096 / 4 * 3);
    let chunk_count = chunks.len();
    let encoded_len = bytes.len().saturating_add(2) / 3 * 4;
    let mut data = String::with_capacity(
        start
            .len()
            .saturating_add(end.len())
            .saturating_add(encoded_len)
            .saturating_add(chunk_count.saturating_mul(48)),
    );
    data.push_str(start);
    for (i, chunk) in chunks.enumerate() {
        data.push_str(escape);

        match i {
            0 => {
                let more = if chunk_count > 1 { 1 } else { 0 };
                let _ = write!(data, "_Gq=2,i={id},a=T,U=1,{format},t=d,m={more};");
            }
            n if n + 1 == chunk_count => {
                data.push_str("_Gq=2,m=0;");
            }
            _ => {
                data.push_str("_Gq=2,m=1;");
            }
        }
        base64::engine::general_purpose::STANDARD.encode_string(chunk, &mut data);
        data.push_str(escape);
        data.push('\\');
    }
    data.push_str(end);

    data
}

fn kitty_delete_image_payload(id: u32) -> String {
    use std::fmt::Write as _;

    let (start, escape, end) = Parser::escape_tmux(kitty_is_tmux());
    let mut data = String::with_capacity(start.len() + end.len() + 48);
    data.push_str(start);
    data.push_str(escape);
    let _ = write!(data, "_Gq=2,a=d,d=I,i={id}");
    data.push_str(escape);
    data.push('\\');
    data.push_str(end);
    data
}

pub(super) fn take_kitty_delete_payloads() -> String {
    let ids = take_kitty_delete_ids();
    let mut payloads = String::with_capacity(ids.len().saturating_mul(48));
    for id in ids {
        payloads.push_str(&kitty_delete_image_payload(id));
    }
    payloads
}

fn kitty_transmit_virtual(img: &DynamicImage, id: u32) -> String {
    // Kitty accepts PNG payloads directly (`f=100`). Mermaid and generated
    // images compress especially well, while the previous raw RGBA upload sent
    // width*height*4 bytes plus base64 overhead. A 780x585 sequence diagram was
    // 97 KiB as PNG but 2.4 MiB as base64 RGBA; a zoomed side-panel image could
    // exceed 20 MiB and block the terminal flush for several seconds.
    let mut png = std::io::Cursor::new(Vec::new());
    if img.write_to(&mut png, image::ImageFormat::Png).is_ok() {
        return kitty_transmit_payload(png.get_ref(), id, "f=100");
    }

    // Encoding should only fail for an unsupported in-memory image shape. Keep
    // the raw path as a defense-in-depth fallback rather than dropping the image.
    let (w, h) = (img.width(), img.height());
    let rgba = img.to_rgba8();
    kitty_transmit_payload(rgba.as_raw(), id, &format!("f=32,s={w},v={h}"))
}

/// Reuse an already-compressed PNG when the transmitted pixels are byte-for-byte
/// the on-disk source. This avoids a full DynamicImage -> PNG deflate pass for
/// unscaled Mermaid diagrams and small PNG screenshots.
fn kitty_transmit_png_path(path: &Path, id: u32) -> Option<String> {
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
    {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    Some(kitty_transmit_payload(&bytes, id, "f=100"))
}

fn kitty_scaled_image_for_zoom(source: &DynamicImage, zoom_percent: u8) -> Cow<'_, DynamicImage> {
    use image::imageops::FilterType;

    let zoom = zoom_percent.clamp(50, 200) as u32;
    if zoom == 100 {
        return Cow::Borrowed(source);
    }

    let scaled_w = ((source.width() as u64).saturating_mul(zoom as u64) / 100)
        .max(1)
        .min(u32::MAX as u64) as u32;
    let scaled_h = ((source.height() as u64).saturating_mul(zoom as u64) / 100)
        .max(1)
        .min(u32::MAX as u64) as u32;
    Cow::Owned(source.resize_exact(scaled_w, scaled_h, FilterType::Nearest))
}

fn div_ceil_u32_local(value: u32, divisor: u32) -> u32 {
    value
        .saturating_add(divisor.saturating_sub(1))
        .checked_div(divisor)
        .unwrap_or(value)
}

fn cell_rect_for_image(img: &DynamicImage, font_size: (u16, u16)) -> (u16, u16) {
    (
        div_ceil_u32_local(img.width().max(1), font_size.0.max(1) as u32).min(u16::MAX as u32)
            as u16,
        div_ceil_u32_local(img.height().max(1), font_size.1.max(1) as u32).min(u16::MAX as u32)
            as u16,
    )
}

pub(super) fn ensure_kitty_viewport_state(
    hash: u64,
    source_path: &Path,
    source: &DynamicImage,
    zoom_percent: u8,
    font_size: (u16, u16),
) -> Option<(u32, u16, u16)> {
    let zoom_percent = zoom_percent.clamp(50, 200);
    let mut cache = KITTY_VIEWPORT_STATE.lock().ok()?;
    if let Some(state) = cache.get_mut(hash)
        && state.source_path == source_path
        && state.zoom_percent == zoom_percent
        && state.font_size == font_size
        && state.fit_target.is_none()
    {
        return Some((state.unique_id, state.full_cols, state.full_rows));
    }

    let scaled = kitty_scaled_image_for_zoom(source, zoom_percent);
    let (full_cols, full_rows) = cell_rect_for_image(&scaled, font_size);
    if full_cols == 0 || full_rows == 0 {
        return None;
    }

    let unique_id = cache
        .get_mut(hash)
        .map(|state| state.unique_id)
        .unwrap_or_else(|| kitty_viewport_unique_id(hash));

    let pending_transmit = if zoom_percent == 100 {
        kitty_transmit_png_path(source_path, unique_id)
            .unwrap_or_else(|| kitty_transmit_virtual(&scaled, unique_id))
    } else {
        kitty_transmit_virtual(&scaled, unique_id)
    };
    let pending_transmit_bytes = pending_transmit.len();
    cache.insert(
        hash,
        KittyViewportState {
            source_path: source_path.to_path_buf(),
            zoom_percent,
            font_size,
            unique_id,
            full_cols,
            full_rows,
            pending_transmit: Some(pending_transmit),
            pending_transmit_bytes,
            fit_target: None,
        },
    );

    if let Ok(mut dbg) = MERMAID_DEBUG.lock() {
        dbg.stats.viewport_protocol_rebuilds += 1;
    }

    cache
        .get_mut(hash)
        .map(|state| (state.unique_id, state.full_cols, state.full_rows))
}

/// Ensure Kitty virtual-placement state for an inline raster image scaled once
/// to fit `(target_cols, target_rows)`. Unlike the zoomable diagram viewport,
/// the scaled size is fixed by the placeholder geometry, so partial visibility
/// during scrolling reuses the already-transmitted pixels and only re-addresses
/// rows via unicode placeholders (no per-frame rescale or retransmit).
pub(super) fn ensure_kitty_fit_state(
    hash: u64,
    source_path: &Path,
    source: &DynamicImage,
    target_cols: u16,
    target_rows: u16,
    font_size: (u16, u16),
) -> Option<(u32, u16, u16)> {
    if target_cols == 0 || target_rows == 0 {
        return None;
    }
    // Fast path: the state already matches. Hold the lock only for this cheap
    // probe, never across the expensive scale/transmit below.
    let existing_unique_id = {
        let mut cache = KITTY_VIEWPORT_STATE.lock().ok()?;
        if let Some(state) = cache.get_mut(hash) {
            if state.source_path == source_path
                && state.font_size == font_size
                && state.fit_target == Some((target_cols, target_rows))
            {
                return Some((state.unique_id, state.full_cols, state.full_rows));
            }
            Some(state.unique_id)
        } else {
            None
        }
    };

    // Heavy work (resize + RGBA conversion + base64 transmit encoding) runs
    // WITHOUT holding KITTY_VIEWPORT_STATE. The off-thread prewarm worker calls
    // this, and the UI thread's per-frame `probe_kitty_fit_state` needs that
    // same lock; holding it across the encode would serialize the prewarm
    // worker against every scroll frame and reintroduce the very stall the
    // off-thread prewarm exists to avoid.
    let scaled = scale_to_fit_box(source, target_cols, target_rows, font_size);
    let (full_cols, full_rows) = cell_rect_for_image(&scaled, font_size);
    if full_cols == 0 || full_rows == 0 {
        return None;
    }
    let unique_id = existing_unique_id.unwrap_or_else(|| kitty_viewport_unique_id(hash));
    let pending_transmit = if matches!(&scaled, Cow::Borrowed(_)) {
        kitty_transmit_png_path(source_path, unique_id)
            .unwrap_or_else(|| kitty_transmit_virtual(&scaled, unique_id))
    } else {
        kitty_transmit_virtual(&scaled, unique_id)
    };
    let pending_transmit_bytes = pending_transmit.len();

    let mut cache = KITTY_VIEWPORT_STATE.lock().ok()?;
    // Re-check under the lock: another thread may have built matching state
    // while we were encoding. If so, reuse it and drop our redundant transmit.
    if let Some(state) = cache.get_mut(hash)
        && state.source_path == source_path
        && state.font_size == font_size
        && state.fit_target == Some((target_cols, target_rows))
    {
        return Some((state.unique_id, state.full_cols, state.full_rows));
    }

    cache.insert(
        hash,
        KittyViewportState {
            source_path: source_path.to_path_buf(),
            zoom_percent: 100,
            font_size,
            unique_id,
            full_cols,
            full_rows,
            pending_transmit: Some(pending_transmit),
            pending_transmit_bytes,
            fit_target: Some((target_cols, target_rows)),
        },
    );

    if let Ok(mut dbg) = MERMAID_DEBUG.lock() {
        dbg.stats.fit_protocol_rebuilds += 1;
    }

    cache
        .get_mut(hash)
        .map(|state| (state.unique_id, state.full_cols, state.full_rows))
}

/// Scale a source image once to fit a `(cols, rows)` cell box at `font_size`,
/// preserving aspect ratio. Returns a clone when the source already fits.
fn scale_to_fit_box<'a>(
    source: &'a DynamicImage,
    target_cols: u16,
    target_rows: u16,
    font_size: (u16, u16),
) -> Cow<'a, DynamicImage> {
    let max_w_px = (target_cols as u32).saturating_mul(font_size.0.max(1) as u32);
    let max_h_px = (target_rows as u32).saturating_mul(font_size.1.max(1) as u32);
    if source.width() <= max_w_px && source.height() <= max_h_px {
        Cow::Borrowed(source)
    } else {
        Cow::Owned(source.resize(max_w_px, max_h_px, image::imageops::FilterType::Triangle))
    }
}

pub(super) fn render_kitty_virtual_viewport(
    hash: u64,
    area: Rect,
    buf: &mut Buffer,
    scroll_x: u16,
    scroll_y: u16,
    visible_width: u16,
    visible_height: u16,
) -> bool {
    if visible_width == 0 || visible_height == 0 {
        return true;
    }

    let mut cache = match KITTY_VIEWPORT_STATE.lock() {
        Ok(cache) => cache,
        Err(_) => return false,
    };
    let Some((unique_id, mut pending_transmit)) = cache.take_pending_transmit(hash) else {
        return false;
    };
    drop(cache);
    let mut pending_deletes = Some(take_kitty_delete_payloads());

    if pending_transmit.is_none()
        && let Ok(mut dbg) = MERMAID_DEBUG.lock()
    {
        dbg.stats.viewport_state_reuse_hits += 1;
    }

    let [id_extra, id_r, id_g, id_b] = unique_id.to_be_bytes();
    let id_color = format!("\x1b[38;2;{id_r};{id_g};{id_b}m");
    let right = area.width.saturating_sub(1);
    let down = area.height.saturating_sub(1);

    for row in 0..area.height {
        let y = area.top() + row;
        if row >= visible_height {
            for x in 0..area.width {
                if let Some(cell) = buf.cell_mut((area.left() + x, y)) {
                    cell.set_symbol(" ");
                    cell.set_skip(false);
                }
            }
            continue;
        }

        let mut symbol = if row == 0 {
            let mut symbol = pending_deletes.take().unwrap_or_default();
            symbol.push_str(&pending_transmit.take().unwrap_or_default());
            symbol
        } else {
            String::new()
        };
        symbol.push_str("\x1b[s");
        symbol.push_str(&id_color);
        kitty_add_placeholder(
            &mut symbol,
            scroll_x,
            scroll_y.saturating_add(row),
            id_extra,
        );
        for x in 1..area.width {
            if let Some(cell) = buf.cell_mut((area.left() + x, y)) {
                if x < visible_width {
                    symbol.push('\u{10EEEE}');
                    cell.set_skip(true);
                } else {
                    cell.set_symbol(" ");
                    cell.set_skip(false);
                }
            }
        }
        symbol.push_str(&format!("\x1b[u\x1b[{right}C\x1b[{down}B"));
        if let Some(cell) = buf.cell_mut((area.left(), y)) {
            cell.set_symbol(&symbol);
        }
    }

    true
}

fn can_use_kitty_virtual_viewport(
    full_cols: u16,
    full_rows: u16,
    scroll_x: u16,
    scroll_y: u16,
) -> bool {
    let max_index = KITTY_DIACRITICS.len() as u16;
    full_cols < max_index && full_rows < max_index && scroll_x < max_index && scroll_y < max_index
}

fn kitty_add_placeholder(buf: &mut String, x: u16, y: u16, id_extra: u8) {
    buf.push('\u{10EEEE}');
    buf.push(kitty_diacritic(y));
    buf.push(kitty_diacritic(x));
    buf.push(kitty_diacritic(id_extra as u16));
}

#[inline]
fn kitty_diacritic(index: u16) -> char {
    KITTY_DIACRITICS
        .get(index as usize)
        .copied()
        .unwrap_or(KITTY_DIACRITICS[0])
}

/// From https://sw.kovidgoyal.net/kitty/_downloads/1792bad15b12979994cd6ecc54c967a6/rowcolumn-diacritics.txt
static KITTY_DIACRITICS: [char; 297] = [
    '\u{305}',
    '\u{30D}',
    '\u{30E}',
    '\u{310}',
    '\u{312}',
    '\u{33D}',
    '\u{33E}',
    '\u{33F}',
    '\u{346}',
    '\u{34A}',
    '\u{34B}',
    '\u{34C}',
    '\u{350}',
    '\u{351}',
    '\u{352}',
    '\u{357}',
    '\u{35B}',
    '\u{363}',
    '\u{364}',
    '\u{365}',
    '\u{366}',
    '\u{367}',
    '\u{368}',
    '\u{369}',
    '\u{36A}',
    '\u{36B}',
    '\u{36C}',
    '\u{36D}',
    '\u{36E}',
    '\u{36F}',
    '\u{483}',
    '\u{484}',
    '\u{485}',
    '\u{486}',
    '\u{487}',
    '\u{592}',
    '\u{593}',
    '\u{594}',
    '\u{595}',
    '\u{597}',
    '\u{598}',
    '\u{599}',
    '\u{59C}',
    '\u{59D}',
    '\u{59E}',
    '\u{59F}',
    '\u{5A0}',
    '\u{5A1}',
    '\u{5A8}',
    '\u{5A9}',
    '\u{5AB}',
    '\u{5AC}',
    '\u{5AF}',
    '\u{5C4}',
    '\u{610}',
    '\u{611}',
    '\u{612}',
    '\u{613}',
    '\u{614}',
    '\u{615}',
    '\u{616}',
    '\u{617}',
    '\u{657}',
    '\u{658}',
    '\u{659}',
    '\u{65A}',
    '\u{65B}',
    '\u{65D}',
    '\u{65E}',
    '\u{6D6}',
    '\u{6D7}',
    '\u{6D8}',
    '\u{6D9}',
    '\u{6DA}',
    '\u{6DB}',
    '\u{6DC}',
    '\u{6DF}',
    '\u{6E0}',
    '\u{6E1}',
    '\u{6E2}',
    '\u{6E4}',
    '\u{6E7}',
    '\u{6E8}',
    '\u{6EB}',
    '\u{6EC}',
    '\u{730}',
    '\u{732}',
    '\u{733}',
    '\u{735}',
    '\u{736}',
    '\u{73A}',
    '\u{73D}',
    '\u{73F}',
    '\u{740}',
    '\u{741}',
    '\u{743}',
    '\u{745}',
    '\u{747}',
    '\u{749}',
    '\u{74A}',
    '\u{7EB}',
    '\u{7EC}',
    '\u{7ED}',
    '\u{7EE}',
    '\u{7EF}',
    '\u{7F0}',
    '\u{7F1}',
    '\u{7F3}',
    '\u{816}',
    '\u{817}',
    '\u{818}',
    '\u{819}',
    '\u{81B}',
    '\u{81C}',
    '\u{81D}',
    '\u{81E}',
    '\u{81F}',
    '\u{820}',
    '\u{821}',
    '\u{822}',
    '\u{823}',
    '\u{825}',
    '\u{826}',
    '\u{827}',
    '\u{829}',
    '\u{82A}',
    '\u{82B}',
    '\u{82C}',
    '\u{82D}',
    '\u{951}',
    '\u{953}',
    '\u{954}',
    '\u{F82}',
    '\u{F83}',
    '\u{F86}',
    '\u{F87}',
    '\u{135D}',
    '\u{135E}',
    '\u{135F}',
    '\u{17DD}',
    '\u{193A}',
    '\u{1A17}',
    '\u{1A75}',
    '\u{1A76}',
    '\u{1A77}',
    '\u{1A78}',
    '\u{1A79}',
    '\u{1A7A}',
    '\u{1A7B}',
    '\u{1A7C}',
    '\u{1B6B}',
    '\u{1B6D}',
    '\u{1B6E}',
    '\u{1B6F}',
    '\u{1B70}',
    '\u{1B71}',
    '\u{1B72}',
    '\u{1B73}',
    '\u{1CD0}',
    '\u{1CD1}',
    '\u{1CD2}',
    '\u{1CDA}',
    '\u{1CDB}',
    '\u{1CE0}',
    '\u{1DC0}',
    '\u{1DC1}',
    '\u{1DC3}',
    '\u{1DC4}',
    '\u{1DC5}',
    '\u{1DC6}',
    '\u{1DC7}',
    '\u{1DC8}',
    '\u{1DC9}',
    '\u{1DCB}',
    '\u{1DCC}',
    '\u{1DD1}',
    '\u{1DD2}',
    '\u{1DD3}',
    '\u{1DD4}',
    '\u{1DD5}',
    '\u{1DD6}',
    '\u{1DD7}',
    '\u{1DD8}',
    '\u{1DD9}',
    '\u{1DDA}',
    '\u{1DDB}',
    '\u{1DDC}',
    '\u{1DDD}',
    '\u{1DDE}',
    '\u{1DDF}',
    '\u{1DE0}',
    '\u{1DE1}',
    '\u{1DE2}',
    '\u{1DE3}',
    '\u{1DE4}',
    '\u{1DE5}',
    '\u{1DE6}',
    '\u{1DFE}',
    '\u{20D0}',
    '\u{20D1}',
    '\u{20D4}',
    '\u{20D5}',
    '\u{20D6}',
    '\u{20D7}',
    '\u{20DB}',
    '\u{20DC}',
    '\u{20E1}',
    '\u{20E7}',
    '\u{20E9}',
    '\u{20F0}',
    '\u{2CEF}',
    '\u{2CF0}',
    '\u{2CF1}',
    '\u{2DE0}',
    '\u{2DE1}',
    '\u{2DE2}',
    '\u{2DE3}',
    '\u{2DE4}',
    '\u{2DE5}',
    '\u{2DE6}',
    '\u{2DE7}',
    '\u{2DE8}',
    '\u{2DE9}',
    '\u{2DEA}',
    '\u{2DEB}',
    '\u{2DEC}',
    '\u{2DED}',
    '\u{2DEE}',
    '\u{2DEF}',
    '\u{2DF0}',
    '\u{2DF1}',
    '\u{2DF2}',
    '\u{2DF3}',
    '\u{2DF4}',
    '\u{2DF5}',
    '\u{2DF6}',
    '\u{2DF7}',
    '\u{2DF8}',
    '\u{2DF9}',
    '\u{2DFA}',
    '\u{2DFB}',
    '\u{2DFC}',
    '\u{2DFD}',
    '\u{2DFE}',
    '\u{2DFF}',
    '\u{A66F}',
    '\u{A67C}',
    '\u{A67D}',
    '\u{A6F0}',
    '\u{A6F1}',
    '\u{A8E0}',
    '\u{A8E1}',
    '\u{A8E2}',
    '\u{A8E3}',
    '\u{A8E4}',
    '\u{A8E5}',
    '\u{A8E6}',
    '\u{A8E7}',
    '\u{A8E8}',
    '\u{A8E9}',
    '\u{A8EA}',
    '\u{A8EB}',
    '\u{A8EC}',
    '\u{A8ED}',
    '\u{A8EE}',
    '\u{A8EF}',
    '\u{A8F0}',
    '\u{A8F1}',
    '\u{AAB0}',
    '\u{AAB2}',
    '\u{AAB3}',
    '\u{AAB7}',
    '\u{AAB8}',
    '\u{AABE}',
    '\u{AABF}',
    '\u{AAC1}',
    '\u{FE20}',
    '\u{FE21}',
    '\u{FE22}',
    '\u{FE23}',
    '\u{FE24}',
    '\u{FE25}',
    '\u{FE26}',
    '\u{10A0F}',
    '\u{10A38}',
    '\u{1D185}',
    '\u{1D186}',
    '\u{1D187}',
    '\u{1D188}',
    '\u{1D189}',
    '\u{1D1AA}',
    '\u{1D1AB}',
    '\u{1D1AC}',
    '\u{1D1AD}',
    '\u{1D242}',
    '\u{1D243}',
    '\u{1D244}',
];

/// Cheap cache probe for the inline fit state: returns the placement triple
/// when the transmitted pixels already match `(target_cols, target_rows)` at
/// this font size, without touching the source image at all. Keeps steady-state
/// frames free of source-cache locks and (worst case) re-decodes.
fn probe_kitty_fit_state(
    hash: u64,
    source_path: &Path,
    target_cols: u16,
    target_rows: u16,
    font_size: (u16, u16),
) -> Option<(u32, u16, u16)> {
    let mut cache = KITTY_VIEWPORT_STATE.lock().ok()?;
    let state = cache.get_mut(hash)?;
    (state.source_path == source_path
        && state.font_size == font_size
        && state.fit_target == Some((target_cols, target_rows)))
    .then_some((state.unique_id, state.full_cols, state.full_rows))
}

/// Readiness of the stable-fit path for an inline image at a given placeholder
/// geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineFitReadiness {
    /// Protocol state (Kitty) or a pre-scaled decoded source (other protocols)
    /// already exists for this geometry.
    Ready,
    /// The scaled state is missing. Building it on the UI thread costs tens of
    /// milliseconds for a large screenshot, so callers should prewarm it
    /// off-thread via [`prewarm_inline_fit_state`].
    NeedsPrewarm,
    /// The stable-fit fast path does not apply (no picker or video export).
    Unsupported,
}

/// Check whether stable-fit state for `hash` is ready at the
/// `(target_cols, target_rows)` placeholder geometry. Cheap: bounded in-memory
/// cache probes only, with no decoding or filesystem access.
pub fn inline_fit_readiness(
    hash: u64,
    target_cols: u16,
    target_rows: u16,
    draw_border: bool,
) -> InlineFitReadiness {
    if VIDEO_EXPORT_MODE.load(Ordering::Relaxed) {
        return InlineFitReadiness::Unsupported;
    }
    let picker = match PICKER.get().and_then(|p| p.as_ref()) {
        Some(picker) => picker,
        None => return InlineFitReadiness::Unsupported,
    };
    // Hot path: runs on the UI thread for every visible and prefetched image,
    // every frame. Use the in-memory-only cache lookup so steady-state scrolling
    // never pays a `path.exists()` stat syscall per image per frame.
    let Some(cached) = get_cached_diagram_in_memory(hash) else {
        return InlineFitReadiness::NeedsPrewarm;
    };
    let border_width = if draw_border { BORDER_WIDTH } else { 0 };
    let fit_cols = target_cols.saturating_sub(border_width);
    let ready = if picker.protocol_type() == ProtocolType::Kitty {
        probe_kitty_fit_state(
            hash,
            &cached.path,
            fit_cols,
            target_rows,
            picker.font_size(),
        )
        .is_some()
    } else {
        fitted_source_is_ready(
            hash,
            &cached.path,
            fit_cols,
            target_rows,
            picker.font_size(),
        )
    };
    if ready {
        InlineFitReadiness::Ready
    } else {
        InlineFitReadiness::NeedsPrewarm
    }
}

/// Build the protocol-specific stable-fit state for an inline image, mirroring
/// the geometry math of [`render_image_widget_fit_stable`]. Intended to run off
/// the UI thread so the first visible frame of a large image does not hitch the
/// render loop. Kitty builds its transmit state; other protocols retain a
/// byte-bounded pre-scaled source for cheap visible-row crops.
/// Returns true when the state exists afterwards.
pub fn prewarm_inline_fit_state(
    hash: u64,
    target_cols: u16,
    target_rows: u16,
    draw_border: bool,
) -> bool {
    let picker = match PICKER.get().and_then(|p| p.as_ref()) {
        Some(picker) => picker,
        None => return false,
    };
    let Some(cached) = get_cached_diagram(hash, None) else {
        return false;
    };
    let font_size = picker.font_size();
    let border_width = if draw_border { BORDER_WIDTH } else { 0 };
    let fit_cols = target_cols.saturating_sub(border_width);
    if picker.protocol_type() != ProtocolType::Kitty {
        return load_fitted_source(hash, &cached.path, fit_cols, target_rows, font_size).is_some();
    }
    if probe_kitty_fit_state(hash, &cached.path, fit_cols, target_rows, font_size).is_some() {
        return true;
    }
    let Some(source) = load_source_image(hash, &cached.path) else {
        return false;
    };
    let prepared = ensure_kitty_fit_state(
        hash,
        &cached.path,
        source.as_ref(),
        fit_cols,
        target_rows,
        font_size,
    )
    .is_some();
    drop(source);
    release_source_image(hash, &cached.path);
    prepared
}

/// Draw a rounded left border that hugs the image's real extent: `╭` on the
/// image's first row, `╰` on its last, `│` between. Rows of the placeholder
/// below the image (when the estimate was taller than the fitted image) get no
/// border at all, so the line never extends past the picture.
fn draw_fitted_left_border(buf: &mut Buffer, area: Rect, skip_rows: u16, full_rows: u16) {
    let clamped = area.intersection(*buf.area());
    if clamped.width == 0 || clamped.height == 0 || full_rows == 0 {
        return;
    }
    let visible = clamped.height.min(full_rows.saturating_sub(skip_rows));
    let border_style = Style::default().fg(rgb(100, 100, 100)); // DIM_COLOR
    for row in 0..visible {
        let global_row = skip_rows.saturating_add(row);
        let ch = match (global_row == 0, global_row + 1 >= full_rows) {
            (true, true) => '╶',
            (true, false) => '╭',
            (false, true) => '╰',
            (false, false) => '│',
        };
        let y = clamped.y + row;
        set_cell_if_visible(buf, clamped.x, y, ch, Some(border_style));
        if clamped.width > 1 {
            set_cell_if_visible(buf, clamped.x.saturating_add(1), y, ' ', None);
        }
    }
}

/// Convert a row scroll over a fitted image into an exact source-pixel crop and
/// visible cell extent. Kept protocol-independent so the geometry can be tested
/// without a terminal.
pub(super) fn fitted_scroll_crop(
    image_width: u32,
    image_height: u32,
    font_size: (u16, u16),
    available_cols: u16,
    available_rows: u16,
    skip_rows: u16,
) -> Option<(u32, u32, u16, u16)> {
    if image_width == 0 || image_height == 0 || available_cols == 0 || available_rows == 0 {
        return None;
    }
    let font_w = font_size.0.max(1) as u32;
    let font_h = font_size.1.max(1) as u32;
    let full_cols = div_ceil_u32_local(image_width, font_w).min(u16::MAX as u32) as u16;
    let full_rows = div_ceil_u32_local(image_height, font_h).min(u16::MAX as u32) as u16;
    if skip_rows >= full_rows {
        return None;
    }
    let visible_cols = available_cols.min(full_cols);
    let visible_rows = available_rows.min(full_rows.saturating_sub(skip_rows));
    if visible_cols == 0 || visible_rows == 0 {
        return None;
    }
    let scroll_y_px = (skip_rows as u32).saturating_mul(font_h).min(image_height);
    let crop_h_px = (visible_rows as u32)
        .saturating_mul(font_h)
        .min(image_height.saturating_sub(scroll_y_px));
    (crop_h_px > 0).then_some((scroll_y_px, crop_h_px, visible_cols, visible_rows))
}

#[allow(clippy::too_many_arguments)]
fn render_non_kitty_fit_stable(
    hash: u64,
    area: Rect,
    buf: &mut Buffer,
    target_cols: u16,
    target_rows: u16,
    skip_rows: u16,
    centered: bool,
    draw_border: bool,
    picker: &Picker,
    cached: CachedDiagram,
) -> bool {
    let border_width = if draw_border { BORDER_WIDTH } else { 0 };
    let fit_cols = target_cols.saturating_sub(border_width);
    let font_size = picker.font_size();
    let source_path = cached.path;
    let Some(source) = load_fitted_source(hash, &source_path, fit_cols, target_rows, font_size)
    else {
        return false;
    };
    let (full_cols, full_rows) = cell_rect_for_image(source.as_ref(), font_size);

    let mut available = Rect {
        x: area.x + border_width,
        y: area.y,
        width: area.width - border_width,
        height: area.height,
    };
    let Some((scroll_y_px, crop_h_px, visible_cols, visible_rows)) = fitted_scroll_crop(
        source.width(),
        source.height(),
        font_size,
        available.width,
        available.height,
        skip_rows,
    ) else {
        clear_image_area(area, buf);
        return true;
    };

    if draw_border {
        draw_fitted_left_border(buf, area, skip_rows, full_rows);
    }
    if centered && full_cols < available.width {
        available.x += (available.width - full_cols) / 2;
    }
    let render_area = Rect {
        x: available.x,
        y: available.y,
        width: visible_cols,
        height: visible_rows,
    };
    let viewport = ViewportState {
        scroll_x_px: 0,
        scroll_y_px,
        view_w_px: source.width(),
        view_h_px: crop_h_px,
    };

    {
        let mut state = IMAGE_STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let needs_reset = state
            .get(&hash)
            .map(|state| {
                state.resize_mode != ResizeMode::FitViewport
                    || state.source_path.as_path() != source_path.as_path()
            })
            .unwrap_or(false);
        if needs_reset {
            state.remove(&hash);
        }
        if let Some(image_state) = state.get_mut(hash)
            && image_state.last_viewport == Some(viewport)
        {
            if let Ok(mut debug) = MERMAID_DEBUG.lock() {
                debug.stats.image_state_hits += 1;
                debug.stats.fit_state_reuse_hits += 1;
            }
            if !render_stateful_image_safe(
                hash,
                render_area,
                buf,
                &mut image_state.protocol,
                Resize::Fit(None),
            ) {
                return false;
            }
            image_state.last_area = Some(render_area);
            return true;
        }
    }

    // Non-Kitty protocols must encode a new payload when the visible slice
    // changes, but the expensive full-image decode and scale have already been
    // completed off-thread and retained in FITTED_SOURCE_CACHE.
    let cropped = source.crop_imm(0, scroll_y_px, source.width(), crop_h_px);
    let source_bytes = cropped.as_bytes().len();
    let protocol = picker.new_resize_protocol(cropped);
    if let Ok(mut debug) = MERMAID_DEBUG.lock() {
        debug.stats.image_state_misses += 1;
        debug.stats.fit_protocol_rebuilds += 1;
    }

    let mut state = IMAGE_STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.insert(
        hash,
        ImageState {
            protocol,
            source_path,
            source_bytes,
            last_area: Some(render_area),
            resize_mode: ResizeMode::FitViewport,
            last_crop_top: false,
            last_viewport: Some(viewport),
        },
    );
    let Some(image_state) = state.get_mut(hash) else {
        return false;
    };
    render_stateful_image_safe(
        hash,
        render_area,
        buf,
        &mut image_state.protocol,
        Resize::Fit(None),
    )
}

/// Render an inline raster image scaled-to-fit a fixed placeholder box, with
/// stable pixels while scrolling.
///
/// `target_cols`/`target_rows` describe the full placeholder geometry computed
/// at prepare time; `skip_rows` is how many of the image's top rows are
/// scrolled off-screen. On Kitty this reuses one transmitted image and just
/// re-addresses rows via unicode placeholders, so partial visibility never
/// rescales or retransmits. Other protocols reuse a pre-scaled source and only
/// crop/re-encode when the visible row slice changes. Returns true when handled;
/// callers should fall back to `render_image_widget_fit` on an actual failure.
#[allow(clippy::too_many_arguments)]
pub fn render_image_widget_fit_stable(
    hash: u64,
    area: Rect,
    buf: &mut Buffer,
    target_cols: u16,
    target_rows: u16,
    skip_rows: u16,
    centered: bool,
    draw_border: bool,
) -> bool {
    if VIDEO_EXPORT_MODE.load(Ordering::Relaxed) {
        return true;
    }

    let buf_area = *buf.area();
    let area = area.intersection(buf_area);
    if area.width == 0 || area.height == 0 {
        return false;
    }

    let border_width = if draw_border { BORDER_WIDTH } else { 0 };
    if area.width <= border_width {
        return false;
    }

    let picker = match PICKER.get().and_then(|p| p.as_ref()) {
        Some(picker) => picker,
        None => return false,
    };
    // Hot path: runs on the UI thread for every visible image every frame.
    // The in-memory-only lookup avoids a per-frame `path.exists()` stat; a
    // genuinely missing file degrades gracefully below when the source decode
    // fails and the caller falls back to the per-area fit renderer.
    let cached = match get_cached_diagram_in_memory(hash) {
        Some(cached) => cached,
        None => return false,
    };
    if picker.protocol_type() != ProtocolType::Kitty {
        return render_non_kitty_fit_stable(
            hash,
            area,
            buf,
            target_cols,
            target_rows,
            skip_rows,
            centered,
            draw_border,
            picker,
            cached,
        );
    }
    let source_path = cached.path;
    let font_size = picker.font_size();
    let fit_cols = target_cols.saturating_sub(border_width);

    // Hot path: steady-state frames hit the already-transmitted state without
    // ever touching (or re-decoding) the source image.
    let placement = probe_kitty_fit_state(hash, &source_path, fit_cols, target_rows, font_size)
        .or_else(|| {
            let source = load_source_image(hash, &source_path)?;
            let placement = ensure_kitty_fit_state(
                hash,
                &source_path,
                source.as_ref(),
                fit_cols,
                target_rows,
                font_size,
            );
            drop(source);
            release_source_image(hash, &source_path);
            placement
        });
    let Some((_, full_cols, full_rows)) = placement else {
        return false;
    };

    if !can_use_kitty_virtual_viewport(full_cols, full_rows, 0, skip_rows) {
        return false;
    }

    let mut image_area = Rect {
        x: area.x + border_width,
        y: area.y,
        width: area.width - border_width,
        height: area.height,
    };
    if draw_border {
        // Border hugs the fitted image: rounded ends, and no line beyond the
        // image's last row even when the placeholder estimate ran taller.
        draw_fitted_left_border(buf, area, skip_rows, full_rows);
    }
    if image_area.width == 0 || image_area.height == 0 {
        return true;
    }
    if centered && full_cols < image_area.width {
        let x_offset = (image_area.width - full_cols) / 2;
        image_area.x += x_offset;
        image_area.width -= x_offset;
    }

    let visible_width = image_area.width.min(full_cols);
    let visible_height = image_area.height.min(full_rows.saturating_sub(skip_rows));

    if let Ok(mut dbg) = MERMAID_DEBUG.lock() {
        dbg.stats.image_state_hits += 1;
        dbg.stats.fit_state_reuse_hits += 1;
    }

    render_kitty_virtual_viewport(
        hash,
        image_area,
        buf,
        0,
        skip_rows,
        visible_width,
        visible_height,
    )
}

/// Render an image by cropping a viewport (for pan/scroll in pinned pane).
pub fn render_image_widget_viewport(
    hash: u64,
    area: Rect,
    buf: &mut Buffer,
    scroll_x: i32,
    scroll_y: i32,
    zoom_percent: u8,
    draw_border: bool,
) -> u16 {
    render_image_widget_viewport_precise(
        hash,
        area,
        buf,
        scroll_x,
        scroll_y,
        zoom_percent as u16,
        draw_border,
    )
}

/// Render a cropped viewport of an image using a wider zoom range than the
/// interactive user zoom. This is used by automatic fit-fill layouts where a
/// very wide or very short diagram needs more than 200% zoom before the crop
/// has the same aspect ratio as the pane. The manual public viewport keeps the
/// historical u8/200% behavior; this path is intentionally opt-in.
pub fn render_image_widget_viewport_precise(
    hash: u64,
    area: Rect,
    buf: &mut Buffer,
    scroll_x: i32,
    scroll_y: i32,
    zoom_percent: u16,
    draw_border: bool,
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

    if image_area.width == 0 || image_area.height == 0 {
        return 0;
    }

    let picker = match PICKER.get().and_then(|p| p.as_ref()) {
        Some(picker) => picker,
        None => return 0,
    };

    let cached = match get_cached_diagram_in_memory(hash).or_else(|| get_cached_diagram(hash, None))
    {
        Some(cached) => cached,
        None => return 0,
    };
    let source_path = cached.path.clone();

    let source = match load_source_image(hash, &source_path) {
        Some(img) => img,
        None => return 0,
    };

    let font_size = picker.font_size();
    let zoom = zoom_percent.clamp(50, 1000) as u32;
    let view_w_px = (image_area.width as u32)
        .saturating_mul(font_size.0 as u32)
        .saturating_mul(100)
        / zoom;
    let view_h_px = (image_area.height as u32)
        .saturating_mul(font_size.1 as u32)
        .saturating_mul(100)
        / zoom;
    if view_w_px == 0 || view_h_px == 0 {
        return 0;
    }

    let img_width = source.width();
    let img_height = source.height();
    let max_scroll_x = img_width.saturating_sub(view_w_px);
    let max_scroll_y = img_height.saturating_sub(view_h_px);

    let cell_w_px = ((font_size.0 as u32).saturating_mul(100) / zoom).max(1);
    let cell_h_px = ((font_size.1 as u32).saturating_mul(100) / zoom).max(1);
    let scroll_x_px = (scroll_x.max(0) as u32)
        .saturating_mul(cell_w_px)
        .min(max_scroll_x);
    let scroll_y_px = (scroll_y.max(0) as u32)
        .saturating_mul(cell_h_px)
        .min(max_scroll_y);

    let crop_w = view_w_px.min(img_width.saturating_sub(scroll_x_px));
    let crop_h = view_h_px.min(img_height.saturating_sub(scroll_y_px));
    if crop_w == 0 || crop_h == 0 {
        return 0;
    }
    let viewport_resize = || {
        if viewport_crop_should_scale_to_area(crop_w, crop_h, view_w_px, view_h_px) {
            // A viewport crop is intentionally smaller than the destination
            // cell area when zoomed in. Scale it back up to the destination,
            // otherwise Resize::Fit leaves the crop at source pixel size and
            // the pane visually stays tiny despite a fit-fill plan.
            Resize::Scale(None)
        } else {
            // If the requested viewport is larger than the source image on an
            // axis, preserve aspect ratio instead of stretching the full image.
            Resize::Fit(None)
        }
    };

    let viewport = ViewportState {
        scroll_x_px,
        scroll_y_px,
        view_w_px,
        view_h_px,
    };

    if zoom_percent <= 200
        && picker.protocol_type() == ProtocolType::Kitty
        && let Some((_, full_cols, full_rows)) = ensure_kitty_viewport_state(
            hash,
            &source_path,
            source.as_ref(),
            zoom_percent as u8,
            font_size,
        )
    {
        let scroll_x_cells = (scroll_x.max(0) as u16).min(full_cols.saturating_sub(1));
        let scroll_y_cells = (scroll_y.max(0) as u16).min(full_rows.saturating_sub(1));
        if can_use_kitty_virtual_viewport(full_cols, full_rows, scroll_x_cells, scroll_y_cells) {
            let visible_width = image_area
                .width
                .min(full_cols.saturating_sub(scroll_x_cells));
            let visible_height = image_area
                .height
                .min(full_rows.saturating_sub(scroll_y_cells));
            if let Ok(mut state) = IMAGE_STATE.lock()
                && let Some(img_state) = state.get_mut(hash)
            {
                img_state.last_area = Some(image_area);
                img_state.last_viewport = Some(viewport);
            }
            if render_kitty_virtual_viewport(
                hash,
                image_area,
                buf,
                scroll_x_cells,
                scroll_y_cells,
                visible_width,
                visible_height,
            ) {
                return area.height;
            }
        }
    }

    {
        let mut state = IMAGE_STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let needs_reset = state
            .get(&hash)
            .map(|s| {
                s.resize_mode != ResizeMode::Viewport
                    || s.source_path.as_path() != source_path.as_path()
            })
            .unwrap_or(false);
        if needs_reset {
            state.remove(&hash);
        }
        if let Some(img_state) = state.get_mut(hash)
            && img_state.last_viewport == Some(viewport)
        {
            if let Ok(mut dbg) = MERMAID_DEBUG.lock() {
                dbg.stats.viewport_state_reuse_hits += 1;
            }
            if !render_stateful_image_safe(
                hash,
                image_area,
                buf,
                &mut img_state.protocol,
                viewport_resize(),
            ) {
                return 0;
            }
            img_state.last_area = Some(image_area);
            return area.height;
        }
    }

    let cropped = source.crop_imm(scroll_x_px, scroll_y_px, crop_w, crop_h);
    if let Ok(mut dbg) = MERMAID_DEBUG.lock() {
        dbg.stats.viewport_protocol_rebuilds += 1;
    }
    let source_bytes = cropped.as_bytes().len();
    let protocol = picker.new_resize_protocol(cropped);

    let mut state = IMAGE_STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.insert(
        hash,
        ImageState {
            protocol,
            source_path,
            source_bytes,
            last_area: Some(image_area),
            resize_mode: ResizeMode::Viewport,
            last_crop_top: false,
            last_viewport: Some(viewport),
        },
    );

    if let Some(img_state) = state.get_mut(hash) {
        if !render_stateful_image_safe(
            hash,
            image_area,
            buf,
            &mut img_state.protocol,
            viewport_resize(),
        ) {
            return 0;
        }
        return area.height;
    }

    0
}

/// Clear an area that previously had an image (removes stale terminal graphics)
/// This is called when an image's marker scrolls off-screen but its area still overlaps
/// the visible region - we need to explicitly clear the terminal graphics layer.
pub(super) fn clear_image_area(area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let clamped = area.intersection(*buf.area());
    if clamped.width == 0 || clamped.height == 0 {
        return;
    }
    if let Ok(mut dbg) = MERMAID_DEBUG.lock() {
        dbg.stats.clear_operations += 1;
    }
    jcode_tui_workspace::color_support::clear_buf(clamped, buf);
}

/// Invalidate last render state for a hash (call when content changes)
pub fn invalidate_render_state(hash: u64) {
    if let Ok(mut last_render) = LAST_RENDER.lock() {
        last_render.remove(&hash);
    }
}

#[cfg(test)]
mod kitty_viewport_leak_tests {
    //! Confirming test for the "image renders above its tag line" report.
    //!
    //! Static analysis of the draw path (`ui_viewport.rs`) showed both image
    //! branches keep Kitty placeholder cells strictly inside the image region,
    //! which sits one wrapped line *below* the label/tag. The lowest-level
    //! emitter is [`render_kitty_virtual_viewport`]: it is the only place that
    //! writes the `U+10EEEE` placeholder char into buffer cells. If the in-buffer
    //! geometry were the culprit, this emitter would have to paint a placeholder
    //! on a row above its `area`. This test drives it directly across a sweep of
    //! `scroll_y`/partial-visibility values and asserts that never happens, which
    //! pins the real defect to terminal-side ghosting rather than buffer layout.

    use super::*;

    const PLACEHOLDER: char = '\u{10EEEE}';

    #[test]
    fn virtual_transmit_uses_compressed_png_payload() {
        let image = DynamicImage::ImageRgba8(image::ImageBuffer::from_pixel(
            256,
            256,
            image::Rgba([245, 245, 245, 255]),
        ));
        let transmit = kitty_transmit_virtual(&image, 7);
        let raw_base64_bytes = (256usize * 256 * 4).div_ceil(3) * 4;

        assert!(transmit.contains("f=100"));
        assert!(!transmit.contains("f=32"));
        assert!(
            transmit.len() < raw_base64_bytes / 10,
            "PNG transmit should be far smaller than raw RGBA: {} vs {} bytes",
            transmit.len(),
            raw_base64_bytes,
        );
    }

    #[test]
    fn kitty_ids_are_process_unique_and_delete_payload_targets_one_image() {
        let first = kitty_viewport_unique_id(0x0000_0001_FFFF_FFFF);
        let second = kitty_viewport_unique_id(0xFFFF_FFFF_0000_0001);
        assert_ne!(
            first, second,
            "64-bit hash folds must never alias Kitty ids"
        );

        let delete = kitty_delete_image_payload(first);
        assert!(delete.contains("a=d,d=I"));
        assert!(delete.contains(&format!("i={first}")));
    }

    #[test]
    fn kitty_cache_eviction_queues_terminal_image_deletion() {
        let _guard = crate::IMAGE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = take_kitty_delete_ids();
        let mut cache = KittyViewportCache::new();
        for index in 0..KITTY_VIEWPORT_STATE_MAX {
            cache.insert(
                index as u64,
                KittyViewportState {
                    source_path: PathBuf::from(format!("/test/evict-{index}.png")),
                    zoom_percent: 100,
                    font_size: (8, 16),
                    unique_id: 10_000 + index as u32,
                    full_cols: 10,
                    full_rows: 10,
                    pending_transmit: None,
                    pending_transmit_bytes: 0,
                    fit_target: Some((10, 10)),
                },
            );
        }
        cache.get_mut(0).expect("touch oldest state");
        let index = KITTY_VIEWPORT_STATE_MAX;
        cache.insert(
            index as u64,
            KittyViewportState {
                source_path: PathBuf::from(format!("/test/evict-{index}.png")),
                zoom_percent: 100,
                font_size: (8, 16),
                unique_id: 10_000 + index as u32,
                full_cols: 10,
                full_rows: 10,
                pending_transmit: None,
                pending_transmit_bytes: 0,
                fit_target: Some((10, 10)),
            },
        );
        assert_eq!(cache.entries.len(), KITTY_VIEWPORT_STATE_MAX);
        assert!(
            cache.entries.contains_key(&0),
            "recently touched state stays hot"
        );
        assert!(
            !cache.entries.contains_key(&1),
            "least-recent state is evicted"
        );
        assert_eq!(take_kitty_delete_ids(), vec![10_001]);
    }

    #[test]
    fn kitty_cache_bounds_not_yet_drawn_transmissions_by_bytes() {
        let _guard = crate::IMAGE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = take_kitty_delete_ids();
        let mut cache = KittyViewportCache::new();
        let accounted = KITTY_VIEWPORT_PENDING_MAX_BYTES / 2 + 1;
        for index in 0..3u64 {
            cache.insert(
                index,
                KittyViewportState {
                    source_path: PathBuf::from(format!("/test/pending-{index}.png")),
                    zoom_percent: 100,
                    font_size: (8, 16),
                    unique_id: 20_000 + index as u32,
                    full_cols: 10,
                    full_rows: 10,
                    pending_transmit: Some(String::from("synthetic")),
                    pending_transmit_bytes: accounted,
                    fit_target: Some((10, 10)),
                },
            );
        }
        assert_eq!(
            cache.entries.len(),
            1,
            "byte budget should evict old prewarms"
        );
        assert_eq!(cache.total_pending_transmit_bytes, accounted);
        assert!(
            take_kitty_delete_ids().is_empty(),
            "never-drawn prewarms have no terminal allocation to delete"
        );

        let (_, pending) = cache.take_pending_transmit(2).expect("newest state");
        assert_eq!(pending.as_deref(), Some("synthetic"));
        assert_eq!(cache.total_pending_transmit_bytes, 0);
    }

    #[test]
    fn pending_terminal_cleanup_renders_without_changing_visible_cell_text() {
        let _guard = crate::IMAGE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = take_kitty_delete_ids();
        queue_kitty_delete(42_424);
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        buf.cell_mut((0, 0)).expect("fixture cell").set_symbol("X");

        assert!(crate::render_pending_terminal_image_cleanup(&mut buf));
        let symbol = buf.cell((0, 0)).expect("cleanup cell").symbol();
        assert!(symbol.contains("a=d,d=I,i=42424"));
        assert!(symbol.ends_with('X'), "visible symbol must be preserved");
        assert!(take_kitty_delete_ids().is_empty());
    }

    #[test]
    fn unchanged_png_transmit_reuses_exact_file_bytes() {
        use image::ImageEncoder as _;

        let pixels = image::RgbaImage::from_pixel(7, 5, image::Rgba([9, 71, 203, 255]));
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(pixels.as_raw(), 7, 5, image::ExtendedColorType::Rgba8)
            .expect("encode fixture");
        let path = std::env::temp_dir().join(format!(
            "jcode-kitty-source-{}-{}.png",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(&path, &png).expect("write fixture");

        let actual = kitty_transmit_png_path(&path, 19).expect("direct PNG transmit");
        let expected = kitty_transmit_payload(&png, 19, "f=100");
        assert_eq!(actual, expected, "source PNG bytes must not be re-encoded");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn halfblocks_partial_scroll_reuses_fitted_source_and_identical_slice() {
        let _guard = crate::IMAGE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        const HASH: u64 = 0x4841_4C46_424C_4B53;
        let path = PathBuf::from("/test/halfblocks-scroll.png");
        let source = DynamicImage::ImageRgba8(image::ImageBuffer::from_pixel(
            160,
            128,
            image::Rgba([31, 127, 223, 255]),
        ));
        SOURCE_CACHE
            .lock()
            .unwrap()
            .insert(HASH, path.clone(), source);
        FITTED_SOURCE_CACHE.lock().unwrap().remove_hash(HASH);
        IMAGE_STATE.lock().unwrap().remove(&HASH);

        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize((2, 4));
        picker.set_protocol_type(ProtocolType::Halfblocks);
        let cached = CachedDiagram {
            path: path.clone(),
            width: 160,
            height: 128,
        };
        let area = Rect::new(0, 0, 42, 5);
        let mut buf = Buffer::empty(Rect::new(0, 0, 42, 16));
        let stats_before = debug_stats();

        assert!(render_non_kitty_fit_stable(
            HASH,
            area,
            &mut buf,
            42,
            16,
            4,
            false,
            true,
            &picker,
            cached.clone(),
        ));
        let first_stats = debug_stats();
        assert_eq!(
            first_stats
                .fit_protocol_rebuilds
                .saturating_sub(stats_before.fit_protocol_rebuilds),
            1,
            "the first visible slice should build one protocol"
        );
        {
            let state = IMAGE_STATE.lock().unwrap();
            let state = state.get(&HASH).expect("halfblocks viewport state");
            assert_eq!(state.resize_mode, ResizeMode::FitViewport);
            assert_eq!(
                state.last_viewport,
                Some(ViewportState {
                    scroll_x_px: 0,
                    scroll_y_px: 16,
                    view_w_px: 80,
                    view_h_px: 20,
                })
            );
        }
        assert!(
            SOURCE_CACHE.lock().unwrap().entries.get(&HASH).is_none(),
            "full decoded original should be released after fitting"
        );
        assert_eq!(
            FITTED_SOURCE_CACHE
                .lock()
                .unwrap()
                .entries
                .keys()
                .filter(|key| key.hash == HASH)
                .count(),
            1
        );

        // Rendering the identical scroll slice must reuse protocol state rather
        // than recropping/re-encoding it.
        assert!(render_non_kitty_fit_stable(
            HASH, area, &mut buf, 42, 16, 4, false, true, &picker, cached,
        ));
        let second_stats = debug_stats();
        assert_eq!(
            second_stats
                .fit_protocol_rebuilds
                .saturating_sub(first_stats.fit_protocol_rebuilds),
            0
        );
        assert_eq!(
            second_stats
                .fit_state_reuse_hits
                .saturating_sub(first_stats.fit_state_reuse_hits),
            1
        );

        IMAGE_STATE.lock().unwrap().remove(&HASH);
        SOURCE_CACHE.lock().unwrap().remove(HASH);
        FITTED_SOURCE_CACHE.lock().unwrap().remove_hash(HASH);
    }

    /// Seed `KITTY_VIEWPORT_STATE` with a fit entry so the emitter has an id to
    /// address without needing a real terminal/transmit.
    fn seed_state(hash: u64, full_cols: u16, full_rows: u16) {
        let mut cache = KITTY_VIEWPORT_STATE.lock().unwrap();
        cache.insert(
            hash,
            KittyViewportState {
                source_path: std::path::PathBuf::from("/test/leak.png"),
                zoom_percent: 100,
                font_size: (8, 16),
                unique_id: 0x00AABBCC,
                full_cols,
                full_rows,
                pending_transmit: Some(String::from("\x1b_Gtransmit\x1b\\")),
                pending_transmit_bytes: "\x1b_Gtransmit\x1b\\".len(),
                fit_target: Some((full_cols, full_rows)),
            },
        );
    }

    /// True if any cell at row `y` carries a Kitty placeholder char.
    fn row_has_placeholder(buf: &Buffer, y: u16) -> bool {
        let area = *buf.area();
        (area.left()..area.right()).any(|x| {
            buf.cell((x, y))
                .is_some_and(|c| c.symbol().contains(PLACEHOLDER))
        })
    }

    /// The emitter must never paint placeholders above its own `area`, for any
    /// scroll position or partial-visibility height. The buffer has sentinel
    /// rows above the image area that stand in for the label/tag line.
    #[test]
    fn placeholders_never_leak_above_image_area() {
        let _guard = crate::IMAGE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let hash = 0xDEAD_BEEF_u64;
        let full_cols = 20;
        let full_rows = 30;
        seed_state(hash, full_cols, full_rows);

        // Buffer taller/wider than the image area, with several "tag" rows on top.
        let buf_w = full_cols + 4;
        let buf_h = full_rows + 8;
        let label_rows = 3u16; // rows 0..3 stand in for blank + label + spacer

        // Sweep: how many of the image's top rows are scrolled off (skip_rows),
        // which also drives the visible height that the draw path would request.
        for skip_rows in 0..full_rows {
            // Re-seed each iteration: render consumes pending_transmit.
            seed_state(hash, full_cols, full_rows);
            let visible_height = full_rows - skip_rows;

            let mut buf = Buffer::empty(Rect::new(0, 0, buf_w, buf_h));
            // Mark the label/tag region with a sentinel so any overwrite is loud.
            for y in 0..label_rows {
                for x in 0..buf_w {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_symbol("T");
                    }
                }
            }

            let image_area = Rect {
                x: 1,
                y: label_rows,
                width: full_cols,
                height: visible_height,
            };

            let ok = render_kitty_virtual_viewport(
                hash,
                image_area,
                &mut buf,
                0,
                skip_rows,
                full_cols.min(image_area.width),
                visible_height,
            );
            assert!(ok, "viewport render failed for skip_rows={skip_rows}");

            // No placeholder may sit on any row above the image area.
            for y in 0..label_rows {
                assert!(
                    !row_has_placeholder(&buf, y),
                    "placeholder leaked onto tag row {y} (skip_rows={skip_rows})"
                );
            }
            // The label/tag sentinel cells must be untouched.
            for y in 0..label_rows {
                for x in 0..buf_w {
                    let sym = buf.cell((x, y)).map(|c| c.symbol().to_string());
                    assert_eq!(
                        sym.as_deref(),
                        Some("T"),
                        "tag cell ({x},{y}) overwritten (skip_rows={skip_rows})"
                    );
                }
            }
            // Sanity: the first image row should actually carry a placeholder, so
            // the test is exercising the real emission and not a no-op.
            assert!(
                row_has_placeholder(&buf, image_area.y),
                "expected placeholders on first image row (skip_rows={skip_rows})"
            );
        }
    }
}
