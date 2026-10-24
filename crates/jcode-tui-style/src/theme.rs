use crate::color;
use crate::color::rgb;
use ratatui::prelude::*;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicPtr, Ordering};

#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ThemeColor {
    Background,
    User,
    Ai,
    Tool,
    FileLink,
    Dim,
    Accent,
    SystemMessage,
    Queued,
    Asap,
    Pending,
    UserText,
    UserBg,
    InputText,
    InputBg,
    AiText,
    Bold,
    MarkdownText,
    HeaderIcon,
    HeaderName,
    HeaderSession,
    /// Success / additions.
    Success,
    /// Warnings.
    Warning,
    /// Errors / deletions.
    Error,
    /// Informational highlights.
    Info,
    /// Borders and rules.
    Border,
    /// Selected row background.
    SelectionBg,
}

const THEME_COLOR_COUNT: usize = ThemeColor::SelectionBg as usize + 1;

#[derive(Debug, Clone)]
pub struct Theme {
    name: String,
    colors: BTreeMap<ThemeColor, Color>,
}

impl Theme {
    fn new(name: impl Into<String>, colors: BTreeMap<ThemeColor, Color>) -> Self {
        Self {
            name: name.into(),
            colors,
        }
    }

    fn color(&self, key: ThemeColor) -> Color {
        self.colors.get(&key).copied().unwrap_or(Color::Reset)
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Debug, Deserialize)]
struct ThemeFile {
    colors: BTreeMap<String, String>,
}

const BUILTIN_THEMES: &[&str] = &["system", "light", "dark"];
static ACTIVE_THEME: OnceLock<AtomicPtr<ThemeSnapshot>> = OnceLock::new();

#[derive(Debug, Clone)]
struct ThemeSnapshot {
    name: String,
    colors: [Color; THEME_COLOR_COUNT],
    uses_terminal_adaptation: bool,
}

impl ThemeSnapshot {
    fn from_theme(theme: &Theme) -> Self {
        let name = theme.name().to_string();
        let uses_terminal_adaptation = theme_uses_terminal_adaptation(&name);
        Self {
            name,
            colors: theme_colors_array(theme),
            uses_terminal_adaptation,
        }
    }

    fn color(&self, key: ThemeColor) -> Color {
        self.colors[key as usize]
    }
}

fn active_theme() -> &'static AtomicPtr<ThemeSnapshot> {
    ACTIVE_THEME.get_or_init(|| AtomicPtr::new(std::ptr::null_mut()))
}

fn active_theme_snapshot() -> &'static ThemeSnapshot {
    let slot = active_theme();
    let current = slot.load(Ordering::Acquire);
    if !current.is_null() {
        // SAFETY: snapshots are intentionally leaked after publication so active
        // render readers can dereference them without a lock or epoch guard.
        return unsafe { &*current };
    }

    let snapshot = Box::into_raw(Box::new(ThemeSnapshot::from_theme(&system_theme())));
    match slot.compare_exchange(
        std::ptr::null_mut(),
        snapshot,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => unsafe { &*snapshot },
        Err(existing) => {
            // Another thread won initialization. Drop our unpublished snapshot.
            unsafe {
                drop(Box::from_raw(snapshot));
                &*existing
            }
        }
    }
}

pub fn active_theme_name() -> String {
    active_theme_snapshot().name.clone()
}

/// Whether the active palette should be post-processed by the terminal
/// light/dark adapter from `theme_mode`.
///
/// `system` and `light` reuse jcode's native palette plus master's rendered
/// buffer adapter. Custom TOML themes are explicit palettes, so adapting them
/// again would surprise users and can invert hand-picked colors.
pub fn active_theme_uses_terminal_adaptation() -> bool {
    active_theme_snapshot().uses_terminal_adaptation
}

fn theme_uses_terminal_adaptation(name: &str) -> bool {
    name.eq_ignore_ascii_case("system") || name.eq_ignore_ascii_case("light")
}

