//! Keymap: winit key events to editor actions.
//!
//! The chords mirror the jcode TUI (see `jcode-tui/src/tui/app/input.rs`) so
//! muscle memory transfers. Resolution is a pure function of key + modifiers,
//! which lets [`PORTED`] act as a machine-checkable parity table: every chord
//! documented as ported is asserted to actually resolve to its action, so the
//! table cannot drift from the behavior.
//!
//! Like the TUI, Ctrl / Cmd (Super) / Alt are accepted interchangeably where
//! the meaning is unambiguous, because which one reaches the app depends on
//! the platform and window manager.

use winit::keyboard::{Key, ModifiersState, NamedKey};

/// What a keypress does to the composer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Insert the event's text at the cursor.
    Insert,
    Submit,
    /// Newline within the input (Shift+Enter), not a submit.
    InsertNewline,

    MoveLeft,
    MoveRight,
    MoveWordLeft,
    MoveWordRight,
    MoveHome,
    MoveEnd,

    DeleteBack,
    DeleteForward,
    DeleteWordBack,
    DeleteWordForward,
    KillToStart,
    KillToEnd,
    CutLine,

    Undo,
    Copy,
    Paste,

    HistoryPrev,
    HistoryNext,

    ScrollUp,
    ScrollDown,
    PageUp,
    PageDown,
    ScrollTop,
    ScrollBottom,

    /// Escape: cancel a running turn, else clear the input, else follow the
    /// tail. Never quits: quitting on Escape loses work.
    Cancel,
    /// Ctrl+C / Ctrl+D: interrupt while busy, quit only when idle and empty.
    InterruptOrQuit,
}

/// One row of the parity table: the chord, its action, and the TUI binding it
/// mirrors. `tui` is documentation for humans; the test asserts `chord`
/// resolves to `action` through [`resolve`].
pub struct Ported {
    pub chord: &'static str,
    pub action: Action,
    pub tui: &'static str,
}

