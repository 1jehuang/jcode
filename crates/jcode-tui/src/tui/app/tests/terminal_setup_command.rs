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
fn decode_key_event_via_pty(bytes: &[u8]) -> (crossterm::event::KeyCode, u8) {
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

    // Configure the PTY before feeding input, not only after the child starts.
    // Otherwise ICRNL can turn a queued CR into LF, which crossterm decodes as
    // Ctrl+J once the child enables raw mode.
    // Safety: device is a valid PTY fd; termios is initialized before use.
    unsafe {
        let mut mode = std::mem::MaybeUninit::<libc::termios>::uninit();
        assert_eq!(libc::tcgetattr(device, mode.as_mut_ptr()), 0);
        let mut mode = mode.assume_init();
        libc::cfmakeraw(&mut mode);
        assert_eq!(libc::tcsetattr(device, libc::TCSANOW, &mode), 0);
    }

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
                    format!("{:?},{}", key.code, key.modifiers.bits())
                }
                other => format!("unexpected read result: {other:?}"),
            },
            Err(err) => format!("cannot enable raw mode: {err}"),
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
    read_end
        .read_to_string(&mut payload)
        .expect("read pty result");
    // Safety: reaping the child we just forked.
    unsafe {
        let mut status: libc::c_int = 0;
        assert_eq!(libc::waitpid(pid, &mut status, 0), pid, "reap pty child");
        assert_eq!(status, 0, "pty child failed: {status}");
    }

    let (code, bits) = payload
        .split_once(',')
        .unwrap_or_else(|| panic!("invalid pty result: {payload:?}"));
    assert_eq!(code, "Enter", "unexpected decoded key: {payload}");
    (
        crossterm::event::KeyCode::Enter,
        bits.parse().expect("key modifiers"),
    )
}

#[test]
#[cfg(unix)]
fn shift_enter_csi_u_sequence_decodes_to_enter_plus_shift() {
    use crossterm::event::{KeyCode, KeyModifiers};

    // Exactly the bytes `terminal_setup` writes into terminal configurations.
    let sequence = crate::tui::terminal_setup::SHIFT_ENTER_CSI_U;
    let (code, bits) = decode_key_event_via_pty(sequence.as_bytes());

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
    let (code, bits) = decode_key_event_via_pty(b"\r");

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
    const CHILD: &str = "JCODE_TEST_TERMINAL_SETUP_CHILD";
    const VERIFIED: &str = "TERMINAL_SETUP_DISPATCH_VERIFIED";
    if std::env::var_os(CHILD).is_none() {
        // HOME and terminal detection must not leak into concurrent tests or
        // let this command rewrite the developer's actual terminal config.
        #[cfg(unix)]
        let targets = ["Apple_Terminal", "WezTerm", "tmux"];
        // Windows resolves HOME via a known folder, not the HOME variable.
        // Keep its dispatch check on the non-writing manual-guidance path.
        #[cfg(not(unix))]
        let targets = ["Apple_Terminal"];
        for target in targets {
            let home = tempfile::tempdir().expect("isolated terminal home");
            let test_name = std::thread::current().name().unwrap().to_owned();
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", &test_name, "--nocapture"])
                .env_clear()
                .env(CHILD, target)
                .env("HOME", home.path())
                .env("JCODE_HOME", home.path().join(".jcode"))
                .env("TMPDIR", home.path())
                .env("TERM", "dumb")
                .env("TERM_PROGRAM", target);
            if target == "tmux" {
                command.env("TMUX", "test-fixture");
            }
            #[cfg(windows)]
            if let Some(system_root) = std::env::var_os("SystemRoot") {
                command.env("SystemRoot", system_root);
            }
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                // Safety: setsid is async-signal-safe and prevents the child
                // from querying the developer's controlling terminal.
                unsafe {
                    command.pre_exec(|| {
                        if libc::setsid() == -1 {
                            return Err(std::io::Error::last_os_error());
                        }
                        Ok(())
                    });
                }
            }
            let output = command.output().expect("isolated terminal command test");
            assert!(
                output.status.success(),
                "{target} command regression failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                String::from_utf8_lossy(&output.stdout).contains(VERIFIED),
                "{target} child did not execute the command assertions"
            );
        }
        return;
    }

    fn dispatch() -> String {
        let mut app = create_test_app();
        app.set_input_for_test("/terminal-setup");
        app.submit_input();
        let content = app
            .display_messages()
            .last()
            .expect("/terminal-setup must produce a message")
            .content
            .clone();
        assert!(
            !content.contains("Unknown"),
            "unregistered command: {content}"
        );
        content
    }

    let target = std::env::var(CHILD).unwrap();
    #[cfg(unix)]
    assert_eq!(
        dirs::home_dir(),
        Some(std::path::PathBuf::from(std::env::var_os("HOME").unwrap())),
        "refuse to dispatch against a home outside the isolated fixture"
    );
    let first = dispatch();
    assert!(first.contains("Shift+Enter"), "missing guidance: {first}");
    if target != "Apple_Terminal" {
        use crate::tui::terminal_setup::{TMUX_CONFIG_BLOCK, WEZTERM_CONFIG_LINE};

        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        let path = home.join(if target == "tmux" {
            ".tmux.conf"
        } else {
            ".wezterm.lua"
        });
        assert!(first.contains(&format!("Configured {target}")), "{first}");
        let created = std::fs::read_to_string(&path).unwrap();
        if target == "tmux" {
            assert_eq!(created, TMUX_CONFIG_BLOCK);
        } else {
            assert!(created.contains(WEZTERM_CONFIG_LINE));
            assert!(created.trim_end().ends_with("return config"));
        }
        assert!(dispatch().contains("already configured for Shift+Enter"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), created);

        let custom = if target == "tmux" {
            "set -g mouse on\n"
        } else {
            "return { font_size = 12 }\n"
        };
        std::fs::write(&path, custom).unwrap();
        let manual = dispatch();
        assert!(manual.contains("Shift+Enter"), "{manual}");
        let updated = std::fs::read_to_string(&path).unwrap();
        if target == "tmux" {
            assert!(updated.starts_with(custom));
            assert!(updated.ends_with(TMUX_CONFIG_BLOCK));
        } else {
            assert!(manual.contains(WEZTERM_CONFIG_LINE), "{manual}");
            assert_eq!(updated, custom);
        }

        let invalid = b"user settings\n\xff\n";
        std::fs::write(&path, invalid).unwrap();
        let error = dispatch();
        assert!(
            error.starts_with(&format!("Failed to update {target} configuration:")),
            "{error}"
        );
        assert!(error.contains("UTF-8"), "{error}");
        assert_eq!(std::fs::read(&path).unwrap(), invalid);

        // A directory is another read error, never an absent/empty config.
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("sentinel"), "keep").unwrap();
        let error = dispatch();
        assert!(
            error.starts_with(&format!("Failed to update {target} configuration:")),
            "{error}"
        );
        assert_eq!(
            std::fs::read_to_string(path.join("sentinel")).unwrap(),
            "keep"
        );
    }
    println!("{VERIFIED}:{target}");
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
