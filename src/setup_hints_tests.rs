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
}

#[test]
fn paused_jcode_shell_command_keeps_failures_visible() {
    let command = paused_jcode_shell_command("/tmp/jcode");
    assert!(command.contains("Press Enter to close"));
    assert!(command.contains("Jcode exited with status"));
    assert!(command.contains("jcode executable not found"));
}
