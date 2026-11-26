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

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod imp {
    use std::collections::HashSet;
    use std::time::Duration;

    const REAP_INTERVAL: Duration = Duration::from_secs(30);

    #[cfg(target_os = "linux")]
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
                        .filter_map(|p| match p.parse::<i32>() {
                            Ok(pid) => Some(pid),
                            Err(error) => {
                                crate::logging::info(&format!(
                                    "Invalid inherited PID {p}: {error}"
                                ));
                                None
                            }
                        }),
                );
            }
        }
        out
    }

    #[cfg(target_os = "linux")]
    fn has_mcp_marker(pid: i32) -> bool {
        let Ok(environ) = std::fs::read(format!("/proc/{pid}/environ")) else {
            return false;
        };
        let needle = format!("{}=", super::MCP_CHILD_MARKER_ENV);
        environ
            .split(|b| *b == 0)
            .any(|entry| entry.starts_with(needle.as_bytes()))
    }

    /// `PROC_PPID_ONLY` from `<sys/proc_info.h>`; not exported by `libc`.
    #[cfg(target_os = "macos")]
    const PROC_PPID_ONLY: u32 = 6;

    /// macOS has no procfs. `proc_listpids(PROC_PPID_ONLY)` walks both the live
    /// and the zombie process lists, so exited children are included. It
    /// returns bytes, unlike `proc_listchildpids`, whose unit changed across
    /// macOS releases.
    #[cfg(target_os = "macos")]
    fn children_of(pid: u32) -> HashSet<i32> {
        let mut out = HashSet::new();
        // SAFETY: a null buffer asks only for the size needed.
        let needed = unsafe { libc::proc_listpids(PROC_PPID_ONLY, pid, std::ptr::null_mut(), 0) };
        if needed <= 0 {
            return out;
        }
        // Headroom for children spawned between the two calls.
        let mut pids = vec![0i32; needed as usize / std::mem::size_of::<i32>() + 32];
        let size = (pids.len() * std::mem::size_of::<i32>()) as libc::c_int;
        // SAFETY: the buffer is `size` bytes long and owned by us.
        let filled =
            unsafe { libc::proc_listpids(PROC_PPID_ONLY, pid, pids.as_mut_ptr().cast(), size) };
        if filled <= 0 {
            return out;
        }
        pids.truncate(filled as usize / std::mem::size_of::<i32>());
        out.extend(pids.into_iter().filter(|&p| p > 0));
        out
    }

    /// The environment of a same-user process via `KERN_PROCARGS2`: argc, the
    /// exec path, argv and then the environment, all NUL separated.
    #[cfg(target_os = "macos")]
    fn has_mcp_marker(pid: i32) -> bool {
        let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
        let mut argmax: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>();
        // SAFETY: `argmax` is a c_int and `len` holds its size.
        let rc = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                2,
                (&mut argmax as *mut libc::c_int).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 || argmax <= 0 {
            return false;
        }
        let mut buf = vec![0u8; argmax as usize];
        let mut len = buf.len();
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        // SAFETY: `buf` is `len` bytes long; the kernel writes at most that.
        let rc = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                buf.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 {
            return false;
        }
        buf.truncate(len);
        let needle = format!("{}=", super::MCP_CHILD_MARKER_ENV);
        buf.split(|b| *b == 0)
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
        if let Err(error) = std::thread::Builder::new()
            .name("jcode-inherited-reaper".to_string())
            .spawn(move || {
                while !inherited.is_empty() {
                    std::thread::sleep(REAP_INTERVAL);
                    reap(&mut inherited);
                }
            })
        {
            crate::logging::info(&format!("Unable to start inherited child reaper: {error}"));
        }
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
            assert!(!super::children_of(std::process::id()).contains(&pid));
        }

        /// A long-lived child whose environment the OS lets us read. macOS hides
        /// the environment of SIP-protected system binaries such as
        /// `/bin/sleep` from `KERN_PROCARGS2`, so the test re-runs its own
        /// binary (ad-hoc signed, not a platform binary) on an ignored test
        /// that only sleeps. Real MCP servers (node, npx, python) are readable.
        fn sleeping_child(marked: bool) -> std::process::Child {
            let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
            cmd.args([
                "--ignored",
                "--exact",
                "inherited_children::imp::tests::sleep_for_marker_test",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
            if marked {
                cmd.env(super::super::MCP_CHILD_MARKER_ENV, "x");
            }
            cmd.spawn().unwrap()
        }

        #[test]
        #[ignore = "helper process for detects_mcp_marker"]
        fn sleep_for_marker_test() {
            std::thread::sleep(std::time::Duration::from_secs(5));
        }

        #[test]
        fn detects_mcp_marker() {
            let mut child = sleeping_child(true);
            let mut plain = sleeping_child(false);
            // Some process launchers return before the final image's environ is
            // visible to the OS. Wait for the observable marker, not a
            // fixed startup delay, and retain diagnostics on a real failure.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while !super::has_mcp_marker(child.id() as i32) {
                if std::time::Instant::now() >= deadline {
                    let environ = format!("marker not seen for pid {}", child.id());
                    child.kill().unwrap();
                    child.wait().unwrap();
                    plain.kill().unwrap();
                    plain.wait().unwrap();
                    panic!("MCP marker did not become visible: {environ:?}");
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
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
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    ONCE.call_once(imp::start);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    ONCE.call_once(|| {});
}
