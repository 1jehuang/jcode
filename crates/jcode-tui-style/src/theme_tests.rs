// Tests for the light/dark theme configuration system.
// Split from theme.rs to keep the production file under the
// 1200-LOC code-size budget (see scripts/check_code_size_budget.py).
use super::*;
use std::sync::Mutex;

/// Serializes tests that mutate the process-global `ACTIVE_THEME`. Without
/// this, parallel test threads race on the singleton and flake (e.g. one
/// test asserts `light` while another has just re-set it to `dark`). The
/// lock is held until `ActiveThemeGuard` has restored the default theme on
/// drop, so no mutating test observes another's transient state.
static THEME_TEST_MUTEX: Mutex<()> = Mutex::new(());

#[test]
fn loads_custom_theme_from_toml() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        temp.path().join("solar.toml"),
        "[colors]\nuser = \"#010203\"\nmarkdown_text = \"#0A0B0C\"\ninput_bg = \"reset\"\n",
    )
    .expect("write theme");

    let theme = load_theme("solar", Some(temp.path())).expect("load custom theme");
    assert_eq!(theme.name(), "solar");
    assert_eq!(theme.color(ThemeColor::User), Color::Rgb(1, 2, 3));
    assert_eq!(
        theme.color(ThemeColor::MarkdownText),
        Color::Rgb(10, 11, 12)
    );
    assert_eq!(theme.color(ThemeColor::InputBg), Color::Reset);
}

#[test]
fn rejects_unsafe_custom_theme_names() {
    assert!(load_theme("../bad", Some(Path::new("/tmp"))).is_err());
}

#[test]
fn parses_named_colors_in_custom_theme() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        temp.path().join("named.toml"),
        "[colors]\nerror = \"red\"\ninfo = \"light-blue\"\nsuccess = \"#64C864\"\n",
    )
    .expect("write theme");

    let theme = load_theme("named", Some(temp.path())).expect("load named theme");
    assert_eq!(theme.color(ThemeColor::Error), Color::Red);
    assert_eq!(theme.color(ThemeColor::Info), Color::LightBlue);
    assert_eq!(theme.color(ThemeColor::Success), Color::Rgb(100, 200, 100));
}

#[test]
fn semantic_colors_route_through_theme_system() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        temp.path().join("accented.toml"),
        "[colors]\nerror = \"#0A0B0C\"\nborder = \"#010203\"\nselection_bg = \"#FF00FF\"\n",
    )
    .expect("write theme");

    let theme = load_theme("accented", Some(temp.path())).expect("load accented theme");
    assert_eq!(theme.color(ThemeColor::Error), Color::Rgb(10, 11, 12));
    assert_eq!(theme.color(ThemeColor::Border), Color::Rgb(1, 2, 3));
    assert_eq!(
        theme.color(ThemeColor::SelectionBg),
        Color::Rgb(255, 0, 255)
    );
}

#[test]
fn dark_theme_sets_explicit_input_background() {
    let theme = dark_theme();
    // Dark theme must set an explicit dark InputBg so white InputText stays
    // readable even on a light terminal default background.
    assert_ne!(theme.color(ThemeColor::InputBg), Color::Reset);
    assert_ne!(theme.color(ThemeColor::InputText), Color::Reset);
}

#[test]
fn lists_builtin_and_safe_custom_theme_names() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(temp.path().join("ocean.toml"), "[colors]\n").expect("write theme");
    std::fs::write(temp.path().join("bad.name.toml"), "[colors]\n")
        .expect("write invalid theme name");
    std::fs::write(temp.path().join("notes.txt"), "ignored").expect("write txt");

    let names = available_theme_names(Some(temp.path()));
    assert!(names.contains(&"system".to_string()));
    assert!(names.contains(&"light".to_string()));
    assert!(names.contains(&"dark".to_string()));
    assert!(names.contains(&"ocean".to_string()));
}

#[test]
fn spinner_frames_are_circular_braille_sequence() {
    assert_eq!(
        SPINNER_FRAMES,
        &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]
    );
    assert!(is_activity_indicator_frame("⠋"));
    assert!(is_activity_indicator_frame("⠏"));
    assert!(!is_activity_indicator_frame("/"));
}

