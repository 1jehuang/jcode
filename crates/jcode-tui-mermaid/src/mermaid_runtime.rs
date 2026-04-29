use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PickerInitMode {
    Fast,
    Probe,
}

fn parse_env_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

pub(super) fn picker_init_mode_from_probe_env(raw: Option<&str>) -> PickerInitMode {
    if let Some(raw) = raw
        && parse_env_bool(raw) == Some(true)
    {
        return PickerInitMode::Probe;
    }
    PickerInitMode::Fast
}

fn picker_init_mode_from_env() -> PickerInitMode {
    picker_init_mode_from_probe_env(std::env::var("JCODE_MERMAID_PICKER_PROBE").ok().as_deref())
}

pub(super) fn infer_protocol_from_env(
    term: Option<&str>,
    term_program: Option<&str>,
    lc_terminal: Option<&str>,
    kitty_window_id: Option<&str>,
) -> Option<ProtocolType> {
    if kitty_window_id.is_some() {
        return Some(ProtocolType::Kitty);
    }

    let term = term.unwrap_or("").to_ascii_lowercase();
    let term_program = term_program.unwrap_or("").to_ascii_lowercase();
    let lc_terminal = lc_terminal.unwrap_or("").to_ascii_lowercase();

    if term.contains("kitty")
        || term_program.contains("kitty")
        || term_program.contains("wezterm")
        || term_program.contains("ghostty")
    {
        return Some(ProtocolType::Kitty);
    }

    if term_program.contains("iterm")
        || term.contains("iterm")
        || lc_terminal.contains("iterm")
        || lc_terminal.contains("wezterm")
    {
        return Some(ProtocolType::Iterm2);
    }

    if term.contains("sixel") {
        return Some(ProtocolType::Sixel);
    }

    None
}

fn query_font_size() -> (u16, u16) {
    match crossterm::terminal::window_size() {
        Ok(ws) if ws.columns > 0 && ws.rows > 0 && ws.width > 0 && ws.height > 0 => {
            let fw = ws.width / ws.columns;
            let fh = ws.height / ws.rows;
            if fw > 0 && fh > 0 {
                crate::log_info(&format!(
                    "Detected terminal font size: {}x{} pixels/cell (window {}x{} px, {}x{} cells)",
                    fw, fh, ws.width, ws.height, ws.columns, ws.rows
                ));
                (fw, fh)
            } else {
                DEFAULT_PICKER_FONT_SIZE
            }
        }
        _ => DEFAULT_PICKER_FONT_SIZE,
    }
}

fn fast_picker() -> Picker {
    let _font_size = query_font_size();
    let mut picker = Picker::halfblocks();
    if let Some(protocol) = infer_protocol_from_env(
        std::env::var("TERM").ok().as_deref(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
        std::env::var("LC_TERMINAL").ok().as_deref(),
        std::env::var("KITTY_WINDOW_ID").ok().as_deref(),
    ) {
        picker.set_protocol_type(protocol);
    }
    picker
}

fn prewarm_svg_font_db_async() {
    SVG_FONT_DB_PREWARM_STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("jcode-mermaid-fontdb-prewarm".to_string())
            .spawn(|| {
                let _ = &*SVG_FONT_DB;
            });
    });
}

/// Initialize the global picker.
/// By default this uses a fast non-blocking path and avoids terminal probing.
/// Set JCODE_MERMAID_PICKER_PROBE=1 to force full stdio capability probing.
/// Also triggers cache eviction on first call.
pub fn init_picker() {
    PICKER.get_or_init(|| match picker_init_mode_from_env() {
        PickerInitMode::Fast => Some(fast_picker()),
        PickerInitMode::Probe => match Picker::from_query_stdio() {
            Ok(picker) => Some(picker),
            Err(err) => {
                crate::log_warn(&format!(
                    "Mermaid picker probe failed ({}); using fast picker fallback",
                    err
                ));
                Some(fast_picker())
            }
        },
    });
    prewarm_svg_font_db_async();
    // Evict old cache files once per process
    CACHE_EVICTED.get_or_init(|| {
        evict_old_cache();
    });
}

