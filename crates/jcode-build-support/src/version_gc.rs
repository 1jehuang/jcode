//! Daily garbage collection for `builds/versions/<version>/`.
//!
//! Every self-dev build, local release install, and update writes a new
//! immutable binary directory (roughly 400 MB each). Installers prune right
//! after publishing; this daemon pass catches builds left by other paths. Both
//! run the same locked pruner (see `prune`), which keeps every version that is
//! still referenced or running plus the most recently installed few.

use crate::builds_dir;
use std::path::Path;
use std::time::SystemTime;

/// Unreferenced versions kept for rollback, newest first.
pub const DEFAULT_KEEP_RECENT_VERSIONS: usize = 5;

/// Override how many unreferenced versions to keep. `0` disables cleanup.
pub const KEEP_VERSIONS_ENV: &str = "JCODE_KEEP_BUILD_VERSIONS";

const PRUNE_INTERVAL_SECS: u64 = 24 * 60 * 60;
const PRUNE_MARKER: &str = "versions-prune.stamp";

/// Remove old unreferenced version directories. Best-effort, rate limited to
/// one pass per day per machine, safe to call from a startup thread.
pub fn prune_old_versions() {
    if crate::keep_versions_setting().is_none() {
        return;
    }
    let Ok(dir) = builds_dir() else {
        return;
    };
    if !prune_due(&dir) {
        return;
    }
    crate::prune_old_versions_after_install();
    mark_pruned(&dir);
}

/// True when the last completed pass is older than the interval. The marker
/// is only written after a pass finishes (see [`mark_pruned`]), so a process
/// killed mid-walk (for example by an exec reload) does not suppress the next
/// attempt for a whole day.
fn prune_due(dir: &Path) -> bool {
    let marker = dir.join(PRUNE_MARKER);
    match std::fs::metadata(&marker).and_then(|meta| meta.modified()) {
        Ok(modified) => SystemTime::now()
            .duration_since(modified)
            .map(|age| age.as_secs() >= PRUNE_INTERVAL_SECS)
            .unwrap_or(true),
        Err(_) => true,
    }
}

fn mark_pruned(dir: &Path) {
    let _ = std::fs::write(dir.join(PRUNE_MARKER), b"");
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use crate::tests::with_temp_jcode_home;

    #[test]
    fn daily_pass_prunes_with_the_shared_pruner_once_per_interval() {
        with_temp_jcode_home(|| {
            let versions = builds_dir().unwrap().join("versions");
            let installed = || std::fs::read_dir(&versions).unwrap().count();
            for index in 0..DEFAULT_KEEP_RECENT_VERSIONS + 2 {
                let dir = versions.join(format!("v{index}"));
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join(crate::binary_name()), b"old build").unwrap();
            }

            prune_old_versions();
            assert_eq!(installed(), DEFAULT_KEEP_RECENT_VERSIONS);

            // Within the interval the completion marker suppresses another pass.
            std::fs::create_dir_all(versions.join("later")).unwrap();
            prune_old_versions();
            assert_eq!(installed(), DEFAULT_KEEP_RECENT_VERSIONS + 1);
        });
    }
}
