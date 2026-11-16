use std::path::{Path, PathBuf};

/// Cargo package names that identify a Desktop checkout. The Desktop repo ships
/// as `jcode-desktop`; the kcode spelling is accepted so a renamed checkout
/// still resolves.
const DESKTOP_PACKAGE_NAMES: &[&str] = &["jcode-desktop", "kcode-desktop"];

/// Desktop UI crate directories, matching [`DESKTOP_PACKAGE_NAMES`].
const DESKTOP_UI_DIRS: &[&str] = &["crates/jcode-desktop-ui", "crates/kcode-desktop-ui"];

/// Find the Jcode Desktop checkout containing `path`.
///
/// Identity comes from the Cargo package and Desktop UI directory, never the
/// checkout's name. Resolve symlinks before walking ancestors so linked nested
/// directories work as well as renamed checkouts. The returned root is canonical.
pub fn desktop_repo_root(path: &Path) -> Option<PathBuf> {
    let canonical = path.canonicalize().ok()?;
    canonical.ancestors().find_map(|ancestor| {
        if !DESKTOP_UI_DIRS
            .iter()
            .any(|dir| ancestor.join(dir).is_dir())
        {
            return None;
        }
        let manifest = std::fs::read_to_string(ancestor.join("Cargo.toml")).ok()?;
        let manifest: toml::Value = toml::from_str(&manifest).ok()?;
        let name = manifest.get("package")?.get("name")?.as_str()?;
        DESKTOP_PACKAGE_NAMES
            .contains(&name)
            .then(|| ancestor.to_path_buf())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkout() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("crates/jcode-desktop-ui/src")).unwrap();
        std::fs::write(
            root.path().join("Cargo.toml"),
            "[workspace]\nmembers = ['crates/jcode-desktop-ui']\n[package]\nname = 'jcode-desktop' # actual package\nversion = '0.1.0'\n",
        ).unwrap();
        root
    }

    #[test]
    fn detects_renamed_checkout_and_nested_files() {
        let root = checkout();
        let expected = root.path().canonicalize().unwrap();
        for relative in ["", "crates/jcode-desktop-ui/src", "Cargo.toml"] {
            assert_eq!(
                desktop_repo_root(&root.path().join(relative)),
                Some(expected.clone())
            );
        }
    }

    #[test]
    fn rejects_unrelated_missing_and_malformed_manifests() {
        let root = checkout();
        for manifest in [
            "[package]\nname = 'other'\n",
            "# [package]\n# name = 'jcode-desktop'\n",
            "[workspace.package]\nname = 'jcode-desktop'\n",
            "[package]\nname = 'jcode-desktop'\ninvalid TOML",
        ] {
            std::fs::write(root.path().join("Cargo.toml"), manifest).unwrap();
            assert_eq!(desktop_repo_root(root.path()), None);
        }
        std::fs::remove_file(root.path().join("Cargo.toml")).unwrap();
        assert_eq!(desktop_repo_root(root.path()), None);
        assert_eq!(desktop_repo_root(&root.path().join("missing")), None);
    }

    #[test]
    fn requires_desktop_ui_directory() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("Cargo.toml"),
            "[package]\nname='jcode-desktop'\n",
        )
        .unwrap();
        assert_eq!(desktop_repo_root(root.path()), None);
    }

    /// The Desktop repo can be renamed to the kcode spelling; the layout is the
    /// same, so detection must not depend on the jcode package name alone.
    #[test]
    fn detects_a_kcode_named_checkout() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("crates/kcode-desktop-ui/src")).unwrap();
        std::fs::write(
            root.path().join("Cargo.toml"),
            "[package]\nname = 'kcode-desktop'\nversion = '0.1.0'\n",
        )
        .unwrap();
        assert_eq!(
            desktop_repo_root(root.path()),
            Some(root.path().canonicalize().unwrap())
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolves_checkout_and_nested_directory_symlinks() {
        let root = checkout();
        let links = tempfile::tempdir().unwrap();
        let expected = root.path().canonicalize().unwrap();
        for (name, target) in [
            ("checkout", root.path().to_path_buf()),
            ("nested", root.path().join("crates/jcode-desktop-ui/src")),
        ] {
            let link = links.path().join(name);
            std::os::unix::fs::symlink(target, &link).unwrap();
            assert_eq!(desktop_repo_root(&link), Some(expected.clone()));
        }
    }
}
