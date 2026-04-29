use anyhow::Result;
use std::io::{self, IsTerminal};
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
    let session_name = id::extract_session_name(session_id).unwrap_or(session_id);
    eprintln!();
    eprintln!(
        "\x1b[33mSession \x1b[1m{}\x1b[0m\x1b[33m - to resume:\x1b[0m",
        session_name
    );
    eprintln!("  jcode --resume {}", session_id);
    eprintln!();
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
            let expected_cmd = format!("jcode --resume {}", session_id);
            assert!(expected_cmd.starts_with("jcode --resume "));
            assert!(!session_id.is_empty());
        } else {
            panic!("Session ID should be set");
        }
    }
}
