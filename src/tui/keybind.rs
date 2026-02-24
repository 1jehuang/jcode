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
            "cmd" | "command" | "super" | "win" | "windows" => {
                modifiers |= KeyModifiers::SUPER
            }
            "hyper" => modifiers |= KeyModifiers::HYPER,
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
    pub up_fallback: Option<KeyBinding>,
    pub down_fallback: Option<KeyBinding>,
    pub page_up: KeyBinding,
    pub page_down: KeyBinding,
    pub prompt_up: KeyBinding,
    pub prompt_down: KeyBinding,
    pub bookmark: KeyBinding,
    pub up_label: String,
    pub down_label: String,
    pub up_fallback_label: Option<String>,
    pub down_fallback_label: Option<String>,
    pub page_up_label: String,
    pub page_down_label: String,
    pub prompt_up_label: String,
    pub prompt_down_label: String,
    pub bookmark_label: String,
}

impl ScrollKeys {
    fn matches_scroll_up(&self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        self.up.matches(code.clone(), modifiers)
            || self
                .up_fallback
                .as_ref()
                .map(|k| k.matches(code, modifiers))
                .unwrap_or(false)
    }

    fn matches_scroll_down(&self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        self.down.matches(code.clone(), modifiers)
            || self
                .down_fallback
                .as_ref()
                .map(|k| k.matches(code, modifiers))
                .unwrap_or(false)
    }

    /// Check if a key matches scroll up (returns scroll amount, negative = up)
    pub fn scroll_amount(&self, code: KeyCode, modifiers: KeyModifiers) -> Option<i32> {
        if self.matches_scroll_up(code.clone(), modifiers) {
            return Some(-3); // Scroll up 3 lines
        }
        if self.matches_scroll_down(code.clone(), modifiers) {
            return Some(3); // Scroll down 3 lines
        }
        if self.page_up.matches(code.clone(), modifiers) {
            return Some(-10); // Page up
        }
        if self.page_down.matches(code, modifiers) {
            return Some(10); // Page down
        }
        if modifiers.contains(KeyModifiers::CONTROL) {
            match code {
                KeyCode::Char('k') => return Some(-3),
                KeyCode::Char('j') => return Some(3),
                _ => {}
            }
        }

        // macOS compatibility fallback: keep historical Cmd+J/K behavior if not explicitly
        // configured, to preserve usability in terminals forwarding SUPER/META.
        let mac_command = cfg!(target_os = "macos")
            && self.up_fallback.is_none()
            && self.down_fallback.is_none()
            && (modifiers.contains(KeyModifiers::SUPER) || modifiers.contains(KeyModifiers::META));
        if mac_command {
            match code {
                KeyCode::Char('k') | KeyCode::Char('K') => return Some(-3),
                KeyCode::Char('j') | KeyCode::Char('J') => return Some(3),
                _ => {}
            }
        }
        None
    }

    /// Check if a key matches prompt jump (returns direction: -1 = prev, 1 = next)
    pub fn prompt_jump(&self, code: KeyCode, modifiers: KeyModifiers) -> Option<i8> {
        if self.prompt_up.matches(code.clone(), modifiers) {
            return Some(-1);
        }
        if self.prompt_down.matches(code, modifiers) {
            return Some(1);
        }

        // Fallback prompt-jump bindings:
        // - Ctrl+[ / Ctrl+] in terminals with keyboard enhancement
        // - Ctrl+5 in legacy terminals where Ctrl+] is reported as Ctrl+5
        //   (Ctrl+[ is indistinguishable from Esc without keyboard enhancement)
        if modifiers.contains(KeyModifiers::CONTROL) {
            match code {
                KeyCode::Char('[') => return Some(-1),
                KeyCode::Char(']') | KeyCode::Char('5') => return Some(1),
                _ => {}
            }
        }
        None
    }

    /// Check if a key matches the scroll bookmark toggle
    pub fn is_bookmark(&self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        self.bookmark.matches(code, modifiers)
    }
}

