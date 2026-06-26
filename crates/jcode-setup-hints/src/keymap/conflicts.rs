//! Detect conflicts between jcode's own key bindings and the bindings
//! discovered on the machine (terminal emulator + macOS system shortcuts).
//!
//! The flow is:
//!   1. enumerate jcode's configured bindings as `(field, label, KeyChord)`
//!      from [`jcode_config_types::KeybindingsConfig`] ([`jcode_bindings`]),
//!   2. index the discovered machine bindings from a [`KeymapSnapshot`],
//!   3. report every overlap as a [`Conflict`] that names the exact config
//!      field so a warning can point the user at the right line.
//!
//! All of this is pure given a `KeybindingsConfig` and a `KeymapSnapshot`, so it
//! is fully unit-testable without touching the machine.

use std::collections::HashMap;

use jcode_config_types::KeybindingsConfig;

use super::chord::KeyChord;
use super::source::{DiscoveredBinding, KeySource};
use super::KeymapSnapshot;

/// One configured jcode binding, tied back to its config field.
#[derive(Debug, Clone)]
pub struct JcodeBinding {
    /// The dotted config path, e.g. `keybindings.model_switch_next`.
    pub field: String,
    /// Human-friendly description of what the binding does.
    pub action: String,
    /// The configured value as written, e.g. `ctrl+tab`.
    pub raw: String,
    /// The parsed chord.
    pub chord: KeyChord,
}

/// A detected conflict between a jcode binding and something on the machine.
#[derive(Debug, Clone)]
pub struct Conflict {
    /// The jcode binding that may not reach the app.
    pub jcode: JcodeBinding,
    /// The machine binding that intercepts it.
    pub interceptor: DiscoveredBinding,
}

impl Conflict {
    /// A single-line, user-facing description of the conflict.
    pub fn summary(&self) -> String {
        format!(
            "{} (`{}` = \"{}\") is also bound by your {} to {}",
            self.jcode.action,
            self.jcode.field,
            self.jcode.raw,
            self.interceptor.source.label(),
            describe_action(&self.interceptor),
        )
    }
}

fn describe_action(b: &DiscoveredBinding) -> String {
    match b.source {
        KeySource::MacosSystem => b.action.clone(),
        KeySource::Terminal => {
            if b.action.is_empty() {
                "a terminal action".to_string()
            } else {
                format!("`{}`", b.action)
            }
        }
    }
}

/// Enumerate jcode's configured bindings as comparable chords. Bindings that are
/// disabled or fail to parse are skipped. Multi-binding fields (the workspace
/// navigation keys accept a comma-separated list) expand into one entry per
/// chord.
pub fn jcode_bindings(cfg: &KeybindingsConfig) -> Vec<JcodeBinding> {
    // (field, action, raw-value) for single-chord fields.
    let single: &[(&str, &str, &str)] = &[
        ("scroll_up", "Scroll up", cfg.scroll_up.as_str()),
        ("scroll_down", "Scroll down", cfg.scroll_down.as_str()),
        ("scroll_page_up", "Page up", cfg.scroll_page_up.as_str()),
        ("scroll_page_down", "Page down", cfg.scroll_page_down.as_str()),
        (
            "model_switch_next",
            "Switch to next model",
            cfg.model_switch_next.as_str(),
        ),
        (
            "model_switch_prev",
            "Switch to previous model",
            cfg.model_switch_prev.as_str(),
        ),
        (
            "effort_increase",
            "Increase reasoning effort",
            cfg.effort_increase.as_str(),
        ),
        (
            "effort_decrease",
            "Decrease reasoning effort",
            cfg.effort_decrease.as_str(),
        ),
        (
            "centered_toggle",
            "Toggle centered layout",
            cfg.centered_toggle.as_str(),
        ),
        (
            "scroll_prompt_up",
            "Jump to previous prompt",
            cfg.scroll_prompt_up.as_str(),
        ),
        (
            "scroll_prompt_down",
            "Jump to next prompt",
            cfg.scroll_prompt_down.as_str(),
        ),
        (
            "scroll_bookmark",
            "Toggle scroll bookmark",
            cfg.scroll_bookmark.as_str(),
        ),
        (
            "scroll_up_fallback",
            "Scroll up (fallback)",
            cfg.scroll_up_fallback.as_str(),
        ),
        (
            "scroll_down_fallback",
            "Scroll down (fallback)",
            cfg.scroll_down_fallback.as_str(),
        ),
        (
            "side_panel_toggle",
            "Toggle side panel",
            cfg.side_panel_toggle.as_str(),
        ),
        (
            "copy_selection_toggle",
            "Toggle copy/selection mode",
            cfg.copy_selection_toggle.as_str(),
        ),
        (
            "diagram_pane_toggle",
            "Toggle diagram pane",
            cfg.diagram_pane_toggle.as_str(),
        ),
        (
            "typing_scroll_lock_toggle",
            "Toggle typing scroll lock",
            cfg.typing_scroll_lock_toggle.as_str(),
        ),
        (
            "diff_mode_cycle",
            "Cycle diff display mode",
            cfg.diff_mode_cycle.as_str(),
        ),
        (
            "info_widget_toggle",
            "Toggle info widget",
            cfg.info_widget_toggle.as_str(),
        ),
    ];

    let mut out = Vec::new();
    for (field, action, raw) in single {
        if let Some(chord) = KeyChord::parse(raw) {
            out.push(JcodeBinding {
                field: format!("keybindings.{field}"),
                action: action.to_string(),
                raw: raw.to_string(),
                chord,
            });
        }
    }

    // Multi-binding (comma-separated list) fields.
    let multi: [(&str, &str, &str); 4] = [
        ("workspace_left", "Move to left workspace", cfg.workspace_left.as_str()),
        ("workspace_down", "Move to lower workspace", cfg.workspace_down.as_str()),
        ("workspace_up", "Move to upper workspace", cfg.workspace_up.as_str()),
        ("workspace_right", "Move to right workspace", cfg.workspace_right.as_str()),
    ];
    for (field, action, raw) in multi {
        for piece in raw.split(',') {
            let piece = piece.trim();
            if let Some(chord) = KeyChord::parse(piece) {
                out.push(JcodeBinding {
                    field: format!("keybindings.{field}"),
                    action: action.to_string(),
                    raw: piece.to_string(),
                    chord,
                });
            }
        }
    }

    out
}

