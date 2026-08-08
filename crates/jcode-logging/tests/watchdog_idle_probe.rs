//! Regression guard for a real false positive: an idle client that is simply
//! waiting for user input must never be reported as stalled.
//!
//! Lives in its own test binary because the watchdog and logger are
//! process-global: sharing a process with the stall probe would let one test's
//! work guard and log file leak into the other.

use std::time::Duration;

#[test]
#[ignore = "takes ~15s; must run in its own process (separate from the stall probe)"]
fn idle_process_is_not_reported_as_stalled() {
    let dir = std::env::temp_dir().join(format!("jcode-wd-idle-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    unsafe {
        std::env::set_var("HOME", &dir);
        std::env::set_var("JCODE_WATCHDOG_STALL_SECS", "3");
        std::env::set_var("JCODE_WATCHDOG_HEARTBEAT_SECS", "0");
    }

    jcode_logging::init();
    jcode_logging::watchdog::beat("idle.probe");
    // No work guard: the process is idle, not working.
    std::thread::sleep(Duration::from_secs(15));

    let contents = jcode_logging::log_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        !contents.contains("watchdog.stall"),
        "idle process was wrongly reported as stalled:\n{contents}"
    );
}