/// Chords ported from the TUI. Adding a row without wiring the chord fails
/// `tests::every_ported_chord_resolves`.
pub const PORTED: &[Ported] = &[
    Ported {
        chord: "enter",
        action: Action::Submit,
        tui: "submit input",
    },
    Ported {
        chord: "shift+enter",
        action: Action::InsertNewline,
        tui: "newline in input",
    },
    Ported {
        chord: "backspace",
        action: Action::DeleteBack,
        tui: "delete back",
    },
    Ported {
        chord: "delete",
        action: Action::DeleteForward,
        tui: "delete forward",
    },
    Ported {
        chord: "left",
        action: Action::MoveLeft,
        tui: "cursor left",
    },
    Ported {
        chord: "right",
        action: Action::MoveRight,
        tui: "cursor right",
    },
    Ported {
        chord: "home",
        action: Action::MoveHome,
        tui: "start of input",
    },
    Ported {
        chord: "end",
        action: Action::MoveEnd,
        tui: "end of input",
    },
    // Emacs motion, from handle_control_key.
    Ported {
        chord: "ctrl+a",
        action: Action::MoveHome,
        tui: "Ctrl+A start of line",
    },
    Ported {
        chord: "ctrl+e",
        action: Action::MoveEnd,
        tui: "Ctrl+E end of line",
    },
    Ported {
        chord: "ctrl+b",
        action: Action::MoveWordLeft,
        tui: "Ctrl+B word back",
    },
    Ported {
        chord: "ctrl+f",
        action: Action::MoveWordRight,
        tui: "Ctrl+F word forward",
    },
    Ported {
        chord: "ctrl+left",
        action: Action::MoveWordLeft,
        tui: "Ctrl+Left word back",
    },
    Ported {
        chord: "ctrl+right",
        action: Action::MoveWordRight,
        tui: "Ctrl+Right word forward",
    },
    // Alt/Option motion, from handle_alt_key.
    Ported {
        chord: "alt+b",
        action: Action::MoveWordLeft,
        tui: "Alt+B word back",
    },
    Ported {
        chord: "alt+f",
        action: Action::MoveWordRight,
        tui: "Alt+F word forward",
    },
    Ported {
        chord: "alt+left",
        action: Action::MoveWordLeft,
        tui: "Alt+Left word back",
    },
    Ported {
        chord: "alt+right",
        action: Action::MoveWordRight,
        tui: "Alt+Right word forward",
    },
    // Cmd motion, from handle_super_key.
    Ported {
        chord: "super+left",
        action: Action::MoveHome,
        tui: "Cmd+Left start of line",
    },
    Ported {
        chord: "super+right",
        action: Action::MoveEnd,
        tui: "Cmd+Right end of line",
    },
    // Deletion.
    Ported {
        chord: "ctrl+u",
        action: Action::KillToStart,
        tui: "Ctrl+U kill to start",
    },
    Ported {
        chord: "ctrl+k",
        action: Action::KillToEnd,
        tui: "Ctrl+K kill to end",
    },
    Ported {
        chord: "ctrl+w",
        action: Action::DeleteWordBack,
        tui: "Ctrl+W delete word back",
    },
    Ported {
        chord: "ctrl+backspace",
        action: Action::DeleteWordBack,
        tui: "Ctrl+Backspace word back",
    },
    Ported {
        chord: "alt+backspace",
        action: Action::DeleteWordBack,
        tui: "Alt+Backspace word back",
    },
    Ported {
        chord: "alt+delete",
        action: Action::DeleteWordBack,
        tui: "Option+Delete word back",
    },
    Ported {
        chord: "super+backspace",
        action: Action::DeleteWordBack,
        tui: "Cmd+Backspace word back",
    },
    Ported {
        chord: "alt+d",
        action: Action::DeleteWordForward,
        tui: "Alt+D delete word forward",
    },
    Ported {
        chord: "ctrl+x",
        action: Action::CutLine,
        tui: "Ctrl+X cut line",
    },
    // Undo and clipboard.
    Ported {
        chord: "ctrl+z",
        action: Action::Undo,
        tui: "Ctrl+Z undo",
    },
    Ported {
        chord: "super+z",
        action: Action::Undo,
        tui: "Cmd+Z undo",
    },
    Ported {
        chord: "ctrl+v",
        action: Action::Paste,
        tui: "Ctrl+V paste",
    },
    Ported {
        chord: "super+v",
        action: Action::Paste,
        tui: "Cmd+V paste",
    },
    Ported {
        chord: "alt+v",
        action: Action::Paste,
        tui: "Alt+V paste",
    },
    Ported {
        chord: "ctrl+shift+c",
        action: Action::Copy,
        tui: "copy selection",
    },
    // History.
    Ported {
        chord: "up",
        action: Action::HistoryPrev,
        tui: "Up recall previous input",
    },
    Ported {
        chord: "down",
        action: Action::HistoryNext,
        tui: "Down recall next input",
    },
    // Scrolling, from ScrollKeys.
    Ported {
        chord: "ctrl+shift+k",
        action: Action::ScrollUp,
        tui: "Ctrl+Shift+K scroll up",
    },
    Ported {
        chord: "ctrl+shift+j",
        action: Action::ScrollDown,
        tui: "Ctrl+Shift+J scroll down",
    },
    Ported {
        chord: "pageup",
        action: Action::PageUp,
        tui: "PageUp",
    },
    Ported {
        chord: "pagedown",
        action: Action::PageDown,
        tui: "PageDown",
    },
    Ported {
        chord: "ctrl+home",
        action: Action::ScrollTop,
        tui: "scroll to top",
    },
    Ported {
        chord: "ctrl+end",
        action: Action::ScrollBottom,
        tui: "scroll to bottom",
    },
    // Interrupt / cancel.
    Ported {
        chord: "escape",
        action: Action::Cancel,
        tui: "Esc cancel or clear input",
    },
    Ported {
        chord: "ctrl+c",
        action: Action::InterruptOrQuit,
        tui: "Ctrl+C interrupt or quit",
    },
    Ported {
        chord: "ctrl+d",
        action: Action::InterruptOrQuit,
        tui: "Ctrl+D interrupt or quit",
    },
];

/// TUI chords deliberately **not** ported, with the reason. Keeps the scope
/// honest: these depend on surfaces desktop2 does not have yet.
pub const NOT_PORTED: &[(&str, &str)] = &[
    ("tab", "no slash-command autocomplete yet"),
    ("ctrl+t", "no queue mode yet"),
    ("ctrl+s", "no input stash yet"),
    ("ctrl+p", "no auto-poke yet"),
    ("ctrl+r", "no session recovery yet"),
    ("ctrl+g", "no diagram overlay yet"),
    ("ctrl+j / ctrl+[ / ctrl+]", "no prompt-jump anchors yet"),
    ("super+5", "onboarding simulator is a TUI dev aid"),
];

