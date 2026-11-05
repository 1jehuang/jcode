//! Test-only temp directories that never inherit `TMPDIR`.
//!
//! The jcode bash tool exports `TMPDIR` pointing at its scratch dir
//! (`~/.jcode/scratch`). `tempfile::tempdir()` honors `TMPDIR`, so a test that
//! does `set_var("JCODE_HOME", tempfile::tempdir()?.path())` ends up with a
//! `JCODE_HOME` inside the live jcode tree: the test then reads and writes real
//! scratch state, and maintenance passes can race it. Such tests pass in a
//! plain shell and misbehave only when run from the jcode tool.
//!
//! These helpers anchor test temp dirs on a stable root that never resolves
//! inside the jcode home.

use std::path::PathBuf;

/// Root directory used for test temp dirs. Prefers the canonical `/private/tmp`
/// (macOS) then `/tmp`, and only falls back to `env::temp_dir()` when neither
/// exists (e.g. Windows).
pub fn root() -> PathBuf {
    for candidate in ["/private/tmp", "/tmp"] {
        let path = PathBuf::from(candidate);
        if path.is_dir() {
            return path;
        }
    }
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
