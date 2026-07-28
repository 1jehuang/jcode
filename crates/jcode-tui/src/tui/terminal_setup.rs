//! Terminal setup for reporting modified Enter (Shift+Enter, Ctrl+Enter).
//!
//! # Why this exists
//!
//! Terminals encode Enter as a single byte, `0x0d`. The Ctrl bit-masking and
//! Shift-changes-the-character schemes inherited from the VT100 have no room to
//! say "Shift was held", so `Enter`, `Shift+Enter`, and `Ctrl+Enter` all arrive
//! as the same byte. An app literally cannot tell them apart.
//!
//! The modern fix is the kitty keyboard protocol: the app asks for
//! disambiguation and the terminal then sends `ESC[13;2u` for Shift+Enter.
//! jcode already requests it at startup, and crossterm decodes it, so on any
//! terminal that implements the protocol (kitty, Ghostty, WezTerm, Alacritty,
//! foot, iTerm2 3.5+, Warp, recent VS Code) Shift+Enter works with no setup.
//!
//! Three cases still break, and each needs different handling:
//!
//! 1. **The terminal ignores the request** (macOS Terminal.app). It has no CSI u
//!    support at all, so no runtime negotiation can help. The only fix is to
//!    remap the key in the terminal's own settings.
//! 2. **A multiplexer swallows the request** (tmux). The outer terminal may be
//!    fully capable while tmux does not forward extended keys, so Shift+Enter
//!    still collapses to a bare CR.
//! 3. **The terminal needs a config flag** (WezTerm's `enable_kitty_keyboard`).
//!
//! This module writes the per-terminal configuration for those cases so the real
//! chord works, rather than teaching users an alternative key.

use std::path::{Path, PathBuf};

/// Outcome of configuring one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// The config was written; the note explains how to activate it.
    Changed { detail: String },
    /// Already configured, nothing to do.
    AlreadyConfigured,
    /// Cannot be fixed by writing config; the message explains what to do.
    Manual { message: String },
}

/// A terminal whose Shift+Enter behavior jcode can configure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupTarget {
    /// macOS Terminal.app: remap Shift+Enter to the CSI u sequence.
    AppleTerminal,
    /// tmux: forward extended keys to the inner application.
    Tmux,
    /// WezTerm: opt in to the kitty keyboard protocol.
    WezTerm,
}

impl SetupTarget {
    pub fn label(self) -> &'static str {
        match self {
            Self::AppleTerminal => "Terminal.app",
            Self::Tmux => "tmux",
            Self::WezTerm => "WezTerm",
        }
    }

    /// Whether the change needs a restart or reload to take effect.
    pub fn activation_note(self) -> &'static str {
        match self {
            Self::AppleTerminal => "Quit and reopen Terminal.app to pick up the key mapping.",
            Self::Tmux => "Run `tmux source ~/.tmux.conf`, then restart jcode.",
            Self::WezTerm => "Restart WezTerm to pick up the config change.",
        }
    }
}

/// The CSI u encoding of Shift+Enter: keycode 13, modifier 2 (1 + shift bit).
///
/// This is the sequence a capable terminal sends when the kitty protocol is
/// active, and the one crossterm decodes back into `KeyCode::Enter` +
/// `KeyModifiers::SHIFT`. Terminals we remap by hand must send exactly this.
pub const SHIFT_ENTER_CSI_U: &str = "\u{1b}[13;2u";

/// `SHIFT_ENTER_CSI_U` as the space-separated hex codes Terminal.app stores.
///
/// Terminal.app's `keyMapBoundKeys` values are the literal bytes to send. We
/// spell them out here so the mapping stays readable and testable.
pub const SHIFT_ENTER_HEX_CODES: &[u8] = &[0x1b, b'[', b'1', b'3', b';', b'2', b'u'];

/// Terminal.app's key identifier for Shift+Return.
///
/// The `keyMapBoundKeys` dictionary keys encode the keypress as a modifier
/// prefix plus the key. `$` is the Shift prefix and `\015` is Return (0x0d, in
/// octal), so Shift+Return is `$\015`.
pub const APPLE_TERMINAL_SHIFT_RETURN_KEY: &str = "$\\015";

