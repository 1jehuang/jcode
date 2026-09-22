//! Fully user-configurable TUI colors.
//!
//! The TUI's colors come from two places:
//!
//! 1. Named semantic roles ([`Role`]) used by `theme.rs` accessors
//!    (`user_color()`, `ai_color()`, ...).
//! 2. Hundreds of ad hoc `rgb(r, g, b)` literals scattered across widgets.
//!
//! Both funnel through this module. Once per frame,
//! [`crate::theme_mode::adapt_buffer_for_display`] rewrites any buffer color
//! that is exactly a role's default onto that role's configured color, and maps
//! ratatui's named colors to the role they conventionally stand for. Ad hoc
//! `rgb(...)` literals carry no role, so they are not configurable: give a shade
//! a role if it needs to follow `/colors`. An unconfigured palette is
//! byte-identical to the historical hard-coded look.
//!
//! Configuration lives in `~/.jcode/config.toml`:
//!
//! ```toml
//! [display.colors]
//! user = "#8ab4f8"
//! ai = "#81c784"
//! accent = "#ba8bff"
//! ```

use ratatui::style::Color;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// A semantic color slot in the TUI.
///
/// Every role has a built-in default equal to the historical hard-coded value,
/// so adding a role never changes the default look.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    /// User message text accent.
    User,
    /// Assistant message accent.
    Ai,
    /// Tool row label color.
    Tool,
    /// Clickable file paths.
    FileLink,
    /// Low-emphasis text (hints, separators).
    Dim,
    /// Primary brand accent (headers, highlights).
    Accent,
    /// System / harness notices.
    System,
    /// Queued-prompt indicator.
    Queued,
    /// ASAP-priority indicator.
    Asap,
    /// Pending / not-yet-run indicator.
    Pending,
    /// User message foreground text.
    UserText,
    /// User message panel background.
    UserBg,
    /// Assistant message foreground text.
    AiText,
    /// Header session icon.
    HeaderIcon,
    /// Header model/agent name.
    HeaderName,
    /// Header session id.
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

/// All roles, in declaration order. Used by `/colors` listings and harmony
/// analysis so a new role is automatically covered by both.
pub const ALL_ROLES: &[Role] = &[
    Role::User,
    Role::Ai,
    Role::Tool,
    Role::FileLink,
    Role::Dim,
    Role::Accent,
    Role::System,
    Role::Queued,
    Role::Asap,
    Role::Pending,
    Role::UserText,
    Role::UserBg,
    Role::AiText,
    Role::HeaderIcon,
    Role::HeaderName,
    Role::HeaderSession,
    Role::Success,
    Role::Warning,
    Role::Error,
    Role::Info,
    Role::Border,
    Role::SelectionBg,
];

impl Role {
    /// Stable config key (also the `/colors` name).
    pub const fn key(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Ai => "ai",
            Role::Tool => "tool",
            Role::FileLink => "file_link",
            Role::Dim => "dim",
            Role::Accent => "accent",
            Role::System => "system",
            Role::Queued => "queued",
            Role::Asap => "asap",
            Role::Pending => "pending",
            Role::UserText => "user_text",
            Role::UserBg => "user_bg",
            Role::AiText => "ai_text",
            Role::HeaderIcon => "header_icon",
            Role::HeaderName => "header_name",
            Role::HeaderSession => "header_session",
            Role::Success => "success",
            Role::Warning => "warning",
            Role::Error => "error",
            Role::Info => "info",
            Role::Border => "border",
            Role::SelectionBg => "selection_bg",
        }
    }

    /// Look up a role by its config key (case/separator insensitive).
    pub fn from_key(key: &str) -> Option<Role> {
        let normalized = key.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        ALL_ROLES
            .iter()
            .copied()
            .find(|role| role.key() == normalized)
    }

    /// Built-in default RGB, matching jcode's historical hard-coded palette.
    pub const fn default_rgb(self) -> (u8, u8, u8) {
        match self {
            Role::User => (138, 180, 248),
            Role::Ai => (129, 199, 132),
            Role::Tool => (120, 120, 120),
            Role::FileLink => (180, 200, 255),
            Role::Dim => (80, 80, 80),
            Role::Accent => (186, 139, 255),
            Role::System => (255, 170, 220),
            Role::Queued => (255, 193, 7),
            Role::Asap => (110, 210, 255),
            Role::Pending => (140, 140, 140),
            Role::UserText => (245, 245, 255),
            Role::UserBg => (35, 40, 50),
            Role::AiText => (220, 220, 215),
            Role::HeaderIcon => (120, 210, 230),
            Role::HeaderName => (190, 210, 235),
            Role::HeaderSession => (255, 255, 255),
            Role::Success => (100, 200, 100),
            Role::Warning => (255, 200, 100),
            Role::Error => (255, 100, 100),
            Role::Info => (140, 180, 255),
            Role::Border => (100, 100, 110),
            Role::SelectionBg => (60, 60, 80),
        }
    }

    /// Whether this role is used as a background. Backgrounds are graded on
    /// different harmony criteria than foreground text.
    pub const fn is_background(self) -> bool {
        matches!(self, Role::UserBg | Role::SelectionBg)
    }
}

