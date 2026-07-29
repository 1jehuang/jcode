//! Terminal light/dark theme detection.
//!
//! Resolves the theme mode once per process, before the TUI enters raw mode:
//!
//! 1. `JCODE_THEME=dark|light` env override (also accepts `auto`).
//! 2. `display.theme` config: "dark", "light", or "auto"/empty.
//! 3. Auto: query the terminal's background color (OSC 11 via
//!    `terminal-colorsaurus`) and classify by perceived lightness. Terminals
//!    known not to support OSC queries are rejected before the bounded query so
//!    they do not add hundreds of milliseconds to startup.
//! 4. Fallback: dark (jcode's native palette).
//!
//! The result is stored in `jcode_tui_style::theme_mode` where the renderer
//! adapts colors for light backgrounds at frame time.

use jcode_tui_style::ThemeMode;
use std::sync::{Mutex, OnceLock};

static DETECTED: OnceLock<ThemeMode> = OnceLock::new();

/// In-flight prewarm of the (blocking) terminal background query. See
/// [`prewarm_theme_mode`].
static PREWARM: Mutex<Option<std::thread::JoinHandle<ThemeMode>>> = Mutex::new(None);

/// Start resolving the theme mode on a background thread so the OSC 11 round
/// trip overlaps other startup work (notably spawning/awaiting the server)
/// instead of adding its full latency to the critical path.
///
/// Only safe before the terminal enters raw mode and while nothing else reads
/// stdin, because the query writes an escape sequence and consumes the reply.
/// Idempotent, and a no-op once the mode is already resolved.
pub fn prewarm_theme_mode() {
    if DETECTED.get().is_some() {
        return;
    }
    let Ok(mut slot) = PREWARM.lock() else {
        return;
    };
    if slot.is_some() {
        return;
    }
    if let Ok(handle) = std::thread::Builder::new()
        .name("jcode-theme-detect".to_string())
        .spawn(resolve_theme_mode)
    {
        *slot = Some(handle);
    }
}

/// Join a prewarm started by [`prewarm_theme_mode`], if any.
fn take_prewarmed_theme_mode() -> Option<ThemeMode> {
    let handle = PREWARM.lock().ok()?.take()?;
    handle.join().ok()
}

/// Resolve and install the global theme mode. Idempotent; the first call does
/// the (potentially blocking, sub-second) terminal query and later calls are
/// free. Must be called before entering raw mode / the alternate screen.
pub fn init_theme_mode() -> ThemeMode {
    let mode = match take_prewarmed_theme_mode() {
        Some(prewarmed) => *DETECTED.get_or_init(|| prewarmed),
        None => *DETECTED.get_or_init(resolve_theme_mode),
    };
    jcode_tui_style::set_theme_mode(mode);
    init_palette();
    mode
}

/// Resolve the theme while resuming an already-active TUI after an `exec` handoff.
///
/// The inherited terminal is already in raw mode and may already have a crossterm
/// event reader attached. Sending a fresh OSC 11 query in that state can leave the
/// terminal's color response in stdin, where it is decoded as ordinary composer
/// input. Prefer the theme captured by the previous process and otherwise resolve
/// configuration without querying the terminal.
pub fn init_theme_mode_for_resume(inherited_theme: Option<&str>) -> ThemeMode {
    let inherited_theme = inherited_theme.and_then(|value| match value {
        "dark" => Some(ThemeMode::Dark),
        "light" => Some(ThemeMode::Light),
        _ => None,
    });
    // A prewarm may already have queried the terminal; prefer its answer over a
    // second query, but never start one on an inherited raw-mode terminal.
    let prewarmed = take_prewarmed_theme_mode();
    let mode = *DETECTED.get_or_init(|| {
        inherited_theme
            .or(prewarmed)
            .unwrap_or_else(resolve_theme_mode_without_terminal_query)
    });
    jcode_tui_style::set_theme_mode(mode);
    init_palette();
    mode
}

/// Install the user's configured color palette from `[display.colors]`.
///
/// Invalid entries are logged and skipped rather than failing the palette, so
/// one typo can never leave the TUI unstyled. Safe to call repeatedly; the TUI
/// calls it again after `/colors` edits so changes apply without a restart.
pub fn init_palette() {
    let configured = &crate::config::config().display.colors;
    let (palette, errors) = jcode_tui_style::Palette::from_pairs(
        configured
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str())),
    );
    for error in errors {
        crate::logging::warn(&format!("display.colors: {error}"));
    }
    jcode_tui_style::set_palette(palette);
}

