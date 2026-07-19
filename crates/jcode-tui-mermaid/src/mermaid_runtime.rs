use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PickerInitMode {
    Fast,
    Probe,
}

/// Terminal multiplexers / agent-multiplexers that sit between jcode and the
/// real outer terminal. Inside any of these the outer terminal's identity env
/// vars (TERM_PROGRAM, KITTY_WINDOW_ID, ...) are masked or rewritten, so
/// env-based protocol detection cannot see whether the outer terminal supports
/// kitty/sixel graphics. The only reliable signal in that case is an
/// authoritative stdio capability probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Multiplexer {
    None,
    Tmux,
    Screen,
    Zellij,
    /// herdr.dev agent multiplexer. Advertises `TERM=xterm-256color` and sets
    /// `HERDR_ENV=1` in every pane, hiding the outer terminal. Recent versions
    /// can pass kitty graphics through to a capable outer terminal.
    Herdr,
}

impl Multiplexer {
    fn label(self) -> &'static str {
        match self {
            Multiplexer::None => "none",
            Multiplexer::Tmux => "tmux",
            Multiplexer::Screen => "screen",
            Multiplexer::Zellij => "zellij",
            Multiplexer::Herdr => "herdr",
        }
    }
}

fn env_is_set(value: Option<&str>) -> bool {
    value.map(|v| !v.trim().is_empty()).unwrap_or(false)
}

/// Detect whether jcode is running inside a known multiplexer, using the same
/// signals the multiplexers themselves expose to child processes.
pub(super) fn detect_multiplexer(
    term: Option<&str>,
    tmux: Option<&str>,
    sty: Option<&str>,
    zellij: Option<&str>,
    herdr_env: Option<&str>,
) -> Multiplexer {
    // Herdr wins first: it rewrites TERM to a bland value but always exports
    // HERDR_ENV=1, so it is the most specific signal.
    if env_is_set(herdr_env) {
        return Multiplexer::Herdr;
    }
    if env_is_set(zellij) {
        return Multiplexer::Zellij;
    }
    if env_is_set(tmux) {
        return Multiplexer::Tmux;
    }
    let term = term.unwrap_or("");
    if env_is_set(sty) || term.starts_with("screen") {
        return Multiplexer::Screen;
    }
    if term.starts_with("tmux") {
        return Multiplexer::Tmux;
    }
    Multiplexer::None
}

fn detect_multiplexer_from_env() -> Multiplexer {
    detect_multiplexer(
        std::env::var("TERM").ok().as_deref(),
        std::env::var("TMUX").ok().as_deref(),
        std::env::var("STY").ok().as_deref(),
        std::env::var("ZELLIJ").ok().as_deref(),
        std::env::var("HERDR_ENV").ok().as_deref(),
    )
}

fn parse_env_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Decide how to initialize the picker.
///
/// * `probe_override` is the parsed value of `JCODE_MERMAID_PICKER_PROBE`
///   (`Some(true)`/`Some(false)` when set explicitly, `None` otherwise) and
///   always wins so users can force either behavior.
/// * When the override is absent, startup stays on the environment-based fast
///   path. The upstream stdio probe can block for two seconds when a terminal
///   does not answer, which made an optional image capability dominate the TUI
///   critical path. Users behind multiplexers that hide the outer terminal can
///   still opt into the authoritative probe with
///   `JCODE_MERMAID_PICKER_PROBE=1`.
pub(super) fn decide_picker_init_mode(
    probe_override: Option<bool>,
    _env_protocol: Option<ProtocolType>,
    _multiplexer: Multiplexer,
) -> PickerInitMode {
    if probe_override == Some(true) {
        PickerInitMode::Probe
    } else {
        PickerInitMode::Fast
    }
}