/// Find conflicts between jcode's configured bindings and the discovered
/// machine bindings in `snapshot`.
///
/// Conflicts are deduplicated per `(jcode field, interceptor chord, source)` so
/// a single overlap is reported once even if the snapshot lists the same chord
/// multiple times (Ghostty, for example, lists `super+1` and `super+digit_1`).
pub fn detect_conflicts(cfg: &KeybindingsConfig, snapshot: &KeymapSnapshot) -> Vec<Conflict> {
    // Index discovered bindings by chord for O(1) lookup.
    let mut by_chord: HashMap<&KeyChord, Vec<&DiscoveredBinding>> = HashMap::new();
    for b in &snapshot.bindings {
        by_chord.entry(&b.chord).or_default().push(b);
    }

    let mut seen: std::collections::HashSet<(String, String, KeySource)> =
        std::collections::HashSet::new();
    let mut conflicts = Vec::new();

    for jcode in jcode_bindings(cfg) {
        let Some(interceptors) = by_chord.get(&jcode.chord) else {
            continue;
        };
        for interceptor in interceptors {
            let dedup_key = (
                jcode.field.clone(),
                interceptor.chord.canonical(),
                interceptor.source,
            );
            if !seen.insert(dedup_key) {
                continue;
            }
            conflicts.push(Conflict {
                jcode: jcode.clone(),
                interceptor: (*interceptor).clone(),
            });
        }
    }

    conflicts
}

