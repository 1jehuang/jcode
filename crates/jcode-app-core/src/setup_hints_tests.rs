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
