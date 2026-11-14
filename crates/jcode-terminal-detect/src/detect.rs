//! Environment-based terminal detection.

use crate::capabilities::{FeatureSet, ShellIntegrationLevel};
use crate::emulator::TerminalEmulator;

/// Detect the terminal emulator and its feature set from environment
/// variables. This is purely env-var based and never sends escape
/// sequences to the TTY.
pub fn detect() -> FeatureSet {
    let emulator = identify_emulator();
    features_for(&emulator)
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn identify_emulator() -> TerminalEmulator {
    // 1. Ghostty sets GHOSTTY_RESOURCES_DIR
    if env("GHOSTTY_RESOURCES_DIR").is_some() {
        return TerminalEmulator::Ghostty;
    }
    // 2. WezTerm
    if env("WEZTERM_CONFIG_FILE").is_some() || env("WEZTERM_PANE").is_some() {
        return TerminalEmulator::WezTerm;
    }
    // 3. Rio
    if env("RIO_CONFIG_PATH").is_some() {
        return TerminalEmulator::Rio;
    }
    // 4. Windows Terminal
    if env("WT_SESSION").is_some() {
        return TerminalEmulator::WindowsTerminal;
    }
    // 5. Kitty
    if env("KITTY_WINDOW_ID").is_some() {
        return TerminalEmulator::Kitty;
    }
    // 6. iTerm2
    if let Some(ref tp) = env("TERM_PROGRAM") {
        if tp == "iTerm.app" {
            return TerminalEmulator::ITerm2;
        }
        if tp == "Apple_Terminal" {
            return TerminalEmulator::MacTerminal;
        }
    }
    // 7. TERM-based fallbacks
    if let Some(ref term) = env("TERM") {
        if term.contains("ghostty") {
            return TerminalEmulator::Ghostty;
        }
        if term.contains("wezterm") {
            return TerminalEmulator::WezTerm;
        }
        if term.contains("kitty") {
            return TerminalEmulator::Kitty;
        }
        if term.contains("alacritty") {
            return TerminalEmulator::Alacritty;
        }
        if term.contains("rio") {
            return TerminalEmulator::Rio;
        }
        if term.contains("foot") {
            return TerminalEmulator::Foot;
        }
        if term.contains("linux") || term == "linux" {
            return TerminalEmulator::LinuxConsole;
        }
    }
    // 8. VTE-based terminals (Tilix, GNOME Terminal, etc.)
    if env("VTE_VERSION").is_some() {
        return TerminalEmulator::Tilix;
    }
    // 9. Unknown
    let name = env("TERM_PROGRAM")
        .or_else(|| env("TERM"))
        .unwrap_or_else(|| "unknown".to_string());
    TerminalEmulator::Unknown(name)
}

fn features_for(em: &TerminalEmulator) -> FeatureSet {
    let (kitty_gfx, kitty_kbd, sixel, osc8, osc133, sync, osc52, truecolor, lightdark, shell) =
        match em {
            TerminalEmulator::Ghostty => (true, true, true, true, true, true, true, true, true, ShellIntegrationLevel::AutoInjected),
            TerminalEmulator::WezTerm => (true, true, true, true, true, true, true, true, false, ShellIntegrationLevel::Full),
            TerminalEmulator::Rio => (false, true, true, true, true, true, true, true, false, ShellIntegrationLevel::Basic),
            TerminalEmulator::WindowsTerminal => (false, false, false, true, true, true, true, true, true, ShellIntegrationLevel::Basic),
            TerminalEmulator::Kitty => (true, true, false, true, true, true, true, true, false, ShellIntegrationLevel::Full),
            TerminalEmulator::ITerm2 => (false, false, false, true, true, false, true, true, false, ShellIntegrationLevel::Full),
            TerminalEmulator::Alacritty => (false, false, false, false, false, true, false, true, false, ShellIntegrationLevel::None),
            TerminalEmulator::Tilix => (false, false, false, false, false, false, false, true, false, ShellIntegrationLevel::None),
            TerminalEmulator::Hyper => (false, false, false, false, false, false, false, true, false, ShellIntegrationLevel::None),
            TerminalEmulator::Terminology => (false, false, false, false, false, false, false, true, false, ShellIntegrationLevel::None),
            TerminalEmulator::Foot => (false, false, false, true, true, true, true, true, false, ShellIntegrationLevel::Basic),
            TerminalEmulator::LinuxConsole => (false, false, false, false, false, false, false, false, false, ShellIntegrationLevel::None),
            TerminalEmulator::MacTerminal => (false, false, false, false, false, false, false, false, false, ShellIntegrationLevel::None),
            TerminalEmulator::Unknown(_) => (false, false, false, false, false, false, false, false, false, ShellIntegrationLevel::None),
        };

    // Truecolor heuristic: if COLORTERM is set to truecolor/24bit
    let tc = truecolor || env("COLORTERM").is_some_and(|v| v == "truecolor" || v == "24bit");

    // Read terminal size from env (curses/terminfo).
    let (cols, rows) = (
        env("COLUMNS").and_then(|v| v.parse().ok()).unwrap_or(0),
        env("LINES").and_then(|v| v.parse().ok()).unwrap_or(0),
    );

    // Determine max colours.
    let max_colors = if tc {
        16_777_216
    } else {
        env("COLORFGBG")
            .and_then(|v| {
                if v.contains("15") || v.contains("white") {
                    Some(256)
                } else {
                    None
                }
            })
            .unwrap_or(256)
    };

    FeatureSet {
        emulator: em.clone(),
        kitty_graphics: kitty_gfx,
        kitty_keyboard: kitty_kbd,
        sixel,
        osc8_hyperlinks: osc8,
        osc133_prompts: osc133,
        synchronized_update: sync,
        osc52_clipboard: osc52,
        truecolor: tc,
        light_dark_mode: lightdark,
        shell_integration: shell,
        max_colors,
        columns: cols,
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    // SAFETY: tests run single-threaded; we restore env after each check.
    #[test]
    fn ghostty_detected_from_resources_dir() {
        unsafe {
            env::set_var("GHOSTTY_RESOURCES_DIR", "/usr/share/ghostty");
        }
        let fs = detect();
        assert_eq!(fs.emulator, TerminalEmulator::Ghostty);
        assert!(fs.kitty_graphics);
        assert!(fs.kitty_keyboard);
        assert!(fs.synchronized_update);
        assert_eq!(fs.shell_integration, ShellIntegrationLevel::AutoInjected);
        unsafe {
            env::remove_var("GHOSTTY_RESOURCES_DIR");
        }
    }

    #[test]
    fn wezterm_detected_from_config_file() {
        unsafe {
            env::set_var("WEZTERM_CONFIG_FILE", "/home/u/.wezterm.lua");
        }
        let fs = detect();
        assert_eq!(fs.emulator, TerminalEmulator::WezTerm);
        assert!(fs.kitty_keyboard);
        assert!(fs.sixel);
        assert!(fs.osc8_hyperlinks);
        assert_eq!(fs.shell_integration, ShellIntegrationLevel::Full);
        unsafe {
            env::remove_var("WEZTERM_CONFIG_FILE");
        }
    }

    #[test]
    fn kitty_detected_from_window_id() {
        unsafe {
            env::set_var("KITTY_WINDOW_ID", "1");
        }
        let fs = detect();
        assert_eq!(fs.emulator, TerminalEmulator::Kitty);
        assert!(fs.kitty_graphics);
        assert!(!fs.sixel);
        unsafe {
            env::remove_var("KITTY_WINDOW_ID");
        }
    }

    #[test]
    fn windows_terminal_detected() {
        unsafe {
            env::set_var("WT_SESSION", "abc-123");
        }
        let fs = detect();
        assert_eq!(fs.emulator, TerminalEmulator::WindowsTerminal);
        assert!(!fs.kitty_graphics);
        assert!(fs.osc133_prompts);
        assert!(fs.light_dark_mode);
        unsafe {
            env::remove_var("WT_SESSION");
        }
    }

    #[test]
    fn iterm2_detected() {
        unsafe {
            env::set_var("TERM_PROGRAM", "iTerm.app");
        }
        let fs = detect();
        assert_eq!(fs.emulator, TerminalEmulator::ITerm2);
        assert!(fs.osc52_clipboard);
        assert!(!fs.synchronized_update);
        unsafe {
            env::remove_var("TERM_PROGRAM");
        }
    }

    #[test]
    fn rio_detected() {
        unsafe {
            env::set_var("RIO_CONFIG_PATH", "/home/u/.config/rioterm");
        }
        let fs = detect();
        assert_eq!(fs.emulator, TerminalEmulator::Rio);
        assert!(fs.sixel);
        assert!(fs.kitty_keyboard);
        assert!(!fs.kitty_graphics);
        unsafe {
            env::remove_var("RIO_CONFIG_PATH");
        }
    }

    #[test]
    fn unknown_falls_back_gracefully() {
        // Clear all relevant env vars
        unsafe {
            for var in [
                "GHOSTTY_RESOURCES_DIR", "WEZTERM_CONFIG_FILE", "WEZTERM_PANE",
                "RIO_CONFIG_PATH", "WT_SESSION", "KITTY_WINDOW_ID", "TERM_PROGRAM",
                "TERM", "VTE_VERSION",
            ] {
                env::remove_var(var);
            }
        }
        let fs = detect();
        assert!(!fs.kitty_graphics);
        assert!(!fs.osc133_prompts);
        assert_eq!(fs.shell_integration, ShellIntegrationLevel::None);
    }

    #[test]
    fn truecolor_from_colorterm() {
        unsafe {
            env::set_var("COLORTERM", "truecolor");
        }
        let fs = detect();
        assert!(fs.truecolor);
        assert_eq!(fs.max_colors, 16_777_216);
        unsafe {
            env::remove_var("COLORTERM");
        }
    }

    #[test]
    fn emulator_display_names() {
        assert_eq!(TerminalEmulator::Ghostty.display_name(), "Ghostty");
        assert_eq!(TerminalEmulator::WezTerm.display_name(), "WezTerm");
        assert_eq!(TerminalEmulator::Unknown("foo".into()).display_name(), "foo");
    }

    #[test]
    fn has_image_support() {
        let mut fs = detect();
        fs.kitty_graphics = true;
        assert!(fs.has_image_support());
        fs.kitty_graphics = false;
        fs.sixel = true;
        assert!(fs.has_image_support());
        fs.sixel = false;
        assert!(!fs.has_image_support());
    }
}
