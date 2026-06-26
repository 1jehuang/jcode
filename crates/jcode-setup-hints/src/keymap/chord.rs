//! A normalized, platform-independent representation of a key chord.
//!
//! Both jcode's own bindings and the bindings we discover on the machine
//! (terminal config, macOS system hotkeys) are reduced to a [`KeyChord`] so they
//! can be compared for conflicts regardless of where they came from.

use serde::{Deserialize, Serialize};

/// A single key combination: a set of modifiers plus one primary key token.
///
/// The `key` token is stored in a canonical lowercase form (see
/// [`KeyChord::normalize_key`]). Modifiers use jcode's vocabulary where the
/// macOS Command key maps to `cmd` (equivalent to crossterm's `SUPER`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyChord {
    #[serde(default, skip_serializing_if = "is_false")]
    pub cmd: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub ctrl: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub alt: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub shift: bool,
    pub key: String,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl KeyChord {
    /// Build a chord from a raw key token, normalizing the token.
    pub fn new(cmd: bool, ctrl: bool, alt: bool, shift: bool, key: &str) -> Self {
        Self {
            cmd,
            ctrl,
            alt,
            shift,
            key: Self::normalize_key(key),
        }
    }

    /// A stable, human-readable canonical string such as `cmd+shift+k` or
    /// `ctrl+[`. Modifier order is fixed (cmd, ctrl, alt, shift) so two chords
    /// that mean the same thing always produce the same string.
    pub fn canonical(&self) -> String {
        let mut out = String::new();
        if self.cmd {
            out.push_str("cmd+");
        }
        if self.ctrl {
            out.push_str("ctrl+");
        }
        if self.alt {
            out.push_str("alt+");
        }
        if self.shift {
            out.push_str("shift+");
        }
        out.push_str(&self.key);
        out
    }

    /// A prettier label for user-facing messages, e.g. `Cmd+Shift+K`.
    pub fn display(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.cmd {
            parts.push("Cmd".to_string());
        }
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        parts.push(pretty_key(&self.key));
        parts.join("+")
    }

    /// Parse a jcode-style binding string such as `ctrl+k`, `alt+right`, or
    /// `cmd+shift+[` into a chord. Mirrors jcode's own keybinding grammar so the
    /// conflict detector compares like with like. Returns `None` for empty or
    /// explicitly-disabled bindings (`none`/`off`/`disabled`).
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        if matches!(
            raw.to_ascii_lowercase().as_str(),
            "none" | "off" | "disabled"
        ) {
            return None;
        }

        let mut cmd = false;
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut key: Option<String> = None;

        for part in raw.split('+').map(str::trim).filter(|s| !s.is_empty()) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => ctrl = true,
                // jcode treats alt/option/meta as Alt.
                "alt" | "option" | "meta" => alt = true,
                "cmd" | "command" | "super" | "win" | "windows" => cmd = true,
                "shift" => shift = true,
                // "backtab" / "shift-tab" imply Shift+Tab.
                "backtab" | "shift-tab" => {
                    shift = true;
                    key = Some("tab".to_string());
                }
                other => key = Some(other.to_string()),
            }
        }

        let key = key?;
        Some(Self::new(cmd, ctrl, alt, shift, &key))
    }

    /// Normalize a raw key token (from any source) into a canonical token.
    /// Handles the differing spellings used by terminals (`arrow_left`,
    /// `page_up`, `digit_1`) and macOS virtual keycodes, collapsing them onto a
    /// single vocabulary shared with jcode's own keybinding parser.
    pub fn normalize_key(raw: &str) -> String {
        let k = raw.trim().to_ascii_lowercase();
        match k.as_str() {
            // Arrows (ghostty/kitty style -> jcode style)
            "arrow_left" | "left" => "left",
            "arrow_right" | "right" => "right",
            "arrow_up" | "up" => "up",
            "arrow_down" | "down" => "down",
            // Paging / navigation
            "page_up" | "pageup" | "prior" => "pageup",
            "page_down" | "pagedown" | "next" => "pagedown",
            "home" => "home",
            "end" => "end",
            "insert" => "insert",
            "delete" | "forward_delete" => "delete",
            "backspace" => "backspace",
            "return" | "enter" => "enter",
            "escape" | "esc" => "esc",
            "tab" => "tab",
            "space" => "space",
            // Named punctuation used by various terminals
            "comma" => ",",
            "period" => ".",
            "slash" => "/",
            "backslash" => "\\",
            "semicolon" => ";",
            "apostrophe" | "quote" => "'",
            "grave" | "backtick" => "`",
            "minus" => "-",
            "equal" => "=",
            "left_bracket" | "bracketleft" => "[",
            "right_bracket" | "bracketright" => "]",
            _ => {
                // digit_N -> N
                if let Some(d) = k.strip_prefix("digit_") {
                    return d.to_string();
                }
                // numpad_N -> N (best effort)
                if let Some(d) = k.strip_prefix("numpad_") {
                    return d.to_string();
                }
                // Anything else (single chars, f1..f24, etc.) passes through.
                return k;
            }
        }
        .to_string()
    }
}

