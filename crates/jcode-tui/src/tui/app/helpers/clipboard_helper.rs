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

/// Runtime ordering tests for issue #684. `copy_to_clipboard` itself is
/// short-circuited under `cfg(test)` (it must never touch the developer's real
/// clipboard), so these exercise the same fallback chain it runs, using stub
/// helpers on a temporary PATH. That pins the property that actually matters:
/// the first helper that takes ownership wins, and one that fails fast hands
/// off to the next instead of reporting a false success.
#[cfg(all(test, not(any(windows, target_os = "macos"))))]
mod ordering_tests {
    use super::copy_via_clipboard_helper;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    /// Same chain as the Linux arm of `copy_to_clipboard`, reporting which
    /// helper claimed the clipboard.
    fn first_helper_that_wins() -> Option<&'static str> {
        for (program, args) in [
            ("wl-copy", &[][..]),
            ("xclip", &["-selection", "clipboard"][..]),
            ("xsel", &["--clipboard", "--input"][..]),
        ] {
            if copy_via_clipboard_helper(program, args, "payload") {
                return Some(program);
            }
        }
        None
    }

    /// Puts only the named stubs on PATH, so any helper not listed behaves as
    /// if it is not installed.
    struct StubPath {
        _dir: tempfile::TempDir,
        prev: Option<std::ffi::OsString>,
    }

    impl StubPath {
        fn with(stubs: &[(&str, &str)]) -> Self {
            let dir = tempfile::tempdir().expect("temp dir");
            for (name, body) in stubs {
                let path = dir.path().join(name);
                let mut file = std::fs::File::create(&path).expect("create stub");
                writeln!(file, "#!/bin/sh").expect("write stub");
                writeln!(file, "{body}").expect("write stub");
                drop(file);
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                    .expect("chmod stub");
            }
            let prev = std::env::var_os("PATH");
            crate::env::set_var("PATH", dir.path());
            Self { _dir: dir, prev }
        }
    }

    impl Drop for StubPath {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(prev) => crate::env::set_var("PATH", prev),
                None => crate::env::remove_var("PATH"),
            }
        }
    }

    /// Regression guard: adding the X11 helpers must not steal the Wayland path
    /// that already worked.
    #[test]
    fn wayland_still_wins_when_wl_copy_works() {
        let _lock = crate::storage::lock_test_env();
        let _stubs = StubPath::with(&[("wl-copy", "cat > /dev/null; exit 0"), ("xclip", "exit 0")]);
        assert_eq!(first_helper_that_wins(), Some("wl-copy"));
    }

    /// The actual #684 scenario: no Wayland display, so wl-copy fails fast and
    /// xclip must take over instead of falling through to arboard.
    #[test]
    fn xclip_takes_over_when_wl_copy_fails() {
        let _lock = crate::storage::lock_test_env();
        let _stubs = StubPath::with(&[
            ("wl-copy", "exit 1"),
            ("xclip", "cat > /dev/null; exit 0"),
            ("xsel", "exit 0"),
        ]);
        assert_eq!(first_helper_that_wins(), Some("xclip"));
    }

    #[test]
    fn xsel_takes_over_when_wl_copy_and_xclip_are_missing() {
        let _lock = crate::storage::lock_test_env();
        let _stubs = StubPath::with(&[("xsel", "cat > /dev/null; exit 0")]);
        assert_eq!(first_helper_that_wins(), Some("xsel"));
    }

    /// With no working helper the chain must report failure, so the real
    /// `copy_to_clipboard` continues to arboard and then OSC 52 rather than
    /// showing a false "Copied" toast.
    #[test]
    fn no_working_helper_reports_failure_so_later_fallbacks_run() {
        let _lock = crate::storage::lock_test_env();
        let _stubs = StubPath::with(&[
            ("wl-copy", "exit 1"),
            ("xclip", "exit 1"),
            ("xsel", "exit 1"),
        ]);
        assert_eq!(first_helper_that_wins(), None);
    }
}
