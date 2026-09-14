//! Feature detection and capability reporting.

use crate::emulator::TerminalEmulator;

/// How deeply the running shell has been integrated with jcode markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShellIntegrationLevel {
    /// No shell integration detected.
    None,
    /// Basic OSC 133 prompt markers.
    Basic,
    /// Prompt markers + CWD reporting + command timing.
    Full,
    /// Terminal-native auto-injection (e.g. Ghostty).
    AutoInjected,
}

/// Feature flags derived from the detected terminal emulator.
#[derive(Debug, Clone)]
pub struct FeatureSet {
    pub emulator: TerminalEmulator,
    /// Kitty inline-image protocol (DCS `q`).
    pub kitty_graphics: bool,
    /// Kitty keyboard protocol (CSI `>u` / CSI `<u`).
    pub kitty_keyboard: bool,
    /// Sixel graphics.
    pub sixel: bool,
    /// OSC 8 clickable hyperlinks.
    pub osc8_hyperlinks: bool,
    /// OSC 133 semantic prompt markers (FTCS).
    pub osc133_prompts: bool,
    /// Synchronized update (DECSET/DECRST 2026).
    pub synchronized_update: bool,
    /// OSC 52 clipboard read/write.
    pub osc52_clipboard: bool,
    /// 24-bit / truecolor support.
    pub truecolor: bool,
    /// Light/dark mode notifications (DECSET 997).
    pub light_dark_mode: bool,
    /// Shell integration level detected.
    pub shell_integration: ShellIntegrationLevel,
    /// Maximum number of colours the terminal reports.
    pub max_colors: u32,
    /// Terminal width in columns (0 = unknown).
    pub columns: u16,
    /// Terminal height in rows (0 = unknown).
    pub rows: u16,
}

impl FeatureSet {
    /// True when the terminal supports at least one image protocol.
    pub fn has_image_support(&self) -> bool {
        self.kitty_graphics || self.sixel
    }

    /// True when both prompt marking and CWD reporting are available.
    pub fn has_osc133(&self) -> bool {
        self.osc133_prompts
    }
}
