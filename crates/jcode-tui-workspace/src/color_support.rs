use ratatui::style::Color;
use std::sync::OnceLock;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorCapability {
    TrueColor,
    Color256,
}

static CAPABILITY: OnceLock<ColorCapability> = OnceLock::new();

pub fn color_capability() -> ColorCapability {
    *CAPABILITY.get_or_init(detect_color_capability)
}

/// Terminals whose GPU glyph atlas corrupts under heavy per-cell *truecolor*
/// churn (the macOS 26 "garbled glyphs" bug in the VS Code integrated terminal
/// and Apple Terminal; see issue #330 and `jcode_tui_style::color`). Capping
/// these to the 256-color palette bounds the distinct-color space the atlas
/// must cache, keeping markdown/mermaid colors readable. Mirrors the detection
/// in `jcode_tui_style::color::fragile_glyph_cache_terminal`. Overridable with
/// `JCODE_GLYPH_SAFE_MODE=on|off`.
fn fragile_glyph_cache_terminal() -> bool {
    if let Ok(raw) = std::env::var("JCODE_GLYPH_SAFE_MODE") {
        match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => return true,
            "0" | "false" | "no" | "off" => return false,
            _ => {}
        }
    }

    if !cfg!(target_os = "macos") {
        return false;
    }

    match std::env::var("TERM_PROGRAM") {
        Ok(tp) => {
            let tp = tp.to_ascii_lowercase();
            tp == "vscode" || tp == "apple_terminal"
        }
        Err(_) => false,
    }
}

fn detect_color_capability() -> ColorCapability {
    let raw = detect_raw_color_capability();
    // Downgrade truecolor to 256-color on fragile-glyph terminals so animated
    // colors quantize to a bounded palette instead of overflowing the GPU
    // glyph atlas (#330).
    if raw == ColorCapability::TrueColor && fragile_glyph_cache_terminal() {
        return ColorCapability::Color256;
    }
    raw
}

fn detect_raw_color_capability() -> ColorCapability {
    if let Ok(val) = std::env::var("COLORTERM") {
        let v = val.to_lowercase();
        if v == "truecolor" || v == "24bit" {
            return ColorCapability::TrueColor;
        }
    }

    if let Ok(term_program) = std::env::var("TERM_PROGRAM") {
        let tp = term_program.to_lowercase();
        if tp == "ghostty"
            || tp == "iterm.app"
            || tp == "wezterm"
            || tp == "warp"
            || tp == "alacritty"
            || tp == "hyper"
        {
            return ColorCapability::TrueColor;
        }
    }

    if std::env::var("GHOSTTY_RESOURCES_DIR").is_ok()
        || std::env::var("GHOSTTY_BIN_DIR").is_ok()
        || std::env::var("WEZTERM_EXECUTABLE").is_ok()
        || std::env::var("WEZTERM_PANE").is_ok()
    {
        return ColorCapability::TrueColor;
    }

    if let Ok(term) = std::env::var("TERM") {
        let t = term.to_lowercase();
        if t.contains("kitty") || t.contains("ghostty") || t.contains("alacritty") {
            return ColorCapability::TrueColor;
        }
        if t.contains("256color") {
            return ColorCapability::Color256;
        }
    }

    ColorCapability::Color256
}

pub fn has_truecolor() -> bool {
    color_capability() == ColorCapability::TrueColor
}

pub fn clear_buf(area: Rect, buf: &mut Buffer) {
    for x in area.left()..area.right() {
        for y in area.top()..area.bottom() {
            buf[(x, y)].reset();
        }
    }
}

#[inline]
pub fn rgb(r: u8, g: u8, b: u8) -> Color {
    if has_truecolor() {
        Color::Rgb(r, g, b)
    } else {
        Color::Indexed(rgb_to_xterm256(r, g, b))
    }
}

