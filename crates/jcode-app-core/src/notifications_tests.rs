use super::*;

/// A bare `display notification` is attributed to Script Editor, so
/// clicking the banner opens Script Editor instead of the user's session.
/// Every macOS notification must be wrapped in `tell application id`.
#[cfg(target_os = "macos")]
#[test]
fn macos_notification_script_is_attributed_to_host_terminal() {
    let script = build_macos_notification_script(
        "jcode: done",
        Some("my-session"),
        "turn complete",
        Some("Glass"),
        Some("com.mitchellh.ghostty"),
    );
    assert_eq!(
        script,
        "tell application id \"com.mitchellh.ghostty\" to \
         display notification \"turn complete\" with title \"jcode: done\" \
         subtitle \"my-session\" sound name \"Glass\""
    );
}

/// Without a resolvable terminal we must still post the notification,
/// just unattributed, rather than emitting a broken `tell` block.
#[cfg(target_os = "macos")]
#[test]
fn macos_notification_script_without_bundle_id_is_unwrapped() {
    let script = build_macos_notification_script("t", None, "b", None, None);
    assert_eq!(script, "display notification \"b\" with title \"t\"");
    let blank = build_macos_notification_script("t", None, "b", None, Some("  "));
    assert_eq!(blank, "display notification \"b\" with title \"t\"");
}

/// Quotes/backslashes in session names or assistant text must not be able
/// to break out of the AppleScript string literal.
#[cfg(target_os = "macos")]
#[test]
fn macos_notification_script_escapes_injection_attempts() {
    let script = build_macos_notification_script(
        "ti\"tle",
        None,
        "bo\\dy\" & (do shell script \"boom\")",
        None,
        Some("com.apple.Terminal"),
    );
    assert!(script.starts_with("tell application id \"com.apple.Terminal\" to "));
    assert!(script.contains("\\\"tle"));
    assert!(script.contains("bo\\\\dy\\\""));
    // The real invariant: only the six literal delimiters (bundle id,
    // body, title) survive as unescaped quotes, so hostile text cannot
    // terminate a string and append its own AppleScript.
    let delimiters = script
        .replace("\\\\", "")
        .replace("\\\"", "")
        .matches('"')
        .count();
    assert_eq!(delimiters, 6, "unbalanced quotes in script: {script}");
}