fn pretty_key(key: &str) -> String {
    match key {
        "left" => "Left".to_string(),
        "right" => "Right".to_string(),
        "up" => "Up".to_string(),
        "down" => "Down".to_string(),
        "pageup" => "PageUp".to_string(),
        "pagedown" => "PageDown".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "enter" => "Enter".to_string(),
        "esc" => "Esc".to_string(),
        "tab" => "Tab".to_string(),
        "space" => "Space".to_string(),
        "backspace" => "Backspace".to_string(),
        "delete" => "Delete".to_string(),
        other => {
            if other.len() == 1 {
                other.to_ascii_uppercase()
            } else if let Some(rest) = other.strip_prefix('f') {
                if rest.chars().all(|c| c.is_ascii_digit()) && !rest.is_empty() {
                    return format!("F{rest}");
                }
                other.to_string()
            } else {
                other.to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_orders_modifiers() {
        let c = KeyChord::new(true, false, true, true, "K");
        assert_eq!(c.canonical(), "cmd+alt+shift+k");
        assert_eq!(c.display(), "Cmd+Alt+Shift+K");
    }

    #[test]
    fn normalizes_terminal_key_spellings() {
        assert_eq!(KeyChord::normalize_key("arrow_left"), "left");
        assert_eq!(KeyChord::normalize_key("page_up"), "pageup");
        assert_eq!(KeyChord::normalize_key("digit_3"), "3");
        assert_eq!(KeyChord::normalize_key("comma"), ",");
        assert_eq!(KeyChord::normalize_key("F5"), "f5");
    }

    #[test]
    fn equal_chords_compare_equal() {
        let a = KeyChord::new(true, false, false, false, "k");
        let b = KeyChord::new(true, false, false, false, "K");
        assert_eq!(a, b);
        assert_eq!(a.canonical(), b.canonical());
    }

    #[test]
    fn parses_jcode_binding_strings() {
        assert_eq!(KeyChord::parse("ctrl+k").unwrap().canonical(), "ctrl+k");
        assert_eq!(KeyChord::parse("alt+right").unwrap().canonical(), "alt+right");
        assert_eq!(
            KeyChord::parse("ctrl+shift+tab").unwrap().canonical(),
            "ctrl+shift+tab"
        );
        // Command/super alias both map to cmd.
        assert_eq!(KeyChord::parse("cmd+j").unwrap().canonical(), "cmd+j");
        assert_eq!(KeyChord::parse("super+j").unwrap().canonical(), "cmd+j");
        // backtab implies shift+tab.
        assert_eq!(KeyChord::parse("backtab").unwrap().canonical(), "shift+tab");
    }

    #[test]
    fn parse_rejects_disabled_and_empty() {
        assert!(KeyChord::parse("").is_none());
        assert!(KeyChord::parse("  ").is_none());
        assert!(KeyChord::parse("none").is_none());
        assert!(KeyChord::parse("OFF").is_none());
        assert!(KeyChord::parse("disabled").is_none());
    }

    #[test]
    fn parse_matches_discovered_chord() {
        // A jcode binding and a terminal binding for the same physical keys must
        // compare equal so the conflict detector can pair them.
        let jcode = KeyChord::parse("cmd+k").unwrap();
        let terminal = KeyChord::new(true, false, false, false, "k");
        assert_eq!(jcode, terminal);
    }
}
