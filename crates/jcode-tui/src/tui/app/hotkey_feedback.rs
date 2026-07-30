//! Inline hotkey feedback.
//!
//! Two complementary behaviors, both gated behind `display.keybinding_hints`:
//!
//! 1. **Rare-hotkey feedback.** When the user presses a chord jcode recognizes
//!    but they use rarely (or have not used in a long time), we surface a short
//!    inline note: `⌨ Ctrl+G → toggle scroll bookmark`. Per-action usage counts
//!    persist across sessions, so the notes stop once an action is familiar.
//! 2. **Near-miss suggestions.** When a modified chord falls through every
//!    dispatcher unhandled, we tell the user instead of silently swallowing it:
//!    `⌨ Ctrl+Shift+P isn't bound · nearest: Ctrl+P → toggle auto-poke`.
//!
//! The registry mirrors the real dispatch tables (configured bindings first,
//! then built-in readline/navigation chords). Matching, familiarity, and the
//! suggestion ranking are pure functions with unit tests; only persistence and
//! the `App` glue touch the environment.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyModifiers};
use jcode_tui_core::keybind::format_binding;
use serde::{Deserialize, Serialize};

use super::App;
use crate::tui::keybind::{
    CenteredToggleKeys, EffortSwitchKeys, KeyBinding, ModelSwitchKeys, OptionalBinding, ScrollKeys,
    ToggleKeys, WorkspaceNavigationKeys,
};

/// An action is "familiar" once used this many times via its hotkey.
const FAMILIAR_USES: u32 = 4;
/// A familiar action becomes worth one reminder after this much disuse.
const STALE_SECS: u64 = 45 * 24 * 60 * 60;
/// Minimum gap between two unknown-chord notices.
const UNKNOWN_NOTICE_MIN_GAP_MS: u128 = 1200;
/// Max times one unknown chord is called out per session.
const UNKNOWN_NOTICE_MAX_PER_CHORD: u32 = 3;

const STATE_FILE: &str = "hotkey_usage.json";

/// A chord jcode responds to, with a human description of what it does.
#[derive(Debug, Clone)]
pub(super) struct KnownHotkey {
    pub binding: KeyBinding,
    /// Stable persistence id for familiarity tracking.
    pub action: &'static str,
    /// Imperative phrase, e.g. "toggle scroll bookmark".
    pub description: &'static str,
    /// Quiet chords never surface rare-use feedback (interrupt, paste, ...).
    pub quiet: bool,
}

impl KnownHotkey {
    fn new(binding: KeyBinding, action: &'static str, description: &'static str) -> Self {
        Self {
            binding,
            action,
            description,
            quiet: false,
        }
    }

    fn quiet(binding: KeyBinding, action: &'static str, description: &'static str) -> Self {
        Self {
            binding,
            action,
            description,
            quiet: true,
        }
    }

    pub fn label(&self) -> String {
        format_binding(&self.binding)
    }
}

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyBinding {
    KeyBinding { code, modifiers }
}

