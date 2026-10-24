//! Console/terminal ANSI capability helpers.
//!
//! Rendering ANSI escapes before the console supports them shows literal
//! `←[90m` garbage on legacy Windows consoles (issue #498). These helpers let
//! early startup output decide whether color is safe, and opportunistically
//! enable VT processing the same way modern CLIs do.
//!
//! The [`emit_osc9_status`] helper centralises the OSC 9 escape format so every
//! caller (main TUI, permissions viewer, future status emitters) shares one
//! definition and cannot drift apart.

/// Best-effort: enable ANSI (virtual terminal processing) on the stderr
/// console, then report whether ANSI output is safe to emit.
///
/// On non-Windows this is true exactly when stderr is a terminal. On Windows
/// it attempts to switch the console to VT mode first (a no-op on Windows
/// Terminal and modern conhost, which already support it) and returns false
/// when the console cannot accept escape sequences, so callers can fall back
/// to plain text instead of printing escape garbage.
pub fn stderr_supports_ansi() -> bool {
    use std::io::IsTerminal;
    #[cfg(windows)]
    {
        std::io::stderr().is_terminal() && enable_stderr_vt_processing()
    }
    #[cfg(not(windows))]
    {
        std::io::stderr().is_terminal()
    }
}

#[cfg(all(test, not(windows)))]
mod ansi_tests {
    use std::io::IsTerminal;

    #[test]
    fn ansi_support_matches_stderr_terminal_detection() {
        assert_eq!(
            super::stderr_supports_ansi(),
            std::io::stderr().is_terminal()
        );
    }
}

/// Emit an OSC 9 progress sequence carrying a `jcode:<state>` payload to stdout.
///
/// Terminal multiplexers and status-bar integrations (e.g. herdr) capture OSC 9
/// payloads as a structured side-channel that does not depend on screen-scraping.
/// The payload format is `jcode:<state>` so detection manifests can match on a
/// version-independent string.
///
/// This is the single source of truth for the escape format. Both the main jcode
/// TUI (`emit_agent_status_osc`) and the permissions viewer call it so the byte
/// sequence can never drift between emitters. Writing to stdout is best-effort:
/// if the terminal does not understand OSC 9 the bytes are silently ignored.
///
/// Returns the underlying `io::Error` instead of swallowing it, so callers that
/// want to log the failure (or run it through a swallowed-error-aware helper) can
/// do so; callers that want fire-and-forget can use [`emit_osc9_status_ignored`].
pub fn emit_osc9_status(state: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let payload = format!("jcode:{state}");
    // OSC 9 ;<payload> BEL. crossterm has no built-in OSC 9 command, so write
    // the escape sequence directly.
    let osc = format!("\x1b]9;{payload}\x07");
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(osc.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

/// Emit an OSC 9 progress sequence and explicitly ignore any write error.
///
/// Stdout write failures are expected here (piped output, closed terminal) and
/// are safe to discard: the OSC payload is advisory status, not load-bearing
/// data. Using a named helper keeps the swallowed `Result` intentional and
/// documented, rather than scattered underscore-assign sites that look
/// accidental.
pub fn emit_osc9_status_ignored(state: &str) {
    let _ = emit_osc9_status(state);
}

#[cfg(windows)]
fn enable_stderr_vt_processing() -> bool {
    use windows_sys::Win32::System::Console::{
        CONSOLE_MODE, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle,
        STD_ERROR_HANDLE, SetConsoleMode,
    };

    unsafe {
        let handle = GetStdHandle(STD_ERROR_HANDLE);
        if handle.is_null() || handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return false;
        }
        let mut mode: CONSOLE_MODE = 0;
        if GetConsoleMode(handle, &mut mode) == 0 {
            return false;
        }
        if mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING != 0 {
            return true;
        }
        SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc9_payload_format() {
        // The exact byte sequence is part of herdr's detection contract, so lock
        // it down: ESC ] 9 ; jcode:<state> BEL.
        let payload = "working";
        let osc = format!("\x1b]9;jcode:{payload}\x07");
        assert_eq!(osc, "\x1b]9;jcode:working\x07");
        assert!(osc.starts_with("\x1b]9;"));
        assert!(osc.ends_with('\x07'));
    }

    #[test]
    fn osc9_state_variants_format() {
        for state in ["working", "idle", "blocked"] {
            let osc = format!("\x1b]9;jcode:{state}\x07");
            assert!(osc.contains(&format!("jcode:{state}")));
        }
    }

    /// `emit_osc9_status_ignored` is the fire-and-forget wrapper: it must run
    /// without panicking for every known state, even when stdout is piped or
    /// captured (as it is under `cargo test`). The OSC 9 payload is advisory
    /// status, so any write error is silently discarded by design.
    #[test]
    fn osc9_status_ignored_does_not_panic() {
        for state in ["working", "idle", "blocked"] {
            emit_osc9_status_ignored(state);
        }
    }
}