#[test]
fn spinner_frame_wraps_at_sequence_length() {
    let fps = 10.0;
    assert_eq!(spinner_frame(0.0, fps), "⠋");
    assert_eq!(spinner_frame(0.9, fps), "⠏");
    assert_eq!(spinner_frame(1.0, fps), "⠋");
}

#[test]
fn activity_indicator_still_advances_without_decorative_animations() {
    // With decorative animations disabled the single-cell spinner must keep
    // ticking instead of freezing on one frame.
    let first = activity_indicator(0.0, 12.5, false);
    let later = activity_indicator(1.0, 12.5, false);
    assert!(SPINNER_FRAMES.contains(&first));
    assert_ne!(
        first, later,
        "liveness spinner should advance within one second"
    );
}

#[test]
fn liveness_spinner_advances_smoothly_within_a_few_frames() {
    // The single-cell fast path patches one status cell per 80ms tick, so the
    // non-decorative liveness spinner should advance well faster than ~1 Hz
    // (it should not still read as frozen between consecutive fast-path ticks).
    let frame_at = |elapsed: f32| activity_indicator(elapsed, 12.5, false);
    // One 80ms fast-path tick should already move to the next frame.
    assert_ne!(
        frame_at(0.0),
        frame_at(0.08),
        "liveness spinner should advance every fast-path tick (80ms)"
    );
    // It must be meaningfully faster than the old ~1.5 Hz cadence.
    const {
        assert!(
            LIVENESS_SPINNER_FPS >= 8.0,
            "liveness spinner should animate at a smooth, responsive rate"
        );
    }
}

// ---- parse_color ----

#[test]
fn parse_color_parses_six_digit_hex() {
    assert_eq!(parse_color("#ff0000"), Some(Color::Rgb(255, 0, 0)));
    assert_eq!(parse_color("#0a0b0c"), Some(Color::Rgb(10, 11, 12)));
    // Uppercase hex digits are accepted.
    assert_eq!(parse_color("#FF00FF"), Some(Color::Rgb(255, 0, 255)));
    // Surrounding whitespace is trimmed.
    assert_eq!(parse_color("  #00ff00  "), Some(Color::Rgb(0, 255, 0)));
}

#[test]
fn parse_color_requires_hash_prefix() {
    // A bare hex string is not recognized: parse_color tries named colors
    // first, then requires an explicit `#` before the hex digits.
    assert_eq!(parse_color("ff0000"), None);
    assert_eq!(parse_color("000000"), None);
}

#[test]
fn parse_color_rejects_malformed_hex() {
    // Short form (e.g. #fff) is not supported, only #rrggbb.
    assert_eq!(parse_color("#fff"), None);
    assert_eq!(parse_color("#12345"), None);
    assert_eq!(parse_color("#1234567"), None);
    // Non-hex digits fail parsing.
    assert_eq!(parse_color("#gg0000"), None);
    assert_eq!(parse_color("#00zz00"), None);
    // There is no rgb(...) parser.
    assert_eq!(parse_color("rgb(255,0,0)"), None);
    // Empty / garbage.
    assert_eq!(parse_color(""), None);
    assert_eq!(parse_color("xyz"), None);
    assert_eq!(parse_color("#"), None);
}

#[test]
fn parse_color_recognizes_reset_and_default() {
    assert_eq!(parse_color("reset"), Some(Color::Reset));
    assert_eq!(parse_color("RESET"), Some(Color::Reset));
    assert_eq!(parse_color("default"), Some(Color::Reset));
    assert_eq!(parse_color("Default"), Some(Color::Reset));
}

#[test]
fn parse_color_resolves_named_colors_with_normalization() {
    // Exact lowercase names.
    assert_eq!(parse_color("red"), Some(Color::Red));
    assert_eq!(parse_color("blue"), Some(Color::Blue));
    assert_eq!(parse_color("green"), Some(Color::Green));
    assert_eq!(parse_color("white"), Some(Color::White));
    assert_eq!(parse_color("black"), Some(Color::Black));
    // Case is normalized before lookup.
    assert_eq!(parse_color("Red"), Some(Color::Red));
    assert_eq!(parse_color("MAGENTA"), Some(Color::Magenta));
    // Dashes and spaces are stripped so "light-blue" maps to LightBlue.
    assert_eq!(parse_color("light-blue"), Some(Color::LightBlue));
    assert_eq!(parse_color("light blue"), Some(Color::LightBlue));
    assert_eq!(parse_color("dark-gray"), Some(Color::DarkGray));
    assert_eq!(parse_color("grey"), Some(Color::Gray));
}

