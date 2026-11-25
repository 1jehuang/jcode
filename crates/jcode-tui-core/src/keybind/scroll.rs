use super::*;

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
    /// Jump to the bottom of the chat (`scroll_to_bottom`). Empty = disabled.
    pub to_bottom: Vec<KeyBinding>,
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

    /// Check if a key matches the jump-to-bottom binding.
    pub fn is_to_bottom(&self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        binding_list_matches(&self.to_bottom, code, modifiers)
    }

    /// Display label for the primary jump-to-bottom chord, or `None` when
    /// the action is unbound.
    pub fn to_bottom_label(&self) -> Option<String> {
        self.to_bottom.first().map(format_binding)
    }
}
