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
                return StdinState::NotReading;
            }
        }

        // Fallback: /proc/PID/wchan (no special permissions needed)
        if let Ok(wchan) = std::fs::read_to_string(format!("/proc/{}/wchan", pid)) {
            let wchan = wchan.trim();
            // Common wchan values when blocked on terminal/pipe read
            if wchan == "n_tty_read"
                || wchan == "wait_woken"
                || wchan == "pipe_read"
                || wchan == "unix_stream_read_generic"
            {
                // wchan tells us it's reading, but not which fd
                // Check if stdin (fd 0) points to a pipe or pty
                if stdin_is_pipe_or_pty(pid) {
                    return StdinState::Reading;
                }
            }
            return StdinState::NotReading;
        }

        StdinState::Unknown
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

        // Check child processes
        if let Ok(entries) = std::fs::read_dir("/proc") {
            for entry in entries.flatten() {
                if let Ok(name) = entry.file_name().into_string() {
                    if let Ok(child_pid) = name.parse::<u32>() {
                        // Check if this process's parent is our pid
                        if let Ok(status) =
                            std::fs::read_to_string(format!("/proc/{}/status", child_pid))
                        {
                            for line in status.lines() {
                                if let Some(ppid_str) = line.strip_prefix("PPid:\t") {
                                    if ppid_str.trim().parse::<u32>().ok() == Some(pid) {
                                        let child_result = check(child_pid);
                                        if child_result == StdinState::Reading {
                                            return StdinState::Reading;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        result
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::mem;

    // libproc bindings
    extern "C" {
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

    const TH_STATE_WAITING: i32 = 2;

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

    pub fn check(pid: u32) -> StdinState {
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
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    #[test]
    fn test_own_process_not_reading_stdin() {
        let pid = std::process::id();
        let state = is_waiting_for_stdin(pid);
        assert_ne!(state, StdinState::Reading);
    }

    #[test]
    fn test_nonexistent_pid() {
        let state = is_waiting_for_stdin(u32::MAX);
        assert_ne!(state, StdinState::Reading);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_blocked_process_detected() {
        let mut child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("failed to spawn cat");

        let pid = child.id();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let state = linux::check_process_tree(pid);

        child.kill().ok();
        child.wait().ok();

        assert_eq!(
            state,
            StdinState::Reading,
            "cat should be waiting for stdin"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_running_process_not_reading() {
        let mut child = Command::new("sleep")
            .arg("10")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .expect("failed to spawn sleep");

        let pid = child.id();
        std::thread::sleep(std::time::Duration::from_millis(100));

        let state = linux::check(pid);

        child.kill().ok();
        child.wait().ok();

        assert_eq!(
            state,
            StdinState::NotReading,
            "sleep should not be reading stdin"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_child_process_tree_detection() {
        // bash -c "cat" spawns bash which spawns cat - cat is the one reading stdin
        let mut child = Command::new("bash")
            .arg("-c")
            .arg("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("failed to spawn bash");

        let pid = child.id();
        std::thread::sleep(std::time::Duration::from_millis(300));

        // The bash process itself may not be reading, but its child (cat) should be
        let state = linux::check_process_tree(pid);

        child.kill().ok();
        child.wait().ok();

        assert_eq!(
            state,
            StdinState::Reading,
            "child cat should be detected via process tree"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_process_that_reads_then_exits() {
        use std::io::Write;

        let mut child = Command::new("head")
            .arg("-n1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("failed to spawn head");

        let pid = child.id();
        std::thread::sleep(std::time::Duration::from_millis(200));

        // Should be reading initially
        let state_before = linux::check(pid);

        // Write a line - head should read it and exit
        if let Some(ref mut stdin) = child.stdin {
            stdin.write_all(b"hello\n").ok();
            stdin.flush().ok();
        }

        // Wait for exit
        let status = child.wait().expect("failed to wait");

        // After exit, checking the pid should not report Reading
        let state_after = is_waiting_for_stdin(pid);

        assert_eq!(
            state_before,
            StdinState::Reading,
            "head should be reading before input"
        );
        assert_ne!(
            state_after,
            StdinState::Reading,
            "head should not be reading after exit"
        );
        assert!(status.success(), "head should exit successfully");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_process_with_closed_stdin_not_reading() {
        // Spawn a process with stdin completely closed (null)
        let mut child = Command::new("cat")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .expect("failed to spawn cat");

        let pid = child.id();
        // cat with /dev/null as stdin should read EOF immediately and exit
        std::thread::sleep(std::time::Duration::from_millis(200));

        let state = is_waiting_for_stdin(pid);

        child.kill().ok();
        child.wait().ok();

        // cat with /dev/null gets EOF immediately, should not be stuck reading
        assert_ne!(state, StdinState::Reading);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_multiple_sequential_reads() {
        use std::io::Write;

        // Use a program that reads multiple lines
        let mut child = Command::new("head")
            .arg("-n2")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("failed to spawn head");

        let pid = child.id();
        std::thread::sleep(std::time::Duration::from_millis(200));

        // Should be reading first line
        let state1 = linux::check(pid);
        assert_eq!(
            state1,
            StdinState::Reading,
            "should be waiting for first line"
        );

        // Send first line
        if let Some(ref mut stdin) = child.stdin {
            stdin.write_all(b"line1\n").ok();
            stdin.flush().ok();
        }
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Should be reading second line
        let state2 = linux::check(pid);
        assert_eq!(
            state2,
            StdinState::Reading,
            "should be waiting for second line"
        );

        // Send second line
        if let Some(ref mut stdin) = child.stdin {
            stdin.write_all(b"line2\n").ok();
            stdin.flush().ok();
        }

        let status = child.wait().expect("failed to wait");
        assert!(status.success());
    }
}