pub fn set_theme(name: &str, themes_dir: Option<&Path>) -> anyhow::Result<()> {
    let theme = load_theme(name, themes_dir)?;
    let snapshot = Box::into_raw(Box::new(ThemeSnapshot::from_theme(&theme)));
    // SAFETY: publish the fully-built snapshot with one AcqRel swap, mirroring
    // the init CAS above. Any snapshot the swap detaches stays leaked on
    // purpose: concurrent Acquire readers may still hold a reference to it, so
    // freeing it would race the next render pass. Theme changes are rare and
    // each snapshot is tiny, making the leak the safe trade.
    let _old = active_theme().swap(snapshot, Ordering::AcqRel);
    Ok(())
}

pub fn load_theme(name: &str, themes_dir: Option<&Path>) -> anyhow::Result<Theme> {
    let name = name.trim();
    match name.to_ascii_lowercase().as_str() {
        "" | "auto" | "system" => Ok(system_theme()),
        "light" => Ok(system_palette_named("light")),
        "dark" => Ok(dark_theme()),
        _ => load_custom_theme(name, themes_dir),
    }
}

pub fn available_theme_names(themes_dir: Option<&Path>) -> Vec<String> {
    let mut names = BUILTIN_THEMES
        .iter()
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    if let Some(dir) = themes_dir
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if !is_safe_custom_theme_name(stem) {
                continue;
            }
            if !names.iter().any(|name| name == stem) {
                names.push(stem.to_string());
            }
        }
    }
    names
}

// Build the snapshot color array indexed by ThemeColor discriminant.
// ALL_THEME_COLORS is count- and order-checked at compile time below, so
// every discriminant slot gets its own variant's color.
fn theme_colors_array(theme: &Theme) -> [Color; THEME_COLOR_COUNT] {
    std::array::from_fn(|i| theme.color(ALL_THEME_COLORS[i]))
}

// One entry per ThemeColor variant, in discriminant order. The declared
// length is THEME_COLOR_COUNT, so adding or removing a variant without
// updating this list fails to compile instead of silently miscoloring.
const ALL_THEME_COLORS: [ThemeColor; THEME_COLOR_COUNT] = [
    ThemeColor::Background,
    ThemeColor::User,
    ThemeColor::Ai,
    ThemeColor::Tool,
    ThemeColor::FileLink,
    ThemeColor::Dim,
    ThemeColor::Accent,
    ThemeColor::SystemMessage,
    ThemeColor::Queued,
    ThemeColor::Asap,
    ThemeColor::Pending,
    ThemeColor::UserText,
    ThemeColor::UserBg,
    ThemeColor::InputText,
    ThemeColor::InputBg,
    ThemeColor::AiText,
    ThemeColor::Bold,
    ThemeColor::MarkdownText,
    ThemeColor::HeaderIcon,
    ThemeColor::HeaderName,
    ThemeColor::HeaderSession,
    ThemeColor::Success,
    ThemeColor::Warning,
    ThemeColor::Error,
    ThemeColor::Info,
    ThemeColor::Border,
    ThemeColor::SelectionBg,
];

// Compile-time check that entry i is the variant with discriminant i.
// Rejects reordering and duplicates, so the array stays index-aligned with
// ThemeColor no matter how it is edited.
const _: () = {
    let mut i = 0;
    while i < ALL_THEME_COLORS.len() {
        assert!(
            ALL_THEME_COLORS[i] as usize == i,
            "ThemeColor index mismatch"
        );
        i += 1;
    }
};

fn themed_color(key: ThemeColor) -> Color {
    active_theme_snapshot().color(key)
}

fn load_custom_theme(name: &str, themes_dir: Option<&Path>) -> anyhow::Result<Theme> {
    if !is_safe_custom_theme_name(name) {
        anyhow::bail!(
            "Invalid theme name '{}': use only ASCII letters, numbers, '-' or '_'",
            name
        );
    }
    let dir = themes_dir.ok_or_else(|| anyhow::anyhow!("No themes directory configured"))?;
    let path = dir.join(format!("{name}.toml"));
    let content = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("Failed to read theme {}: {}", path.display(), e))?;
    let file: ThemeFile = toml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("Failed to parse theme {}: {}", path.display(), e))?;

    let mut theme = system_palette_named(name);
    for (raw_key, raw_value) in file.colors {
        let key = parse_theme_color(&raw_key).ok_or_else(|| {
            anyhow::anyhow!("Unknown theme color '{}': {}", raw_key, path.display())
        })?;
        let value = parse_color(&raw_value).ok_or_else(|| {
            anyhow::anyhow!("Invalid theme color '{}': {}", raw_value, path.display())
        })?;
        theme.colors.insert(key, value);
    }
    Ok(theme)
}

