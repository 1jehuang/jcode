//! Keymap: winit key events to editor actions.
//!
//! desktop2 is a GUI app, so the primary contract is the **web/browser text
//! field**: Ctrl+A selects all, Ctrl+C copies, Ctrl+Z / Ctrl+Shift+Z undo and
//! redo, Shift+arrows extend the selection, Ctrl+Tab and Ctrl+PageUp/PageDown
//! walk the "tabs" (the session strip). Those are the chords a desktop user
//! already has in their hands, and violating them is a data-loss trap
//! (Ctrl+A jumping the caret, Ctrl+C killing a turn while text is selected).
//!
//! TUI/emacs chords are kept wherever they do not collide, so terminal muscle
//! memory still mostly works: Ctrl+E, Ctrl+U, Ctrl+W, Alt+B/F. Ctrl+J/K walk
//! prompts, matching the TUI's prompt navigation. Where
//! the two disagree the web binding wins and start-of-line stays reachable via
//! Home.
//!
//! Resolution is a pure function of key + modifiers, which lets [`PORTED`] act
//! as a machine-checkable table: every documented chord is asserted to really
//! resolve to its action, so the table cannot drift from the behavior.
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
    /// Open or close the Desktop2 help overlay. F1 is intentionally unmodified
    /// so the feature is discoverable without knowing a command first.
    ToggleHelp,
    /// Newline within the input (Shift+Enter), not a submit.
    InsertNewline,

    MoveLeft,
    MoveRight,
    MoveWordLeft,
    MoveWordRight,
    MoveHome,
    MoveEnd,

    /// Shift+motion: extend the selection instead of collapsing it.
    ExtendLeft,
    ExtendRight,
    ExtendWordLeft,
    ExtendWordRight,
    ExtendHome,
    ExtendEnd,
    /// Shift+Up / Shift+Down: grow the selection a line at a time, clamping to
    /// the buffer edges like a browser textarea.
    ExtendLineUp,
    ExtendLineDown,
    /// Ctrl+Home / Ctrl+End inside a text field: caret to the very start/end.
    MoveDocStart,
    MoveDocEnd,
    ExtendDocStart,
    ExtendDocEnd,
    SelectAll,

    DeleteBack,
    DeleteForward,
    DeleteWordBack,
    DeleteWordForward,
    KillToStart,
    KillToEnd,
    CutLine,

    Undo,
    Redo,
    Copy,
    /// Ctrl+C: copy when something is selected, otherwise interrupt or quit.
    /// The web meaning wins whenever it can apply, so Ctrl+C never silently
    /// kills a turn while the user was trying to copy.
    CopyOrInterrupt,
    Paste,

    HistoryPrev,
    HistoryNext,

    ScrollUp,
    ScrollDown,
    PageUp,
    PageDown,
    ScrollTop,
    ScrollBottom,
    /// Jump to the adjacent user prompt. "Previous" travels toward older
    /// transcript content and "next" travels back toward the live tail.
    PromptPrev,
    PromptNext,

    /// Escape: cancel a running turn, else clear the input, else follow the
    /// tail. Never quits: quitting on Escape loses work.
    Cancel,
    /// Ctrl+C / Ctrl+D: interrupt while busy, quit only when idle and empty.
    InterruptOrQuit,

    /// Session strip navigation, mirroring niri's spatial motion: left/right
    /// walk the sessions in the current working directory, up/down walk the
    /// directories. Bound to Ctrl+Shift+arrows because plain and Alt arrows
    /// are already text motion, and typing must always win over chrome.
    SessionLeft,
    SessionRight,
    SessionUp,
    SessionDown,
    /// Ctrl+Alt+Shift+Left/Right: resize only the focused session page while
    /// leaving the application window and UI scale unchanged.
    PanelShrink,
    PanelGrow,

    /// Cmd/Ctrl+T or Ctrl+Shift+N: create a fresh session panel and focus it.
    /// This is spatially a new tab/panel, never a clear of the current page.
    SessionNew,
    /// Overview field navigation, while the overview is held open. Spatial
    /// rather than list motion: the field is 2D, so these move to whichever
    /// blob actually lies that way.
    OverviewLeft,
    OverviewRight,
    OverviewUp,
    OverviewDown,
    /// Cycle the field in reading order, for Alt+Tab muscle memory.
    OverviewNext,
    OverviewPrev,
    /// Attach to the highlighted session and close the field.
    OverviewCommit,
    /// Close the field without switching.
    OverviewCancel,

    /// Ctrl+R: open or shut the resume-from-disk picker. The TUI spends the
    /// same chord on session recovery, and this is the desktop's answer to it:
    /// every stored session, grouped by the project it ran in.
    ToggleResume,
    /// Picker navigation, while the resume overlay is up. A list rather than a
    /// field, so these are line and group motion instead of spatial moves.
    ResumeUp,
    ResumeDown,
    /// Collapse the project the highlight is in, or expand it (and step in).
    ResumeCollapse,
    ResumeExpand,
    /// Jump to the previous or next project heading, for a long list.
    ResumeGroupUp,
    ResumeGroupDown,
    /// Attach to the highlighted stored session and close the overlay.
    ResumeCommit,
    /// Close the overlay without switching.
    ResumeCancel,
    /// Narrow the list by the typed character, and undo that.
    ResumeType,
    ResumeBackspace,

    /// Ctrl+Shift+D: flip the window between light and dark without opening
    /// anything. The panel is where a setting is *found*; a palette is the one
    /// setting people change often enough to want it on a key.
    ToggleTheme,

    /// Ctrl+comma: open or shut the settings panel. The chord every desktop
    /// app puts preferences on, so it needs no discovering.
    ToggleSettings,

    /// Ctrl+M: open or shut the model catalog. The caption beside the composer
    /// advertises this chord so switching models becomes a learned keyboard
    /// action rather than a pointer-only control.
    ToggleModelPicker,

    /// Cycle how much of the model's thinking the transcript keeps (`current`
    /// -> `full` -> `off`). This remains available from the settings panel.
    CycleReasoningDisplay,

    /// Ctrl+Shift+R: activate changed application code inside the stable native host.
    /// Deliberately global, so it works even while an overlay owns the keyboard.
    ManualReload,

    /// Ctrl+plus / Ctrl+minus / Ctrl+0: grow, shrink, or reset the UI zoom.
    /// The browser's chords, because "the text is too small" is a browser-
    /// shaped problem and everyone already has the muscle memory.
    ZoomIn,
    ZoomOut,
    ZoomReset,
}