/// A stable signature for a set of conflicts, used to decide whether to re-warn
/// the user. Two runs that find the same conflicts (regardless of order)
/// produce the same signature; any change (new conflict, resolved conflict,
/// rebind) produces a different one.
pub fn conflict_signature(conflicts: &[Conflict]) -> String {
    let mut parts: Vec<String> = conflicts
        .iter()
        .map(|c| {
            format!(
                "{}|{}|{}",
                c.jcode.field,
                c.jcode.chord.canonical(),
                c.interceptor.chord.canonical()
            )
        })
        .collect();
    parts.sort();
    parts.dedup();
    parts.join(";")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::source::KeySource;

    fn term_binding(canonical_keys: &str, action: &str) -> DiscoveredBinding {
        DiscoveredBinding {
            chord: KeyChord::parse(canonical_keys).unwrap(),
            source: KeySource::Terminal,
            action: action.to_string(),
            raw: format!("{canonical_keys}={action}"),
        }
    }

    fn snapshot_with(bindings: Vec<DiscoveredBinding>) -> KeymapSnapshot {
        KeymapSnapshot {
            version: 1,
            captured_at: "0".to_string(),
            os: "macos".to_string(),
            terminal: "Ghostty".to_string(),
            terminal_version: String::new(),
            bindings,
        }
    }

    #[test]
    fn enumerates_default_bindings() {
        let cfg = KeybindingsConfig::default();
        let binds = jcode_bindings(&cfg);
        // The defaults include model_switch_next = ctrl+tab.
        assert!(
            binds
                .iter()
                .any(|b| b.field == "keybindings.model_switch_next"
                    && b.chord.canonical() == "ctrl+tab"),
            "expected model_switch_next ctrl+tab in {binds:#?}"
        );
        // Workspace nav defaults are alt+h/j/k/l.
        assert!(binds.iter().any(|b| b.field == "keybindings.workspace_left"
            && b.chord.canonical() == "alt+h"));
    }

    #[test]
    fn detects_ghostty_ctrl_tab_conflict() {
        let cfg = KeybindingsConfig::default();
        let snapshot = snapshot_with(vec![
            term_binding("ctrl+tab", "next_tab"),
            term_binding("ctrl+shift+tab", "previous_tab"),
        ]);
        let conflicts = detect_conflicts(&cfg, &snapshot);
        let fields: Vec<&str> = conflicts.iter().map(|c| c.jcode.field.as_str()).collect();
        assert!(fields.contains(&"keybindings.model_switch_next"));
        assert!(fields.contains(&"keybindings.model_switch_prev"));
        let summary = conflicts[0].summary();
        assert!(summary.contains("model"), "summary was: {summary}");
        assert!(summary.contains("terminal"), "summary was: {summary}");
    }

    #[test]
    fn no_conflict_when_chords_differ() {
        let cfg = KeybindingsConfig::default();
        let snapshot = snapshot_with(vec![term_binding("cmd+t", "new_tab")]);
        // jcode has no cmd+t binding by default.
        assert!(detect_conflicts(&cfg, &snapshot).is_empty());
    }

    #[test]
    fn deduplicates_repeated_interceptor_chords() {
        let mut cfg = KeybindingsConfig::default();
        cfg.side_panel_toggle = "cmd+1".to_string();
        // Ghostty lists both super+1 and super+digit_1 for goto_tab:1.
        let snapshot = snapshot_with(vec![
            term_binding("cmd+1", "goto_tab:1"),
            term_binding("cmd+1", "goto_tab:1"),
        ]);
        let conflicts = detect_conflicts(&cfg, &snapshot);
        assert_eq!(conflicts.len(), 1, "duplicate interceptors should collapse");
    }

    #[test]
    fn disabled_binding_is_not_reported() {
        let mut cfg = KeybindingsConfig::default();
        cfg.model_switch_next = "none".to_string();
        let snapshot = snapshot_with(vec![term_binding("ctrl+tab", "next_tab")]);
        let conflicts = detect_conflicts(&cfg, &snapshot);
        assert!(
            !conflicts
                .iter()
                .any(|c| c.jcode.field == "keybindings.model_switch_next")
        );
    }

    #[test]
    fn signature_is_order_independent_and_changes_on_diff() {
        let cfg = KeybindingsConfig::default();
        let snap_a = snapshot_with(vec![
            term_binding("ctrl+tab", "next_tab"),
            term_binding("ctrl+shift+tab", "previous_tab"),
        ]);
        // Same conflicts, reversed discovery order.
        let snap_b = snapshot_with(vec![
            term_binding("ctrl+shift+tab", "previous_tab"),
            term_binding("ctrl+tab", "next_tab"),
        ]);
        let sig_a = conflict_signature(&detect_conflicts(&cfg, &snap_a));
        let sig_b = conflict_signature(&detect_conflicts(&cfg, &snap_b));
        assert_eq!(sig_a, sig_b, "signature must be order-independent");

        let snap_c = snapshot_with(vec![term_binding("ctrl+tab", "next_tab")]);
        let sig_c = conflict_signature(&detect_conflicts(&cfg, &snap_c));
        assert_ne!(sig_a, sig_c, "different conflict set => different signature");

        // No conflicts => empty signature.
        let clean = snapshot_with(vec![term_binding("cmd+t", "new_tab")]);
        assert_eq!(conflict_signature(&detect_conflicts(&cfg, &clean)), "");
    }
}