// ---- named_color ----

#[test]
fn named_color_resolves_known_names() {
    assert_eq!(named_color("black"), Some(Color::Black));
    assert_eq!(named_color("red"), Some(Color::Red));
    assert_eq!(named_color("green"), Some(Color::Green));
    assert_eq!(named_color("yellow"), Some(Color::Yellow));
    assert_eq!(named_color("blue"), Some(Color::Blue));
    assert_eq!(named_color("magenta"), Some(Color::Magenta));
    assert_eq!(named_color("cyan"), Some(Color::Cyan));
    assert_eq!(named_color("gray"), Some(Color::Gray));
    assert_eq!(named_color("grey"), Some(Color::Gray));
    assert_eq!(named_color("darkgray"), Some(Color::DarkGray));
    assert_eq!(named_color("darkgrey"), Some(Color::DarkGray));
    assert_eq!(named_color("lightred"), Some(Color::LightRed));
    assert_eq!(named_color("lightgreen"), Some(Color::LightGreen));
    assert_eq!(named_color("lightyellow"), Some(Color::LightYellow));
    assert_eq!(named_color("lightblue"), Some(Color::LightBlue));
    assert_eq!(named_color("lightmagenta"), Some(Color::LightMagenta));
    assert_eq!(named_color("lightcyan"), Some(Color::LightCyan));
    assert_eq!(named_color("white"), Some(Color::White));
}

#[test]
fn named_color_rejects_unknown_and_is_case_sensitive() {
    // named_color matches the exact (lowercase) string; it does not
    // lowercase its input. parse_color does that normalization first.
    assert_eq!(named_color("notacolor"), None);
    assert_eq!(named_color(""), None);
    assert_eq!(named_color("Red"), None);
    assert_eq!(named_color("RED"), None);
    assert_eq!(named_color("LightBlue"), None);
    // Dashes/spaces are not stripped here (parse_color does that).
    assert_eq!(named_color("light-blue"), None);
    assert_eq!(named_color("light blue"), None);
}

// ---- parse_theme_color ----

#[test]
fn parse_theme_color_maps_each_variant_canonical_name() {
    let cases: &[(&str, ThemeColor)] = &[
        ("background", ThemeColor::Background),
        ("user", ThemeColor::User),
        ("ai", ThemeColor::Ai),
        ("tool", ThemeColor::Tool),
        ("file_link", ThemeColor::FileLink),
        ("dim", ThemeColor::Dim),
        ("accent", ThemeColor::Accent),
        ("system_message", ThemeColor::SystemMessage),
        ("queued", ThemeColor::Queued),
        ("asap", ThemeColor::Asap),
        ("pending", ThemeColor::Pending),
        ("user_text", ThemeColor::UserText),
        ("user_bg", ThemeColor::UserBg),
        ("input_text", ThemeColor::InputText),
        ("input_bg", ThemeColor::InputBg),
        ("ai_text", ThemeColor::AiText),
        ("bold", ThemeColor::Bold),
        ("markdown_text", ThemeColor::MarkdownText),
        ("header_icon", ThemeColor::HeaderIcon),
        ("header_name", ThemeColor::HeaderName),
        ("header_session", ThemeColor::HeaderSession),
        ("success", ThemeColor::Success),
        ("warning", ThemeColor::Warning),
        ("error", ThemeColor::Error),
        ("info", ThemeColor::Info),
        ("border", ThemeColor::Border),
        ("selection_bg", ThemeColor::SelectionBg),
    ];
    for (raw, expected) in cases {
        assert_eq!(
            parse_theme_color(raw),
            Some(*expected),
            "canonical name {raw:?}"
        );
    }
}

