use crossterm::event::{KeyCode, KeyModifiers};

pub const LINE_SCROLL_AMOUNT: i32 = 3;

/// The macOS keycap for the Option/Alt modifier. Mac keyboards have no key
/// labelled "Alt", so hints must show `⌥` there instead.
pub const MACOS_OPTION_SYMBOL: &str = "⌥";

/// Platform label for the Alt/Option modifier: `⌥` on macOS, `Alt` elsewhere.
pub fn alt_label() -> &'static str {
    alt_label_for_platform(cfg!(target_os = "macos"))
}

pub fn alt_label_for_platform(is_macos: bool) -> &'static str {
    if is_macos { MACOS_OPTION_SYMBOL } else { "Alt" }
}

/// Build a title-case Alt chord label, e.g. `Alt+N` or `⌥+N`.
pub fn alt_chord(keys: &str) -> String {
    format!("{}+{}", alt_label(), keys)
}

/// Build a lowercase Alt chord label for compact inline hints, e.g. `alt+n`
/// or `⌥+n`.
pub fn alt_chord_lower(keys: &str) -> String {
    let label = alt_label();
    if label == MACOS_OPTION_SYMBOL {
        format!("{label}+{keys}")
    } else {
        format!("{}+{keys}", label.to_ascii_lowercase())
    }
}

#[derive(Clone, Debug)]
pub struct KeyBinding {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyBinding {
    pub fn matches(&self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        self.matches_for_platform(code, modifiers, cfg!(target_os = "macos"))
    }

