use super::*;

#[test]
fn first_launch_shows_explicit_alignment_hint_first() {
    let state = SetupHintsState {
        launch_count: 1,
        ..SetupHintsState::default()
    };

    let hints = startup_hints_for_launch(&state).expect("expected startup hint");
    assert_eq!(
        hints.status_notice.as_deref(),
        Some("Tip: `/alignment centered` or Alt+C toggles alignment.")
    );

    let (title, message) = hints.display_message.expect("expected display message");
    assert_eq!(title, "Alignment");
    assert!(message.contains("Alt+C"));
    assert!(message.contains("/alignment centered"));
    assert!(message.contains("left-aligned by default"));
    assert!(!message.contains("display.centered = true"));
}

#[test]
fn second_and_third_launches_include_alignment_tip() {
    let state = SetupHintsState {
        launch_count: 2,
        ..SetupHintsState::default()
    };

    let hints = startup_hints_for_launch(&state).expect("expected startup hint");
    assert_eq!(
        hints.status_notice.as_deref(),
        Some("Tip: Alt+C toggles left/center alignment.")
    );

    let (title, message) = hints.display_message.expect("expected display message");
    assert_eq!(title, "Welcome");
    assert!(message.contains("Alt+C"));
    assert!(message.contains("/alignment centered"));
    assert!(message.contains("/alignment left"));
    assert!(message.contains("display.centered = true"));
    assert!(message.contains("Left-aligned mode is the default"));
}

#[test]
fn launches_after_third_do_not_show_generic_alignment_tip() {
    let state = SetupHintsState {
        launch_count: 4,
        ..SetupHintsState::default()
    };

    assert!(startup_hints_for_launch(&state).is_none());
}

#[cfg(any(test, target_os = "macos"))]
#[test]
fn first_three_launches_can_include_hotkey_notice_too() {
    let state = SetupHintsState {
        launch_count: 2,
        hotkey_configured: true,
        ..SetupHintsState::default()
    };

    let hints = startup_hints_for_launch(&state).expect("expected startup hint");
    let (_, message) = hints.display_message.expect("expected display message");
    assert!(message.contains("Alt+C"));
    assert!(message.contains("Cmd+;"));
    // The notice should make clear the hotkey works globally, not just inside jcode.
    assert!(message.contains("system-wide"));
    // All three launch hotkeys should be mentioned.
    assert!(message.contains("Cmd+'"));
    assert!(message.contains("Cmd+Shift+'"));
}

#[test]
fn default_resolved_hotkeys_match_legacy_three() {
    // With no config, the resolver reproduces the historical three hotkeys.
    let resolved = launch_hotkeys::resolve_launch_hotkeys(
        &jcode_config_types::LaunchHotkeysConfig::default(),
        "/usr/local/bin/jcode",
        "/home/u/.jcode/hotkey/last_dir",
        "/home/u/.jcode/hotkey/last_repo",
    );
    let chords: Vec<&str> = resolved.iter().map(|r| r.chord.as_str()).collect();
    assert_eq!(chords, vec!["cmd+;", "cmd+'", "cmd+shift+'"]);

    // Home launch passes no extra subcommand; self-dev passes `self-dev`.
    let home = launch_hotkeys::shell_command_for(&resolved[0], "/usr/local/bin/jcode");
    assert!(home.starts_with("cd \"$HOME\"; "));
    assert!(!home.contains("self-dev"));

    let last_dir = launch_hotkeys::shell_command_for(&resolved[1], "/usr/local/bin/jcode");
    assert!(last_dir.contains("cat '/home/u/.jcode/hotkey/last_dir'"));
    assert!(last_dir.contains("cd \"$HOME\""));

    let selfdev = launch_hotkeys::shell_command_for(&resolved[2], "/usr/local/bin/jcode");
    assert!(selfdev.contains("cat '/home/u/.jcode/hotkey/last_repo'"));
    assert!(selfdev.contains("'/usr/local/bin/jcode' 'self-dev';"));
}