/// Parse only the explicit `JCODE_MERMAID_PICKER_PROBE` override into a mode,
/// ignoring env/multiplexer detection. `Some(true)` probes; unset or any other
/// value keeps the historical fast default. Used for the force-on/off path and
/// as a focused unit-test seam.
#[cfg(test)]
pub(super) fn picker_init_mode_from_probe_env(raw: Option<&str>) -> PickerInitMode {
    match raw.and_then(parse_env_bool) {
        Some(true) => PickerInitMode::Probe,
        _ => PickerInitMode::Fast,
    }
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
        || term_program.contains("handterm")
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
    // Use the real cell size from the terminal. Building the picker with the
    // hardcoded halfblocks default (10x20) makes every cell<->pixel conversion
    // wrong on HiDPI displays: images get scaled for 20px rows while Kitty
    // renders them at the true row height, so placeholders/borders end up much
    // taller than the picture.
    let font_size = query_font_size();
    // `from_fontsize` is deprecated upstream in favor of `from_query_stdio` /
    // `halfblocks`, but neither lets us inject an already-measured cell size:
    // `halfblocks` hardcodes 10x20 and there is no public font-size setter. We
    // deliberately keep it to stay correct on HiDPI terminals.
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(font_size);
    picker.set_protocol_type(ProtocolType::Halfblocks);
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

/// Build a picker via the authoritative stdio capability probe.
///
/// The probe reports the protocol the *outer* terminal actually supports, which
/// is the only way to get crisp graphics inside a multiplexer that masks the
/// outer terminal's identity. ratatui-image has no public font-size setter, and
/// its `from_query_stdio` uses a placeholder 10x20 cell when the terminal does
/// not answer the cell-size query, which breaks HiDPI scaling. So we keep our
/// own font-size-correct `fast_picker()` as the base and only adopt the
/// probe's *protocol* decision. Any probe failure leaves the env-based fast
/// picker untouched.
fn probe_picker() -> Picker {
    let mut picker = fast_picker();
    match Picker::from_query_stdio() {
        Ok(probed) => {
            let protocol = probed.protocol_type();
            crate::log_info(&format!(
                "Mermaid picker stdio probe detected protocol: {:?}",
                protocol
            ));
            picker.set_protocol_type(protocol);
        }
        Err(err) => {
            crate::log_warn(&format!(
                "Mermaid picker probe failed ({}); using fast picker fallback",
                err
            ));
        }
    }
    picker
}

/// Start loading the system font database on a background thread.
///
/// Called lazily from [`crate::is_mermaid_lang`] the first time mermaid
/// content is actually detected, NOT at startup: loading the font DB costs
/// tens of milliseconds of CPU and most sessions never render a diagram, so
/// prewarming on every spawn made the font load one of the larger fixed costs
/// of launching a client. Detection happens while markdown is still
/// streaming/rendering, so the DB is warm (or loading concurrently) by the
/// time the first real diagram render needs it; if the render wins the race it
/// just blocks on the same `LazyLock`.
pub(crate) fn prewarm_svg_font_db_async() {
    SVG_FONT_DB_PREWARM_STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("jcode-mermaid-fontdb-prewarm".to_string())
            .spawn(|| {
                let _ = &*SVG_FONT_DB;
            });
    });
}

