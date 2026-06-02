//! Issue #5: minimal theme loader.
//!
//! Reads a TOML file containing per-element color overrides. Loaded
//! from (in order):
//!
//!   1. `JCODE_THEME` env var pointing at an absolute path
//!   2. `~/.jcode/theme.toml` (user-level)
//!   3. `<repo>/.jcode/theme.toml` (project-level)
//!
//! When none of those exist, [`Theme::default()`] is returned and
//! the existing hard-coded palette continues to render.
//!
//! This PR ships the **loader + schema** only. Wiring individual
//! theme values into the renderer is a follow-up; doing it
//! incrementally avoids a multi-thousand-line refactor.
//!
//! ## Schema (theme.toml)
//!
//! ```toml
//! # All fields optional. Missing fields fall back to defaults.
//! name = "solarized-dark"
//!
//! [colors]
//! primary    = "#268bd2"
//! secondary  = "#2aa198"
//! success    = "#859900"
//! warning    = "#b58900"
//! error      = "#dc322f"
//! muted      = "#586e75"
//! background = "#002b36"
//! foreground = "#839496"
//! ```
//!
//! Color values must be a 7-char hex string (`#RRGGBB`). Lower or
//! upper case both accepted. Any other format causes the loader to
//! return [`Theme::default()`] and log a warning, matching the
//! 'graceful fallback' behavior of the rest of jcode.

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

// ---- Issue #5: themes hot reload ----
//
// `load_or_default()` already re-reads from disk on every call.
// Hot reload adds two cheap probes the TUI / `/theme reload` slash
// command can poll without full re-parse:
//
//   discover_cached(home, repo) -> (Theme, ThemeCacheToken)
//   theme_changed_since(token, home, repo) -> bool
//
// The cache invalidates when the active theme.toml's mtime advances
// or when the resolved theme path changes (user moved theme between
// project + user level, JCODE_THEME env var changed).

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeCacheToken(SystemTime, Option<PathBuf>);

impl ThemeCacheToken {
    pub fn epoch() -> Self {
        Self(SystemTime::UNIX_EPOCH, None)
    }
}

struct ThemeCache {
    last_mtime: SystemTime,
    last_path: Option<PathBuf>,
    cached: Theme,
}

impl Default for ThemeCache {
    fn default() -> Self {
        Self {
            last_mtime: SystemTime::UNIX_EPOCH,
            last_path: None,
            cached: Theme::default(),
        }
    }
}

static THEME_CACHE: Mutex<Option<ThemeCache>> = Mutex::new(None);

/// Hot-reload aware theme load. Returns the cached theme when the
/// resolved theme.toml hasn't changed mtime + path since last call.
pub fn discover_cached(jcode_home: &Path, repo_root: Option<&Path>) -> (Theme, ThemeCacheToken) {
    let path = resolve_theme_path(jcode_home, repo_root);
    let mtime = path
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let mut guard = THEME_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = guard.get_or_insert_with(ThemeCache::default);

    let stale = cache.last_mtime != mtime || cache.last_path != path;
    if stale {
        cache.cached = load_or_default(jcode_home, repo_root);
        cache.last_mtime = mtime;
        cache.last_path = path.clone();
    }
    (
        cache.cached.clone(),
        ThemeCacheToken(cache.last_mtime, cache.last_path.clone()),
    )
}

/// Cheap probe — has the active theme file changed since `token`?
/// No re-parse, just an mtime + path lookup.
pub fn theme_changed_since(
    token: &ThemeCacheToken,
    jcode_home: &Path,
    repo_root: Option<&Path>,
) -> bool {
    let path = resolve_theme_path(jcode_home, repo_root);
    let mtime = path
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    mtime != token.0 || path != token.1
}

/// Drop the cache (test helper).
pub fn clear_theme_cache_for_tests() {
    let mut guard = THEME_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    *guard = None;
}

/// RGB color, 0-255 per channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn parse_hex(s: &str) -> Option<Self> {
        let s = s.trim();
        let s = s.strip_prefix('#').unwrap_or(s);
        if s.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some(Self { r, g, b })
    }

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// User-tunable palette. Each field falls back to a baked default
/// when not explicitly set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub name: String,
    pub primary: Color,
    pub secondary: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub muted: Color,
    pub background: Color,
    pub foreground: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            primary: Color::rgb(0x52, 0x95, 0xe3), // soft blue
            secondary: Color::rgb(0x4e, 0xc9, 0xb0), // teal
            success: Color::rgb(0x73, 0xc9, 0x91), // green
            warning: Color::rgb(0xe5, 0xc0, 0x7b), // amber
            error: Color::rgb(0xe0, 0x6c, 0x75),   // red
            muted: Color::rgb(0x80, 0x80, 0x80),   // gray
            background: Color::rgb(0x1e, 0x1e, 0x1e), // near black
            foreground: Color::rgb(0xd4, 0xd4, 0xd4), // off white
        }
    }
}