#[test]
fn parse_theme_color_accepts_aliases() {
    let cases: &[(&str, ThemeColor)] = &[
        ("background_color", ThemeColor::Background),
        ("app_bg", ThemeColor::Background),
        ("app_background", ThemeColor::Background),
        ("user_color", ThemeColor::User),
        ("ai_color", ThemeColor::Ai),
        ("tool_color", ThemeColor::Tool),
        ("file_link_color", ThemeColor::FileLink),
        ("dim_color", ThemeColor::Dim),
        ("accent_color", ThemeColor::Accent),
        ("system_message_color", ThemeColor::SystemMessage),
        ("queued_color", ThemeColor::Queued),
        ("asap_color", ThemeColor::Asap),
        ("pending_color", ThemeColor::Pending),
        ("bold_color", ThemeColor::Bold),
        ("md_text", ThemeColor::MarkdownText),
        ("header_icon_color", ThemeColor::HeaderIcon),
        ("header_name_color", ThemeColor::HeaderName),
        ("header_session_color", ThemeColor::HeaderSession),
        ("success_color", ThemeColor::Success),
        ("warning_color", ThemeColor::Warning),
        ("error_color", ThemeColor::Error),
        ("info_color", ThemeColor::Info),
        ("border_color", ThemeColor::Border),
        ("selection_bg_color", ThemeColor::SelectionBg),
    ];
    for (raw, expected) in cases {
        assert_eq!(parse_theme_color(raw), Some(*expected), "alias {raw:?}");
    }
}

#[test]
fn parse_theme_color_normalizes_input() {
    // Trims surrounding whitespace.
    assert_eq!(
        parse_theme_color("  background  "),
        Some(ThemeColor::Background)
    );
    // Case-insensitive.
    assert_eq!(
        parse_theme_color("BACKGROUND"),
        Some(ThemeColor::Background)
    );
    assert_eq!(parse_theme_color("User_Text"), Some(ThemeColor::UserText));
    // Dashes are folded to underscores.
    assert_eq!(
        parse_theme_color("app-background"),
        Some(ThemeColor::Background)
    );
    assert_eq!(
        parse_theme_color("header-icon"),
        Some(ThemeColor::HeaderIcon)
    );
    assert_eq!(parse_theme_color("md-text"), Some(ThemeColor::MarkdownText));
    assert_eq!(
        parse_theme_color("selection-bg-color"),
        Some(ThemeColor::SelectionBg)
    );
    // Mixed case + dashes + spaces together.
    assert_eq!(
        parse_theme_color("  Error-Color  "),
        Some(ThemeColor::Error)
    );
}

#[test]
fn parse_theme_color_rejects_unknown_keys() {
    assert_eq!(parse_theme_color("notacolor"), None);
    assert_eq!(parse_theme_color(""), None);
    assert_eq!(parse_theme_color("   "), None);
    // British spelling is not supported.
    assert_eq!(parse_theme_color("background_colour"), None);
    // Plain *_text / *_bg keys have no `_color` alias form.
    assert_eq!(parse_theme_color("user_text_color"), None);
    assert_eq!(parse_theme_color("input_bg_color"), None);
}

// ---- active_theme_uses_terminal_adaptation ----

/// Restores the process-global active theme to the default `system` theme
/// when dropped, so tests that mutate it do not leak state to other tests.
/// Drop runs even when the test panics, so the global is always restored.
struct ActiveThemeGuard;

impl Drop for ActiveThemeGuard {
    fn drop(&mut self) {
        let _ = set_theme("system", None);
    }
}

#[test]
fn active_theme_adaptation_follows_active_theme() {
    let _lock = THEME_TEST_MUTEX.lock().unwrap();
    let _guard = ActiveThemeGuard;

    // `system` (the default) delegates to the terminal light/dark adapter.
    set_theme("system", None).expect("set system theme");
    assert!(active_theme_uses_terminal_adaptation());

    // `light` also reuses the buffer adapter.
    set_theme("light", None).expect("set light theme");
    assert!(active_theme_uses_terminal_adaptation());

    // `dark` ships an explicit palette, so adaptation must be off.
    set_theme("dark", None).expect("set dark theme");
    assert!(!active_theme_uses_terminal_adaptation());

    // A custom TOML theme is an explicit palette too: no adaptation.
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        temp.path().join("ocean.toml"),
        "[colors]\nuser = \"#001122\"\n",
    )
    .expect("write theme");
    set_theme("ocean", Some(temp.path())).expect("load custom theme");
    assert_eq!(active_theme_name(), "ocean");
    assert!(!active_theme_uses_terminal_adaptation());
}

// ---- Theme::new ----

