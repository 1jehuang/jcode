//! Carry the terminal focus state across a client re-exec.
//!
//! A client replaces itself in place on reload, update and resume. The new
//! process used to assume it was focused, and a terminal only reports focus
//! *changes*, so a window that was already in the background never corrected
//! it. Such a client kept animating spinners at full frame rate on a hidden
//! workspace until the user next visited it.
//!
//! The state is kept in the environment, tagged with the process id. An exec
//! keeps the pid, so the replacement process recognises its own marker, while
//! child processes that merely inherit the variable ignore it.

const UNFOCUSED_ENV: &str = "JCODE_CLIENT_UNFOCUSED_PID";
/// Set once a focus report or an input event has told this process what its
/// focus state really is. Without it the state is only a presumption.
const CONFIRMED_ENV: &str = "JCODE_CLIENT_FOCUS_KNOWN_PID";

fn marker_matches(value: Option<&str>, pid: u32) -> bool {
    value.and_then(|v| v.trim().parse::<u32>().ok()) == Some(pid)
}

/// Record a focus change so a later re-exec of this process can restore it.
pub(crate) fn record(focused: bool) {
    // Tests share one process: a marker left by one test would change the
    // starting focus of every client created after it.
    if cfg!(test) {
        return;
    }
    confirm();
    if focused {
        if std::env::var_os(UNFOCUSED_ENV).is_some() {
            crate::env::remove_var(UNFOCUSED_ENV);
        }
    } else {
        crate::env::set_var(UNFOCUSED_ENV, std::process::id().to_string());
    }
}

/// Record that the focus state is known rather than presumed.
pub(crate) fn confirm() {
    if cfg!(test) {
        return;
    }
    let pid = std::process::id();
    if !marker_matches(std::env::var(CONFIRMED_ENV).ok().as_deref(), pid) {
        crate::env::set_var(CONFIRMED_ENV, pid.to_string());
    }
}

/// Whether this process learned its real focus state before it re-executed.
pub(crate) fn initial_confirmed() -> bool {
    let pid = std::process::id();
    marker_matches(std::env::var(CONFIRMED_ENV).ok().as_deref(), pid)
        || marker_matches(std::env::var(UNFOCUSED_ENV).ok().as_deref(), pid)
}

/// Focus state a freshly started client should assume: unfocused only when
/// this very process recorded losing focus before it re-executed.
pub(crate) fn initial_focus() -> bool {
    !marker_matches(
        std::env::var(UNFOCUSED_ENV).ok().as_deref(),
        std::process::id(),
    )
}

#[cfg(test)]
mod tests {
    use super::marker_matches;

    #[test]
    fn only_this_process_marker_counts() {
        assert!(marker_matches(Some("42"), 42));
        assert!(marker_matches(Some(" 42\n"), 42));
        // Inherited by a child process: different pid, so it starts focused.
        assert!(!marker_matches(Some("41"), 42));
        assert!(!marker_matches(Some("1"), 42));
        assert!(!marker_matches(Some(""), 42));
        assert!(!marker_matches(None, 42));
    }
}