/// A complete set of role colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    entries: [(u8, u8, u8); ALL_ROLES.len()],
    /// Which roles the user explicitly configured. Only overridden roles
    /// participate in literal remapping, so a default palette is a no-op.
    overridden: [bool; ALL_ROLES.len()],
}

impl Default for Palette {
    fn default() -> Self {
        let mut entries = [(0, 0, 0); ALL_ROLES.len()];
        for (slot, role) in entries.iter_mut().zip(ALL_ROLES) {
            *slot = role.default_rgb();
        }
        Self {
            entries,
            overridden: [false; ALL_ROLES.len()],
        }
    }
}

/// Index of `role` within [`ALL_ROLES`].
///
/// A missing role would be a programming error (a variant added without listing
/// it), but this sits on the render path, so it must not panic a live session
/// over a palette lookup. Falling back to index 0 renders one role with another
/// role's color, which is cosmetic; `every_role_is_indexable` catches the
/// mistake in CI instead.
fn index_of(role: Role) -> usize {
    ALL_ROLES
        .iter()
        .position(|candidate| *candidate == role)
        .unwrap_or(0)
}

impl Palette {
    /// RGB for `role`.
    pub fn rgb(&self, role: Role) -> (u8, u8, u8) {
        self.entries[index_of(role)]
    }

    /// Ratatui color for `role`, quantized for 256-color terminals.
    pub fn color(&self, role: Role) -> Color {
        let (r, g, b) = self.rgb(role);
        crate::color::rgb(r, g, b)
    }

    /// Whether the user explicitly set `role`.
    pub fn is_overridden(&self, role: Role) -> bool {
        self.overridden[index_of(role)]
    }

    /// Override `role`.
    pub fn set(&mut self, role: Role, rgb: (u8, u8, u8)) {
        let index = index_of(role);
        self.entries[index] = rgb;
        self.overridden[index] = true;
    }

    /// Whether any role is overridden (fast path guard for remapping).
    pub fn has_overrides(&self) -> bool {
        self.overridden.iter().any(|flag| *flag)
    }

    /// Build a palette from `key = "#rrggbb"` pairs, returning per-entry
    /// errors instead of failing the whole palette so one typo cannot make the
    /// TUI unstyled.
    pub fn from_pairs<'a, I>(pairs: I) -> (Self, Vec<String>)
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        let mut palette = Self::default();
        let mut errors = Vec::new();
        for (key, value) in pairs {
            match (Role::from_key(key), parse_hex(value)) {
                (Some(role), Some(rgb)) => palette.set(role, rgb),
                (None, _) => errors.push(format!("unknown color role '{key}'")),
                (Some(role), None) => errors.push(format!(
                    "invalid color '{value}' for '{}' (expected #rrggbb)",
                    role.key()
                )),
            }
        }
        (palette, errors)
    }
}