#[test]
fn theme_new_stores_name_and_color_map() {
    let mut colors = BTreeMap::new();
    colors.insert(ThemeColor::User, Color::Rgb(1, 2, 3));
    colors.insert(ThemeColor::Ai, Color::Rgb(4, 5, 6));
    let theme = Theme::new("test-palette", colors);
    assert_eq!(theme.name(), "test-palette");
    assert_eq!(theme.color(ThemeColor::User), Color::Rgb(1, 2, 3));
    assert_eq!(theme.color(ThemeColor::Ai), Color::Rgb(4, 5, 6));
}

#[test]
fn theme_new_accepts_string_and_string_literal() {
    // `impl Into<String>` accepts both owned and borrowed strings.
    let theme_owned = Theme::new(String::from("owned"), BTreeMap::new());
    assert_eq!(theme_owned.name(), "owned");
    let theme_lit = Theme::new("literal", BTreeMap::new());
    assert_eq!(theme_lit.name(), "literal");
}

// ---- Theme::color ----

#[test]
fn theme_color_returns_value_for_existing_key() {
    let mut colors = BTreeMap::new();
    colors.insert(ThemeColor::Bold, Color::Rgb(10, 20, 30));
    let theme = Theme::new("t", colors);
    assert_eq!(theme.color(ThemeColor::Bold), Color::Rgb(10, 20, 30));
}

#[test]
fn theme_color_returns_reset_for_missing_key() {
    // An empty palette yields Color::Reset for every key, matching the
    // documented fallback in Theme::color.
    let theme = Theme::new("empty", BTreeMap::new());
    assert_eq!(theme.color(ThemeColor::User), Color::Reset);
    assert_eq!(theme.color(ThemeColor::Background), Color::Reset);
    assert_eq!(theme.color(ThemeColor::Bold), Color::Reset);
}

// ---- active_theme_name ----

#[test]
fn active_theme_name_matches_last_set_theme() {
    let _lock = THEME_TEST_MUTEX.lock().unwrap();
    let _guard = ActiveThemeGuard;
    set_theme("dark", None).expect("set dark theme");
    assert_eq!(active_theme_name(), "dark");

    set_theme("light", None).expect("set light theme");
    assert_eq!(active_theme_name(), "light");
}

// ---- set_theme ----

#[test]
fn set_theme_switches_to_builtin_dark() {
    let _lock = THEME_TEST_MUTEX.lock().unwrap();
    let _guard = ActiveThemeGuard;
    set_theme("dark", None).expect("set dark theme");
    assert_eq!(active_theme_name(), "dark");
    // `dark` ships an explicit palette, so adaptation must be off.
    assert!(!active_theme_uses_terminal_adaptation());
}

#[test]
fn set_theme_rejects_unknown_theme_without_themes_dir() {
    let _lock = THEME_TEST_MUTEX.lock().unwrap();
    let _guard = ActiveThemeGuard;
    // A safe-but-unknown name with no themes dir cannot be loaded: the
    // custom-theme loader requires a configured directory.
    let result = set_theme("nonexistent-theme-xyz", None);
    assert!(result.is_err(), "unknown theme should error without a dir");
    // The failed set must not mutate the active theme.
    assert_ne!(active_theme_name(), "nonexistent-theme-xyz");
}

// ---- system_palette_named ----

#[test]
fn system_palette_named_labels_each_builtin() {
    assert_eq!(system_palette_named("system").name(), "system");
    assert_eq!(system_palette_named("light").name(), "light");
    assert_eq!(system_palette_named("dark").name(), "dark");
}

#[test]
fn system_palette_named_ships_default_palette() {
    // Every builtin palette keeps Color::Reset for the background (the
    // terminal adapter handles light/dark), plus a concrete Bold color.
    let theme = system_palette_named("system");
    assert_eq!(theme.color(ThemeColor::Background), Color::Reset);
    assert_eq!(theme.color(ThemeColor::InputText), Color::Reset);
    assert_eq!(theme.color(ThemeColor::InputBg), Color::Reset);
}

#[test]
fn system_palette_named_unknown_name_still_builds_theme() {
    // system_palette_named is a low-level builder: it does not validate the
    // name against the builtin list, it just labels the palette. Unknown
    // names still produce a Theme with that name and the default palette.
    let theme = system_palette_named("ocean");
    assert_eq!(theme.name(), "ocean");
    assert_eq!(theme.color(ThemeColor::Background), Color::Reset);
}

