//! Discover global key bindings declared by third-party apps that grab hotkeys
//! *before* the terminal (and therefore jcode) ever sees them.
//!
//! macOS lets window managers and automation tools register system-wide hotkeys.
//! When one of those overlaps a key jcode wants (the classic case is a tiling WM
//! binding `Cmd+J`/`Cmd+K` to window focus, which shadows jcode's prompt
//! navigation), the keystroke never reaches the terminal and the jcode binding
//! silently does nothing. The terminal/macOS scanners cannot see these, so we
//! read the relevant app config files directly.
//!
//! Each app has its own config grammar, so there is one pure parser per app
//! (`parse_*`) plus a thin reader (`read_*`) that locates and loads the file.
//! Parsers are unit-tested without touching the machine.

use std::path::{Path, PathBuf};

use super::chord::KeyChord;
use super::source::{DiscoveredBinding, KeySource};

/// A modifier mask accumulated while parsing a chord, kept separate from the key
/// token so apps that express the "hyper" key as a bundle of modifiers can be
/// expanded uniformly.
#[derive(Clone, Copy, Default)]
struct Mods {
    cmd: bool,
    ctrl: bool,
    alt: bool,
    shift: bool,
}

impl Mods {
    fn or(self, other: Mods) -> Mods {
        Mods {
            cmd: self.cmd || other.cmd,
            ctrl: self.ctrl || other.ctrl,
            alt: self.alt || other.alt,
            shift: self.shift || other.shift,
        }
    }
}

/// Apply a single modifier token to `mods`, expanding `hyper` via `hyper_mods`.
/// Returns `true` if the token was a recognized modifier, `false` if it should
/// be treated as the primary key.
fn apply_modifier(token: &str, mods: &mut Mods, hyper_mods: Mods) -> bool {
    match token.trim().to_ascii_lowercase().as_str() {
        "cmd" | "command" | "super" | "win" | "windows" => mods.cmd = true,
        "ctrl" | "control" => mods.ctrl = true,
        "alt" | "opt" | "option" => mods.alt = true,
        "shift" => mods.shift = true,
        // "hyper" is an app-defined alias for some bundle of real modifiers.
        "hyper" => *mods = mods.or(hyper_mods),
        // "fn" is not representable as a jcode modifier; ignore it so the rest of
        // the chord still parses.
        "fn" | "function" => {}
        _ => return false,
    }
    true
}

/// Parse a "Hyper" definition (e.g. OmniWM's `hyperTrigger = "Option"`, or a
/// compound like "Cmd+Ctrl+Alt+Shift") into the modifier bundle it stands for.
fn parse_hyper_mods(spec: &str) -> Mods {
    let mut mods = Mods::default();
    for token in spec.split(['+', '-']) {
        // Recurse-safe: "hyper" inside a hyper definition is meaningless, so pass
        // an empty bundle.
        apply_modifier(token, &mut mods, Mods::default());
    }
    mods
}

/// Build a chord from a list of tokens (modifiers + one key), where modifiers and
/// the key are already separated out. `hyper_mods` expands any `hyper` token.
/// Returns `None` if no primary key token was found.
fn chord_from_tokens<'a>(
    tokens: impl IntoIterator<Item = &'a str>,
    hyper_mods: Mods,
) -> Option<KeyChord> {
    let mut mods = Mods::default();
    let mut key: Option<String> = None;
    for token in tokens {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if !apply_modifier(token, &mut mods, hyper_mods) {
            // Last non-modifier token wins as the key.
            key = Some(token.to_string());
        }
    }
    let key = key?;
    Some(KeyChord::new(
        mods.cmd, mods.ctrl, mods.alt, mods.shift, &key,
    ))
}

// ---------------------------------------------------------------------------
// OmniWM (~/.config/omniwm/settings.toml)
// ---------------------------------------------------------------------------

/// Parse an OmniWM `settings.toml` into discovered bindings. OmniWM stores
/// hotkeys as an array of tables:
///
/// ```toml
/// hyperTrigger = "Option"
///
/// [[hotkeys]]
/// binding = "Command+J"
/// id = "focus.down"
/// ```
///
/// `binding = "Unassigned"` entries are skipped. The `hyperTrigger` value (or a
/// default of Option) expands any `Hyper+...` binding.
pub fn parse_omniwm(text: &str) -> Vec<DiscoveredBinding> {
    let Ok(value) = text.parse::<toml::Value>() else {
        return Vec::new();
    };

    // Hyper expands to the configured trigger; OmniWM defaults to Option.
    let hyper_mods = value
        .get("general")
        .and_then(|g| g.get("hyperTrigger"))
        .or_else(|| value.get("hyperTrigger"))
        .and_then(|h| h.as_str())
        .map(parse_hyper_mods)
        .unwrap_or(Mods {
            alt: true,
            ..Mods::default()
        });

    let Some(hotkeys) = value.get("hotkeys").and_then(|h| h.as_array()) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for hk in hotkeys {
        let Some(binding) = hk.get("binding").and_then(|b| b.as_str()) else {
            continue;
        };
        if binding.trim().eq_ignore_ascii_case("unassigned") || binding.trim().is_empty() {
            continue;
        }
        let action = hk
            .get("id")
            .and_then(|i| i.as_str())
            .unwrap_or("")
            .to_string();
        if let Some(chord) = chord_from_tokens(binding.split(['+', '-']), hyper_mods) {
            out.push(DiscoveredBinding {
                chord,
                source: KeySource::ExternalApp,
                action,
                raw: binding.to_string(),
                tool: "OmniWM".to_string(),
            });
        }
    }
    out
}

