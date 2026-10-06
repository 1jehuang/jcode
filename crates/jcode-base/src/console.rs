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

/// Best-effort `print!` + flush that never panics on a dead terminal.
pub fn eprompt_best_effort(msg: &str) {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    let _ = write!(stdout, "{msg}");
    let _ = stdout.flush();
}