#[derive(Debug, Deserialize, Default)]
struct ThemeFile {
    name: Option<String>,
    #[serde(default)]
    colors: ThemeColors,
}

#[derive(Debug, Deserialize, Default)]
struct ThemeColors {
    primary: Option<String>,
    secondary: Option<String>,
    success: Option<String>,
    warning: Option<String>,
    error: Option<String>,
    muted: Option<String>,
    background: Option<String>,
    foreground: Option<String>,
}

/// Parse a TOML string into a [`Theme`], applying defaults for
/// missing fields. Returns `Err` only when the input is malformed
/// TOML or contains an invalid hex value. Callers typically prefer
/// [`Theme::load_or_default`] which logs and falls back instead.
pub fn parse_toml(toml_str: &str) -> Result<Theme, String> {
    let parsed: ThemeFile =
        toml::from_str(toml_str).map_err(|e| format!("invalid theme TOML: {e}"))?;
    let mut theme = Theme::default();
    if let Some(name) = parsed.name {
        theme.name = name;
    }
    fn apply(slot: &mut Color, raw: Option<String>, key: &str) -> Result<(), String> {
        if let Some(s) = raw {
            *slot = Color::parse_hex(&s).ok_or_else(|| {
                format!("invalid hex color for `{key}`: {s:?} (expected #RRGGBB)")
            })?;
        }
        Ok(())
    }
    let c = parsed.colors;
    apply(&mut theme.primary, c.primary, "primary")?;
    apply(&mut theme.secondary, c.secondary, "secondary")?;
    apply(&mut theme.success, c.success, "success")?;
    apply(&mut theme.warning, c.warning, "warning")?;
    apply(&mut theme.error, c.error, "error")?;
    apply(&mut theme.muted, c.muted, "muted")?;
    apply(&mut theme.background, c.background, "background")?;
    apply(&mut theme.foreground, c.foreground, "foreground")?;
    Ok(theme)
}

/// Resolve which theme.toml to load.
///
/// Returns the path of the first found candidate, or `None` if no
/// theme file is configured.
pub fn resolve_theme_path(jcode_home: &Path, repo_root: Option<&Path>) -> Option<PathBuf> {
    if let Ok(p) = std::env::var("JCODE_THEME") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    let user = jcode_home.join("theme.toml");
    if user.exists() {
        return Some(user);
    }
    if let Some(repo) = repo_root {
        let proj = repo.join(".jcode").join("theme.toml");
        if proj.exists() {
            return Some(proj);
        }
    }
    None
}

/// Convenience: load the active theme, falling back to default on
/// any error (with a warning log message).
pub fn load_or_default(jcode_home: &Path, repo_root: Option<&Path>) -> Theme {
    let Some(path) = resolve_theme_path(jcode_home, repo_root) else {
        return Theme::default();
    };
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "jcode theme: failed to read {}: {} (using default)",
                path.display(),
                e
            );
            return Theme::default();
        }
    };
    match parse_toml(&raw) {
        Ok(t) => t,
        Err(e) => {
            eprintln!(
                "jcode theme: {} parse error: {} (using default)",
                path.display(),
                e
            );
            Theme::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_theme_with_all_fields() {
        let toml = r##"
            name = "solarized-dark"

            [colors]
            primary    = "#268bd2"
            secondary  = "#2aa198"
            success    = "#859900"
            warning    = "#b58900"
            error      = "#dc322f"
            muted      = "#586e75"
            background = "#002b36"
            foreground = "#839496"
        "##;
        let theme = parse_toml(toml).expect("parse");
        assert_eq!(theme.name, "solarized-dark");
        assert_eq!(theme.primary, Color::rgb(0x26, 0x8b, 0xd2));
        assert_eq!(theme.background, Color::rgb(0x00, 0x2b, 0x36));
        assert_eq!(theme.foreground, Color::rgb(0x83, 0x94, 0x96));
    }

    #[test]
    fn empty_theme_falls_back_to_defaults() {
        let theme = parse_toml("").expect("empty parses");
        assert_eq!(theme, Theme::default());
    }

    #[test]
    fn partial_theme_overrides_only_specified_fields() {
        let toml = r##"
            [colors]
            primary = "#ff00ff"
        "##;
        let theme = parse_toml(toml).expect("parse");
        assert_eq!(theme.primary, Color::rgb(0xff, 0x00, 0xff));
        // Other fields still defaults.
        assert_eq!(theme.secondary, Theme::default().secondary);
    }

    #[test]
    fn case_insensitive_hex() {
        assert_eq!(
            Color::parse_hex("#ABCDEF"),
            Some(Color::rgb(0xab, 0xcd, 0xef))
        );
        assert_eq!(
            Color::parse_hex("#abcdef"),
            Some(Color::rgb(0xab, 0xcd, 0xef))
        );
    }

    #[test]
    fn rejects_short_hex() {
        assert_eq!(Color::parse_hex("#abc"), None);
        assert_eq!(Color::parse_hex("#abcdef00"), None);
    }

    #[test]
    fn rejects_garbage_hex() {
        let toml = r##"
            [colors]
            primary = "not a color"
        "##;
        let err = parse_toml(toml).unwrap_err();
        assert!(err.contains("primary"), "{err}");
    }

    #[test]
    fn to_hex_round_trips() {
        let c = Color::rgb(0x12, 0x34, 0x56);
        assert_eq!(c.to_hex(), "#123456");
        assert_eq!(Color::parse_hex(&c.to_hex()), Some(c));
    }

    #[test]
    fn resolve_falls_back_through_user_then_project() {
        let temp = tempfile::TempDir::new().unwrap();
        let home = temp.path().join("home");
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(repo.join(".jcode")).unwrap();

        // Nothing configured.
        assert_eq!(resolve_theme_path(&home, Some(&repo)), None);

        // Project-level only.
        std::fs::write(repo.join(".jcode/theme.toml"), "").unwrap();
        assert_eq!(
            resolve_theme_path(&home, Some(&repo)),
            Some(repo.join(".jcode/theme.toml"))
        );

        // User-level wins over project-level.
        std::fs::write(home.join("theme.toml"), "").unwrap();
        assert_eq!(
            resolve_theme_path(&home, Some(&repo)),
            Some(home.join("theme.toml"))
        );
    }

    #[test]
    fn load_or_default_returns_default_on_missing_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let home = temp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let theme = load_or_default(&home, None);
        assert_eq!(theme, Theme::default());
    }

    #[test]
    fn load_or_default_returns_default_on_parse_error_without_panic() {
        let temp = tempfile::TempDir::new().unwrap();
        let home = temp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("theme.toml"), "[[[ totally not toml").unwrap();
        let theme = load_or_default(&home, None);
        // Should silently fall back, not panic.
        assert_eq!(theme, Theme::default());
    }
}