/// Parse `#rrggbb`, `#rgb`, or a bare `rrggbb` hex string.
pub fn parse_hex(text: &str) -> Option<(u8, u8, u8)> {
    let hex = text.trim().trim_start_matches('#');
    // `from_str_radix` failing here just means "not a hex color", which callers
    // surface to the user as an invalid-color message, so the error value itself
    // carries nothing extra.
    let byte = |slice: &str| u8::from_str_radix(slice, 16).ok();
    match hex.len() {
        3 => {
            let mut chars = hex.chars();
            let mut next = || {
                let c = chars.next()?;
                byte(&format!("{c}{c}"))
            };
            Some((next()?, next()?, next()?))
        }
        6 => Some((byte(&hex[0..2])?, byte(&hex[2..4])?, byte(&hex[4..6])?)),
        _ => None,
    }
}

/// Format an RGB triple as `#rrggbb`.
pub fn to_hex((r, g, b): (u8, u8, u8)) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

static ACTIVE: RwLock<Option<Palette>> = RwLock::new(None);

/// Lock-free fast path for the per-cell substitution, which sits on the render
/// hot path. Palettes without overrides (the default) must not pay a lock.
static HAS_OVERRIDES: AtomicBool = AtomicBool::new(false);

/// Install the active palette. Called once at startup from config, and again
/// when the user changes colors at runtime.
pub fn set_palette(palette: Palette) {
    // Recover from a poisoned lock rather than silently keeping the old
    // palette: a panic elsewhere must not leave the user's configured colors
    // permanently unapplied for the rest of the session.
    let mut active = ACTIVE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *active = Some(palette);
    drop(active);
    HAS_OVERRIDES.store(palette.has_overrides(), Ordering::Relaxed);
}

/// The active palette.
///
/// Before configuration is loaded this is the built-in palette, which is what
/// every historical call site rendered, so an unconfigured session looks exactly
/// as it always has.
pub fn palette() -> Palette {
    // `unwrap_or_default` here is the built-in palette, not a swallowed error:
    // `None` means "config has not been loaded yet", and the default palette is
    // exactly what every historical call site rendered.
    (*ACTIVE
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner()))
    .unwrap_or_default()
}

/// Avoid locking/copying the palette on the default render path.
pub(crate) fn configured_palette() -> Option<Palette> {
    HAS_OVERRIDES.load(Ordering::Relaxed).then(palette)
}

/// Resolve an override from the original, native-palette color, before light
/// contrast repair can collapse two distinct muted roles to the same ink.
///
/// A color is attributed only when it *is* a role's default (or a named color
/// with a role), so an override can never recolor a color some other role
/// carries. Ad hoc `rgb(...)` literals are unattributed and returned as-is:
/// give a shade a role if it should follow `/colors`.
pub(crate) fn configured_native_color(palette: &Palette, color: Color) -> Option<Color> {
    let rgb = match color {
        Color::Reset => return None,
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(index) => crate::color::indexed_to_rgb(index),
        named => {
            let mapped = remap_named_with(palette, named);
            return (mapped != named).then_some(mapped);
        }
    };
    // Only the role whose own default this is may claim it.
    let role = ALL_ROLES
        .iter()
        .copied()
        .find(|role| role.default_rgb() == rgb && palette.is_overridden(*role))?;
    let (r, g, b) = palette.rgb(role);
    Some(crate::color::rgb(r, g, b))
}

/// Resolve a role to a renderable color.
///
/// This deliberately returns the role's *default* color, not the configured
/// one: substitution happens once per frame in
/// [`crate::theme_mode::adapt_buffer_for_display`]. Returning the configured
/// color here would let
/// the same cell be remapped twice (once by the accessor, once by the buffer
/// pass), which compounds the hue/lightness offsets.
pub fn role_color(role: Role) -> Color {
    let (r, g, b) = role.default_rgb();
    crate::color::rgb(r, g, b)
}

