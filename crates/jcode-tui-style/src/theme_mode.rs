//! Light/dark terminal theme support.
//!
//! jcode's palette (`theme.rs` and the many ad hoc `rgb(...)` call sites) is
//! designed for dark terminal backgrounds. Rather than maintaining a second
//! hand-tuned palette for light terminals, we adapt colors at the single choke
//! point every style ultimately flows through: the rendered frame buffer.
//!
//! When the theme mode is [`ThemeMode::Light`], [`adapt_buffer_for_theme`]
//! rewrites each cell's colors with a hue-preserving luminance flip, then
//! darkens washed-out text to meet a 7:1 contrast target against its surface.
//! Dark panel backgrounds remain light tints. `Color::Reset` is left alone so
//! the terminal's own (light) default background shows through, exactly like
//! it does on dark themes today.
//!
//! The mode itself is set once at startup by the TUI's terminal-background
//! detection (OSC 11 query / `JCODE_THEME` / `display.theme` config) and
//! defaults to dark, which keeps every existing code path byte-identical.

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use std::sync::atomic::{AtomicU8, Ordering};

/// Whether the terminal background is dark or light.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    /// Dark background, light text (jcode's native palette).
    #[default]
    Dark,
    /// Light background, dark text. Colors are adapted at render time.
    Light,
}

static THEME_MODE: AtomicU8 = AtomicU8::new(0);

/// Set the global theme mode. Called once at startup after terminal
/// background detection (and again if the user overrides it).
pub fn set_theme_mode(mode: ThemeMode) {
    THEME_MODE.store(
        match mode {
            ThemeMode::Dark => 0,
            ThemeMode::Light => 1,
        },
        Ordering::Relaxed,
    );
}

/// Current global theme mode. Defaults to [`ThemeMode::Dark`].
pub fn theme_mode() -> ThemeMode {
    match THEME_MODE.load(Ordering::Relaxed) {
        1 => ThemeMode::Light,
        _ => ThemeMode::Dark,
    }
}

pub fn is_light_theme() -> bool {
    theme_mode() == ThemeMode::Light
}

/// Adapt a single color for the current theme mode. Identity in dark mode.
///
/// In light mode this flips the color's perceived lightness while preserving
/// hue and saturation, so "light blue on dark" becomes "dark blue on light".
/// `Color::Reset` is preserved (the terminal supplies correct defaults).
pub fn adapt_color_for_theme(color: Color) -> Color {
    if !is_light_theme() {
        return color;
    }
    adapt_color_for_light(color)
}

fn adapt_color_for_light(color: Color) -> Color {
    let Some((r, g, b)) = color_rgb(color) else {
        return color;
    };
    flip_luminance(r, g, b)
}

fn color_rgb(color: Color) -> Option<(u8, u8, u8)> {
    Some(match color {
        Color::Reset => return None,
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(n) => crate::color::indexed_to_rgb(n),
        named => {
            let idx = match named {
                Color::Black => 0,
                Color::Red => 1,
                Color::Green => 2,
                Color::Yellow => 3,
                Color::Blue => 4,
                Color::Magenta => 5,
                Color::Cyan => 6,
                Color::Gray => 7,
                Color::DarkGray => 8,
                Color::LightRed => 9,
                Color::LightGreen => 10,
                Color::LightYellow => 11,
                Color::LightBlue => 12,
                Color::LightMagenta => 13,
                Color::LightCyan => 14,
                Color::White => 15,
                _ => return None,
            };
            crate::color::indexed_to_rgb(idx)
        }
    })
}

// Use a conservative light surface for terminal-default backgrounds. This
// covers off-white themes and recessed/inactive panes, not just pure white.
const LIGHT_SURFACE: (u8, u8, u8) = (224, 224, 224);
// Enhanced contrast keeps small terminal glyphs and muted labels clearly legible.
const TARGET_TEXT_CONTRAST: f32 = 7.0;

