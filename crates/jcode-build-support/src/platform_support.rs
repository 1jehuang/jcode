use std::path::Path;

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
