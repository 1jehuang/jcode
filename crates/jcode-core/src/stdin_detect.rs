#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdinState {
    Reading,
    NotReading,
    Unknown,
}

pub fn is_waiting_for_stdin(pid: u32) -> StdinState {
    #[cfg(target_os = "linux")]
    return linux::check(pid);

    #[cfg(target_os = "macos")]
    return macos::check(pid);

    #[cfg(target_os = "windows")]
    return windows::check(pid);

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    return StdinState::Unknown;
}

#[cfg(target_os = "linux")]
pub mod linux {
    use super::*;

    pub fn check(pid: u32) -> StdinState {
        check_inner(pid, false)
    }

    fn check_inner(pid: u32, strict: bool) -> StdinState {
        // First try /proc/PID/syscall (most accurate - shows exact syscall + fd)
        if let Ok(contents) = std::fs::read_to_string(format!("/proc/{}/syscall", pid)) {
            // Format: "syscall_nr fd ..."
            // read = 0 on x86_64, 63 on aarch64
            // We want: read(0, ...) i.e. syscall read on fd 0 (stdin)
            let parts: Vec<&str> = contents.split_whitespace().collect();
            if parts.len() >= 2 {
                let syscall_nr = parts[0];
                let fd = parts[1];
                // read syscall: 0 on x86_64, 63 on aarch64
                let is_read = syscall_nr == "0" || syscall_nr == "63";
                let is_stdin = fd == "0x0";
                if is_read && is_stdin {
                    return StdinState::Reading;
                }
            }
        }

        // Fallback: /proc/PID/wchan (no special permissions needed).
        // This is less exact than /proc/PID/syscall, so pair it with an fd 0
        // pipe/pty check. For child processes, check_process_tree also verifies
        // the child shares the parent's stdin pipe before calling strict mode.
        if let Ok(wchan) = std::fs::read_to_string(format!("/proc/{}/wchan", pid)) {
            let wchan = wchan.trim();
            if (wchan == "n_tty_read"
                || wchan == "wait_woken"
                || wchan == "pipe_read"
                || wchan == "pipe_wait_readable"
                || wchan == "unix_stream_read_generic")
                && stdin_is_pipe_or_pty(pid)
            {
                return StdinState::Reading;
            }
            return StdinState::NotReading;
        }

        if strict {
            StdinState::NotReading
        } else {
            StdinState::Unknown
        }
    }

    fn stdin_is_pipe_or_pty(pid: u32) -> bool {
        if let Ok(link) = std::fs::read_link(format!("/proc/{}/fd/0", pid)) {
            let path = link.to_string_lossy();
            return path.contains("pipe") || path.contains("pts") || path.contains("ptmx");
        }
        false
    }

    /// Check all threads in a process group (for cases where a child is the one reading)
    pub fn check_process_tree(pid: u32) -> StdinState {
        // Check the process itself
        let result = check(pid);
        if result == StdinState::Reading {
            return result;
        }

        // Walk the descendant tree (children, grandchildren, ...) so wrapper
        // chains like `sh -> wrapper -> reader` are detected even when the
        // intermediate process is not itself reading stdin. At each hop we keep
        // the existing same-stdin-pipe gate: a descendant only counts if it
        // shares fd 0 with `pid`, so unrelated background processes that happen
        // to read their own stdin do not trigger a false positive.
        let parent_stdin_link = std::fs::read_link(format!("/proc/{}/fd/0", pid))
            .ok()
            .map(|p| p.to_string_lossy().to_string());

        // Bound the traversal so a pathological/deep tree cannot turn this
        // few-hundred-ms poll into an expensive scan, and guard against cycles.
        const MAX_DEPTH: usize = 32;
        const MAX_VISITED: usize = 512;
        let mut visited: std::collections::HashSet<u32> = std::collections::HashSet::new();
        visited.insert(pid);
        // (pid, depth) work queue for a breadth-first descent.
        let mut queue: std::collections::VecDeque<(u32, usize)> = direct_children(pid)
            .into_iter()
            .map(|child| (child, 1usize))
            .collect();

        while let Some((child_pid, depth)) = queue.pop_front() {
            if !visited.insert(child_pid) || visited.len() > MAX_VISITED {
                continue;
            }

            // Only descendants sharing the same stdin pipe are relevant.
            if let Some(ref parent_link) = parent_stdin_link {
                let child_link = std::fs::read_link(format!("/proc/{}/fd/0", child_pid))
                    .ok()
                    .map(|p| p.to_string_lossy().to_string());
                if child_link.as_deref() != Some(parent_link) {
                    continue;
                }
            }

            if check_inner(child_pid, true) == StdinState::Reading {
                return StdinState::Reading;
            }

            if depth < MAX_DEPTH {
                for grandchild in direct_children(child_pid) {
                    queue.push_back((grandchild, depth + 1));
                }
            }
        }

        result
    }

