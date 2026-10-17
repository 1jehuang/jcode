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
//! signal than `IsTerminal`. On macOS, where `/proc` is unavailable and closing
//! a terminal window does not reliably deliver SIGHUP, it probes the foreground
//! process group of the controlling TTY. A revoked PTY makes `tcgetpgrp` fail,
//! which lets the event loops terminate instead of keeping the window alive.
//!
//! # Windows
//!
//! Windows has no `tty_nr`, so the same "did my startup terminal go away?"
//! question is answered with the console window handle instead. A process
//! attached to a classic console is hosted by a conhost window, and
//! [`GetConsoleWindow`] returns a stable `HWND` identifying that window for as
//! long as the process stays attached to it. The startup handle is captured in
//! the shared `INITIAL_TTY_NR` slot (the same slot Linux and macOS use, with
//! the `HWND` playing the role of `tty_nr`), and later probes compare against
//! it:
//!
//!   * startup handle equals the current handle -> the original console is
//!     still ours -> alive;
//!   * the current probe reports no console at all -> the startup console is
//!     gone -> abandoned.
//!
//! Abandonment is reported only on that explicit "no console any more" signal.
//! A nonzero handle that merely differs from the startup one is ambiguous and
//! is treated as alive, because a false positive here would terminate a live,
//! healthy user session (see [`windows_console_abandoned`] for the reasoning).
//!
//! This is the direct analogue of Linux's `tty_nr` collapsing to 0, and it
//! catches the same orphaning failure the module was written for.
//!
//! Two honest limits of the Windows approach, which are deliberately *not*
//! claimed to be covered:
//!
//!   * A process attached to a pseudoconsole (ConPTY, as used by herdr on
//!     Windows, by Windows Terminal, and by `conhost` in ConPTY mode) has no
//!     visible console window, so `GetConsoleWindow` returns NULL for it from
//!     the very first probe. Such a process takes the "never had a console"
//!     path and is never reported abandoned. Detecting ConPTY teardown would
//!     require the pseudoconsole host's pipe handles, which the child process
//!     does not own, so it is out of scope here.
//!   * A process whose std handles are pipes (headless or piped usage, e.g. the
//!     server, or the video-export path) also reports no console, which
//!     correctly lands on the same "never had a console" path.
//!
//! In both cases the correct answer is "not abandoned", because a process that
//! never had a console has no controlling terminal to lose. That keeps the
//! `initial == 0` short-circuit in [`terminal_abandoned`] doing exactly the job
//! it already does on the Unix platforms.

use std::sync::OnceLock;

/// Identity of the controlling terminal captured on first call, using each
/// platform's own notion of it (Linux `tty_nr`, macOS foreground process group
/// id, Windows console window handle). `None` until initialized, `Some(0)` when
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
/// disappeared. Cheap (one small `/proc` read on Linux, one `GetConsoleWindow`
/// probe on Windows), safe to call from tick loops.
pub(crate) fn terminal_abandoned() -> bool {
    capture_initial_tty();
    let initial = INITIAL_TTY_NR.get().copied().unwrap_or(0);

    #[cfg(windows)]
    {
        // The Windows probe always yields a value (`0` is its lost-terminal
        // sentinel), so the `unwrap_or` arm is unreachable today. If a future
        // refactor ever makes it None again, `1` means "some console is still
        // there" so we assume alive: the conservative choice.
        windows_console_abandoned(initial, current_tty_nr().unwrap_or(1))
    }

    #[cfg(not(windows))]
    {
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
}

#[cfg(target_os = "linux")]
fn current_tty_nr() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    parse_tty_nr(&stat)
}

#[cfg(target_os = "macos")]
fn current_tty_nr() -> Option<u64> {
    // `isatty` alone is insufficient here: Darwin may leave the stale file
    // descriptor looking like a TTY after Terminal.app or another emulator has
    // closed its window. Querying the PTY itself fails with ENOTTY/EIO once it
    // has been revoked. Map that failure to zero to match Linux's lost-tty
    // sentinel used by `terminal_abandoned` above.
    let foreground_pgrp = unsafe { libc::tcgetpgrp(libc::STDIN_FILENO) };
    if foreground_pgrp < 0 {
        Some(0)
    } else {
        Some(foreground_pgrp as u64)
    }
}

