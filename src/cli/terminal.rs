use anyhow::Result;
use std::io::{self, IsTerminal, Write};
use std::panic;

use crate::{id, session, telemetry, tui};

pub struct TuiRuntimeState {
    mouse_capture: bool,
    keyboard_enhanced: bool,
    focus_change: bool,
}

pub fn set_current_session(session_id: &str) {
    crate::set_current_session(session_id);
}

pub fn get_current_session() -> Option<String> {
    crate::get_current_session()
}

/// Defensive terminal reset bytes for issue #158.
///
/// Sent after `ratatui::restore()` (and after the equivalent in the signal
/// handler). Each sequence covers a state ratatui itself doesn't reset:
///
/// - `ESC [ r` — Reset scroll region (DECSTBM). Without this, some terminals
///   constrain typed input rendering to the alt-screen region even after
///   LeaveAlternateScreen, which is exactly the reported "input invisible" symptom.
/// - `ESC [ ?25 h` — Show cursor (defensive duplicate of crossterm `Show`).
/// - `ESC [ ?2004 l` — Disable bracketed paste (defensive duplicate).
///
/// Returning the bytes (rather than writing inline) keeps the sequence
/// testable.
fn defensive_terminal_reset_bytes() -> &'static [u8] {
    b"\x1b[r\x1b[?25h\x1b[?2004l"
}

/// Same as `defensive_terminal_reset_bytes` but adds DisableFocusChange
/// (`ESC [ ?1004 l`). Used by the signal-handler path which doesn't track
/// which modes were enabled, so it has to disable everything blind.
fn defensive_terminal_reset_bytes_signal() -> &'static [u8] {
    b"\x1b[r\x1b[?25h\x1b[?2004l\x1b[?1004l"
}

pub fn install_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        // Issue #105: the std default panic hook calls eprintln!, which
        // panics if stderr writes fail (broken pipe, EBADF, closed fd).
        // A panic inside a panic hook aborts the process before our
        // session-recovery cleanup runs, leaving sessions in 'Active'
        // state forever. Run the default hook inside catch_unwind so
        // a stderr write failure is degraded to a silent skip rather
        // than a process abort.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            default_hook(info);
        }));

        if let Some(session_id) = get_current_session() {
            // print_session_resume_hint is already write-tolerant.
            print_session_resume_hint(&session_id);

            let provider_model = telemetry::current_provider_model();

            if let Some((provider, model)) = provider_model.as_ref() {
                telemetry::record_crash(provider, model, telemetry::SessionEndReason::Panic);
            }

            if let Ok(mut session) = session::Session::load(&session_id) {
                session.mark_crashed(Some(format!("Panic: {}", info)));
                let _ = session.save();
            }

            // Issue #162: write a structured crash log to ~/.jcode/logs/
            // so users have a breadcrumb to share with maintainers when
            // jcode unexpectedly drops back to the shell. Best-effort —
            // wrapped in catch_unwind so a write failure doesn't trigger
            // a double panic before the rest of cleanup runs.
            let panic_msg = format!("{info}");
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if let Some(path) = crate::crash_log::write_crash_log(
                    &panic_msg,
                    Some(&session_id),
                    provider_model
                        .as_ref()
                        .map(|(p, m)| (p.as_str(), m.as_str())),
                ) {
                    let mut stderr = io::stderr().lock();
                    let _ = writeln!(stderr, "\x1b[33m   Crash log:\x1b[0m   {}", path.display());
                }
            }));
        }
    }));
}

pub fn mark_current_session_crashed(message: String) {
    if let Some(session_id) = get_current_session() {
        if let Some((provider, model)) = telemetry::current_provider_model() {
            telemetry::record_crash(&provider, &model, telemetry::SessionEndReason::Signal);
        }
        if let Ok(mut session) = session::Session::load(&session_id)
            && matches!(session.status, session::SessionStatus::Active)
        {
            session.mark_crashed(Some(message));
            let _ = session.save();
        }
    }
}