    /// Return the direct child PIDs of `pid` using the kernel's
    /// `/proc/<pid>/task/<tid>/children` interface, which lists only the
    /// immediate children of each thread as a space-separated list. This avoids
    /// scanning the entire `/proc` directory and reading every process's
    /// `status` file just to filter on `PPid`.
    ///
    /// Requires `CONFIG_PROC_CHILDREN` (standard on modern Linux). If the file
    /// is unavailable we fall back to a `/proc` scan so behavior is preserved on
    /// older kernels.
    pub(crate) fn direct_children(pid: u32) -> Vec<u32> {
        // A process's children are tracked per-thread, so union across all
        // threads of `pid`.
        let mut children = Vec::new();
        // Distinguish "interface works and the process has no children" (the
        // common leaf-process case) from "interface unavailable". Falling back
        // to the full /proc scan on a merely-empty result reintroduced the
        // exact per-poll whole-/proc scan this function exists to avoid
        // (issue #392 A1): every childless polled process triggered a scan of
        // every PID's status file on each 500ms stdin poll.
        let mut children_interface_readable = false;
        if let Ok(threads) = std::fs::read_dir(format!("/proc/{}/task", pid)) {
            for thread in threads.flatten() {
                let tid = thread.file_name();
                let Some(tid) = tid.to_str() else { continue };
                if let Ok(list) =
                    std::fs::read_to_string(format!("/proc/{}/task/{}/children", pid, tid))
                {
                    children_interface_readable = true;
                    for child in list.split_whitespace() {
                        if let Ok(child_pid) = child.parse::<u32>() {
                            children.push(child_pid);
                        }
                    }
                }
            }
        }

        if children_interface_readable {
            return children;
        }

        // Fallback for kernels without CONFIG_PROC_CHILDREN: scan /proc once.
        // This preserves the original behavior on those systems.
        children_via_proc_scan(pid)
    }

    fn children_via_proc_scan(pid: u32) -> Vec<u32> {
        let mut children = Vec::new();
        if let Ok(entries) = std::fs::read_dir("/proc") {
            for entry in entries.flatten() {
                if let Ok(name) = entry.file_name().into_string()
                    && let Ok(child_pid) = name.parse::<u32>()
                    && let Ok(status) =
                        std::fs::read_to_string(format!("/proc/{}/status", child_pid))
                {
                    for line in status.lines() {
                        if let Some(ppid_str) = line.strip_prefix("PPid:\t")
                            && ppid_str.trim().parse::<u32>().ok() == Some(pid)
                        {
                            children.push(child_pid);
                            break;
                        }
                    }
                }
            }
        }
        children
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::mem;

    // libproc bindings
    unsafe extern "C" {
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut libc::c_void,
            buffersize: i32,
        ) -> i32;
        fn proc_pidfdinfo(
            pid: i32,
            fd: i32,
            flavor: i32,
            buffer: *mut libc::c_void,
            buffersize: i32,
        ) -> i32;
    }

    const PROC_PIDLISTFDS: i32 = 1;
    const PROC_PIDFDVNODEPATHINFO: i32 = 2;
    const PROC_PIDFDSOCKETINFO: i32 = 3;
    const PROC_PIDFDPIPEINFO: i32 = 6;

    #[repr(C)]
    struct proc_fdinfo {
        proc_fd: i32,
        proc_fdtype: u32,
    }

    // Thread info
    const PROC_PIDTHREADINFO: i32 = 5;
    const PROC_PIDLISTTHREADS: i32 = 6;