/// Map a terminal-named color (`Color::White`, `Color::Red`, ...) onto the
/// configured palette.
///
/// Widgets also use ratatui's named colors, which carry no RGB for literal
/// matching. Named colors are mapped to the semantic role they conventionally
/// stand for, and left untouched when that role is not configured, so default
/// behavior is unchanged.
pub fn remap_named_with(palette: &Palette, color: Color) -> Color {
    let role = match color {
        Color::Red | Color::LightRed => Role::Error,
        Color::Green | Color::LightGreen => Role::Success,
        Color::Yellow | Color::LightYellow => Role::Warning,
        Color::Blue | Color::LightBlue => Role::Info,
        Color::Magenta | Color::LightMagenta => Role::Accent,
        Color::Cyan | Color::LightCyan => Role::HeaderIcon,
        Color::White => Role::AiText,
        Color::Gray | Color::DarkGray => Role::Dim,
        // Used as a panel/inverse background rather than as text.
        Color::Black => Role::UserBg,
        // `Reset` is the terminal's own default and must stay untouched, which
        // is what lets the user's real background show through.
        _ => return color,
    };
    if !palette.is_overridden(role) {
        return color;
    }
    let (r, g, b) = palette.rgb(role);
    crate::color::rgb(r, g, b)
}

/// Maximum perceptual distance (in oklab units) at which a rendered color is
/// considered "an instance of" a role color, for the measurement helper
/// [`role_for_rendered`]. Roughly a same-hue family match.
const FAMILY_RADIUS: f32 = 0.16;

/// Which role a rendered color belongs to, if any.
///
/// Measurement only, for tooling that attributes *rendered frames* back to
/// roles. Measuring which roles actually cover and touch each other on screen is
/// what [`crate::harmony::graph`] needs, and hard-coding that layout by hand
/// would encode an assumption about the UI instead of an observation of it. It
/// is not on the render path and never applies an override.
pub fn role_for_rendered(color: Color) -> Option<Role> {
    let rgb = match color {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(index) => crate::color::indexed_to_rgb(index),
        _ => return None,
    };
    let source = crate::harmony::Oklab::from_rgb(rgb);
    let mut best: Option<(f32, Role)> = None;
    for role in ALL_ROLES.iter().copied() {
        let distance = source.distance(crate::harmony::Oklab::from_rgb(role.default_rgb()));
        if distance <= FAMILY_RADIUS && best.is_none_or(|(previous, _)| distance < previous) {
            best = Some((distance, role));
        }
    }
    best.map(|(_, role)| role)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_role_has_a_unique_key_and_roundtrips() {
        let mut keys = std::collections::HashSet::new();
        for role in ALL_ROLES.iter().copied() {
            assert!(keys.insert(role.key()), "duplicate key {}", role.key());
            assert_eq!(Role::from_key(role.key()), Some(role));
            // Accept the friendlier dashed/uppercase spellings too.
            assert_eq!(Role::from_key(&role.key().replace('_', "-")), Some(role));
        }
    }

    /// `index_of` falls back instead of panicking on the render path, so the
    /// invariant it relies on is enforced here.
    #[test]
    fn every_role_is_indexable() {
        for (expected, role) in ALL_ROLES.iter().copied().enumerate() {
            assert_eq!(
                index_of(role),
                expected,
                "{} must be listed in ALL_ROLES at its own index",
                role.key()
            );
        }
    }

    #[test]
    fn parses_hex_in_common_spellings() {
        assert_eq!(parse_hex("#8ab4f8"), Some((138, 180, 248)));
        assert_eq!(parse_hex("8AB4F8"), Some((138, 180, 248)));
        assert_eq!(parse_hex("#fff"), Some((255, 255, 255)));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("nope"), None);
    }

    #[test]
    fn default_palette_matches_historical_values() {
        let palette = Palette::default();
        assert_eq!(palette.rgb(Role::User), (138, 180, 248));
        assert!(!palette.has_overrides());
    }

    /// The contract, in one place: only a color that *is* a role's default is
    /// attributed to that role. Ad hoc shades carry no role and are left alone,
    /// which is the trade this design makes: configurable means role-tagged.
    #[test]
    fn only_exact_role_defaults_follow_an_override() {
        let mut palette = Palette::default();
        palette.set(Role::Warning, (80, 220, 120));
        let (r, g, b) = Role::Warning.default_rgb();
        assert_eq!(
            configured_native_color(&palette, Color::Rgb(r, g, b)),
            Some(crate::color::rgb(80, 220, 120))
        );
        // An amber shade near the warning default is a plain literal, not the role.
        assert_eq!(
            configured_native_color(&palette, Color::Rgb(200, 150, 70)),
            None
        );
        // An unconfigured palette attributes nothing, so the default frame is
        // untouched.
        assert_eq!(
            configured_native_color(&Palette::default(), Color::Rgb(r, g, b)),
            None
        );
    }

    #[test]
    fn from_pairs_reports_errors_without_dropping_valid_entries() {
        let (palette, errors) = Palette::from_pairs([
            ("accent", "#ff0000"),
            ("bogus", "#00ff00"),
            ("ai", "not-a-color"),
        ]);
        assert_eq!(palette.rgb(Role::Accent), (255, 0, 0));
        assert_eq!(palette.rgb(Role::Ai), Role::Ai.default_rgb());
        assert_eq!(errors.len(), 2);
    }
}