impl Action {
    /// Whether this action can leave a different selection behind.
    ///
    /// Auto-copy keys off this rather than off a diff of the editor state,
    /// because the point is to publish what the user *selected*: republishing
    /// on unrelated actions would mean an arrow key quietly overwrites the
    /// primary selection with the same text over and over.
    pub fn changes_the_selection(self) -> bool {
        matches!(
            self,
            Self::ExtendLeft
                | Self::ExtendRight
                | Self::ExtendWordLeft
                | Self::ExtendWordRight
                | Self::ExtendHome
                | Self::ExtendEnd
                | Self::ExtendLineUp
                | Self::ExtendLineDown
                | Self::ExtendDocStart
                | Self::ExtendDocEnd
                | Self::SelectAll
        )
    }
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
        chord: "f1",
        action: Action::ToggleHelp,
        tui: "/help (Desktop2 overlay)",
    },
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
        action: Action::SelectAll,
        tui: "web: select all (Home keeps start-of-line)",
    },
    Ported {
        chord: "super+a",
        action: Action::SelectAll,
        tui: "web: Cmd+A select all",
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
        action: Action::PromptPrev,
        tui: "previous prompt",
    },
    Ported {
        chord: "ctrl+j",
        action: Action::PromptNext,
        tui: "next prompt",
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
        chord: "ctrl+c",
        action: Action::CopyOrInterrupt,
        tui: "web: copy when selected, else interrupt",
    },
    Ported {
        chord: "super+c",
        action: Action::CopyOrInterrupt,
        tui: "web: Cmd+C copy",
    },
    Ported {
        chord: "ctrl+shift+c",
        action: Action::Copy,
        tui: "copy selection",
    },
    Ported {
        chord: "ctrl+shift+a",
        action: Action::SelectAll,
        tui: "web: select all (alias)",
    },
    Ported {
        chord: "ctrl+shift+d",
        action: Action::ToggleTheme,
        tui: "light/dark theme",
    },
    Ported {
        chord: "ctrl+shift+r",
        action: Action::ManualReload,
        tui: "desktop: manually reload the activated build",
    },
    Ported {
        chord: "ctrl+shift+n",
        action: Action::SessionNew,
        tui: "new session",
    },
    Ported {
        chord: "ctrl+t",
        action: Action::SessionNew,
        tui: "new session panel (browser convention)",
    },
    Ported {
        chord: "super+t",
        action: Action::SessionNew,
        tui: "new session panel (macOS convention)",
    },
    Ported {
        chord: "ctrl+r",
        action: Action::ToggleResume,
        tui: "Ctrl+R resume a stored session",
    },
    Ported {
        chord: "ctrl+,",
        action: Action::ToggleSettings,
        tui: "settings panel",
    },
    Ported {
        chord: "ctrl+shift+z",
        action: Action::Redo,
        tui: "web: redo",
    },
    Ported {
        chord: "ctrl+y",
        action: Action::Redo,
        tui: "web/Windows: redo",
    },
    Ported {
        chord: "ctrl+delete",
        action: Action::DeleteWordForward,
        tui: "web: delete word forward",
    },
    Ported {
        chord: "shift+up",
        action: Action::ExtendLineUp,
        tui: "web: extend selection a line up",
    },
    Ported {
        chord: "shift+down",
        action: Action::ExtendLineDown,
        tui: "web: extend selection a line down",
    },
    Ported {
        chord: "ctrl+shift+home",
        action: Action::ExtendDocStart,
        tui: "web: extend to start of field",
    },
    Ported {
        chord: "ctrl+shift+end",
        action: Action::ExtendDocEnd,
        tui: "web: extend to end of field",
    },
    Ported {
        chord: "ctrl+tab",
        action: Action::SessionRight,
        tui: "web: next tab",
    },
    Ported {
        chord: "ctrl+shift+tab",
        action: Action::SessionLeft,
        tui: "web: previous tab",
    },
    Ported {
        chord: "ctrl+pagedown",
        action: Action::SessionRight,
        tui: "web: next tab",
    },
    Ported {
        chord: "ctrl+pageup",
        action: Action::SessionLeft,
        tui: "web: previous tab",
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
        action: Action::MoveDocStart,
        tui: "web: caret to start of field",
    },
    Ported {
        chord: "ctrl+end",
        action: Action::MoveDocEnd,
        tui: "web: caret to end of field",
    },
    Ported {
        chord: "alt+home",
        action: Action::ScrollTop,
        tui: "scroll transcript to top",
    },
    Ported {
        chord: "alt+end",
        action: Action::ScrollBottom,
        tui: "scroll transcript to bottom",
    },
    // Interrupt / cancel.
    Ported {
        chord: "escape",
        action: Action::Cancel,
        tui: "Esc cancel or clear input",
    },
    Ported {
        chord: "ctrl+d",
        action: Action::InterruptOrQuit,
        tui: "Ctrl+D interrupt or quit",
    },
    // Session strip, in the spirit of niri's window/workspace motion.
    Ported {
        chord: "super+h",
        action: Action::SessionLeft,
        tui: "no TUI equivalent: niri focus-column-left",
    },
    Ported {
        chord: "super+l",
        action: Action::SessionRight,
        tui: "no TUI equivalent: niri focus-column-right",
    },
    Ported {
        chord: "super+k",
        action: Action::SessionUp,
        tui: "no TUI equivalent: niri focus-window-up",
    },
    Ported {
        chord: "super+j",
        action: Action::SessionDown,
        tui: "no TUI equivalent: niri focus-window-down",
    },
    Ported {
        chord: "ctrl+alt+left",
        action: Action::SessionLeft,
        tui: "no TUI equivalent: desktop-only session strip",
    },
    Ported {
        chord: "ctrl+alt+right",
        action: Action::SessionRight,
        tui: "no TUI equivalent: desktop-only session strip",
    },
    Ported {
        chord: "ctrl+alt+up",
        action: Action::SessionUp,
        tui: "no TUI equivalent: desktop-only session strip",
    },
    Ported {
        chord: "ctrl+alt+down",
        action: Action::SessionDown,
        tui: "no TUI equivalent: desktop-only session strip",
    },
    Ported {
        chord: "ctrl+=",
        action: Action::ZoomIn,
        tui: "no TUI equivalent: the terminal owns font size there",
    },
    Ported {
        chord: "ctrl+-",
        action: Action::ZoomOut,
        tui: "no TUI equivalent: the terminal owns font size there",
    },
    Ported {
        chord: "ctrl+0",
        action: Action::ZoomReset,
        tui: "no TUI equivalent: the terminal owns font size there",
    },
];

