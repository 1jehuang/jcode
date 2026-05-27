use crate::config::config;
use crossterm::event::{KeyCode, KeyModifiers};

pub use jcode_tui_core::keybind::{
    CenteredToggleKeys, EffortSwitchKeys, KeyBinding, ModelSwitchKeys, OptionalBinding, ScrollKeys,
    WorkspaceNavigationDirection, WorkspaceNavigationKeys,
};
use jcode_tui_core::keybind::{
    format_binding, is_disabled, macos_option_char_to_ascii_key, parse_bindings_or_default,
    parse_keybinding, parse_optional, parse_or_default,
};

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

    let (next, _) = parse_or_default(&cfg.keybindings.model_switch_next, default_next, "Ctrl+Tab");
    let (prev, _) = parse_optional(
        &cfg.keybindings.model_switch_prev,
        default_prev,
        "Ctrl+Shift+Tab",
    );

    ModelSwitchKeys { next, prev }
}

pub fn load_workspace_navigation_keys() -> WorkspaceNavigationKeys {
    let cfg = config();

    let default_left = KeyBinding {
        code: KeyCode::Char('h'),
        modifiers: KeyModifiers::ALT,
    };
    let default_down = KeyBinding {
        code: KeyCode::Char('j'),
        modifiers: KeyModifiers::ALT,
    };
    let default_up = KeyBinding {
        code: KeyCode::Char('k'),
        modifiers: KeyModifiers::ALT,
    };
    let default_right = KeyBinding {
        code: KeyCode::Char('l'),
        modifiers: KeyModifiers::ALT,
    };

    let (left, _) =
        parse_bindings_or_default(&cfg.keybindings.workspace_left, vec![default_left], "Alt+H");
    let (down, _) =
        parse_bindings_or_default(&cfg.keybindings.workspace_down, vec![default_down], "Alt+J");
    let (up, _) =
        parse_bindings_or_default(&cfg.keybindings.workspace_up, vec![default_up], "Alt+K");
    let (right, _) = parse_bindings_or_default(
        &cfg.keybindings.workspace_right,
        vec![default_right],
        "Alt+L",
    );

    WorkspaceNavigationKeys {
        left,
        down,
        up,
        right,
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

    let (up, _) = parse_or_default(&cfg.keybindings.scroll_up, default_up, "Ctrl+K");
    let (down, _) = parse_or_default(&cfg.keybindings.scroll_down, default_down, "Ctrl+J");
    let default_up_fallback = KeyBinding {
        code: KeyCode::Char('k'),
        modifiers: KeyModifiers::SUPER,
    };
    let default_down_fallback = KeyBinding {
        code: KeyCode::Char('j'),
        modifiers: KeyModifiers::SUPER,
    };
    let (up_fallback, _) = parse_optional(
        &cfg.keybindings.scroll_up_fallback,
        default_up_fallback,
        "Cmd+K",
    );
    let (down_fallback, _) = parse_optional(
        &cfg.keybindings.scroll_down_fallback,
        default_down_fallback,
        "Cmd+J",
    );
    let (page_up, _) = parse_or_default(&cfg.keybindings.scroll_page_up, default_page_up, "Alt+U");
    let (page_down, _) = parse_or_default(
        &cfg.keybindings.scroll_page_down,
        default_page_down,
        "Alt+D",
    );
    let (prompt_up, _) = parse_or_default(
        &cfg.keybindings.scroll_prompt_up,
        default_prompt_up,
        "Ctrl+[",
    );
    let (prompt_down, _) = parse_or_default(
        &cfg.keybindings.scroll_prompt_down,
        default_prompt_down,
        "Ctrl+]",
    );
    let (bookmark, _) =
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

    let (increase, _) = parse_or_default(
        &cfg.keybindings.effort_increase,
        default_increase,
        "Alt+Right",
    );
    let (decrease, _) = parse_or_default(
        &cfg.keybindings.effort_decrease,
        default_decrease,
        "Alt+Left",
    );

    EffortSwitchKeys { increase, decrease }
}

pub fn load_centered_toggle_key() -> CenteredToggleKeys {
    let cfg = config();

    let default_toggle = KeyBinding {
        code: KeyCode::Char('c'),
        modifiers: KeyModifiers::ALT,
    };

    let (toggle, _) = parse_or_default(&cfg.keybindings.centered_toggle, default_toggle, "Alt+C");

    CenteredToggleKeys { toggle }
}

pub(crate) fn side_panel_toggle_key_label() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "⌥+M"
    }
    #[cfg(not(target_os = "macos"))]
    {
        "Alt+M"
    }
}

pub(crate) fn matches_side_panel_toggle_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    matches_side_panel_toggle_key_for_platform(code, modifiers, cfg!(target_os = "macos"))
}

