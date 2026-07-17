//! Console/terminal ANSI capability helpers.
//!
//! Rendering ANSI escapes before the console supports them shows literal
//! `←[90m` garbage on legacy Windows consoles (issue #498). These helpers let
//! early startup output decide whether color is safe, and opportunistically
//! enable VT processing the same way modern CLIs do.

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
    if !std::io::stderr().is_terminal() {
        return false;
    }

    #[cfg(windows)]
    {
        enable_stderr_vt_processing()
    }
    #[cfg(not(windows))]
    {
        true
    }
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