    #[repr(C)]
    struct proc_threadinfo {
        pth_user_time: u64,
        pth_system_time: u64,
        pth_cpu_usage: i32,
        pth_policy: i32,
        pth_run_state: i32,
        pth_flags: i32,
        pth_sleep_time: i32,
        pth_curpri: i32,
        pth_priority: i32,
        pth_maxpriority: i32,
        pth_name: [u8; 64],
    }

    // Take this from `libc` rather than hand-rolling it. It was previously
    // defined locally as 2, which is `TH_STATE_STOPPED`, so a thread blocked on
    // `read(0)` never registered and macOS stdin forwarding never fired
    // (issue #651). Apple's `mach/thread_info.h` has RUNNING=1, STOPPED=2,
    // WAITING=3, and `libc` already exposes it, so sourcing it removes the
    // opportunity for this constant to drift again.
    use libc::TH_STATE_WAITING;

    pub fn check(pid: u32) -> StdinState {
        // Check if fd 0 (stdin) is a pipe or pty
        if !stdin_is_interactive(pid as i32) {
            return StdinState::NotReading;
        }

        // Check thread states - if any thread is in WAITING state,
        // the process might be blocked on I/O
        if is_thread_waiting(pid as i32) {
            return StdinState::Reading;
        }

        StdinState::NotReading
    }

    fn stdin_is_interactive(pid: i32) -> bool {
        // Get list of file descriptors
        let fd_size = mem::size_of::<proc_fdinfo>() as i32;
        let buf_size = fd_size * 256; // up to 256 fds
        let mut buf = vec![0u8; buf_size as usize];

        let ret = unsafe {
            proc_pidinfo(
                pid,
                PROC_PIDLISTFDS,
                0,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf_size,
            )
        };

        if ret <= 0 {
            return false;
        }

        let num_fds = ret / fd_size;
        let fds = unsafe {
            std::slice::from_raw_parts(buf.as_ptr() as *const proc_fdinfo, num_fds as usize)
        };

        // Check if fd 0 exists and is a pipe or vnode (pty)
        for fd in fds {
            if fd.proc_fd == 0 {
                // fd type 1 = vnode (could be pty), 6 = pipe
                return fd.proc_fdtype == 1 || fd.proc_fdtype == 6;
            }
        }

        false
    }

    fn is_thread_waiting(pid: i32) -> bool {
        // Get thread list
        let mut thread_ids = vec![0u64; 64];
        let ret = unsafe {
            proc_pidinfo(
                pid,
                PROC_PIDLISTTHREADS,
                0,
                thread_ids.as_mut_ptr() as *mut libc::c_void,
                (thread_ids.len() * mem::size_of::<u64>()) as i32,
            )
        };

        if ret <= 0 {
            return false;
        }

        let num_threads = ret as usize / mem::size_of::<u64>();

        // Check each thread's state
        for i in 0..num_threads {
            let mut tinfo: proc_threadinfo = unsafe { mem::zeroed() };
            let ret = unsafe {
                proc_pidinfo(
                    pid,
                    PROC_PIDTHREADINFO,
                    thread_ids[i],
                    &mut tinfo as *mut _ as *mut libc::c_void,
                    mem::size_of::<proc_threadinfo>() as i32,
                )
            };

            if ret > 0 && tinfo.pth_run_state == TH_STATE_WAITING {
                return true;
            }
        }

        false
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;

    pub fn check(_pid: u32) -> StdinState {
        // Windows: use NtQueryInformationThread to check thread state
        // A process blocked on ReadFile/ReadConsole on stdin will have
        // its thread in a Wait state with a wait reason of UserRequest
        //
        // For now, use the simpler approach: check if the process has
        // a console handle and its thread is in a wait state via
        // WaitForSingleObject with zero timeout on the process handle

        // TODO: implement with windows-sys crate
        // - OpenProcess(PROCESS_QUERY_INFORMATION, pid)
        // - NtQuerySystemInformation for thread states
        // - Check for KWAIT_REASON::WrUserRequest on stdin handle
        StdinState::Unknown
    }
}

#[cfg(test)]
#[path = "stdin_detect_tests.rs"]
mod stdin_detect_tests;
