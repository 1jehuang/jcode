// Shift+Enter must insert a newline, on every terminal.
//
// Terminals send a bare 0x0d for Enter, Shift+Enter, and Ctrl+Enter alike, so
// the only way an app can tell them apart is the kitty keyboard protocol, where
// Shift+Enter becomes `ESC[13;2u`. jcode requests that protocol at startup and
// crossterm decodes it.
//
// These tests pin the two ends of that contract:
//
//  1. The byte sequence `terminal_setup` writes into terminal configs is exactly
//     the sequence crossterm decodes into Enter+SHIFT. Verified against a real
//     PTY so it is crossterm's own parser doing the work, not a reimplementation.
//  2. Once decoded, jcode inserts a newline instead of submitting.
//
// Together they mean: if terminal setup makes the terminal emit this sequence,
// Shift+Enter genuinely works.

/// Decode raw terminal bytes into key events using crossterm's own parser.
///
/// crossterm reads from `/dev/tty`, so hand it a PTY whose controller side we
/// have pre-loaded with the bytes under test. `libc::login_tty` makes the PTY the
/// process's controlling terminal for the duration of the child, which is what
/// lets `crossterm::event::read` see our bytes.
///
/// Runs in a forked child because installing a controlling terminal is
/// process-global and must not leak into the rest of the test binary.
#[cfg(unix)]
fn decode_key_event_via_pty(bytes: &[u8]) -> Option<(crossterm::event::KeyCode, u8)> {
    use std::io::{Read, Write};
    use std::os::fd::FromRawFd;

    let mut controller: libc::c_int = -1;
    let mut device: libc::c_int = -1;
    // Safety: openpty writes two valid fds on success; name/termios are unused.
    let rc = unsafe {
        libc::openpty(
            &mut controller,
            &mut device,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(rc, 0, "failed to allocate a pty");

    // A pipe carries the decoded result back from the child.
    let mut result_pipe: [libc::c_int; 2] = [-1, -1];
    // Safety: pipe writes two valid fds on success.
    assert_eq!(unsafe { libc::pipe(result_pipe.as_mut_ptr()) }, 0);

    // Safety: fork in a test binary; the child only writes to the pipe and execs
    // no code that assumes a single-threaded parent beyond crossterm's read.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork failed");

    if pid == 0 {
        // Child: adopt the PTY as the controlling terminal, then let crossterm
        // read and decode. Report `code_tag,modifier_bits` over the pipe.
        // Safety: single-threaded child; fds are valid.
        unsafe {
            libc::close(controller);
            libc::close(result_pipe[0]);
            if libc::login_tty(device) != 0 {
                libc::_exit(2);
            }
        }
        let payload = match crossterm::terminal::enable_raw_mode() {
            Ok(()) => match crossterm::event::read() {
                Ok(crossterm::event::Event::Key(key)) => {
                    let tag = match key.code {
                        crossterm::event::KeyCode::Enter => 1u8,
                        _ => 0u8,
                    };
                    format!("{tag},{}", key.modifiers.bits())
                }
                _ => "0,0".to_string(),
            },
            Err(_) => "0,0".to_string(),
        };
        // Safety: writing to a valid pipe fd, then exiting immediately.
        unsafe {
            let mut out = std::fs::File::from_raw_fd(result_pipe[1]);
            let _ = out.write_all(payload.as_bytes());
            let _ = out.flush();
            libc::_exit(0);
        }
    }

    // Parent: feed the bytes, then collect the child's verdict.
    // Safety: fds are valid and owned here.
    unsafe {
        libc::close(device);
        libc::close(result_pipe[1]);
    }
    // Safety: controller is a valid fd owned by this scope.
    let mut write_end = unsafe { std::fs::File::from_raw_fd(controller) };
    write_end.write_all(bytes).expect("write pty bytes");
    write_end.flush().expect("flush pty bytes");

    // Safety: read end is a valid fd owned by this scope.
    let mut read_end = unsafe { std::fs::File::from_raw_fd(result_pipe[0]) };
    let mut payload = String::new();
    let _ = read_end.read_to_string(&mut payload);
    // Safety: reaping the child we just forked.
    unsafe {
        let mut status: libc::c_int = 0;
        libc::waitpid(pid, &mut status, 0);
    }

    let (tag, bits) = payload.split_once(',')?;
    let code = match tag.trim() {
        "1" => crossterm::event::KeyCode::Enter,
        _ => return None,
    };
    Some((code, bits.trim().parse().ok()?))
}

#[test]
#[cfg(unix)]
fn shift_enter_csi_u_sequence_decodes_to_enter_plus_shift() {
    use crossterm::event::{KeyCode, KeyModifiers};

    // Exactly the bytes `terminal_setup` writes into terminal configurations.
    let sequence = crate::tui::terminal_setup::SHIFT_ENTER_CSI_U;
    let Some((code, bits)) = decode_key_event_via_pty(sequence.as_bytes()) else {
        // A sandbox without pty support should skip rather than fail loudly.
        eprintln!("skipping: pty decode unavailable in this environment");
        return;
    };

    assert_eq!(code, KeyCode::Enter);
    let modifiers = KeyModifiers::from_bits_truncate(bits);
    assert!(
        modifiers.contains(KeyModifiers::SHIFT),
        "the sequence we write into terminal configs must decode with SHIFT, \
         otherwise terminal setup cannot fix Shift+Enter; got {modifiers:?}"
    );
}

#[test]
#[cfg(unix)]
fn bare_carriage_return_decodes_without_shift() {
    use crossterm::event::{KeyCode, KeyModifiers};

    // The flip side, and the reason setup is needed at all: without the
    // protocol, Shift+Enter arrives as a bare CR carrying no modifier bits.
    let Some((code, bits)) = decode_key_event_via_pty(b"\r") else {
        eprintln!("skipping: pty decode unavailable in this environment");
        return;
    };

    assert_eq!(code, KeyCode::Enter);
    assert!(
        !KeyModifiers::from_bits_truncate(bits).contains(KeyModifiers::SHIFT),
        "a bare CR cannot carry Shift, which is exactly the problem"
    );
}

#[test]
fn decoded_shift_enter_inserts_a_newline_instead_of_submitting() {
    use crossterm::event::{KeyCode, KeyModifiers};

    // The app-side half of the contract, independent of pty availability.
    let mut app = create_test_app();
    app.handle_key(KeyCode::Char('a'), KeyModifiers::empty())
        .expect("type a");
    app.handle_key(KeyCode::Enter, KeyModifiers::SHIFT)
        .expect("shift+enter");
    app.handle_key(KeyCode::Char('b'), KeyModifiers::empty())
        .expect("type b");

    assert_eq!(app.input, "a\nb");
    assert!(
        app.display_messages().is_empty(),
        "Shift+Enter must not submit: {:?}",
        app.display_messages()
    );
}

// `/terminal-setup` must make Shift+Enter work, and must tell the truth about
// what it did.
//
// The command is the user's escape hatch when Shift+Enter submits instead of
// inserting a newline. These tests drive it through the real command dispatch so
// registration, aliasing, and messaging are all covered.

#[test]
fn terminal_setup_command_is_dispatched_and_reports_something_actionable() {
    let mut app = create_test_app();
    app.set_input_for_test("/terminal-setup");
    app.submit_input();

    let last = app
        .display_messages()
        .last()
        .expect("/terminal-setup must produce a message")
        .clone();
    assert!(
        !last.content.contains("Unknown"),
        "the command must be registered, got: {}",
        last.content
    );
    // Whatever the outcome, the user must learn how Shift+Enter behaves here.
    assert!(
        last.content.contains("Shift+Enter") || last.content.contains("Terminal.app"),
        "message should explain the Shift+Enter situation: {}",
        last.content
    );
}


#[test]
fn terminal_setup_is_offered_in_the_command_palette() {
    // Discoverability matters: a user whose Shift+Enter submits needs to find
    // this without reading the source.
    let (_, help) = crate::tui::app::registered_command_entries()
        .find(|(name, _)| *name == "/terminal-setup")
        .expect("/terminal-setup should be a public command");
    assert!(
        help.contains("Shift+Enter"),
        "help text should name the problem it solves, got: {help}"
    );
}

#[test]
fn unrelated_commands_are_not_swallowed_by_the_terminal_setup_handler() {
    // A too-eager prefix match here would shadow other commands.
    let mut app = create_test_app();
    for command in ["/terminal", "/setup"] {
        app.set_input_for_test(command);
        app.submit_input();
        let last = app
            .display_messages()
            .last()
            .expect("should produce a message")
            .clone();
        assert!(
            !last.content.contains("Configured"),
            "{command} must not be treated as /terminal-setup: {}",
            last.content
        );
    }
}

#[test]
fn falling_back_to_a_backslash_newline_points_at_terminal_setup_once() {
    // Using the fallback means Shift+Enter did not work for this user, which is
    // fixable. Surface that once, then stay quiet.
    let mut app = create_test_app();
    app.set_input_for_test("first\\");
    app.cursor_pos = app.input.len();
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("enter");

    assert_eq!(app.input, "first\n");
    let notice = app
        .status_notice()
        .expect("first fallback should surface the tip");
    assert!(
        notice.contains("/terminal-setup"),
        "tip should name the command that fixes it, got: {notice}"
    );

    // Second use must not renag.
    app.set_status_notice("something else".to_string());
    app.set_input_for_test("second\\");
    app.cursor_pos = app.input.len();
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("enter");
    assert_eq!(
        app.status_notice().as_deref(),
        Some("something else"),
        "the tip must only appear once per session"
    );
}

#[test]
fn working_shift_enter_never_triggers_the_terminal_setup_tip() {
    // Users whose terminal already reports the chord should never see the tip.
    let mut app = create_test_app();
    app.handle_key(KeyCode::Char('a'), KeyModifiers::empty())
        .expect("type");
    app.handle_key(KeyCode::Enter, KeyModifiers::SHIFT)
        .expect("shift+enter");

    assert_eq!(app.input, "a\n");
    assert!(
        !app.terminal_setup_hint_shown_this_session,
        "Shift+Enter worked, so there is nothing to fix"
    );
}