#[test]
fn baked_repo_hotkey_cds_into_fixed_dir() {
    // A config-baked per-repo hotkey opens a fixed directory.
    let config = jcode_config_types::LaunchHotkeysConfig {
        enabled: Some(true),
        imported: true,
        entries: vec![jcode_config_types::LaunchHotkeyEntry {
            chord: "cmd+[".to_string(),
            dir: "/Users/jeremy/jcode-github".to_string(),
            label: "jcode-github".to_string(),
            self_dev: false,
        }],
    };
    let resolved = launch_hotkeys::resolve_launch_hotkeys(
        &config,
        "/usr/local/bin/jcode",
        "/home/u/.jcode/hotkey/last_dir",
        "/home/u/.jcode/hotkey/last_repo",
    );
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].chord, "cmd+[");
    let cmd = launch_hotkeys::shell_command_for(&resolved[0], "/usr/local/bin/jcode");
    assert!(cmd.contains("/Users/jeremy/jcode-github"));
    assert!(cmd.contains("cd \"$HOME\""), "must keep a home fallback");
    assert!(!cmd.contains("self-dev"));
}

#[test]
fn should_record_last_dir_skips_home_only() {
    use std::path::Path;
    let home = Path::new("/Users/jeremy");
    // Home itself is skipped (Cmd+; already covers home).
    assert!(!super::should_record_last_dir(home, Some(home)));
    // Any other project dir is recorded for Cmd+'.
    assert!(super::should_record_last_dir(
        Path::new("/Users/jeremy/projects/foo"),
        Some(home)
    ));
    // With no known home, always record.
    assert!(super::should_record_last_dir(home, None));
}

#[cfg(target_os = "macos")]
#[test]
fn install_writes_executable_scripts_and_plan() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let resolved = launch_hotkeys::resolve_launch_hotkeys(
        &jcode_config_types::LaunchHotkeysConfig::default(),
        "/usr/local/bin/jcode",
        "/home/u/.jcode/hotkey/last_dir",
        "/home/u/.jcode/hotkey/last_repo",
    );
    let plan = super::write_hotkey_launch_scripts(
        dir.path(),
        MacTerminalKind::Ghostty,
        "/usr/local/bin/jcode",
        &resolved,
    )
    .expect("scripts should write");

    // One plan entry per resolved hotkey, each pointing at an executable bash
    // script that exists on disk.
    assert_eq!(plan.len(), resolved.len());
    for entry in &plan {
        let path = std::path::Path::new(&entry.script);
        let body = std::fs::read_to_string(path).expect("script exists");
        assert!(body.starts_with("#!/bin/bash"));
        let mode = std::fs::metadata(path).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "script should be executable");
    }

    // Only the self-dev (3rd) script invokes the self-dev subcommand.
    let selfdev = std::fs::read_to_string(&plan[2].script).unwrap();
    assert!(selfdev.contains("self-dev"));
    let home = std::fs::read_to_string(&plan[0].script).unwrap();
    assert!(!home.contains("self-dev"));
}

#[test]
fn mac_hotkey_launch_agent_plist_uses_valid_xml_quotes() {
    let plist = mac_hotkey_launch_agent_plist(
        "/Applications/Jcode.app/Contents/MacOS/jcode",
        "/tmp/jcode-hotkey.out.log",
        "/tmp/jcode-hotkey.err.log",
        "ghostty",
    );

    assert!(plist.contains("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    assert!(plist.contains("<plist version=\"1.0\">"));
    assert!(!plist.contains("\\\""));
    assert!(plist.contains("<string>setup-hotkey</string>"));
    assert!(plist.contains("<string>--listen-macos-hotkey</string>"));
    // The listener must load into the GUI (Aqua) session so it has a
    // window-server connection and can receive Carbon hotkey events.
    assert!(plist.contains("<key>LimitLoadToSessionType</key>"));
    assert!(plist.contains("<string>Aqua</string>"));
}

#[test]
fn paused_jcode_shell_command_keeps_failures_visible() {
    let command = paused_jcode_shell_command("/tmp/jcode");
    assert!(command.contains("Press Enter to close"));
    assert!(command.contains("Jcode exited with status"));
    assert!(command.contains("jcode executable not found"));
}