fn ctrl(c: char) -> KeyBinding {
    key(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn alt(c: char) -> KeyBinding {
    key(KeyCode::Char(c), KeyModifiers::ALT)
}

/// Everything needed to enumerate the configured bindings, decoupled from
/// `App` so the registry is constructible in tests.
pub(super) struct RegistryInputs<'a> {
    pub model_switch: &'a ModelSwitchKeys,
    pub effort: &'a EffortSwitchKeys,
    pub scroll: &'a ScrollKeys,
    pub centered: &'a CenteredToggleKeys,
    pub toggles: &'a ToggleKeys,
    pub workspace: &'a WorkspaceNavigationKeys,
    pub dictation: &'a OptionalBinding,
    pub new_terminal: &'a OptionalBinding,
    pub open_resume: &'a OptionalBinding,
    pub fallback_switch: &'a OptionalBinding,
    /// Workspace navigation only dispatches in remote/client mode.
    pub remote: bool,
}

/// Enumerate the known hotkeys in rough dispatch order (configured bindings
/// first, then built-ins), so `lookup` reports what would actually run.
pub(super) fn build_registry(inputs: &RegistryInputs<'_>) -> Vec<KnownHotkey> {
    let mut out: Vec<KnownHotkey> = Vec::with_capacity(64);

    let mut push = |binding: Option<KeyBinding>, action: &'static str, desc: &'static str| {
        if let Some(binding) = binding {
            out.push(KnownHotkey::new(binding, action, desc));
        }
    };

    // Configured pane/mode toggles (pre-control shortcuts).
    push(
        inputs.toggles.copy_selection.binding().cloned(),
        "copy_selection_toggle",
        "toggle copy/selection mode",
    );
    push(
        inputs.toggles.side_panel.binding().cloned(),
        "side_panel_toggle",
        "toggle the side panel",
    );
    push(
        inputs.toggles.diagram_pane.binding().cloned(),
        "diagram_pane_toggle",
        "toggle the diagram pane",
    );
    push(
        inputs.toggles.typing_scroll_lock.binding().cloned(),
        "typing_scroll_lock_toggle",
        "toggle typing scroll lock",
    );
    push(
        inputs.toggles.diff_mode_cycle.binding().cloned(),
        "diff_mode_cycle",
        "cycle the diff display mode",
    );
    push(
        inputs.toggles.info_widget.binding().cloned(),
        "info_widget_toggle",
        "toggle the info widget",
    );
    push(
        inputs.toggles.todo_card.binding().cloned(),
        "todo_card_toggle",
        "show/dismiss the inline todo card",
    );
    push(
        inputs.toggles.swarm_panel_focus.binding().cloned(),
        "swarm_panel_focus",
        "focus the swarm panel",
    );
    push(
        inputs.dictation.binding.clone(),
        "dictation",
        "start or stop dictation",
    );
    push(
        inputs.new_terminal.binding.clone(),
        "new_terminal",
        "open a fresh session in a new terminal",
    );
    push(
        inputs.open_resume.binding.clone(),
        "open_resume",
        "open the session picker",
    );
    // Context-armed accept key (fallback offer / update merge). Quiet: it only
    // acts when an offer is on screen, which already explains itself.
    // Pushed directly (not via `push`), so re-create the closure afterwards to
    // keep the borrow checker happy about the interleaved direct `out` access.
    if let Some(binding) = inputs.fallback_switch.binding.clone() {
        out.push(KnownHotkey::quiet(
            binding,
            "fallback_switch",
            "accept the on-screen fallback/merge offer",
        ));
    }
    let mut push = |binding: Option<KeyBinding>, action: &'static str, desc: &'static str| {
        if let Some(binding) = binding {
            out.push(KnownHotkey::new(binding, action, desc));
        }
    };
    push(
        Some(inputs.model_switch.next.clone()),
        "model_switch_next",
        "switch to the next model",
    );
    push(
        inputs.model_switch.prev.clone(),
        "model_switch_prev",
        "switch to the previous model",
    );
    push(
        Some(inputs.effort.increase.clone()),
        "effort_increase",
        "raise reasoning effort",
    );
    push(
        Some(inputs.effort.decrease.clone()),
        "effort_decrease",
        "lower reasoning effort",
    );
    push(
        inputs.centered.toggle.clone(),
        "centered_toggle",
        "toggle centered layout",
    );

    if inputs.remote {
        for binding in &inputs.workspace.left {
            push(
                Some(binding.clone()),
                "workspace_left",
                "focus the workspace to the left",
            );
        }
        for binding in &inputs.workspace.down {
            push(
                Some(binding.clone()),
                "workspace_down",
                "focus the workspace below",
            );
        }
        for binding in &inputs.workspace.up {
            push(
                Some(binding.clone()),
                "workspace_up",
                "focus the workspace above",
            );
        }
        for binding in &inputs.workspace.right {
            push(
                Some(binding.clone()),
                "workspace_right",
                "focus the workspace to the right",
            );
        }
    }

    // Configured scroll / prompt navigation.
    push(
        Some(inputs.scroll.up.clone()),
        "scroll_up",
        "scroll the chat up",
    );
    push(
        Some(inputs.scroll.down.clone()),
        "scroll_down",
        "scroll the chat down",
    );
    push(
        inputs.scroll.up_fallback.clone(),
        "scroll_up",
        "scroll the chat up",
    );
    push(
        inputs.scroll.down_fallback.clone(),
        "scroll_down",
        "scroll the chat down",
    );
    push(
        Some(inputs.scroll.page_up.clone()),
        "scroll_page_up",
        "scroll the chat up a page",
    );
    push(
        Some(inputs.scroll.page_down.clone()),
        "scroll_page_down",
        "scroll the chat down a page",
    );
    push(
        Some(inputs.scroll.prompt_up.clone()),
        "prompt_jump_up",
        "jump to the previous prompt",
    );
    push(
        Some(inputs.scroll.prompt_down.clone()),
        "prompt_jump_down",
        "jump to the next prompt",
    );
    push(
        Some(inputs.scroll.bookmark.clone()),
        "scroll_bookmark",
        "toggle the scroll bookmark",
    );
    out.push(KnownHotkey::new(
        ctrl('l'),
        "clear_view",
        "clear the view, keeping context (/cls)",
    ));

    // Built-in readline-style editing chords.
    out.push(KnownHotkey::new(
        ctrl('a'),
        "input_home",
        "jump to the start of the input",
    ));
    out.push(KnownHotkey::new(
        ctrl('e'),
        "input_end",
        "jump to the end of the input",
    ));
    out.push(KnownHotkey::new(
        ctrl('b'),
        "word_back",
        "move back one word",
    ));
    out.push(KnownHotkey::new(
        ctrl('f'),
        "word_forward",
        "move forward one word",
    ));
    out.push(KnownHotkey::new(
        ctrl('u'),
        "kill_to_start",
        "delete to the start of the input",
    ));
    out.push(KnownHotkey::new(
        ctrl('w'),
        "delete_word_back",
        "delete the previous word",
    ));
    out.push(KnownHotkey::new(ctrl('z'), "input_undo", "undo input edit"));
    out.push(KnownHotkey::new(
        ctrl('x'),
        "cut_input_line",
        "cut the input line to the clipboard",
    ));
    out.push(KnownHotkey::new(
        ctrl('s'),
        "input_stash",
        "stash or restore the input draft",
    ));
    out.push(KnownHotkey::new(
        ctrl('p'),
        "auto_poke_toggle",
        "toggle auto-poke",
    ));
    out.push(KnownHotkey::new(
        ctrl('t'),
        "queue_mode_toggle",
        "toggle queue mode",
    ));
    out.push(KnownHotkey::quiet(
        ctrl('v'),
        "paste",
        "paste from the clipboard",
    ));
    out.push(KnownHotkey::quiet(
        ctrl('c'),
        "interrupt",
        "interrupt (or quit when idle)",
    ));
    out.push(KnownHotkey::quiet(
        ctrl('d'),
        "interrupt",
        "interrupt (or quit when idle)",
    ));
    out.push(KnownHotkey::new(
        ctrl('r'),
        "history_search",
        "search prompt history across sessions",
    ));
    out.push(KnownHotkey::new(
        key(KeyCode::Enter, KeyModifiers::CONTROL),
        "alternate_enter",
        "send now, bypassing queue mode",
    ));
    out.push(KnownHotkey::new(
        key(KeyCode::Enter, KeyModifiers::SUPER),
        "alternate_enter",
        "send now, bypassing queue mode",
    ));
    out.push(KnownHotkey::quiet(
        key(KeyCode::Enter, KeyModifiers::SHIFT),
        "newline",
        "insert a newline",
    ));
    out.push(KnownHotkey::quiet(
        key(KeyCode::Enter, KeyModifiers::ALT),
        "newline",
        "insert a newline",
    ));
    out.push(KnownHotkey::new(
        key(KeyCode::Up, KeyModifiers::CONTROL),
        "prompt_recall",
        "recall queued prompt / prompt history",
    ));
    out.push(KnownHotkey::quiet(
        key(KeyCode::Down, KeyModifiers::CONTROL),
        "prompt_recall",
        "walk prompt history forward",
    ));
    out.push(KnownHotkey::new(
        // Registered as Shift+Tab: `matches` normalizes BackTab to the same
        // chord, and the label formats as "Shift+Tab" instead of "Key".
        key(KeyCode::Tab, KeyModifiers::SHIFT),
        "model_favorite_cycle",
        "cycle favorite models",
    ));
    out.push(KnownHotkey::new(
        key(KeyCode::Char(' '), KeyModifiers::ALT),
        "route_new_session",
        "route the next prompt to a new session",
    ));
    out.push(KnownHotkey::new(
        key(KeyCode::Char(' '), KeyModifiers::SUPER),
        "route_new_session",
        "route the next prompt to a new session",
    ));
    for c in ['1', '2', '3', '4'] {
        out.push(KnownHotkey::new(
            ctrl(c),
            "side_panel_ratio",
            "resize the side panel",
        ));
    }
    for c in ['5', '6', '7', '8', '9'] {
        out.push(KnownHotkey::new(
            ctrl(c),
            "prompt_rank_jump",
            "jump to a recent prompt",
        ));
    }
    out.push(KnownHotkey::new(
        alt('b'),
        "word_back",
        "move back one word",
    ));
    out.push(KnownHotkey::new(
        alt('f'),
        "word_forward",
        "move forward one word",
    ));
    out.push(KnownHotkey::new(
        alt('d'),
        "delete_word_forward",
        "delete the next word",
    ));
    out.push(KnownHotkey::new(
        key(KeyCode::Backspace, KeyModifiers::ALT),
        "delete_word_back",
        "delete the previous word",
    ));

    out
}

/// Context-sensitive chords that shadow the registry, mirroring the dispatch
/// special cases in `handle_pre_control_shortcuts` / `handle_control_key`.
fn contextual_lookup(
    input_empty: bool,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Option<KnownHotkey> {
    let only_ctrl = modifiers == KeyModifiers::CONTROL;
    match code {
        // Plain Ctrl+K kills to end of line while a draft exists; the
        // prompt-jump binding only wins on an empty input.
        KeyCode::Char('k') if only_ctrl && !input_empty => Some(KnownHotkey::new(
            ctrl('k'),
            "kill_to_end",
            "delete to the end of the input",
        )),
        // Ctrl+A / Alt+A copy the visible chat when the input is empty.
        KeyCode::Char('a') if only_ctrl && input_empty => Some(KnownHotkey::new(
            ctrl('a'),
            "copy_viewport",
            "copy the visible chat to the clipboard",
        )),
        KeyCode::Char('a') if modifiers == KeyModifiers::ALT && input_empty => {
            Some(KnownHotkey::new(
                alt('a'),
                "copy_viewport",
                "copy the visible chat to the clipboard",
            ))
        }
        _ => None,
    }
}

/// Resolve what a chord does, or `None` when jcode has no binding for it.
pub(super) fn lookup(
    registry: &[KnownHotkey],
    input_empty: bool,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Option<KnownHotkey> {
    if let Some(info) = contextual_lookup(input_empty, code, modifiers) {
        return Some(info);
    }
    registry
        .iter()
        .find(|entry| entry.binding.matches(code, modifiers))
        .cloned()
}

fn modifier_distance(a: KeyModifiers, b: KeyModifiers) -> u32 {
    (a.bits() ^ b.bits()).count_ones()
}

/// Rank the closest known hotkey to an unbound chord, or `None` when nothing
/// is plausibly related. "Close" means same key with nearly the same
/// modifiers, or the same letter under different modifiers.
pub(super) fn nearest_hotkey(
    registry: &[KnownHotkey],
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Option<KnownHotkey> {
    let mut best: Option<(u32, &KnownHotkey)> = None;
    for entry in registry {
        let score = if entry.binding.code == code {
            match modifier_distance(entry.binding.modifiers, modifiers) {
                0 => continue, // would have matched exactly
                1 => 9,
                2 => 7,
                _ => continue,
            }
        } else if let (KeyCode::Char(a), KeyCode::Char(b)) = (entry.binding.code, code) {
            if a.eq_ignore_ascii_case(&b) {
                match modifier_distance(entry.binding.modifiers, modifiers) {
                    0 => 8,
                    1 => 6,
                    _ => continue,
                }
            } else {
                continue;
            }
        } else {
            continue;
        };
        match best {
            Some((best_score, _)) if best_score >= score => {}
            _ => best = Some((score, entry)),
        }
    }
    best.map(|(_, entry)| entry.clone())
}

/// Message for an unbound chord, with the nearest suggestion when one exists.
pub(super) fn unknown_chord_message(
    registry: &[KnownHotkey],
    code: KeyCode,
    modifiers: KeyModifiers,
) -> String {
    let chord = format_binding(&KeyBinding { code, modifiers });
    match nearest_hotkey(registry, code, modifiers) {
        Some(near) => format!(
            "⌨ {} isn't bound · nearest: {} → {}",
            chord,
            near.label(),
            near.description
        ),
        None => format!("⌨ {} isn't bound · /help lists shortcuts", chord),
    }
}

/// Per-action persisted usage counters.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct UsageStat {
    #[serde(default)]
    uses: u32,
    #[serde(default)]
    last_used_unix: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct HotkeyUsageState {
    #[serde(default)]
    version: u8,
    #[serde(default)]
    actions: HashMap<String, UsageStat>,
}

/// Whether a use of this action deserves the "you just pressed X" note.
fn is_unfamiliar(stat: &UsageStat, now_unix: u64) -> bool {
    stat.uses < FAMILIAR_USES || now_unix.saturating_sub(stat.last_used_unix) >= STALE_SECS
}

fn state_path() -> Option<std::path::PathBuf> {
    crate::storage::app_config_dir()
        .ok()
        .map(|dir| dir.join(STATE_FILE))
}

fn load_state() -> HotkeyUsageState {
    let Some(path) = state_path() else {
        return HotkeyUsageState::default();
    };
    crate::storage::read_json::<HotkeyUsageState>(&path).unwrap_or_default()
}

fn save_state(state: &HotkeyUsageState) {
    let Some(path) = state_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = crate::storage::write_json(&path, state) {
        crate::logging::info(&format!(
            "Failed to persist hotkey usage state {}: {}",
            path.display(),
            error
        ));
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Render the `/hotkeys` listing: every known chord with its action and the
/// user's personal usage level. Pure over its inputs for testability.
pub(super) fn render_hotkeys_listing(
    registry: &[KnownHotkey],
    usage: &HotkeyUsageState,
    now_unix: u64,
) -> String {
    // Group rows by action so multi-chord actions (workspace nav aliases,
    // scroll fallbacks) render one line with all chords.
    let mut rows: Vec<(String, &'static str, &'static str)> = Vec::new();
    let mut seen: std::collections::HashMap<&'static str, usize> = std::collections::HashMap::new();
    for entry in registry {
        match seen.get(entry.action) {
            Some(&idx) => {
                let labels = &mut rows[idx].0;
                let label = entry.label();
                if !labels.split(", ").any(|existing| existing == label) {
                    labels.push_str(", ");
                    labels.push_str(&label);
                }
            }
            None => {
                seen.insert(entry.action, rows.len());
                rows.push((entry.label(), entry.action, entry.description));
            }
        }
    }

    let mut used: Vec<String> = Vec::new();
    let mut unused: Vec<String> = Vec::new();
    for (labels, action, description) in rows {
        let stat = usage.actions.get(action);
        let uses = stat.map(|s| s.uses).unwrap_or(0);
        let line = format!("- `{}` → {}", labels, description);
        if uses == 0 {
            unused.push(line);
        } else {
            let freshness = stat
                .filter(|s| now_unix.saturating_sub(s.last_used_unix) >= STALE_SECS)
                .map(|_| ", not recently")
                .unwrap_or("");
            used.push(format!(
                "{} ({} use{}{})",
                line,
                uses,
                if uses == 1 { "" } else { "s" },
                freshness
            ));
        }
    }

    let mut out = String::from("## Hotkeys\n");
    if !unused.is_empty() {
        out.push_str("\n**Not yet used** (try these):\n");
        out.push_str(&unused.join("\n"));
        out.push('\n');
    }
    if !used.is_empty() {
        out.push_str("\n**In your muscle memory:**\n");
        out.push_str(&used.join("\n"));
        out.push('\n');
    }
    out.push_str("\nRebind under `[keybindings]` in config. Full reference: /help\n");
    out
}

impl App {
    /// Handle the `/hotkeys` command: list every known chord with a
    /// description and the user's personal usage counts.
    pub(super) fn handle_hotkeys_command(&mut self, trimmed: &str) -> bool {
        // `/keys` belongs to the keymap-conflict diagnostics command, which is
        // dispatched earlier; claiming it here was dead code that only made the
        // alias ambiguous.
        if trimmed != "/hotkeys" {
            return false;
        }
        let registry = self.hotkey_registry(self.is_remote);
        let usage = self.hotkey_usage.get_or_insert_with(load_state);
        let listing = render_hotkeys_listing(&registry, usage, now_unix());
        self.push_display_message(jcode_tui_messages::DisplayMessage::system(listing));
        true
    }

    fn hotkey_registry(&self, remote: bool) -> Vec<KnownHotkey> {
        build_registry(&RegistryInputs {
            model_switch: &self.model_switch_keys,
            effort: &self.effort_switch_keys,
            scroll: &self.scroll_keys,
            centered: &self.centered_toggle_keys,
            toggles: &self.toggle_keys,
            workspace: &self.workspace_navigation_keys,
            dictation: &self.dictation_key,
            new_terminal: &self.new_terminal_key,
            open_resume: &self.open_resume_key,
            fallback_switch: &self.fallback_switch_key,
            remote,
        })
    }

    /// Called from the key dispatchers for every non-overlay key press. When
    /// the chord is a known hotkey the user rarely uses, surface a short
    /// inline "you just pressed X → does Y" note and record the use.
    pub(crate) fn observe_known_hotkey(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        remote: bool,
    ) {
        if !crate::config::config().display.keybinding_hints {
            return;
        }
        // Only modified chords (or BackTab) can be hotkeys; skip plain typing.
        // Exception: macOS terminals that keep Option as a character key send
        // Option+M as a bare 'µ' with no ALT modifier; `KeyBinding::matches`
        // translates those, so let them through to the lookup.
        let macos_option_char = cfg!(target_os = "macos")
            && modifiers.is_empty()
            && jcode_tui_core::keybind::macos_option_char_to_ascii_key(code).is_some();
        if modifiers.is_empty() && code != KeyCode::BackTab && !macos_option_char {
            return;
        }
        // Shift alone on a character is just typing (uppercase letters, symbols).
        if modifiers == KeyModifiers::SHIFT && matches!(code, KeyCode::Char(_)) {
            return;
        }
        // While the slash-command suggestion list is open, Ctrl+J/K etc. move
        // the selection instead of their global meanings; stay silent.
        if !self.command_suggestions().is_empty() {
            return;
        }

        let registry = self.hotkey_registry(remote);
        let Some(info) = lookup(&registry, self.input.is_empty(), code, modifiers) else {
            return;
        };

        let now = now_unix();
        let state = self.hotkey_usage.get_or_insert_with(load_state);
        let stat = state.actions.entry(info.action.to_string()).or_default();
        let unfamiliar = is_unfamiliar(stat, now);
        stat.uses = stat.uses.saturating_add(1);
        stat.last_used_unix = now;
        // Persist while the counters still matter, plus an occasional refresh so
        // `last_used_unix` on disk tracks reality without rewriting the file on
        // every rapid keypress (word-nav, scrolling).
        if stat.uses <= FAMILIAR_USES || unfamiliar || stat.uses.is_multiple_of(32) {
            save_state(state);
        }

        if unfamiliar && !info.quiet {
            self.hotkey_feedback = Some((
                format!("⌨ {} → {}", info.label(), info.description),
                std::time::Instant::now(),
            ));
        }
    }

    /// Called from the key dispatchers when a modified chord fell all the way
    /// through unhandled. Tells the user the chord is unbound and suggests the
    /// nearest known hotkey.
    pub(crate) fn note_unrecognized_hotkey(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        remote: bool,
    ) {
        if !crate::config::config().display.keybinding_hints {
            return;
        }
        let interesting = modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
            || matches!(code, KeyCode::F(_));
        if !interesting {
            return;
        }
        // Modifier-only key events (e.g. pressing and releasing Ctrl) are not chords.
        if matches!(code, KeyCode::Modifier(_)) {
            return;
        }

        let registry = self.hotkey_registry(remote);
        // A known-but-contextually-inert chord (e.g. the fallback-accept key
        // with no offer armed) is not "unbound"; stay silent.
        if lookup(&registry, self.input.is_empty(), code, modifiers).is_some() {
            return;
        }

        // Rate-limit: repeats of held keys and frantic mashing should not spam.
        let now = std::time::Instant::now();
        if let Some(last) = self.last_unknown_hotkey_notice
            && now.duration_since(last).as_millis() < UNKNOWN_NOTICE_MIN_GAP_MS
        {
            return;
        }
        let chord = format_binding(&KeyBinding { code, modifiers });
        let seen = self.unknown_hotkey_seen.entry(chord).or_insert(0);
        if *seen >= UNKNOWN_NOTICE_MAX_PER_CHORD {
            return;
        }
        *seen += 1;
        self.last_unknown_hotkey_notice = Some(now);

        self.hotkey_feedback = Some((
            unknown_chord_message(&registry, code, modifiers),
            std::time::Instant::now(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_inputs_registry(remote: bool) -> Vec<KnownHotkey> {
        let model_switch = ModelSwitchKeys {
            next: key(KeyCode::Tab, KeyModifiers::CONTROL),
            prev: Some(key(
                KeyCode::Tab,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            )),
        };
        let effort = EffortSwitchKeys {
            increase: key(KeyCode::Right, KeyModifiers::ALT),
            decrease: key(KeyCode::Left, KeyModifiers::ALT),
        };
        let scroll = ScrollKeys {
            up: key(
                KeyCode::Char('k'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
            down: key(
                KeyCode::Char('j'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
            up_fallback: None,
            down_fallback: None,
            page_up: alt('u'),
            page_down: alt('d'),
            prompt_up: ctrl('k'),
            prompt_down: ctrl('j'),
            bookmark: ctrl('g'),
        };
        let centered = CenteredToggleKeys {
            toggle: Some(alt('c')),
        };
        let toggles = crate::tui::keybind::load_toggle_keys();
        let workspace = WorkspaceNavigationKeys {
            left: vec![alt('h')],
            down: vec![alt('j')],
            up: vec![alt('k')],
            right: vec![alt('l')],
        };
        let dictation = OptionalBinding::default();
        // Bind the optional chords in the fixture so coverage tests can verify
        // they flow through build_registry when configured.
        let new_terminal = OptionalBinding {
            binding: Some(key(KeyCode::Enter, KeyModifiers::ALT)),
            label: Some("Alt+Enter".to_string()),
        };
        let open_resume = OptionalBinding {
            binding: Some(alt('r')),
            label: Some("Alt+R".to_string()),
        };
        let fallback_switch = OptionalBinding {
            binding: Some(ctrl('y')),
            label: Some("Ctrl+Y".to_string()),
        };
        build_registry(&RegistryInputs {
            model_switch: &model_switch,
            effort: &effort,
            scroll: &scroll,
            centered: &centered,
            toggles: &toggles,
            workspace: &workspace,
            dictation: &dictation,
            new_terminal: &new_terminal,
            open_resume: &open_resume,
            fallback_switch: &fallback_switch,
            remote,
        })
    }

    #[test]
    fn lookup_finds_builtin_ctrl_p_auto_poke() {
        let registry = test_inputs_registry(false);
        let info = lookup(&registry, true, KeyCode::Char('p'), KeyModifiers::CONTROL)
            .expect("ctrl+p known");
        assert_eq!(info.action, "auto_poke_toggle");
    }

    #[test]
    fn lookup_prefers_kill_line_over_prompt_jump_with_draft() {
        let registry = test_inputs_registry(false);
        let with_draft = lookup(&registry, false, KeyCode::Char('k'), KeyModifiers::CONTROL)
            .expect("ctrl+k known");
        assert_eq!(with_draft.action, "kill_to_end");

        let empty = lookup(&registry, true, KeyCode::Char('k'), KeyModifiers::CONTROL)
            .expect("ctrl+k known");
        assert_eq!(empty.action, "prompt_jump_up");
    }

    #[test]
    fn lookup_reports_copy_viewport_for_ctrl_a_on_empty_input() {
        let registry = test_inputs_registry(false);
        let empty = lookup(&registry, true, KeyCode::Char('a'), KeyModifiers::CONTROL)
            .expect("ctrl+a known");
        assert_eq!(empty.action, "copy_viewport");

        let with_draft = lookup(&registry, false, KeyCode::Char('a'), KeyModifiers::CONTROL)
            .expect("ctrl+a known");
        assert_eq!(with_draft.action, "input_home");
    }

    #[test]
    fn workspace_bindings_only_register_in_remote_mode() {
        let local = test_inputs_registry(false);
        assert!(!local.iter().any(|k| k.action.starts_with("workspace_")));

        let remote = test_inputs_registry(true);
        assert!(remote.iter().any(|k| k.action == "workspace_left"));
    }

    #[test]
    fn nearest_suggests_same_key_one_modifier_away() {
        let registry = test_inputs_registry(false);
        let near = nearest_hotkey(
            &registry,
            KeyCode::Char('p'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        )
        .expect("suggestion for ctrl+shift+p");
        assert_eq!(near.action, "auto_poke_toggle");
    }

    #[test]
    fn nearest_suggests_same_letter_under_other_modifier() {
        let registry = test_inputs_registry(false);
        // Ctrl+M is unbound; Alt+M toggles the side panel.
        let near = nearest_hotkey(&registry, KeyCode::Char('m'), KeyModifiers::CONTROL)
            .expect("suggestion for ctrl+m");
        assert_eq!(near.action, "side_panel_toggle");
    }

    #[test]
    fn nearest_returns_none_for_unrelated_chords() {
        let registry = test_inputs_registry(false);
        assert!(nearest_hotkey(&registry, KeyCode::Char(';'), KeyModifiers::CONTROL).is_none());
    }

    #[test]
    fn unknown_chord_message_mentions_chord_and_suggestion() {
        let registry = test_inputs_registry(false);
        let msg = unknown_chord_message(
            &registry,
            KeyCode::Char('p'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert!(msg.contains("Ctrl+Shift+P"), "{msg}");
        assert!(msg.contains("Ctrl+P"), "{msg}");
        assert!(msg.contains("auto-poke"), "{msg}");

        let fallback = unknown_chord_message(&registry, KeyCode::Char(';'), KeyModifiers::CONTROL);
        assert!(fallback.contains("isn't bound"), "{fallback}");
        assert!(fallback.contains("/help"), "{fallback}");
    }

    #[test]
    fn familiarity_thresholds() {
        let now = 10_000_000u64;
        let fresh = UsageStat {
            uses: 0,
            last_used_unix: 0,
        };
        assert!(is_unfamiliar(&fresh, now));

        let learning = UsageStat {
            uses: FAMILIAR_USES - 1,
            last_used_unix: now,
        };
        assert!(is_unfamiliar(&learning, now));

        let familiar = UsageStat {
            uses: FAMILIAR_USES,
            last_used_unix: now,
        };
        assert!(!is_unfamiliar(&familiar, now));

        let stale = UsageStat {
            uses: 50,
            last_used_unix: now - STALE_SECS,
        };
        assert!(is_unfamiliar(&stale, now));
    }

    /// Drift guard: every configurable keybinding in the canonical registry
    /// must be represented in the hotkey-feedback registry (as one of the
    /// action ids the builder can emit). If this fails, someone added a new
    /// keybinding without teaching the feedback system about it, which would
    /// make `note_unrecognized_hotkey` falsely claim the chord "isn't bound".
    #[test]
    fn registry_covers_every_canonical_keybinding_default() {
        use crate::tui::keybind::KEYBINDING_DEFAULTS;

        // Canonical ids -> the action id(s) the feedback registry uses.
        // A None mapping documents an intentional exclusion.
        let mapping: &[(&str, Option<&[&str]>)] = &[
            ("scroll_up", Some(&["scroll_up"])),
            ("scroll_down", Some(&["scroll_down"])),
            ("scroll_page_up", Some(&["scroll_page_up"])),
            ("scroll_page_down", Some(&["scroll_page_down"])),
            ("model_switch_next", Some(&["model_switch_next"])),
            ("model_switch_prev", Some(&["model_switch_prev"])),
            ("fallback_switch", Some(&["fallback_switch"])),
            ("effort_increase", Some(&["effort_increase"])),
            ("effort_decrease", Some(&["effort_decrease"])),
            ("centered_toggle", Some(&["centered_toggle"])),
            ("scroll_prompt_up", Some(&["prompt_jump_up"])),
            ("scroll_prompt_down", Some(&["prompt_jump_down"])),
            ("scroll_bookmark", Some(&["scroll_bookmark"])),
            ("scroll_up_fallback", Some(&["scroll_up"])),
            ("scroll_down_fallback", Some(&["scroll_down"])),
            ("workspace_left", Some(&["workspace_left"])),
            ("workspace_down", Some(&["workspace_down"])),
            ("workspace_up", Some(&["workspace_up"])),
            ("workspace_right", Some(&["workspace_right"])),
            ("new_terminal", Some(&["new_terminal"])),
            ("open_resume", Some(&["open_resume"])),
        ];

        let registry = test_inputs_registry(true);
        let registry_actions: std::collections::HashSet<&str> =
            registry.iter().map(|k| k.action).collect();

        for default in KEYBINDING_DEFAULTS {
            let entry = mapping.iter().find(|(id, _)| *id == default.id);
            let Some((_, actions)) = entry else {
                panic!(
                    "keybinding default `{}` is not mapped in the hotkey-feedback \
                     registry test. Add it to build_registry (so users get \
                     press-feedback and never a false \"isn't bound\" notice) and \
                     record the mapping here.",
                    default.id
                );
            };
            if let Some(actions) = actions {
                for action in *actions {
                    assert!(
                        registry_actions.contains(action),
                        "mapped action `{action}` for keybinding default `{}` is \
                         missing from build_registry output",
                        default.id
                    );
                }
            }
        }
    }

    #[test]
    fn hotkeys_listing_groups_chords_and_splits_by_usage() {
        let registry = test_inputs_registry(true);
        let mut usage = HotkeyUsageState::default();
        usage.actions.insert(
            "prompt_jump_up".to_string(),
            UsageStat {
                uses: 7,
                last_used_unix: 10_000_000,
            },
        );
        let listing = render_hotkeys_listing(&registry, &usage, 10_000_000);

        // Used action lands in the muscle-memory section with its count.
        assert!(listing.contains("jump to the previous prompt"), "{listing}");
        assert!(listing.contains("(7 uses)"), "{listing}");
        // Unused actions land in the try-these section.
        assert!(listing.contains("Not yet used"), "{listing}");
        assert!(listing.contains("toggle the scroll bookmark"), "{listing}");
        // Stale familiar actions get flagged.
        let mut stale_usage = HotkeyUsageState::default();
        stale_usage.actions.insert(
            "prompt_jump_up".to_string(),
            UsageStat {
                uses: 7,
                last_used_unix: 0,
            },
        );
        let stale = render_hotkeys_listing(&registry, &stale_usage, 10_000_000);
        assert!(stale.contains("not recently"), "{stale}");
    }

    /// Drift guard for the pane/mode toggles that live outside
    /// KEYBINDING_DEFAULTS (ToggleKeys fields). Uses the real loader so a new
    /// ToggleKeys field without registry coverage fails here.
    #[test]
    fn registry_covers_every_toggle_binding() {
        let registry = test_inputs_registry(false);
        let toggles = crate::tui::keybind::load_toggle_keys();
        let toggle_bindings: &[(&str, Option<&KeyBinding>)] = &[
            ("side_panel_toggle", toggles.side_panel.binding()),
            ("copy_selection_toggle", toggles.copy_selection.binding()),
            ("diagram_pane_toggle", toggles.diagram_pane.binding()),
            (
                "typing_scroll_lock_toggle",
                toggles.typing_scroll_lock.binding(),
            ),
            ("diff_mode_cycle", toggles.diff_mode_cycle.binding()),
            ("info_widget_toggle", toggles.info_widget.binding()),
            ("swarm_panel_focus", toggles.swarm_panel_focus.binding()),
        ];
        for (name, binding) in toggle_bindings {
            let Some(binding) = binding else { continue };
            assert!(
                lookup(&registry, true, binding.code, binding.modifiers).is_some(),
                "toggle `{name}` chord {:?} is not resolvable in the feedback registry",
                binding
            );
        }
    }
}