/// Read and parse OmniWM's config, if present.
pub fn read_omniwm() -> Vec<DiscoveredBinding> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let path = home.join(".config/omniwm/settings.toml");
    read_to_string(&path)
        .map(|t| parse_omniwm(&t))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// AeroSpace (~/.aerospace.toml or ~/.config/aerospace/aerospace.toml)
// ---------------------------------------------------------------------------

/// Parse an AeroSpace config into discovered bindings. AeroSpace declares
/// bindings under `[mode.<name>.binding]` tables where the key is a chord like
/// `alt-h` and the value is the command:
///
/// ```toml
/// [mode.main.binding]
/// alt-h = 'focus left'
/// cmd-shift-l = ['move right', 'mode main']
/// ```
pub fn parse_aerospace(text: &str) -> Vec<DiscoveredBinding> {
    let Ok(value) = text.parse::<toml::Value>() else {
        return Vec::new();
    };
    let Some(modes) = value.get("mode").and_then(|m| m.as_table()) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for mode in modes.values() {
        let Some(bindings) = mode.get("binding").and_then(|b| b.as_table()) else {
            continue;
        };
        for (chord_str, action_val) in bindings {
            // AeroSpace separates modifiers and key with '-'.
            let Some(chord) = chord_from_tokens(chord_str.split('-'), Mods::default()) else {
                continue;
            };
            let action = aerospace_action_label(action_val);
            out.push(DiscoveredBinding {
                chord,
                source: KeySource::ExternalApp,
                action,
                raw: chord_str.clone(),
                tool: "AeroSpace".to_string(),
            });
        }
    }
    out
}

fn aerospace_action_label(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => s.clone(),
        toml::Value::Array(items) => items
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join("; "),
        other => other.to_string(),
    }
}

/// Read and parse AeroSpace's config from either supported location.
pub fn read_aerospace() -> Vec<DiscoveredBinding> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    for rel in [".aerospace.toml", ".config/aerospace/aerospace.toml"] {
        let path = home.join(rel);
        if let Some(text) = read_to_string(&path) {
            return parse_aerospace(&text);
        }
    }
    Vec::new()
}

// ---------------------------------------------------------------------------
// skhd (~/.config/skhd/skhdrc or ~/.skhdrc)
// ---------------------------------------------------------------------------

/// Parse an skhd config into discovered bindings. skhd lines look like:
///
/// ```text
/// cmd - h : yabai -m window --focus west
/// cmd + shift - 0x2C : echo hi
/// # comment
/// :: mode @ : ...        # mode declaration, ignored
/// ```
///
/// The activation (left of the first `:`) is `mods - key`, where modifiers are
/// joined with `+` and separated from the key by `-`.
pub fn parse_skhd(text: &str) -> Vec<DiscoveredBinding> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("::") {
            continue;
        }
        // Activation is everything before the first ':' that starts the command.
        let Some(colon) = line.find(':') else {
            continue;
        };
        let activation = line[..colon].trim();
        let action = line[colon + 1..].trim();
        if activation.is_empty() {
            continue;
        }
        // Strip an optional leading mode list ("mode_name <") if present.
        let activation = activation.rsplit('<').next().unwrap_or(activation).trim();

        // Split modifiers from key on the first '-'. Modifiers use '+'.
        let (mods_part, key_part) = match activation.split_once('-') {
            Some((m, k)) => (m, k),
            None => ("", activation),
        };
        let tokens = mods_part
            .split('+')
            .chain(std::iter::once(key_part))
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let Some(chord) = chord_from_tokens(tokens, Mods::default()) {
            out.push(DiscoveredBinding {
                chord,
                source: KeySource::ExternalApp,
                action: action.to_string(),
                raw: activation.to_string(),
                tool: "skhd".to_string(),
            });
        }
    }
    out
}

