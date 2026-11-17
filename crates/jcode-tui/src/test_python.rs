//! Shared helpers for tests that need an external interpreter.
//!
//! Kept out of the modules that use them so the production files stay within
//! their size budgets.

/// A Python 3 interpreter that actually runs.
///
/// Windows ships a `python3` App Execution Alias that is not an interpreter: it
/// prints an install hint and exits with 9009, so a test that assumes `python3`
/// fails on a perfectly good host. Try `python3`, then `python`, and remember the
/// answer for the rest of the binary.
pub(crate) fn python_executable() -> &'static str {
    static CHOICE: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    CHOICE.get_or_init(|| {
        for candidate in ["python3", "python"] {
            let runs = std::process::Command::new(candidate)
                .arg("--version")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if runs {
                return candidate;
            }
        }
        "python3"
    })
}