#[cfg(test)]
mod buffer_tests {
    use super::*;
    use crate::theme_mode::{ThemeMode, adapt_buffer_for_display};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    /// The active palette is process-global, so palette tests must not run
    /// concurrently with each other.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Install `palette` for the duration of `body`, always restoring the
    /// default so a failure cannot leak state into another test.
    fn with_palette(palette: Palette, body: impl FnOnce()) {
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                set_palette(Palette::default());
            }
        }
        let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _restore = Restore;
        set_palette(palette);
        body();
    }

    fn buffer_with(colors: &[Color]) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, colors.len() as u16, 1));
        for (cell, color) in buf.content.iter_mut().zip(colors) {
            cell.fg = *color;
        }
        buf
    }

    // The default palette must render byte-identically to the historical
    // hard-coded look. This is the regression that would silently recolor
    // every existing user's terminal.
    #[test]
    fn default_palette_leaves_the_frame_untouched() {
        with_palette(Palette::default(), || {
            crate::theme_mode::set_theme_mode(ThemeMode::Dark);
            let original = buffer_with(&[
                Color::Rgb(255, 200, 100),
                Color::White,
                Color::Indexed(42),
                Color::Reset,
            ]);
            let mut adapted = original.clone();
            adapt_buffer_for_display(&mut adapted);
            assert_eq!(adapted, original);
        });
    }

    #[test]
    fn configured_role_recolors_role_cells_and_named_colors_only() {
        let mut palette = Palette::default();
        palette.set(Role::Error, (10, 80, 240));
        with_palette(palette, || {
            crate::theme_mode::set_theme_mode(ThemeMode::Dark);
            let mut buf = buffer_with(&[
                role_color(Role::Error), // the role's own output
                Color::Red,              // the named stand-in for error
                Color::Rgb(40, 200, 90), // unrelated green, no role
                Color::Reset,
            ]);
            adapt_buffer_for_display(&mut buf);

            assert_eq!(buf.content[0].fg, crate::color::rgb(10, 80, 240));
            assert_eq!(buf.content[1].fg, crate::color::rgb(10, 80, 240));
            assert_eq!(
                buf.content[2].fg,
                crate::color::rgb(40, 200, 90),
                "an untagged literal must not follow any role"
            );
            assert_eq!(buf.content[3].fg, Color::Reset, "Reset must be preserved");
        });
    }

    // Applying the pass twice must be a no-op beyond the first, otherwise a
    // double-render path would compound shifts.
    #[test]
    fn palette_substitution_is_idempotent() {
        let mut palette = Palette::default();
        palette.set(Role::Warning, (90, 220, 130));
        with_palette(palette, || {
            crate::theme_mode::set_theme_mode(ThemeMode::Dark);
            let mut once = buffer_with(&[role_color(Role::Warning)]);
            adapt_buffer_for_display(&mut once);
            let mut twice = once.clone();
            adapt_buffer_for_display(&mut twice);
            assert_eq!(
                once, twice,
                "a second palette pass must not shift colors again"
            );
        });
    }
}

