//! Refuse accidental redirection back into the other product's storage.
//!
//! The hazard this guards is concrete: a `KCODE_HOME`/`JCODE_HOME` that resolves
//! to the *user's real* jcode home makes this product read and write another
//! product's sessions, config and credentials. That is the case worth refusing.
//!
//! An earlier version refused any path containing a `.jcode` or `jcode`
//! component. That is too blunt: it rejected every test sandbox and every
//! deliberately redirected home that happens to be named `.jcode` (twenty-plus
//! of this repository's own tests do exactly that), and those paths cannot reach
//! the real jcode home. The check is therefore anchored to the one directory
//! that actually matters.
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

/// The user's real jcode home, the directory this product must never adopt.
fn real_jcode_home() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".jcode"))
}

/// Resolve symlinks and `..` in the longest existing prefix of `path`, then
/// re-append the part that does not exist yet.
fn resolve_existing_prefix(path: &Path) -> PathBuf {
    let mut remainder: Vec<&std::ffi::OsStr> = Vec::new();
    let mut cursor = path;
    loop {
        if let Ok(canonical) = cursor.canonicalize() {
            let mut resolved = canonical;
            for part in remainder.iter().rev() {
                resolved.push(part);
            }
            return resolved;
        }
        let Some(parent) = cursor.parent() else {
            return path.to_path_buf();
        };
        if let Some(name) = cursor.file_name() {
            remainder.push(name);
        }
        cursor = parent;
    }
}

pub fn require_kcode_path(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        bail!("KCode storage must use an absolute path");
    }
    let Some(jcode_home) = real_jcode_home() else {
        return Ok(());
    };
    let jcode_home = resolve_existing_prefix(&jcode_home);
    let target = resolve_existing_prefix(path);
    if target == jcode_home || target.starts_with(&jcode_home) {
        // Resolving the prefix also catches a `.kcode` symlink or junction that
        // points at `.jcode`, which a textual check would miss.
        bail!(
            "KCode storage must stay outside the jcode home ({})",
            jcode_home.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_jcode_sessions() -> PathBuf {
        real_jcode_home().unwrap().join("sessions")
    }

    #[test]
    fn refuses_the_real_jcode_home_and_anything_below_it() {
        let home = real_jcode_home().unwrap();
        assert!(require_kcode_path(&home).is_err());
        assert!(require_kcode_path(&home_jcode_sessions()).is_err());
        assert!(require_kcode_path(&home.join("config").join("config.toml")).is_err());
    }

    #[test]
    fn allows_an_explicitly_redirected_sandbox_home() {
        // A test sandbox or a deliberate override may be named anything,
        // including `.jcode`; it cannot reach the real home.
        let temp = tempfile::tempdir().unwrap();
        assert!(require_kcode_path(&temp.path().join(".jcode").join("sessions")).is_ok());
        assert!(require_kcode_path(&temp.path().join(".kcode")).is_ok());
        assert!(require_kcode_path(&temp.path().join("jcode")).is_ok());
    }

    #[test]
    fn refuses_relative_paths() {
        assert!(require_kcode_path(Path::new("relative")).is_err());
        assert!(require_kcode_path(Path::new(".kcode/sessions")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlink_that_resolves_into_the_real_jcode_home() {
        let temp = tempfile::tempdir().unwrap();
        let link = temp.path().join(".kcode");
        std::os::unix::fs::symlink(real_jcode_home().unwrap(), &link).unwrap();
        assert!(require_kcode_path(&link.join("sessions")).is_err());
    }
}
