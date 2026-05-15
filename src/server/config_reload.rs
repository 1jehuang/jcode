use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

const CONFIG_RELOAD_POLL_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileSignature {
    modified: Option<SystemTime>,
    len: u64,
}

type ConfigSnapshot = BTreeMap<PathBuf, Option<FileSignature>>;

pub(super) fn spawn_config_reload_monitor() {
    tokio::spawn(async move {
        monitor_config_reload().await;
    });
}

async fn monitor_config_reload() {
    let mut previous = config_snapshot();
    let mut interval = tokio::time::interval(CONFIG_RELOAD_POLL_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;
        let current = config_snapshot();
        if current == previous {
            continue;
        }

        crate::logging::info("Config change detected; triggering server reload");
        let request_id =
            crate::server::send_reload_signal(env!("JCODE_GIT_HASH").to_string(), None, false);
        crate::logging::info(&format!(
            "Config reload signal queued with request_id={request_id}"
        ));
        previous = current;

        // A real reload replaces this process. In test/no-exec modes, avoid
        // enqueueing duplicate reloads while filesystem timestamps settle.
        tokio::time::sleep(CONFIG_RELOAD_POLL_INTERVAL).await;
    }
}

fn config_snapshot() -> ConfigSnapshot {
    config_watch_paths()
        .into_iter()
        .map(|path| {
            let signature = std::fs::metadata(&path).ok().map(|metadata| FileSignature {
                modified: metadata.modified().ok(),
                len: metadata.len(),
            });
            (path, signature)
        })
        .collect()
}

fn config_watch_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(path) = crate::config::Config::path() {
        paths.push(path);
    }

    if let Ok(jcode_dir) = crate::storage::jcode_dir() {
        paths.push(jcode_dir.join("mcp.json"));
    }

    if let Ok(config_dir) = crate::storage::app_config_dir()
        && let Ok(entries) = std::fs::read_dir(config_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("env") {
                paths.push(path);
            }
        }
    }

    paths.sort();
    paths.dedup();
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EnvGuard {
        key: &'static str,
        old: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let old = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, old }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = self.old.take() {
                crate::env::set_var(self.key, value);
            } else {
                crate::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn config_watch_paths_include_primary_config_mcp_and_env_files() {
        let _lock = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let _home = EnvGuard::set("JCODE_HOME", temp.path());
        let config_dir = crate::storage::app_config_dir().expect("config dir");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("opencode-go.env"),
            "OPENCODE_GO_API_KEY=test\n",
        )
        .expect("write env");
        std::fs::write(config_dir.join("cache.json"), "{}\n").expect("write cache");

        let paths = config_watch_paths();

        assert!(paths.contains(&temp.path().join("config.toml")));
        assert!(paths.contains(&temp.path().join("mcp.json")));
        assert!(paths.contains(&config_dir.join("opencode-go.env")));
        assert!(!paths.contains(&config_dir.join("cache.json")));
    }

    #[test]
    fn config_snapshot_changes_when_watched_file_changes() {
        let _lock = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let _home = EnvGuard::set("JCODE_HOME", temp.path());
        let config_path = temp.path().join("config.toml");
        std::fs::write(&config_path, "[provider]\n").expect("write config");

        let before = config_snapshot();
        std::fs::write(&config_path, "[provider]\ndefault_model = \"gpt-5.5\"\n")
            .expect("rewrite config");
        let after = config_snapshot();

        assert_ne!(before, after);
    }
}