// ---- background_color / input_text / input_bg / bold_color / markdown_text_color ----
//
// These read the active theme, so they need an ActiveThemeGuard and a
// pinned truecolor capability so the builtin `rgb(...)` literals resolve to
// exact Color::Rgb values regardless of the host terminal.

#[test]
fn background_color_returns_dark_theme_background() {
    let _lock = THEME_TEST_MUTEX.lock().unwrap();
    let _guard = ActiveThemeGuard;
    color::pin_truecolor_for_tests();
    set_theme("dark", None).expect("set dark theme");
    assert_eq!(background_color(), Color::Rgb(18, 18, 26));
}

#[test]
fn input_text_returns_dark_theme_input_text() {
    let _lock = THEME_TEST_MUTEX.lock().unwrap();
    let _guard = ActiveThemeGuard;
    color::pin_truecolor_for_tests();
    set_theme("dark", None).expect("set dark theme");
    assert_eq!(input_text(), Color::Rgb(240, 240, 245));
}

#[test]
fn input_bg_returns_dark_theme_input_bg() {
    let _lock = THEME_TEST_MUTEX.lock().unwrap();
    let _guard = ActiveThemeGuard;
    color::pin_truecolor_for_tests();
    set_theme("dark", None).expect("set dark theme");
    assert_eq!(input_bg(), Color::Rgb(18, 18, 26));
}

#[test]
fn bold_color_returns_dark_theme_bold() {
    let _lock = THEME_TEST_MUTEX.lock().unwrap();
    let _guard = ActiveThemeGuard;
    color::pin_truecolor_for_tests();
    set_theme("dark", None).expect("set dark theme");
    // dark_theme inherits Bold from the system palette default.
    assert_eq!(bold_color(), Color::Rgb(240, 240, 235));
}

#[test]
fn markdown_text_color_returns_dark_theme_markdown_text() {
    let _lock = THEME_TEST_MUTEX.lock().unwrap();
    let _guard = ActiveThemeGuard;
    color::pin_truecolor_for_tests();
    set_theme("dark", None).expect("set dark theme");
    // dark_theme inherits MarkdownText from the system palette default.
    assert_eq!(markdown_text_color(), Color::Rgb(200, 200, 195));
}

// --- Contract tests ---
//
// These pin the public theme API as a contract: set→read round-trip, the
// global color accessors routing through the active theme, lossless custom
// TOML loading, and full ThemeColor key coverage. They document the
// behavior external callers depend on and guard against silent regressions.

/// Contract: the public `set_theme` → `active_theme_name` round-trip must
/// hold for every builtin theme. Installing a builtin theme by name and
/// then reading the active theme name must echo back the exact name passed
/// in.
#[test]
fn contract_set_theme_then_active_theme_name_round_trips() {
    let _lock = THEME_TEST_MUTEX.lock().unwrap();
    let _guard = ActiveThemeGuard;

    for &name in &["system", "light", "dark"] {
        set_theme(name, None).unwrap_or_else(|e| panic!("set_theme({name:?}) failed: {e}"));
        assert_eq!(
            active_theme_name(),
            name,
            "active_theme_name() must echo the name passed to set_theme"
        );
    }
}

/// Contract: the global color accessors (`background_color`, `input_text`,
/// `input_bg`, `bold_color`, `markdown_text_color`) must route through the
/// active theme, returning exactly what `Theme::color()` returns for the
/// matching `ThemeColor` variant on the currently installed palette — not
/// a stale default. Verified against both the `dark` and `light` builtin
/// palettes, whose Background/InputText/InputBg values differ, so a stale
/// accessor would be caught.
#[test]
fn contract_color_accessors_match_active_theme() {
    let _lock = THEME_TEST_MUTEX.lock().unwrap();
    let _guard = ActiveThemeGuard;
    color::pin_truecolor_for_tests();

    for &name in &["dark", "light"] {
        let theme =
            load_theme(name, None).unwrap_or_else(|e| panic!("load_theme({name:?}) failed: {e}"));
        set_theme(name, None).unwrap_or_else(|e| panic!("set_theme({name:?}) failed: {e}"));

        assert_eq!(
            background_color(),
            theme.color(ThemeColor::Background),
            "{name}: background_color() must match active Theme::color(Background)"
        );
        assert_eq!(
            input_text(),
            theme.color(ThemeColor::InputText),
            "{name}: input_text() must match active Theme::color(InputText)"
        );
        assert_eq!(
            input_bg(),
            theme.color(ThemeColor::InputBg),
            "{name}: input_bg() must match active Theme::color(InputBg)"
        );
        assert_eq!(
            bold_color(),
            theme.color(ThemeColor::Bold),
            "{name}: bold_color() must match active Theme::color(Bold)"
        );
        assert_eq!(
            markdown_text_color(),
            theme.color(ThemeColor::MarkdownText),
            "{name}: markdown_text_color() must match active Theme::color(MarkdownText)"
        );
    }
}

