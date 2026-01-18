use crate::config::config;
use crossterm::event::{KeyCode, KeyModifiers};

#[derive(Clone, Debug)]
pub struct KeyBinding {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyBinding {
    pub fn matches(&self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        let (code, modifiers) = normalize_key(code, modifiers);
        let (bind_code, bind_mods) = normalize_key(self.code.clone(), self.modifiers);
        code == bind_code && modifiers == bind_mods
    }
}

#[derive(Clone, Debug)]
pub struct ModelSwitchKeys {
    pub next: KeyBinding,
    pub prev: Option<KeyBinding>,
    pub next_label: String,
    pub prev_label: Option<String>,
}

impl ModelSwitchKeys {
    pub fn direction_for(&self, code: KeyCode, modifiers: KeyModifiers) -> Option<i8> {
        if self.next.matches(code.clone(), modifiers) {
            return Some(1);
        }
        if let Some(prev) = &self.prev {
            if prev.matches(code, modifiers) {
                return Some(-1);
            }
        }
        None
    }
}

pub fn load_model_switch_keys() -> ModelSwitchKeys {
    let cfg = config();

    let default_next = KeyBinding {
        code: KeyCode::Tab,
        modifiers: KeyModifiers::CONTROL,
    };
    let default_prev = KeyBinding {
        code: KeyCode::Tab,
        modifiers: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    };

    let (next, next_label) =
        parse_or_default(&cfg.keybindings.model_switch_next, default_next, "Ctrl+Tab");
    let (prev, prev_label) = parse_optional(
        &cfg.keybindings.model_switch_prev,
        default_prev,
        "Ctrl+Shift+Tab",
    );

    ModelSwitchKeys {
        next,
        prev,
        next_label,
        prev_label,
    }
}

fn parse_or_default(raw: &str, fallback: KeyBinding, fallback_label: &str) -> (KeyBinding, String) {
    match parse_keybinding(raw) {
        Some(binding) => (binding.clone(), format_binding(&binding)),
        None => (fallback.clone(), fallback_label.to_string()),
    }
}

fn parse_optional(
    raw: &str,
    fallback: KeyBinding,
    fallback_label: &str,
) -> (Option<KeyBinding>, Option<String>) {
    let raw = raw.trim();
    if raw.is_empty() || is_disabled(raw) {
        return (None, None);
    }
    match parse_keybinding(raw) {
        Some(binding) => (Some(binding.clone()), Some(format_binding(&binding))),
        None => (Some(fallback.clone()), Some(fallback_label.to_string())),
    }
}

fn is_disabled(raw: &str) -> bool {
    matches!(
        raw.to_ascii_lowercase().as_str(),
        "none" | "off" | "disabled"
    )
}

fn parse_keybinding(raw: &str) -> Option<KeyBinding> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if is_disabled(raw) {
        return None;
    }
    let lower = raw.to_ascii_lowercase();
    let parts: Vec<&str> = lower
        .split('+')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        return None;
    }

    let mut modifiers = KeyModifiers::empty();
    let mut key_part: Option<&str> = None;

    for part in parts {
        match part {
            "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
            "alt" | "option" | "meta" => modifiers |= KeyModifiers::ALT,
            "shift" => modifiers |= KeyModifiers::SHIFT,
            _ => {
                key_part = Some(part);
            }
        }
    }

    let key = key_part?;
    let code = match key {
        "tab" => KeyCode::Tab,
        "backtab" | "shift-tab" => {
            modifiers |= KeyModifiers::SHIFT;
            KeyCode::Tab
        }
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "space" => KeyCode::Char(' '),
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "insert" => KeyCode::Insert,
        "delete" => KeyCode::Delete,
        "backspace" => KeyCode::Backspace,
        _ => {
            if key.len() == 1 {
                KeyCode::Char(key.chars().next().unwrap())
            } else {
                return None;
            }
        }
    };

    Some(KeyBinding { code, modifiers })
}

fn normalize_key(code: KeyCode, modifiers: KeyModifiers) -> (KeyCode, KeyModifiers) {
    if code == KeyCode::BackTab {
        (KeyCode::Tab, modifiers | KeyModifiers::SHIFT)
    } else {
        (code, modifiers)
    }
}

/// Configurable scroll keybindings
#[derive(Clone, Debug)]
pub struct ScrollKeys {
    pub up: KeyBinding,
    pub down: KeyBinding,
    pub page_up: KeyBinding,
    pub page_down: KeyBinding,
    pub up_label: String,
    pub down_label: String,
    pub page_up_label: String,
    pub page_down_label: String,
}

impl ScrollKeys {
    /// Check if a key matches scroll up (returns scroll amount, negative = up)
    pub fn scroll_amount(&self, code: KeyCode, modifiers: KeyModifiers) -> Option<i32> {
        if self.up.matches(code.clone(), modifiers) {
            return Some(-3); // Scroll up 3 lines
        }
        if self.down.matches(code.clone(), modifiers) {
            return Some(3); // Scroll down 3 lines
        }
        if self.page_up.matches(code.clone(), modifiers) {
            return Some(-10); // Page up
        }
        if self.page_down.matches(code, modifiers) {
            return Some(10); // Page down
        }
        None
    }
}

pub fn load_scroll_keys() -> ScrollKeys {
    let cfg = config();

    // Default to Alt+K/J for scroll - more terminal compatible than Ctrl+Shift
    let default_up = KeyBinding {
        code: KeyCode::Char('k'),
        modifiers: KeyModifiers::ALT,
    };
    let default_down = KeyBinding {
        code: KeyCode::Char('j'),
        modifiers: KeyModifiers::ALT,
    };
    let default_page_up = KeyBinding {
        code: KeyCode::Char('u'),
        modifiers: KeyModifiers::ALT,
    };
    let default_page_down = KeyBinding {
        code: KeyCode::Char('d'),
        modifiers: KeyModifiers::ALT,
    };

    let (up, up_label) = parse_or_default(&cfg.keybindings.scroll_up, default_up, "Alt+K");
    let (down, down_label) = parse_or_default(&cfg.keybindings.scroll_down, default_down, "Alt+J");
    let (page_up, page_up_label) =
        parse_or_default(&cfg.keybindings.scroll_page_up, default_page_up, "Alt+U");
    let (page_down, page_down_label) =
        parse_or_default(&cfg.keybindings.scroll_page_down, default_page_down, "Alt+D");

    ScrollKeys {
        up,
        down,
        page_up,
        page_down,
        up_label,
        down_label,
        page_up_label,
        page_down_label,
    }
}

fn format_binding(binding: &KeyBinding) -> String {
    let mut parts: Vec<String> = Vec::new();
    if binding.modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("Ctrl".to_string());
    }
    if binding.modifiers.contains(KeyModifiers::ALT) {
        parts.push("Alt".to_string());
    }
    if binding.modifiers.contains(KeyModifiers::SHIFT) {
        parts.push("Shift".to_string());
    }

    let key = match binding.code {
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::PageUp => "PageUp".to_string(),
        KeyCode::PageDown => "PageDown".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::Insert => "Insert".to_string(),
        KeyCode::Delete => "Delete".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => c.to_ascii_uppercase().to_string(),
        _ => "Key".to_string(),
    };

    parts.push(key);
    parts.join("+")
}