/// The tmux settings that make a capable outer terminal's modified keys reach
/// the inner application.
///
/// `extended-keys on` only forwards when the app asks (jcode does), which is
/// safer than `always` for other panes. `extended-keys-format csi-u` picks the
/// encoding crossterm understands, and `terminal-features` is required for tmux
/// to request extended keys from the outer terminal in the first place.
pub const TMUX_CONFIG_BLOCK: &str = "\
# jcode: let Shift+Enter reach the application instead of collapsing to Enter.
set -s extended-keys on
set -s extended-keys-format csi-u
set -as terminal-features 'xterm*:extkeys'
";

/// The WezTerm setting that opts in to the kitty keyboard protocol.
pub const WEZTERM_CONFIG_LINE: &str = "config.enable_kitty_keyboard = true";

/// Marker used to detect a block this command already wrote, so re-running is
/// idempotent instead of appending duplicates.
pub const MANAGED_MARKER: &str = "# jcode: let Shift+Enter reach the application";

/// Whether `config` already contains jcode's managed tmux block.
pub fn tmux_config_is_configured(config: &str) -> bool {
    config.contains(MANAGED_MARKER)
        || (config.contains("extended-keys on") && config.contains("csi-u"))
}

/// The tmux config text after ensuring the managed block is present.
///
/// Returns `None` when nothing needs to change, so callers can report "already
/// configured" without rewriting the file.
pub fn tmux_config_with_block(existing: &str) -> Option<String> {
    if tmux_config_is_configured(existing) {
        return None;
    }
    let mut out = existing.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(TMUX_CONFIG_BLOCK);
    Some(out)
}

/// Whether `config` already enables the kitty keyboard protocol in WezTerm.
pub fn wezterm_config_is_configured(config: &str) -> bool {
    config
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("--"))
        .any(|line| line.contains("enable_kitty_keyboard") && line.contains("true"))
}

/// Path to the user's tmux config.
pub fn tmux_config_path(home: &Path) -> PathBuf {
    home.join(".tmux.conf")
}

/// Path to the user's WezTerm config.
pub fn wezterm_config_path(home: &Path) -> PathBuf {
    home.join(".wezterm.lua")
}

/// Path to Terminal.app's preferences plist.
pub fn apple_terminal_plist_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Preferences")
        .join("com.apple.Terminal.plist")
}

/// The `defaults`-style value Terminal.app expects for the Shift+Return mapping.
///
/// Terminal.app stores the raw bytes to transmit, so we render the escape as its
/// octal form (`\033`) the way the Keyboard settings pane does.
pub fn apple_terminal_shift_return_value() -> String {
    let mut out = String::from("\\033");
    for &byte in &SHIFT_ENTER_HEX_CODES[1..] {
        out.push(byte as char);
    }
    out
}

/// What is preventing Shift+Enter from working, based on the environment.
///
/// Order matters: a multiplexer is checked first because it masks the outer
/// terminal's capability, so "your terminal is fine, tmux is eating it" is the
/// actionable message in that case.
pub fn diagnose(term_program: Option<&str>, inside_tmux: bool) -> SetupTarget {
    if inside_tmux {
        return SetupTarget::Tmux;
    }
    match term_program.map(str::to_ascii_lowercase).as_deref() {
        Some("wezterm") => SetupTarget::WezTerm,
        _ => SetupTarget::AppleTerminal,
    }
}

/// Apply the fix for `target`, or return instructions when config cannot do it.
pub fn apply(target: SetupTarget, home: &Path) -> std::io::Result<Applied> {
    match target {
        SetupTarget::Tmux => apply_tmux(home),
        SetupTarget::WezTerm => apply_wezterm(home),
        SetupTarget::AppleTerminal => Ok(apple_terminal_guidance()),
    }
}