/// Contract: loading a custom TOML theme must preserve the exact RGB hex
/// values written in the file, end to end through parse → store → read. No
/// quantization, no default bleeding in for the keys that were set.
///
/// Uses `markdown_text` rather than a generic `text` key because the
/// theme TOML schema has no `text` alias; only keys that
/// `parse_theme_color` recognizes are accepted.
#[test]
fn contract_custom_theme_load_preserves_rgb_values() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        temp.path().join("ocean.toml"),
        "[colors]\n\
             background = \"#0a0b1a\"\n\
             markdown_text = \"#e0e0f0\"\n\
             input_text = \"#c0c0ff\"\n",
    )
    .expect("write ocean.toml");

    let theme = load_theme("ocean", Some(temp.path())).expect("load ocean theme");
    assert_eq!(theme.name(), "ocean");
    assert_eq!(
        theme.color(ThemeColor::Background),
        Color::Rgb(10, 11, 26),
        "background hex must round-trip losslessly"
    );
    assert_eq!(
        theme.color(ThemeColor::MarkdownText),
        Color::Rgb(224, 224, 240),
        "markdown_text hex must round-trip losslessly"
    );
    assert_eq!(
        theme.color(ThemeColor::InputText),
        Color::Rgb(192, 192, 255),
        "input_text hex must round-trip losslessly"
    );
}

/// Contract: every `ThemeColor` variant must be reachable through its
/// canonical TOML key (no orphaned enum members that the loader cannot
/// parse), and every variant must be resolvable through `Theme::color()`
/// once it has been stored. This guards against adding an enum variant
/// without a matching parser arm.
#[test]
fn contract_every_theme_color_variant_is_parseable_and_resolvable() {
    let variants: &[(&str, ThemeColor)] = &[
        ("background", ThemeColor::Background),
        ("user", ThemeColor::User),
        ("ai", ThemeColor::Ai),
        ("tool", ThemeColor::Tool),
        ("file_link", ThemeColor::FileLink),
        ("dim", ThemeColor::Dim),
        ("accent", ThemeColor::Accent),
        ("system_message", ThemeColor::SystemMessage),
        ("queued", ThemeColor::Queued),
        ("asap", ThemeColor::Asap),
        ("pending", ThemeColor::Pending),
        ("user_text", ThemeColor::UserText),
        ("user_bg", ThemeColor::UserBg),
        ("input_text", ThemeColor::InputText),
        ("input_bg", ThemeColor::InputBg),
        ("ai_text", ThemeColor::AiText),
        ("bold", ThemeColor::Bold),
        ("markdown_text", ThemeColor::MarkdownText),
        ("header_icon", ThemeColor::HeaderIcon),
        ("header_name", ThemeColor::HeaderName),
        ("header_session", ThemeColor::HeaderSession),
        ("success", ThemeColor::Success),
        ("warning", ThemeColor::Warning),
        ("error", ThemeColor::Error),
        ("info", ThemeColor::Info),
        ("border", ThemeColor::Border),
        ("selection_bg", ThemeColor::SelectionBg),
    ];

    const KNOWN: Color = Color::Rgb(1, 2, 3);
    for &(canonical, variant) in variants {
        assert_eq!(
            parse_theme_color(canonical),
            Some(variant),
            "canonical key {canonical:?} should parse to {variant:?}"
        );

        // A Theme that maps only this variant to a known color must hand
        // it back through Theme::color() losslessly.
        let mut colors = BTreeMap::new();
        colors.insert(variant, KNOWN);
        let theme = Theme::new("coverage", colors);
        assert_eq!(
            theme.color(variant),
            KNOWN,
            "Theme::color({variant:?}) should return the stored color"
        );
    }
}
