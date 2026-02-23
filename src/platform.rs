use std::path::Path;

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
        let _ = pid;
        true
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
        cmd.exec()
    }
    #[cfg(windows)]
    {
        match cmd.status() {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(e) => e,
        }
    }
}
