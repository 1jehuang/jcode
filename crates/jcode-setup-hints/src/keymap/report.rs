//! Human-readable rendering of the keymap snapshot and detected conflicts,
//! shared by the `/keys` command and the startup conflict hint.

use jcode_config_types::KeybindingsConfig;

use super::KeymapSnapshot;
use super::conflicts::{Conflict, detect_conflicts};
use super::source::KeySource;

/// Render a full diagnostic report: detected terminal, discovered binding
/// counts, and any conflicts with jcode's configured bindings.
pub fn render_report(cfg: &KeybindingsConfig, snapshot: &KeymapSnapshot) -> String {
    let mut out = String::new();
    out.push_str("Keymap diagnostics\n");
    out.push_str(&format!(
        "Terminal: {}{}\n",
        snapshot.terminal,
        if snapshot.terminal_version.is_empty() {
            String::new()
        } else {
            format!(" {}", snapshot.terminal_version)
        }
    ));
    out.push_str(&format!("OS: {}\n", snapshot.os));

    let term_count = snapshot.bindings_from_source(KeySource::Terminal).count();
    let sys_count = snapshot
        .bindings_from_source(KeySource::MacosSystem)
        .count();
    let app_count = snapshot
        .bindings_from_source(KeySource::ExternalApp)
        .count();
    out.push_str(&format!(
        "Discovered bindings: {term_count} terminal, {sys_count} macOS system, {app_count} app\n",
    ));

    if app_count > 0 {
        let mut tools: Vec<&str> = snapshot
            .bindings_from_source(KeySource::ExternalApp)
            .map(|b| b.tool.as_str())
            .filter(|t| !t.is_empty())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        tools.dedup();
        if !tools.is_empty() {
            out.push_str(&format!("Apps scanned: {}\n", tools.join(", ")));
        }
    }

    if term_count == 0 && sys_count == 0 && app_count == 0 {
        out.push_str(
            "\nNo machine bindings were discovered. jcode can read Ghostty bindings, macOS\n\
             system shortcuts, and a few window managers (OmniWM, AeroSpace, skhd); other\n\
             terminals and tools are not yet inspected, so conflicts there will not be\n\
             detected.\n",
        );
    }

    let conflicts = detect_conflicts(cfg, snapshot);
    out.push('\n');
    if conflicts.is_empty() {
        out.push_str("No conflicts found between your jcode keybindings and the machine.\n");
    } else {
        out.push_str(&format!(
            "{} potential conflict{} found:\n\n",
            conflicts.len(),
            if conflicts.len() == 1 { "" } else { "s" }
        ));
        for c in &conflicts {
            out.push_str(&render_conflict_block(c));
            out.push('\n');
        }
        out.push_str(
            "These keys may be captured by your terminal, macOS, or another app (window\n\
             manager, launcher) before jcode sees them.\n\
             To fix: rebind the jcode action in ~/.jcode/config.toml under [keybindings],\n\
             or change the conflicting shortcut in the other app's settings.\n",
        );
    }

    out
}

fn render_conflict_block(c: &Conflict) -> String {
    let interceptor_desc = match c.interceptor.source {
        KeySource::MacosSystem => format!("macOS: {}", c.interceptor.action),
        KeySource::Terminal => {
            if c.interceptor.action.is_empty() {
                "terminal action".to_string()
            } else {
                format!("terminal: {}", c.interceptor.action)
            }
        }
        KeySource::ExternalApp => {
            let tool = if c.interceptor.tool.is_empty() {
                "app"
            } else {
                c.interceptor.tool.as_str()
            };
            if c.interceptor.action.is_empty() {
                tool.to_string()
            } else {
                format!("{tool}: {}", c.interceptor.action)
            }
        }
    };
    format!(
        "  ⚠ {key}\n      jcode: {action} ({field} = \"{raw}\")\n      taken by {interceptor}\n",
        key = c.jcode.chord.display(),
        action = c.jcode.action,
        field = c.jcode.field,
        raw = c.jcode.raw,
        interceptor = interceptor_desc,
    )
}

/// A compact one-line status string suitable for a startup notice, or `None`
/// when there are no conflicts.
pub fn render_status_line(cfg: &KeybindingsConfig, snapshot: &KeymapSnapshot) -> Option<String> {
    let conflicts = detect_conflicts(cfg, snapshot);
    if conflicts.is_empty() {
        return None;
    }
    let keys: Vec<String> = conflicts
        .iter()
        .map(|c| c.jcode.chord.display())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    Some(format!(
        "Keybinding conflict: {} may be intercepted by your terminal/OS/apps. Run /keys for details.",
        keys.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::KeyChord;
    use crate::keymap::source::DiscoveredBinding;

    fn snapshot_with(bindings: Vec<DiscoveredBinding>) -> KeymapSnapshot {
        KeymapSnapshot {
            version: 1,
            captured_at: "0".to_string(),
            os: "macos".to_string(),
            terminal: "Ghostty".to_string(),
            terminal_version: "1.3.1".to_string(),
            bindings,
        }
    }

    fn term(keys: &str, action: &str) -> DiscoveredBinding {
        DiscoveredBinding {
            chord: KeyChord::parse(keys).unwrap(),
            source: KeySource::Terminal,
            action: action.to_string(),
            raw: format!("{keys}={action}"),
            tool: String::new(),
        }
    }

    #[test]
    fn report_lists_conflicts_with_field_names() {
        let cfg = KeybindingsConfig::default();
        let snap = snapshot_with(vec![term("ctrl+tab", "next_tab")]);
        let report = render_report(&cfg, &snap);
        assert!(report.contains("Ghostty 1.3.1"));
        assert!(report.contains("keybindings.model_switch_next"));
        assert!(report.contains("next_tab"));
        assert!(report.contains("Ctrl+Tab"));
    }

    #[test]
    fn report_says_clean_when_no_conflicts() {
        let cfg = KeybindingsConfig::default();
        let snap = snapshot_with(vec![term("cmd+t", "new_tab")]);
        let report = render_report(&cfg, &snap);
        assert!(report.contains("No conflicts found"));
    }

    #[test]
    fn status_line_present_only_on_conflict() {
        let cfg = KeybindingsConfig::default();
        let clean = snapshot_with(vec![term("cmd+t", "new_tab")]);
        assert!(render_status_line(&cfg, &clean).is_none());

        let dirty = snapshot_with(vec![term("ctrl+tab", "next_tab")]);
        let line = render_status_line(&cfg, &dirty).unwrap();
        assert!(line.contains("Ctrl+Tab"));
        assert!(line.contains("/keys"));
    }
}