pub fn load_scroll_keys() -> ScrollKeys {
    let cfg = config();

    // Default to Ctrl+K/J for scroll (vim-style), Alt+U/D for page scroll
    let default_up = KeyBinding {
        code: KeyCode::Char('k'),
        modifiers: KeyModifiers::CONTROL,
    };
    let default_down = KeyBinding {
        code: KeyCode::Char('j'),
        modifiers: KeyModifiers::CONTROL,
    };
    let default_page_up = KeyBinding {
        code: KeyCode::Char('u'),
        modifiers: KeyModifiers::ALT,
    };
    let default_page_down = KeyBinding {
        code: KeyCode::Char('d'),
        modifiers: KeyModifiers::ALT,
    };
    let default_prompt_up = KeyBinding {
        code: KeyCode::Char('['),
        modifiers: KeyModifiers::CONTROL,
    };
    let default_prompt_down = KeyBinding {
        code: KeyCode::Char(']'),
        modifiers: KeyModifiers::CONTROL,
    };
    let default_bookmark = KeyBinding {
        code: KeyCode::Char('g'),
        modifiers: KeyModifiers::CONTROL,
    };

    let (up, up_label) = parse_or_default(&cfg.keybindings.scroll_up, default_up, "Ctrl+K");
    let (down, down_label) = parse_or_default(&cfg.keybindings.scroll_down, default_down, "Ctrl+J");
    let default_up_fallback = KeyBinding {
        code: KeyCode::Char('k'),
        modifiers: KeyModifiers::SUPER,
    };
    let default_down_fallback = KeyBinding {
        code: KeyCode::Char('j'),
        modifiers: KeyModifiers::SUPER,
    };
    let (up_fallback, up_fallback_label) = parse_optional(
        &cfg.keybindings.scroll_up_fallback,
        default_up_fallback,
        "Cmd+K",
    );
    let (down_fallback, down_fallback_label) = parse_optional(
        &cfg.keybindings.scroll_down_fallback,
        default_down_fallback,
        "Cmd+J",
    );
    let (page_up, page_up_label) =
        parse_or_default(&cfg.keybindings.scroll_page_up, default_page_up, "Alt+U");
    let (page_down, page_down_label) = parse_or_default(
        &cfg.keybindings.scroll_page_down,
        default_page_down,
        "Alt+D",
    );
    let (prompt_up, prompt_up_label) = parse_or_default(
        &cfg.keybindings.scroll_prompt_up,
        default_prompt_up,
        "Ctrl+[",
    );
    let (prompt_down, prompt_down_label) = parse_or_default(
        &cfg.keybindings.scroll_prompt_down,
        default_prompt_down,
        "Ctrl+]",
    );
    let (bookmark, bookmark_label) =
        parse_or_default(&cfg.keybindings.scroll_bookmark, default_bookmark, "Ctrl+G");

    ScrollKeys {
        up,
        down,
        up_fallback,
        down_fallback,
        page_up,
        page_down,
        prompt_up,
        prompt_down,
        bookmark,
        up_label,
        down_label,
        up_fallback_label,
        down_fallback_label,
        page_up_label,
        page_down_label,
        prompt_up_label,
        prompt_down_label,
        bookmark_label,
    }
}

#[derive(Clone, Debug)]
pub struct EffortSwitchKeys {
    pub increase: KeyBinding,
    pub decrease: KeyBinding,
    pub increase_label: String,
    pub decrease_label: String,
}

impl EffortSwitchKeys {
    pub fn direction_for(&self, code: KeyCode, modifiers: KeyModifiers) -> Option<i8> {
        if self.increase.matches(code.clone(), modifiers) {
            return Some(1);
        }
        if self.decrease.matches(code, modifiers) {
            return Some(-1);
        }
        None
    }
}

