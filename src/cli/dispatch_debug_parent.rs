#[cfg(any(target_os = "linux", test))]
pub(super) fn is_file_controlled_debug_client() -> bool {
    std::env::var_os("JCODE_DEBUG_CMD_PATH").is_some()
}

#[cfg(target_os = "linux")]
pub(super) fn is_orphan_adopter_name(name: &str) -> bool {
    matches!(name.trim(), "init" | "systemd")
}

#[cfg(target_os = "linux")]
pub(super) fn parent_is_orphan_adopter(parent_pid: libc::pid_t) -> bool {
    if parent_pid <= 1 {
        return true;
    }
    std::fs::read_to_string(format!("/proc/{parent_pid}/comm"))
        .is_ok_and(|name| is_orphan_adopter_name(&name))
}

/// Tie file-controlled debug clients to the process that launched them.
///
/// These clients are automation helpers, not user-owned terminals. Without a
/// parent-death signal they are reparented to init when a verification script
/// or debug server exits, retaining a full TUI and session history indefinitely.
#[cfg(target_os = "linux")]
pub(super) fn arm_debug_client_parent_death_signal() {
    if !is_file_controlled_debug_client() {
        return;
    }

    // Capture the parent first, then check it again after prctl. This closes the
    // race where the launcher exits immediately before the signal is armed.
    // Safety: getppid has no preconditions and does not dereference pointers.
    let parent_pid = unsafe { libc::getppid() };
    // Safety: PR_SET_PDEATHSIG accepts a signal number as its scalar argument.
    let armed = unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) } == 0;
    // Safety: getppid has no preconditions and does not dereference pointers.
    let current_parent_pid = unsafe { libc::getppid() };
    if armed
        && (parent_is_orphan_adopter(parent_pid)
            || current_parent_pid != parent_pid
            || parent_is_orphan_adopter(current_parent_pid))
    {
        std::process::exit(0);
    }
}

#[cfg(not(target_os = "linux"))]
pub(super) fn arm_debug_client_parent_death_signal() {}
