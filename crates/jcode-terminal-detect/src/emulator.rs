//! Terminal emulator identification.

use std::fmt;

/// Identifies a known terminal emulator or stores the `TERM_PROGRAM` value
/// for unknown terminals.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TerminalEmulator {
    Ghostty,
    WezTerm,
    Rio,
    WindowsTerminal,
    Kitty,
    ITerm2,
    Alacritty,
    Tilix,
    Hyper,
    Terminology,
    Foot,
    LinuxConsole,
    MacTerminal,
    Unknown(String),
}

impl TerminalEmulator {
    /// Returns true when the emulator is known (not [`Unknown`](Self::Unknown)).
    pub fn is_known(&self) -> bool {
        !matches!(self, Self::Unknown(_))
    }

    /// Human-friendly display name.
    pub fn display_name(&self) -> &str {
        match self {
            Self::Ghostty => "Ghostty",
            Self::WezTerm => "WezTerm",
            Self::Rio => "Rio",
            Self::WindowsTerminal => "Windows Terminal",
            Self::Kitty => "Kitty",
            Self::ITerm2 => "iTerm2",
            Self::Alacritty => "Alacritty",
            Self::Tilix => "Tilix",
            Self::Hyper => "Hyper",
            Self::Terminology => "Terminology",
            Self::Foot => "Foot",
            Self::LinuxConsole => "Linux Console",
            Self::MacTerminal => "Terminal.app",
            Self::Unknown(name) => name,
        }
    }
}

impl fmt::Display for TerminalEmulator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}
