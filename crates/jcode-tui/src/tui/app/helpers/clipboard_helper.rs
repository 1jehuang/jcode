//! Spawning external clipboard helpers (`wl-copy`, `xclip`, `xsel`).
//!
//! Kept out of `helpers.rs` so the clipboard-ownership contract has one home
//! and its tests live next to it.

/// Pipe `text` into an external clipboard helper (`wl-copy`, `xclip`, `xsel`)
/// and report whether it took ownership of the selection.
///
/// These helpers fork and stay alive to serve paste requests, so the caller
/// must not block on `wait()`: that would hang for as long as the clipboard is
/// owned, and this runs on the UI thread from copy keybindings where a stall is
/// felt directly as input lag. Instead poll briefly for an early failure (e.g.
/// no display server) so the remaining fallbacks still run, then treat a live
/// child as success and reap it in the background.
#[cfg(not(any(windows, target_os = "macos")))]
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn copy_via_clipboard_helper(program: &str, args: &[&str], text: &str) -> bool {
    use std::io::Write;

    let Ok(mut child) = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };

    let wrote = match child.stdin.as_mut() {
        Some(stdin) => stdin.write_all(text.as_bytes()).is_ok(),
        None => false,
    };
    drop(child.stdin.take());
    if !wrote {
        let _ = child.kill();
        let _ = child.wait();
        return false;
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(150);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return true,
            // Exited nonzero (e.g. xclip with no DISPLAY): let the next
            // fallback try.
            Ok(Some(_)) => return false,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    // Still running: the helper became the selection owner,
                    // which is the success case.
                    std::thread::spawn(move || {
                        let _ = child.wait();
                    });
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(_) => return false,
        }
    }
}

/// Tests for the external clipboard-helper spawn path (issue #684). They use
/// ordinary coreutils instead of real clipboard tools so they pass on headless
/// CI: what matters is the contract (writes stdin, does not block on a
/// long-lived owner, reports failure for a nonzero exit or a missing binary).
#[cfg(all(test, not(any(windows, target_os = "macos"))))]
mod tests {
    use super::copy_via_clipboard_helper;

    #[test]
    fn helper_that_exits_successfully_counts_as_a_copy() {
        assert!(copy_via_clipboard_helper("cat", &[], "hello"));
    }

    #[test]
    fn helper_that_exits_nonzero_falls_through() {
        assert!(!copy_via_clipboard_helper("false", &[], "hello"));
    }

    #[test]
    fn missing_helper_binary_falls_through() {
        assert!(!copy_via_clipboard_helper(
            "jcode-nonexistent-clipboard-helper",
            &[],
            "hello"
        ));
    }

    /// A helper that keeps running is the success case (it owns the selection),
    /// and must not block the UI thread for its whole lifetime.
    #[test]
    fn long_lived_helper_counts_as_a_copy_without_blocking() {
        let start = std::time::Instant::now();
        assert!(copy_via_clipboard_helper("sleep", &["30"], "hello"));
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "spawn path blocked for {:?}",
            start.elapsed()
        );
    }
}