#[cfg(test)]
mod light_theme_interaction {
    use super::*;
    use crate::theme_mode::{ThemeMode, adapt_buffer_for_display};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    /// A user on a light terminal who configures a dark, readable color must get
    /// that color, not its inverse.
    ///
    /// The light adapter exists because jcode's *built-in* palette is designed
    /// for dark backgrounds, so it flips luminance to make that palette work on
    /// light terminals. A color the user chose explicitly is already the color
    /// they want, so flipping it turns a readable dark red into an unreadable
    /// pale one. This is the ordering bug that pipeline is prone to, so pin the
    /// behavior.
    #[test]
    fn configured_colors_survive_the_light_theme_pass() {
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                set_palette(Palette::default());
                crate::theme_mode::set_theme_mode(ThemeMode::Dark);
            }
        }
        let _restore = Restore;
        // The buffer pass reads the global theme mode, so set it to the mode
        // being exercised.
        crate::theme_mode::set_theme_mode(ThemeMode::Light);

        // A dark red: exactly what a user would pick for errors on white.
        let chosen = (171, 60, 58);
        let mut palette = Palette::default();
        palette.set(Role::Error, chosen);
        set_palette(palette);

        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        buf.content[0].fg = Color::Rgb(255, 100, 100); // the error default
        // The actual display pipeline attributes overrides before contrast repair.
        adapt_buffer_for_display(&mut buf);

        let rendered = match buf.content[0].fg {
            Color::Rgb(r, g, b) => (r, g, b),
            Color::Indexed(index) => crate::color::indexed_to_rgb(index),
            other => panic!("expected a concrete color, got {other:?}"),
        };
        assert_eq!(
            rendered, chosen,
            "the user's configured color must reach the terminal unmodified"
        );

        // These three native grays all need contrast repair. Matching after
        // clamping loses their identities and sends all three to Tool's color.
        let roles = [
            (Role::Tool, (171, 60, 58)),
            (Role::Dim, (20, 80, 100)),
            (Role::Pending, (125, 64, 110)),
        ];
        let mut palette = Palette::default();
        for (role, chosen) in roles {
            palette.set(role, chosen);
        }
        palette.set(Role::UserBg, (232, 235, 238));
        set_palette(palette);
        for background in [Color::Reset, role_color(Role::UserBg)] {
            let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
            for (cell, (role, _)) in buf.content.iter_mut().zip(roles) {
                cell.fg = role_color(role);
                cell.bg = background;
            }
            adapt_buffer_for_display(&mut buf);
            for (cell, (_, (r, g, b))) in buf.content.iter().zip(roles) {
                assert_eq!(cell.fg, crate::color::rgb(r, g, b));
                if background != Color::Reset {
                    assert_eq!(cell.bg, crate::color::rgb(232, 235, 238));
                }
            }
        }
    }
}

#[cfg(test)]
mod named_colors {
    use super::*;

    /// Every ratatui named color the TUI actually uses must map to a role,
    /// except `Reset`.
    ///
    /// Named colors carry no RGB for literal matching to work with, so an
    /// unmapped one is a color the user simply cannot change. `Color::Black`
    /// was exactly that until this test existed.
    #[test]
    fn every_named_color_used_by_the_tui_is_configurable() {
        let used = [
            Color::White,
            Color::Black,
            Color::Gray,
            Color::DarkGray,
            Color::Red,
            Color::Green,
            Color::Yellow,
            Color::Blue,
            Color::Magenta,
            Color::Cyan,
            Color::LightRed,
            Color::LightGreen,
            Color::LightYellow,
            Color::LightBlue,
            Color::LightMagenta,
            Color::LightCyan,
        ];

        // Override every role so the only reason a color stays put is that it
        // has no mapping at all.
        let mut palette = Palette::default();
        for role in ALL_ROLES.iter().copied() {
            let (r, g, b) = role.default_rgb();
            palette.set(role, (r.wrapping_add(40), g, b));
        }

        for color in used {
            assert_ne!(
                remap_named_with(&palette, color),
                color,
                "{color:?} is used by the TUI but maps to no role, so a user cannot change it"
            );
        }
    }

