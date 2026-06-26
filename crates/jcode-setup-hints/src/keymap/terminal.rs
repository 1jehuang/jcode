//! Discover key bindings declared by terminal emulators.
//!
//! Different terminals store bindings in different ways. The most reliable
//! approach for Ghostty is to ask it for its *effective* binding set via
//! `ghostty +list-keybinds`, which merges built-in defaults with the user's
//! config. The parsing is pure and unit-tested; only [`read_ghostty_keybinds`]
//! shells out.

use super::chord::KeyChord;
use super::source::{DiscoveredBinding, KeySource};

/// Parse a single Ghostty keybind line of the form:
///
/// ```text
/// keybind = super+shift+,=reload_config
/// super+backspace=text:\x17
/// ```
///
/// The left side (up to the first top-level `=`) is the trigger; the right side
/// is the action. The trigger is `mod+mod+key`. Returns `None` for lines that
/// are not bindings (comments, blanks, multi-key sequences we do not model).
pub fn parse_ghostty_keybind_line(line: &str) -> Option<DiscoveredBinding> {
    let mut line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    // Strip an optional leading `keybind =` / `keybind:` prefix.
    if let Some(rest) = line.strip_prefix("keybind") {
        let rest = rest.trim_start();
        let rest = rest.strip_prefix('=').or_else(|| rest.strip_prefix(':'))?;
        line = rest.trim();
    }

    // Split trigger=action on the first '='.
    let eq = line.find('=')?;
    let trigger = line[..eq].trim();
    let action = line[eq + 1..].trim();
    if trigger.is_empty() {
        return None;
    }

    let chord = parse_trigger(trigger)?;
    Some(DiscoveredBinding {
        chord,
        source: KeySource::Terminal,
        action: action.to_string(),
        raw: line.to_string(),
    })
}

/// Parse a `mod+mod+key` trigger into a chord. Returns `None` for triggers that
/// describe a multi-key sequence (Ghostty uses `>` between chords) since we only
/// model single chords for conflict detection.
fn parse_trigger(trigger: &str) -> Option<KeyChord> {
    if trigger.contains('>') {
        return None;
    }
    // Ghostty exposes a few logical triggers (mapped to the platform's native
    // shortcut) that are not real key chords. They can never collide with a
    // jcode binding, so drop them to keep the snapshot clean.
    if matches!(
        trigger.to_ascii_lowercase().as_str(),
        "copy" | "paste" | "unbind" | "ignore"
    ) {
        return None;
    }
    let mut cmd = false;
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut key: Option<String> = None;

    // Split on '+', but a trailing '+' means the key itself is '+'.
    let tokens = split_trigger_tokens(trigger);
    for tok in tokens {
        match tok.to_ascii_lowercase().as_str() {
            "super" | "cmd" | "command" => cmd = true,
            "ctrl" | "control" => ctrl = true,
            "alt" | "opt" | "option" => alt = true,
            "shift" => shift = true,
            other => {
                // Last non-modifier token wins as the key.
                key = Some(other.to_string());
            }
        }
    }

    let key = key?;
    Some(KeyChord::new(cmd, ctrl, alt, shift, &key))
}

/// Split a trigger on '+' while treating a literal '+' key correctly. For
/// example `super++` is `["super", "+"]` and `ctrl+shift++` is
/// `["ctrl", "shift", "+"]`.
fn split_trigger_tokens(trigger: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = trigger.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '+' {
            if cur.is_empty() {
                // A '+' with nothing before it is the literal '+' key.
                // Only treat it as the key when it is not a separator between
                // two names (i.e. previous char was already a separator).
                let is_trailing_or_double = i + 1 == chars.len() || chars[i + 1] == '+';
                if is_trailing_or_double || tokens.is_empty() {
                    tokens.push("+".to_string());
                    continue;
                }
            }
            if !cur.is_empty() {
                tokens.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

/// Parse the full output of `ghostty +list-keybinds` (or a Ghostty config file)
/// into discovered bindings.
pub fn parse_ghostty_keybinds(output: &str) -> Vec<DiscoveredBinding> {
    output
        .lines()
        .filter_map(parse_ghostty_keybind_line)
        .collect()
}

/// Run `ghostty +list-keybinds` and parse its output. Returns an empty vec on
/// any failure. Tries the bundled macOS binary first, then `ghostty` on PATH.
#[cfg(target_os = "macos")]
pub fn read_ghostty_keybinds() -> Vec<DiscoveredBinding> {
    use std::process::Command;

    const CANDIDATES: [&str; 2] = [
        "/Applications/Ghostty.app/Contents/MacOS/ghostty",
        "ghostty",
    ];
    for bin in CANDIDATES {
        let Ok(output) = Command::new(bin).arg("+list-keybinds").output() else {
            continue;
        };
        if output.status.success() && !output.stdout.is_empty() {
            let text = String::from_utf8_lossy(&output.stdout);
            return parse_ghostty_keybinds(&text);
        }
    }
    Vec::new()
}

#[cfg(not(target_os = "macos"))]
pub fn read_ghostty_keybinds() -> Vec<DiscoveredBinding> {
    use std::process::Command;
    let Ok(output) = Command::new("ghostty").arg("+list-keybinds").output() else {
        return Vec::new();
    };
    if output.status.success() && !output.stdout.is_empty() {
        let text = String::from_utf8_lossy(&output.stdout);
        return parse_ghostty_keybinds(&text);
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listed_keybind() {
        let b = parse_ghostty_keybind_line("keybind = super+c=copy_to_clipboard:mixed").unwrap();
        assert_eq!(b.chord.canonical(), "cmd+c");
        assert_eq!(b.action, "copy_to_clipboard:mixed");
    }

    #[test]
    fn parses_bare_config_line() {
        let b = parse_ghostty_keybind_line("super+backspace=text:\\x17").unwrap();
        assert_eq!(b.chord.canonical(), "cmd+backspace");
        assert_eq!(b.action, "text:\\x17");
    }

    #[test]
    fn parses_named_punctuation_key() {
        let b = parse_ghostty_keybind_line("keybind = super+shift+,=reload_config").unwrap();
        assert_eq!(b.chord.canonical(), "cmd+shift+,");
    }

    #[test]
    fn parses_digit_key() {
        let b = parse_ghostty_keybind_line("keybind = super+digit_1=goto_tab:1").unwrap();
        assert_eq!(b.chord.canonical(), "cmd+1");
    }

    #[test]
    fn parses_literal_plus_key() {
        let b = parse_ghostty_keybind_line("keybind = super++=increase_font_size:1").unwrap();
        assert_eq!(b.chord.canonical(), "cmd++");
    }

    #[test]
    fn skips_comments_and_blanks() {
        assert!(parse_ghostty_keybind_line("# a comment").is_none());
        assert!(parse_ghostty_keybind_line("   ").is_none());
    }

    #[test]
    fn skips_multi_key_sequences() {
        assert!(parse_ghostty_keybind_line("keybind = ctrl+a>n=new_window").is_none());
    }

    #[test]
    fn skips_logical_copy_paste_triggers() {
        assert!(parse_ghostty_keybind_line("keybind = copy=copy_to_clipboard:mixed").is_none());
        assert!(parse_ghostty_keybind_line("keybind = paste=paste_from_clipboard").is_none());
    }

    #[test]
    fn parses_full_output() {
        let out = "\
keybind = super+c=copy_to_clipboard:mixed
keybind = super+v=paste_from_clipboard
# comment
keybind = super+enter=new_window
";
        let binds = parse_ghostty_keybinds(out);
        assert_eq!(binds.len(), 3);
    }
}
