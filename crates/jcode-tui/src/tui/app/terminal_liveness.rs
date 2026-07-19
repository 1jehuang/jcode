//! Detection of an abandoned controlling terminal.
//!
//! A TUI client can outlive its terminal: if the terminal emulator dies
//! without delivering SIGHUP (or the signal arrives while the runtime is
//! wedged), the client keeps running headless forever, holding its full
//! transcript and ~80-150 MB of heap. Dozens of such orphans were observed
//! stacking up from spawned swarm windows. crossterm's `EventStream` returns
//! `None` after input EOF, and the run loops used to just sleep and retry,
//! so nothing ever exited.
//!
//! This module answers one question cheaply: "did the controlling terminal
//! this process started with go away?" On Linux it compares the `tty_nr`
//! field of `/proc/self/stat` against the value captured at startup: when the
//! controlling terminal is torn down the kernel resets `tty_nr` to 0 while
//! the stale fd 0 still reports `isatty=true`, so this is a strictly stronger
//! signal than `IsTerminal`. On other platforms it conservatively reports
//! `false` and orphan exit relies on SIGHUP alone.

use std::sync::OnceLock;

/// tty_nr captured on first call. `None` until initialized, `Some(0)` when
/// the process never had a controlling terminal (piped/headless usage), in
/// which case abandonment is never reported.
static INITIAL_TTY_NR: OnceLock<u64> = OnceLock::new();

/// Record the startup controlling terminal. Called implicitly by
/// [`terminal_abandoned`], but callers may invoke it early (before any chance
/// of the terminal dying) for a more faithful baseline.
pub(crate) fn capture_initial_tty() {
    let _ = INITIAL_TTY_NR.get_or_init(|| current_tty_nr().unwrap_or(0));
}

/// True when this process started with a controlling terminal that has since
/// disappeared. Cheap (one small `/proc` read), safe to call from tick loops.
pub(crate) fn terminal_abandoned() -> bool {
    capture_initial_tty();
    let initial = INITIAL_TTY_NR.get().copied().unwrap_or(0);
    if initial == 0 {
        // Never had a controlling terminal: nothing to lose.
        return false;
    }
    match current_tty_nr() {
        Some(0) => true,
        // Unreadable /proc or a live tty: assume alive.
        _ => false,
    }
}

#[cfg(target_os = "linux")]
fn current_tty_nr() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    parse_tty_nr(&stat)
}

#[cfg(not(target_os = "linux"))]
fn current_tty_nr() -> Option<u64> {
    None
}

/// Extract the `tty_nr` field (field 7) from `/proc/<pid>/stat` content.
/// The comm field (2) may contain spaces and parentheses, so fields are
/// counted from after the *last* `)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_tty_nr(stat: &str) -> Option<u64> {
    let after_comm = &stat[stat.rfind(')')? + 1..];
    // after_comm fields: state(3) ppid(4) pgrp(5) session(6) tty_nr(7) ...
    after_comm
        .split_whitespace()
        .nth(4)
        .and_then(|field| field.parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tty_nr_from_stat_line() {
        let stat = "12345 (jcode) S 1 12345 12345 34823 12345 4194304 0 0 0 0 1 2 0 0 20";
        assert_eq!(parse_tty_nr(stat), Some(34823));
    }

    #[test]
    fn parses_tty_nr_with_hostile_comm() {
        let stat = "1 (a b) c) R 1 1 1 0 1 0";
        assert_eq!(parse_tty_nr(stat), Some(0));
    }

    #[test]
    fn missing_paren_yields_none() {
        assert_eq!(parse_tty_nr("garbage"), None);
    }
}