/// Read and parse skhd's config from either supported location.
pub fn read_skhd() -> Vec<DiscoveredBinding> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    for rel in [".config/skhd/skhdrc", ".skhdrc"] {
        let path = home.join(rel);
        if let Some(text) = read_to_string(&path) {
            return parse_skhd(&text);
        }
    }
    Vec::new()
}

// ---------------------------------------------------------------------------
// Aggregation
// ---------------------------------------------------------------------------

/// Read every supported external app's bindings on this machine.
pub fn read_external_bindings() -> Vec<DiscoveredBinding> {
    let mut out = Vec::new();
    out.extend(read_omniwm());
    out.extend(read_aerospace());
    out.extend(read_skhd());
    out
}

fn read_to_string(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Exposed for tests/diagnostics: the config paths we look for, relative to the
/// home directory.
pub fn external_config_paths() -> Vec<PathBuf> {
    [
        ".config/omniwm/settings.toml",
        ".aerospace.toml",
        ".config/aerospace/aerospace.toml",
        ".config/skhd/skhdrc",
        ".skhdrc",
    ]
    .iter()
    .map(PathBuf::from)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omniwm_cmd_jk_focus_bindings() {
        let cfg = r#"
[general]
hyperTrigger = "Option"

[[hotkeys]]
binding = "Command+J"
id = "focus.down"

[[hotkeys]]
binding = "Command+K"
id = "focus.up"

[[hotkeys]]
binding = "Unassigned"
id = "focusPrevious"

[[hotkeys]]
binding = "Hyper+1"
id = "switchWorkspace.0"
"#;
        let binds = parse_omniwm(cfg);
        // Cmd+J, Cmd+K, and Hyper(=Option=alt)+1; Unassigned is skipped.
        assert_eq!(binds.len(), 3);
        let jk: Vec<_> = binds
            .iter()
            .filter(|b| b.chord.canonical() == "cmd+j" || b.chord.canonical() == "cmd+k")
            .collect();
        assert_eq!(jk.len(), 2);
        for b in &jk {
            assert_eq!(b.source, KeySource::ExternalApp);
            assert_eq!(b.tool, "OmniWM");
        }
        // Hyper+1 expands to alt+1 (Option trigger).
        assert!(binds.iter().any(|b| b.chord.canonical() == "alt+1"));
    }

    #[test]
    fn omniwm_hyper_defaults_to_option_when_unset() {
        let cfg = r#"
[[hotkeys]]
binding = "Hyper+2"
id = "switchWorkspace.1"
"#;
        let binds = parse_omniwm(cfg);
        assert_eq!(binds.len(), 1);
        assert_eq!(binds[0].chord.canonical(), "alt+2");
    }

    #[test]
    fn omniwm_cmd_shift_move_bindings() {
        let cfg = r#"
[[hotkeys]]
binding = "Command+Shift+K"
id = "move.up"
"#;
        let binds = parse_omniwm(cfg);
        assert_eq!(binds.len(), 1);
        assert_eq!(binds[0].chord.canonical(), "cmd+shift+k");
        assert_eq!(binds[0].action, "move.up");
    }

    #[test]
    fn aerospace_binding_section() {
        let cfg = r#"
[mode.main.binding]
alt-h = 'focus left'
cmd-shift-l = ['move right', 'mode main']
"#;
        let binds = parse_aerospace(cfg);
        assert_eq!(binds.len(), 2);
        let h = binds
            .iter()
            .find(|b| b.chord.canonical() == "alt+h")
            .unwrap();
        assert_eq!(h.action, "focus left");
        assert_eq!(h.tool, "AeroSpace");
        let l = binds
            .iter()
            .find(|b| b.chord.canonical() == "cmd+shift+l")
            .unwrap();
        assert_eq!(l.action, "move right; mode main");
    }

    #[test]
    fn skhd_basic_lines() {
        let cfg = "\
# comment\n\
cmd - h : yabai -m window --focus west\n\
cmd + shift - j : yabai -m window --swap south\n\
:: default : echo mode\n\
";
        let binds = parse_skhd(cfg);
        assert_eq!(binds.len(), 2);
        assert_eq!(binds[0].chord.canonical(), "cmd+h");
        assert_eq!(binds[0].tool, "skhd");
        assert_eq!(binds[1].chord.canonical(), "cmd+shift+j");
    }

    #[test]
    fn skhd_keyless_modifier_only_is_skipped() {
        // A bare modifier with no key cannot form a chord.
        let binds = parse_skhd("cmd - : noop\n");
        assert!(binds.is_empty());
    }

    #[test]
    fn malformed_toml_yields_nothing() {
        assert!(parse_omniwm("this is = = not toml").is_empty());
        assert!(parse_aerospace("[[[bad").is_empty());
    }
}