pub(crate) fn shortcut_char_for_macos_option_key(
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Option<char> {
    shortcut_char_for_macos_option_key_for_platform(code, modifiers, cfg!(target_os = "macos"))
}

pub(crate) fn shortcut_char_for_macos_option_shift_key(
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Option<char> {
    shortcut_char_for_macos_option_shift_key_for_platform(
        code,
        modifiers,
        cfg!(target_os = "macos"),
    )
}

fn shortcut_char_for_macos_option_key_for_platform(
    code: KeyCode,
    modifiers: KeyModifiers,
    is_macos: bool,
) -> Option<char> {
    if !is_macos || !modifiers.is_empty() {
        return None;
    }
    macos_option_char_to_ascii_key(code)
}

fn shortcut_char_for_macos_option_shift_key_for_platform(
    code: KeyCode,
    modifiers: KeyModifiers,
    is_macos: bool,
) -> Option<char> {
    if !is_macos || !modifiers.is_empty() {
        return None;
    }
    macos_option_shift_char_to_ascii_key(code)
}

fn macos_option_shift_char_to_ascii_key(code: KeyCode) -> Option<char> {
    let KeyCode::Char(ch) = code else {
        return None;
    };

    // macOS terminals that do not treat Option as Meta/Alt insert these Unicode
    // characters for Option+Shift+letter on a US keyboard. Copy badges advertise
    // [Alt] [⇧] [key], so normalize the inserted character back to the badge key.
    match ch {
        'Å' => Some('a'),
        'ı' => Some('b'),
        'Ç' => Some('c'),
        'Î' => Some('d'),
        '´' => Some('e'),
        'Ï' => Some('f'),
        'Ó' => Some('h'),
        'ˆ' => Some('i'),
        'Ô' => Some('j'),
        '' => Some('k'),
        'Ò' => Some('l'),
        'Â' => Some('m'),
        'Í' => Some('s'),
        'ˇ' => Some('t'),
        '¨' => Some('u'),
        '◊' => Some('v'),
        'Á' => Some('y'),
        _ => None,
    }
}

fn matches_side_panel_toggle_key_for_platform(
    code: KeyCode,
    modifiers: KeyModifiers,
    is_macos: bool,
) -> bool {
    if modifiers.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char('m')) {
        return true;
    }

    // macOS terminals often insert Option+M as `µ` unless Option is configured
    // as Meta/Alt. Treat that character as the same toggle so the advertised
    // shortcut works with the default Terminal/iTerm-style Option behavior.
    if shortcut_char_for_macos_option_key_for_platform(code, modifiers, is_macos) == Some('m') {
        return true;
    }

    false
}

pub fn load_dictation_key() -> OptionalBinding {
    let cfg = config();
    let raw = cfg.dictation.key.trim();
    if raw.is_empty() || is_disabled(raw) {
        return OptionalBinding::default();
    }
    match parse_keybinding(raw) {
        Some(binding) => OptionalBinding {
            label: Some(format_binding(&binding)),
            binding: Some(binding),
        },
        None => OptionalBinding::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_panel_toggle_matches_alt_m_on_all_platforms() {
        assert!(matches_side_panel_toggle_key_for_platform(
            KeyCode::Char('m'),
            KeyModifiers::ALT,
            false,
        ));
        assert!(matches_side_panel_toggle_key_for_platform(
            KeyCode::Char('m'),
            KeyModifiers::ALT,
            true,
        ));
    }

    #[test]
    fn side_panel_toggle_matches_macos_option_m_micro_sign() {
        assert!(matches_side_panel_toggle_key_for_platform(
            KeyCode::Char('µ'),
            KeyModifiers::empty(),
            true,
        ));
        assert!(!matches_side_panel_toggle_key_for_platform(
            KeyCode::Char('µ'),
            KeyModifiers::empty(),
            false,
        ));
    }

    #[test]
    fn side_panel_toggle_rejects_plain_m() {
        assert!(!matches_side_panel_toggle_key_for_platform(
            KeyCode::Char('m'),
            KeyModifiers::empty(),
            true,
        ));
    }

    #[test]
    fn macos_option_shortcut_chars_cover_builtin_alt_letter_shortcuts() {
        for (option_char, ascii) in [
            ('å', 'a'),
            ('∫', 'b'),
            ('ç', 'c'),
            ('∂', 'd'),
            ('´', 'e'),
            ('ƒ', 'f'),
            ('˙', 'h'),
            ('ˆ', 'i'),
            ('∆', 'j'),
            ('˚', 'k'),
            ('¬', 'l'),
            ('µ', 'm'),
            ('ß', 's'),
            ('†', 't'),
            ('¨', 'u'),
            ('√', 'v'),
            ('¥', 'y'),
        ] {
            assert_eq!(
                shortcut_char_for_macos_option_key_for_platform(
                    KeyCode::Char(option_char),
                    KeyModifiers::empty(),
                    true,
                ),
                Some(ascii),
                "Option+{ascii} should map from {option_char}"
            );
        }
    }

    #[test]
    fn macos_option_shift_shortcut_chars_cover_builtin_alt_shift_letter_shortcuts() {
        for (option_shift_char, ascii) in [
            ('Å', 'a'),
            ('ı', 'b'),
            ('Ç', 'c'),
            ('Î', 'd'),
            ('´', 'e'),
            ('Ï', 'f'),
            ('Ó', 'h'),
            ('ˆ', 'i'),
            ('Ô', 'j'),
            ('', 'k'),
            ('Ò', 'l'),
            ('Â', 'm'),
            ('Í', 's'),
            ('ˇ', 't'),
            ('¨', 'u'),
            ('◊', 'v'),
            ('Á', 'y'),
        ] {
            assert_eq!(
                shortcut_char_for_macos_option_shift_key_for_platform(
                    KeyCode::Char(option_shift_char),
                    KeyModifiers::empty(),
                    true,
                ),
                Some(ascii),
                "Option+Shift+{ascii} should map from {option_shift_char}"
            );
        }
    }
}