/// Initialize the global picker.
/// By default jcode uses environment-based detection and never blocks startup
/// on terminal capability responses. Set JCODE_MERMAID_PICKER_PROBE=1 to run an
/// authoritative stdio probe when a multiplexer masks the outer terminal, or
/// =0 to explicitly retain the fast path. Cache eviction runs once before image
/// rendering can begin so it cannot delete a file between materialization and
/// in-memory cache registration.
pub fn init_picker() {
    PICKER.get_or_init(|| {
        let env_protocol = infer_protocol_from_env(
            std::env::var("TERM").ok().as_deref(),
            std::env::var("TERM_PROGRAM").ok().as_deref(),
            std::env::var("LC_TERMINAL").ok().as_deref(),
            std::env::var("KITTY_WINDOW_ID").ok().as_deref(),
        );
        let multiplexer = detect_multiplexer_from_env();
        let probe_override = std::env::var("JCODE_MERMAID_PICKER_PROBE")
            .ok()
            .as_deref()
            .and_then(parse_env_bool);
        let mode = decide_picker_init_mode(probe_override, env_protocol, multiplexer);
        crate::log_info(&format!(
            "Mermaid picker init: mode={:?} multiplexer={} env_protocol={:?} probe_override={:?}",
            mode,
            multiplexer.label(),
            env_protocol,
            probe_override
        ));
        match mode {
            PickerInitMode::Fast => Some(fast_picker()),
            PickerInitMode::Probe => Some(probe_picker()),
        }
    });
    // Note: the SVG font-DB prewarm is intentionally NOT triggered here.
    // init_picker() runs on every TUI startup, and the font load is only
    // needed if a mermaid diagram is actually rendered; see
    // prewarm_svg_font_db_async() for the lazy trigger.
    // This is intentionally synchronous. An asynchronous pass can observe an
    // image file after it is written but before it is registered in RENDER_CACHE,
    // delete it, and leave stale in-memory metadata that suppresses recovery.
    CACHE_EVICTED.get_or_init(evict_old_cache);
}

/// Force the global picker into Kitty protocol for deterministic benchmarks and
/// tests. No-op if the picker is already initialized. Uses a font-size-correct
/// fast picker base so cell<->pixel math matches a real Kitty terminal.
pub fn force_test_kitty_picker() {
    PICKER.get_or_init(|| {
        let mut picker = fast_picker();
        picker.set_protocol_type(ProtocolType::Kitty);
        Some(picker)
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

thread_local! {
    /// Scoped test override for image-protocol availability. The real signal
    /// (PICKER) is a process-global OnceLock that any test can initialize as
    /// a side effect, so "no protocol" tests need a thread-local pin instead
    /// of relying on process-wide ordering.
    static IMAGE_PROTOCOL_OVERRIDE: std::cell::Cell<Option<bool>> =
        const { std::cell::Cell::new(None) };
}

/// Run `f` with image-protocol availability forced on/off on the current
/// thread (or `None` to restore the real detection).
pub fn with_image_protocol_override<T>(enabled: Option<bool>, f: impl FnOnce() -> T) -> T {
    IMAGE_PROTOCOL_OVERRIDE.with(|cell| {
        let prev = cell.replace(enabled);
        struct Reset<'a>(&'a std::cell::Cell<Option<bool>>, Option<bool>);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.set(self.1);
            }
        }
        let _reset = Reset(cell, prev);
        f()
    })
}