/// Get the current protocol type (for debugging/display)
pub fn protocol_type() -> Option<ProtocolType> {
    let real = PICKER
        .get()
        .and_then(|p| p.as_ref().map(|picker| picker.protocol_type()));
    if real.is_some() {
        return real;
    }
    if VIDEO_EXPORT_MODE.load(Ordering::Relaxed) {
        Some(ProtocolType::Halfblocks)
    } else {
        None
    }
}

pub fn image_protocol_available() -> bool {
    PICKER.get().and_then(|p| p.as_ref()).is_some() || VIDEO_EXPORT_MODE.load(Ordering::Relaxed)
}

/// Enable video-export mode: mermaid images produce hash-placeholder lines
/// even without a real terminal image protocol.
pub fn set_video_export_mode(enabled: bool) {
    VIDEO_EXPORT_MODE.store(enabled, Ordering::Relaxed);
}

/// Check if video export mode is active.
pub fn is_video_export_mode() -> bool {
    VIDEO_EXPORT_MODE.load(Ordering::Relaxed)
}

/// Look up a cached PNG for the given mermaid content hash.
/// Returns (path, width, height) if a cached render exists on disk.
pub fn get_cached_png(hash: u64) -> Option<(PathBuf, u32, u32)> {
    let mut cache = RENDER_CACHE.lock().ok()?;
    let diagram = cache.get(hash, None)?;
    Some((diagram.path, diagram.width, diagram.height))
}

/// Register an external image file (e.g. from file_read) in the render cache
/// so it can be displayed with render_image_widget_fit/render_image_widget.
/// Returns the hash used for rendering.
pub fn register_external_image(path: &Path, width: u32, height: u32) -> u64 {
    use std::hash::{Hash as _, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    path.hash(&mut hasher);
    let hash = hasher.finish();

    if let Ok(mut cache) = RENDER_CACHE.lock() {
        cache.insert(
            hash,
            CachedDiagram {
                path: path.to_path_buf(),
                width,
                height,
            },
        );
    }
    hash
}

pub fn register_inline_image(media_type: &str, data_b64: &str) -> Option<(u64, u32, u32)> {
    use std::hash::{Hash as _, Hasher};

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_b64)
        .ok()?;

    let mut hasher = std::hash::DefaultHasher::new();
    media_type.hash(&mut hasher);
    bytes.hash(&mut hasher);
    let hash = hasher.finish();

    if let Ok(mut cache) = RENDER_CACHE.lock() {
        if let Some(existing) = cache.get(hash, None) {
            return Some((hash, existing.width, existing.height));
        }

        let image = image::load_from_memory(&bytes).ok()?;
        let (width, height) = image.dimensions();
        let ext = inline_image_extension(media_type);
        let path = cache
            .cache_dir
            .join(format!("{:016x}_inline.{}", hash, ext));
        if !path.exists() {
            fs::write(&path, &bytes).ok()?;
        }
        cache.insert(
            hash,
            CachedDiagram {
                path,
                width,
                height,
            },
        );
        return Some((hash, width, height));
    }

    None
}

fn inline_image_extension(media_type: &str) -> &'static str {
    match media_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/x-icon" | "image/vnd.microsoft.icon" => "ico",
        _ => "img",
    }
}

pub fn error_lines_for(hash: u64) -> Option<Vec<Line<'static>>> {
    let message = RENDER_ERRORS
        .lock()
        .ok()
        .and_then(|errors| errors.get(&hash).cloned());
    message.map(|msg| error_to_lines(&msg))
}

/// Get terminal font size for adaptive sizing
pub fn get_font_size() -> Option<(u16, u16)> {
    PICKER
        .get()
        .and_then(|p| p.as_ref().map(|picker| picker.font_size()))
}
