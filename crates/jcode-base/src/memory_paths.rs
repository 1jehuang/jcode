//! Project-scoped storage paths for memories and legacy notes.
//!
//! Both paths are keyed by [`crate::project_scope::project_key`] so a migrated
//! session keeps its data, and both carry forward files written under the old
//! `DefaultHasher` key so an upgrade does not silently wipe user data (P3.1).

use std::path::PathBuf;

use anyhow::Result;

use crate::storage;

/// Per-project memory file for `project_dir`. Keyed by `project_scope::project_key`,
/// so a migrated session that keeps the same repo keeps the same memories, and two
/// spellings of one repo (differing case, trailing separator, a junction) do not
/// fragment them (P3.1).
pub fn project_memory_file(project_dir: &std::path::Path) -> Result<PathBuf> {
    let project_hash = crate::project_scope::project_key(project_dir);
    let memory_dir = storage::jcode_dir()?.join("memory").join("projects");
    let path = memory_dir.join(format!("{}.json", project_hash));

    // Carry forward a file written under the old DefaultHasher key, otherwise
    // every existing user's memories would silently vanish on this upgrade.
    // A failure stays non-fatal: it must not block the session from starting,
    // but it is logged because a dropped migration is indistinguishable from a
    // user who never had memories in this project.
    let legacy = memory_dir.join(format!(
        "{}.json",
        crate::project_scope::legacy_project_key(project_dir)
    ));
    if legacy != path {
        if let Err(migrate_err) = crate::project_scope::migrate_legacy_project_key(&legacy, &path) {
            crate::logging::warn(&format!(
                "Could not migrate legacy memory {} to {}: {migrate_err}",
                legacy.display(),
                path.display()
            ));
        }
    }
    Ok(path)
}

/// Per-project legacy notes file for `project_dir`, keyed the same way as
/// [`project_memory_file`] and carrying an old-keyed file forward on upgrade.
pub fn legacy_notes_file(project_dir: &std::path::Path) -> Result<PathBuf> {
    let project_hash = crate::project_scope::project_key(project_dir);
    let notes_dir = storage::jcode_dir()?.join("notes");
    let notes_path = notes_dir.join(format!("{}.json", project_hash));

    // Carry forward a file written under the old DefaultHasher key, otherwise
    // every existing user's notes would vanish on this upgrade (P3.1).
    let legacy = notes_dir.join(format!(
        "{}.json",
        crate::project_scope::legacy_project_key(project_dir)
    ));
    if legacy != notes_path {
        if let Err(migrate_err) =
            crate::project_scope::migrate_legacy_project_key(&legacy, &notes_path)
        {
            crate::logging::warn(&format!(
                "Could not migrate legacy notes {} to {}: {migrate_err}",
                legacy.display(),
                notes_path.display()
            ));
        }
    }

    Ok(notes_path)
}
