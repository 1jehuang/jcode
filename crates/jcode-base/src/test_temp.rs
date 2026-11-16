//! Test-only temp directories that never inherit `TMPDIR`.
//!
//! The jcode bash tool exports `TMPDIR` pointing at its scratch dir
//! (`~/.kcode/scratch`). `tempfile::tempdir()` honors `TMPDIR`, so a test that
//! does `set_var("JCODE_HOME", tempfile::tempdir()?.path())` ends up with a
//! `JCODE_HOME` inside the live jcode tree: the test then reads and writes real
//! scratch state, and maintenance passes can race it. Such tests pass in a
//! plain shell and misbehave only when run from the jcode tool.
//!
//! These helpers anchor test temp dirs on a stable root that never resolves
//! inside the jcode home.

use std::path::PathBuf;

/// Root directory used for test temp dirs. On unix it prefers the canonical
/// `/private/tmp` then `/tmp` and falls back to `env::temp_dir()`; on Windows it
/// always uses `env::temp_dir()`, because a `/tmp` path there is drive-relative.
pub fn root() -> PathBuf {
    // On Windows `PathBuf::from("/tmp")` is drive-relative, not absolute, and
    // `Path::is_absolute` says so. Returning it handed tests paths that the OS
    // resolved against the current drive (`C:\tmp`) while every absolute-path
    // check in the workspace rejected them. Use the real temp dir there.
    #[cfg(windows)]
    {
        return std::env::temp_dir();
    }
    #[cfg(not(windows))]
    for candidate in ["/private/tmp", "/tmp"] {
        let path = PathBuf::from(candidate);
        if path.is_dir() {
            return path;
        }
    }
    #[cfg(not(windows))]
    std::env::temp_dir()
}

/// Create a temp dir under [`root`], ignoring `TMPDIR`.
pub fn tempdir() -> std::io::Result<tempfile::TempDir> {
    tempfile::Builder::new().prefix("jcode-test-").tempdir_in(root())
}

/// Create a prefixed temp dir under [`root`], ignoring `TMPDIR`.
pub fn tempdir_with_prefix(prefix: &str) -> std::io::Result<tempfile::TempDir> {
    tempfile::Builder::new().prefix(prefix).tempdir_in(root())
}

/// Backdate a file that mtime fixtures have just rewritten.
///
/// A read-only handle cannot set timestamps, and on Windows the handle also needs
/// write access to carry `FILE_WRITE_ATTRIBUTES`. Every mtime fixture needs the
/// same open-then-stamp sequence, so it lives here instead of being repeated.
pub fn set_mtime(
    path: impl AsRef<std::path::Path>,
    modified: std::time::SystemTime,
) -> std::io::Result<()> {
    std::fs::File::options()
        .read(true)
        .write(true)
        .open(path.as_ref())?
        .set_modified(modified)
}
