use anyhow::Result;
use std::io::{self, IsTerminal, Write};
use std::panic;

use crate::{id, session, telemetry, tui};

pub struct TuiRuntimeState {
    mouse_capture: bool,
    keyboard_enhanced: bool,
    focus_change: bool,
}

const INHERITED_MODES_ENV: &str = "JCODE_TUI_INHERITED_MODES";
const INHERITED_THEME_ENV: &str = "JCODE_TUI_INHERITED_THEME";

// Crossterm's Windows implementation enables Win32 console mouse input but does
// not emit the VT mouse-tracking modes. Windows Terminal and other ConPTY hosts
// use those VT modes to decide whether a wheel detent is a mouse event or should
// be translated into Up/Down keys in the alternate screen. Without this second
// signal, wheel scrolling can accidentally browse prompt history instead of the
// chat transcript even though crossterm reports mouse capture as enabled.
#[cfg(any(windows, test))]
const WINDOWS_VT_MOUSE_ENABLE: &[u8] = b"\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1015h\x1b[?1006h";
#[cfg(any(windows, test))]
const WINDOWS_VT_MOUSE_DISABLE: &[u8] = b"\x1b[?1006l\x1b[?1015l\x1b[?1003l\x1b[?1002l\x1b[?1000l";

#[cfg(windows)]
fn sync_windows_vt_mouse_capture(enabled: bool) -> io::Result<()> {
    let sequence = if enabled {
        WINDOWS_VT_MOUSE_ENABLE
    } else {
        WINDOWS_VT_MOUSE_DISABLE
    };
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    stdout.write_all(sequence)?;
    stdout.flush()
}

