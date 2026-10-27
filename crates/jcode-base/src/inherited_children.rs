//! Reap child processes inherited across an `exec`-based reload.
//!
//! When a server reloads it `exec`s a new image in place. Everything the old
//! image spawned (MCP servers, tool subprocesses) stays our child, but the
//! only record of those pids lived in the old image's memory. Close-on-exec
//! drops their stdin pipes, so most of them exit right away, and with nobody
//! left to `wait` on them they linger as zombies forever, one burst per reload.
//!
//! The fix must not use `waitpid(-1)`: that would also steal exit statuses
//! from children this image spawns through `tokio::process`. Instead we take a
//! snapshot of our children at the very start of `main`, before this image has
//! spawned anything, so every pid in it was necessarily inherited. A small
//! thread then reaps exactly those pids as they exit and stops when none are
//! left. Children that are still running are never signalled.

use std::time::Duration;

/// How often inherited children are polled for exit.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Snapshot the children this process already has and reap them in the
/// background as they exit. Call once, first thing in `main`.
///
/// Returns the number of inherited children found (0 for a fresh process,
/// in which case no thread is started).
pub fn reap_inherited_children() -> usize {
    let mut pids = current_children();
    // Collect anything that is already a zombie right away.
    pids.retain(|&pid| !try_reap(pid));
    let count = pids.len();
    if count == 0 {
        return 0;
    }
    let spawned = std::thread::Builder::new()
        .name("jcode-inherited-reaper".to_string())
        .spawn(move || {
            while !pids.is_empty() {
                std::thread::sleep(POLL_INTERVAL);
                pids.retain(|&pid| !try_reap(pid));
            }
        });
    if spawned.is_err() {
        return 0;
    }
    count
}

/// Reap `pid` if it has exited. Returns true once the pid is gone for good
/// (reaped now, or no longer our child), false while it is still running.
#[cfg(unix)]
fn try_reap(pid: i32) -> bool {
    let mut status = 0;
    // SAFETY: waitpid on a specific pid with WNOHANG never blocks and only
    // touches `status`.
    let result = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    // 0 = still running. A positive result is the reaped pid. -1 (ECHILD) means
    // it is not our child any more, so stop tracking it.
    result != 0
}

#[cfg(not(unix))]
fn try_reap(_pid: i32) -> bool {
    true
}

/// Pids of this process's direct children, including zombies.
#[cfg(target_os = "macos")]
pub fn current_children() -> Vec<i32> {
    let me = std::process::id() as libc::pid_t;
    let mut capacity = 256usize;
    loop {
        let mut buf = vec![0 as libc::pid_t; capacity];
        let bytes = (capacity * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
        // SAFETY: the buffer is valid for `bytes` bytes and the kernel writes at
        // most that many.
        let written = unsafe { libc::proc_listchildpids(me, buf.as_mut_ptr().cast(), bytes) };
        if written <= 0 {
            return Vec::new();
        }
        // proc_listchildpids returns a count of pids on current macOS, but has
        // historically returned bytes. Accept either and never read past the
        // buffer.
        let count = (written as usize).min(capacity);
        if count >= capacity && capacity < 65536 {
            capacity *= 4;
            continue;
        }
        buf.truncate(count);
        buf.retain(|&pid| pid > 0);
        return buf;
    }
}

/// Pids of this process's direct children, including zombies.
#[cfg(target_os = "linux")]
pub fn current_children() -> Vec<i32> {
    let mut pids = Vec::new();
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else {
        return pids;
    };
    for task in tasks.flatten() {
        if let Ok(text) = std::fs::read_to_string(task.path().join("children")) {
            pids.extend(
                text.split_whitespace()
                    .filter_map(|x| x.parse::<i32>().ok()),
            );
        }
    }
    pids.sort_unstable();
    pids.dedup();
    pids
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn current_children() -> Vec<i32> {
    Vec::new()
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;
    use std::process::Command;

    fn is_zombie_or_gone(pid: i32) -> bool {
        // Signal 0 fails for a reaped pid. A zombie still exists, so check
        // `ps` for its state.
        let out = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("ps");
        let stat = String::from_utf8_lossy(&out.stdout);
        stat.trim().is_empty() || stat.trim_start().starts_with('Z')
    }

    #[test]
    fn lists_spawned_children() {
        let mut child = Command::new("sleep").arg("5").spawn().expect("spawn");
        let pid = child.id() as i32;
        assert!(current_children().contains(&pid));
        child.kill().ok();
        child.wait().ok();
        assert!(!current_children().contains(&pid));
    }

    #[test]
    fn reaps_an_exited_child_that_nobody_waits_for() {
        // Simulate a child left over by a previous image: spawn it and forget
        // the handle, the way exec forgets tokio's Child.
        let child = Command::new("true").spawn().expect("spawn");
        let pid = child.id() as i32;
        std::mem::forget(child);
        std::thread::sleep(Duration::from_millis(200));
        assert!(is_zombie_or_gone(pid));
        assert!(try_reap(pid), "an exited child must be reaped");
        assert!(!current_children().contains(&pid), "zombie must be gone");
    }

    #[test]
    fn leaves_running_children_alone() {
        let mut child = Command::new("sleep").arg("5").spawn().expect("spawn");
        let pid = child.id() as i32;
        assert!(!try_reap(pid), "a running child must not be reported gone");
        child.kill().ok();
        child.wait().ok();
    }
}