#[test]
fn fresh_user_gets_hotkey_install() {
    let state = SetupHintsState::default();
    assert_eq!(
        mac_hotkey_action_for_state(&state),
        MacHotkeyAction::Install
    );
}

#[test]
fn legacy_configured_user_gets_migrated_on_update() {
    // Configured before the version field existed -> version defaults to 0.
    let state = SetupHintsState {
        hotkey_configured: true,
        hotkey_dismissed: true,
        hotkey_listener_version: 0,
        ..SetupHintsState::default()
    };
    assert_eq!(
        mac_hotkey_action_for_state(&state),
        MacHotkeyAction::Migrate
    );
}

#[test]
fn current_version_user_is_left_alone() {
    let state = SetupHintsState {
        hotkey_configured: true,
        hotkey_dismissed: true,
        hotkey_listener_version: HOTKEY_LISTENER_VERSION,
        ..SetupHintsState::default()
    };
    assert_eq!(mac_hotkey_action_for_state(&state), MacHotkeyAction::None);
}

#[test]
fn previous_listener_version_user_gets_migrated_on_update() {
    // A user who already installed an earlier listener version (e.g. the v1
    // run-loop-only listener that still never fired) must be re-migrated to the
    // current listener on update.
    for old_version in 0..HOTKEY_LISTENER_VERSION {
        let state = SetupHintsState {
            hotkey_configured: true,
            hotkey_dismissed: true,
            hotkey_listener_version: old_version,
            ..SetupHintsState::default()
        };
        assert_eq!(
            mac_hotkey_action_for_state(&state),
            MacHotkeyAction::Migrate,
            "listener version {old_version} should be migrated"
        );
    }
}

#[test]
fn macos_terminal_notice_only_fires_for_default_terminal_app() {
    let mut state = SetupHintsState::default();
    let hints = macos_terminal_notice(&mut state, MacTerminalKind::AppleTerminal)
        .expect("Terminal.app should produce a notice");

    assert_eq!(
        hints.status_notice.as_deref(),
        Some("Tip: Terminal.app renders jcode poorly. Try Ghostty, iTerm2, or Alacritty.")
    );
    let (title, message) = hints.display_message.expect("expected display message");
    assert_eq!(title, "Terminal");
    assert!(message.contains("Terminal.app renders jcode poorly"));
    assert!(message.contains("Ghostty"));
    // It is a plain notice, not an AI handoff prompt.
    assert!(hints.auto_send_message.is_none());
    // The nudge is marked handled so it only ever shows once.
    assert!(state.mac_ghostty_guided);
    assert!(state.mac_ghostty_dismissed);
}

#[test]
fn macos_terminal_notice_silent_for_modern_terminals() {
    for terminal in [
        MacTerminalKind::Ghostty,
        MacTerminalKind::Iterm2,
        MacTerminalKind::WezTerm,
        MacTerminalKind::Warp,
        MacTerminalKind::Alacritty,
        MacTerminalKind::Vscode,
        MacTerminalKind::Unknown,
    ] {
        let mut state = SetupHintsState::default();
        assert!(
            macos_terminal_notice(&mut state, terminal).is_none(),
            "{terminal:?} should not be nudged"
        );
        // Even when silent, the nudge is marked handled so we never re-check it.
        assert!(state.mac_ghostty_guided);
        assert!(state.mac_ghostty_dismissed);
    }
}

#[test]
fn nudge_budget_caps_at_max_and_persists() {
    let mut state = SetupHintsState::default();
    assert_eq!(state.terminal_nudge_count, 0);

    for shown in 1..=MAX_TERMINAL_NUDGES {
        assert!(
            state.nudge_budget_remaining(),
            "should still allow nudge before #{shown}"
        );
        state.terminal_nudge_count = shown;
    }

    // After MAX_TERMINAL_NUDGES, we stop asking even without an explicit dismiss.
    assert_eq!(state.terminal_nudge_count, MAX_TERMINAL_NUDGES);
    assert!(!state.nudge_budget_remaining());
}

