use crate::config::config;
use crossterm::event::{KeyCode, KeyModifiers};

pub use jcode_tui_core::keybind::{
    CenteredToggleKeys, EffortSwitchKeys, KeyBinding, ModelSwitchKeys, OptionalBinding, ScrollKeys,
    WorkspaceNavigationDirection, WorkspaceNavigationKeys,
};
use jcode_tui_core::keybind::{
    format_binding, is_disabled, parse_bindings_or_default, parse_keybinding, parse_optional,
    parse_or_default,
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
