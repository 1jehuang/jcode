//! End-to-end probe: a real stalled thread must produce a `watchdog.stall`
//! log line naming the last phase, so silent hangs stop being invisible.
//!
//! Run with: cargo test -p jcode-logging --test watchdog_stall_probe -- --ignored

use std::time::{Duration, Instant};

#[test]
#[ignore = "takes ~20s; exercises the real monitor thread and log file"]
fn stalled_process_emits_watchdog_stall_event() {
    let dir = std::env::temp_dir().join(format!("jcode-wd-probe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    unsafe {
        std::env::set_var("HOME", &dir);
        std::env::set_var("JCODE_WATCHDOG_STALL_SECS", "5");
        std::env::set_var("JCODE_WATCHDOG_HEARTBEAT_SECS", "5");
    }

    jcode_logging::init();
    let _work = jcode_logging::watchdog::begin_work("probe.phase");
    jcode_logging::watchdog::set_detail("probe detail");

    // Simulate the hang: work is in flight (guard alive) but no beats follow.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut found = false;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_secs(1));
        let Some(path) = jcode_logging::log_path() else {
            continue;
        };
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        if contents.contains("watchdog.stall") {
            assert!(
                contents.contains("probe.phase"),
                "stall dump lost the phase"
            );
            assert!(
                contents.contains("thread_states"),
                "stall dump lost thread states"
            );
            found = true;
            break;
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(found, "watchdog never reported the simulated stall");
}
