//! Decode macOS system keyboard shortcuts from `com.apple.symbolichotkeys`.
//!
//! macOS stores global shortcuts (Spotlight, Mission Control, screenshots,
//! input-source switching, etc.) under the `AppleSymbolicHotKeys` key of the
//! `com.apple.symbolichotkeys` preference domain. Each entry is:
//!
//! ```text
//! <id> = { enabled = 0/1; value = { parameters = (ascii, keycode, modmask); ... }; }
//! ```
//!
//! We read it with `defaults export ... | plutil -convert json` and decode the
//! `[ascii, keycode, modmask]` triple into a [`KeyChord`]. The pure decoding
//! logic lives here (and is unit-tested); the subprocess plumbing is isolated in
//! [`read_symbolic_hotkeys`].

use super::chord::KeyChord;
use super::source::{DiscoveredBinding, KeySource};

/// NSEvent modifier flag bits used in the symbolic-hotkeys `modmask`.
const NS_SHIFT: u64 = 0x0002_0000;
const NS_CONTROL: u64 = 0x0004_0000;
const NS_OPTION: u64 = 0x0008_0000;
const NS_COMMAND: u64 = 0x0010_0000;

/// Human-readable names for well-known symbolic-hotkey IDs. Only used to make
/// the snapshot and warnings legible; unknown IDs still get decoded.
fn action_name(id: i64) -> String {
    let name = match id {
        32 => "Mission Control",
        33 => "Mission Control: Application windows",
        36 => "Show Launchpad",
        60 => "Select previous input source",
        61 => "Select next input source",
        64 => "Spotlight: Show search",
        65 => "Spotlight: Show Finder search window",
        79 => "Move left a space",
        81 => "Move right a space",
        // Screenshots
        28 => "Screenshot: Save picture of screen",
        29 => "Screenshot: Copy picture of screen",
        30 => "Screenshot: Save picture of selected area",
        31 => "Screenshot: Copy picture of selected area",
        184 => "Screenshot and recording options",
        // Misc
        162 => "Show Notification Center",
        163 => "Toggle Do Not Disturb",
        175 => "Dictation",
        _ => "",
    };
    if name.is_empty() {
        format!("macOS hotkey #{id}")
    } else {
        name.to_string()
    }
}

/// Map a macOS virtual keycode to a normalized key token. Covers the common
/// keys that appear in default system shortcuts. Returns `None` for keys we do
/// not have a stable mapping for (we then fall back to the ASCII parameter).
fn keycode_to_token(keycode: i64) -> Option<&'static str> {
    Some(match keycode {
        0 => "a",
        1 => "s",
        2 => "d",
        3 => "f",
        4 => "h",
        5 => "g",
        6 => "z",
        7 => "x",
        8 => "c",
        9 => "v",
        11 => "b",
        12 => "q",
        13 => "w",
        14 => "e",
        15 => "r",
        16 => "y",
        17 => "t",
        18 => "1",
        19 => "2",
        20 => "3",
        21 => "4",
        22 => "6",
        23 => "5",
        24 => "=",
        25 => "9",
        26 => "7",
        27 => "-",
        28 => "8",
        29 => "0",
        30 => "]",
        31 => "o",
        32 => "u",
        33 => "[",
        34 => "i",
        35 => "p",
        37 => "l",
        38 => "j",
        39 => "'",
        40 => "k",
        41 => ";",
        42 => "\\",
        43 => ",",
        44 => "/",
        45 => "n",
        46 => "m",
        47 => ".",
        50 => "`",
        36 => "enter",
        48 => "tab",
        49 => "space",
        51 => "backspace",
        53 => "esc",
        // Function keys
        122 => "f1",
        120 => "f2",
        99 => "f3",
        118 => "f4",
        96 => "f5",
        97 => "f6",
        98 => "f7",
        100 => "f8",
        101 => "f9",
        109 => "f10",
        103 => "f11",
        111 => "f12",
        // Arrows
        123 => "left",
        124 => "right",
        125 => "down",
        126 => "up",
        _ => return None,
    })
}

/// Decode a single `[ascii, keycode, modmask]` parameter triple into a chord.
///
/// `ascii == 65535` (and `keycode == 65535`) means "no key assigned", in which
/// case we return `None`. Prefer the virtual keycode for the key token; fall
/// back to the ASCII value when the keycode is unknown.
pub fn decode_parameters(ascii: i64, keycode: i64, modmask: i64) -> Option<KeyChord> {
    if keycode == 65535 && ascii == 65535 {
        return None;
    }
    let mask = modmask as u64;
    let cmd = mask & NS_COMMAND != 0;
    let ctrl = mask & NS_CONTROL != 0;
    let alt = mask & NS_OPTION != 0;
    let shift = mask & NS_SHIFT != 0;

    let key = if let Some(tok) = keycode_to_token(keycode) {
        tok.to_string()
    } else if ascii > 0 && ascii < 0x10_FFFF && ascii != 65535 {
        // Fall back to the literal character from the ascii parameter.
        char::from_u32(ascii as u32)
            .filter(|c| !c.is_control())
            .map(|c| c.to_ascii_lowercase().to_string())?
    } else {
        return None;
    };

    Some(KeyChord::new(cmd, ctrl, alt, shift, &key))
}