/// TUI chords deliberately **not** ported, with the reason. Keeps the scope
/// honest: these depend on surfaces desktop2 does not have yet.
pub const NOT_PORTED: &[(&str, &str)] = &[
    (
        "tab",
        "no slash-command autocomplete yet (Ctrl+Tab switches sessions)",
    ),
    ("ctrl+t", "no queue mode yet"),
    ("ctrl+a as start-of-line", "web select-all wins; use Home"),
    ("ctrl+s", "no input stash yet"),
    ("ctrl+p", "no auto-poke yet"),
    ("ctrl+g", "no diagram overlay yet"),
    (
        "ctrl+[ / ctrl+]",
        "prompt-jump aliases are not bound on desktop",
    ),
    ("super+5", "onboarding simulator is a TUI dev aid"),
];

/// Resolve a key press while the overview is held open.
///
/// A separate table rather than more arms in [`resolve`]: the overview owns
/// the whole keyboard while it is up, so the same arrow that moves the text
/// cursor in the composer moves across the field here. Keeping the two
/// resolvers apart is what makes that impossible to get wrong, and lets the
/// overview's bindings be tested without a window.
///
/// `None` means the key does nothing while the field is up. Unbound keys are
/// swallowed rather than typed: text arriving in a composer you cannot see
/// would be worse than a dead key.
pub fn resolve_overview(key: &Key) -> Option<Action> {
    match key {
        Key::Named(named) => match named {
            NamedKey::ArrowLeft => Some(Action::OverviewLeft),
            NamedKey::ArrowRight => Some(Action::OverviewRight),
            NamedKey::ArrowUp => Some(Action::OverviewUp),
            NamedKey::ArrowDown => Some(Action::OverviewDown),
            NamedKey::Tab => Some(Action::OverviewNext),
            NamedKey::Enter | NamedKey::Space => Some(Action::OverviewCommit),
            NamedKey::Escape => Some(Action::OverviewCancel),
            _ => None,
        },
        Key::Character(text) => match text.chars().next()?.to_ascii_lowercase() {
            // vim motion, because the field is a spatial surface and the
            // author's hands are already on the home row.
            'h' => Some(Action::OverviewLeft),
            'l' => Some(Action::OverviewRight),
            'k' => Some(Action::OverviewUp),
            'j' => Some(Action::OverviewDown),
            _ => None,
        },
        _ => None,
    }
}