pub fn panic_payload_to_string(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

pub fn show_crash_resume_hint() {
    // Issue #105: write through write_crash_resume_hint so any stderr write
    // failure (broken pipe, EBADF) is silently dropped rather than panicking
    // before jcode has finished initializing. This is called at startup, so a
    // panic here would abort the process before set_hook even runs.
    let _ = write_crash_resume_hint(io::stderr().lock());
}

fn write_crash_resume_hint(mut writer: impl Write) -> io::Result<()> {
    let crashed = session::find_recent_crashed_sessions();
    if crashed.is_empty() {
        return Ok(());
    }

    let (id, name) = &crashed[0];
    let session_label = id::extract_session_name(id).unwrap_or(name.as_str());

    if crashed.len() == 1 {
        writeln!(
            writer,
            "\x1b[33m💥 Session \x1b[1m{}\x1b[0m\x1b[33m crashed. Resume with:\x1b[0m  jcode --resume {}",
            session_label, id
        )?;
    } else {
        writeln!(
            writer,
            "\x1b[33m💥 {} sessions crashed recently. Most recent: \x1b[1m{}\x1b[0m",
            crashed.len(),
            session_label
        )?;
        writeln!(
            writer,
            "\x1b[33m   Resume with:\x1b[0m  jcode --resume {}",
            id
        )?;
        writeln!(writer, "\x1b[33m   List all:\x1b[0m     jcode --resume")?;
    }
    writeln!(writer)?;
    Ok(())
}

fn init_tui_terminal() -> Result<ratatui::DefaultTerminal> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("jcode TUI requires an interactive terminal (stdin/stdout must be a TTY)");
    }
    let is_resuming = std::env::var("JCODE_RESUMING").is_ok();
    if is_resuming {
        init_tui_terminal_resume()
    } else {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(ratatui::init)).map_err(|payload| {
            anyhow::anyhow!(
                "failed to initialize terminal: {}",
                panic_payload_to_string(payload.as_ref())
            )
        })
    }
}

pub fn init_tui_runtime() -> Result<(ratatui::DefaultTerminal, TuiRuntimeState)> {
    let terminal = init_tui_terminal()?;
    crate::tui::mermaid::install_jcode_mermaid_hooks();
    crate::tui::markdown::install_jcode_markdown_hooks();
    crate::tui::mermaid::init_picker();

    let perf_policy = crate::perf::tui_policy();
    let mouse_capture = perf_policy.enable_mouse_capture;
    let focus_change = perf_policy.enable_focus_change;
    let keyboard_enhanced = if perf_policy.enable_keyboard_enhancement {
        tui::enable_keyboard_enhancement()
    } else {
        false
    };

    crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;
    if focus_change {
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableFocusChange)?;
    }
    if mouse_capture {
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
    }

    Ok((
        terminal,
        TuiRuntimeState {
            mouse_capture,
            keyboard_enhanced,
            focus_change,
        },
    ))
}

pub fn cleanup_tui_runtime(state: &TuiRuntimeState, restore_terminal: bool) {
    if restore_terminal {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
        if state.focus_change {
            let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableFocusChange);
        }
        if state.mouse_capture {
            let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
        }
        if state.keyboard_enhanced {
            tui::disable_keyboard_enhancement();
        }
        ratatui::restore();

        // Issue #158: defensive belt-and-suspenders reset for terminals
        // that may still hold state ratatui::restore() doesn't clear (see
        // defensive_terminal_reset_bytes for the rationale per byte).
        // Errors silently ignored — we're already exiting; better to leak
        // an escape than to fail cleanup.
        let mut stdout = io::stdout();
        let _ = stdout.write_all(defensive_terminal_reset_bytes());
        let _ = stdout.flush();
    }

    crate::tui::mermaid::clear_image_state();
}