#[cfg(not(windows))]
fn sync_windows_vt_mouse_capture(_enabled: bool) -> io::Result<()> {
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InheritedTerminalModes {
    mouse_capture: bool,
    keyboard_enhanced: bool,
    focus_change: bool,
}

impl InheritedTerminalModes {
    fn encode(self) -> String {
        format!(
            "mouse={},keyboard={},focus={}",
            u8::from(self.mouse_capture),
            u8::from(self.keyboard_enhanced),
            u8::from(self.focus_change)
        )
    }

    fn decode(value: &str) -> Option<Self> {
        let mut modes = Self {
            mouse_capture: false,
            keyboard_enhanced: false,
            focus_change: false,
        };
        let mut seen = 0u8;
        for field in value.split(',') {
            let (name, raw) = field.split_once('=')?;
            let enabled = match raw {
                "0" => false,
                "1" => true,
                _ => return None,
            };
            match name {
                "mouse" => {
                    modes.mouse_capture = enabled;
                    seen |= 1;
                }
                "keyboard" => {
                    modes.keyboard_enhanced = enabled;
                    seen |= 2;
                }
                "focus" => {
                    modes.focus_change = enabled;
                    seen |= 4;
                }
                _ => return None,
            }
        }
        (seen == 7).then_some(modes)
    }
}

fn has_terminal_exec_handoff(
    is_resuming: bool,
    inherited_modes: Option<InheritedTerminalModes>,
) -> bool {
    is_resuming && inherited_modes.is_some()
}

/// RAII guard that guarantees the terminal is restored to a sane state when the
/// TUI runtime ends, even if the run loop returns an error or unwinds via panic.
///
/// Without this guard, an error propagated by `?` (e.g. an I/O error from a
/// `terminal.draw` call, or any other fallible step in the event loop) would
/// skip the explicit `cleanup_tui_runtime` call and leave the terminal in raw
/// mode / alternate screen. That manifests as a corrupted terminal after exit:
/// typed input is invisible because echo and cooked mode were never restored
/// (see issue #214).
///
/// The normal teardown path should call [`TuiRuntimeGuard::finish`] (or
/// [`TuiRuntimeGuard::finish_for_run_result`]) which performs the restore and
/// disarms the guard. If neither is called (error/panic path), `Drop` performs
/// a best-effort full restore.
pub struct TuiRuntimeGuard {
    state: TuiRuntimeState,
    armed: bool,
}

#[cfg(test)]
thread_local! {
    /// Counts how many times the guard's `Drop` performed an emergency restore.
    /// Used by tests to verify the error/panic safety net fires exactly once.
    static GUARD_DROP_RESTORES: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

impl TuiRuntimeGuard {
    fn new(state: TuiRuntimeState) -> Self {
        Self { state, armed: true }
    }

    /// Normal teardown for the simple case: restore the terminal and disarm.
    pub fn finish(mut self, restore_terminal: bool) {
        cleanup_tui_runtime(&self.state, restore_terminal);
        self.armed = false;
    }

    /// Normal teardown for the interactive client: restore unless we are about
    /// to exec a follow-up process (reload/rebuild/update), in which case the
    /// next process inherits the terminal modes.
    pub fn finish_for_run_result(mut self, run_result: &crate::tui::RunResult, extra_exec: bool) {
        if run_result_will_exec(run_result, extra_exec) {
            export_tui_exec_handoff(&self.state);
        }
        cleanup_tui_runtime_for_run_result(&self.state, run_result, extra_exec);
        self.armed = false;
    }
}

impl Drop for TuiRuntimeGuard {
    fn drop(&mut self) {
        if self.armed {
            // Reached only on an error/panic path that skipped explicit
            // teardown. Always perform a full restore so the user's terminal is
            // not left corrupted.
            cleanup_tui_runtime(&self.state, true);
            self.armed = false;
            #[cfg(test)]
            GUARD_DROP_RESTORES.with(|c| c.set(c.get() + 1));
        }
    }
}

pub fn set_current_session(session_id: &str) {
    crate::set_current_session(session_id);
}

pub fn get_current_session() -> Option<String> {
    crate::get_current_session()
}

pub fn install_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        default_hook(info);

        if let Some(session_id) = get_current_session() {
            print_session_resume_hint(&session_id);

            if let Some((provider, model)) = telemetry::current_provider_model() {
                telemetry::record_crash(&provider, &model, telemetry::SessionEndReason::Panic);
            }

            if let Ok(mut session) = session::Session::load(&session_id) {
                session.mark_crashed(Some(format!("Panic: {}", info)));
                let _ = session.save();
            }
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
    let crashed = session::find_recent_crashed_sessions();
    if crashed.is_empty() {
        return;
    }

    let (id, name) = &crashed[0];
    let session_label = id::extract_session_name(id).unwrap_or(name.as_str());

    if crashed.len() == 1 {
        eprintln!(
            "\x1b[33m💥 Session \x1b[1m{}\x1b[0m\x1b[33m crashed. Resume with:\x1b[0m  jcode --resume {}",
            session_label, id
        );
    } else {
        eprintln!(
            "\x1b[33m💥 {} sessions crashed recently. Most recent: \x1b[1m{}\x1b[0m",
            crashed.len(),
            session_label
        );
        eprintln!("\x1b[33m   Resume with:\x1b[0m  jcode --resume {}", id);
        eprintln!("\x1b[33m   List all:\x1b[0m     jcode --resume");
    }
    eprintln!();
}

fn init_tui_terminal(inherited_terminal: bool) -> Result<ratatui::DefaultTerminal> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("jcode TUI requires an interactive terminal (stdin/stdout must be a TTY)");
    }
    if inherited_terminal {
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

pub fn init_tui_runtime() -> Result<(ratatui::DefaultTerminal, TuiRuntimeGuard)> {
    let is_resuming = std::env::var_os("JCODE_RESUMING").is_some();
    let inherited_theme = std::env::var(INHERITED_THEME_ENV).ok();
    let inherited_modes_raw = std::env::var(INHERITED_MODES_ENV).ok();
    let inherited_modes = inherited_modes_raw
        .as_deref()
        .and_then(InheritedTerminalModes::decode);
    // JCODE_RESUMING describes the session lifecycle, but only a valid modes
    // handoff proves the previous process deliberately left the terminal live
    // across exec. A restart used to restore the terminal before exec while the
    // new process still took the resume path, leaving it on the primary screen
    // without mouse capture.
    let inherited_terminal = has_terminal_exec_handoff(is_resuming, inherited_modes);
    if inherited_terminal {
        // OSC terminal queries are unsafe here because the previous process
        // deliberately exec'd without leaving raw mode or the alternate screen.
        crate::tui::theme_detect::init_theme_mode_for_resume(inherited_theme.as_deref());
    } else {
        // The OSC 11 query needs the cooked terminal and must happen before init.
        crate::tui::theme_detect::init_theme_mode();
    }
    let terminal = init_tui_terminal(inherited_terminal)?;
    crate::tui::mermaid::install_jcode_mermaid_hooks();
    crate::tui::markdown::install_jcode_markdown_hooks();
    crate::tui::mermaid::init_picker();

    let perf_policy = crate::perf::tui_policy();
    // These private handoff values apply only to this exec boundary. Avoid
    // leaking them into tools or unrelated child jcode processes.
    crate::env::remove_var(INHERITED_MODES_ENV);
    crate::env::remove_var(INHERITED_THEME_ENV);

    let fallback_modes = InheritedTerminalModes {
        mouse_capture: perf_policy.enable_mouse_capture,
        keyboard_enhanced: perf_policy.enable_keyboard_enhancement,
        focus_change: perf_policy.enable_focus_change,
    };
    let modes = if inherited_terminal {
        // The previous process intentionally preserved these modes across exec.
        // Reassert idempotent modes because terminals, multiplexers, or an older
        // process may have cleared them during the handoff. Do not push Kitty's
        // stack-based keyboard enhancement flags again. A later normal exit must
        // still disable every inherited mode, so retain them in the guard.
        let modes = inherited_modes.unwrap_or(fallback_modes);
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;
        if modes.focus_change {
            crossterm::execute!(std::io::stdout(), crossterm::event::EnableFocusChange)?;
        }
        if modes.mouse_capture {
            crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
            if let Err(err) = sync_windows_vt_mouse_capture(true) {
                crate::logging::warn(&format!(
                    "failed to enable Windows VT mouse tracking: {err}"
                ));
            }
        }
        modes
    } else {
        let keyboard_enhanced = if perf_policy.enable_keyboard_enhancement {
            tui::enable_keyboard_enhancement()
        } else {
            false
        };
        let modes = InheritedTerminalModes {
            mouse_capture: perf_policy.enable_mouse_capture,
            keyboard_enhanced,
            focus_change: perf_policy.enable_focus_change,
        };
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;
        if modes.focus_change {
            crossterm::execute!(std::io::stdout(), crossterm::event::EnableFocusChange)?;
        }
        if modes.mouse_capture {
            crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
            if let Err(err) = sync_windows_vt_mouse_capture(true) {
                crate::logging::warn(&format!(
                    "failed to enable Windows VT mouse tracking: {err}"
                ));
            }
        }
        modes
    };

    crate::logging::info(&format!(
        "EVENT event=TUI_TERMINAL_MODES phase=initialized pid={} resuming={} handoff={} handoff_raw={} raw_mode={} mouse_capture={} keyboard_enhanced={} focus_change={} idempotent_modes_reasserted={}",
        std::process::id(),
        is_resuming,
        inherited_terminal,
        inherited_modes_raw.as_deref().unwrap_or("none"),
        crossterm::terminal::is_raw_mode_enabled().unwrap_or(false),
        modes.mouse_capture,
        modes.keyboard_enhanced,
        modes.focus_change,
        inherited_terminal,
    ));

    Ok((
        terminal,
        TuiRuntimeGuard::new(TuiRuntimeState {
            mouse_capture: modes.mouse_capture,
            keyboard_enhanced: modes.keyboard_enhanced,
            focus_change: modes.focus_change,
        }),
    ))
}

fn cleanup_tui_runtime(state: &TuiRuntimeState, restore_terminal: bool) {
    crate::logging::info(&format!(
        "EVENT event=TUI_TERMINAL_MODES phase=cleanup pid={} restore_terminal={} raw_mode={} mouse_capture={} keyboard_enhanced={} focus_change={}",
        std::process::id(),
        restore_terminal,
        crossterm::terminal::is_raw_mode_enabled().unwrap_or(false),
        state.mouse_capture,
        state.keyboard_enhanced,
        state.focus_change,
    ));
    crate::tui::mermaid::clear_image_state();
    let image_cleanup = crate::tui::mermaid::take_terminal_image_cleanup_payload();
    if !image_cleanup.is_empty() {
        use std::io::Write as _;
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(image_cleanup.as_bytes());
        let _ = stdout.flush();
    }

    if restore_terminal {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
        if state.focus_change {
            let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableFocusChange);
        }
        if state.mouse_capture {
            if let Err(error) = sync_windows_vt_mouse_capture(false) {
                crate::logging::warn(&format!(
                    "failed to disable Windows VT mouse capture: {error}"
                ));
            }
            let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
        }
        if state.keyboard_enhanced {
            tui::disable_keyboard_enhancement();
        }
        ratatui::restore();
    }
}

fn cleanup_tui_runtime_for_run_result(
    state: &TuiRuntimeState,
    run_result: &crate::tui::RunResult,
    extra_exec: bool,
) {
    cleanup_tui_runtime(state, !run_result_will_exec(run_result, extra_exec));
}

fn run_result_will_exec(run_result: &crate::tui::RunResult, extra_exec: bool) -> bool {
    extra_exec
        || run_result.reload_session.is_some()
        || run_result.rebuild_session.is_some()
        || run_result.update_session.is_some()
        || run_result.restart_session.is_some()
}

fn export_tui_exec_handoff(state: &TuiRuntimeState) {
    let modes = InheritedTerminalModes {
        mouse_capture: state.mouse_capture,
        keyboard_enhanced: state.keyboard_enhanced,
        focus_change: state.focus_change,
    };
    crate::env::set_var(INHERITED_MODES_ENV, modes.encode());
    let theme = crate::tui::theme_detect::current_theme_label();
    crate::env::set_var(INHERITED_THEME_ENV, theme);
    crate::logging::info(&format!(
        "EVENT event=TUI_TERMINAL_MODES phase=exec_handoff pid={} raw_mode={} modes={} theme={}",
        std::process::id(),
        crossterm::terminal::is_raw_mode_enabled().unwrap_or(false),
        modes.encode(),
        theme,
    ));
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

    fn test_guard() -> TuiRuntimeGuard {
        // All terminal-mode flags disabled so teardown only performs the minimal
        // (and TTY-safe) restore path during tests.
        TuiRuntimeGuard::new(TuiRuntimeState {
            mouse_capture: false,
            keyboard_enhanced: false,
            focus_change: false,
        })
    }

    #[test]
    fn inherited_terminal_modes_roundtrip() {
        let modes = InheritedTerminalModes {
            mouse_capture: true,
            keyboard_enhanced: false,
            focus_change: true,
        };
        assert_eq!(InheritedTerminalModes::decode(&modes.encode()), Some(modes));
    }

    #[test]
    fn windows_vt_mouse_modes_enable_and_disable_the_same_tracking_protocols() {
        let enable = String::from_utf8_lossy(WINDOWS_VT_MOUSE_ENABLE);
        let disable = String::from_utf8_lossy(WINDOWS_VT_MOUSE_DISABLE);
        for mode in ["1000", "1002", "1003", "1015", "1006"] {
            assert!(
                enable.contains(&format!("?{mode}h")),
                "enable sequence must turn on VT mouse mode {mode}"
            );
            assert!(
                disable.contains(&format!("?{mode}l")),
                "disable sequence must turn off VT mouse mode {mode}"
            );
        }
    }

    #[test]
    fn inherited_terminal_modes_reject_malformed_values() {
        assert_eq!(InheritedTerminalModes::decode("mouse=1,keyboard=1"), None);
        assert_eq!(
            InheritedTerminalModes::decode("mouse=yes,keyboard=1,focus=1"),
            None
        );
    }

    #[test]
    fn resume_requires_valid_terminal_handoff_metadata() {
        let modes = InheritedTerminalModes {
            mouse_capture: true,
            keyboard_enhanced: true,
            focus_change: true,
        };
        assert!(has_terminal_exec_handoff(true, Some(modes)));
        assert!(!has_terminal_exec_handoff(true, None));
        assert!(!has_terminal_exec_handoff(false, Some(modes)));
    }

    #[test]
    fn every_exec_action_preserves_terminal_modes() {
        let with = |field: &str| {
            let mut result = crate::tui::RunResult::default();
            match field {
                "reload" => result.reload_session = Some("session_test".into()),
                "rebuild" => result.rebuild_session = Some("session_test".into()),
                "update" => result.update_session = Some("session_test".into()),
                "restart" => result.restart_session = Some("session_test".into()),
                _ => unreachable!(),
            }
            result
        };

        for field in ["reload", "rebuild", "update", "restart"] {
            assert!(
                run_result_will_exec(&with(field), false),
                "{field} must preserve terminal modes across exec"
            );
        }
        assert!(run_result_will_exec(
            &crate::tui::RunResult::default(),
            true
        ));
        assert!(!run_result_will_exec(
            &crate::tui::RunResult::default(),
            false
        ));
    }

    #[test]
    fn guard_drop_restores_terminal_when_not_finished() {
        // Simulates the error/panic path where explicit teardown is skipped:
        // the guard must restore the terminal exactly once on drop (issue #214).
        GUARD_DROP_RESTORES.with(|c| c.set(0));
        {
            let _guard = test_guard();
        }
        let restores = GUARD_DROP_RESTORES.with(|c| c.get());
        assert_eq!(
            restores, 1,
            "dropping an un-finished guard must restore the terminal once"
        );
    }

    #[test]
    fn guard_finish_disarms_drop_restore() {
        // The happy path calls finish(); the drop safety net must NOT fire again.
        GUARD_DROP_RESTORES.with(|c| c.set(0));
        let guard = test_guard();
        guard.finish(true);
        let restores = GUARD_DROP_RESTORES.with(|c| c.get());
        assert_eq!(
            restores, 0,
            "finish() should disarm the guard so drop does not double-restore"
        );
    }

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
}