/// Resolve a key press while the resume picker is up.
///
/// Its own resolver for the same reason the overview has one: the overlay owns
/// the keyboard while it is open, so the letters that would go into the
/// composer narrow the list instead. Keeping the two tables apart is what
/// makes it impossible for a search keystroke to leak into a message.
///
/// `None` means the key does nothing here and is swallowed rather than typed.
pub fn resolve_resume(key: &Key, mods: ModifiersState) -> Option<Action> {
    let ctrl = mods.control_key();
    let sup = mods.super_key();
    let alt = mods.alt_key();
    let cmd = ctrl || sup;
    match key {
        Key::Named(named) => match named {
            NamedKey::ArrowUp => Some(Action::ResumeUp),
            NamedKey::ArrowDown => Some(Action::ResumeDown),
            NamedKey::ArrowLeft => Some(Action::ResumeCollapse),
            NamedKey::ArrowRight => Some(Action::ResumeExpand),
            NamedKey::PageUp => Some(Action::ResumeGroupUp),
            NamedKey::PageDown => Some(Action::ResumeGroupDown),
            // Tab walks the list rather than the field: the overlay is a
            // vertical list, so "next" means the row below.
            NamedKey::Tab if mods.shift_key() => Some(Action::ResumeUp),
            NamedKey::Tab => Some(Action::ResumeDown),
            NamedKey::Enter => Some(Action::ResumeCommit),
            NamedKey::Escape => Some(Action::ResumeCancel),
            NamedKey::Backspace => Some(Action::ResumeBackspace),
            NamedKey::Space if !cmd && !alt => Some(Action::ResumeType),
            _ => None,
        },
        Key::Character(text) => {
            let ch = text.chars().next()?.to_ascii_lowercase();
            // The chord that opened the overlay also shuts it, and Ctrl+C /
            // Ctrl+D keep meaning "get me out of here" rather than typing a
            // letter into the search.
            if cmd && !alt {
                return match ch {
                    'r' => Some(Action::ToggleResume),
                    'c' | 'd' | 'g' => Some(Action::ResumeCancel),
                    'j' | 'n' => Some(Action::ResumeDown),
                    'k' | 'p' => Some(Action::ResumeUp),
                    _ => None,
                };
            }
            // Everything else is search text: a picker you cannot type into is
            // a list you have to scroll through a thousand rows of.
            (!alt).then_some(Action::ResumeType)
        }
        _ => None,
    }
}

/// Resolve a key while the help card owns the keyboard.
///
/// Only its two close chords are live. Returning `None` deliberately swallows
/// everything else so typing cannot edit a composer obscured by a modal card.
pub fn resolve_help(key: &Key) -> Option<Action> {
    matches!(key, Key::Named(NamedKey::Escape | NamedKey::F1)).then_some(Action::ToggleHelp)
}