fn is_safe_custom_theme_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn parse_theme_color(raw: &str) -> Option<ThemeColor> {
    match raw.trim().replace('-', "_").to_ascii_lowercase().as_str() {
        "background" | "background_color" | "app_bg" | "app_background" => {
            Some(ThemeColor::Background)
        }
        "user" | "user_color" => Some(ThemeColor::User),
        "ai" | "ai_color" => Some(ThemeColor::Ai),
        "tool" | "tool_color" => Some(ThemeColor::Tool),
        "file_link" | "file_link_color" => Some(ThemeColor::FileLink),
        "dim" | "dim_color" => Some(ThemeColor::Dim),
        "accent" | "accent_color" => Some(ThemeColor::Accent),
        "system_message" | "system_message_color" => Some(ThemeColor::SystemMessage),
        "queued" | "queued_color" => Some(ThemeColor::Queued),
        "asap" | "asap_color" => Some(ThemeColor::Asap),
        "pending" | "pending_color" => Some(ThemeColor::Pending),
        "user_text" => Some(ThemeColor::UserText),
        "user_bg" => Some(ThemeColor::UserBg),
        "input_text" => Some(ThemeColor::InputText),
        "input_bg" => Some(ThemeColor::InputBg),
        "ai_text" => Some(ThemeColor::AiText),
        "bold" | "bold_color" => Some(ThemeColor::Bold),
        "markdown_text" | "md_text" => Some(ThemeColor::MarkdownText),
        "header_icon" | "header_icon_color" => Some(ThemeColor::HeaderIcon),
        "header_name" | "header_name_color" => Some(ThemeColor::HeaderName),
        "header_session" | "header_session_color" => Some(ThemeColor::HeaderSession),
        "success" | "success_color" => Some(ThemeColor::Success),
        "warning" | "warning_color" => Some(ThemeColor::Warning),
        "error" | "error_color" => Some(ThemeColor::Error),
        "info" | "info_color" => Some(ThemeColor::Info),
        "border" | "border_color" => Some(ThemeColor::Border),
        "selection_bg" | "selection_bg_color" => Some(ThemeColor::SelectionBg),
        _ => None,
    }
}

fn named_color(name: &str) -> Option<Color> {
    // ratatui named colors. Useful for users who want a terminal-quantized
    // color (256-color friendly) instead of an explicit RGB value.
    Some(match name {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        "white" => Color::White,
        _ => return None,
    })
}

fn parse_color(raw: &str) -> Option<Color> {
    let raw = raw.trim();
    if raw.eq_ignore_ascii_case("reset") || raw.eq_ignore_ascii_case("default") {
        return Some(Color::Reset);
    }
    // Named colors (red, blue, light-cyan, ...) for user convenience.
    if let Some(color) = named_color(&raw.to_ascii_lowercase().replace(['-', ' '], "")) {
        return Some(color);
    }
    let hex = raw.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    // Length and hex-digit checks above guarantee every 2-byte slice exists
    // and parses, so the invalid branch is unreachable.
    let channel = |pair: usize| -> u8 {
        u8::from_str_radix(hex.get(pair..pair + 2).expect("validated hex slice"), 16)
            .expect("ASCII hex digits always parse")
    };
    Some(Color::Rgb(channel(0), channel(2), channel(4)))
}

fn system_theme() -> Theme {
    system_palette_named("system")
}

