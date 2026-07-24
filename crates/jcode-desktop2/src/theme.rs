//! Theme-agnostic design tokens.
//!
//! The scene code only speaks semantic roles (`background`, `text`,
//! `muted`, `rule`, ...), never literal colors. Concrete themes are plain
//! data, so new themes are additions, not rewrites. Follows the old
//! desktop's `DesktopTheme` shape (mode + roles) with the website's print
//! language as the default light theme.

use vello::peniko::Color;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeMode {
    System,
    Light,
    Dark,
}

/// Semantic color roles. Scene code must not hardcode colors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub mode: ThemeMode,
    /// Page background.
    pub background: Color,
    /// Primary foreground text.
    pub text: Color,
    /// Secondary text: captions, status, timestamps.
    pub muted: Color,
    /// Tertiary text: hints, placeholders.
    pub faint: Color,
    /// Hairline rules and borders.
    pub rule: Color,
    /// Quiet fill for code blocks and wells.
    pub wash: Color,
    /// Errors. The print theme keeps this ink-only per the style guide;
    /// other themes may use hue.
    pub error: Color,
}

impl Theme {
    /// The website print language: ink on paper, grays as ink densities.
    pub fn print_light() -> Self {
        Self {
            mode: ThemeMode::Light,
            background: Color::from_rgb8(0xff, 0xff, 0xff),
            text: Color::from_rgb8(0x11, 0x11, 0x11),
            muted: Color::from_rgb8(0x66, 0x66, 0x66),
            faint: Color::from_rgb8(0x99, 0x99, 0x99),
            rule: Color::from_rgb8(0xcc, 0xcc, 0xcc),
            wash: Color::from_rgb8(0xf4, 0xf4, 0xf4),
            error: Color::from_rgb8(0x11, 0x11, 0x11),
        }
    }

    /// Print language inverted: paper ink on near-black, same densities.
    pub fn print_dark() -> Self {
        Self {
            mode: ThemeMode::Dark,
            background: Color::from_rgb8(0x0e, 0x0e, 0x0e),
            text: Color::from_rgb8(0xee, 0xee, 0xee),
            muted: Color::from_rgb8(0x99, 0x99, 0x99),
            faint: Color::from_rgb8(0x66, 0x66, 0x66),
            rule: Color::from_rgb8(0x33, 0x33, 0x33),
            wash: Color::from_rgb8(0x1a, 0x1a, 0x1a),
            error: Color::from_rgb8(0xee, 0xee, 0xee),
        }
    }

    pub fn for_mode(mode: ThemeMode, system_dark: bool) -> Self {
        match mode {
            ThemeMode::Light => Self::print_light(),
            ThemeMode::Dark => Self::print_dark(),
            ThemeMode::System if system_dark => Self::print_dark(),
            ThemeMode::System => Self::print_light(),
        }
    }

    /// Resolve from the environment: `JCODE_DESKTOP2_THEME=light|dark|system`.
    pub fn from_env() -> Self {
        let mode = match std::env::var("JCODE_DESKTOP2_THEME").as_deref() {
            Ok("dark") => ThemeMode::Dark,
            Ok("light") => ThemeMode::Light,
            _ => ThemeMode::System,
        };
        // System detection: honor common portals later; default light for now
        // to match the website.
        Self::for_mode(mode, false)
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::print_light()
    }
}
