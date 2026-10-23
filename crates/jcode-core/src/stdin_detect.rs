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
    }

    const PROC_PIDLISTFDS: i32 = 1;

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

    // Take these from `libc` rather than hand-rolling them. `TH_STATE_WAITING`
    // was previously defined locally as 2, which is `TH_STATE_STOPPED`, so a
    // thread blocked on `read(0)` never registered and macOS stdin forwarding
    // never fired (issue #651). Apple's `mach/thread_info.h` has RUNNING=1,
    // STOPPED=2, WAITING=3, and `libc` already exposes it, so sourcing it
    // removes the opportunity for this constant to drift again.
    use libc::{TH_FLAGS_SWAPPED, TH_STATE_WAITING};

    /// Whether one sampled thread is blocked on `read(0)`.
    ///
    /// `TH_STATE_WAITING` means "blocked", not "blocked on stdin": macOS
    /// reports it for threads parked on a condition variable or in a timed
    /// sleep too, so the run state alone made every process with an idle
    /// thread look like it was waiting for terminal input (issue #1146).
    ///
    /// `TH_FLAGS_SWAPPED` tracks kernel-stack reclamation: the kernel sets it
    /// whenever `thread->kernel_stack` is null (`osfmk/kern/thread.c`). A
    /// thread that parks hands its stack off within milliseconds, while a
    /// thread blocked in `read(2)` on a tty or a pipe keeps it. Measured on
    /// macOS aarch64 (2026-10-11) by sampling
    /// `proc_pidinfo(PROC_PIDTHREADINFO)`: `cat` blocked on a pty slave and on
    /// a pipe both report `WAITING` with flags `0x0` for as long as they stay
    /// blocked (10s), whereas `sleep`, a shell waiting on a child, and threads
    /// parked on condition variables report `WAITING` with flags `0x1` within
    /// ~55ms of parking. Requiring the flag to be clear therefore keeps real
    /// stdin waits and drops parked ones.
    ///
    /// The asymmetry is deliberate: a swapped-out thread that really is in
    /// `read(2)` is missed (no "waiting for stdin" hint), whereas counting
    /// parked threads reports every sleeping command as waiting for input. The
    /// residual false positive is a thread sampled inside that ~55ms parking
    /// window; the bash tool waits `STDIN_INITIAL_DELAY_MS` (300ms) before its
    /// first poll, so a command that parks at startup is swapped out by then.
    pub(super) fn thread_is_reading_stdin(run_state: i32, flags: i32) -> bool {
        run_state == TH_STATE_WAITING && flags & TH_FLAGS_SWAPPED == 0
    }

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

        // The process itself is not reading, but a wrapper shell's own thread
        // parks while it waits on a child, and the `TH_FLAGS_SWAPPED` filter
        // above correctly excludes that parked thread. `bash -c 'cat; echo'`
        // never exec-optimizes multi-command scripts, so the actual reader is
        // a grandchild: walk the descendant tree so the blocked `cat` is still
        // detected and the stdin prompt fires. Children inherit the wrapper's
        // fd 0 by default, so the same pipe/pty gate keeps unrelated readers
        // out.
        if descendant_is_reading(pid) {
            return StdinState::Reading;
        }

        StdinState::NotReading
    }

    /// `struct kinfo_proc` is not exposed by the `libc` crate, but its
    /// layout is a stable kernel ABI: total size 648 bytes with `kp_proc.p_pid`
    /// at offset 40 and `kp_eproc.e_ppid` at offset 560 (verified against
    /// `sys/sysctl.h` on macOS 15). Parsing the raw `sysctl` buffer with these
    /// offsets avoids hand-rolling the full struct and its padding rules.
    const KINFO_PROC_SIZE: usize = 648;
    const KINFO_PID_OFFSET: usize = 40;
    const KINFO_PPID_OFFSET: usize = 560;

    /// One `sysctl(KERN_PROC_ALL)` snapshot mapped to parent-child edges,
    /// so the walk costs one syscall regardless of tree width. A process that
    /// spawned between the snapshot and a thread check is simply missed until
    /// the next poll.
    fn child_map() -> std::collections::HashMap<u32, Vec<u32>> {
        let mut mib: [libc::c_int; 3] = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_ALL];
        let mut size = 0usize;
        let ret = unsafe { libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        ) };
        if ret != 0 || size == 0 || size % KINFO_PROC_SIZE != 0 {
            return std::collections::HashMap::new();
        }
        // Bound the snapshot: a busy machine can report thousands of processes,
        // and this runs inside the bash tool's poll loop.
        let count = (size / KINFO_PROC_SIZE).min(4096);
        let mut buf: Vec<u8> = vec![0u8; count * KINFO_PROC_SIZE];
        let filled = unsafe { libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr() as *mut libc::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        ) };
        if filled != 0 {
            return std::collections::HashMap::new();
        }
        let count = (size / KINFO_PROC_SIZE).min(count);
        let read_pid = |entry: &[u8], offset: usize| -> u32 {
            i32::from_ne_bytes([
                entry[offset],
                entry[offset + 1],
                entry[offset + 2],
                entry[offset + 3],
            ]) as u32
        };
        let mut map: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
        for i in 0..count {
            let entry = &buf[i * KINFO_PROC_SIZE..(i + 1) * KINFO_PROC_SIZE];
            let child = read_pid(entry, KINFO_PID_OFFSET);
            let parent = read_pid(entry, KINFO_PPID_OFFSET);
            if parent == 0 {
                continue;
            }
            map.entry(parent).or_default().push(child);
        }
        map
    }

    /// Breadth-first descent of the descendant tree looking for a blocked
    /// stdin reader, mirroring Linux's `check_process_tree`. The fd 0 gate
    /// applies to every descendant, so a background child that redirected its
    /// own stdin elsewhere never matches.
    fn descendant_is_reading(pid: u32) -> bool {
        const MAX_DEPTH: usize = 32;
        const MAX_VISITED: usize = 512;
        let children = child_map();
        let mut visited: std::collections::HashSet<u32> = std::collections::HashSet::new();
        visited.insert(pid);
        let mut queue: std::collections::VecDeque<(u32, usize)> = children
            .get(&pid)
            .map(|cs| cs.iter().map(|&c| (c, 1usize)).collect())
            .unwrap_or_default();
        while let Some((child, depth)) = queue.pop_front() {
            if !visited.insert(child) || visited.len() > MAX_VISITED || depth > MAX_DEPTH {
                continue;
            }
            if stdin_is_interactive(child as i32) && is_thread_waiting(child as i32) {
                return true;
            }
            if let Some(grandchildren) = children.get(&child) {
                for &grandchild in grandchildren {
                    queue.push_back((grandchild, depth + 1));
                }
            }
        }
        false
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
        for &thread_id in thread_ids.iter().take(num_threads) {
            let mut tinfo: proc_threadinfo = unsafe { mem::zeroed() };
            let ret = unsafe {
                proc_pidinfo(
                    pid,
                    PROC_PIDTHREADINFO,
                    thread_id,
                    &mut tinfo as *mut _ as *mut libc::c_void,
                    mem::size_of::<proc_threadinfo>() as i32,
                )
            };

            if ret > 0 && thread_is_reading_stdin(tinfo.pth_run_state, tinfo.pth_flags) {
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