fn system_palette_named(name: &str) -> Theme {
    Theme::new(
        name,
        BTreeMap::from([
            (ThemeColor::Background, Color::Reset),
            (ThemeColor::User, rgb(138, 180, 248)),
            (ThemeColor::Ai, rgb(129, 199, 132)),
            (ThemeColor::Tool, rgb(120, 120, 120)),
            (ThemeColor::FileLink, rgb(180, 200, 255)),
            (ThemeColor::Dim, rgb(80, 80, 80)),
            (ThemeColor::Accent, rgb(186, 139, 255)),
            (ThemeColor::SystemMessage, rgb(255, 170, 220)),
            (ThemeColor::Queued, rgb(255, 193, 7)),
            (ThemeColor::Asap, rgb(110, 210, 255)),
            (ThemeColor::Pending, rgb(140, 140, 140)),
            (ThemeColor::UserText, rgb(245, 245, 255)),
            (ThemeColor::UserBg, rgb(35, 40, 50)),
            (ThemeColor::InputText, Color::Reset),
            (ThemeColor::InputBg, Color::Reset),
            (ThemeColor::AiText, rgb(220, 220, 215)),
            (ThemeColor::Bold, rgb(240, 240, 235)),
            (ThemeColor::MarkdownText, rgb(200, 200, 195)),
            (ThemeColor::HeaderIcon, rgb(120, 210, 230)),
            (ThemeColor::HeaderName, rgb(190, 210, 235)),
            (ThemeColor::HeaderSession, rgb(255, 255, 255)),
            // Semantic accents. These mirror crate::palette::Role defaults so
            // the built-in theme is byte-identical to the historical palette,
            // but custom TOML themes can now override them independently.
            (ThemeColor::Success, rgb(100, 200, 100)),
            (ThemeColor::Warning, rgb(255, 200, 100)),
            (ThemeColor::Error, rgb(255, 100, 100)),
            (ThemeColor::Info, rgb(140, 180, 255)),
            (ThemeColor::Border, rgb(100, 100, 110)),
            (ThemeColor::SelectionBg, rgb(60, 60, 80)),
        ]),
    )
}

fn dark_theme() -> Theme {
    let mut theme = system_palette_named("dark");
    // Force an explicit dark background so the app is readable even on
    // terminals with a white/light default background. The system theme keeps
    // Color::Reset (terminal default) and relies on the buffer adapter for
    // light backgrounds; the explicit "dark" theme must guarantee a dark bg.
    theme.colors.insert(ThemeColor::Background, rgb(18, 18, 26));
    // Force input text to white so it is readable on the dark background.
    // Color::Reset inherits the terminal default fg, which may be dark or
    // low-contrast on some terminals, making typed text invisible.
    theme
        .colors
        .insert(ThemeColor::InputText, rgb(240, 240, 245));
    // Force an explicit dark input background too. The system theme keeps
    // Color::Reset and relies on the buffer adapter, but the explicit "dark"
    // theme must guarantee contrast: a light terminal default background would
    // make white input text invisible without an explicit dark InputBg.
    theme.colors.insert(ThemeColor::InputBg, rgb(18, 18, 26));
    theme
}

pub fn user_color() -> Color {
    themed_color(ThemeColor::User)
}
pub fn background_color() -> Color {
    themed_color(ThemeColor::Background)
}
pub fn ai_color() -> Color {
    themed_color(ThemeColor::Ai)
}
pub fn tool_color() -> Color {
    themed_color(ThemeColor::Tool)
}
pub fn file_link_color() -> Color {
    themed_color(ThemeColor::FileLink)
}
pub fn dim_color() -> Color {
    themed_color(ThemeColor::Dim)
}
pub fn accent_color() -> Color {
    themed_color(ThemeColor::Accent)
}
pub fn system_message_color() -> Color {
    themed_color(ThemeColor::SystemMessage)
}
pub fn queued_color() -> Color {
    themed_color(ThemeColor::Queued)
}
pub fn asap_color() -> Color {
    themed_color(ThemeColor::Asap)
}
pub fn pending_color() -> Color {
    themed_color(ThemeColor::Pending)
}
pub fn user_text() -> Color {
    themed_color(ThemeColor::UserText)
}
pub fn user_bg() -> Color {
    themed_color(ThemeColor::UserBg)
}
pub fn input_text() -> Color {
    themed_color(ThemeColor::InputText)
}
/// Ready-made style for input text lines, so callers do not each wrap
/// `input_text()` in a fresh `Style`.
pub fn input_text_style() -> Style {
    Style::default().fg(input_text())
}
pub fn input_bg() -> Color {
    themed_color(ThemeColor::InputBg)
}
pub fn ai_text() -> Color {
    themed_color(ThemeColor::AiText)
}
pub fn bold_color() -> Color {
    themed_color(ThemeColor::Bold)
}
pub fn markdown_text_color() -> Color {
    themed_color(ThemeColor::MarkdownText)
}
pub fn header_icon_color() -> Color {
    themed_color(ThemeColor::HeaderIcon)
}
pub fn header_name_color() -> Color {
    themed_color(ThemeColor::HeaderName)
}
pub fn header_session_color() -> Color {
    themed_color(ThemeColor::HeaderSession)
}

