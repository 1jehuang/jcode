//! Reap child processes inherited across an `exec` reload.
//!
//! The daemon reloads itself with `exec`, which keeps the PID. Children the
//! previous image spawned (MCP servers, power-inhibit helpers, shell tools)
//! stay our children, but the new image holds no `Child` handle for them. Their
//! pipes are close-on-exec, so most exit right after the reload and then sit
//! as zombies forever because nobody calls `waitpid`. Over a day of self-dev
//! reloads that reached hundreds of zombie `npm`/`node` entries.
//!
//! Only PIDs present at startup are touched. Children spawned by this image
//! are owned by tokio/std handles, and reaping them here would steal their
//! exit status. An inherited PID cannot be reused while it is unreaped, so the
//! snapshot stays accurate until we reap it.

/// Environment marker set on MCP stdio children so a fresh image can tell
/// leaked MCP servers (safe to kill: the new image reconnects its own) apart
/// from detached work that is meant to outlive a reload.
pub const MCP_CHILD_MARKER_ENV: &str = "JCODE_MCP_CHILD";

#[cfg(target_os = "linux")]
mod imp {
    use std::collections::HashSet;
    use std::time::Duration;

    const REAP_INTERVAL: Duration = Duration::from_secs(30);

    fn children_of(pid: u32) -> HashSet<i32> {
        let mut out = HashSet::new();
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
            return out;
        };
        for task in tasks.flatten() {
            let path = task.path().join("children");
            if let Ok(text) = std::fs::read_to_string(path) {
                out.extend(
                    text.split_whitespace()
                        .filter_map(|p| p.parse::<i32>().ok()),
                );
            }
        }
        out
    }

    fn has_mcp_marker(pid: i32) -> bool {
        let Ok(environ) = std::fs::read(format!("/proc/{pid}/environ")) else {
            return false;
        };
        let needle = format!("{}=", super::MCP_CHILD_MARKER_ENV);
        environ
            .split(|b| *b == 0)
            .any(|entry| entry.starts_with(needle.as_bytes()))
    }

    /// Reap whatever has exited. Returns the PIDs still alive.
    fn reap(pids: &mut HashSet<i32>) {
        pids.retain(|&pid| {
            let mut status = 0;
            // SAFETY: plain syscall on a PID we inherited and nobody else owns.
            let rc = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
            rc == 0
        });
    }

    pub fn start() {
        let mut inherited = children_of(std::process::id());
        if inherited.is_empty() {
            return;
        }
        let mut killed = 0usize;
        for &pid in &inherited {
            if has_mcp_marker(pid) {
                // MCP children run in their own process group (see mcp client),
                // so this also stops `npx` wrappers' node grandchildren.
                // SAFETY: signalling a process group we created.
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                    libc::kill(pid, libc::SIGKILL);
                }
                killed += 1;
            }
        }
        let before = inherited.len();
        reap(&mut inherited);
        crate::logging::info(&format!(
            "inherited_children: {before} inherited, killed {killed} stale MCP servers, reaped {}",
            before - inherited.len()
        ));
        if inherited.is_empty() {
            return;
        }
        let _ = std::thread::Builder::new()
            .name("jcode-inherited-reaper".to_string())
            .spawn(move || {
                while !inherited.is_empty() {
                    std::thread::sleep(REAP_INTERVAL);
                    reap(&mut inherited);
                }
            });
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn reaps_exited_inherited_child() {
            let child = std::process::Command::new("true").spawn().unwrap();
            let pid = child.id() as i32;
            std::mem::forget(child);
            std::thread::sleep(std::time::Duration::from_millis(200));
            let mut set = std::collections::HashSet::from([pid]);
            super::reap(&mut set);
            assert!(set.is_empty());
            assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
        }

        #[test]
        fn detects_mcp_marker() {
            let mut child = std::process::Command::new("sleep")
                .arg("5")
                .env(super::super::MCP_CHILD_MARKER_ENV, "x")
                .spawn()
                .unwrap();
            let mut plain = std::process::Command::new("sleep")
                .arg("5")
                .spawn()
                .unwrap();
            assert!(super::has_mcp_marker(child.id() as i32));
            assert!(!super::has_mcp_marker(plain.id() as i32));
            assert!(super::children_of(std::process::id()).contains(&(child.id() as i32)));
            let _ = child.kill();
            let _ = plain.kill();
            let _ = child.wait();
            let _ = plain.wait();
        }
    }
}

/// Kill stale MCP servers and reap exited children inherited from a previous
/// process image. Call once at daemon startup, before spawning anything.
pub fn start() {
    // Idempotent: process startup calls this for every mode (clients exec
    // reload too), and the server calls it again. A second snapshot taken
    // after this image spawned children would steal their exit statuses.
    static ONCE: std::sync::Once = std::sync::Once::new();
    #[cfg(target_os = "linux")]
    ONCE.call_once(imp::start);
    #[cfg(not(target_os = "linux"))]
    ONCE.call_once(|| {});
}