/// Whether the terminal can report modified Enter (Shift/Ctrl+Enter) distinctly.
///
/// Writing the kitty-protocol activation sequence almost always succeeds, so the
/// return value of [`enable_keyboard_enhancement`] only says "the bytes went
/// out", not "the terminal understood them". Terminals with no CSI u support
/// (macOS Terminal.app most notably) silently ignore the sequence and keep
/// sending a bare `\r` for every Enter chord, indistinguishable from a plain
/// Enter. Ask the terminal directly instead of assuming.
///
/// `None` means the query was inconclusive (no tty, or the terminal did not
/// answer in time), in which case callers should stay quiet rather than warn.
pub fn supports_modified_enter_reporting() -> Option<bool> {
    match crossterm::terminal::supports_keyboard_enhancement() {
        Ok(supported) => {
            crate::logging::info(&format!(
                "Kitty keyboard protocol support query: {}",
                if supported {
                    "supported (Shift+Enter is reportable)"
                } else {
                    "unsupported (Shift+Enter arrives as a bare CR)"
                }
            ));
            Some(supported)
        }
        Err(err) => {
            crate::logging::info(&format!(
                "Kitty keyboard protocol support query inconclusive: {err}"
            ));
            None
        }
    }
}

/// Configure tmux so a capable outer terminal's Shift+Enter reaches jcode.
pub fn apply_tmux(home: &Path) -> std::io::Result<Applied> {
    let path = tmux_config_path(home);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    match tmux_config_with_block(&existing) {
        None => Ok(Applied::AlreadyConfigured),
        Some(updated) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, updated)?;
            Ok(Applied::Changed {
                detail: format!("Updated {}", path.display()),
            })
        }
    }
}

/// Configure WezTerm to opt in to the kitty keyboard protocol.
pub fn apply_wezterm(home: &Path) -> std::io::Result<Applied> {
    let path = wezterm_config_path(home);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if wezterm_config_is_configured(&existing) {
        return Ok(Applied::AlreadyConfigured);
    }
    // WezTerm's config is Lua that must `return` a config object, so an
    // automatic edit can only be trusted for the simple case where we are
    // creating the file. Otherwise, hand the user the exact line to add rather
    // than risk corrupting a config we do not understand.
    if existing.trim().is_empty() {
        std::fs::write(
            &path,
            format!(
                "local wezterm = require 'wezterm'\n\
                 local config = wezterm.config_builder()\n\
                 -- jcode: report Shift+Enter distinctly from Enter.\n\
                 {WEZTERM_CONFIG_LINE}\n\
                 return config\n"
            ),
        )?;
        return Ok(Applied::Changed {
            detail: format!("Created {}", path.display()),
        });
    }
    Ok(Applied::Manual {
        message: format!(
            "Add `{WEZTERM_CONFIG_LINE}` to {} (before `return config`).",
            path.display()
        ),
    })
}