/// Semantic accents. These resolve through the theme system so custom TOML
/// themes can override them, falling back to the historical palette defaults
/// when unset. This is a deliberate design choice: the palette buffer-adapt pass
/// still re-expresses configured palette overrides, so these accessors return
/// the role default (not the configured palette color) to avoid double-substitution.
pub fn success_color() -> Color {
    themed_color(ThemeColor::Success)
}
pub fn warning_color() -> Color {
    themed_color(ThemeColor::Warning)
}
pub fn error_color() -> Color {
    themed_color(ThemeColor::Error)
}
pub fn info_color() -> Color {
    themed_color(ThemeColor::Info)
}
pub fn border_color() -> Color {
    themed_color(ThemeColor::Border)
}
pub fn selection_bg_color() -> Color {
    themed_color(ThemeColor::SelectionBg)
}

// Spinner frames for animated status. Keep these single-cell because the fast
// spinner-only renderer patches one status cell between full TUI redraws. This
// sequence should read as a circular spin, not a grow/recede pulse.
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Frame rate for slow, full-line "liveness" indicators that can only be
/// repainted by a full TUI redraw (e.g. the running-tool progress bar) when
/// decorative animations are disabled (Minimal tier, SSH, WSL, etc.). These
/// ride the ~1 Hz passive-liveness redraw, so advancing them faster would just
/// skip frames. Keep this slow so they read as alive without forcing more
/// expensive full-frame redraws.
pub const LIVENESS_INDICATOR_FPS: f32 = 1.5;

/// Frame rate for the low-cost single-cell circular spinner when decorative
/// animations are disabled. Unlike the full-line indicators above, this spinner
/// is patched by the cheap one-cell fast path between full redraws, so it can
/// animate at a smooth, responsive cadence (well above ~1 Hz) while still
/// staying very light on resources. Keep this in sync with the spinner-only
/// tick interval in the TUI run loop (`STATUS_SPINNER_ONLY_INTERVAL`, 80ms) so
/// each tick lands on exactly one new frame.
pub const LIVENESS_SPINNER_FPS: f32 = 12.5;

pub fn spinner_frame_index(elapsed: f32, fps: f32) -> usize {
    ((elapsed * fps) as usize) % SPINNER_FRAMES.len()
}

pub fn spinner_frame(elapsed: f32, fps: f32) -> &'static str {
    SPINNER_FRAMES[spinner_frame_index(elapsed, fps)]
}

/// Whether `symbol` is one of the cells owned by the primary activity spinner.
///
/// The TUI's single-cell spinner redraw uses this to avoid patching a status-row
/// cell after a late overlay, such as the slash-command palette, has taken
/// ownership of it.
pub fn is_activity_indicator_frame(symbol: &str) -> bool {
    SPINNER_FRAMES.contains(&symbol)
}

pub fn activity_indicator_frame_index(
    elapsed: f32,
    fps: f32,
    enable_decorative_animations: bool,
) -> usize {
    if enable_decorative_animations {
        spinner_frame_index(elapsed, fps)
    } else {
        // Keep ticking at the smooth liveness rate instead of freezing on a
        // single frame. The single-cell fast path repaints this cheaply, so it
        // can animate well above ~1 Hz without a full-frame redraw.
        spinner_frame_index(elapsed, LIVENESS_SPINNER_FPS)
    }
}

pub fn activity_indicator(
    elapsed: f32,
    fps: f32,
    enable_decorative_animations: bool,
) -> &'static str {
    SPINNER_FRAMES[activity_indicator_frame_index(elapsed, fps, enable_decorative_animations)]
}

/// Convert HSL to RGB (h in 0-360, s and l in 0-1)
/// Chroma color based on position and time - creates flowing rainbow wave
/// Calculate chroma color with fade-in from dim during startup
/// Calculate smooth animated color for the header (single color, no position)
pub fn color_to_floats(c: Color, fallback: (f32, f32, f32)) -> (f32, f32, f32) {
    match c {
        Color::Rgb(r, g, b) => (r as f32, g as f32, b as f32),
        Color::Indexed(n) => {
            let (r, g, b) = color::indexed_to_rgb(n);
            (r as f32, g as f32, b as f32)
        }
        _ => fallback,
    }
}

