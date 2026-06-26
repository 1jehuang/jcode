//! Where a discovered key binding came from, and the binding record itself.

use serde::{Deserialize, Serialize};

use super::chord::KeyChord;

/// The origin of a discovered binding on the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeySource {
    /// A macOS system-wide shortcut (`com.apple.symbolichotkeys`).
    MacosSystem,
    /// A binding declared by the terminal emulator (config or built-in default).
    Terminal,
}

impl KeySource {
    pub fn label(self) -> &'static str {
        match self {
            KeySource::MacosSystem => "macOS system shortcut",
            KeySource::Terminal => "terminal",
        }
    }
}

/// A key binding discovered on the machine that may intercept input before it
/// reaches jcode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredBinding {
    /// The normalized chord this binding triggers on.
    pub chord: KeyChord,
    /// Which layer owns this binding.
    pub source: KeySource,
    /// What the binding does, e.g. "Spotlight: Show search" or
    /// "copy_to_clipboard:mixed".
    pub action: String,
    /// The raw declaration we parsed, for debugging (e.g. the original config
    /// line or the symbolic-hotkey id).
    pub raw: String,
}