#[cfg(test)]
mod hot_reload_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn cached_returns_default_when_no_theme_file() {
        let _lock = crate::storage::lock_test_env();
        clear_theme_cache_for_tests();
        let temp = tempfile::TempDir::new().unwrap();
        let prev_env = std::env::var_os("JCODE_THEME");
        unsafe {
            std::env::remove_var("JCODE_THEME");
        }
        let (theme, _) = discover_cached(temp.path(), None);
        assert_eq!(theme, Theme::default());
        if let Some(p) = prev_env {
            unsafe {
                std::env::set_var("JCODE_THEME", p);
            }
        }
    }

    #[test]
    fn cached_invalidates_when_theme_file_added() {
        let _lock = crate::storage::lock_test_env();
        clear_theme_cache_for_tests();
        let temp = tempfile::TempDir::new().unwrap();
        let prev_env = std::env::var_os("JCODE_THEME");
        unsafe {
            std::env::remove_var("JCODE_THEME");
        }

        let (theme1, token1) = discover_cached(temp.path(), None);
        assert_eq!(theme1.name, "default");

        // Add a theme file.
        std::fs::write(temp.path().join("theme.toml"), "name = \"custom\"\n").unwrap();

        std::thread::sleep(Duration::from_millis(50));
        let (theme2, token2) = discover_cached(temp.path(), None);
        assert_eq!(theme2.name, "custom");
        assert_ne!(token1, token2);

        if let Some(p) = prev_env {
            unsafe {
                std::env::set_var("JCODE_THEME", p);
            }
        }
    }

    #[test]
    fn changed_since_detects_mtime_advance() {
        let _lock = crate::storage::lock_test_env();
        clear_theme_cache_for_tests();
        let temp = tempfile::TempDir::new().unwrap();
        let prev_env = std::env::var_os("JCODE_THEME");
        unsafe {
            std::env::remove_var("JCODE_THEME");
        }

        std::fs::write(temp.path().join("theme.toml"), "name = \"v1\"\n").unwrap();

        let (_, token) = discover_cached(temp.path(), None);
        assert!(!theme_changed_since(&token, temp.path(), None));

        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(temp.path().join("theme.toml"), "name = \"v2\"\n").unwrap();

        assert!(theme_changed_since(&token, temp.path(), None));

        if let Some(p) = prev_env {
            unsafe {
                std::env::set_var("JCODE_THEME", p);
            }
        }
    }

    #[test]
    fn epoch_token_signals_change_when_file_exists() {
        let _lock = crate::storage::lock_test_env();
        clear_theme_cache_for_tests();
        let temp = tempfile::TempDir::new().unwrap();
        let prev_env = std::env::var_os("JCODE_THEME");
        unsafe {
            std::env::remove_var("JCODE_THEME");
        }

        std::fs::write(temp.path().join("theme.toml"), "name = \"x\"\n").unwrap();
        assert!(theme_changed_since(
            &ThemeCacheToken::epoch(),
            temp.path(),
            None
        ));

        if let Some(p) = prev_env {
            unsafe {
                std::env::set_var("JCODE_THEME", p);
            }
        }
    }
}
