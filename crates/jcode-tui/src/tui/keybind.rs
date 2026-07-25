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

// Re-export the per-platform keybinding registry + provenance + validation API
// so the rest of the TUI can reach it via `crate::tui::keybind::*`.
#[allow(unused_imports)]
pub use jcode_config_types::keybindings::{
    KEYBINDING_DEFAULTS, KeybindingDefault, KeybindingIssue, KeybindingIssueKind,
    KeybindingPlatform, KeybindingProvenance, PlatformDefault, default_binding,
    keybinding_defaults_report, validate_keybinding_defaults,
};

/// Emit a one-time log warning for every keybinding default that is asymmetric
/// across platforms or relies on an unconfirmed auto-translation. This is the
/// "check layer": it nudges developers to confirm/fix per-platform defaults
/// without blocking startup.
pub fn log_keybinding_default_warnings() {
    let issues = validate_keybinding_defaults();
    if issues.is_empty() {
        return;
    }
    crate::logging::warn(&format!(
        "KEYBINDINGS: {} default(s) need review (platform asymmetry / unconfirmed auto-translation)",
        issues.len()
    ));
    for issue in issues {
        crate::logging::warn(&format!("KEYBINDINGS: {}", issue.message));
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

    let (next, _) = parse_or_default(&cfg.keybindings.model_switch_next, default_next, "Ctrl+Tab");
    let (prev, _) = parse_optional(
        &cfg.keybindings.model_switch_prev,
        default_prev,
        "Ctrl+Shift+Tab",
    );

    ModelSwitchKeys { next, prev }
}

/// Binding that accepts the post-error fallback offer (switch to the next best
/// model/auth-method and resend). Defaults to Ctrl+Y; set "" to disable.
pub fn load_fallback_switch_key() -> OptionalBinding {
    let cfg = config();
    let raw = cfg.keybindings.fallback_switch.trim();
    if raw.is_empty() || is_disabled(raw) {
        return OptionalBinding::default();
    }
    match parse_keybinding(raw) {
        Some(binding) => OptionalBinding {
            label: Some(format_binding(&binding)),
            binding: Some(binding),
        },
        None => OptionalBinding {
            label: Some("Ctrl+Y".to_string()),
            binding: Some(KeyBinding {
                code: KeyCode::Char('y'),
                modifiers: KeyModifiers::CONTROL,
            }),
        },
    }
}

/// Human-facing label for the fallback-switch key, for use in offer messages.
pub fn fallback_switch_key_label() -> String {
    load_fallback_switch_key()
        .label
        .unwrap_or_else(|| "Ctrl+Y".to_string())
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

    // Default to Ctrl+Shift+K/J for incremental scroll; Ctrl+K/J (un-shifted)
    // move by prompt. Alt+U/D for page scroll.
    let default_up = KeyBinding {
        code: KeyCode::Char('k'),
        modifiers: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    };
    let default_down = KeyBinding {
        code: KeyCode::Char('j'),
        modifiers: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
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
        code: KeyCode::Char('k'),
        modifiers: KeyModifiers::CONTROL,
    };
    let default_prompt_down = KeyBinding {
        code: KeyCode::Char('j'),
        modifiers: KeyModifiers::CONTROL,
    };
    let default_bookmark = KeyBinding {
        code: KeyCode::Char('g'),
        modifiers: KeyModifiers::CONTROL,
    };

    let (up, _) = parse_or_default(&cfg.keybindings.scroll_up, default_up, "Ctrl+Shift+K");
    let (down, _) = parse_or_default(&cfg.keybindings.scroll_down, default_down, "Ctrl+Shift+J");
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
        "Ctrl+K",
    );
    let (prompt_down, _) = parse_or_default(
        &cfg.keybindings.scroll_prompt_down,
        default_prompt_down,
        "Ctrl+J",
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

    // macOS defaults to Cmd+Left/Right so Option+Left/Right stays free for
    // word navigation; other platforms keep Alt+Left/Right.
    let (default_increase, default_decrease, increase_label, decrease_label) =
        if cfg!(target_os = "macos") {
            (
                KeyBinding {
                    code: KeyCode::Right,
                    modifiers: KeyModifiers::SUPER,
                },
                KeyBinding {
                    code: KeyCode::Left,
                    modifiers: KeyModifiers::SUPER,
                },
                "Cmd+Right",
                "Cmd+Left",
            )
        } else {
            (
                KeyBinding {
                    code: KeyCode::Right,
                    modifiers: KeyModifiers::ALT,
                },
                KeyBinding {
                    code: KeyCode::Left,
                    modifiers: KeyModifiers::ALT,
                },
                "Alt+Right",
                "Alt+Left",
            )
        };

    let (increase, _) = parse_or_default(
        &cfg.keybindings.effort_increase,
        default_increase,
        increase_label,
    );
    let (decrease, _) = parse_or_default(
        &cfg.keybindings.effort_decrease,
        default_decrease,
        decrease_label,
    );

    EffortSwitchKeys { increase, decrease }
}

/// User-facing label for the effort cycle keys, e.g. "Cmd+Left / Cmd+Right".
pub fn effort_switch_keys_label() -> String {
    let keys = load_effort_switch_keys();
    format!(
        "{} / {}",
        format_binding(&keys.decrease),
        format_binding(&keys.increase)
    )
}

/// Display label for the "next model" switch chord, or `None` when the binding
/// is disabled. Used by the learned-keybinding hint registry.
pub(crate) fn model_switch_next_label() -> Option<String> {
    let cfg = config();
    let raw = cfg.keybindings.model_switch_next.trim();
    if raw.is_empty() || is_disabled(raw) {
        return None;
    }
    Some(format_binding(&load_model_switch_keys().next))
}

/// Display label for the effort-increase chord, or `None` when disabled.
pub(crate) fn effort_increase_label() -> Option<String> {
    let cfg = config();
    let raw = cfg.keybindings.effort_increase.trim();
    if raw.is_empty() || is_disabled(raw) {
        return None;
    }
    Some(format_binding(&load_effort_switch_keys().increase))
}

/// Display label for the alignment (centered-mode) toggle, or `None` when unbound.
pub(crate) fn centered_toggle_label() -> Option<String> {
    load_centered_toggle_key()
        .toggle
        .as_ref()
        .map(format_binding)
}

pub fn load_centered_toggle_key() -> CenteredToggleKeys {
    let cfg = config();

    let default_toggle = KeyBinding {
        code: KeyCode::Char('c'),
        modifiers: KeyModifiers::ALT,
    };

    let (toggle, _) = parse_optional(&cfg.keybindings.centered_toggle, default_toggle, "Alt+C");

    CenteredToggleKeys { toggle }
}

/// A single configurable toggle binding plus, when the binding is `alt+<letter>`,
/// the letter used to match macOS terminals that insert Option+letter as a Unicode
/// character (e.g. Option+M -> `µ`).
#[derive(Clone, Debug)]
pub struct ToggleBinding {
    binding: Option<KeyBinding>,
    macos_option_letter: Option<char>,
}

impl ToggleBinding {
    fn load(raw: &str, default_letter: char) -> Self {
        Self::load_with_default(
            raw,
            KeyBinding {
                code: KeyCode::Char(default_letter),
                modifiers: KeyModifiers::ALT,
            },
        )
    }

    /// Load a toggle binding from an explicit default chord.
    fn load_with_default(raw: &str, default: KeyBinding) -> Self {
        let default_label = format_binding(&default);
        let (binding, _) = parse_optional(raw, default, &default_label);
        let macos_option_letter = binding.as_ref().and_then(|b| {
            if b.modifiers == KeyModifiers::ALT
                && let KeyCode::Char(c) = b.code
            {
                return Some(c.to_ascii_lowercase());
            }
            None
        });
        Self {
            binding,
            macos_option_letter,
        }
    }

    pub fn matches(&self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        if let Some(binding) = &self.binding
            && binding.matches(code, modifiers)
        {
            return true;
        }
        if let Some(letter) = self.macos_option_letter
            && shortcut_char_for_macos_option_key(code, modifiers) == Some(letter)
        {
            return true;
        }
        false
    }

    /// The configured chord, or `None` when the toggle is disabled.
    pub fn binding(&self) -> Option<&KeyBinding> {
        self.binding.as_ref()
    }
}

/// All configurable pane / mode toggle keybindings.
#[derive(Clone, Debug)]
pub struct ToggleKeys {
    pub side_panel: ToggleBinding,
    pub copy_selection: ToggleBinding,
    pub diagram_pane: ToggleBinding,
    pub typing_scroll_lock: ToggleBinding,
    pub diff_mode_cycle: ToggleBinding,
    pub info_widget: ToggleBinding,
    pub todo_card: ToggleBinding,
    pub swarm_panel_focus: ToggleBinding,
}

pub fn load_toggle_keys() -> ToggleKeys {
    let cfg = config();
    ToggleKeys {
        side_panel: ToggleBinding::load(&cfg.keybindings.side_panel_toggle, 'm'),
        copy_selection: ToggleBinding::load(&cfg.keybindings.copy_selection_toggle, 'y'),
        diagram_pane: ToggleBinding::load(&cfg.keybindings.diagram_pane_toggle, 't'),
        typing_scroll_lock: ToggleBinding::load(&cfg.keybindings.typing_scroll_lock_toggle, 's'),
        diff_mode_cycle: ToggleBinding::load(&cfg.keybindings.diff_mode_cycle, 'g'),
        info_widget: ToggleBinding::load(&cfg.keybindings.info_widget_toggle, 'i'),
        todo_card: ToggleBinding::load(&cfg.keybindings.todo_card_toggle, 'x'),
        swarm_panel_focus: ToggleBinding::load_with_default(
            &cfg.keybindings.swarm_panel_focus,
            swarm_panel_focus_default(),
        ),
    }
}

/// `/effort` hint text. Mac keyboards label the Alt modifier ⌥, not "Alt".
#[cfg(target_os = "macos")]
pub(crate) const EFFORT_HELP: &str = "Show/change reasoning effort (⌥+left/right)";
#[cfg(not(target_os = "macos"))]
pub(crate) const EFFORT_HELP: &str = "Show/change reasoning effort (Alt+left/right)";

/// The default swarm-panel focus chord: Alt+N.
fn swarm_panel_focus_default() -> KeyBinding {
    KeyBinding {
        code: KeyCode::Char('n'),
        modifiers: KeyModifiers::ALT,
    }
}

pub(crate) fn side_panel_toggle_key_label() -> String {
    jcode_tui_core::keybind::alt_chord("M")
}

/// Status-line hint shown when the inline swarm controls open.
pub(crate) fn swarm_view_hint(next: &str) -> String {
    use jcode_tui_core::keybind::alt_chord_lower;
    format!(
        "Swarm: {} {next} · {} select · {} open · esc",
        alt_chord_lower("n"),
        alt_chord_lower("↑/↓"),
        alt_chord_lower("o"),
    )
}

/// Status-line hint shown when the full swarm page opens.
pub(crate) fn swarm_page_hint() -> String {
    use jcode_tui_core::keybind::alt_chord_lower;
    format!(
        "Swarm page: {} chat · {} select · {} open · esc",
        alt_chord_lower("n"),
        alt_chord_lower("↑/↓"),
        alt_chord_lower("o"),
    )
}

/// Human-friendly label for the configured swarm-panel focus chord (e.g.
/// "Alt+N"), used in the inline swarm strip's enter-controls hint.
pub(crate) fn swarm_panel_focus_key_label() -> String {
    let cfg = config();
    let default = swarm_panel_focus_default();
    let default_label = format_binding(&default);
    let (binding, _) = parse_optional(&cfg.keybindings.swarm_panel_focus, default, &default_label);
    match binding {
        Some(b) => format_binding(&b),
        None => default_label,
    }
}

/// Human-friendly label for the configured todo-card toggle chord (e.g.
/// "Alt+X"), used in /todos help text.
pub(crate) fn todo_card_key_label() -> String {
    let cfg = config();
    let default = KeyBinding {
        code: KeyCode::Char('x'),
        modifiers: KeyModifiers::ALT,
    };
    let default_label = format_binding(&default);
    let (binding, _) = parse_optional(&cfg.keybindings.todo_card_toggle, default, &default_label);
    match binding {
        Some(b) => format_binding(&b),
        None => default_label,
    }
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

#[cfg(test)]
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

/// Optional binding that spawns a fresh jcode session in a new terminal window.
/// Unbound by default; users opt in with e.g. `new_terminal = "alt+enter"`.
pub fn load_new_terminal_key() -> OptionalBinding {
    let cfg = config();
    let raw = cfg.keybindings.new_terminal.trim();
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

/// Optional binding that opens the `/resume` session picker.
/// Default: Cmd+B on macOS, Alt+R elsewhere. Set "" to disable.
pub fn load_open_resume_key() -> OptionalBinding {
    let cfg = config();
    let raw = cfg.keybindings.open_resume.trim();
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
    fn new_terminal_alt_enter_binding_parses_and_matches() {
        let binding = parse_keybinding("alt+enter").expect("alt+enter should parse");
        assert!(binding.matches(KeyCode::Enter, KeyModifiers::ALT));
        assert!(!binding.matches(KeyCode::Enter, KeyModifiers::empty()));
        assert!(!binding.matches(KeyCode::Enter, KeyModifiers::SHIFT));
        assert_eq!(format_binding(&binding), "Alt+Enter");
    }

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
