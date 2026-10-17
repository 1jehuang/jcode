//! Race-resistant removal of state files beneath the active Jcode root.
//!
//! Adapted from the KCode fix at ceb860bf2 ("fix(storage): allow normal
//! symlinked state home"). jcode has no dev-isolation feature, so the
//! `KCODE_DEV_NAMESPACE` branch is dropped and the root is always
//! canonicalized. This module is exposed as a public API so callers can
//! opt in to the race-resistant unlink without changing the default
//! `std::fs::remove_file` callsites.

use std::path::Path;

/// Remove a private state file without following a directory swapped in after
/// the caller's path validation. Unix traversal stays anchored to a directory
/// descriptor and uses `unlinkat(2)` for the final component.
///
/// Use this in place of `std::fs::remove_file` whenever the path is derived
/// from user input or any other source an attacker could race against.
pub fn remove_state_file(path: &Path) -> std::io::Result<()> {
    let home = crate::jcode_dir().map_err(std::io::Error::other)?;
    remove_file_beneath(&home, path)
}

/// Remove a durable state file without following a directory swapped in after
/// the caller's path validation. Durable state lives under the configured
/// runtime root rather than `KCODE_HOME`.
pub fn remove_durable_state_file(path: &Path) -> std::io::Result<()> {
    remove_file_beneath(&crate::durable_state_dir(), path)
}

fn remove_file_beneath(root: &Path, path: &Path) -> std::io::Result<()> {
    let relative = path.strip_prefix(root).map_err(std::io::Error::other)?;
    if relative.as_os_str().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "state path must name a file beneath its root",
        ));
    }

    // User-configured homes may intentionally be symlinks. Resolve the root
    // before opening it with O_NOFOLLOW so the unlink is anchored to the
    // canonical inode rather than the user-facing symlink.
    let root = root.canonicalize()?;

    #[cfg(unix)]
    {
        remove_unix(&root, relative)
    }

    #[cfg(not(unix))]
    {
        let _ = root;
        std::fs::remove_file(path)
    }
}

#[cfg(unix)]
fn remove_unix(home: &Path, relative: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;

    let mut components = relative.components().peekable();
    let mut directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(home)?;

    while let Some(component) = components.next() {
        let std::path::Component::Normal(name) = component else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "state path must be relative and contain no dot components",
            ));
        };
        let name = CString::new(name.as_bytes())
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
        if components.peek().is_some() {
            let fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            directory = unsafe { std::fs::File::from_raw_fd(fd) };
        } else {
            let result = unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) };
            if result < 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::remove_state_file;
    use std::io::Write;
    use std::os::unix::fs::symlink;

    /// Serialize tests that mutate `KCODE_HOME`. Uses the shared lock
    /// from the crate root so other test modules cannot race on the env.
    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        crate::lock_env()
    }

    #[test]
    fn removes_a_state_file_beneath_the_configured_home() {
        let _guard = lock_env();
        let home = tempfile::tempdir().expect("tempdir");
        let state_file = home.path().join("state.json");
        let mut file = std::fs::File::create(&state_file).expect("create");
        file.write_all(b"{}").expect("write");

        jcode_core::env::set_var("KCODE_HOME", home.path());
        let result = remove_state_file(&state_file);
        jcode_core::env::remove_var("KCODE_HOME");

        result.expect("remove_state_file");
        assert!(!state_file.exists(), "state file must be removed");
    }

    #[test]
    fn traverses_a_symlinked_home_to_remove_the_real_file() {
        let _guard = lock_env();
        let real_home = tempfile::tempdir().expect("tempdir");
        let state_file = real_home.path().join("state.json");
        let mut file = std::fs::File::create(&state_file).expect("create");
        file.write_all(b"{}").expect("write");

        let link_dir = tempfile::tempdir().expect("tempdir");
        let link = link_dir.path().join("jcode-link");
        symlink(real_home.path(), &link).expect("symlink");

        jcode_core::env::set_var("KCODE_HOME", &link);
        let result = remove_state_file(&link.join("state.json"));
        jcode_core::env::remove_var("KCODE_HOME");

        result.expect("remove through symlink");
        assert!(
            !state_file.exists(),
            "real file must be removed via symlink"
        );
    }

    #[test]
    fn rejects_path_outside_the_home() {
        let _guard = lock_env();
        let home = tempfile::tempdir().expect("tempdir");

        jcode_core::env::set_var("KCODE_HOME", home.path());
        let outside = home.path().parent().expect("parent").join("escape.json");
        std::fs::write(&outside, b"{}").expect("seed");
        let err = remove_state_file(&outside);
        jcode_core::env::remove_var("KCODE_HOME");

        let err = err.expect_err("must refuse outside path");
        assert_eq!(err.kind(), std::io::ErrorKind::Other);
        assert!(
            outside.exists(),
            "file outside the home must not be removed"
        );
    }

    #[test]
    fn returns_not_found_for_missing_file() {
        let _guard = lock_env();
        let home = tempfile::tempdir().expect("tempdir");

        jcode_core::env::set_var("KCODE_HOME", home.path());
        let missing = home.path().join("does-not-exist.json");
        let err = remove_state_file(&missing);
        jcode_core::env::remove_var("KCODE_HOME");

        let err = err.expect_err("missing file errors");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}