// The xterm-256 color cube: indices 16-231 map to a 6x6x6 RGB cube.
// Each axis uses values: 0, 95, 135, 175, 215, 255 (indices 0-5).
// Indices 232-255 are a grayscale ramp from rgb(8,8,8) to rgb(238,238,238).
fn rgb_to_xterm256(r: u8, g: u8, b: u8) -> u8 {
    let gray_avg = (r as u16 + g as u16 + b as u16) / 3;

    let cube_idx = nearest_cube_index(r, g, b);
    let cube_color = cube_index_to_rgb(cube_idx);
    let cube_dist = color_distance(r, g, b, cube_color.0, cube_color.1, cube_color.2);

    // Always evaluate the grayscale ramp candidate too and pick whichever is
    // perceptually closer. The previous `is_grayish` gate (all channels within
    // 15 of each other) excluded near-neutral colors whose channels happened to
    // span exactly 15, so subtle dark gray-blues like the user-prompt
    // background `rgb(35,40,50)` snapped to a saturated navy cube corner
    // (index 17 = `(0,0,95)`) on 256-color terminals such as Apple Terminal.
    // Comparing both candidates is strictly never worse and keeps these tones
    // reading as the intended neutral gray.
    let gray_idx = nearest_gray_index(gray_avg as u8);
    let gray_val = gray_index_to_value(gray_idx);
    let gray_dist = color_distance(r, g, b, gray_val, gray_val, gray_val);

    if gray_dist < cube_dist {
        return 232 + gray_idx;
    }

    cube_idx as u8 + 16
}

const CUBE_VALUES: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// Return the one or two cube axis indices whose value is nearest `v`. There
/// are exactly two when `v` sits at a midpoint between adjacent steps (e.g.
/// 115 is equidistant from 95 and 135); those are genuine ties that the older
/// code silently resolved toward the lower step.
fn nearest_cube_components(v: u8) -> ([u8; 2], usize) {
    let mut best = 0u8;
    let mut best_dist = u16::MAX;
    for (i, &cv) in CUBE_VALUES.iter().enumerate() {
        let d = (v as i16 - cv as i16).unsigned_abs();
        if d < best_dist {
            best_dist = d;
            best = i as u8;
        }
    }
    // Check whether the next step up ties the best distance.
    let next = best as usize + 1;
    if next < CUBE_VALUES.len() && (v as i16 - CUBE_VALUES[next] as i16).unsigned_abs() == best_dist
    {
        ([best, best + 1], 2)
    } else {
        ([best, best], 1)
    }
}

/// Hue scaled to 0..1530 (= 6 * 255) using only integer math, mirroring HSV
/// hue ordering. Achromatic colors return 0 so a gray candidate never looks
/// "hue-closer" to a tinted target than a same-hue cube color.
fn hue_scaled(r: u8, g: u8, b: u8) -> i32 {
    let (r, g, b) = (r as i32, g as i32, b as i32);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let chroma = max - min;
    if chroma == 0 {
        return 0;
    }
    let h = if max == r {
        ((g - b) * 255 / chroma).rem_euclid(1530)
    } else if max == g {
        (b - r) * 255 / chroma + 510
    } else {
        (r - g) * 255 / chroma + 1020
    };
    h.rem_euclid(1530)
}

fn hue_distance(target_hue: i32, r: u8, g: u8, b: u8) -> i32 {
    let d = (target_hue - hue_scaled(r, g, b)).abs();
    d.min(1530 - d)
}

/// Pick the nearest cube color, breaking exact per-channel ties in favor of the
/// candidate whose hue best matches the target (then higher chroma). All tie
/// candidates are equidistant under the weighted metric, so this never picks a
/// color farther from the target; it only stops light tints like
/// `rgb(190,210,235)` from collapsing onto a duller, hue-shifted neighbor
/// (e.g. teal `(175,215,215)` instead of light blue `(175,215,255)`) on
/// 256-color terminals such as Apple Terminal.
fn nearest_cube_index(r: u8, g: u8, b: u8) -> u16 {
    let (rs, rn) = nearest_cube_components(r);
    let (gs, gn) = nearest_cube_components(g);
    let (bs, bn) = nearest_cube_components(b);

    if rn == 1 && gn == 1 && bn == 1 {
        return rs[0] as u16 * 36 + gs[0] as u16 * 6 + bs[0] as u16;
    }

    let target_hue = hue_scaled(r, g, b);
    let mut best_idx = 0u16;
    let mut best_key = (i32::MAX, i32::MIN);
    for &ri in &rs[..rn] {
        for &gi in &gs[..gn] {
            for &bi in &bs[..bn] {
                let cr = CUBE_VALUES[ri as usize];
                let cg = CUBE_VALUES[gi as usize];
                let cb = CUBE_VALUES[bi as usize];
                let chroma = cr.max(cg).max(cb) as i32 - cr.min(cg).min(cb) as i32;
                let key = (hue_distance(target_hue, cr, cg, cb), -chroma);
                if key < best_key {
                    best_key = key;
                    best_idx = ri as u16 * 36 + gi as u16 * 6 + bi as u16;
                }
            }
        }
    }
    best_idx
}

