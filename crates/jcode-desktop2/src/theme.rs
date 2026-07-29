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
    /// Fill behind an *inline* code span. One density stronger than
    /// [`Self::wash`], because a wash tuned for a whole block is too faint to
    /// mark a single word inside a line of prose, and a code span you cannot
    /// see is the same as no code span.
    pub code_wash: Color,
    /// Fill of an input field. The composer is a field, not a code block, so
    /// it gets its own role: a grey slab reads as disabled, paper with a
    /// hairline reads as somewhere to type.
    pub field: Color,
    /// Hairline around an unfocused field.
    pub field_border: Color,
    /// Hairline around the focused field. Stronger than `field_border` so
    /// focus is visible without a colour accent.
    pub field_border_focus: Color,
    /// Errors. The print theme keeps this ink-only per the style guide;
    /// other themes may use hue.
    pub error: Color,
    /// Text selection band. Ink density, not hue, so selected text stays
    /// readable against it.
    pub selection: Color,
    /// Selection band drawn on a washed surface: a user card, or a code
    /// block. One density darker than [`Self::selection`], because a band
    /// tuned to contrast with paper nearly vanishes on a wash, and a
    /// highlight you cannot see is the same as no highlight.
    pub selection_on_wash: Color,
    /// Ink for an added line of a diff, and for a removed one. The one place
    /// the print theme spends hue: a diff is read by scanning for which side a
    /// line is on, and `+`/`-` alone makes that a character-by-character job.
    /// Kept desaturated so a card full of them still reads as a document
    /// rather than as a terminal.
    pub added: Color,
    pub removed: Color,
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
            code_wash: Color::from_rgb8(0xea, 0xea, 0xea),
            field: Color::from_rgb8(0xff, 0xff, 0xff),
            field_border: Color::from_rgb8(0xd4, 0xd4, 0xd4),
            field_border_focus: Color::from_rgb8(0x77, 0x77, 0x77),
            error: Color::from_rgb8(0x11, 0x11, 0x11),
            selection: Color::from_rgb8(0xd8, 0xd8, 0xd8),
            selection_on_wash: Color::from_rgb8(0xc4, 0xc4, 0xc4),
            added: Color::from_rgb8(0x1a, 0x6b, 0x3a),
            removed: Color::from_rgb8(0x9b, 0x22, 0x26),
        }
    }

    /// Print language inverted: paper ink on pure black, same densities.
    pub fn print_dark() -> Self {
        Self {
            mode: ThemeMode::Dark,
            background: Color::from_rgb8(0x00, 0x00, 0x00),
            text: Color::from_rgb8(0xee, 0xee, 0xee),
            muted: Color::from_rgb8(0x99, 0x99, 0x99),
            faint: Color::from_rgb8(0x66, 0x66, 0x66),
            rule: Color::from_rgb8(0x33, 0x33, 0x33),
            wash: Color::from_rgb8(0x14, 0x14, 0x14),
            code_wash: Color::from_rgb8(0x24, 0x24, 0x24),
            field: Color::from_rgb8(0x10, 0x10, 0x10),
            field_border: Color::from_rgb8(0x3a, 0x3a, 0x3a),
            field_border_focus: Color::from_rgb8(0x88, 0x88, 0x88),
            error: Color::from_rgb8(0xee, 0xee, 0xee),
            selection: Color::from_rgb8(0x3a, 0x3a, 0x3a),
            selection_on_wash: Color::from_rgb8(0x4c, 0x4c, 0x4c),
            added: Color::from_rgb8(0x6d, 0xd4, 0x92),
            removed: Color::from_rgb8(0xf0, 0x8b, 0x8b),
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

    /// The mode asked for by `JCODE_DESKTOP2_THEME=light|dark|system`.
    /// Kept separate from resolution so the app can remember that "system"
    /// was requested and re-resolve when the system preference changes.
    pub fn preference_from_env() -> ThemeMode {
        match std::env::var("JCODE_DESKTOP2_THEME").as_deref() {
            Ok("dark") => ThemeMode::Dark,
            Ok("light") => ThemeMode::Light,
            _ => ThemeMode::System,
        }
    }

    /// Resolve from the environment: `JCODE_DESKTOP2_THEME=light|dark|system`.
    pub fn from_env() -> Self {
        Self::for_mode(Self::preference_from_env(), system_prefers_dark())
    }
}

/// Whether the desktop asks for dark, read from the XDG settings portal.
///
/// The portal's `org.freedesktop.appearance color-scheme` key is the one
/// cross-desktop source of truth (GNOME, KDE, niri via darkman all serve it),
/// so it is asked first; `gsettings` covers a GNOME session without a portal.
/// Both are asked through short-lived subprocesses rather than a D-Bus crate:
/// this is one read at startup, not a protocol relationship, and a zbus
/// dependency tree is a lot to carry for one integer. No answer means light,
/// which matches the website.
fn system_prefers_dark() -> bool {
    // `busctl call` prints `v u 1` for prefer-dark, `v u 2` for prefer-light,
    // and `v u 0` for no preference.
    let portal = std::process::Command::new("busctl")
        .args([
            "--user",
            "--timeout=1",
            "call",
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings",
            "ReadOne",
            "ss",
            "org.freedesktop.appearance",
            "color-scheme",
        ])
        .output();
    if let Ok(output) = portal
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        return stdout.split_whitespace().last() == Some("1");
    }
    let gsettings = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output();
    if let Ok(output) = gsettings
        && output.status.success()
    {
        return String::from_utf8_lossy(&output.stdout).contains("dark");
    }
    false
}