/// One raw symbolic-hotkey entry, as decoded from JSON.
#[derive(Debug, Clone)]
pub struct RawHotkey {
    pub id: i64,
    pub enabled: bool,
    pub parameters: Option<(i64, i64, i64)>,
}

/// Parse the JSON produced by `plutil -convert json` of the symbolic-hotkeys
/// domain into raw hotkey entries. Tolerates the parameters being encoded as
/// either JSON numbers or numeric strings (macOS does both).
pub fn parse_symbolic_hotkeys_json(json: &str) -> Vec<RawHotkey> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(map) = value.get("AppleSymbolicHotKeys").and_then(|v| v.as_object()) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for (id_str, entry) in map {
        let Ok(id) = id_str.parse::<i64>() else {
            continue;
        };
        let enabled = match entry.get("enabled") {
            Some(serde_json::Value::Bool(b)) => *b,
            Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
            _ => false,
        };
        let parameters = entry
            .get("value")
            .and_then(|v| v.get("parameters"))
            .and_then(|p| p.as_array())
            .and_then(|arr| {
                let nums: Vec<i64> = arr.iter().filter_map(json_as_i64).collect();
                if nums.len() >= 3 {
                    Some((nums[0], nums[1], nums[2]))
                } else {
                    None
                }
            });
        out.push(RawHotkey {
            id,
            enabled,
            parameters,
        });
    }
    out
}

fn json_as_i64(v: &serde_json::Value) -> Option<i64> {
    match v {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// Turn raw hotkey entries into discovered bindings, skipping ones that are
/// disabled or have no assignable key.
pub fn hotkeys_to_bindings(raw: &[RawHotkey]) -> Vec<DiscoveredBinding> {
    let mut out = Vec::new();
    for hk in raw {
        if !hk.enabled {
            continue;
        }
        let Some((ascii, keycode, modmask)) = hk.parameters else {
            continue;
        };
        let Some(chord) = decode_parameters(ascii, keycode, modmask) else {
            continue;
        };
        out.push(DiscoveredBinding {
            chord,
            source: KeySource::MacosSystem,
            action: action_name(hk.id),
            raw: format!("symbolichotkey #{}", hk.id),
        });
    }
    out
}

/// Read and decode the live macOS symbolic hotkeys via `defaults` + `plutil`.
/// Returns an empty vec on any failure (missing tools, non-macOS, parse error).
#[cfg(target_os = "macos")]
pub fn read_symbolic_hotkeys() -> Vec<DiscoveredBinding> {
    use std::process::Command;

    let export = Command::new("/usr/bin/defaults")
        .args(["export", "com.apple.symbolichotkeys", "-"])
        .output();
    let Ok(export) = export else {
        return Vec::new();
    };
    if !export.status.success() || export.stdout.is_empty() {
        return Vec::new();
    }

    // Pipe the exported plist through plutil to get JSON.
    let mut child = match Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(&export.stdout);
        // stdin dropped here, closing the pipe.
    }

    let Ok(output) = child.wait_with_output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let json = String::from_utf8_lossy(&output.stdout);
    let raw = parse_symbolic_hotkeys_json(&json);
    hotkeys_to_bindings(&raw)
}

#[cfg(not(target_os = "macos"))]
pub fn read_symbolic_hotkeys() -> Vec<DiscoveredBinding> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_spotlight_cmd_space() {
        // [32 (space ascii), 49 (space keycode), 0x100000 (cmd)]
        let chord = decode_parameters(32, 49, 1_048_576).unwrap();
        assert_eq!(chord.canonical(), "cmd+space");
    }

    #[test]
    fn decodes_input_source_ctrl_space() {
        // Select previous input source: [32, 49, 0x40000 (ctrl)]
        let chord = decode_parameters(32, 49, 262_144).unwrap();
        assert_eq!(chord.canonical(), "ctrl+space");
    }

    #[test]
    fn unassigned_returns_none() {
        assert!(decode_parameters(65535, 65535, 0).is_none());
    }

    #[test]
    fn combined_modifiers_decode() {
        // ctrl+option+cmd = 0x40000 | 0x80000 | 0x100000 = 0x1C0000
        let chord = decode_parameters(0, 40, 0x1C_0000).unwrap();
        assert!(chord.cmd && chord.ctrl && chord.alt && !chord.shift);
        assert_eq!(chord.key, "k");
    }

    #[test]
    fn parses_json_with_string_and_numeric_params() {
        let json = r#"{
            "AppleSymbolicHotKeys": {
                "64": {"enabled": 1, "value": {"parameters": ["32", "49", "1048576"]}},
                "60": {"enabled": false, "value": {"parameters": [32, 49, 262144]}},
                "999": {"enabled": 1}
            }
        }"#;
        let raw = parse_symbolic_hotkeys_json(json);
        assert_eq!(raw.len(), 3);
        let bindings = hotkeys_to_bindings(&raw);
        // Only #64 is enabled with params.
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].chord.canonical(), "cmd+space");
        assert!(bindings[0].action.contains("Spotlight"));
    }
}