/// Resolve a key press to an action. `None` means the key is not bound and
/// should fall through to text insertion.
pub fn resolve(key: &Key, mods: ModifiersState) -> Option<Action> {
    let ctrl = mods.control_key();
    let alt = mods.alt_key();
    let sup = mods.super_key();
    let shift = mods.shift_key();
    // Ctrl and Cmd are interchangeable for editing chords: which one arrives
    // depends on the platform.
    let cmd = ctrl || sup;

    match key {
        Key::Named(named) => match named {
            NamedKey::Enter => Some(if shift {
                Action::InsertNewline
            } else {
                Action::Submit
            }),
            NamedKey::Escape => Some(Action::Cancel),
            NamedKey::Backspace => {
                if alt || sup || ctrl {
                    Some(Action::DeleteWordBack)
                } else {
                    Some(Action::DeleteBack)
                }
            }
            NamedKey::Delete => {
                // Option+Delete is word-delete-back on macOS keyboards, which
                // the TUI also honors.
                if alt || sup {
                    Some(Action::DeleteWordBack)
                } else {
                    Some(Action::DeleteForward)
                }
            }
            NamedKey::ArrowLeft => Some(if sup {
                Action::MoveHome
            } else if ctrl || alt {
                Action::MoveWordLeft
            } else {
                Action::MoveLeft
            }),
            NamedKey::ArrowRight => Some(if sup {
                Action::MoveEnd
            } else if ctrl || alt {
                Action::MoveWordRight
            } else {
                Action::MoveRight
            }),
            NamedKey::ArrowUp => Some(Action::HistoryPrev),
            NamedKey::ArrowDown => Some(Action::HistoryNext),
            NamedKey::Home => Some(if cmd {
                Action::ScrollTop
            } else {
                Action::MoveHome
            }),
            NamedKey::End => Some(if cmd {
                Action::ScrollBottom
            } else {
                Action::MoveEnd
            }),
            NamedKey::PageUp => Some(Action::PageUp),
            NamedKey::PageDown => Some(Action::PageDown),
            _ => None,
        },
        Key::Character(text) => {
            let ch = text.chars().next().map(|c| c.to_ascii_lowercase())?;
            // Shifted J/K with a nav modifier scroll by line, matching the
            // TUI's ScrollKeys fallback.
            if (ctrl || sup || alt) && shift {
                match ch {
                    'k' => return Some(Action::ScrollUp),
                    'j' => return Some(Action::ScrollDown),
                    'c' => return Some(Action::Copy),
                    _ => {}
                }
            }
            if alt && !ctrl && !sup {
                return match ch {
                    'b' => Some(Action::MoveWordLeft),
                    'f' => Some(Action::MoveWordRight),
                    'd' => Some(Action::DeleteWordForward),
                    'v' => Some(Action::Paste),
                    _ => None,
                };
            }
            if cmd {
                return match ch {
                    'a' => Some(Action::MoveHome),
                    'e' => Some(Action::MoveEnd),
                    'b' => Some(Action::MoveWordLeft),
                    'f' => Some(Action::MoveWordRight),
                    'u' => Some(Action::KillToStart),
                    'k' => Some(Action::KillToEnd),
                    'w' => Some(Action::DeleteWordBack),
                    'x' => Some(Action::CutLine),
                    'z' => Some(Action::Undo),
                    'v' => Some(Action::Paste),
                    'c' | 'd' => Some(Action::InterruptOrQuit),
                    _ => None,
                };
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::SmolStr;

    /// Parse a chord like `"ctrl+shift+k"` into a key and modifier state, so
    /// the parity table is written the way a human describes a shortcut.
    fn parse(chord: &str) -> (Key, ModifiersState) {
        let mut mods = ModifiersState::empty();
        let mut key = None;
        for part in chord.split('+') {
            match part {
                "ctrl" => mods |= ModifiersState::CONTROL,
                "alt" => mods |= ModifiersState::ALT,
                "shift" => mods |= ModifiersState::SHIFT,
                "super" => mods |= ModifiersState::SUPER,
                name => {
                    key = Some(match name {
                        "enter" => Key::Named(NamedKey::Enter),
                        "escape" => Key::Named(NamedKey::Escape),
                        "backspace" => Key::Named(NamedKey::Backspace),
                        "delete" => Key::Named(NamedKey::Delete),
                        "left" => Key::Named(NamedKey::ArrowLeft),
                        "right" => Key::Named(NamedKey::ArrowRight),
                        "up" => Key::Named(NamedKey::ArrowUp),
                        "down" => Key::Named(NamedKey::ArrowDown),
                        "home" => Key::Named(NamedKey::Home),
                        "end" => Key::Named(NamedKey::End),
                        "pageup" => Key::Named(NamedKey::PageUp),
                        "pagedown" => Key::Named(NamedKey::PageDown),
                        other => {
                            assert_eq!(other.chars().count(), 1, "unknown key '{other}'");
                            Key::Character(SmolStr::new(other))
                        }
                    });
                }
            }
        }
        (key.expect("chord had no key"), mods)
    }

    /// The parity table is the contract: every documented chord must really
    /// resolve to its action through the same code path the app uses.
    #[test]
    fn every_ported_chord_resolves() {
        for row in PORTED {
            let (key, mods) = parse(row.chord);
            let got = resolve(&key, mods);
            assert_eq!(
                got,
                Some(row.action),
                "chord '{}' (TUI: {}) resolved to {:?}",
                row.chord,
                row.tui,
                got
            );
        }
    }

    #[test]
    fn the_parity_table_has_no_duplicate_chords() {
        let mut seen = std::collections::HashSet::new();
        for row in PORTED {
            assert!(
                seen.insert(row.chord),
                "duplicate chord in the table: {}",
                row.chord
            );
        }
    }

    #[test]
    fn ported_and_unported_lists_do_not_overlap() {
        for (chord, _) in NOT_PORTED {
            assert!(
                !PORTED.iter().any(|row| row.chord == *chord),
                "{chord} is listed both ported and not ported"
            );
        }
    }

    /// Cmd is an alias for Ctrl on editing chords, since which modifier
    /// arrives depends on the platform and window manager.
    #[test]
    fn cmd_and_ctrl_are_interchangeable_for_editing() {
        for ch in ['a', 'e', 'u', 'k', 'w', 'x', 'z', 'v'] {
            let key = Key::Character(SmolStr::new(ch.to_string()));
            let with_ctrl = resolve(&key, ModifiersState::CONTROL);
            let with_cmd = resolve(&key, ModifiersState::SUPER);
            assert_eq!(with_ctrl, with_cmd, "ctrl+{ch} and super+{ch} diverged");
            assert!(with_ctrl.is_some(), "ctrl+{ch} is unbound");
        }
    }

    /// Every terminal spelling of "delete a word backwards" must work; the TUI
    /// accepts all of them because macOS terminals disagree.
    #[test]
    fn all_word_delete_aliases_resolve() {
        let cases = [
            (Key::Named(NamedKey::Backspace), ModifiersState::ALT),
            (Key::Named(NamedKey::Backspace), ModifiersState::SUPER),
            (Key::Named(NamedKey::Backspace), ModifiersState::CONTROL),
            (Key::Named(NamedKey::Delete), ModifiersState::ALT),
            (Key::Named(NamedKey::Delete), ModifiersState::SUPER),
            (Key::Character(SmolStr::new("w")), ModifiersState::CONTROL),
        ];
        for (key, mods) in cases {
            assert_eq!(
                resolve(&key, mods),
                Some(Action::DeleteWordBack),
                "{key:?} + {mods:?} is not word-delete-back"
            );
        }
    }

    /// Escape must never map to a quit action: quitting on Escape loses work.
    #[test]
    fn escape_cancels_and_never_quits() {
        for mods in [
            ModifiersState::empty(),
            ModifiersState::SHIFT,
            ModifiersState::CONTROL,
            ModifiersState::ALT,
        ] {
            assert_eq!(
                resolve(&Key::Named(NamedKey::Escape), mods),
                Some(Action::Cancel),
                "Escape with {mods:?} was not Cancel"
            );
        }
    }

    #[test]
    fn plain_typing_is_not_captured_as_a_shortcut() {
        // Unmodified letters must fall through to insertion, otherwise typing
        // "a" would jump the cursor home.
        for ch in ['a', 'e', 'k', 'u', 'v', 'w', 'x', 'z', 'j'] {
            let key = Key::Character(SmolStr::new(ch.to_string()));
            assert_eq!(
                resolve(&key, ModifiersState::empty()),
                None,
                "plain '{ch}' was captured as a shortcut"
            );
            assert_eq!(
                resolve(&key, ModifiersState::SHIFT),
                None,
                "shift+'{ch}' was captured as a shortcut"
            );
        }
    }

    #[test]
    fn shift_enter_inserts_a_newline_instead_of_submitting() {
        let key = Key::Named(NamedKey::Enter);
        assert_eq!(resolve(&key, ModifiersState::empty()), Some(Action::Submit));
        assert_eq!(
            resolve(&key, ModifiersState::SHIFT),
            Some(Action::InsertNewline)
        );
    }

    #[test]
    fn shifted_nav_chords_scroll_rather_than_moving_the_cursor() {
        for mods in [
            ModifiersState::CONTROL | ModifiersState::SHIFT,
            ModifiersState::SUPER | ModifiersState::SHIFT,
            ModifiersState::ALT | ModifiersState::SHIFT,
        ] {
            assert_eq!(
                resolve(&Key::Character(SmolStr::new("k")), mods),
                Some(Action::ScrollUp)
            );
            assert_eq!(
                resolve(&Key::Character(SmolStr::new("j")), mods),
                Some(Action::ScrollDown)
            );
        }
    }

    #[test]
    fn uppercase_characters_resolve_like_lowercase() {
        // Terminals and layouts may report the shifted glyph.
        assert_eq!(
            resolve(
                &Key::Character(SmolStr::new("K")),
                ModifiersState::CONTROL | ModifiersState::SHIFT
            ),
            Some(Action::ScrollUp)
        );
    }
}
