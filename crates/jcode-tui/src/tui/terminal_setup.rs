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
    /// The config was written; `detail` says what changed and `note` how to
    /// activate it, both naming the file that was actually written.
    Changed { detail: String, note: String },
    /// Already configured, nothing to do.
    AlreadyConfigured {
        /// The config file that already carries the setting, when known.
        path: Option<PathBuf>,
    },
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

    /// How to activate a change, naming `config` when the caller knows which
    /// file was touched.
    ///
    /// The tmux hint must name the real file: a user whose config lives at
    /// `~/.config/tmux/tmux.conf` cannot reload it with `tmux source
    /// ~/.tmux.conf`.
    pub fn activation_note(self, config: Option<&Path>) -> String {
        match self {
            Self::AppleTerminal => {
                "Quit and reopen Terminal.app to pick up the key mapping.".to_string()
            }
            Self::Tmux => match config {
                Some(path) => format!(
                    "Run `tmux source-file {}`, then restart jcode.",
                    path.display()
                ),
                None => "Run `tmux source-file ~/.tmux.conf`, then restart jcode.".to_string(),
            },
            Self::WezTerm => "Restart WezTerm to pick up the config change.".to_string(),
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

/// `$XDG_CONFIG_HOME` when it is an absolute path, else `home/.config`.
///
/// An empty or relative `XDG_CONFIG_HOME` is ignored by the XDG spec, so a
/// stray value must not redirect a write somewhere unexpected.
pub fn resolve_xdg_config_home(home: &Path, configured: Option<&Path>) -> PathBuf {
    match configured {
        Some(dir) if dir.is_absolute() => dir.to_path_buf(),
        _ => home.join(".config"),
    }
}

/// `$XDG_CONFIG_HOME` from the environment, resolved against `home`.
pub fn default_xdg_config_home(home: &Path) -> PathBuf {
    let configured = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    resolve_xdg_config_home(home, configured.as_deref())
}

/// The files tmux may read, in tmux's own default search order.
///
/// tmux reads `~/.tmux.conf` and otherwise the XDG location, and some setups
/// load only one of them. Writing a file that is not the one tmux reads leaves
/// the user with two configs and no working chord, so every candidate has to be
/// considered before anything is written.
pub fn tmux_config_candidates(home: &Path, xdg: &Path) -> Vec<PathBuf> {
    vec![home.join(".tmux.conf"), xdg.join("tmux").join("tmux.conf")]
}

/// The files WezTerm may read, in WezTerm's documented order.
///
/// WezTerm prefers the XDG location and falls back to the legacy dotfile.
pub fn wezterm_config_candidates(home: &Path, xdg: &Path) -> Vec<PathBuf> {
    vec![
        xdg.join("wezterm").join("wezterm.lua"),
        home.join(".wezterm.lua"),
    ]
}

/// Where a first tmux config is created when none exists.
///
/// Only used when no candidate exists, so it never competes with a real config.
pub fn tmux_config_path(home: &Path) -> PathBuf {
    home.join(".tmux.conf")
}

/// Where a first WezTerm config is created when none exists.
pub fn wezterm_config_path(home: &Path) -> PathBuf {
    home.join(".wezterm.lua")
}

/// The config files the running tmux server actually loaded.
///
/// `tmux display-message -p '#{config_files}'` is the only authoritative answer
/// when a wrapper or plugin manager starts tmux with an explicit `-f`. Returns
/// an empty list outside tmux or when the query fails, in which case the static
/// search order decides.
pub fn tmux_loaded_config_files() -> Vec<PathBuf> {
    // Outside tmux there is no server to ask, and spawning a client just to
    // watch it fail is wasted work on every setup run.
    if std::env::var_os("TMUX").is_none() {
        return Vec::new();
    }
    let output = match std::process::Command::new("tmux")
        .args(["display-message", "-p", "#{config_files}"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => return Vec::new(),
    };
    parse_tmux_config_files(&String::from_utf8_lossy(&output.stdout))
}

/// Split tmux's space-separated `#{config_files}` reply into paths.
pub fn parse_tmux_config_files(raw: &str) -> Vec<PathBuf> {
    raw.split_whitespace().map(PathBuf::from).collect()
}

/// Where a tmux write should go, and what else tmux may read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxConfigSelection {
    /// The file to edit, or `None` when several configs exist and nothing says
    /// which one the server loaded.
    pub target: Option<PathBuf>,
    /// Other existing config files, reported so the user sees both.
    pub others: Vec<PathBuf>,
}

/// Decide which tmux config to edit.
///
/// Order of evidence: the file the running server reports, then a single
/// existing candidate, then the default creation path. Several existing files
/// with no server answer is ambiguous, and guessing would risk writing a config
/// that changes which settings tmux loads.
pub fn select_tmux_config(home: &Path, xdg: &Path, loaded: &[PathBuf]) -> TmuxConfigSelection {
    let candidates = tmux_config_candidates(home, xdg);
    let existing: Vec<PathBuf> = candidates.iter().filter(|p| p.is_file()).cloned().collect();
    let target = loaded
        .iter()
        .find(|p| p.is_file())
        .cloned()
        .or_else(|| match existing.len() {
            0 => Some(tmux_config_path(home)),
            1 => existing.first().cloned(),
            _ => None,
        });
    let others = existing
        .into_iter()
        .filter(|path| Some(path) != target.as_ref())
        .collect();
    TmuxConfigSelection { target, others }
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

// Only a missing config is safe to treat as empty. In particular, invalid
// UTF-8 must not turn an existing user's config into a new generated file.
fn read_config_or_empty(path: &Path) -> std::io::Result<String> {
    match std::fs::read_to_string(path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        result => result,
    }
}

/// Configure tmux so a capable outer terminal's Shift+Enter reaches jcode.
pub fn apply_tmux(home: &Path) -> std::io::Result<Applied> {
    apply_tmux_with(
        home,
        &default_xdg_config_home(home),
        &tmux_loaded_config_files(),
    )
}

/// [`apply_tmux`] with its discovery inputs injected.
///
/// Tests use this so they never read the real environment or talk to a running
/// tmux server.
pub fn apply_tmux_with(home: &Path, xdg: &Path, loaded: &[PathBuf]) -> std::io::Result<Applied> {
    let selection = select_tmux_config(home, xdg, loaded);
    let mut known = selection.others.clone();
    known.extend(selection.target.clone());
    // A config tmux already reads that carries the setting is enough. Writing
    // another file would add a second config that tmux may never load, which is
    // exactly the state that makes Shift+Enter appear broken after "setup".
    for path in &known {
        if tmux_config_is_configured(&read_config_or_empty(path)?) {
            return Ok(Applied::AlreadyConfigured {
                path: Some(path.clone()),
            });
        }
    }
    let Some(path) = selection.target else {
        return Ok(Applied::Manual {
            message: ambiguous_tmux_config_message(&selection.others),
        });
    };
    let existed = path.is_file();
    let existing = read_config_or_empty(&path)?;
    let Some(updated) = tmux_config_with_block(&existing) else {
        return Ok(Applied::AlreadyConfigured { path: Some(path) });
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let appended = updated
        .lines()
        .count()
        .saturating_sub(existing.lines().count());
    std::fs::write(&path, updated)?;
    let mut detail = if existed {
        format!(
            "Appended {} lines to {}\n\n{}",
            appended,
            path.display(),
            TMUX_CONFIG_BLOCK.trim_end()
        )
    } else {
        format!(
            "Created new file {}\n\n{}",
            path.display(),
            TMUX_CONFIG_BLOCK.trim_end()
        )
    };
    if !selection.others.is_empty() {
        detail.push_str(&format!(
            "\n\nAlso found {}, which tmux may read instead. jcode did not touch it.",
            display_paths(&selection.others)
        ));
    }
    Ok(Applied::Changed {
        detail,
        note: SetupTarget::Tmux.activation_note(Some(&path)),
    })
}

/// What to tell the user when several tmux configs exist and nothing says which
/// one tmux reads.
fn ambiguous_tmux_config_message(existing: &[PathBuf]) -> String {
    format!(
        "Found more than one tmux config, so jcode cannot tell which one tmux reads:\n{}\n\
         Run /terminal-setup from inside tmux, where the server reports the file it loaded, \
         or add these lines to the config you use:\n\n{}",
        display_paths(existing),
        TMUX_CONFIG_BLOCK.trim_end()
    )
}

/// Render paths one per line, indented, for inclusion in a message.
fn display_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| format!("  {}", path.display()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Configure WezTerm to opt in to the kitty keyboard protocol.
pub fn apply_wezterm(home: &Path) -> std::io::Result<Applied> {
    apply_wezterm_with(home, &default_xdg_config_home(home))
}

/// [`apply_wezterm`] with the XDG directory injected.
pub fn apply_wezterm_with(home: &Path, xdg: &Path) -> std::io::Result<Applied> {
    // WezTerm reads the XDG location first and the legacy dotfile second, so an
    // existing config must be edited where it lives rather than shadowed by a
    // new `~/.wezterm.lua`.
    let path = wezterm_config_candidates(home, xdg)
        .into_iter()
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| wezterm_config_path(home));
    let existed = path.is_file();
    let existing = read_config_or_empty(&path)?;
    if wezterm_config_is_configured(&existing) {
        return Ok(Applied::AlreadyConfigured { path: Some(path) });
    }
    // WezTerm's config is Lua that must `return` a config object, so an
    // automatic edit can only be trusted for the simple case where we are
    // creating the file. Otherwise, hand the user the exact line to add rather
    // than risk corrupting a config we do not understand.
    if existing.trim().is_empty() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
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
            detail: if existed {
                format!(
                    "Filled in the empty {}\n\n{WEZTERM_CONFIG_LINE}",
                    path.display()
                )
            } else {
                format!(
                    "Created new file {}\n\n{WEZTERM_CONFIG_LINE}",
                    path.display()
                )
            },
            note: SetupTarget::WezTerm.activation_note(Some(&path)),
        });
    }
    Ok(Applied::Manual {
        message: format!(
            "For Shift+Enter support, add `{WEZTERM_CONFIG_LINE}` to {} (before `return config`).",
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
        let xdg = home.path().join(".config");
        assert!(matches!(
            apply_tmux_with(home.path(), &xdg, &[]).expect("tmux"),
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
        let xdg = home.path().join(".config");
        let first = apply_tmux_with(home.path(), &xdg, &[]).expect("first apply");
        assert!(matches!(first, Applied::Changed { .. }), "got {first:?}");

        let written = std::fs::read_to_string(tmux_config_path(home.path())).expect("read config");
        assert!(written.contains("set -s extended-keys on"));
        assert!(written.contains("csi-u"));

        // Re-running must not duplicate the block.
        assert_eq!(
            apply_tmux_with(home.path(), &xdg, &[]).expect("second apply"),
            Applied::AlreadyConfigured {
                path: Some(tmux_config_path(home.path()))
            }
        );
        assert_eq!(
            std::fs::read_to_string(tmux_config_path(home.path())).expect("reread"),
            written
        );
    }

    #[test]
    fn applying_tmux_setup_preserves_existing_user_config() {
        let home = tempfile::tempdir().expect("tempdir");
        let xdg = home.path().join(".config");
        std::fs::write(tmux_config_path(home.path()), "set -g mouse on\n").expect("seed");

        apply_tmux_with(home.path(), &xdg, &[]).expect("apply");

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
        let xdg = home.path().join(".config");
        let result = apply_wezterm_with(home.path(), &xdg).expect("apply");
        assert!(matches!(result, Applied::Changed { .. }), "got {result:?}");

        let written =
            std::fs::read_to_string(wezterm_config_path(home.path())).expect("read config");
        // A WezTerm config is only valid if it returns the config object.
        assert!(written.contains("config_builder()"));
        assert!(written.contains(WEZTERM_CONFIG_LINE));
        assert!(written.trim_end().ends_with("return config"));
        assert!(wezterm_config_is_configured(&written));
        assert_eq!(
            apply_wezterm_with(home.path(), &xdg).expect("second apply"),
            Applied::AlreadyConfigured {
                path: Some(wezterm_config_path(home.path()))
            }
        );
    }

    #[test]
    fn applying_tmux_setup_preserves_invalid_utf8_config() {
        let home = tempfile::tempdir().expect("tempdir");
        let xdg = home.path().join(".config");
        let path = tmux_config_path(home.path());
        let original = b"# user settings\n\xff\n";
        std::fs::write(&path, original).expect("seed config");

        let result = apply_tmux_with(home.path(), &xdg, &[]);

        assert_eq!(std::fs::read(&path).expect("reread config"), original);
        assert_eq!(
            result.expect_err("must reject unreadable config").kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn applying_wezterm_setup_preserves_invalid_utf8_config() {
        let home = tempfile::tempdir().expect("tempdir");
        let xdg = home.path().join(".config");
        let path = wezterm_config_path(home.path());
        let original = b"-- user settings\n\xff\n";
        std::fs::write(&path, original).expect("seed config");

        let result = apply_wezterm_with(home.path(), &xdg);

        assert_eq!(std::fs::read(&path).expect("reread config"), original);
        assert_eq!(
            result.expect_err("must reject unreadable config").kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn applying_wezterm_setup_never_rewrites_an_existing_config() {
        let home = tempfile::tempdir().expect("tempdir");
        let xdg = home.path().join(".config");
        let original = "local wezterm = require 'wezterm'\nreturn { font_size = 12 }\n";
        std::fs::write(wezterm_config_path(home.path()), original).expect("seed");

        let result = apply_wezterm_with(home.path(), &xdg).expect("apply");
        assert!(
            matches!(result, Applied::Manual { ref message }
                if message.contains("Shift+Enter") && message.contains(WEZTERM_CONFIG_LINE)),
            "must explain Shift+Enter setup without editing unfamiliar Lua: {result:?}"
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
        let xdg = Path::new("/home/u/.config");
        assert_eq!(
            tmux_config_candidates(home, xdg),
            vec![
                PathBuf::from("/home/u/.tmux.conf"),
                PathBuf::from("/home/u/.config/tmux/tmux.conf"),
            ],
            "tmux reads the dotfile first and the XDG file otherwise"
        );
        assert_eq!(
            wezterm_config_candidates(home, xdg),
            vec![
                PathBuf::from("/home/u/.config/wezterm/wezterm.lua"),
                PathBuf::from("/home/u/.wezterm.lua"),
            ],
            "WezTerm prefers XDG and falls back to the legacy dotfile"
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
                target.activation_note(None).len() > 20,
                "{} needs an actionable activation note",
                target.label()
            );
        }
    }

    #[test]
    fn tmux_reload_note_names_the_file_that_was_written() {
        // `tmux source ~/.tmux.conf` is wrong advice for a user whose config is
        // the XDG file, and the old code always printed exactly that.
        let note =
            SetupTarget::Tmux.activation_note(Some(Path::new("/home/u/.config/tmux/tmux.conf")));
        assert!(note.contains("/home/u/.config/tmux/tmux.conf"), "{note}");
        assert!(note.contains("tmux source-file"), "{note}");
    }

    /// Home plus the XDG directory tests inject, so nothing reads the real env.
    fn isolated_home() -> (tempfile::TempDir, PathBuf) {
        let home = tempfile::tempdir().expect("tempdir");
        let xdg = home.path().join(".config");
        (home, xdg)
    }

    #[test]
    fn tmux_setup_edits_the_xdg_config_tmux_actually_reads() {
        // Issue #1972: a user whose only config is the XDG file got a second
        // `~/.tmux.conf` created, which tmux never reads, so the chord stayed
        // broken while jcode reported success.
        let (home, xdg) = isolated_home();
        let xdg_config = xdg.join("tmux").join("tmux.conf");
        std::fs::create_dir_all(xdg_config.parent().expect("parent")).expect("mkdir");
        std::fs::write(&xdg_config, "set -g mouse on\n").expect("seed");

        let result = apply_tmux_with(home.path(), &xdg, &[]).expect("apply");

        let Applied::Changed { detail, note } = result else {
            panic!("expected the XDG config to be updated, got {result:?}");
        };
        assert!(
            detail.contains(&xdg_config.display().to_string()),
            "{detail}"
        );
        assert!(note.contains(&xdg_config.display().to_string()), "{note}");
        let written = std::fs::read_to_string(&xdg_config).expect("reread");
        assert!(written.contains("set -g mouse on"), "{written}");
        assert!(tmux_config_is_configured(&written), "{written}");
        assert!(
            !tmux_config_path(home.path()).exists(),
            "must not create a second config that tmux does not read"
        );
    }

    #[test]
    fn tmux_setup_prefers_the_config_the_running_server_reported() {
        // A plugin manager or a `tmux -f` wrapper can load a file that the
        // static search order would not pick, and the server knows which.
        let (home, xdg) = isolated_home();
        let xdg_config = xdg.join("tmux").join("tmux.conf");
        std::fs::create_dir_all(xdg_config.parent().expect("parent")).expect("mkdir");
        std::fs::write(&xdg_config, "set -g mouse on\n").expect("seed");
        let dotfile = tmux_config_path(home.path());
        std::fs::write(&dotfile, "set -g status on\n").expect("seed");

        let loaded = vec![xdg_config.clone()];
        let result = apply_tmux_with(home.path(), &xdg, &loaded).expect("apply");

        let Applied::Changed { detail, .. } = result else {
            panic!("expected a write, got {result:?}");
        };
        assert!(tmux_config_is_configured(
            &std::fs::read_to_string(&xdg_config).expect("reread xdg")
        ));
        assert_eq!(
            std::fs::read_to_string(&dotfile).expect("reread dotfile"),
            "set -g status on\n",
            "the file tmux did not load must be left alone"
        );
        assert!(detail.contains(&dotfile.display().to_string()), "{detail}");
        assert!(detail.contains("did not touch"), "{detail}");
    }

    #[test]
    fn tmux_setup_asks_when_two_configs_exist_and_the_server_is_silent() {
        // Guessing here can change which settings tmux loads, so the only safe
        // answer is to name both files and let the user choose.
        let (home, xdg) = isolated_home();
        let xdg_config = xdg.join("tmux").join("tmux.conf");
        std::fs::create_dir_all(xdg_config.parent().expect("parent")).expect("mkdir");
        std::fs::write(&xdg_config, "set -g mouse on\n").expect("seed");
        let dotfile = tmux_config_path(home.path());
        std::fs::write(&dotfile, "set -g status on\n").expect("seed");

        let result = apply_tmux_with(home.path(), &xdg, &[]).expect("apply");

        let Applied::Manual { message } = result else {
            panic!("ambiguous configs must not be edited, got {result:?}");
        };
        assert!(
            message.contains(&dotfile.display().to_string()),
            "{message}"
        );
        assert!(
            message.contains(&xdg_config.display().to_string()),
            "{message}"
        );
        assert!(message.contains("extended-keys on"), "{message}");
        assert_eq!(
            std::fs::read_to_string(&dotfile).expect("reread dotfile"),
            "set -g status on\n"
        );
        assert_eq!(
            std::fs::read_to_string(&xdg_config).expect("reread xdg"),
            "set -g mouse on\n"
        );
    }

    #[test]
    fn tmux_setup_respects_a_setting_the_other_candidate_already_has() {
        // If any config tmux may read is configured, writing another file adds a
        // config without fixing anything.
        let (home, xdg) = isolated_home();
        let xdg_config = xdg.join("tmux").join("tmux.conf");
        std::fs::create_dir_all(xdg_config.parent().expect("parent")).expect("mkdir");
        std::fs::write(&xdg_config, TMUX_CONFIG_BLOCK).expect("seed");
        let dotfile = tmux_config_path(home.path());
        std::fs::write(&dotfile, "set -g status on\n").expect("seed");

        let result = apply_tmux_with(home.path(), &xdg, &[]).expect("apply");

        assert_eq!(
            result,
            Applied::AlreadyConfigured {
                path: Some(xdg_config.clone())
            }
        );
        assert_eq!(
            std::fs::read_to_string(&dotfile).expect("reread dotfile"),
            "set -g status on\n"
        );
    }

    #[test]
    fn tmux_setup_reports_created_versus_appended() {
        // The user has to be able to tell "jcode made a new file" from "jcode
        // edited mine", which is the other half of #1972.
        let (home, xdg) = isolated_home();
        let fresh = apply_tmux_with(home.path(), &xdg, &[]).expect("apply");
        let Applied::Changed { detail, .. } = fresh else {
            panic!("expected creation, got {fresh:?}");
        };
        assert!(detail.contains("Created new file"), "{detail}");
        assert!(detail.contains(&tmux_config_path(home.path()).display().to_string()));

        std::fs::write(tmux_config_path(home.path()), "set -g mouse on\n").expect("seed");
        let appended = apply_tmux_with(home.path(), &xdg, &[]).expect("apply again");
        let Applied::Changed { detail, .. } = appended else {
            panic!("expected an append, got {appended:?}");
        };
        // The block plus the blank separator line.
        assert!(
            detail.contains(&format!(
                "Appended {} lines to",
                TMUX_CONFIG_BLOCK.lines().count() + 1
            )),
            "{detail}"
        );
    }

    #[test]
    fn tmux_setup_creates_the_default_config_only_when_none_exists() {
        let (home, xdg) = isolated_home();
        let selection = select_tmux_config(home.path(), &xdg, &[]);
        assert_eq!(selection.target, Some(tmux_config_path(home.path())));
        assert!(selection.others.is_empty());
    }

    #[test]
    fn wezterm_setup_edits_the_xdg_config_instead_of_creating_a_dotfile() {
        let (home, xdg) = isolated_home();
        let xdg_config = xdg.join("wezterm").join("wezterm.lua");
        std::fs::create_dir_all(xdg_config.parent().expect("parent")).expect("mkdir");
        let original = "local wezterm = require 'wezterm'\nreturn { font_size = 12 }\n";
        std::fs::write(&xdg_config, original).expect("seed");

        let result = apply_wezterm_with(home.path(), &xdg).expect("apply");

        let Applied::Manual { message } = result else {
            panic!("unfamiliar Lua must not be edited, got {result:?}");
        };
        assert!(
            message.contains(&xdg_config.display().to_string()),
            "{message}"
        );
        assert!(
            !wezterm_config_path(home.path()).exists(),
            "the XDG config is the one WezTerm loads, so no dotfile may be created"
        );
    }

    #[test]
    fn tmux_server_reply_is_split_into_config_paths() {
        // `#{config_files}` is space separated and often has a trailing newline.
        assert_eq!(
            parse_tmux_config_files("/home/u/.tmux.conf /home/u/x.conf\n"),
            vec![
                PathBuf::from("/home/u/.tmux.conf"),
                PathBuf::from("/home/u/x.conf"),
            ]
        );
        assert!(parse_tmux_config_files("").is_empty());
    }

    #[test]
    fn xdg_config_home_falls_back_for_relative_or_missing_values() {
        let home = Path::new("/home/u");
        // The XDG spec says a relative value must be ignored, and honoring it
        // would point writes at the process working directory.
        assert_eq!(
            resolve_xdg_config_home(home, Some(Path::new("relative"))),
            Path::new("/home/u/.config")
        );
        assert_eq!(
            resolve_xdg_config_home(home, None),
            Path::new("/home/u/.config")
        );
        assert_eq!(
            resolve_xdg_config_home(home, Some(Path::new("/xdg"))),
            Path::new("/xdg")
        );
    }
}
