//! Terminal light/dark theme detection.
//!
//! Resolves the theme mode once per process, before the TUI enters raw mode:
//!
//! 1. `JCODE_THEME=dark|light` env override (also accepts `auto`).
//! 2. `display.theme` config: "dark", "light", "system", custom name, or "auto"/empty.
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

/// Start resolving the theme mode on a background thread so the OSC 11 query
/// overlaps other startup work instead of extending the critical path.
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
    apply_configured_palette();
    let mode = match take_prewarmed_theme_mode() {
        Some(prewarmed) => *DETECTED.get_or_init(|| prewarmed),
        None => *DETECTED.get_or_init(resolve_theme_mode),
    };
    jcode_tui_style::set_theme_mode(mode);
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
    apply_configured_palette();
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
    mode
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

fn configured_theme_name() -> String {
    std::env::var("JCODE_THEME")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| crate::config::config().display.theme.clone())
}

/// Custom-theme search directories, most preferred first.
///
/// `/theme` help, the config template, and TUI_COLOR_CONFIGURATION.md all
/// advertise `~/.jcode/themes/<name>.toml`, so that directory is searched
/// first. Themes written while the resolver still pointed at the platform
/// config dir (`~/.config/jcode/themes` and friends) keep working as a
/// fallback: a valid file there still loads instead of failing with
/// "Unknown theme".
pub(crate) fn configured_theme_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(base) = crate::storage::jcode_dir() {
        dirs.push(base.join("themes"));
    }
    if let Ok(base) = crate::storage::app_config_dir() {
        let fallback = base.join("themes");
        if !dirs.contains(&fallback) {
            dirs.push(fallback);
        }
    }
    dirs
}

/// Try `attempt` against every configured theme directory in order.
///
/// Builtin names succeed on the first directory, so they never pay for the
/// search. A custom name loads from whichever directory holds the file; if
/// none does, the error from the advertised directory comes back, since that
/// is where the user was told to put the file.
fn search_theme_dirs<T>(
    mut attempt: impl FnMut(Option<&std::path::Path>) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let mut first_error = None;
    for dir in configured_theme_dirs() {
        match attempt(Some(&dir)) {
            Ok(value) => return Ok(value),
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => attempt(None),
    }
}

/// Load a theme by name from the configured directories without installing
/// it, so `/theme <name>` can reject unknown names with the real error.
pub(crate) fn try_load_configured_theme(
    name: &str,
) -> anyhow::Result<jcode_tui_style::theme::Theme> {
    search_theme_dirs(|dir| jcode_tui_style::theme::load_theme(name, dir))
}

/// Load and install a theme by name from the configured directories.
pub(crate) fn set_configured_theme(name: &str) -> anyhow::Result<()> {
    search_theme_dirs(|dir| jcode_tui_style::theme::set_theme(name, dir))
}

fn palette_name(configured: &str) -> &str {
    match configured.trim() {
        "" | "auto" => "system",
        other => other,
    }
}

pub(crate) fn apply_configured_palette() {
    reinstall_palette();
    let configured = configured_theme_name();
    let name = palette_name(&configured);
    if let Err(err) = set_configured_theme(name) {
        crate::logging::info(&format!(
            "Failed to load theme '{name}' ({err:#}); using system theme"
        ));
        if let Err(fallback_err) = jcode_tui_style::theme::set_theme("system", None) {
            crate::logging::info(&format!(
                "Failed to apply system theme fallback ({fallback_err:#})"
            ));
        }
    }
}

/// Install the user's configured color palette from `[display.colors]`.
///
/// Invalid entries are logged and skipped rather than failing the palette, so
/// one typo can never leave the TUI unstyled. Safe to call repeatedly; the TUI
/// calls it again after `/colors` edits so changes apply without a restart.
fn reinstall_palette() {
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

fn resolve_configured_theme(query_terminal: bool) -> ThemeMode {
    let configured = configured_theme_name();

    match configured.trim().to_ascii_lowercase().as_str() {
        "dark" => return ThemeMode::Dark,
        "light" => return ThemeMode::Light,
        "" | "auto" | "system" => {}
        other => {
            crate::logging::info(&format!(
                "Custom theme '{other}' selected; using terminal background detection only when palette allows it"
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
    // Ask for only OSC 11. `theme_mode` queries both OSC 10 (foreground) and
    // OSC 11 (background); on some terminals the two replies can be split far
    // enough apart that the query consumes one and crossterm later decodes the
    // other as ordinary keys. That produces composer garbage such as
    // `10;rgb:cdcd/d6d6/f4f4\\11;rgb:0000/0000/0000\\` at startup. Background
    // lightness is all our renderer needs, and a single request has no second
    // reply that can escape into the event reader.
    match terminal_colorsaurus::background_color(options) {
        Ok(background) if background.perceived_lightness() > 0.5 => {
            crate::logging::info("Detected light terminal background; adapting theme");
            Some(ThemeMode::Light)
        }
        Ok(_) => Some(ThemeMode::Dark),
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
        SILENT_TERMINAL_CACHE_MAX, cache_silent_terminal_at, configured_theme_dirs,
        silent_terminal_is_cached_at, terminal_background_query_supported,
        try_load_configured_theme,
    };
    use crate::storage::lock_test_env;

    #[test]
    fn theme_dirs_prefer_the_advertised_jcode_home_location() {
        let _lock = lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        let dirs = configured_theme_dirs();

        assert_eq!(
            dirs.first(),
            Some(&temp.path().join("themes")),
            "the advertised ~/.jcode/themes path must be searched first"
        );
        assert_eq!(
            dirs.get(1),
            Some(&temp.path().join("config").join("jcode").join("themes")),
            "the platform config dir stays a fallback"
        );

        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn custom_theme_loads_from_the_advertised_jcode_home_dir() {
        let _lock = lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());
        std::fs::create_dir_all(temp.path().join("themes")).expect("themes dir");
        std::fs::write(
            temp.path().join("themes").join("solar.toml"),
            "[colors]\nuser = \"#010203\"\n",
        )
        .expect("write theme");

        let theme = try_load_configured_theme("solar").expect("advertised location must load");
        assert_eq!(theme.name(), "solar");

        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn custom_theme_still_loads_from_the_legacy_config_dir() {
        let _lock = lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());
        std::fs::create_dir_all(temp.path().join("config").join("jcode").join("themes"))
            .expect("legacy themes dir");
        std::fs::write(
            temp.path()
                .join("config")
                .join("jcode")
                .join("themes")
                .join("legacy.toml"),
            "[colors]\nuser = \"#0A0B0C\"\n",
        )
        .expect("write theme");

        let theme = try_load_configured_theme("legacy").expect("legacy location must still load");
        assert_eq!(theme.name(), "legacy");

        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

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