fn cube_index_to_rgb(idx: u16) -> (u8, u8, u8) {
    let bi = (idx % 6) as usize;
    let gi = ((idx / 6) % 6) as usize;
    let ri = (idx / 36) as usize;
    (CUBE_VALUES[ri], CUBE_VALUES[gi], CUBE_VALUES[bi])
}

fn nearest_gray_index(v: u8) -> u8 {
    // Grayscale ramp: 232-255, values 8, 18, 28, ..., 238 (24 steps, step=10).
    // Use signed math so values just below the first ramp entry (1..=7) round
    // to index 0 instead of underflowing (`v - 8`).
    if v > 243 {
        return 23;
    }
    (((v as i16 - 8 + 5) / 10).clamp(0, 23)) as u8
}

fn gray_index_to_value(idx: u8) -> u8 {
    8 + idx * 10
}

fn color_distance(r1: u8, g1: u8, b1: u8, r2: u8, g2: u8, b2: u8) -> u32 {
    let dr = r1 as i32 - r2 as i32;
    let dg = g1 as i32 - g2 as i32;
    let db = b1 as i32 - b2 as i32;
    // Weighted Euclidean - human eye is more sensitive to green
    (2 * dr * dr + 4 * dg * dg + 3 * db * db) as u32
}

pub fn indexed_to_rgb(idx: u8) -> (u8, u8, u8) {
    if idx >= 232 {
        let v = gray_index_to_value(idx - 232);
        (v, v, v)
    } else if idx >= 16 {
        cube_index_to_rgb((idx - 16) as u16)
    } else {
        match idx {
            0 => (0, 0, 0),
            1 => (128, 0, 0),
            2 => (0, 128, 0),
            3 => (128, 128, 0),
            4 => (0, 0, 128),
            5 => (128, 0, 128),
            6 => (0, 128, 128),
            7 => (192, 192, 192),
            8 => (128, 128, 128),
            9 => (255, 0, 0),
            10 => (0, 255, 0),
            11 => (255, 255, 0),
            12 => (0, 0, 255),
            13 => (255, 0, 255),
            14 => (0, 255, 255),
            _ => (255, 255, 255),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pure_black() {
        let idx = rgb_to_xterm256(0, 0, 0);
        assert_eq!(idx, 16); // cube index 0,0,0
    }

    #[test]
    fn test_pure_white() {
        let idx = rgb_to_xterm256(255, 255, 255);
        assert_eq!(idx, 231); // cube index 5,5,5
    }

    #[test]
    fn test_mid_gray() {
        let idx = rgb_to_xterm256(128, 128, 128);
        // Should pick grayscale 243 (value 128) or nearby
        assert!(
            (232..=255).contains(&u16::from(idx)),
            "Expected grayscale, got {}",
            idx
        );
    }

    #[test]
    fn test_dim_gray() {
        let idx = rgb_to_xterm256(80, 80, 80);
        assert!(
            (232..=255).contains(&u16::from(idx)),
            "Expected grayscale for dim, got {}",
            idx
        );
    }

    #[test]
    fn test_red() {
        let idx = rgb_to_xterm256(255, 0, 0);
        assert_eq!(idx, 196); // cube 5,0,0
    }

    #[test]
    fn test_green() {
        let idx = rgb_to_xterm256(0, 255, 0);
        assert_eq!(idx, 46); // cube 0,5,0
    }

    #[test]
    fn test_blue() {
        let idx = rgb_to_xterm256(0, 0, 255);
        assert_eq!(idx, 21); // cube 0,0,5
    }

    #[test]
    fn test_rgb_truecolor() {
        // When we have truecolor, rgb() should return Color::Rgb
        // (can't easily test since it depends on env, but test the mapper)
        let color = Color::Indexed(rgb_to_xterm256(138, 180, 248));
        match color {
            Color::Indexed(n) => assert!(n >= 16, "Should be extended color"),
            _ => panic!("Expected indexed color"),
        }
    }

    #[test]
    fn test_near_colors_are_stable() {
        let a = rgb_to_xterm256(80, 80, 80);
        let b = rgb_to_xterm256(82, 82, 82);
        assert_eq!(a, b, "Similar grays should map to same index");
    }

    /// Regression for the Apple Terminal "navy user prompt" bug: the subtle
    /// dark gray-blue user-prompt background `rgb(35,40,50)` must quantize to a
    /// neutral grayscale ramp entry, not a saturated navy cube corner
    /// (index 17 = `(0,0,95)`). Its channels span exactly 15, which the old
    /// `is_grayish` gate (`< 15`) excluded, snapping it to navy on 256-color
    /// terminals.
    #[test]
    fn test_near_neutral_dark_blue_quantizes_to_gray_not_navy() {
        let idx = rgb_to_xterm256(35, 40, 50);
        assert_ne!(idx, 17, "must not snap to saturated navy (0,0,95)");
        assert!(
            (232..=255).contains(&u16::from(idx)),
            "near-neutral dark tone should map to the grayscale ramp, got {idx}"
        );
        let (r, g, b) = indexed_to_rgb(idx);
        assert_eq!((r, g, b), (38, 38, 38), "expected neutral gray ramp entry");
    }

    /// Light blue tints whose blue channel sits exactly between cube steps
    /// (e.g. `header_name` = `rgb(190,210,235)`, blue 235 ties 215/255) must
    /// keep their blue cast instead of collapsing onto the duller teal
    /// neighbor `(175,215,215)`. The old code always rounded ties down, which
    /// dropped blue to equal green and shifted the hue ~33 degrees toward cyan
    /// on 256-color terminals such as Apple Terminal.
    #[test]
    fn test_light_blue_tint_keeps_blue_cast_on_tie() {
        let idx = rgb_to_xterm256(190, 210, 235);
        let (r, g, b) = indexed_to_rgb(idx);
        assert_eq!(
            (r, g, b),
            (175, 215, 255),
            "light blue should stay blue, not become teal (175,215,215)"
        );
        assert!(b > g, "blue channel must remain dominant over green");
    }
}

#[cfg(test)]
mod fragile_glyph_tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn temp_env_scope(vars: &[(&str, Option<&str>)], body: impl FnOnce()) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let saved: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|(k, _)| ((*k).to_string(), std::env::var(k).ok()))
            .collect();
        for (k, v) in vars {
            match v {
                Some(val) => unsafe { std::env::set_var(k, val) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
        body();
        for (k, v) in saved {
            match v {
                Some(val) => unsafe { std::env::set_var(&k, val) },
                None => unsafe { std::env::remove_var(&k) },
            }
        }
    }

    #[test]
    fn override_off_forces_truecolor() {
        temp_env_scope(
            &[
                ("JCODE_GLYPH_SAFE_MODE", Some("off")),
                ("TERM_PROGRAM", Some("vscode")),
            ],
            || assert!(!fragile_glyph_cache_terminal()),
        );
    }

    #[test]
    fn override_on_forces_quantize() {
        temp_env_scope(&[("JCODE_GLYPH_SAFE_MODE", Some("on"))], || {
            assert!(fragile_glyph_cache_terminal())
        });
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn detects_vscode_and_apple_terminal() {
        temp_env_scope(
            &[
                ("JCODE_GLYPH_SAFE_MODE", None),
                ("TERM_PROGRAM", Some("vscode")),
            ],
            || assert!(fragile_glyph_cache_terminal()),
        );
        temp_env_scope(
            &[
                ("JCODE_GLYPH_SAFE_MODE", None),
                ("TERM_PROGRAM", Some("Apple_Terminal")),
            ],
            || assert!(fragile_glyph_cache_terminal()),
        );
        temp_env_scope(
            &[
                ("JCODE_GLYPH_SAFE_MODE", None),
                ("TERM_PROGRAM", Some("ghostty")),
            ],
            || assert!(!fragile_glyph_cache_terminal()),
        );
    }
}