pub fn cleanup_tui_runtime_for_run_result(
    state: &TuiRuntimeState,
    run_result: &crate::tui::RunResult,
    extra_exec: bool,
) {
    let will_exec = extra_exec
        || run_result.reload_session.is_some()
        || run_result.rebuild_session.is_some()
        || run_result.update_session.is_some();
    cleanup_tui_runtime(state, !will_exec);
}

pub fn print_session_resume_hint(session_id: &str) {
    let _ = write_session_resume_hint(io::stderr().lock(), session_id);
}

fn write_session_resume_hint(mut writer: impl Write, session_id: &str) -> io::Result<()> {
    let session_name = id::extract_session_name(session_id).unwrap_or(session_id);
    writeln!(writer)?;
    writeln!(
        writer,
        "\x1b[33mSession \x1b[1m{}\x1b[0m\x1b[33m - to resume:\x1b[0m",
        session_name
    )?;
    writeln!(writer, "  jcode --resume {}", session_id)?;
    writeln!(writer)?;
    Ok(())
}

fn init_tui_terminal_resume() -> Result<ratatui::DefaultTerminal> {
    use ratatui::{Terminal, backend::CrosstermBackend};

    crossterm::terminal::enable_raw_mode()
        .map_err(|e| anyhow::anyhow!("failed to enable raw mode on resume: {}", e))?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)
        .map_err(|e| anyhow::anyhow!("failed to create terminal on resume: {}", e))?;

    terminal
        .clear()
        .map_err(|e| anyhow::anyhow!("failed to clear terminal on resume: {}", e))?;

    Ok(terminal)
}

#[cfg(unix)]
pub fn signal_name(sig: i32) -> &'static str {
    match sig {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        6 => "SIGABRT",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        _ => "unknown",
    }
}

#[cfg(not(unix))]
pub fn signal_name(_sig: i32) -> &'static str {
    "unknown"
}

#[cfg(unix)]
fn signal_crash_reason(sig: i32) -> String {
    match sig {
        libc::SIGHUP => "Terminal or window closed (SIGHUP)".to_string(),
        libc::SIGTERM => "Terminated (SIGTERM)".to_string(),
        libc::SIGINT => "Interrupted (SIGINT)".to_string(),
        libc::SIGQUIT => "Quit signal (SIGQUIT)".to_string(),
        _ => format!("Terminated by signal {} ({})", signal_name(sig), sig),
    }
}

