//! Low-level secret/credential input helpers.
//!
//! Reading an API key or verification code from the terminal is a pure
//! stdin/terminal concern with no dependency on the CLI command layer. Keeping
//! it in a low-level module lets lower layers (e.g. `auth`) read secrets
//! without taking a dependency on `cli`.

use anyhow::{Context, Result};
use std::io::{self, IsTerminal};

/// Read a single line of secret input from stdin.
///
/// When stdin is a TTY this reads in raw mode without echoing the typed
/// characters (so secrets don't appear on screen). When stdin is not a TTY
/// (piped input) it falls back to a plain line read.
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
                    input.pop();
                }
                KeyCode::Char(c) => {
                    input.push(c);
                }
                _ => {}
            }
        }
    }

    Ok(input.trim().to_string())
}