    pub fn matches_for_platform(
        &self,
        code: KeyCode,
        modifiers: KeyModifiers,
        is_macos: bool,
    ) -> bool {
        let (code, modifiers) = normalize_key(code, modifiers);
        let (bind_code, bind_mods) = normalize_key(self.code, self.modifiers);
        if code == bind_code && modifiers == bind_mods {
            return true;
        }

        is_macos
            && modifiers.is_empty()
            && bind_mods == KeyModifiers::ALT
            && macos_option_char_to_ascii_key(code)
                .is_some_and(|ascii| bind_code == KeyCode::Char(ascii))
    }
}

pub fn macos_option_char_to_ascii_key(code: KeyCode) -> Option<char> {
    let KeyCode::Char(ch) = code else {
        return None;
    };

    match ch {
        'å' => Some('a'),
        '∫' => Some('b'),
        'ç' => Some('c'),
        '∂' => Some('d'),
        '´' => Some('e'),
        'ƒ' => Some('f'),
        '˙' => Some('h'),
        'ˆ' => Some('i'),
        '∆' => Some('j'),
        '˚' => Some('k'),
        '¬' => Some('l'),
        'µ' => Some('m'),
        'ß' => Some('s'),
        '†' => Some('t'),
        '¨' => Some('u'),
        '√' => Some('v'),
        '¥' => Some('y'),
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub struct ModelSwitchKeys {
    pub next: KeyBinding,
    pub prev: Option<KeyBinding>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceNavigationDirection {
    Left,
    Down,
    Up,
    Right,
}

#[derive(Clone, Debug)]
pub struct WorkspaceNavigationKeys {
    pub left: Vec<KeyBinding>,
    pub down: Vec<KeyBinding>,
    pub up: Vec<KeyBinding>,
    pub right: Vec<KeyBinding>,
}

impl WorkspaceNavigationKeys {
    pub fn direction_for(
        &self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Option<WorkspaceNavigationDirection> {
        if binding_list_matches(&self.left, code, modifiers) {
            return Some(WorkspaceNavigationDirection::Left);
        }
        if binding_list_matches(&self.down, code, modifiers) {
            return Some(WorkspaceNavigationDirection::Down);
        }
        if binding_list_matches(&self.up, code, modifiers) {
            return Some(WorkspaceNavigationDirection::Up);
        }
        if binding_list_matches(&self.right, code, modifiers) {
            return Some(WorkspaceNavigationDirection::Right);
        }
        None
    }
}

impl ModelSwitchKeys {
    pub fn direction_for(&self, code: KeyCode, modifiers: KeyModifiers) -> Option<i8> {
        if self.next.matches(code, modifiers) {
            return Some(1);
        }
        if let Some(prev) = &self.prev
            && prev.matches(code, modifiers)
        {
            return Some(-1);
        }
        None
    }
}

fn binding_list_matches(bindings: &[KeyBinding], code: KeyCode, modifiers: KeyModifiers) -> bool {
    bindings
        .iter()
        .any(|binding| binding.matches(code, modifiers))
}

pub fn parse_or_default(
    raw: &str,
    fallback: KeyBinding,
    fallback_label: &str,
) -> (KeyBinding, String) {
    match parse_keybinding(raw) {
        Some(binding) => (binding.clone(), format_binding(&binding)),
        None => (fallback.clone(), fallback_label.to_string()),
    }
}

pub fn parse_bindings_or_default(
    raw: &str,
    fallback: Vec<KeyBinding>,
    fallback_label: &str,
) -> (Vec<KeyBinding>, String) {
    let bindings = parse_keybinding_list(raw);
    if bindings.is_empty() {
        return (fallback, fallback_label.to_string());
    }
    let label = bindings
        .iter()
        .map(format_binding)
        .collect::<Vec<_>>()
        .join(", ");
    (bindings, label)
}

pub fn parse_optional(
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

pub fn parse_keybinding_list(raw: &str) -> Vec<KeyBinding> {
    let raw = raw.trim();
    if raw.is_empty() || is_disabled(raw) {
        return Vec::new();
    }

    raw.split(',').filter_map(parse_keybinding).collect()
}

pub fn is_disabled(raw: &str) -> bool {
    matches!(
        raw.to_ascii_lowercase().as_str(),
        "none" | "off" | "disabled"
    )
}

pub fn parse_keybinding(raw: &str) -> Option<KeyBinding> {
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
            "cmd" | "command" | "super" | "win" | "windows" => modifiers |= KeyModifiers::SUPER,
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
        _ => match parse_function_key(key) {
            Some(number) => KeyCode::F(number),
            None => {
                if key.len() == 1 {
                    let mut chars = key.chars();
                    let ch = chars.next()?;
                    KeyCode::Char(ch)
                } else {
                    return None;
                }
            }
        },
    };

    Some(KeyBinding { code, modifiers })
}

fn normalize_key(code: KeyCode, modifiers: KeyModifiers) -> (KeyCode, KeyModifiers) {
    if code == KeyCode::BackTab {
        return (KeyCode::Tab, modifiers | KeyModifiers::SHIFT);
    }
    // With the Kitty keyboard protocol, terminals report Ctrl+Shift+<letter>
    // as an uppercase Char plus CONTROL|SHIFT. Since Shift is already explicit
    // in the modifiers, fold the letter to lowercase so "ctrl+shift+e" matches
    // both Char('e') and Char('E') encodings.
    if modifiers.contains(KeyModifiers::SHIFT)
        && let KeyCode::Char(c) = code
        && c.is_ascii_uppercase()
    {
        return (KeyCode::Char(c.to_ascii_lowercase()), modifiers);
    }
    (code, modifiers)
}

fn parse_function_key(raw: &str) -> Option<u8> {
    let number = raw.strip_prefix('f')?.parse::<u8>().ok()?;
    (1..=24).contains(&number).then_some(number)
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
}

impl ScrollKeys {
    fn matches_scroll_up(&self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        self.up.matches(code, modifiers)
            || self
                .up_fallback
                .as_ref()
                .map(|k| k.matches(code, modifiers))
                .unwrap_or(false)
    }

    fn matches_scroll_down(&self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        self.down.matches(code, modifiers)
            || self
                .down_fallback
                .as_ref()
                .map(|k| k.matches(code, modifiers))
                .unwrap_or(false)
    }

    /// Check if a key matches scroll up (returns scroll amount, negative = up)
    pub fn scroll_amount(&self, code: KeyCode, modifiers: KeyModifiers) -> Option<i32> {
        if self.matches_scroll_up(code, modifiers) {
            return Some(-LINE_SCROLL_AMOUNT);
        }
        if self.matches_scroll_down(code, modifiers) {
            return Some(LINE_SCROLL_AMOUNT);
        }
        if self.page_up.matches(code, modifiers) {
            return Some(-10); // Page up
        }
        if self.page_down.matches(code, modifiers) {
            return Some(10); // Page down
        }

        // Built-in incremental-scroll fallback: <mod>+Shift+K / <mod>+Shift+J
        // scroll up / down one line, where <mod> is Ctrl, Cmd, or Option. This is
        // the shifted counterpart of the prompt navigation on the un-shifted
        // chords (see `prompt_jump`): plain J/K move by prompt, holding Shift
        // makes them scroll line-by-line. Terminals with the Kitty keyboard
        // protocol report these as Char('k'/'j') (or shifted 'K'/'J') with the
        // modifier set including SHIFT.
        let has_nav_mod = modifiers.intersects(
            KeyModifiers::CONTROL | KeyModifiers::SUPER | KeyModifiers::META | KeyModifiers::ALT,
        );
        if has_nav_mod && modifiers.contains(KeyModifiers::SHIFT) {
            match code {
                KeyCode::Char('k') | KeyCode::Char('K') => return Some(-LINE_SCROLL_AMOUNT),
                KeyCode::Char('j') | KeyCode::Char('J') => return Some(LINE_SCROLL_AMOUNT),
                _ => {}
            }
        }

        // NOTE: The un-shifted <mod>+J / <mod>+K chords move by prompt (see
        // `prompt_jump`) rather than line-scrolling, so they intentionally fall
        // through here to reach the prompt-jump handler.
        None
    }

    /// Check if a key matches prompt jump (returns direction: -1 = prev, 1 = next)
    pub fn prompt_jump(&self, code: KeyCode, modifiers: KeyModifiers) -> Option<i8> {
        if self.prompt_up.matches(code, modifiers) {
            return Some(-1);
        }
        if self.prompt_down.matches(code, modifiers) {
            return Some(1);
        }

        // Shifted chords are reserved for incremental scrolling (see
        // `scroll_amount`), so never treat them as prompt jumps.
        if modifiers.contains(KeyModifiers::SHIFT) {
            return None;
        }

        // Built-in prompt-jump fallbacks. With any navigation modifier (Ctrl,
        // Cmd, or Option) held and no Shift:
        //   - K / J move to the previous / next prompt, and
        //   - [ / ] do the same (Ctrl+[ / Ctrl+] also work in terminals with
        //     keyboard enhancement, where Ctrl+[ is distinguishable from Esc).
        // Cmd and Option are best-effort: they only fire if the terminal/window
        // manager forwards them instead of consuming them first.
        let has_nav_mod = modifiers.intersects(
            KeyModifiers::CONTROL | KeyModifiers::SUPER | KeyModifiers::META | KeyModifiers::ALT,
        );
        if has_nav_mod {
            match code {
                KeyCode::Char('[') => return Some(-1),
                KeyCode::Char(']') => return Some(1),
                KeyCode::Char('k') | KeyCode::Char('K') => return Some(-1),
                KeyCode::Char('j') | KeyCode::Char('J') => return Some(1),
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

#[derive(Clone, Debug)]
pub struct EffortSwitchKeys {
    pub increase: KeyBinding,
    pub decrease: KeyBinding,
}

#[derive(Clone, Debug)]
pub struct CenteredToggleKeys {
    /// The toggle binding, or `None` when the user disabled it (e.g. `none`).
    pub toggle: Option<KeyBinding>,
}

impl CenteredToggleKeys {
    pub fn matches(&self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        self.toggle
            .as_ref()
            .is_some_and(|binding| binding.matches(code, modifiers))
    }
}

#[derive(Clone, Debug, Default)]
pub struct OptionalBinding {
    pub binding: Option<KeyBinding>,
    pub label: Option<String>,
}

impl EffortSwitchKeys {
    pub fn direction_for(&self, code: KeyCode, modifiers: KeyModifiers) -> Option<i8> {
        if self.increase.matches(code, modifiers) {
            return Some(1);
        }
        if self.decrease.matches(code, modifiers) {
            return Some(-1);
        }
        None
    }

    pub fn macos_option_arrow_escape_direction_for(
        &self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Option<i8> {
        if !self.uses_default_alt_arrow_bindings() {
            return None;
        }

        let (code, modifiers) = normalize_key(code, modifiers);
        if modifiers != KeyModifiers::ALT {
            return None;
        }

        // Terminal.app and common iTerm2 profiles encode Option+Left/Right as
        // ESC+b / ESC+f. Crossterm exposes those as Alt+B / Alt+F, not Alt+Arrow.
        match code {
            KeyCode::Char('f') => Some(1),
            KeyCode::Char('b') => Some(-1),
            _ => None,
        }
    }

    fn uses_default_alt_arrow_bindings(&self) -> bool {
        self.increase.matches(KeyCode::Right, KeyModifiers::ALT)
            && self.decrease.matches(KeyCode::Left, KeyModifiers::ALT)
    }
}

pub fn format_binding(binding: &KeyBinding) -> String {
    let mut parts: Vec<String> = Vec::new();
    if binding.modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("Ctrl".to_string());
    }
    if binding.modifiers.contains(KeyModifiers::ALT) {
        parts.push(alt_label().to_string());
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
        KeyCode::F(number) => format!("F{}", number),
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

    /// Decode a kitty CSI-u modifier byte (bitfield + 1) into `KeyModifiers`.
    /// This mirrors the sequences we ask Ghostty to forward for Cmd hotkeys, so
    /// the test fails if our binding parsing drifts from that wire encoding.
    fn kitty_mods(modbyte: u8) -> KeyModifiers {
        let bits = modbyte - 1;
        let mut mods = KeyModifiers::empty();
        if bits & 1 != 0 {
            mods |= KeyModifiers::SHIFT;
        }
        if bits & 2 != 0 {
            mods |= KeyModifiers::ALT;
        }
        if bits & 4 != 0 {
            mods |= KeyModifiers::CONTROL;
        }
        if bits & 8 != 0 {
            mods |= KeyModifiers::SUPER;
        }
        mods
    }

    #[test]
    fn alt_label_uses_option_keycap_on_macos() {
        assert_eq!(alt_label_for_platform(true), MACOS_OPTION_SYMBOL);
        assert_eq!(alt_label_for_platform(false), "Alt");
    }

    #[test]
    fn ghostty_cmd_b_sequence_matches_open_resume_binding() {
        // Ghostty forwards Cmd+B as ESC[98;9u (98='b', super-only).
        let code = KeyCode::Char(char::from_u32(98).unwrap());
        let mods = kitty_mods(9);
        let binding = parse_keybinding("cmd+b").expect("cmd+b parses");
        assert!(
            binding.matches_for_platform(code, mods, true),
            "Cmd+B kitty sequence must trigger the open_resume binding"
        );
    }

    #[test]
    fn ghostty_cmd_shift_semicolon_sequence_matches_new_terminal_binding() {
        // Ghostty forwards Cmd+Shift+; as ESC[59;10u (59=';', shift+super).
        let code = KeyCode::Char(char::from_u32(59).unwrap());
        let mods = kitty_mods(10);
        let binding = parse_keybinding("cmd+shift+;").expect("cmd+shift+; parses");
        assert!(
            binding.matches_for_platform(code, mods, true),
            "Cmd+Shift+; kitty sequence must trigger the new_terminal binding"
        );
    }

    #[test]
    fn ctrl_shift_letter_matches_uppercase_and_lowercase_encodings() {
        // Terminals with the Kitty keyboard protocol report Ctrl+Shift+E as
        // either Char('e') or Char('E') with CONTROL|SHIFT. User-configured
        // ctrl+shift+<letter> chords must match both encodings.
        let binding = parse_keybinding("ctrl+shift+e").expect("ctrl+shift+e parses");
        let mods = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        assert!(binding.matches(KeyCode::Char('e'), mods));
        assert!(binding.matches(KeyCode::Char('E'), mods));
        // Plain Ctrl+E (no Shift) must not trigger the shifted binding.
        assert!(!binding.matches(KeyCode::Char('e'), KeyModifiers::CONTROL));
    }

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
        }
    }

    #[test]
    fn test_scroll_amount_ctrl_fallback() {
        let mut keys = test_scroll_keys();
        keys.up = KeyBinding {
            code: KeyCode::Char('k'),
            modifiers: KeyModifiers::CONTROL,
        };
        keys.down = KeyBinding {
            code: KeyCode::Char('j'),
            modifiers: KeyModifiers::CONTROL,
        };

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
    fn test_scroll_amount_ctrl_fallback_disabled_when_rebound() {
        let keys = test_scroll_keys();

        assert_eq!(
            keys.scroll_amount(KeyCode::Char('k'), KeyModifiers::CONTROL),
            None
        );
        assert_eq!(
            keys.scroll_amount(KeyCode::Char('j'), KeyModifiers::CONTROL),
            None
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
    fn test_line_scroll_keys_scroll_three_lines() {
        let keys = test_scroll_keys();

        assert_eq!(LINE_SCROLL_AMOUNT, 3);
        assert_eq!(
            keys.scroll_amount(KeyCode::Char('k'), KeyModifiers::ALT),
            Some(-3)
        );
        assert_eq!(
            keys.scroll_amount(KeyCode::Char('j'), KeyModifiers::ALT),
            Some(3)
        );
    }

    #[test]
    fn test_scroll_amount_cmd_jk_not_line_scroll() {
        // Cmd+J / Cmd+K are prompt navigation (see test_prompt_jump_cmd_jk),
        // so they must never be treated as line scrolling on any platform.
        let mut keys = test_scroll_keys();
        keys.up_fallback = None;
        keys.down_fallback = None;

        assert_eq!(
            keys.scroll_amount(KeyCode::Char('k'), KeyModifiers::SUPER),
            None
        );
        assert_eq!(
            keys.scroll_amount(KeyCode::Char('j'), KeyModifiers::SUPER),
            None
        );
    }

    #[test]
    fn test_scroll_amount_cmd_shift_jk_line_scroll() {
        // Cmd+Shift+K / Cmd+Shift+J mirror Ctrl+Shift+K / Ctrl+Shift+J: they
        // line-scroll up / down on macOS regardless of the configured bindings.
        let mut keys = test_scroll_keys();
        keys.up_fallback = None;
        keys.down_fallback = None;

        for code in [KeyCode::Char('k'), KeyCode::Char('K')] {
            assert_eq!(
                keys.scroll_amount(code, KeyModifiers::SUPER | KeyModifiers::SHIFT),
                Some(-LINE_SCROLL_AMOUNT)
            );
        }
        for code in [KeyCode::Char('j'), KeyCode::Char('J')] {
            assert_eq!(
                keys.scroll_amount(code, KeyModifiers::SUPER | KeyModifiers::SHIFT),
                Some(LINE_SCROLL_AMOUNT)
            );
        }
    }

    #[test]
    fn test_scroll_amount_ctrl_shift_jk_line_scroll() {
        // Ctrl+Shift+K / Ctrl+Shift+J line-scroll up / down. This is the shifted
        // counterpart to the un-shifted Ctrl+J/K prompt navigation.
        let mut keys = test_scroll_keys();
        keys.up_fallback = None;
        keys.down_fallback = None;

        for code in [KeyCode::Char('k'), KeyCode::Char('K')] {
            assert_eq!(
                keys.scroll_amount(code, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
                Some(-LINE_SCROLL_AMOUNT)
            );
        }
        for code in [KeyCode::Char('j'), KeyCode::Char('J')] {
            assert_eq!(
                keys.scroll_amount(code, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
                Some(LINE_SCROLL_AMOUNT)
            );
        }
    }

    #[test]
    fn test_prompt_jump_ctrl_jk() {
        // Ctrl+K / Ctrl+J (un-shifted) move up / down by prompt: the primary
        // default that survives a stock Ghostty + tiling-WM setup.
        let keys = test_scroll_keys();
        assert_eq!(
            keys.prompt_jump(KeyCode::Char('k'), KeyModifiers::CONTROL),
            Some(-1)
        );
        assert_eq!(
            keys.prompt_jump(KeyCode::Char('j'), KeyModifiers::CONTROL),
            Some(1)
        );
    }

    #[test]
    fn test_prompt_jump_shifted_jk_is_not_prompt() {
        // Shifted chords are reserved for incremental scrolling, so they must
        // never be reported as prompt jumps regardless of the modifier family.
        let keys = test_scroll_keys();
        for mods in [
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            KeyModifiers::SUPER | KeyModifiers::SHIFT,
            KeyModifiers::ALT | KeyModifiers::SHIFT,
        ] {
            for code in [
                KeyCode::Char('k'),
                KeyCode::Char('K'),
                KeyCode::Char('j'),
                KeyCode::Char('J'),
            ] {
                assert_eq!(
                    keys.prompt_jump(code, mods),
                    None,
                    "mods={mods:?} code={code:?}"
                );
            }
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
    fn test_prompt_jump_cmd_bracket_fallback() {
        let keys = test_scroll_keys();
        assert_eq!(
            keys.prompt_jump(KeyCode::Char('['), KeyModifiers::SUPER),
            Some(-1)
        );
        assert_eq!(
            keys.prompt_jump(KeyCode::Char(']'), KeyModifiers::SUPER),
            Some(1)
        );
        assert_eq!(
            keys.prompt_jump(KeyCode::Char('['), KeyModifiers::META),
            Some(-1)
        );
        assert_eq!(
            keys.prompt_jump(KeyCode::Char(']'), KeyModifiers::META),
            Some(1)
        );
    }

    #[test]
    fn test_prompt_jump_cmd_jk() {
        // Cmd+K / Cmd+J move up / down by prompt on macOS (and any terminal that
        // forwards Command as SUPER/META).
        let keys = test_scroll_keys();
        for mods in [KeyModifiers::SUPER, KeyModifiers::META] {
            assert_eq!(keys.prompt_jump(KeyCode::Char('k'), mods), Some(-1));
            assert_eq!(keys.prompt_jump(KeyCode::Char('K'), mods), Some(-1));
            assert_eq!(keys.prompt_jump(KeyCode::Char('j'), mods), Some(1));
            assert_eq!(keys.prompt_jump(KeyCode::Char('J'), mods), Some(1));
        }
    }

    #[test]
    fn test_prompt_jump_option_jk() {
        // Option (Alt) + K / J mirror Cmd+K / Cmd+J for prompt navigation on macOS.
        let keys = test_scroll_keys();
        assert_eq!(
            keys.prompt_jump(KeyCode::Char('k'), KeyModifiers::ALT),
            Some(-1)
        );
        assert_eq!(
            keys.prompt_jump(KeyCode::Char('K'), KeyModifiers::ALT),
            Some(-1)
        );
        assert_eq!(
            keys.prompt_jump(KeyCode::Char('j'), KeyModifiers::ALT),
            Some(1)
        );
        assert_eq!(
            keys.prompt_jump(KeyCode::Char('J'), KeyModifiers::ALT),
            Some(1)
        );
    }

    #[test]
    fn test_prompt_jump_ctrl_digit_reserved_for_rank_jump() {
        let keys = test_scroll_keys();
        assert_eq!(
            keys.prompt_jump(KeyCode::Char('5'), KeyModifiers::CONTROL),
            None
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

        for raw in ["command+k", "super+k", "win+k", "windows+k"] {
            let binding = parse_keybinding(raw).unwrap_or_else(|| panic!("{raw} should parse"));
            assert_eq!(binding.code, KeyCode::Char('k'));
            assert_eq!(binding.modifiers, KeyModifiers::SUPER);
        }

        let control = parse_keybinding("control+j").expect("control+j should parse");
        assert_eq!(control.code, KeyCode::Char('j'));
        assert_eq!(control.modifiers, KeyModifiers::CONTROL);

        let option_left = parse_keybinding("option+left").expect("option+left should parse");
        assert_eq!(option_left.code, KeyCode::Left);
        assert!(option_left.modifiers.contains(KeyModifiers::ALT));

        let meta = parse_keybinding("meta+k").expect("meta+k should parse");
        assert_eq!(meta.code, KeyCode::Char('k'));
        assert!(meta.modifiers.contains(KeyModifiers::ALT));
    }

    #[test]
    fn key_binding_matches_macos_option_translated_characters() {
        let binding = parse_keybinding("alt+s").expect("alt+s should parse");

        assert!(binding.matches_for_platform(KeyCode::Char('s'), KeyModifiers::ALT, false,));
        assert!(binding.matches_for_platform(KeyCode::Char('ß'), KeyModifiers::empty(), true,));
        assert!(!binding.matches_for_platform(KeyCode::Char('ß'), KeyModifiers::empty(), false,));
    }

    #[test]
    fn macos_option_character_map_covers_default_alt_shortcuts() {
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
                macos_option_char_to_ascii_key(KeyCode::Char(option_char)),
                Some(ascii)
            );
        }
    }

    #[test]
    fn effort_switch_keys_match_macos_option_arrows_as_alt_arrows() {
        let keys = EffortSwitchKeys {
            increase: parse_keybinding("alt+right").expect("alt+right should parse"),
            decrease: parse_keybinding("alt+left").expect("alt+left should parse"),
        };

        // macOS labels the Alt modifier as Option (⌥). Terminals that forward
        // Option-arrow as an Alt-modified arrow should adjust reasoning effort.
        assert_eq!(
            keys.direction_for(KeyCode::Right, KeyModifiers::ALT),
            Some(1)
        );
        assert_eq!(
            keys.direction_for(KeyCode::Left, KeyModifiers::ALT),
            Some(-1)
        );
        assert_eq!(
            parse_keybinding("option+right")
                .expect("option+right should parse")
                .modifiers,
            KeyModifiers::ALT
        );
    }

    #[test]
    fn effort_switch_keys_match_macos_terminal_option_arrow_escape_encoding() {
        let keys = EffortSwitchKeys {
            increase: parse_keybinding("alt+right").expect("alt+right should parse"),
            decrease: parse_keybinding("alt+left").expect("alt+left should parse"),
        };

        // Terminal.app and many iTerm2 profiles encode Option+Right as ESC+f
        // and Option+Left as ESC+b. Crossterm reports those as Alt+F/B.
        assert_eq!(
            keys.macos_option_arrow_escape_direction_for(KeyCode::Char('f'), KeyModifiers::ALT),
            Some(1)
        );
        assert_eq!(
            keys.macos_option_arrow_escape_direction_for(KeyCode::Char('b'), KeyModifiers::ALT),
            Some(-1)
        );
        assert_eq!(
            keys.macos_option_arrow_escape_direction_for(KeyCode::Char('f'), KeyModifiers::empty()),
            None
        );
    }

    #[test]
    fn effort_switch_keys_do_not_apply_macos_escape_aliases_after_remap() {
        let keys = EffortSwitchKeys {
            increase: parse_keybinding("ctrl+right").expect("ctrl+right should parse"),
            decrease: parse_keybinding("ctrl+left").expect("ctrl+left should parse"),
        };

        assert_eq!(
            keys.macos_option_arrow_escape_direction_for(KeyCode::Char('f'), KeyModifiers::ALT),
            None
        );
        assert_eq!(
            keys.macos_option_arrow_escape_direction_for(KeyCode::Char('b'), KeyModifiers::ALT),
            None
        );
    }

    #[test]
    fn test_parse_function_keybinding_for_copilot_style_keys() {
        let binding = parse_keybinding("ctrl+shift+f23").expect("f23 binding should parse");
        assert_eq!(binding.code, KeyCode::F(23));
        assert!(binding.modifiers.contains(KeyModifiers::CONTROL));
        assert!(binding.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(format_binding(&binding), "Ctrl+Shift+F23");
    }

    #[test]
    fn workspace_navigation_keys_match_super_bindings() {
        let keys = WorkspaceNavigationKeys {
            left: vec![KeyBinding {
                code: KeyCode::Char('h'),
                modifiers: KeyModifiers::SUPER,
            }],
            down: vec![KeyBinding {
                code: KeyCode::Char('j'),
                modifiers: KeyModifiers::SUPER,
            }],
            up: vec![KeyBinding {
                code: KeyCode::Char('k'),
                modifiers: KeyModifiers::SUPER,
            }],
            right: vec![KeyBinding {
                code: KeyCode::Char('l'),
                modifiers: KeyModifiers::SUPER,
            }],
        };

        assert_eq!(
            keys.direction_for(KeyCode::Char('h'), KeyModifiers::SUPER),
            Some(WorkspaceNavigationDirection::Left)
        );
        assert_eq!(
            keys.direction_for(KeyCode::Char('j'), KeyModifiers::SUPER),
            Some(WorkspaceNavigationDirection::Down)
        );
        assert_eq!(
            keys.direction_for(KeyCode::Char('k'), KeyModifiers::SUPER),
            Some(WorkspaceNavigationDirection::Up)
        );
        assert_eq!(
            keys.direction_for(KeyCode::Char('l'), KeyModifiers::SUPER),
            Some(WorkspaceNavigationDirection::Right)
        );
        assert_eq!(
            keys.direction_for(KeyCode::Char('h'), KeyModifiers::ALT),
            None
        );
    }

    #[test]
    fn workspace_navigation_keys_support_multiple_aliases() {
        let keys = WorkspaceNavigationKeys {
            left: vec![
                KeyBinding {
                    code: KeyCode::Char('h'),
                    modifiers: KeyModifiers::SUPER,
                },
                KeyBinding {
                    code: KeyCode::Left,
                    modifiers: KeyModifiers::SUPER,
                },
                KeyBinding {
                    code: KeyCode::Left,
                    modifiers: KeyModifiers::ALT,
                },
                KeyBinding {
                    code: KeyCode::Char('h'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ],
            down: vec![
                KeyBinding {
                    code: KeyCode::Char('j'),
                    modifiers: KeyModifiers::SUPER,
                },
                KeyBinding {
                    code: KeyCode::Down,
                    modifiers: KeyModifiers::SUPER,
                },
                KeyBinding {
                    code: KeyCode::Down,
                    modifiers: KeyModifiers::ALT,
                },
                KeyBinding {
                    code: KeyCode::Char('j'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ],
            up: vec![
                KeyBinding {
                    code: KeyCode::Char('k'),
                    modifiers: KeyModifiers::SUPER,
                },
                KeyBinding {
                    code: KeyCode::Up,
                    modifiers: KeyModifiers::SUPER,
                },
                KeyBinding {
                    code: KeyCode::Up,
                    modifiers: KeyModifiers::ALT,
                },
                KeyBinding {
                    code: KeyCode::Char('k'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ],
            right: vec![
                KeyBinding {
                    code: KeyCode::Char('l'),
                    modifiers: KeyModifiers::SUPER,
                },
                KeyBinding {
                    code: KeyCode::Right,
                    modifiers: KeyModifiers::SUPER,
                },
                KeyBinding {
                    code: KeyCode::Right,
                    modifiers: KeyModifiers::ALT,
                },
                KeyBinding {
                    code: KeyCode::Char('l'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ],
        };

        assert_eq!(
            keys.direction_for(KeyCode::Left, KeyModifiers::SUPER),
            Some(WorkspaceNavigationDirection::Left)
        );
        assert_eq!(
            keys.direction_for(KeyCode::Right, KeyModifiers::ALT),
            Some(WorkspaceNavigationDirection::Right)
        );
        assert_eq!(
            keys.direction_for(KeyCode::Char('j'), KeyModifiers::CONTROL),
            Some(WorkspaceNavigationDirection::Down)
        );
        assert_eq!(
            keys.direction_for(KeyCode::Char('k'), KeyModifiers::CONTROL),
            Some(WorkspaceNavigationDirection::Up)
        );
    }
}