#[cfg(unix)]
fn handle_termination_signal(sig: i32) -> ! {
    mark_current_session_crashed(signal_crash_reason(sig));

    let _ = crossterm::terminal::disable_raw_mode();
    let _ = crossterm::execute!(
        std::io::stderr(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show
    );

    // Issue #158: defensive reset for terminals that may still hold state
    // crossterm doesn't clear (scroll region, bracketed paste, focus mode).
    // On signal we don't know which were enabled, so reset all unconditionally.
    let mut stderr = io::stderr();
    let _ = stderr.write_all(defensive_terminal_reset_bytes_signal());
    let _ = stderr.flush();

    if let Some(session_id) = get_current_session() {
        print_session_resume_hint(&session_id);
    }

    std::process::exit(128 + sig);
}

#[cfg(unix)]
pub fn spawn_session_signal_watchers() {
    use tokio::signal::unix::{SignalKind, signal};

    fn spawn_one(sig: i32, kind: SignalKind) {
        tokio::spawn(async move {
            let mut stream = match signal(kind) {
                Ok(s) => s,
                Err(e) => {
                    crate::logging::error(&format!(
                        "Failed to install {} handler: {}",
                        signal_name(sig),
                        e
                    ));
                    return;
                }
            };
            if stream.recv().await.is_some() {
                crate::logging::info(&format!("Received {} in TUI process", signal_name(sig)));
                handle_termination_signal(sig);
            }
        });
    }

    spawn_one(libc::SIGHUP, SignalKind::hangup());
    spawn_one(libc::SIGTERM, SignalKind::terminate());
    spawn_one(libc::SIGINT, SignalKind::interrupt());
    spawn_one(libc::SIGQUIT, SignalKind::quit());
}

#[cfg(not(unix))]
pub fn spawn_session_signal_watchers() {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_SESSION_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_session_recovery_tracking() {
        let _guard = TEST_SESSION_LOCK.lock().unwrap();
        set_current_session("test_session_123");

        let stored = get_current_session();
        assert_eq!(stored.as_deref(), Some("test_session_123"));
    }

    #[test]
    fn test_session_recovery_message_format() {
        let _guard = TEST_SESSION_LOCK.lock().unwrap();
        let test_session = "session_format_test_12345";
        set_current_session(test_session);

        if let Some(session_id) = get_current_session() {
            let mut output = Vec::new();
            write_session_resume_hint(&mut output, &session_id).unwrap();
            let output = String::from_utf8(output).unwrap();
            let expected_cmd = format!("jcode --resume {}", session_id);
            assert!(output.contains(&expected_cmd));
            assert!(output.contains("to resume"));
            assert!(!session_id.is_empty());
        } else {
            panic!("Session ID should be set");
        }
    }

    #[test]
    fn session_resume_hint_writer_reports_closed_stderr_without_panicking() {
        struct ClosedWriter;

        impl Write for ClosedWriter {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "stderr closed"))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let error = write_session_resume_hint(ClosedWriter, "session_closed_pipe")
            .expect_err("closed stderr should be reported as an I/O error");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn crash_resume_hint_writer_reports_closed_stderr_without_panicking() {
        // Regression test for #105: show_crash_resume_hint() used to call
        // eprintln! directly, which panics on a broken pipe / closed stderr.
        // The new helper returns Result so callers can swallow the error
        // and the panic-hook path is no longer at risk of double-panicking.
        struct ClosedWriter;

        impl Write for ClosedWriter {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "stderr closed"))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        // The function may write or short-circuit (no crashed sessions).
        // Either Ok or BrokenPipe is acceptable; what matters is it doesn't
        // panic.
        let _ = write_crash_resume_hint(ClosedWriter);
    }

    // ---- Issue #158: terminal reset bytes ----

    #[test]
    fn defensive_reset_bytes_contain_scroll_region_reset() {
        let bytes = defensive_terminal_reset_bytes();
        let s = std::str::from_utf8(bytes).expect("ASCII");
        assert!(s.contains("\x1b[r"), "missing scroll-region reset: {:?}", s);
        assert!(s.contains("\x1b[?25h"), "missing cursor show: {:?}", s);
        assert!(
            s.contains("\x1b[?2004l"),
            "missing bracketed-paste disable: {:?}",
            s
        );
    }

    #[test]
    fn defensive_reset_signal_includes_focus_disable() {
        let regular = defensive_terminal_reset_bytes();
        let signal = defensive_terminal_reset_bytes_signal();

        // Signal variant is a superset of the regular one.
        assert!(signal.starts_with(regular));

        let s = std::str::from_utf8(signal).expect("ASCII");
        assert!(
            s.contains("\x1b[?1004l"),
            "signal-path reset must disable focus mode: {:?}",
            s
        );
    }

    #[test]
    fn defensive_reset_bytes_are_pure_ascii() {
        // Terminals should not see unexpected bytes; verify these are valid
        // 7-bit ASCII so they can't accidentally be interpreted as UTF-8
        // multi-byte starts.
        for b in defensive_terminal_reset_bytes() {
            assert!(*b < 0x80, "non-ASCII byte {:#x} in reset", b);
        }
        for b in defensive_terminal_reset_bytes_signal() {
            assert!(*b < 0x80, "non-ASCII byte {:#x} in signal reset", b);
        }
    }
}
