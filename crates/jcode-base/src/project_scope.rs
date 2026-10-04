//! Stable project scope keys.
//!
//! One `jcode` daemon serves sessions in many project directories at once, so
//! any process-wide resource that a project can influence has to be keyed by
//! *which project* it came from, not by a bare name (see
//! `docs/plans/PROJECT_ISOLATION_HARDENING_PLAN.md`).
//!
//! This module is the single shared place that derives such a key. Deriving it
//! in one place is the whole point: two subsystems that each canonicalize a
//! path slightly differently will silently split one project in half, which is
//! the exact failure mode this key exists to prevent (P3.1).

use std::path::Path;

/// Normalize a path for comparison/hashing without requiring it to exist.
///
/// `canonicalize` is tried first because it resolves symlinks, junctions, `..`
/// and relative segments into one spelling. It fails for paths that do not
/// exist yet, so the lexical fallback is used instead.
fn normalize_path(path: &Path) -> String {
    let resolved = path
        .canonicalize()
        .ok()
        .unwrap_or_else(|| path.to_path_buf());
    let text = strip_verbatim_prefix(&resolved.to_string_lossy());
    trim_trailing_separator(&text)
}

/// Remove the Windows verbatim (`\\?\`) prefix that `canonicalize` adds.
///
/// Without this, one directory hashes differently depending on whether the
/// caller spelled it `C:\repo` or received `\\?\C:\repo` back from the OS,
/// which would split a single project into two buckets. UNC targets are
/// rewritten back to their normal `\\server\share` form.
fn strip_verbatim_prefix(text: &str) -> String {
    let Some(rest) = text.strip_prefix(r"\\?\") else {
        return text.to_string();
    };
    match rest.strip_prefix(r"UNC\") {
        Some(server_share) => format!(r"\\{server_share}"),
        None => rest.to_string(),
    }
}

/// Drop trailing separators, but keep a bare root (`/`, `C:\`) intact.
///
/// A naive `trim_end_matches` would turn `C:\` into `C:`, which Win32 reads as
/// "the current directory on drive C" rather than the drive root.
fn trim_trailing_separator(text: &str) -> String {
    let trimmed = text.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        // A path made only of separators is a root; it names itself.
        return text.to_string();
    }
    if trimmed.len() == 2 && trimmed.as_bytes()[1] == b':' {
        return format!("{}\\", trimmed);
    }
    trimmed.to_string()
}

/// Case-fold only where paths are case-insensitive.
///
/// On case-sensitive filesystems `C:\Repo` and `c:\repo` are genuinely
/// different directories, so folding there would merge two real projects.
fn case_fold(text: &str) -> String {
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text.to_string()
    }
}

/// A stable, canonical identity for one project directory.
///
/// Two spellings of the same directory (`/repo` vs `/repo/.`, a Windows path
/// differing only in case, or a path carrying the `\\?\` verbatim prefix) map
/// to one key. Two different directories map to different keys.
///
/// The digest is a hex SHA-256 prefix, deliberately *not* `DefaultHasher`,
/// whose output is explicitly documented as unstable across Rust releases
/// (P3.1 problem 1). The algorithm and prefix are part of this function's
/// contract: changing either orphans every stored key.
pub fn project_key(path: &Path) -> String {
    let normalized = case_fold(&normalize_path(path));
    stable_digest(&normalized)
}

/// The key for a project that may be unknown (`working_dir: None`).
///
/// `None` is never resolved to the daemon's cwd: one daemon serves many
/// projects, so its cwd is an arbitrary one of them. An unknown project gets
/// its own bucket, which keeps project-scoped state from being shared across
/// projects or attributed to whichever project happened to start the daemon.
pub fn optional_project_key(path: Option<&Path>) -> String {
    match path {
        Some(path) => project_key(path),
        None => "none".to_string(),
    }
}