/// Probe the console window that currently hosts this process.
///
/// Windows has no `tty_nr`, so the console window `HWND` stands in for it and is
/// mapped onto the same convention the Unix implementations use, so that
/// `Some(0)` means "lost terminal" everywhere in this module:
///
///   * `Some(hwnd)` -> attached to a console whose window handle is `hwnd`,
///     nonzero and stable while that console is alive;
///   * `Some(0)`    -> no console window to identify (no console attached, or a
///     pseudoconsole, which has no visible window), the lost-tty sentinel;
///   * `None`       -> not produced here; kept total to match the signature the
///     Unix implementations share.
///
/// `HWND` is a pointer, so the cast to `u64` is lossless on both 32- and
/// 64-bit Windows.
#[cfg(windows)]
fn current_tty_nr() -> Option<u64> {
    windows_console_identity().or(Some(0))
}

/// Thin syscall boundary for the Windows probe. Returns the console window
/// handle when a console is attached *and* has a window identity:
///
///   * `Some(hwnd)` -> attached to a console with a resolvable window handle,
///   * `None` -> no console attached, or a pseudoconsole (ConPTY child) which
///     has no visible window handle.
///
/// All of the abandon/not-abandon *decision* lives in the pure
/// [`windows_console_abandoned`] helper so that logic is unit-testable on any
/// host, not just on Windows.
#[cfg(windows)]
fn windows_console_identity() -> Option<u64> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        CONSOLE_MODE, GetConsoleMode, GetConsoleWindow, GetStdHandle, STD_OUTPUT_HANDLE,
    };

    unsafe {
        // A std handle we can query proves a console is attached to this
        // process, including in a pseudoconsole child whose window handle is
        // NULL. This mirrors the style already used by
        // `jcode-core::console::enable_stderr_vt_processing`.
        let std_handle = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut mode: CONSOLE_MODE = 0;
        let has_console = !std_handle.is_null()
            && std_handle != INVALID_HANDLE_VALUE
            && GetConsoleMode(std_handle, &mut mode) != 0;

        if !has_console {
            return None;
        }

        // A pseudoconsole has no visible window, so there is no handle identity
        // to compare. Collapsing that to `None` matches "no console"; the
        // startup identity is `None` too in that case, so the decision helper
        // treats it as "never had a console" and stays quiet.
        let hwnd = GetConsoleWindow();
        if hwnd.is_null() {
            None
        } else {
            Some(hwnd as u64)
        }
    }
}

/// Pure decision helper for the Windows abandon check.
///
/// Compiled on every platform so the cross-platform semantics are exercised by
/// unit tests on whichever host CI runs, rather than only on Windows. `initial`
/// is the value captured at startup (a console window `HWND` as `u64`, or `0`
/// when the process never had a console window) and `now` is a fresh probe in
/// the same convention, where `0` is the lost-tty sentinel that `current_tty_nr`
/// returns when no console window can be identified.
///
/// The rule deliberately matches the Unix implementations exactly: fire only
/// on an *explicit* loss signal, and assume alive in every ambiguous case.
///
/// In particular, a nonzero handle that merely *differs* from the startup one
/// is treated as ALIVE, not abandoned. Treating it as abandoned would depend on
/// the unverified claim that a console window keeps the same `HWND` for the life
/// of the process, and the downside of being wrong is asymmetric and severe:
/// a false positive terminates a live, healthy user session, whereas a missed
/// detection only leaves the pre-existing orphan in place for one more tick of
/// an already-degraded scenario. Since this code path cannot be exercised
/// without a Windows console, the safe reading is the one that cannot kill a
/// live session.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_console_abandoned(initial: u64, now: u64) -> bool {
    // Never had a console window (headless/piped, or a ConPTY pseudoconsole):
    // nothing to lose, stay quiet.
    if initial == 0 {
        return false;
    }
    // Explicit loss only: the probe found no console window at all.
    now == 0
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
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

    #[test]
    fn windows_console_still_attached_is_not_abandoned() {
        // Same console window handle as at startup: alive.
        assert!(!windows_console_abandoned(0x1234, 0x1234));
    }

    #[test]
    fn windows_console_lost_is_abandoned() {
        // Lost-tty sentinel: no console window could be identified any more,
        // which is the explicit loss signal.
        assert!(windows_console_abandoned(0x1234, 0));
    }

    #[test]
    fn windows_different_console_handle_is_assumed_alive() {
        // A nonzero handle that differs from the startup one is ambiguous: we
        // have no verified evidence the console went away, so we assume alive
        // rather than risk terminating a live user session on a false positive.
        assert!(!windows_console_abandoned(0x1234, 0x5678));
    }

    #[test]
    fn windows_never_had_console_is_not_abandoned() {
        // Headless/piped or a ConPTY pseudoconsole: never had a console
        // window, so there is nothing to lose and we never report abandoned.
        assert!(!windows_console_abandoned(0, 0));
        assert!(!windows_console_abandoned(0, 0x5678));
    }
}