pub fn image_protocol_available() -> bool {
    if let Some(enabled) = IMAGE_PROTOCOL_OVERRIDE.with(|cell| cell.get()) {
        return enabled;
    }
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
    let diagram = get_cached_diagram(hash, None)?;
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
            RenderProfile::default(),
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
        if let Some(existing) = cache.get(hash, None, Some(RenderProfile::default())) {
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
            RenderProfile::default(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_protocol_detects_kitty_family() {
        assert_eq!(
            infer_protocol_from_env(Some("xterm-kitty"), None, None, None),
            Some(ProtocolType::Kitty)
        );
        assert_eq!(
            infer_protocol_from_env(None, Some("ghostty"), None, None),
            Some(ProtocolType::Kitty)
        );
        assert_eq!(
            infer_protocol_from_env(None, Some("HandTerm"), None, None),
            Some(ProtocolType::Kitty)
        );
        assert_eq!(
            infer_protocol_from_env(None, Some("WezTerm"), None, None),
            Some(ProtocolType::Kitty)
        );
        // KITTY_WINDOW_ID present is sufficient.
        assert_eq!(
            infer_protocol_from_env(Some("xterm-256color"), None, None, Some("3")),
            Some(ProtocolType::Kitty)
        );
    }

    #[test]
    fn infer_protocol_detects_iterm_and_sixel() {
        assert_eq!(
            infer_protocol_from_env(None, Some("iTerm.app"), None, None),
            Some(ProtocolType::Iterm2)
        );
        assert_eq!(
            infer_protocol_from_env(Some("xterm-sixel"), None, None, None),
            Some(ProtocolType::Sixel)
        );
    }

    #[test]
    fn infer_protocol_misses_inside_masking_multiplexer() {
        // Herdr/tmux advertise a bland TERM with no graphics hints.
        assert_eq!(
            infer_protocol_from_env(Some("xterm-256color"), None, None, None),
            None
        );
    }

    #[test]
    fn detect_multiplexer_identifies_each() {
        assert_eq!(
            detect_multiplexer(Some("xterm-256color"), None, None, None, Some("1")),
            Multiplexer::Herdr
        );
        assert_eq!(
            detect_multiplexer(Some("xterm-256color"), None, None, Some("0.40.1"), None),
            Multiplexer::Zellij
        );
        assert_eq!(
            detect_multiplexer(
                Some("tmux-256color"),
                Some("/tmp/tmux-1000/default,1,0"),
                None,
                None,
                None
            ),
            Multiplexer::Tmux
        );
        assert_eq!(
            detect_multiplexer(
                Some("screen.xterm-256color"),
                None,
                Some("1234.pts-0.host"),
                None,
                None
            ),
            Multiplexer::Screen
        );
        // TERM prefix alone is enough for screen/tmux even without TMUX/STY.
        assert_eq!(
            detect_multiplexer(Some("screen-256color"), None, None, None, None),
            Multiplexer::Screen
        );
        assert_eq!(
            detect_multiplexer(Some("tmux-256color"), None, None, None, None),
            Multiplexer::Tmux
        );
        assert_eq!(
            detect_multiplexer(Some("xterm-kitty"), None, None, None, None),
            Multiplexer::None
        );
    }

    #[test]
    fn detect_multiplexer_herdr_wins_over_others() {
        // Herdr is the most specific signal even if a stale TMUX leaks through.
        assert_eq!(
            detect_multiplexer(
                Some("xterm-256color"),
                Some("/tmp/tmux"),
                None,
                None,
                Some("1")
            ),
            Multiplexer::Herdr
        );
    }

    #[test]
    fn decide_mode_respects_explicit_override() {
        // Force-on always probes, even when env already detected a protocol.
        assert_eq!(
            decide_picker_init_mode(Some(true), Some(ProtocolType::Kitty), Multiplexer::None),
            PickerInitMode::Probe
        );
        // Force-off never probes, even on an env miss inside a multiplexer.
        assert_eq!(
            decide_picker_init_mode(Some(false), None, Multiplexer::Herdr),
            PickerInitMode::Fast
        );
    }

    #[test]
    fn decide_mode_defaults_to_fast_path() {
        // Env already identified a graphics terminal: stay fast.
        assert_eq!(
            decide_picker_init_mode(None, Some(ProtocolType::Kitty), Multiplexer::None),
            PickerInitMode::Fast
        );
        // An env miss must not turn an optional capability query into a
        // two-second startup stall. Multiplexer users can opt in explicitly.
        assert_eq!(
            decide_picker_init_mode(None, None, Multiplexer::Herdr),
            PickerInitMode::Fast
        );
        assert_eq!(
            decide_picker_init_mode(None, None, Multiplexer::None),
            PickerInitMode::Fast
        );
    }

    #[test]
    fn probe_env_helper_back_compat() {
        // Unset / disabled keeps the historical fast default.
        assert_eq!(picker_init_mode_from_probe_env(None), PickerInitMode::Fast);
        assert_eq!(
            picker_init_mode_from_probe_env(Some("0")),
            PickerInitMode::Fast
        );
        // Explicit enable still probes.
        assert_eq!(
            picker_init_mode_from_probe_env(Some("1")),
            PickerInitMode::Probe
        );
    }
}