/// The pre-P3.1 project key: `DefaultHasher` over the raw `PathBuf`.
///
/// Kept only so [`migrate_legacy_project_key`] can find files written by an
/// older build. It reproduces the old derivation exactly, quirks included, and
/// must never be used to *name* new data: `DefaultHasher` is explicitly
/// documented as unstable across Rust releases, so a toolchain bump would
/// change this value and strand the very files the migration exists to find.
pub fn legacy_project_key(path: &Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Move a project-scoped file or directory from the legacy key to [`project_key`].
///
/// Returns `true` if a migration happened. The legacy location is left in
/// place: it is the only copy of that data until the new location has been
/// written, and the caller decides when to remove it.
pub fn migrate_legacy_project_key(old_item: &Path, new_item: &Path) -> std::io::Result<bool> {
    if !old_item.exists() || new_item.exists() {
        return Ok(false);
    }
    if let Some(parent) = new_item.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::rename(old_item, new_item) {
        Ok(()) => Ok(true),
        // A rename across devices (an old store on another mount) cannot be
        // atomic. Copy then remove, and report failure rather than pretending.
        Err(_) => {
            copy_recursively(old_item, new_item)?;
            std::fs::remove_dir_all(old_item).or_else(|_| std::fs::remove_file(old_item))?;
            Ok(true)
        }
    }
}

fn copy_recursively(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_recursively(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// Stable hex SHA-256 prefix. See [`project_key`] for why this is not
/// `DefaultHasher`.
///
/// The output is deliberately bare hex with no algorithm prefix: these keys name
/// directories as well as scope strings, and `:` is an illegal character in a
/// Windows path component. The prefix this function used to emit
/// (`sha256:<hex>`) only became a real bug once P3.1 started using the key as a
/// directory name, which is why it is worth stating rather than assuming.
fn stable_digest(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(input.as_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        legacy_project_key, migrate_legacy_project_key, optional_project_key, project_key,
        strip_verbatim_prefix,
    };
    use std::path::{Path, PathBuf};

    /// Compare two spellings without tripping over the verbatim prefix that
    /// `canonicalize` adds on Windows and strips in the key derivation itself.
    fn spelling_variants(path: &Path) -> Vec<PathBuf> {
        let canonical = path.canonicalize().expect("canonicalize");
        let text = canonical.to_string_lossy().replace(r"\\?\", "");
        let plain = PathBuf::from(text);
        vec![canonical, plain.clone(), plain.join(".")]
    }

    #[test]
    fn project_key_is_stable_across_calls() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(project_key(dir.path()), project_key(dir.path()));
    }

    #[test]
    fn project_key_ignores_trailing_separator_and_dot_segments() {
        let dir = tempfile::tempdir().expect("tempdir");

        let mut keys = Vec::new();
        for variant in spelling_variants(dir.path()) {
            keys.push((variant.clone(), project_key(&variant)));
        }

        let first = &keys[0].1;
        for (variant, key) in &keys[1..] {
            assert_eq!(
                first, key,
                "'{variant:?}' must resolve to the same project as '{:?}'",
                keys[0].0
            );
        }
    }

    #[test]
    fn project_key_separates_different_directories() {
        let first = tempfile::tempdir().expect("first tempdir");
        let second = tempfile::tempdir().expect("second tempdir");

        assert_ne!(
            project_key(first.path()),
            project_key(second.path()),
            "two different projects must never share a key"
        );
    }

    #[test]
    fn project_key_survives_a_directory_that_does_not_exist() {
        // Sessions can target a directory before it is created, so a missing
        // path must still yield a usable key instead of erroring.
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("not-created-yet");

        assert_ne!(project_key(&missing), project_key(dir.path()));
        assert!(
            project_key(&missing).chars().all(|c| c.is_ascii_hexdigit()),
            "a project key must be usable as a filename on every platform, got {:?}",
            project_key(&missing)
        );
    }

    #[test]
    fn optional_project_key_separates_unknown_from_a_real_project() {
        let dir = tempfile::tempdir().expect("tempdir");

        assert_eq!(optional_project_key(None), "none");
        assert_ne!(
            optional_project_key(None),
            optional_project_key(Some(dir.path())),
            "an unknown project must not inherit a real project's bucket"
        );
    }

    #[test]
    fn verbatim_prefix_is_stripped_from_drive_and_unc_paths() {
        assert_eq!(strip_verbatim_prefix(r"\\?\C:\repo"), r"C:\repo");
        assert_eq!(
            strip_verbatim_prefix(r"\\?\UNC\srv\share\repo"),
            r"\\srv\share\repo"
        );
        // Non-Windows and already-normal paths pass through untouched.
        assert_eq!(strip_verbatim_prefix("/repo"), "/repo");
        assert_eq!(strip_verbatim_prefix(r"C:\repo"), r"C:\repo");
    }

    #[test]
    fn legacy_project_key_reproduces_the_pre_p31_derivation() {
        // The whole migration depends on this reproducing the old value exactly.
        // If it drifts, every existing user's memories and goals are stranded
        // with no way to find them.
        let dir = tempfile::tempdir().expect("tempdir");
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        dir.path().hash(&mut hasher);
        let expected = format!("{:016x}", hasher.finish());
        assert_eq!(legacy_project_key(dir.path()), expected);

        // Positive control: the legacy key must NOT equal the new one, or the
        // migration would have nothing to move and this test proves nothing.
        assert_ne!(legacy_project_key(dir.path()), project_key(dir.path()));
    }

    #[test]
    fn migration_moves_a_legacy_file_to_the_stable_key() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = tempfile::tempdir().expect("tempdir");
        let old = root
            .path()
            .join(format!("{}.json", legacy_project_key(dir.path())));
        let new = root
            .path()
            .join(format!("{}.json", project_key(dir.path())));
        std::fs::write(&old, "{\"memories\":[]}").expect("seed legacy");

        assert!(migrate_legacy_project_key(&old, &new).expect("migrate"));
        assert!(new.exists(), "the file must be reachable at the new key");
        assert_eq!(
            std::fs::read_to_string(&new).expect("read"),
            "{\"memories\":[]}",
            "the migrated file must keep its contents"
        );
    }

    #[test]
    fn migration_leaves_an_existing_new_key_alone() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = tempfile::tempdir().expect("tempdir");
        let old = root
            .path()
            .join(format!("{}.json", legacy_project_key(dir.path())));
        let new = root
            .path()
            .join(format!("{}.json", project_key(dir.path())));
        std::fs::write(&old, "legacy").expect("seed legacy");
        std::fs::write(&new, "current").expect("seed current");

        assert!(
            !migrate_legacy_project_key(&old, &new).expect("migrate"),
            "a project that already has data under the new key must not be overwritten"
        );
        assert_eq!(
            std::fs::read_to_string(&new).expect("read"),
            "current",
            "the newer data must win"
        );
    }

    #[test]
    fn migration_moves_a_legacy_goal_directory_with_its_contents() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = tempfile::tempdir().expect("tempdir");
        let old = root.path().join(legacy_project_key(dir.path()));
        let new = root.path().join(project_key(dir.path()));
        std::fs::create_dir_all(old.join("nested")).expect("seed legacy dir");
        std::fs::write(old.join("nested").join("goal.json"), "{}").expect("seed goal");

        assert!(migrate_legacy_project_key(&old, &new).expect("migrate"));
        assert_eq!(
            std::fs::read_to_string(new.join("nested").join("goal.json")).expect("read"),
            "{}",
            "a goal directory must migrate recursively, not as an empty folder"
        );
    }

    #[test]
    fn migration_is_a_no_op_when_there_is_nothing_to_move() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = tempfile::tempdir().expect("tempdir");
        let old = root
            .path()
            .join(format!("{}.json", legacy_project_key(dir.path())));
        let new = root
            .path()
            .join(format!("{}.json", project_key(dir.path())));
        assert!(
            !migrate_legacy_project_key(&old, &new).expect("migrate"),
            "a fresh install has no legacy file and must report no migration"
        );
    }
}