pub fn current_theme_label() -> &'static str {
    match jcode_tui_style::theme_mode() {
        ThemeMode::Dark => "dark",
        ThemeMode::Light => "light",
    }
}

fn resolve_theme_mode() -> ThemeMode {
    resolve_configured_theme(true)
}

fn resolve_theme_mode_without_terminal_query() -> ThemeMode {
    resolve_configured_theme(false)
}

fn resolve_configured_theme(query_terminal: bool) -> ThemeMode {
    let configured = std::env::var("JCODE_THEME")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| crate::config::config().display.theme.clone());

    match configured.trim().to_ascii_lowercase().as_str() {
        "dark" => return ThemeMode::Dark,
        "light" => return ThemeMode::Light,
        "" | "auto" => {}
        other => {
            crate::logging::info(&format!(
                "Unknown theme '{other}' (expected auto/dark/light); using auto detection"
            ));
        }
    }

    if query_terminal {
        detect_terminal_theme().unwrap_or(ThemeMode::Dark)
    } else {
        crate::logging::info(
            "Skipping terminal background query during reload handoff; preserving a safe theme",
        );
        ThemeMode::Dark
    }
}

/// Query the terminal background color and classify it as dark or light.
/// Returns None when the terminal does not support querying or the query
/// fails, in which case the caller falls back to dark.
fn detect_terminal_theme() -> Option<ThemeMode> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return None;
    }
    if !terminal_background_query_supported(
        std::env::var("TERM").ok().as_deref(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
        std::env::var("LC_TERMINAL").ok().as_deref(),
    ) {
        crate::logging::info(
            "Skipping terminal background query for a terminal without OSC query support",
        );
        return None;
    }
    // A terminal that advertises OSC support but never answers costs the full
    // timeout on every single launch. Remember that verdict per terminal
    // identity so only the first launch pays it.
    if silent_terminal_is_cached_at(
        silent_terminal_cache_path().as_deref(),
        &terminal_identity(),
    ) {
        crate::logging::info(
            "Skipping terminal background query for a terminal cached as non-answering",
        );
        return None;
    }
    let mut options = terminal_colorsaurus::QueryOptions::default();
    // Keep startup snappy; supporting terminals answer in a few ms, and
    // colorsaurus detects non-supporting terminals before the timeout anyway.
    // This bound lands on the launch critical path whenever a terminal claims
    // OSC support but never replies (multiplexers, remote shells), so keep it
    // far below the perceptible-stall threshold.
    options.timeout = std::time::Duration::from_millis(120);
    match terminal_colorsaurus::theme_mode(options) {
        Ok(terminal_colorsaurus::ThemeMode::Light) => {
            crate::logging::info("Detected light terminal background; adapting theme");
            Some(ThemeMode::Light)
        }
        Ok(terminal_colorsaurus::ThemeMode::Dark) => Some(ThemeMode::Dark),
        Err(e) => {
            if matches!(e, terminal_colorsaurus::Error::Timeout(_)) {
                cache_silent_terminal_at(
                    silent_terminal_cache_path().as_deref(),
                    &terminal_identity(),
                );
            }
            crate::logging::info(&format!(
                "Terminal background detection unavailable ({e}); defaulting to dark theme"
            ));
            None
        }
    }
}

/// Identity of the terminal we are talking to, for caching purposes. Keep it
/// coarse: the emulator identity, not the individual window or session.
fn terminal_identity() -> String {
    let value = |name: &str| std::env::var(name).unwrap_or_default();
    format!(
        "{}|{}|{}",
        value("TERM"),
        value("TERM_PROGRAM"),
        value("LC_TERMINAL")
    )
}

fn silent_terminal_cache_path() -> Option<std::path::PathBuf> {
    Some(
        crate::storage::jcode_dir()
            .ok()?
            .join("cache")
            .join("osc11-silent-terminals"),
    )
}

/// Longest cache we keep, so an upgraded or reconfigured terminal that gains
/// OSC support is re-probed instead of being written off forever.
const SILENT_TERMINAL_CACHE_TTL: std::time::Duration =
    std::time::Duration::from_secs(60 * 60 * 24 * 7);