#[test]
fn load_from_falls_back_to_bak_when_primary_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("setup_hints.json");
    let bak = dir.path().join("setup_hints.bak");

    std::fs::write(&bak, r#"{"launch_count":42}"#).unwrap();

    // Primary file missing: must recover launch_count from the .bak instead of
    // resetting to default (which would re-trigger first-run onboarding).
    let loaded = SetupHintsState::load_from(&path);
    assert_eq!(loaded.launch_count, 42);
}

#[test]
fn load_from_falls_back_to_bak_when_primary_corrupt_without_inline_recovery() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("setup_hints.json");
    let bak = dir.path().join("setup_hints.bak");

    std::fs::write(&path, b"{not json").unwrap();
    std::fs::write(&bak, r#"{"launch_count":7}"#).unwrap();

    let loaded = SetupHintsState::load_from(&path);
    assert_eq!(loaded.launch_count, 7);
}

#[test]
fn load_from_defaults_when_both_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("setup_hints.json");
    let loaded = SetupHintsState::load_from(&path);
    assert_eq!(loaded.launch_count, 0);
}

#[test]
fn conflict_hint_decision_warns_only_when_conflicts_change() {
    // No conflicts ever: empty == empty => stay silent.
    assert_eq!(
        conflict_hint_decision("", ""),
        ConflictHintDecision::Unchanged
    );

    // New conflicts where there were none: warn.
    assert_eq!(
        conflict_hint_decision("keybindings.model_switch_next|ctrl+tab|ctrl+tab", ""),
        ConflictHintDecision::Warn
    );

    // Same conflicts as last time: stay silent.
    let sig = "keybindings.model_switch_next|ctrl+tab|ctrl+tab";
    assert_eq!(
        conflict_hint_decision(sig, sig),
        ConflictHintDecision::Unchanged
    );

    // Conflicts resolved since last time (had some, now none): update silently.
    assert_eq!(
        conflict_hint_decision("", sig),
        ConflictHintDecision::ResolvedSilently
    );

    // Conflict set changed (different conflicts): warn again.
    assert_eq!(
        conflict_hint_decision("keybindings.scroll_up|ctrl+k|ctrl+k", sig),
        ConflictHintDecision::Warn
    );
}