    /// `Reset` must survive: it is how the terminal's own background shows
    /// through, on both light and dark themes.
    #[test]
    fn reset_is_never_substituted() {
        let mut palette = Palette::default();
        for role in ALL_ROLES.iter().copied() {
            palette.set(role, (1, 2, 3));
        }
        assert_eq!(remap_named_with(&palette, Color::Reset), Color::Reset);
    }
}

#[cfg(test)]
mod default_palette_is_frozen {
    use super::*;

    /// The exact hand-tuned palette jcode has always shipped.
    ///
    /// This is a deliberate, redundant copy of [`Role::default_rgb`]. It exists
    /// so the shipped look cannot drift: the generator, the harmony scorer, and
    /// the repair pass all consume these values, and it would be easy to "improve"
    /// a default while tuning one of them. Any change here is a change to what
    /// every existing user sees on launch, so it must be a deliberate edit to
    /// this table rather than a side effect of tooling work.
    ///
    /// Values were chosen by hand and are not derived from any metric. A low
    /// harmony score on this table is not a reason to change it.
    const HAND_TUNED: &[(Role, (u8, u8, u8))] = &[
        (Role::User, (138, 180, 248)),
        (Role::Ai, (129, 199, 132)),
        (Role::Tool, (120, 120, 120)),
        (Role::FileLink, (180, 200, 255)),
        (Role::Dim, (80, 80, 80)),
        (Role::Accent, (186, 139, 255)),
        (Role::System, (255, 170, 220)),
        (Role::Queued, (255, 193, 7)),
        (Role::Asap, (110, 210, 255)),
        (Role::Pending, (140, 140, 140)),
        (Role::UserText, (245, 245, 255)),
        (Role::UserBg, (35, 40, 50)),
        (Role::AiText, (220, 220, 215)),
        (Role::HeaderIcon, (120, 210, 230)),
        (Role::HeaderName, (190, 210, 235)),
        (Role::HeaderSession, (255, 255, 255)),
        (Role::Success, (100, 200, 100)),
        (Role::Warning, (255, 200, 100)),
        (Role::Error, (255, 100, 100)),
        (Role::Info, (140, 180, 255)),
        (Role::Border, (100, 100, 110)),
        (Role::SelectionBg, (60, 60, 80)),
    ];

    #[test]
    fn every_role_keeps_its_hand_tuned_default() {
        for (role, expected) in HAND_TUNED.iter().copied() {
            assert_eq!(
                role.default_rgb(),
                expected,
                "{} changed from its hand-tuned default {expected:?}. If this is intentional, \
                 update HAND_TUNED too and understand that every existing user's colors change.",
                role.key()
            );
        }
        assert_eq!(
            HAND_TUNED.len(),
            ALL_ROLES.len(),
            "a role was added or removed without recording its hand-tuned default"
        );
    }

    /// An unconfigured palette must resolve to exactly that table, so the
    /// default *experience* is the hand-tuned one and not merely the constants.
    #[test]
    fn unconfigured_palette_resolves_to_the_hand_tuned_table() {
        let palette = Palette::default();
        for (role, expected) in HAND_TUNED.iter().copied() {
            assert_eq!(palette.rgb(role), expected, "{}", role.key());
            assert!(
                !palette.is_overridden(role),
                "{} must not be marked as user-configured by default",
                role.key()
            );
        }
        assert!(
            !palette.has_overrides(),
            "the default palette must claim no overrides, or literal remapping would engage"
        );
    }

    /// Generation and scoring must never mutate the shipped default. They take
    /// `&Palette` and return new values, but a future refactor could plausibly
    /// reach for the global, so assert the global is untouched by both.
    #[test]
    fn generating_and_scoring_do_not_disturb_the_default() {
        let before = Palette::default();
        let _generated = crate::harmony::generate_from_seed((138, 180, 248), (18, 18, 18));
        let _report = crate::harmony::analyze(&before, (18, 18, 18));
        assert_eq!(
            Palette::default(),
            before,
            "the default palette must be immutable"
        );
        for (role, expected) in HAND_TUNED.iter().copied() {
            assert_eq!(role.default_rgb(), expected, "{}", role.key());
        }
    }
}