/// Cap on remembered terminal identities. This is a cache, not a record.
const SILENT_TERMINAL_CACHE_MAX: usize = 32;

fn silent_terminal_is_cached_at(path: Option<&std::path::Path>, identity: &str) -> bool {
    let Some(path) = path else {
        return false;
    };
    let Ok(modified) = std::fs::metadata(path).and_then(|meta| meta.modified()) else {
        return false;
    };
    if modified
        .elapsed()
        .is_ok_and(|age| age > SILENT_TERMINAL_CACHE_TTL)
    {
        return false;
    }
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    contents.lines().any(|line| line == identity)
}

fn cache_silent_terminal_at(path: Option<&std::path::Path>, identity: &str) {
    let Some(path) = path else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    if existing.lines().any(|line| line == identity) {
        return;
    }
    let mut kept: Vec<&str> = existing
        .lines()
        .filter(|line| !line.trim().is_empty())
        .rev()
        .take(SILENT_TERMINAL_CACHE_MAX - 1)
        .collect();
    kept.reverse();
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(identity);
    out.push('\n');
    let _ = std::fs::write(path, out);
}

/// Reject terminal classes that cannot answer OSC 11 before entering the
/// colorsaurus timeout path. A concrete terminal-program hint wins because
/// launchers and multiplexers occasionally leave a conservative `TERM` value
/// in place even though the outer emulator supports OSC queries.
fn terminal_background_query_supported(
    term: Option<&str>,
    term_program: Option<&str>,
    lc_terminal: Option<&str>,
) -> bool {
    if term_program.is_some_and(|value| !value.trim().is_empty())
        || lc_terminal.is_some_and(|value| !value.trim().is_empty())
    {
        return true;
    }

    let term = term.unwrap_or("").trim().to_ascii_lowercase();
    !matches!(term.as_str(), "" | "dumb" | "linux" | "cons25" | "emacs")
}

#[cfg(test)]
mod tests {
    use super::{
        SILENT_TERMINAL_CACHE_MAX, cache_silent_terminal_at, silent_terminal_is_cached_at,
        terminal_background_query_supported,
    };

    #[test]
    fn skips_terminals_without_osc_query_support() {
        for term in [None, Some(""), Some("dumb"), Some("linux"), Some("cons25")] {
            assert!(!terminal_background_query_supported(term, None, None));
        }
    }

    #[test]
    fn queries_terminal_emulators_and_honors_program_hints() {
        assert!(terminal_background_query_supported(
            Some("xterm-256color"),
            None,
            None
        ));
        assert!(terminal_background_query_supported(
            Some("linux"),
            Some("kitty"),
            None
        ));
        assert!(terminal_background_query_supported(
            Some("linux"),
            None,
            Some("iTerm2")
        ));
    }

    #[test]
    fn caches_a_non_answering_terminal_so_only_the_first_launch_pays_the_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache").join("osc11-silent-terminals");

        assert!(!silent_terminal_is_cached_at(Some(&path), "xterm|kitty|"));
        cache_silent_terminal_at(Some(&path), "xterm|kitty|");
        assert!(silent_terminal_is_cached_at(Some(&path), "xterm|kitty|"));
        // A different terminal must still be probed.
        assert!(!silent_terminal_is_cached_at(Some(&path), "xterm|wezterm|"));
    }

    #[test]
    fn silent_terminal_cache_is_bounded_and_keeps_the_newest_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("osc11-silent-terminals");
        for i in 0..(SILENT_TERMINAL_CACHE_MAX * 2) {
            cache_silent_terminal_at(Some(&path), &format!("term-{i}||"));
        }
        let contents = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = contents.lines().filter(|l| !l.is_empty()).collect();
        assert!(lines.len() <= SILENT_TERMINAL_CACHE_MAX, "{}", lines.len());
        let newest = format!("term-{}||", SILENT_TERMINAL_CACHE_MAX * 2 - 1);
        assert!(silent_terminal_is_cached_at(Some(&path), &newest));
    }

    #[test]
    fn missing_cache_path_never_skips_the_query() {
        assert!(!silent_terminal_is_cached_at(None, "xterm||"));
        // Must not panic when there is nowhere to write.
        cache_silent_terminal_at(None, "xterm||");
    }
}
