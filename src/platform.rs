use std::path::Path;

fn desired_nofile_soft_limit(current: u64, hard: u64, minimum: u64) -> Option<u64> {
    let desired = current.max(minimum).min(hard);
    (desired > current).then_some(desired)
}

/// Create a symlink (Unix) or copy the file (Windows).
///
/// On Windows, symlinks require elevated privileges or Developer Mode,
/// so we fall back to copying.
pub fn symlink_or_copy(src: &Path, dst: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(src, dst)
    }
    #[cfg(windows)]
    {
        if src.is_dir() {
            std::os::windows::fs::symlink_dir(src, dst).or_else(|_| copy_dir_recursive(src, dst))
        } else {
            std::os::windows::fs::symlink_file(src, dst)
                .or_else(|_| std::fs::copy(src, dst).map(|_| ()))
        }
    }
}

#[cfg(windows)]
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Set file permissions to owner-only read/write (0o600).
/// No-op on Windows.
pub fn set_permissions_owner_only(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms)
    }
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
}

/// Set directory permissions to owner-only read/write/execute (0o700).
/// No-op on Windows.
pub fn set_directory_permissions_owner_only(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(path, perms)
    }
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
}

/// Set file permissions to owner read/write/execute (0o755).
/// No-op on Windows (executability is determined by file extension).
pub fn set_permissions_executable(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(path, perms)
    }
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
}

/// Best-effort increase of the current process soft `RLIMIT_NOFILE` on Unix.
///
/// This helps jcode survive short-lived reload/connect spikes even when it was
/// launched from a shell with a conservative `ulimit -n` like 1024.
pub fn raise_nofile_limit_best_effort(minimum_soft_limit: u64) {
    #[cfg(unix)]
    {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
            crate::logging::warn(&format!(
                "Failed to read RLIMIT_NOFILE: {}",
                std::io::Error::last_os_error()
            ));
            return;
        }

        let current: u64 = limit.rlim_cur;
        let hard: u64 = limit.rlim_max;
        let Some(desired) = desired_nofile_soft_limit(current, hard, minimum_soft_limit) else {
            return;
        };

        let updated = libc::rlimit {
            rlim_cur: desired as libc::rlim_t,
            rlim_max: limit.rlim_max,
        };
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &updated) } == 0 {
            crate::logging::info(&format!(
                "Raised RLIMIT_NOFILE soft limit from {} to {} (hard={})",
                current, desired, hard
            ));
        } else {
            crate::logging::warn(&format!(
                "Failed to raise RLIMIT_NOFILE from {} toward {} (hard={}): {}",
                current,
                desired,
                hard,
                std::io::Error::last_os_error()
            ));
        }
    }

    #[cfg(not(unix))]
    {
        let _ = minimum_soft_limit;
    }
}

/// Check if a process is running by PID.
///
/// On Unix, uses `kill(pid, 0)` to check without sending a signal.
/// On Windows, uses OpenProcess to query the process.
pub fn is_process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let result = unsafe { libc::kill(pid as i32, 0) };
        if result == 0 {
            return true;
        }
        let err = std::io::Error::last_os_error();
        !matches!(err.raw_os_error(), Some(code) if code == libc::ESRCH)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut exit_code = 0u32;
            let ok = GetExitCodeProcess(handle, &mut exit_code);
            CloseHandle(handle);
            ok != 0 && exit_code == STILL_ACTIVE as u32
        }
    }
}

/// Send a signal to an entire detached process group/session led by `pid`.
///
/// On Unix, detached tasks are spawned with `setsid()`, so the leader PID is
/// also the process-group/session ID. Signaling `-pid` reaches the full tree.
pub fn signal_detached_process_group(pid: u32, signal: i32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let rc = unsafe { libc::kill(-(pid as i32), signal) };
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(windows)]
    {
        let _ = signal;
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_TERMINATE, TerminateProcess,
        };
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let ok = TerminateProcess(handle, 1);
            CloseHandle(handle);
            if ok == 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
    }
}

/// Best-effort non-blocking reap for a child process owned by the current process.
///
/// Returns:
/// - `Ok(Some(exit_code))` if the child exited and was reaped now
/// - `Ok(None)` if it is still running or is not our child
pub fn try_reap_child_process(pid: u32) -> std::io::Result<Option<i32>> {
    #[cfg(unix)]
    {
        let mut status = 0;
        let rc = unsafe { libc::waitpid(pid as i32, &mut status, libc::WNOHANG) };
        if rc == 0 {
            return Ok(None);
        }
        if rc == -1 {
            let err = std::io::Error::last_os_error();
            if matches!(err.raw_os_error(), Some(code) if code == libc::ECHILD) {
                return Ok(None);
            }
            return Err(err);
        }

        if libc::WIFEXITED(status) {
            Ok(Some(libc::WEXITSTATUS(status)))
        } else if libc::WIFSIGNALED(status) {
            Ok(Some(128 + libc::WTERMSIG(status)))
        } else {
            Ok(Some(-1))
        }
    }
    #[cfg(windows)]
    {
        let _ = pid;
        Ok(None)
    }
}