/// End-to-end guard: the generated script must actually compile as
/// AppleScript, proving the `tell` wrapper and escaping are well formed.
#[cfg(target_os = "macos")]
#[test]
fn macos_notification_script_compiles_as_applescript() {
    let script = build_macos_notification_script(
        "ti\"tle",
        Some("sub\\title"),
        "bo\"dy & (do shell script \"boom\")",
        Some("Glass"),
        Some("com.apple.Terminal"),
    );
    // `osacompile` parses without executing, so no notification is posted.
    let output = std::process::Command::new("osacompile")
        .args(["-o", "/dev/null", "-e", &script])
        .output();
    let Ok(output) = output else {
        return; // osacompile unavailable; nothing to assert.
    };
    assert!(
        output.status.success(),
        "script failed to compile: {script}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The launching app's bundle id wins, since it names the exact terminal
/// that started this process.
#[cfg(target_os = "macos")]
#[test]
fn host_terminal_bundle_id_prefers_launching_app() {
    assert_eq!(
        resolve_macos_host_terminal_bundle_id(Some("com.mitchellh.ghostty"), Some("terminal")),
        Some("com.mitchellh.ghostty".to_string())
    );
}

/// `osascript` launched from a shell reports Script Editor's helper id.
/// Trusting it would reintroduce the Script Editor bug, so it is ignored
/// in favor of terminal detection.
#[cfg(target_os = "macos")]
#[test]
fn host_terminal_bundle_id_ignores_script_editor_helper() {
    assert_eq!(
        resolve_macos_host_terminal_bundle_id(
            Some("com.apple.Terminal.osascript"),
            Some("ghostty")
        ),
        Some("com.mitchellh.ghostty".to_string())
    );
}

#[cfg(target_os = "macos")]
#[test]
fn host_terminal_bundle_id_falls_back_to_detected_terminal() {
    for (terminal, expected) in [
        ("ghostty", "com.mitchellh.ghostty"),
        ("kitty", "net.kovidgoyal.kitty"),
        ("wezterm", "com.github.wez.wezterm"),
        ("alacritty", "org.alacritty"),
        ("iterm2", "com.googlecode.iterm2"),
        ("terminal", "com.apple.Terminal"),
        ("handterm", "com.jcode.handterm"),
    ] {
        assert_eq!(
            resolve_macos_host_terminal_bundle_id(None, Some(terminal)),
            Some(expected.to_string()),
            "terminal {terminal} should map to {expected}"
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
fn host_terminal_bundle_id_is_none_when_unknown() {
    assert_eq!(
        resolve_macos_host_terminal_bundle_id(Some(""), Some("some-unknown-term")),
        None
    );
    assert_eq!(resolve_macos_host_terminal_bundle_id(None, None), None);
}

#[test]
fn test_format_cycle_body_safe() {
    let transcript = AmbientTranscript {
        session_id: "test_001".to_string(),
        started_at: chrono::Utc::now(),
        ended_at: Some(chrono::Utc::now()),
        status: crate::safety::TranscriptStatus::Complete,
        provider: "claude".to_string(),
        model: "claude-sonnet-4".to_string(),
        actions: Vec::new(),
        pending_permissions: 0,
        summary: Some("Cleaned up 3 stale memories.".to_string()),
        compactions: 1,
        memories_modified: 3,
        conversation: None,
    };

    let body = format_cycle_body_safe(&transcript);
    assert!(body.contains("Memories modified: 3"));
    assert!(body.contains("Compactions: 1"));
    assert!(body.contains("Check jcode for full details"));
    // Safe body must NOT include model-generated summary
    assert!(!body.contains("Cleaned up"));
    assert!(!body.contains("permission"));
}

#[test]
fn test_format_cycle_body_detailed() {
    let transcript = AmbientTranscript {
        session_id: "test_001".to_string(),
        started_at: chrono::Utc::now(),
        ended_at: Some(chrono::Utc::now()),
        status: crate::safety::TranscriptStatus::Complete,
        provider: "claude".to_string(),
        model: "claude-sonnet-4".to_string(),
        actions: Vec::new(),
        pending_permissions: 0,
        summary: Some("Cleaned up 3 stale memories.".to_string()),
        compactions: 1,
        memories_modified: 3,
        conversation: Some("### User\n\nBegin cycle.\n\n### Assistant\n\nDone.\n".to_string()),
    };

    let body = format_cycle_body_detailed(&transcript);
    // Detailed body SHOULD include the summary
    assert!(body.contains("Cleaned up 3 stale memories."));
    assert!(body.contains("**Memories:** 3"));
    assert!(body.contains("claude"));
    // Should include conversation transcript
    assert!(body.contains("# Full Transcript"));
    assert!(body.contains("### User"));
    assert!(body.contains("Begin cycle."));
}

#[test]
fn test_format_cycle_body_with_pending_permissions() {
    let transcript = AmbientTranscript {
        session_id: "test_002".to_string(),
        started_at: chrono::Utc::now(),
        ended_at: Some(chrono::Utc::now()),
        status: crate::safety::TranscriptStatus::Complete,
        provider: "claude".to_string(),
        model: "claude-sonnet-4".to_string(),
        actions: Vec::new(),
        pending_permissions: 2,
        summary: None,
        compactions: 0,
        memories_modified: 0,
        conversation: None,
    };

    let safe = format_cycle_body_safe(&transcript);
    assert!(safe.contains("2 permission request(s) pending"));
    assert!(safe.contains("Check jcode for full details"));

    let detailed = format_cycle_body_detailed(&transcript);
    assert!(detailed.contains("2 permission request(s) pending"));
}

#[test]
fn test_priority_values() {
    assert_eq!(Priority::Default.ntfy_value(), "3");
    assert_eq!(Priority::High.ntfy_value(), "4");
    assert_eq!(Priority::Urgent.ntfy_value(), "5");
}

#[test]
fn test_dispatcher_creation() {
    // Just verify it doesn't panic
    let cfg = SafetyConfig::default();
    let _dispatcher = NotificationDispatcher::from_config(cfg);
}

#[test]
fn macos_origin_detects_terminal_identifiers() {
    let terminal = MacosNotificationOrigin::from_values(
        "Apple_Terminal",
        "xterm-256color",
        Some("/dev/ttys007"),
        Some("4F3C"),
        None,
        false,
    );
    assert_eq!(terminal.terminal, MacosTerminalKind::AppleTerminal);
    assert_eq!(terminal.bundle_id.as_deref(), Some("com.apple.Terminal"));
    assert_eq!(terminal.tty.as_deref(), Some("/dev/ttys007"));
    assert_eq!(terminal.session_id.as_deref(), Some("4F3C"));

    let iterm = MacosNotificationOrigin::from_values(
        "iTerm.app",
        "xterm-256color",
        Some("/dev/ttys011"),
        None,
        Some("w0t1p0:ABC"),
        false,
    );
    assert_eq!(iterm.terminal, MacosTerminalKind::Iterm2);
    assert_eq!(iterm.session_id.as_deref(), Some("w0t1p0:ABC"));

    let ghostty = MacosNotificationOrigin::from_values(
        "",
        "xterm-ghostty",
        Some("/dev/ttys019"),
        None,
        None,
        true,
    );
    assert_eq!(ghostty.terminal, MacosTerminalKind::Ghostty);
    assert_eq!(ghostty.bundle_id.as_deref(), Some("com.mitchellh.ghostty"));
}

#[test]
fn macos_origin_rejects_untrusted_route_values() {
    let origin = MacosNotificationOrigin::from_values(
        "Apple_Terminal",
        "",
        Some("/dev/ttys001\"\nrun script"),
        Some("bad\nidentifier"),
        None,
        false,
    );
    assert_eq!(origin.tty, None);
    assert_eq!(origin.session_id, None);

    let (_, args) = macos_notification_activation_command(&origin).expect("Terminal route");
    assert_eq!(
        args,
        vec!["-e", "tell application \"Terminal\" to activate"]
    );
}

#[test]
fn macos_activation_targets_terminal_and_iterm_ttys() {
    let terminal = MacosNotificationOrigin {
        terminal: MacosTerminalKind::AppleTerminal,
        bundle_id: Some("com.apple.Terminal".to_string()),
        tty: Some("/dev/ttys003".to_string()),
        session_id: Some("session-a".to_string()),
    };
    let (program, args) =
        macos_notification_activation_command(&terminal).expect("Terminal command");
    assert_eq!(program, "/usr/bin/osascript");
    assert!(args[1].contains("if tty of t is \"/dev/ttys003\""));
    assert!(args[1].contains("set selected tab of w to t"));

    let iterm = MacosNotificationOrigin {
        terminal: MacosTerminalKind::Iterm2,
        bundle_id: Some("com.googlecode.iterm2".to_string()),
        tty: Some("/dev/ttys004".to_string()),
        session_id: Some("w0t0p0:guid".to_string()),
    };
    let (_, args) = macos_notification_activation_command(&iterm).expect("iTerm command");
    assert!(args[1].contains("if tty of s is \"/dev/ttys004\""));
    assert!(args[1].contains("select s"));
}

#[test]
fn macos_ghostty_activation_is_application_scoped() {
    let origin = MacosNotificationOrigin {
        terminal: MacosTerminalKind::Ghostty,
        bundle_id: Some("evil.bundle".to_string()),
        tty: Some("/dev/ttys005".to_string()),
        session_id: None,
    };
    assert_eq!(
        macos_notification_activation_command(&origin),
        Some((
            "/usr/bin/open".to_string(),
            vec!["-b".to_string(), "com.mitchellh.ghostty".to_string()]
        ))
    );
}

#[test]
fn macos_envelope_roundtrip_preserves_origin_metadata() {
    let envelope = MacosNotificationEnvelope {
        schema_version: MACOS_NOTIFICATION_SCHEMA_VERSION,
        notification_id: "jcode-turn-test".to_string(),
        title: "jcode · done".to_string(),
        subtitle: Some("2/2 todos".to_string()),
        body: "Finished broker".to_string(),
        sound: Some("Glass".to_string()),
        origin: MacosNotificationOrigin {
            terminal: MacosTerminalKind::Iterm2,
            bundle_id: Some("com.googlecode.iterm2".to_string()),
            tty: Some("/dev/ttys009".to_string()),
            session_id: Some("w1t2p0:route".to_string()),
        },
    };
    let encoded = serde_json::to_vec(&envelope).expect("encode envelope");
    let decoded: MacosNotificationEnvelope =
        serde_json::from_slice(&encoded).expect("decode envelope");
    assert_eq!(decoded, envelope);
}
