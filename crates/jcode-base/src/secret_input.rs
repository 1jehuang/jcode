//! Low-level secret/credential input helpers.
//!
//! Reading an API key or verification code from the terminal is a pure
//! stdin/terminal concern with no dependency on the CLI command layer. Keeping
//! it in a low-level module lets lower layers (e.g. `auth`) read secrets
//! without taking a dependency on `cli`.

use anyhow::{Context, Result};
use std::io::{self, IsTerminal, Write};

/// Write masking feedback for one keystroke straight to stderr.
///
/// Returns whether the echo landed. Callers ignore it deliberately: a terminal
/// that refuses the write must not abort an in-progress credential entry, since
/// the read itself is still working. Reported rather than swallowed so the
/// decision is the caller's.
fn echo_secret_mask(rendered: &str) -> bool {
    let mut stderr = io::stderr();
    stderr.write_all(rendered.as_bytes()).is_ok() && stderr.flush().is_ok()
}

/// Read a single line of secret input from stdin.
///
/// When stdin is a TTY this reads in raw mode, echoing one `*` per character
/// rather than the characters themselves, so the secret stays off screen while
/// the prompt still visibly responds to typing. When stdin is not a TTY (piped
/// input) it falls back to a plain line read.
pub fn read_secret_line() -> Result<String> {
    use crossterm::terminal;

    if !io::stdin().is_terminal() {
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        return Ok(input.trim().to_string());
    }

    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !was_raw && terminal::enable_raw_mode().is_err() {
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        return Ok(input.trim().to_string());
    }

    struct RawModeGuard(bool);
    impl Drop for RawModeGuard {
        fn drop(&mut self) {
            if self.0 {
                let _ = crossterm::terminal::disable_raw_mode();
            }
        }
    }

    let _guard = RawModeGuard(!was_raw);

    // Echo a masking character per accepted keystroke. Without any feedback the
    // prompt is indistinguishable from a hung process: raw mode also suppresses
    // the terminal's own echo, so a user pasting a key sees a frozen screen and
    // reasonably concludes login is stuck (issue #660).
    let mut input = String::new();
    loop {
        if let crossterm::event::Event::Key(key_event) =
            crossterm::event::read().context("Failed to read key input")?
        {
            use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
            if !matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                continue;
            }
            match key_event.code {
                KeyCode::Enter => {
                    eprintln!();
                    break;
                }
                KeyCode::Char('c') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
                    anyhow::bail!("Cancelled.");
                }
                KeyCode::Backspace => {
                    if input.pop().is_some() {
                        // Erase one masking glyph.
                        let _echoed = echo_secret_mask("\u{8} \u{8}");
                    }
                }
                KeyCode::Char(c) => {
                    input.push(c);
                    let _echoed = echo_secret_mask("*");
                }
                _ => {}
            }
        }
    }

    Ok(input.trim().to_string())
}

// The masking path only runs on a real terminal, so its tests need a pty.
// Unix-only: openpty/fork have no Windows equivalent here.
#[cfg(all(test, unix))]
#[path = "secret_input_pty_tests.rs"]
mod pty_tests;