/// Resolve a key press to an action. `None` means the key is not bound and
/// should fall through to text insertion.
pub fn resolve(key: &Key, mods: ModifiersState) -> Option<Action> {
    let ctrl = mods.control_key();
    let alt = mods.alt_key();
    let sup = mods.super_key();
    let shift = mods.shift_key();
    // Ctrl and Cmd are interchangeable for editing chords: which one arrives
    // depends on the platform, and a Linux user pressing Ctrl+C expects the
    // same thing a macOS user pressing Cmd+C does.
    let cmd = ctrl || sup;

    match key {
        Key::Named(named) => match named {
            NamedKey::F1 => Some(Action::ToggleHelp),
            // Session-strip motion is checked first: the arrow arms below
            // would otherwise swallow it, and chrome navigation has to be
            // reachable from any editor state.
            NamedKey::ArrowLeft if ctrl && alt && shift => Some(Action::PanelShrink),
            NamedKey::ArrowRight if ctrl && alt && shift => Some(Action::PanelGrow),
            NamedKey::ArrowLeft if ctrl && alt => Some(Action::SessionLeft),
            NamedKey::ArrowRight if ctrl && alt => Some(Action::SessionRight),
            NamedKey::ArrowUp if ctrl && alt => Some(Action::SessionUp),
            NamedKey::ArrowDown if ctrl && alt => Some(Action::SessionDown),
            // Ctrl+Tab / Ctrl+PageUp / Ctrl+PageDown are the browser's tab
            // chords, and sessions are this app's tabs.
            NamedKey::Tab if cmd => Some(if shift {
                Action::SessionLeft
            } else {
                Action::SessionRight
            }),
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
                if ctrl {
                    // Web/Windows: Ctrl+Delete deletes the word *ahead*.
                    Some(Action::DeleteWordForward)
                } else if alt || sup {
                    // Option+Delete is word-delete-back on macOS keyboards.
                    Some(Action::DeleteWordBack)
                } else {
                    Some(Action::DeleteForward)
                }
            }
            NamedKey::ArrowLeft => Some(match (shift, ctrl || alt, sup) {
                (true, true, _) => Action::ExtendWordLeft,
                (true, false, _) => Action::ExtendLeft,
                (false, _, true) => Action::MoveHome,
                (false, true, _) => Action::MoveWordLeft,
                (false, false, false) => Action::MoveLeft,
            }),
            NamedKey::ArrowRight => Some(match (shift, ctrl || alt, sup) {
                (true, true, _) => Action::ExtendWordRight,
                (true, false, _) => Action::ExtendRight,
                (false, _, true) => Action::MoveEnd,
                (false, true, _) => Action::MoveWordRight,
                (false, false, false) => Action::MoveRight,
            }),
            // Shift+Up/Down extends by line, exactly like a textarea. Without
            // shift the caret walks lines and falls through to history recall
            // at the edges (handled by the app).
            NamedKey::ArrowUp => Some(if shift {
                Action::ExtendLineUp
            } else {
                Action::HistoryPrev
            }),
            NamedKey::ArrowDown => Some(if shift {
                Action::ExtendLineDown
            } else {
                Action::HistoryNext
            }),
            // Home/End behave as they do in a focused text field: line edges,
            // and with Ctrl the whole field. Transcript scroll-to-edge lives on
            // the Alt chords, since the field owns the Ctrl ones.
            NamedKey::Home => Some(match (shift, cmd, alt) {
                (_, _, true) => Action::ScrollTop,
                (true, true, _) => Action::ExtendDocStart,
                (true, false, _) => Action::ExtendHome,
                (false, true, _) => Action::MoveDocStart,
                (false, false, _) => Action::MoveHome,
            }),
            NamedKey::End => Some(match (shift, cmd, alt) {
                (_, _, true) => Action::ScrollBottom,
                (true, true, _) => Action::ExtendDocEnd,
                (true, false, _) => Action::ExtendEnd,
                (false, true, _) => Action::MoveDocEnd,
                (false, false, _) => Action::MoveEnd,
            }),
            NamedKey::PageUp => Some(if cmd {
                Action::SessionLeft
            } else {
                Action::PageUp
            }),
            NamedKey::PageDown => Some(if cmd {
                Action::SessionRight
            } else {
                Action::PageDown
            }),
            _ => None,
        },
        Key::Character(text) => {
            let ch = text.chars().next().map(|c| c.to_ascii_lowercase())?;
            // Zoom first: the chord is Ctrl plus a punctuation key whose glyph
            // depends on the layout and on whether Shift is held (`+` vs `=`,
            // `_` vs `-`), so every spelling is accepted rather than only the
            // unshifted one. Checked before the shifted and Alt blocks so
            // Ctrl+Shift+= cannot be swallowed on the way past.
            if cmd && !alt {
                match ch {
                    // Preferences, on the chord the whole desktop uses. Both
                    // spellings, because a shifted comma is `<` on most
                    // layouts and the user's finger does not know that.
                    ',' | '<' => return Some(Action::ToggleSettings),
                    '+' | '=' => return Some(Action::ZoomIn),
                    '-' | '_' => return Some(Action::ZoomOut),
                    '0' => return Some(Action::ZoomReset),
                    // New-tab muscle memory maps directly to a new spatial
                    // session panel. Shift remains free for reopen-closed-tab.
                    't' if !shift => return Some(Action::SessionNew),
                    _ => {}
                }
            }
            if (ctrl || sup || alt) && shift {
                match ch {
                    // Ctrl+Shift+Z is redo everywhere on the web.
                    'z' => return Some(Action::Redo),
                    // Shifted J/K with a nav modifier scroll by line, matching
                    // the TUI's ScrollKeys fallback.
                    'k' => return Some(Action::ScrollUp),
                    'j' => return Some(Action::ScrollDown),
                    'c' => return Some(Action::Copy),
                    'a' => return Some(Action::SelectAll),
                    // Ctrl+Shift+D: light/dark. Shifted so it cannot collide
                    // with a plain Ctrl+D, which is interrupt-or-quit.
                    'd' => return Some(Action::ToggleTheme),
                    // Ctrl+Shift+R: gracefully adopt the activated desktop
                    // build. Plain Ctrl+R remains session recovery.
                    'r' => return Some(Action::ManualReload),
                    // Ctrl+Shift+N: a new session. Shifted so it cannot be hit
                    // by a plain Ctrl+N reflex while typing, and matching the
                    // "new window" chord rather than "new tab": a session is a
                    // whole place, not a view of this one.
                    'n' => return Some(Action::SessionNew),
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
            // Super+HJKL walks the session strip: niri's focus motion,
            // verbatim, because that is the muscle memory this app lives
            // inside. Checked before the cmd block so Super+K means "session
            // up" here while Ctrl+K keeps its emacs kill-to-end.
            if sup && !ctrl && !alt && !shift {
                match ch {
                    'h' => return Some(Action::SessionLeft),
                    'l' => return Some(Action::SessionRight),
                    'k' => return Some(Action::SessionUp),
                    'j' => return Some(Action::SessionDown),
                    _ => {}
                }
            }
            // Prompt navigation is deliberately Control-only. Cmd+K keeps its
            // text-field meaning on macOS, while the TUI's Ctrl+J/K muscle
            // memory walks whole turns instead of shaving a few text rows.
            if ctrl && !sup && !alt && !shift {
                match ch {
                    'k' => return Some(Action::PromptPrev),
                    'j' => return Some(Action::PromptNext),
                    _ => {}
                }
            }
            if cmd {
                return match ch {
                    // Web first: select all, copy, cut, paste, undo, redo.
                    'a' => Some(Action::SelectAll),
                    'c' => Some(Action::CopyOrInterrupt),
                    'x' => Some(Action::CutLine),
                    'v' => Some(Action::Paste),
                    'z' => Some(Action::Undo),
                    'y' => Some(Action::Redo),
                    // Emacs chords that do not collide with the web set.
                    'e' => Some(Action::MoveEnd),
                    'b' => Some(Action::MoveWordLeft),
                    'f' => Some(Action::MoveWordRight),
                    'u' => Some(Action::KillToStart),
                    'k' => Some(Action::KillToEnd),
                    'w' => Some(Action::DeleteWordBack),
                    // Ctrl+R: the stored-session picker. The TUI's recovery
                    // chord, on the desktop's overlay: it opens over the
                    // conversation rather than replacing it, so the session
                    // you are in stays legible while you pick another.
                    'r' => Some(Action::ToggleResume),
                    'm' => Some(Action::ToggleModelPicker),
                    'd' => Some(Action::InterruptOrQuit),
                    _ => None,
                };
            }
            None
        }
        _ => None,
    }
}

/// Parse a chord like `"ctrl+shift+k"` into a key and modifier state. Shared
/// by the parity-table tests and `--script`, so scripted verification exercises
/// the same spelling the table documents.
pub fn parse_chord(chord: &str) -> Option<(Key, ModifiersState)> {
    let mut mods = ModifiersState::empty();
    let mut key = None;
    for part in chord.split('+') {
        match part {
            "ctrl" => mods |= ModifiersState::CONTROL,
            "alt" => mods |= ModifiersState::ALT,
            "shift" => mods |= ModifiersState::SHIFT,
            "super" | "cmd" => mods |= ModifiersState::SUPER,
            name => {
                key = Some(match name {
                    "enter" => Key::Named(NamedKey::Enter),
                    "escape" | "esc" => Key::Named(NamedKey::Escape),
                    "backspace" => Key::Named(NamedKey::Backspace),
                    "delete" | "del" => Key::Named(NamedKey::Delete),
                    "left" => Key::Named(NamedKey::ArrowLeft),
                    "right" => Key::Named(NamedKey::ArrowRight),
                    "up" => Key::Named(NamedKey::ArrowUp),
                    "down" => Key::Named(NamedKey::ArrowDown),
                    "home" => Key::Named(NamedKey::Home),
                    "end" => Key::Named(NamedKey::End),
                    "pageup" => Key::Named(NamedKey::PageUp),
                    "pagedown" => Key::Named(NamedKey::PageDown),
                    "tab" => Key::Named(NamedKey::Tab),
                    "f1" => Key::Named(NamedKey::F1),
                    other if other.chars().count() == 1 => {
                        Key::Character(winit::keyboard::SmolStr::new(other))
                    }
                    _ => return None,
                });
            }
        }
    }
    Some((key?, mods))
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::SmolStr;

    fn parse(chord: &str) -> (Key, ModifiersState) {
        parse_chord(chord).unwrap_or_else(|| panic!("unparsable chord '{chord}'"))
    }

    #[test]
    fn unknown_chords_are_rejected_rather_than_guessed() {
        assert!(parse_chord("ctrl+nope").is_none());
        assert!(parse_chord("ctrl").is_none(), "a chord needs a key");
    }

    #[test]
    fn resume_picker_supports_ctrl_j_and_k_navigation() {
        for (chord, action) in [("ctrl+j", Action::ResumeDown), ("ctrl+k", Action::ResumeUp)] {
            let (key, mods) = parse(chord);
            assert_eq!(resolve_resume(&key, mods), Some(action), "'{chord}'");
        }
    }

    /// R7: chrome navigation must never cost the user their text motion.
    /// Bare and Alt arrows stay in the editor; only Ctrl+Shift leaves it.
    #[test]
    fn bare_arrows_still_edit_text() {
        for (chord, action) in [
            ("left", Action::MoveLeft),
            ("right", Action::MoveRight),
            ("up", Action::HistoryPrev),
            ("down", Action::HistoryNext),
            ("alt+left", Action::MoveWordLeft),
            ("alt+right", Action::MoveWordRight),
            ("shift+left", Action::ExtendLeft),
            ("shift+right", Action::ExtendRight),
            ("ctrl+left", Action::MoveWordLeft),
            ("ctrl+right", Action::MoveWordRight),
            ("ctrl+shift+left", Action::ExtendWordLeft),
            ("ctrl+shift+right", Action::ExtendWordRight),
        ] {
            let (key, mods) = parse(chord);
            assert_eq!(
                resolve(&key, mods),
                Some(action),
                "'{chord}' was stolen by the session strip"
            );
        }
    }

    /// R7: and the strip chords really do reach the strip.
    #[test]
    fn ctrl_alt_arrows_resolve_to_session_actions() {
        for (chord, action) in [
            ("ctrl+alt+left", Action::SessionLeft),
            ("ctrl+alt+right", Action::SessionRight),
            ("ctrl+alt+up", Action::SessionUp),
            ("ctrl+alt+down", Action::SessionDown),
        ] {
            let (key, mods) = parse(chord);
            assert_eq!(resolve(&key, mods), Some(action), "'{chord}' did not bind");
        }
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
        // 'k' is deliberately absent: Super+K is session-up (niri motion)
        // while Ctrl+K keeps its emacs kill-to-end.
        for ch in ['a', 'e', 'u', 'w', 'x', 'z', 'v', 'c'] {
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

    /// Shift+motion must extend a selection rather than move the caret, and
    /// the unshifted chord must still move.
    #[test]
    fn shift_motion_extends_instead_of_moving() {
        let cases = [
            (NamedKey::ArrowLeft, Action::MoveLeft, Action::ExtendLeft),
            (NamedKey::ArrowRight, Action::MoveRight, Action::ExtendRight),
            (NamedKey::Home, Action::MoveHome, Action::ExtendHome),
            (NamedKey::End, Action::MoveEnd, Action::ExtendEnd),
        ];
        for (key, plain, extended) in cases {
            assert_eq!(
                resolve(&Key::Named(key), ModifiersState::empty()),
                Some(plain),
                "{key:?} without shift should move"
            );
            assert_eq!(
                resolve(&Key::Named(key), ModifiersState::SHIFT),
                Some(extended),
                "{key:?} with shift should extend the selection"
            );
        }
    }

    #[test]
    fn ctrl_shift_arrows_extend_by_word() {
        assert_eq!(
            resolve(
                &Key::Named(NamedKey::ArrowLeft),
                ModifiersState::CONTROL | ModifiersState::SHIFT
            ),
            Some(Action::ExtendWordLeft)
        );
        assert_eq!(
            resolve(
                &Key::Named(NamedKey::ArrowRight),
                ModifiersState::CONTROL | ModifiersState::SHIFT
            ),
            Some(Action::ExtendWordRight)
        );
    }

    /// Ctrl+A is select-all, as in every browser and GUI text field, and
    /// start-of-line stays reachable on Home. Reverting this would make the
    /// most reflexive shortcut on the platform silently do something else.
    #[test]
    fn ctrl_a_selects_all_like_the_web() {
        let a = Key::Character(SmolStr::new("a"));
        for mods in [
            ModifiersState::CONTROL,
            ModifiersState::SUPER,
            ModifiersState::CONTROL | ModifiersState::SHIFT,
        ] {
            assert_eq!(
                resolve(&a, mods),
                Some(Action::SelectAll),
                "{mods:?}+A is not select-all"
            );
        }
        assert_eq!(
            resolve(&Key::Named(NamedKey::Home), ModifiersState::empty()),
            Some(Action::MoveHome),
            "start-of-line must stay reachable on Home"
        );
    }

    /// The web clipboard/undo set must be exactly what a browser does.
    #[test]
    fn the_web_clipboard_and_undo_set_matches_a_browser() {
        for (ch, action) in [
            ('a', Action::SelectAll),
            ('c', Action::CopyOrInterrupt),
            ('x', Action::CutLine),
            ('v', Action::Paste),
            ('z', Action::Undo),
        ] {
            let key = Key::Character(SmolStr::new(ch.to_string()));
            for mods in [ModifiersState::CONTROL, ModifiersState::SUPER] {
                assert_eq!(
                    resolve(&key, mods),
                    Some(action),
                    "{mods:?}+{ch} diverged from the web binding"
                );
            }
        }
        let z = Key::Character(SmolStr::new("z"));
        assert_eq!(
            resolve(&z, ModifiersState::CONTROL | ModifiersState::SHIFT),
            Some(Action::Redo),
            "Ctrl+Shift+Z is redo on the web"
        );
        assert_eq!(
            resolve(&Key::Character(SmolStr::new("y")), ModifiersState::CONTROL),
            Some(Action::Redo),
            "Ctrl+Y is redo on Windows/web"
        );
    }

    /// Ctrl+Tab and Ctrl+PageUp/PageDown are the browser's tab chords, and
    /// sessions are this app's tabs.
    #[test]
    fn browser_tab_chords_walk_the_session_strip() {
        for (chord, action) in [
            ("ctrl+tab", Action::SessionRight),
            ("ctrl+shift+tab", Action::SessionLeft),
            ("ctrl+pagedown", Action::SessionRight),
            ("ctrl+pageup", Action::SessionLeft),
        ] {
            let (key, mods) = parse(chord);
            assert_eq!(resolve(&key, mods), Some(action), "'{chord}' did not bind");
        }
        // Bare PageUp/PageDown must still scroll the transcript.
        assert_eq!(
            resolve(&Key::Named(NamedKey::PageUp), ModifiersState::empty()),
            Some(Action::PageUp)
        );
        assert_eq!(
            resolve(&Key::Named(NamedKey::PageDown), ModifiersState::empty()),
            Some(Action::PageDown)
        );
    }

    /// Shift+arrows extend, Ctrl+Home/End move within the field, and only the
    /// Alt chords reach the transcript, matching a focused browser textarea.
    #[test]
    fn field_navigation_matches_a_browser_textarea() {
        for (chord, action) in [
            ("shift+up", Action::ExtendLineUp),
            ("shift+down", Action::ExtendLineDown),
            ("ctrl+home", Action::MoveDocStart),
            ("ctrl+end", Action::MoveDocEnd),
            ("ctrl+shift+home", Action::ExtendDocStart),
            ("ctrl+shift+end", Action::ExtendDocEnd),
            ("alt+home", Action::ScrollTop),
            ("alt+end", Action::ScrollBottom),
        ] {
            let (key, mods) = parse(chord);
            assert_eq!(resolve(&key, mods), Some(action), "'{chord}' did not bind");
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