pub fn load_effort_switch_keys() -> EffortSwitchKeys {
    let cfg = config();

    let default_increase = KeyBinding {
        code: KeyCode::Right,
        modifiers: KeyModifiers::ALT,
    };
    let default_decrease = KeyBinding {
        code: KeyCode::Left,
        modifiers: KeyModifiers::ALT,
    };

    let (increase, increase_label) = parse_or_default(
        &cfg.keybindings.effort_increase,
        default_increase,
        "Alt+Right",
    );
    let (decrease, decrease_label) = parse_or_default(
        &cfg.keybindings.effort_decrease,
        default_decrease,
        "Alt+Left",
    );

    EffortSwitchKeys {
        increase,
        decrease,
        increase_label,
        decrease_label,
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
    if binding.modifiers.contains(KeyModifiers::SUPER) {
        let label = if cfg!(target_os = "macos") {
            "Cmd"
        } else if cfg!(windows) {
            "Win"
        } else {
            "Super"
        };
        parts.push(label.to_string());
    }
    if binding.modifiers.contains(KeyModifiers::META) {
        parts.push("Meta".to_string());
    }
    if binding.modifiers.contains(KeyModifiers::HYPER) {
        parts.push("Hyper".to_string());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_scroll_keys() -> ScrollKeys {
        ScrollKeys {
            up: KeyBinding {
                code: KeyCode::Char('k'),
                modifiers: KeyModifiers::ALT,
            },
            down: KeyBinding {
                code: KeyCode::Char('j'),
                modifiers: KeyModifiers::ALT,
            },
            up_fallback: Some(KeyBinding {
                code: KeyCode::Char('K'),
                modifiers: KeyModifiers::SHIFT,
            }),
            down_fallback: Some(KeyBinding {
                code: KeyCode::Char('J'),
                modifiers: KeyModifiers::SHIFT,
            }),
            page_up: KeyBinding {
                code: KeyCode::Char('u'),
                modifiers: KeyModifiers::ALT,
            },
            page_down: KeyBinding {
                code: KeyCode::Char('d'),
                modifiers: KeyModifiers::ALT,
            },
            prompt_up: KeyBinding {
                code: KeyCode::Char('['),
                modifiers: KeyModifiers::ALT,
            },
            prompt_down: KeyBinding {
                code: KeyCode::Char(']'),
                modifiers: KeyModifiers::ALT,
            },
            bookmark: KeyBinding {
                code: KeyCode::Char('g'),
                modifiers: KeyModifiers::CONTROL,
            },
            up_label: "Alt+K".to_string(),
            down_label: "Alt+J".to_string(),
            up_fallback_label: Some("Shift+K".to_string()),
            down_fallback_label: Some("Shift+J".to_string()),
            page_up_label: "Alt+U".to_string(),
            page_down_label: "Alt+D".to_string(),
            prompt_up_label: "Alt+[".to_string(),
            prompt_down_label: "Alt+]".to_string(),
            bookmark_label: "Ctrl+G".to_string(),
        }
    }

    #[test]
    fn test_scroll_amount_ctrl_fallback() {
        let keys = test_scroll_keys();

        assert_eq!(
            keys.scroll_amount(KeyCode::Char('k'), KeyModifiers::CONTROL),
            Some(-3)
        );
        assert_eq!(
            keys.scroll_amount(KeyCode::Char('j'), KeyModifiers::CONTROL),
            Some(3)
        );
    }

    #[test]
    fn test_scroll_amount_configured_fallback_keys() {
        let keys = test_scroll_keys();

        assert_eq!(
            keys.scroll_amount(KeyCode::Char('K'), KeyModifiers::SHIFT),
            Some(-3)
        );
        assert_eq!(
            keys.scroll_amount(KeyCode::Char('J'), KeyModifiers::SHIFT),
            Some(3)
        );
    }

    #[test]
    fn test_scroll_amount_cmd_fallback_macos_only() {
        let mut keys = test_scroll_keys();
        keys.up_fallback = None;
        keys.down_fallback = None;

        let up = keys.scroll_amount(KeyCode::Char('k'), KeyModifiers::SUPER);
        let down = keys.scroll_amount(KeyCode::Char('j'), KeyModifiers::SUPER);

        if cfg!(target_os = "macos") {
            assert_eq!(up, Some(-3));
            assert_eq!(down, Some(3));
        } else {
            assert_eq!(up, None);
            assert_eq!(down, None);
        }
    }

    #[test]
    fn test_prompt_jump_ctrl_bracket_fallback() {
        let keys = test_scroll_keys();
        assert_eq!(
            keys.prompt_jump(KeyCode::Char('['), KeyModifiers::CONTROL),
            Some(-1)
        );
        assert_eq!(
            keys.prompt_jump(KeyCode::Char(']'), KeyModifiers::CONTROL),
            Some(1)
        );
    }

    #[test]
    fn test_prompt_jump_ctrl_legacy_digit_fallback() {
        let keys = test_scroll_keys();
        assert_eq!(
            keys.prompt_jump(KeyCode::Char('5'), KeyModifiers::CONTROL),
            Some(1)
        );
        assert_eq!(
            keys.prompt_jump(KeyCode::Char('4'), KeyModifiers::CONTROL),
            None
        );
    }

    #[test]
    fn test_parse_keybinding_command_and_meta_modifiers() {
        let cmd = parse_keybinding("cmd+j").expect("cmd+j should parse");
        assert_eq!(cmd.code, KeyCode::Char('j'));
        assert!(cmd.modifiers.contains(KeyModifiers::SUPER));

        let meta = parse_keybinding("meta+k").expect("meta+k should parse");
        assert_eq!(meta.code, KeyCode::Char('k'));
        assert!(meta.modifiers.contains(KeyModifiers::ALT));
    }
}
