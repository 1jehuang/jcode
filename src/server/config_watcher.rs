use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CONFIG_RELOAD_WATCH_INTERVAL: Duration = Duration::from_secs(2);
const CONFIG_RELOAD_SETTLE_DELAY: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq, Eq)]
struct WatchedFileState {
    path: PathBuf,
    exists: bool,
    modified: Option<SystemTime>,
    len: Option<u64>,
}

pub(super) fn spawn_config_reload_watcher(server_git_hash: String) {
    if config_reload_watcher_disabled_by_env() {
        crate::logging::info("Server config reload watcher disabled by JCODE_CONFIG_AUTO_RELOAD=0");
        return;
    }

    tokio::spawn(async move {
        run_config_reload_watcher(server_git_hash).await;
    });
}

async fn run_config_reload_watcher(server_git_hash: String) {
    let mut previous = watched_config_state();
    let mut interval = tokio::time::interval(CONFIG_RELOAD_WATCH_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;
        let current = watched_config_state();
        if current == previous {
            continue;
        }

        tokio::time::sleep(CONFIG_RELOAD_SETTLE_DELAY).await;
        let settled = watched_config_state();
        previous = settled.clone();

        // Force the reloadable config cache to observe the changed file before
        // deciding whether process reloads are enabled. This means flipping
        // display.auto_server_reload to false takes effect without restarting.
        if !config_auto_server_reload_enabled() {
            crate::logging::info(
                "Jcode config/instruction files changed; server auto reload is disabled by display.auto_server_reload=false",
            );
            continue;
        }

        let changed_paths = format_watched_paths(&settled);
        crate::logging::info(&format!(
            "Jcode config/instruction files changed ({}); reloading shared server so future sessions use fresh configuration",
            changed_paths
        ));
        let reload_hash = format!(
            "{}:config:{}",
            server_git_hash,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis())
                .unwrap_or_default()
        );
        super::send_reload_signal(reload_hash, None, false);
    }
}

fn config_auto_server_reload_enabled() -> bool {
    if config_reload_watcher_disabled_by_env() {
        return false;
    }
    crate::config::config().display.auto_server_reload
}

fn config_reload_watcher_disabled_by_env() -> bool {
    std::env::var("JCODE_CONFIG_AUTO_RELOAD")
        .ok()
        .map(|value| {
            let value = value.trim();
            value == "0" || value.eq_ignore_ascii_case("false") || value.eq_ignore_ascii_case("off")
        })
        .unwrap_or(false)
}

fn watched_config_state() -> Vec<WatchedFileState> {
    watched_config_paths()
        .into_iter()
        .map(|path| watched_file_state(path.as_path()))
        .collect()
}

fn watched_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(path) = crate::config::Config::path() {
        paths.push(path);
    }

    if let Ok(jcode_dir) = crate::storage::jcode_dir() {
        paths.push(jcode_dir.join("prompt-overlay.md"));
    }

    if let Ok(home_agents) = crate::storage::user_home_path("AGENTS.md") {
        paths.push(home_agents);
    }

    if let Ok(current_dir) = std::env::current_dir() {
        paths.push(current_dir.join("AGENTS.md"));
        paths.push(current_dir.join(".jcode").join("prompt-overlay.md"));
    }

    dedupe_paths(paths)
}

fn watched_file_state(path: &Path) -> WatchedFileState {
    let metadata = std::fs::metadata(path).ok();
    WatchedFileState {
        path: path.to_path_buf(),
        exists: metadata.is_some(),
        modified: metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok()),
        len: metadata.as_ref().map(std::fs::Metadata::len),
    }
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();

    for path in paths {
        let key = path
            .canonicalize()
            .unwrap_or_else(|_| path.clone())
            .to_string_lossy()
            .to_string();
        if seen.insert(key) {
            deduped.push(path);
        }
    }

    deduped
}

fn format_watched_paths(states: &[WatchedFileState]) -> String {
    states
        .iter()
        .filter(|state| state.exists)
        .map(|state| state.path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watched_file_state_changes_when_file_is_created() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");

        let before = watched_file_state(&path);
        std::fs::write(&path, "[display]\nauto_server_reload = true\n").expect("write config");
        let after = watched_file_state(&path);

        assert!(!before.exists);
        assert!(after.exists);
        assert_ne!(before, after);
    }

    #[test]
    fn dedupe_paths_collapses_symlinked_instruction_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("AGENTS.md");
        let link = dir.path().join(".AGENTS.md");
        std::fs::write(&target, "instructions").expect("write target");

        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&target, &link).expect("symlink");

        let deduped = dedupe_paths(vec![target.clone(), link]);
        assert_eq!(deduped, vec![target]);
    }
}
