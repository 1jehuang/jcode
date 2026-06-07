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

pub(super) fn viewport_crop_should_scale_to_area(
    crop_w: u32,
    crop_h: u32,
    view_w_px: u32,
    view_h_px: u32,
) -> bool {
    crop_w == view_w_px && crop_h == view_h_px
}

fn kitty_viewport_unique_id(hash: u64) -> u32 {
    let mixed = (hash as u32) ^ ((hash >> 32) as u32) ^ 0x4B49_5459;
    mixed.max(1)
}

fn kitty_is_tmux() -> bool {
    std::env::var("TERM").is_ok_and(|term| term.starts_with("tmux"))
        || std::env::var("TERM_PROGRAM").is_ok_and(|term_program| term_program == "tmux")
}

fn kitty_transmit_virtual(img: &DynamicImage, id: u32) -> String {
    let (w, h) = (img.width(), img.height());
    let img_rgba8 = img.to_rgba8();
    let bytes = img_rgba8.as_raw();

    let (start, escape, end) = Parser::escape_tmux(kitty_is_tmux());
    let mut data = String::from(start);

    let chunks = bytes.chunks(4096 / 4 * 3);
    let chunk_count = chunks.len();
    for (i, chunk) in chunks.enumerate() {
        let payload = base64::engine::general_purpose::STANDARD.encode(chunk);
        data.push_str(escape);

        match i {
            0 => {
                let more = if chunk_count > 1 { 1 } else { 0 };
                data.push_str(&format!(
                    "_Gq=2,i={id},a=T,U=1,f=32,t=d,s={w},v={h},m={more};{payload}"
                ));
            }
            n if n + 1 == chunk_count => {
                data.push_str(&format!("_Gq=2,m=0;{payload}"));
            }
            _ => {
                data.push_str(&format!("_Gq=2,m=1;{payload}"));
            }
        }
        data.push_str(escape);
        data.push('\\');
    }
    data.push_str(end);

    data
}

fn kitty_scaled_image_for_zoom(source: &DynamicImage, zoom_percent: u8) -> DynamicImage {
    use image::imageops::FilterType;

    let zoom = zoom_percent.clamp(50, 200) as u32;
    if zoom == 100 {
        return source.clone();
    }

    let scaled_w = ((source.width() as u64).saturating_mul(zoom as u64) / 100)
        .max(1)
        .min(u32::MAX as u64) as u32;
    let scaled_h = ((source.height() as u64).saturating_mul(zoom as u64) / 100)
        .max(1)
        .min(u32::MAX as u64) as u32;
    source.resize_exact(scaled_w, scaled_h, FilterType::Nearest)
}

fn div_ceil_u32_local(value: u32, divisor: u32) -> u32 {
    value
        .saturating_add(divisor.saturating_sub(1))
        .checked_div(divisor)
        .unwrap_or(value)
}

fn kitty_full_rect_for_image(img: &DynamicImage, font_size: (u16, u16)) -> (u16, u16) {
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
    {
        return Some((state.unique_id, state.full_cols, state.full_rows));
    }

    let scaled = kitty_scaled_image_for_zoom(source, zoom_percent);
    let (full_cols, full_rows) = kitty_full_rect_for_image(&scaled, font_size);
    if full_cols == 0 || full_rows == 0 {
        return None;
    }

    let unique_id = cache
        .get_mut(hash)
        .map(|state| state.unique_id)
        .unwrap_or_else(|| kitty_viewport_unique_id(hash));

    cache.insert(
        hash,
        KittyViewportState {
            source_path: source_path.to_path_buf(),
            zoom_percent,
            font_size,
            unique_id,
            full_cols,
            full_rows,
            pending_transmit: Some(kitty_transmit_virtual(&scaled, unique_id)),
        },
    );

    if let Ok(mut dbg) = MERMAID_DEBUG.lock() {
        dbg.stats.viewport_protocol_rebuilds += 1;
    }

    cache
        .get_mut(hash)
        .map(|state| (state.unique_id, state.full_cols, state.full_rows))
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
    let Some(state) = cache.get_mut(hash) else {
        return false;
    };
    let unique_id = state.unique_id;
    let pending_transmit = state.pending_transmit.take();
    drop(cache);

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
            pending_transmit.clone().unwrap_or_default()
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

    let cached = match get_cached_diagram(hash, None) {
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
    let protocol = picker.new_resize_protocol(cropped);

    let mut state = IMAGE_STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.insert(
        hash,
        ImageState {
            protocol,
            source_path,
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