impl Default for Theme {
    fn default() -> Self {
        Self::print_light()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_role_differs_from_the_background() {
        for theme in [Theme::print_light(), Theme::print_dark()] {
            for (name, role) in [
                ("text", theme.text),
                ("muted", theme.muted),
                ("faint", theme.faint),
                ("rule", theme.rule),
                ("error", theme.error),
                ("selection", theme.selection),
            ] {
                assert_ne!(
                    role.components, theme.background.components,
                    "{name} is invisible in {:?} mode",
                    theme.mode
                );
            }
        }
    }

    #[test]
    fn ink_densities_are_ordered() {
        // Hierarchy comes from ink density: text is the strongest contrast
        // against paper, then muted, then faint. If this inverts, emphasis
        // silently reverses.
        let luma = |color: Color| {
            let [r, g, b, _] = color.components;
            0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b)
        };
        for theme in [Theme::print_light(), Theme::print_dark()] {
            let bg = luma(theme.background);
            let contrast = |color: Color| (luma(color) - bg).abs();
            assert!(
                contrast(theme.text) > contrast(theme.muted),
                "text is not stronger than muted in {:?}",
                theme.mode
            );
            assert!(
                contrast(theme.muted) > contrast(theme.faint),
                "muted is not stronger than faint in {:?}",
                theme.mode
            );
            assert!(
                contrast(theme.faint) > contrast(theme.rule),
                "faint is not stronger than a hairline in {:?}",
                theme.mode
            );
        }
    }

    /// Selected text must stay readable: the band has to contrast with the
    /// text drawn on top of it, not just with the page.
    #[test]
    fn selected_text_stays_readable() {
        let luma = |color: Color| {
            let [r, g, b, _] = color.components;
            0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b)
        };
        for theme in [Theme::print_light(), Theme::print_dark()] {
            let contrast = (luma(theme.text) - luma(theme.selection)).abs();
            assert!(
                contrast > 0.35,
                "text on the selection band is unreadable in {:?} (contrast {contrast:.2})",
                theme.mode
            );
            // The band must also be visible against the page.
            let against_page = (luma(theme.selection) - luma(theme.background)).abs();
            assert!(
                against_page > 0.03,
                "the selection band is invisible in {:?}",
                theme.mode
            );
        }
    }

    /// The band on a washed surface (a user card, a code block) has the same
    /// two jobs, against a different backdrop. A band tuned only for paper is
    /// almost invisible on a wash, which is exactly the case a user hits first:
    /// their own message is a card.
    #[test]
    fn a_selection_on_a_wash_stays_visible_and_readable() {
        let luma = |color: Color| {
            let [r, g, b, _] = color.components;
            0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b)
        };
        for theme in [Theme::print_light(), Theme::print_dark()] {
            let band = luma(theme.selection_on_wash);
            assert!(
                (luma(theme.text) - band).abs() > 0.35,
                "text on a washed selection band is unreadable in {:?}",
                theme.mode
            );
            let against_wash = (band - luma(theme.wash)).abs();
            assert!(
                against_wash > 0.03,
                "the selection band vanishes on a wash in {:?} (contrast {against_wash:.3})",
                theme.mode
            );
            // And it must be the stronger of the two, or it would be doing
            // less work than the band it replaces.
            assert!(
                (band - luma(theme.wash)).abs() > (luma(theme.selection) - luma(theme.wash)).abs(),
                "the washed band is weaker than the paper band in {:?}",
                theme.mode
            );
        }
    }

    #[test]
    fn both_modes_are_defined_for_every_role() {
        let light = Theme::print_light();
        let dark = Theme::print_dark();
        assert_eq!(light.mode, ThemeMode::Light);
        assert_eq!(dark.mode, ThemeMode::Dark);
        // A theme is data: light and dark must actually differ, so "adding a
        // theme" can never be a copy of another mode.
        assert_ne!(light.background.components, dark.background.components);
        assert_ne!(light.text.components, dark.text.components);
    }
}
