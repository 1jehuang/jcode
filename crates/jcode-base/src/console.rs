pub use jcode_core::console::stderr_supports_ansi;

/// Best-effort `eprintln!` that never panics on a dead terminal.
///
/// std's `eprintln!` panics when the underlying write fails (closed window,
/// dropped remote client mid-login). During an OAuth login flow that runs
/// after the terminal died, such a panic aborts the process with exit 101
/// instead of surfacing the real error. Log the failure and move on.
pub fn eprintln_best_effort(msg: &str) {
    use std::io::Write;
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{msg}");
}

/// Best-effort `eprint!` + flush that never panics on a dead terminal.
///
/// Writes to stderr: every call site replaced an `eprint!` prompt, and prompts
/// must stay on the interactive stream even when stdout is redirected to a
/// file, or the login would wait for input whose question the user never sees.
pub fn eprompt_best_effort(msg: &str) {
    use std::io::Write;
    let mut stderr = std::io::stderr().lock();
    let _ = write!(stderr, "{msg}");
    let _ = stderr.flush();
}
