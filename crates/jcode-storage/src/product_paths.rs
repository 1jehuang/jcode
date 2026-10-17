//! Refuse accidental redirection back into the other product's storage.
use anyhow::{Result, bail};
use std::path::{Component, Path};

pub fn require_kcode_path(path: &Path) -> Result<()> {
    fn belongs_to_jcode(path: &Path) -> bool {
        path.components().any(|part| {
            let Component::Normal(name) = part else {
                return false;
            };
            let name = name.to_string_lossy();
            let name = name.to_ascii_lowercase();
            name == "jcode"
                || name.starts_with(".jcode")
                || name.starts_with("jcode.")
                || name.starts_with("jcode-")
        })
    }
    if !path.is_absolute() || belongs_to_jcode(path) {
        bail!("KCode storage must use an absolute path outside jcode directories");
    }
    // Canonicalize the existing prefix as well, so a .kcode symlink/junction
    // cannot silently route normal startup writes into .jcode.
    for ancestor in path.ancestors() {
        if ancestor.exists() {
            if belongs_to_jcode(&ancestor.canonicalize()?) {
                bail!("KCode storage resolves into a jcode directory");
            }
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_storage_cannot_be_redirected_to_jcode() {
        let temp = tempfile::tempdir().unwrap();
        for name in [".jcode", ".JCODE", "jcode", ".jcode-dev"] {
            assert!(require_kcode_path(&temp.path().join(name).join("sessions")).is_err());
        }
        assert!(require_kcode_path(&temp.path().join(".kcode")).is_ok());
        assert!(require_kcode_path(Path::new("relative")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn product_storage_cannot_alias_jcode_through_a_symlink() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join(".jcode")).unwrap();
        std::os::unix::fs::symlink(temp.path().join(".jcode"), temp.path().join(".kcode")).unwrap();
        assert!(require_kcode_path(&temp.path().join(".kcode/sessions")).is_err());
    }
}