pub fn blend_color(from: Color, to: Color, t: f32) -> Color {
    let (fr, fg, fb) = color_to_floats(from, (80.0, 80.0, 80.0));
    let (tr, tg, tb) = color_to_floats(to, (200.0, 200.0, 200.0));
    let r = fr + (tr - fr) * t;
    let g = fg + (tg - fg) * t;
    let b = fb + (tb - fb) * t;
    rgb(
        r.clamp(0.0, 255.0) as u8,
        g.clamp(0.0, 255.0) as u8,
        b.clamp(0.0, 255.0) as u8,
    )
}

pub fn rainbow_prompt_color(distance: usize) -> Color {
    // Rainbow colors (hue progression): red -> orange -> yellow -> green -> cyan -> blue -> violet
    const RAINBOW: [(u8, u8, u8); 7] = [
        (255, 80, 80),   // Red (softened)
        (255, 160, 80),  // Orange
        (255, 230, 80),  // Yellow
        (80, 220, 100),  // Green
        (80, 200, 220),  // Cyan
        (100, 140, 255), // Blue
        (180, 100, 255), // Violet
    ];

    // Gray target (dim_color())
    const GRAY: (u8, u8, u8) = (80, 80, 80);

    // Exponential decay factor - how quickly we fade to gray
    // decay = e^(-distance * rate), rate of ~0.4 gives nice falloff
    let decay = (-0.4 * distance as f32).exp();

    // Select rainbow color based on distance (cycle through)
    let rainbow_idx = distance.min(RAINBOW.len() - 1);
    let (r, g, b) = RAINBOW[rainbow_idx];

    // Blend rainbow color with gray based on decay
    // At distance 0: 100% rainbow, as distance increases: approaches gray
    let blend = |rainbow: u8, gray: u8| -> u8 {
        (rainbow as f32 * decay + gray as f32 * (1.0 - decay)) as u8
    };

    rgb(blend(r, GRAY.0), blend(g, GRAY.1), blend(b, GRAY.2))
}

pub fn prompt_entry_color(base: Color, t: f32) -> Color {
    let peak = rgb(255, 230, 120);
    // Quick pulse in/out over the animation window.
    let phase = if t < 0.5 { t * 2.0 } else { (1.0 - t) * 2.0 };
    blend_color(base, peak, phase.clamp(0.0, 1.0) * 0.7)
}

pub fn prompt_entry_bg_color(base: Color, t: f32) -> Color {
    let spotlight = rgb(58, 66, 82);
    let ease_in = 1.0 - (1.0 - t).powi(3);
    let ease_out = (1.0 - t).powi(2);
    let phase = (ease_in * ease_out * 1.65).clamp(0.0, 1.0);
    blend_color(base, spotlight, phase * 0.85)
}

pub fn prompt_entry_shimmer_color(base: Color, pos: f32, t: f32) -> Color {
    let travel = (t * 1.15).clamp(0.0, 1.0);
    let width = 0.18;
    let dist = (pos - travel).abs();
    let shimmer = (1.0 - (dist / width).clamp(0.0, 1.0)).powf(2.2);
    let pulse = (1.0 - t).powf(0.55);
    let highlight = rgb(255, 248, 210);
    blend_color(base, highlight, shimmer * pulse * 0.7)
}

/// Generate an animated color that pulses between two colors
pub fn animated_tool_color(elapsed: f32, enable_decorative_animations: bool) -> Color {
    if !enable_decorative_animations {
        return tool_color();
    }

    // Cycle period of ~1.5 seconds
    let t = (elapsed * 2.0).sin() * 0.5 + 0.5; // 0.0 to 1.0

    // Interpolate between cyan and purple
    let r = (80.0 + t * 106.0) as u8; // 80 -> 186
    let g = (200.0 - t * 61.0) as u8; // 200 -> 139
    let b = (220.0 + t * 35.0) as u8; // 220 -> 255

    rgb(r, g, b)
}

#[cfg(test)]
#[path = "theme_tests.rs"]
mod tests;
