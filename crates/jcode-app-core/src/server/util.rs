use crate::build;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::OnceCell;

/// Default embedding idle unload threshold (15 minutes).
const EMBEDDING_IDLE_UNLOAD_DEFAULT_SECS: u64 = 15 * 60;

pub(crate) fn debug_control_allowed() -> bool {
    // Check config file setting
    if crate::config::config().display.debug_socket {
        return true;
    }
    if std::env::var("JCODE_DEBUG_CONTROL")
        .ok()
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
    {
        return true;
    }
    // Check for file-based toggle (allows enabling without restart)
    if let Ok(jcode_dir) = crate::storage::jcode_dir()
        && jcode_dir.join("debug_control").exists()
    {
        return true;
    }
    false
}

pub(crate) fn embedding_idle_unload_secs() -> u64 {
    std::env::var("JCODE_EMBEDDING_IDLE_UNLOAD_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(EMBEDDING_IDLE_UNLOAD_DEFAULT_SECS)
}

pub(crate) async fn get_shared_mcp_pool(
    cell: &OnceCell<Arc<crate::mcp::SharedMcpPool>>,
) -> Arc<crate::mcp::SharedMcpPool> {
    cell.get_or_init(|| async { Arc::new(crate::mcp::SharedMcpPool::from_default_config()) })
        .await
        .clone()
}

pub(crate) fn server_update_candidate(is_selfdev_session: bool) -> Option<(PathBuf, &'static str)> {
    build::shared_server_update_candidate(is_selfdev_session)
}

fn canonicalize_or(path: PathBuf) -> PathBuf {
    std::fs::canonicalize(&path).unwrap_or(path)
}

pub(crate) fn git_common_dir_for(path: &Path) -> Option<PathBuf> {
    let mut current = Some(path);
    while let Some(dir) = current {
        let dotgit = dir.join(".git");
        if dotgit.is_dir() {
            return Some(canonicalize_or(dotgit));
        }
        if dotgit.is_file() {
            let content = std::fs::read_to_string(&dotgit).ok()?;
            let gitdir_line = content
                .lines()
                .find(|line| line.trim_start().starts_with("gitdir:"))?;
            let raw = gitdir_line
                .trim_start()
                .trim_start_matches("gitdir:")
                .trim();
            if raw.is_empty() {
                return None;
            }
            let gitdir = if Path::new(raw).is_absolute() {
                PathBuf::from(raw)
            } else {
                dir.join(raw)
            };
            let gitdir = canonicalize_or(gitdir);
            // Worktree gitdir looks like: <repo>/.git/worktrees/<name>
            if let Some(parent) = gitdir.parent()
                && parent.file_name().and_then(|s| s.to_str()) == Some("worktrees")
                && let Some(common) = parent.parent()
            {
                return Some(canonicalize_or(common.to_path_buf()));
            }
            return Some(gitdir);
        }
        current = dir.parent();
    }
    None
}

pub(crate) fn swarm_id_for_dir(dir: Option<PathBuf>) -> Option<String> {
    if let Ok(sw_id) = std::env::var("JCODE_SWARM_ID") {
        let trimmed = sw_id.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    let dir = dir?;
    if let Some(git_common) = git_common_dir_for(&dir) {
        return Some(git_common.to_string_lossy().to_string());
    }
    Some(dir.to_string_lossy().to_string())
}

/// Decide whether any reload candidate is *provably* newer than the running
/// server binary.
///
/// This is intentionally conservative. An earlier version reported "update
/// available" whenever the mtime comparison was inconclusive (e.g. a metadata
/// read failed) as long as the candidate path differed from the running exe.
/// On some systems that fallback fired permanently, so the client would
/// auto-reload the server, the server would exec into the candidate, and the
/// freshly-exec'd server would again report an update -> an infinite reload
/// loop that flickers the terminal (see issue #277).
///
/// We now only report an update when we can read both mtimes and the candidate
/// is strictly newer than the running binary. Any uncertainty suppresses the
/// auto-reload signal so it can never wedge the client into a loop.
fn newer_binary_available(
    current_mtime: Option<std::time::SystemTime>,
    current_canonical: Option<&Path>,
    candidates: impl IntoIterator<Item = (PathBuf, Option<std::time::SystemTime>)>,
) -> bool {
    let Some(current_time) = current_mtime else {
        crate::logging::warn(
            "server_has_newer_binary: current executable mtime unavailable; suppressing auto-reload update signal",
        );
        return false;
    };

    candidates.into_iter().any(|(candidate, candidate_mtime)| {
        // Reloading into ourselves is never an "update".
        if current_canonical == Some(candidate.as_path()) {
            return false;
        }

        match candidate_mtime {
            Some(candidate_time) => candidate_time > current_time,
            None => {
                crate::logging::warn(&format!(
                    "server_has_newer_binary: candidate mtime unavailable for {}; suppressing auto-reload update signal",
                    candidate.display()
                ));
                false
            }
        }
    })
}

pub(crate) fn server_has_newer_binary() -> bool {
    let current_exe = std::env::current_exe().ok();
    let current_mtime = current_exe
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());
    let current_canonical = current_exe
        .as_ref()
        .map(|path| canonicalize_or(path.clone()));

    let mut candidates = HashSet::new();
    for is_selfdev_session in [false, true] {
        if let Some((candidate, _label)) = server_update_candidate(is_selfdev_session) {
            candidates.insert(canonicalize_or(candidate));
        }
    }

    let candidates_with_mtimes = candidates.into_iter().map(|candidate| {
        let candidate_mtime = std::fs::metadata(&candidate)
            .ok()
            .and_then(|m| m.modified().ok());
        (candidate, candidate_mtime)
    });

    newer_binary_available(
        current_mtime,
        current_canonical.as_deref(),
        candidates_with_mtimes,
    )
}

/// Server identity for multi-server support
#[derive(Debug, Clone)]
pub struct ServerIdentity {
    /// Full server ID (e.g., "server_blazing_1705012345678")
    pub id: String,
    /// Short name (e.g., "blazing")
    pub name: String,
    /// Icon for display (e.g., "🔥")
    pub icon: String,
    /// Git hash of the binary
    pub git_hash: String,
    /// Version string (e.g., "v0.1.123")
    pub version: String,
}

impl ServerIdentity {
    /// Display name with icon (e.g., "🔥 blazing")
    pub fn display_name(&self) -> String {
        format!("{} {}", self.icon, self.name)
    }
}

pub(crate) fn startup_headless_recovery_test_delay() -> Option<std::time::Duration> {
    let raw = std::env::var("JCODE_TEST_HEADLESS_STARTUP_RECOVERY_DELAY_MS").ok()?;
    let delay_ms = raw.trim().parse::<u64>().ok()?;
    (delay_ms > 0).then(|| std::time::Duration::from_millis(delay_ms))
}

#[cfg(test)]
mod newer_binary_tests {
    use super::newer_binary_available;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn reports_update_when_candidate_is_strictly_newer() {
        let candidates = vec![(PathBuf::from("/x/stable/jcode"), Some(t(200)))];
        assert!(newer_binary_available(
            Some(t(100)),
            Some(std::path::Path::new("/x/current/jcode")),
            candidates,
        ));
    }

    #[test]
    fn ignores_candidate_that_is_not_newer() {
        let candidates = vec![(PathBuf::from("/x/stable/jcode"), Some(t(100)))];
        assert!(!newer_binary_available(
            Some(t(100)),
            Some(std::path::Path::new("/x/current/jcode")),
            candidates,
        ));
    }

    #[test]
    fn never_reloads_into_self_even_if_paths_were_equal() {
        // Same canonical path must never count as an update, regardless of mtime.
        let candidates = vec![(PathBuf::from("/x/current/jcode"), Some(t(999)))];
        assert!(!newer_binary_available(
            Some(t(100)),
            Some(std::path::Path::new("/x/current/jcode")),
            candidates,
        ));
    }

    #[test]
    fn suppresses_update_when_current_mtime_unavailable() {
        // Regression for issue #277: an unreadable current mtime previously fell
        // through to a path-difference heuristic that could loop forever.
        let candidates = vec![(PathBuf::from("/x/stable/jcode"), Some(t(200)))];
        assert!(!newer_binary_available(
            None,
            Some(std::path::Path::new("/x/current/jcode")),
            candidates,
        ));
    }

    #[test]
    fn suppresses_update_when_candidate_mtime_unavailable() {
        // The dangerous case from issue #277: candidate path differs but its
        // mtime cannot be read. Must NOT report an update.
        let candidates = vec![(PathBuf::from("/x/stable/jcode"), None)];
        assert!(!newer_binary_available(
            Some(t(100)),
            Some(std::path::Path::new("/x/current/jcode")),
            candidates,
        ));
    }

    #[test]
    fn reports_update_if_any_candidate_is_newer() {
        let candidates = vec![
            (PathBuf::from("/x/stable/jcode"), None),
            (PathBuf::from("/x/shared/jcode"), Some(t(300))),
        ];
        assert!(newer_binary_available(
            Some(t(100)),
            Some(std::path::Path::new("/x/current/jcode")),
            candidates,
        ));
    }
}