/// Instructions for Terminal.app, which cannot be fixed by config alone.
///
/// Terminal.app has no CSI u support, and its `keyMapBoundKeys` GUI does not
/// offer Return as a bindable key, so there is no reliable programmatic remap.
/// Recommending a capable terminal is the honest answer.
pub fn apple_terminal_guidance() -> Applied {
    Applied::Manual {
        message: format!(
            "Terminal.app cannot report Shift+Enter: it sends the same byte (0x0d) for \
             Enter and Shift+Enter, and has no CSI u support.\n\
             Fix it by switching to a terminal that does: Ghostty, iTerm2, kitty, \
             WezTerm, or Alacritty. Shift+Enter then works with no setup.\n\
             To stay on Terminal.app, add a key mapping under \
             Settings > Profiles > Keyboard that sends {} for Shift+Return, \
             or use Ctrl+J / a trailing backslash to insert a newline.",
            apple_terminal_shift_return_value()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmux_is_diagnosed_first_because_it_masks_a_capable_terminal() {
        // Ghostty fully supports the protocol, but inside tmux the keys still do
        // not arrive, so tmux is the thing to fix.
        assert_eq!(diagnose(Some("ghostty"), true), SetupTarget::Tmux);
        assert_eq!(diagnose(Some("WezTerm"), true), SetupTarget::Tmux);
    }

    #[test]
    fn wezterm_is_diagnosed_case_insensitively() {
        // TERM_PROGRAM casing varies between launch methods.
        assert_eq!(diagnose(Some("WezTerm"), false), SetupTarget::WezTerm);
        assert_eq!(diagnose(Some("wezterm"), false), SetupTarget::WezTerm);
    }

    #[test]
    fn unknown_terminals_fall_back_to_the_apple_terminal_explanation() {
        // The fallback explains the underlying 0x0d collision and names capable
        // terminals, which is useful advice for any unsupported terminal.
        assert_eq!(
            diagnose(Some("Apple_Terminal"), false),
            SetupTarget::AppleTerminal
        );
        assert_eq!(diagnose(None, false), SetupTarget::AppleTerminal);
    }

    #[test]
    fn apply_routes_each_target_to_its_own_fix() {
        let home = tempfile::tempdir().expect("tempdir");
        assert!(matches!(
            apply(SetupTarget::Tmux, home.path()).expect("tmux"),
            Applied::Changed { .. }
        ));
        assert!(std::fs::metadata(tmux_config_path(home.path())).is_ok());

        assert!(matches!(
            apply(SetupTarget::AppleTerminal, home.path()).expect("apple"),
            Applied::Manual { .. }
        ));
    }

    #[test]
    fn applying_tmux_setup_creates_config_then_becomes_a_no_op() {
        let home = tempfile::tempdir().expect("tempdir");
        let first = apply_tmux(home.path()).expect("first apply");
        assert!(matches!(first, Applied::Changed { .. }), "got {first:?}");

        let written = std::fs::read_to_string(tmux_config_path(home.path())).expect("read config");
        assert!(written.contains("set -s extended-keys on"));
        assert!(written.contains("csi-u"));

        // Re-running must not duplicate the block.
        assert_eq!(
            apply_tmux(home.path()).expect("second apply"),
            Applied::AlreadyConfigured
        );
        assert_eq!(
            std::fs::read_to_string(tmux_config_path(home.path())).expect("reread"),
            written
        );
    }

    #[test]
    fn applying_tmux_setup_preserves_existing_user_config() {
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmux_config_path(home.path()), "set -g mouse on\n").expect("seed");

        apply_tmux(home.path()).expect("apply");

        let written = std::fs::read_to_string(tmux_config_path(home.path())).expect("read");
        assert!(
            written.contains("set -g mouse on"),
            "must not clobber existing settings: {written}"
        );
        assert!(written.contains("extended-keys on"));
    }

    #[test]
    fn applying_wezterm_setup_writes_a_loadable_config_when_absent() {
        let home = tempfile::tempdir().expect("tempdir");
        let result = apply_wezterm(home.path()).expect("apply");
        assert!(matches!(result, Applied::Changed { .. }), "got {result:?}");

        let written =
            std::fs::read_to_string(wezterm_config_path(home.path())).expect("read config");
        // A WezTerm config is only valid if it returns the config object.
        assert!(written.contains("config_builder()"));
        assert!(written.contains(WEZTERM_CONFIG_LINE));
        assert!(written.trim_end().ends_with("return config"));
        assert!(wezterm_config_is_configured(&written));
        assert_eq!(
            apply_wezterm(home.path()).expect("second apply"),
            Applied::AlreadyConfigured
        );
    }

    #[test]
    fn applying_wezterm_setup_never_rewrites_an_existing_config() {
        let home = tempfile::tempdir().expect("tempdir");
        let original = "local wezterm = require 'wezterm'\nreturn { font_size = 12 }\n";
        std::fs::write(wezterm_config_path(home.path()), original).expect("seed");

        let result = apply_wezterm(home.path()).expect("apply");
        assert!(
            matches!(result, Applied::Manual { .. }),
            "editing an unfamiliar Lua config could break it, so instruct instead: {result:?}"
        );
        assert_eq!(
            std::fs::read_to_string(wezterm_config_path(home.path())).expect("reread"),
            original,
            "the user's config must be left untouched"
        );
    }

    #[test]
    fn apple_terminal_guidance_names_the_real_cause_and_a_real_fix() {
        let Applied::Manual { message } = apple_terminal_guidance() else {
            panic!("Terminal.app cannot be fixed by writing config");
        };
        // The user needs to know why, and what actually resolves it.
        assert!(message.contains("0x0d"));
        assert!(message.contains("Ghostty") && message.contains("iTerm2"));
        assert!(message.contains(&apple_terminal_shift_return_value()));
    }

    #[test]
    fn shift_enter_csi_u_matches_the_kitty_protocol_encoding() {
        // keycode 13 = Enter, modifier 2 = 1 + shift bit. crossterm decodes this
        // back into KeyCode::Enter + KeyModifiers::SHIFT.
        assert_eq!(SHIFT_ENTER_CSI_U, "\x1b[13;2u");
        assert_eq!(
            SHIFT_ENTER_HEX_CODES,
            SHIFT_ENTER_CSI_U.as_bytes(),
            "the hex codes we write into terminal configs must be the exact \
             sequence crossterm parses"
        );
    }

    #[test]
    fn apple_terminal_value_spells_the_escape_in_octal() {
        assert_eq!(apple_terminal_shift_return_value(), "\\033[13;2u");
    }

    #[test]
    fn apple_terminal_key_encodes_shift_plus_return() {
        // `$` is Terminal.app's Shift prefix; \015 is octal for 0x0d (Return).
        assert_eq!(APPLE_TERMINAL_SHIFT_RETURN_KEY, "$\\015");
        assert_eq!(u8::from_str_radix("15", 8).unwrap(), b'\r');
    }

    #[test]
    fn tmux_block_sets_every_option_required_for_forwarding() {
        // Missing any one of these silently leaves Shift+Enter broken, so assert
        // all three rather than just that we wrote something.
        assert!(TMUX_CONFIG_BLOCK.contains("set -s extended-keys on"));
        assert!(TMUX_CONFIG_BLOCK.contains("set -s extended-keys-format csi-u"));
        assert!(TMUX_CONFIG_BLOCK.contains("terminal-features 'xterm*:extkeys'"));
    }

    #[test]
    fn tmux_block_is_appended_to_an_existing_config_with_a_blank_line() {
        let updated = tmux_config_with_block("set -g mouse on").expect("should update");
        assert!(updated.starts_with("set -g mouse on\n\n"));
        assert!(updated.contains("extended-keys on"));
    }

    #[test]
    fn tmux_block_is_not_appended_twice() {
        let once = tmux_config_with_block("").expect("should update");
        assert!(
            tmux_config_with_block(&once).is_none(),
            "re-running setup must be idempotent"
        );
    }

    #[test]
    fn tmux_hand_written_equivalent_config_is_respected() {
        // A user who already configured this by hand should not get a duplicate.
        let hand_rolled = "set -s extended-keys on\nset -s extended-keys-format csi-u\n";
        assert!(tmux_config_is_configured(hand_rolled));
        assert!(tmux_config_with_block(hand_rolled).is_none());
    }

    #[test]
    fn wezterm_detects_the_enabled_flag_and_ignores_comments() {
        assert!(wezterm_config_is_configured(
            "config.enable_kitty_keyboard = true"
        ));
        assert!(!wezterm_config_is_configured(
            "-- config.enable_kitty_keyboard = true"
        ));
        assert!(!wezterm_config_is_configured(
            "config.enable_kitty_keyboard = false"
        ));
        assert!(!wezterm_config_is_configured("config.font_size = 12"));
    }

    #[test]
    fn config_paths_are_the_documented_locations() {
        let home = Path::new("/home/u");
        assert_eq!(tmux_config_path(home), Path::new("/home/u/.tmux.conf"));
        assert_eq!(wezterm_config_path(home), Path::new("/home/u/.wezterm.lua"));
        assert_eq!(
            apple_terminal_plist_path(home),
            Path::new("/home/u/Library/Preferences/com.apple.Terminal.plist")
        );
    }

    #[test]
    fn every_target_explains_how_to_activate_the_change() {
        // Writing config without telling the user to reload leaves them thinking
        // the fix did not work.
        for target in [
            SetupTarget::AppleTerminal,
            SetupTarget::Tmux,
            SetupTarget::WezTerm,
        ] {
            assert!(!target.label().is_empty());
            assert!(
                target.activation_note().len() > 20,
                "{} needs an actionable activation note",
                target.label()
            );
        }
    }
}
