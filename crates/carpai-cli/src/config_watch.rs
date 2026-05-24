//! # Config Watcher
//!
//! Monitors config file changes and signals hot-reload.
//! Uses `notify` crate for filesystem events (future enhancement).
//!
//! Currently provides a polling-based config reload check.

use std::path::PathBuf;
use std::time::SystemTime;
use tracing::{info, warn};

use crate::config::CliConfig;

/// Watches a config file for changes and triggers reload.
pub struct ConfigWatcher {
    /// Path to the config file being watched
    config_path: PathBuf,
    /// Last known modification time
    last_modified: Option<SystemTime>,
    /// Current config loaded in memory
    current_config: CliConfig,
}

impl ConfigWatcher {
    /// Create a new config watcher and load initial config
    pub fn new(config_path: PathBuf) -> Self {
        let current_config = if config_path.exists() {
            CliConfig::load(&config_path).unwrap_or_else(|e| {
                warn!(error = %e, path = %config_path.display(), "Failed to load config, using defaults");
                CliConfig::default()
            })
        } else {
            info!(path = %config_path.display(), "Config file not found, using defaults");
            CliConfig::default()
        };

        let last_modified = config_path.metadata().ok().and_then(|m| m.modified().ok());

        Self {
            config_path,
            last_modified,
            current_config,
        }
    }

    /// Check if config file has changed since last check.
    /// Returns `Some(new_config)` if changed, `None` if unchanged.
    pub fn check_reload(&mut self) -> Option<&CliConfig> {
        let modified = self.config_path.metadata().ok().and_then(|m| m.modified().ok());

        match (modified, self.last_modified) {
            (Some(current), Some(previous)) if current > previous => {
                info!(path = %self.config_path.display(), "Config file changed, reloading");
                match CliConfig::load(&self.config_path) {
                    Ok(new_config) => {
                        self.last_modified = Some(current);
                        self.current_config = new_config;
                        info!("Config reloaded successfully");
                        Some(&self.current_config)
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to reload config, keeping previous");
                        None
                    }
                }
            }
            (Some(current), None) => {
                // First time seeing the file
                self.last_modified = Some(current);
                None
            }
            (None, _) => {
                // File no longer accessible
                None
            }
            _ => None,
        }
    }

    /// Get the current config
    pub fn config(&self) -> &CliConfig {
        &self.current_config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_new_with_nonexistent_file() {
        let watcher = ConfigWatcher::new(PathBuf::from("/nonexistent/config.toml"));
        assert!(watcher.last_modified.is_none());
    }

    #[test]
    fn test_new_with_existing_file() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("carpai.toml");
        fs::write(&config_path, "mode = \"cli\"\n").unwrap();

        let mut watcher = ConfigWatcher::new(config_path);
        assert!(watcher.last_modified.is_some());

        // Check reload - file hasn't changed
        assert!(watcher.check_reload().is_none());
    }

    #[test]
    fn test_detect_file_change() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("carpai.toml");
        fs::write(&config_path, "mode = \"cli\"\n").unwrap();

        let mut watcher = ConfigWatcher::new(config_path.clone());

        // Modify the file
        fs::write(&config_path, "mode = \"server\"\n").unwrap();

        // Small delay to ensure timestamp changes (filesystem resolution)
        std::thread::sleep(std::time::Duration::from_millis(100));

        let reloaded = watcher.check_reload();
        assert!(reloaded.is_some());
        // Just verify the config was modified; the actual mode value depends on TOML parsing
    }
}