/// Atomically swap a symlink by creating a temp symlink and renaming.
///
/// On Unix: creates temp symlink, then renames over target (atomic).
/// On Windows: removes target, copies source (not atomic, but best effort).
pub fn atomic_symlink_swap(src: &Path, dst: &Path, temp: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let _ = std::fs::remove_file(temp);
        std::os::unix::fs::symlink(src, temp)?;
        std::fs::rename(temp, dst)?;
    }
    #[cfg(windows)]
    {
        let _ = std::fs::remove_file(temp);
        let _ = std::fs::remove_file(dst);
        std::fs::copy(src, dst).map(|_| ())?;
    }
    Ok(())
}

/// Spawn a process detached from the current client session.
///
/// This is used for launching new terminal windows (for `/resume`, `/split`,
/// crash restore, etc.) so the new client survives if the invoking jcode
/// process exits or its terminal closes.
pub fn spawn_detached(cmd: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};

        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }

    cmd.spawn()
}

#[cfg(windows)]
fn spawn_replacement_process(
    cmd: &mut std::process::Command,
) -> std::io::Result<std::process::Child> {
    cmd.spawn()
}

/// Replace the current process with a new command (exec on Unix).
///
/// On Unix, this calls exec() which never returns on success.
/// On Windows, this spawns the process and exits.
///
/// Returns an error only if the operation fails. On success (Unix exec),
/// this function never returns.
pub fn replace_process(cmd: &mut std::process::Command) -> std::io::Error {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        crate::logging::error(&format!(
            "replace_process failed: {} ({})",
            err,
            crate::util::process_fd_diagnostic_snapshot()
        ));
        err
    }
    #[cfg(windows)]
    {
        match spawn_replacement_process(cmd) {
            Ok(_child) => std::process::exit(0),
            Err(e) => e,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desired_nofile_soft_limit_only_raises_when_possible() {
        assert_eq!(desired_nofile_soft_limit(1024, 524_288, 8192), Some(8192));
        assert_eq!(desired_nofile_soft_limit(8192, 524_288, 8192), None);
        assert_eq!(desired_nofile_soft_limit(1024, 4096, 8192), Some(4096));
    }

    #[cfg(unix)]
    #[test]
    fn spawn_detached_creates_new_session() {
        use tempfile::NamedTempFile;

        let output = NamedTempFile::new().expect("temp file");
        let output_path = output.path().to_string_lossy().to_string();
        let parent_sid = unsafe { libc::getsid(0) };

        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg("ps -o sid= -p $$ > \"$JCODE_TEST_OUTPUT\"")
            .env("JCODE_TEST_OUTPUT", &output_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        let mut child = super::spawn_detached(&mut cmd).expect("spawn detached child");
        let status = child.wait().expect("wait for child");
        assert!(status.success(), "child should exit successfully");

        let child_sid = std::fs::read_to_string(&output_path)
            .expect("read child sid")
            .trim()
            .parse::<u32>()
            .expect("parse child sid");

        assert_eq!(
            child_sid,
            child.id(),
            "detached child should lead its own session"
        );
        assert_ne!(
            child_sid as i32, parent_sid,
            "detached child should not share parent session"
        );
    }

    #[cfg(windows)]
    #[test]
    fn is_process_running_reports_exited_children_as_stopped() {
        use std::process::{Command, Stdio};
        use std::time::Duration;

        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/C", "ping -n 3 127.0.0.1 >NUL"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let mut child = cmd.spawn().expect("spawn child");
        let pid = child.id();
        assert!(
            super::is_process_running(pid),
            "child should initially be running"
        );

        let status = child.wait().expect("wait for child");
        assert!(status.success(), "child should exit successfully");
        std::thread::sleep(Duration::from_millis(100));

        assert!(
            !super::is_process_running(pid),
            "exited child should not be reported as running"
        );
    }

    #[cfg(windows)]
    #[test]
    fn spawn_replacement_process_returns_without_waiting_for_child_exit() {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/C", "ping -n 4 127.0.0.1 >NUL"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let start = Instant::now();
        let mut child = super::spawn_replacement_process(&mut cmd)
            .expect("spawn replacement process should succeed");
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_secs(1),
            "replacement spawn should not block, took {:?}",
            elapsed
        );
        assert!(
            child.try_wait().expect("poll child status").is_none(),
            "replacement child should still be running immediately after spawn"
        );

        child.kill().ok();
        let _ = child.wait();
    }
}