#[test]
fn keymap_conflict_hint_full_path_debounces_and_persists_signature() {
    use crate::keymap::source::{DiscoveredBinding, KeySource};
    use crate::keymap::{KeyChord, KeymapSnapshot};
    use jcode_config_types::KeybindingsConfig;

    fn snapshot(bindings: Vec<DiscoveredBinding>) -> KeymapSnapshot {
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

    let cfg = KeybindingsConfig::default();
    let mut state = SetupHintsState::default();

    // 1) First time with a real conflict: warn + state changes.
    let conflicting = snapshot(vec![term("ctrl+tab", "next_tab")]);
    let (hint, changed) = keymap_conflict_hint_for(&cfg, &conflicting, &mut state);
    assert!(hint.is_some(), "should warn on first conflict");
    assert!(changed, "state signature should be recorded");
    let (title, body) = hint.unwrap().display_message.unwrap();
    assert_eq!(title, "Keybindings");
    assert!(body.contains("keybindings.model_switch_next"));
    assert!(!state.keymap_conflict_signature.is_empty());

    // 2) Same conflict again: debounced, no state change.
    let (hint2, changed2) = keymap_conflict_hint_for(&cfg, &conflicting, &mut state);
    assert!(hint2.is_none(), "same conflict set must not re-warn");
    assert!(!changed2, "no state change when nothing changed");

    // 3) Conflict resolved (clean snapshot): silent, but signature cleared.
    let clean = snapshot(vec![term("cmd+t", "new_tab")]);
    let (hint3, changed3) = keymap_conflict_hint_for(&cfg, &clean, &mut state);
    assert!(hint3.is_none(), "resolved conflicts show nothing");
    assert!(changed3, "signature should be cleared");
    assert!(state.keymap_conflict_signature.is_empty());
}

#[test]
fn glyph_safe_notice_shows_once_then_debounces() {
    let mut state = SetupHintsState::default();

    // First launch in a fragile terminal: disclose the tradeoff and persist.
    let (hint, changed) = glyph_safe_notice_for(true, &mut state);
    assert!(
        hint.is_some(),
        "should disclose glyph-safe mode on first launch"
    );
    assert!(changed, "state should be marked shown");
    assert!(state.glyph_safe_notice_shown);
    let (title, body) = hint.unwrap().display_message.unwrap();
    assert_eq!(title, "Display");
    assert!(body.contains("quantizes colors"));
    assert!(body.contains("JCODE_GLYPH_SAFE_MODE=off"));

    // Subsequent launches: debounced, no repeat.
    let (hint2, changed2) = glyph_safe_notice_for(true, &mut state);
    assert!(hint2.is_none(), "must not re-disclose on later launches");
    assert!(!changed2);
}

#[test]
fn glyph_safe_notice_silent_on_robust_terminals() {
    let mut state = SetupHintsState::default();
    let (hint, changed) = glyph_safe_notice_for(false, &mut state);
    assert!(
        hint.is_none(),
        "no disclosure when glyph-safe mode is inactive"
    );
    assert!(!changed);
    assert!(!state.glyph_safe_notice_shown);
}

fn row(chord: &str, label: &str, self_dev: bool) -> LaunchHotkeyRow {
    LaunchHotkeyRow {
        chord: chord.to_string(),
        label: label.to_string(),
        cwd_display: format!("/repos/{label}"),
        self_dev,
    }
}

#[test]
fn launch_hotkey_notice_lists_all_unlearned_bindings() {
    let rows = vec![
        row("cmd+;", "home", false),
        row("cmd+'", "last project", false),
        row("cmd+shift+'", "self-dev", true),
    ];
    let usage = std::collections::HashMap::new();
    let lines = launch_hotkey_notice_lines(&rows, &usage, 1).expect("should show all bindings");
    assert_eq!(lines.len(), 3);
    assert!(lines[0].starts_with("cmd+; → home (/repos/home)"));
    assert!(lines[2].ends_with("[self-dev]"));
}

#[test]
fn launch_hotkey_notice_hides_individually_learned_bindings() {
    let rows = vec![
        row("cmd+;", "home", false),
        row("cmd+'", "last project", false),
    ];
    let mut usage = std::collections::HashMap::new();
    // cmd+; used enough to be considered learned; cmd+' still new.
    usage.insert("cmd+;".to_string(), LAUNCH_HOTKEY_LEARNED_USES);
    let lines = launch_hotkey_notice_lines(&rows, &usage, 3).expect("one binding still new");
    assert_eq!(lines.len(), 1);
    assert!(lines[0].starts_with("cmd+' → last project"));
}

#[test]
fn launch_hotkey_notice_stops_once_learned_and_experienced() {
    let rows = vec![
        row("cmd+;", "home", false),
        row("cmd+'", "last project", false),
    ];
    let mut usage = std::collections::HashMap::new();
    usage.insert("cmd+;".to_string(), LAUNCH_HOTKEY_LEARNED_USES);
    // Learned at least one binding AND launched enough overall -> stop entirely,
    // even though cmd+' was never used.
    assert!(
        launch_hotkey_notice_lines(&rows, &usage, LAUNCH_HOTKEY_NOTICE_MIN_LAUNCHES_TO_STOP)
            .is_none()
    );
}

#[test]
fn launch_hotkey_notice_keeps_showing_for_new_user_with_many_launches() {
    // Many launches but no binding learned yet: keep showing so they can adopt it.
    let rows = vec![row("cmd+;", "home", false)];
    let usage = std::collections::HashMap::new();
    let lines =
        launch_hotkey_notice_lines(&rows, &usage, 50).expect("never learned -> keep showing");
    assert_eq!(lines.len(), 1);
}