fn relative_luminance((r, g, b): (u8, u8, u8)) -> f32 {
    let linear = |channel: u8| {
        let value = channel as f32 / 255.;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

fn contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f32 {
    let a = relative_luminance(a);
    let b = relative_luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Repair an already light-adapted foreground. Backgrounds must not take this
/// path: the same gray can be unreadable text but a perfectly good panel tint.
/// Check the quantized output too, so 256-color terminals keep the guarantee.
pub(crate) fn readable_light_foreground(color: Color, background: Color) -> Color {
    let Some(rgb) = color_rgb(color) else {
        return color;
    };
    let surface = color_rgb(background).unwrap_or(LIGHT_SURFACE);
    if contrast(rgb, surface) >= TARGET_TEXT_CONTRAST {
        return color;
    }
    let (h, s, l) = rgb_to_hsl(rgb.0, rgb.1, rgb.2);
    let endpoint = if contrast((0, 0, 0), surface) >= contrast((255, 255, 255), surface) {
        0.
    } else {
        1.
    };
    let mut failing = l;
    let mut passing = endpoint;
    let mut result = crate::color::rgb(
        (endpoint * 255.) as u8,
        (endpoint * 255.) as u8,
        (endpoint * 255.) as u8,
    );
    for _ in 0..12 {
        let candidate = (failing + passing) / 2.;
        let (r, g, b) = hsl_to_rgb(h, s, candidate);
        let quantized = crate::color::rgb(r, g, b);
        if contrast(color_rgb(quantized).unwrap(), surface) >= TARGET_TEXT_CONTRAST {
            passing = candidate;
            result = quantized;
        } else {
            failing = candidate;
        }
    }
    result
}

/// Foreground counterpart to the generic color flip, for partial repaint paths.
pub fn adapt_foreground_for_theme(color: Color, background: Color) -> Color {
    if is_light_theme() {
        readable_light_foreground(adapt_color_for_light(color), background)
    } else {
        color
    }
}

/// Hue/saturation-preserving lightness inversion, quantized through the
/// capability-aware `rgb()` so 256-color terminals stay palette-bounded.
fn flip_luminance(r: u8, g: u8, b: u8) -> Color {
    let (h, s, l) = rgb_to_hsl(r, g, b);
    let (r2, g2, b2) = hsl_to_rgb(h, s, 1.0 - l);
    crate::color::rgb(r2, g2, b2)
}

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let r = r as f32 / 255.0;
    let g = g as f32 / 255.0;
    let b = b as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if (max - r).abs() < f32::EPSILON {
        ((g - b) / d).rem_euclid(6.0)
    } else if (max - g).abs() < f32::EPSILON {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } * 60.0;
    (h, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let l = l.clamp(0.0, 1.0);
    if s <= 0.0 {
        let v = (l * 255.0).round() as u8;
        return (v, v, v);
    }
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = (h.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let to_u8 = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    (to_u8(r1), to_u8(g1), to_u8(b1))
}

/// Adapt a fully rendered frame buffer for the current theme mode.
///
/// No-op in dark mode. In light mode, flips each cell's colors and repairs
/// visible foreground/underline contrast without changing its adapted surface.
/// This is called once per frame after the UI has been drawn, so every widget
/// (transcript, markdown, pickers, overlays) is covered without needing
/// per-call-site changes.
pub fn adapt_buffer_for_theme(buf: &mut Buffer) {
    adapt_buffer(buf, theme_mode());
}

/// Final display pipeline. Attribute user overrides on the original colors,
/// before contrast repair can make distinct muted grays converge to one ink.
pub fn adapt_buffer_for_display(buf: &mut Buffer) {
    let palette = crate::palette::configured_palette();
    adapt_buffer_impl(buf, theme_mode(), palette.as_ref());
}

/// The same ordering for a foreground patched outside a full-frame redraw.
pub fn adapt_foreground_for_display(color: Color, background: Color) -> Color {
    if let Some(palette) = crate::palette::configured_palette()
        && let Some(chosen) = crate::palette::configured_native_color(&palette, color)
    {
        return chosen;
    }
    adapt_foreground_for_theme(color, background)
}

/// Explicit-mode variant of [`adapt_buffer_for_theme`]. Useful for tests and
/// callers that already resolved the mode.
pub fn adapt_buffer(buf: &mut Buffer, mode: ThemeMode) {
    adapt_buffer_impl(buf, mode, None);
}

fn adapt_buffer_impl(buf: &mut Buffer, mode: ThemeMode, palette: Option<&crate::palette::Palette>) {
    if mode != ThemeMode::Light && palette.is_none() {
        return;
    }
    // Frames contain few distinct colors; memoize the flip per unique color.
    let mut cache = std::collections::HashMap::new();
    let mut adapt = |c: Color| -> (Color, bool) {
        if c == Color::Reset {
            return (c, false);
        }
        *cache.entry(c).or_insert_with(|| {
            if let Some(chosen) =
                palette.and_then(|palette| crate::palette::configured_native_color(palette, c))
            {
                (chosen, true)
            } else {
                (
                    if mode == ThemeMode::Light {
                        adapt_color_for_light(c)
                    } else {
                        c
                    },
                    false,
                )
            }
        })
    };
    let mut foreground_cache = std::collections::HashMap::new();
    let mut readable = |color, background| {
        *foreground_cache
            .entry((color, background))
            .or_insert_with(|| readable_light_foreground(color, background))
    };
    for cell in buf.content.iter_mut() {
        let (fg, fg_override) = adapt(cell.fg);
        let (bg, bg_override) = adapt(cell.bg);
        let (underline, underline_override) = adapt(cell.underline_color);
        cell.fg = fg;
        cell.bg = bg;
        cell.underline_color = underline;
        if mode != ThemeMode::Light {
            continue;
        }
        if cell.modifier.contains(Modifier::REVERSED) {
            // With reverse video, the logical background is the visible ink.
            let surface = if cell.fg == Color::Reset {
                Color::Rgb(32, 32, 32)
            } else {
                cell.fg
            };
            if !bg_override {
                cell.bg = readable(cell.bg, surface);
            }
            if !underline_override {
                cell.underline_color = readable(cell.underline_color, surface);
            }
        } else {
            if !fg_override {
                cell.fg = readable(cell.fg, cell.bg);
            }
            if !underline_override {
                cell.underline_color = readable(cell.underline_color, cell.bg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    struct ThemeGuard;
    impl Drop for ThemeGuard {
        fn drop(&mut self) {
            set_theme_mode(ThemeMode::Dark);
        }
    }

    // The theme mode is a process-global; serialize tests that mutate it.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_light_theme(f: impl FnOnce()) {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = ThemeGuard;
        set_theme_mode(ThemeMode::Light);
        f();
    }

    fn as_rgb(c: Color) -> (u8, u8, u8) {
        match c {
            Color::Rgb(r, g, b) => (r, g, b),
            Color::Indexed(n) => crate::color::indexed_to_rgb(n),
            other => panic!("expected concrete color, got {other:?}"),
        }
    }

    fn luminance(c: Color) -> f32 {
        let (r, g, b) = as_rgb(c);
        let (_, _, l) = rgb_to_hsl(r, g, b);
        l
    }

    #[test]
    fn dark_mode_is_identity() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_theme_mode(ThemeMode::Dark);
        let c = Color::Rgb(138, 180, 248);
        assert_eq!(adapt_color_for_theme(c), c);
        assert_eq!(adapt_color_for_theme(Color::White), Color::White);
    }

    #[test]
    fn light_mode_flips_black_and_white() {
        with_light_theme(|| {
            assert_eq!(as_rgb(adapt_color_for_theme(Color::Rgb(0, 0, 0))).0, 255);
            assert_eq!(
                as_rgb(adapt_color_for_theme(Color::Rgb(255, 255, 255))),
                (0, 0, 0)
            );
        });
    }

    #[test]
    fn light_mode_darkens_light_palette_colors_preserving_hue() {
        with_light_theme(|| {
            // user_color: a light blue. Should become a dark blue (blue channel
            // still dominant) that reads on a white background.
            let adapted = adapt_color_for_theme(Color::Rgb(138, 180, 248));
            let (r, g, b) = as_rgb(adapted);
            assert!(
                b > r && b > g,
                "hue should stay blue-dominant, got ({r},{g},{b})"
            );
            assert!(
                luminance(adapted) < 0.45,
                "light blue should become dark on light bg, got ({r},{g},{b})"
            );
        });
    }

    #[test]
    fn light_mode_lightens_dark_backgrounds() {
        with_light_theme(|| {
            // user_bg: a dark navy panel. Should become a light tint.
            let adapted = adapt_color_for_theme(Color::Rgb(35, 40, 50));
            assert!(luminance(adapted) > 0.7, "dark bg should become light");
        });
    }

    #[test]
    fn light_mode_preserves_reset() {
        with_light_theme(|| {
            assert_eq!(adapt_color_for_theme(Color::Reset), Color::Reset);
        });
    }

    #[test]
    fn light_mode_maps_named_colors() {
        with_light_theme(|| {
            assert_eq!(as_rgb(adapt_color_for_theme(Color::White)), (0, 0, 0));
            assert_eq!(as_rgb(adapt_color_for_theme(Color::Black)), (255, 255, 255));
        });
    }

    #[test]
    fn adapt_buffer_rewrites_cells_only_in_light_mode() {
        let area = Rect::new(0, 0, 4, 1);
        let mut buf = Buffer::empty(area);
        for cell in buf.content.iter_mut() {
            cell.fg = Color::Rgb(245, 245, 255);
            cell.bg = Color::Rgb(35, 40, 50);
        }

        {
            let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            set_theme_mode(ThemeMode::Dark);
            let mut dark_buf = buf.clone();
            adapt_buffer_for_theme(&mut dark_buf);
            assert_eq!(dark_buf.content[0].fg, Color::Rgb(245, 245, 255));
        }

        with_light_theme(|| {
            let mut light_buf = buf.clone();
            adapt_buffer_for_theme(&mut light_buf);
            let fg = light_buf.content[0].fg;
            let bg = light_buf.content[0].bg;
            assert!(luminance(fg) < 0.3, "near-white text should become dark");
            assert!(luminance(bg) > 0.7, "dark panel bg should become light");
        });
    }

    #[test]
    fn muted_text_is_readable_on_off_white_without_darkening_the_surface() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        for cell in &mut buf.content {
            cell.fg = Color::Rgb(80, 80, 80);
            cell.underline_color = Color::Rgb(80, 80, 80);
        }
        buf.content[1].bg = Color::Rgb(35, 40, 50);
        adapt_buffer(&mut buf, ThemeMode::Light);
        assert_eq!(buf.content[0].bg, Color::Reset);
        assert_eq!(
            buf.content[1].bg,
            adapt_color_for_light(Color::Rgb(35, 40, 50))
        );
        for cell in &buf.content {
            let surface = color_rgb(cell.bg).unwrap_or(LIGHT_SURFACE);
            assert!(contrast(as_rgb(cell.fg), surface) >= TARGET_TEXT_CONTRAST);
            assert!(contrast(as_rgb(cell.underline_color), surface) >= TARGET_TEXT_CONTRAST);
            assert!(
                as_rgb(cell.fg).0 < 80,
                "muted labels must remain dark, not merely pass minimum contrast"
            );
        }
        assert!(
            contrast((175, 175, 175), LIGHT_SURFACE) < 2.0,
            "pin the original failure"
        );
    }

    #[test]
    fn stronger_light_contrast_rejects_the_previous_muted_ink() {
        assert!(contrast((99, 99, 99), LIGHT_SURFACE) < TARGET_TEXT_CONTRAST);
        let ink = readable_light_foreground(Color::Rgb(99, 99, 99), Color::Reset);
        assert!(contrast(as_rgb(ink), LIGHT_SURFACE) >= TARGET_TEXT_CONTRAST);
        assert!(as_rgb(ink).0 <= 71);
    }

    #[test]
    fn intermediate_surfaces_use_the_best_possible_contrast() {
        // Neither black nor white can reach 7:1 on a midtone surface.
        let surface = Color::Rgb(128, 128, 128);
        let ink = readable_light_foreground(surface, surface);
        assert_eq!(as_rgb(ink), (0, 0, 0));
    }

    #[test]
    fn every_tui_literal_has_readable_light_text_on_default_and_tinted_panels() {
        let literals: &[(u8, u8, u8)] = &include!("palette_literals.rs");
        for background in [
            Color::Reset,
            Color::Black,
            Color::Rgb(35, 40, 50),
            Color::Rgb(50, 50, 70),
            Color::Rgb(220, 220, 220),
        ] {
            let mut buf = Buffer::empty(Rect::new(0, 0, literals.len() as u16, 1));
            for (cell, &(r, g, b)) in buf.content.iter_mut().zip(literals) {
                cell.fg = Color::Rgb(r, g, b);
                cell.bg = background;
            }
            let mut dark = buf.clone();
            adapt_buffer(&mut dark, ThemeMode::Dark);
            assert_eq!(dark, buf, "dark palette must stay byte-identical");
            adapt_buffer(&mut buf, ThemeMode::Light);
            for (cell, original) in buf.content.iter().zip(literals) {
                let surface = color_rgb(cell.bg).unwrap_or(LIGHT_SURFACE);
                assert!(
                    contrast(as_rgb(cell.fg), surface) >= TARGET_TEXT_CONTRAST,
                    "{original:?} on {background:?} became {:?} on {:?}",
                    cell.fg,
                    cell.bg
                );
            }
        }
    }

    #[test]
    fn named_indexed_and_reverse_video_text_keep_contrast() {
        for color in [
            Color::DarkGray,
            Color::Yellow,
            Color::Green,
            Color::Indexed(244),
            Color::Indexed(250),
        ] {
            for reversed in [false, true] {
                let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
                let cell = &mut buf.content[0];
                cell.fg = color;
                cell.bg = Color::Black;
                if reversed {
                    cell.modifier.insert(Modifier::REVERSED);
                }
                adapt_buffer(&mut buf, ThemeMode::Light);
                let cell = &buf.content[0];
                let surface = as_rgb(if reversed { cell.fg } else { cell.bg });
                let attainable = contrast((0, 0, 0), surface)
                    .max(contrast((255, 255, 255), surface))
                    .min(TARGET_TEXT_CONTRAST);
                assert!(contrast(as_rgb(cell.fg), as_rgb(cell.bg)) >= attainable);
            }
        }
    }

    #[test]
    fn configured_panel_surface_drives_unconfigured_text_contrast() {
        let mut palette = crate::palette::Palette::default();
        palette.set(crate::palette::Role::UserBg, (32, 32, 32));
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        buf.content[0].fg = Color::Rgb(245, 245, 245);
        buf.content[0].bg = Color::Rgb(35, 40, 50);
        adapt_buffer_impl(&mut buf, ThemeMode::Light, Some(&palette));
        assert_eq!(buf.content[0].bg, crate::color::rgb(32, 32, 32));
        assert!(
            contrast(as_rgb(buf.content[0].fg), as_rgb(buf.content[0].bg)) >= TARGET_TEXT_CONTRAST
        );
    }

    #[test]
    fn explicit_low_contrast_choices_remain_exact_in_both_theme_modes() {
        let mut palette = crate::palette::Palette::default();
        palette.set(crate::palette::Role::Dim, (210, 210, 210));
        palette.set(crate::palette::Role::UserBg, (220, 220, 220));
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
            buf.content[0].fg = Color::Rgb(80, 80, 80);
            buf.content[0].bg = Color::Rgb(35, 40, 50);
            adapt_buffer_impl(&mut buf, mode, Some(&palette));
            assert_eq!(buf.content[0].fg, crate::color::rgb(210, 210, 210));
            assert_eq!(buf.content[0].bg, crate::color::rgb(220, 220, 220));
        }
    }

    #[test]
    fn hsl_round_trips_reasonably() {
        for (r, g, b) in [
            (0, 0, 0),
            (255, 255, 255),
            (138, 180, 248),
            (129, 199, 132),
            (255, 80, 80),
            (80, 80, 80),
        ] {
            let (h, s, l) = rgb_to_hsl(r, g, b);
            let (r2, g2, b2) = hsl_to_rgb(h, s, l);
            assert!(
                (r as i16 - r2 as i16).abs() <= 2
                    && (g as i16 - g2 as i16).abs() <= 2
                    && (b as i16 - b2 as i16).abs() <= 2,
                "round trip drifted: ({r},{g},{b}) -> ({r2},{g2},{b2})"
            );
        }
    }
}
