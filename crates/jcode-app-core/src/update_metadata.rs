//! On-disk state shared by every jcode process that checks for updates.
//!
//! Update checks are cheap but rate limited, and several jcode processes can run
//! on one machine at once, so the cadence and backoff state live in a single
//! metadata file rather than per-process memory.

use super::UPDATE_CHECK_INTERVAL;
use crate::storage;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateMetadata {
    pub last_check: SystemTime,
    pub installed_version: Option<String>,
    pub installed_from: Option<String>,
    #[serde(default)]
    pub last_release_update_secs: Option<f64>,
    #[serde(default)]
    pub last_source_update_secs: Option<f64>,
    /// When set, automatic update checks are suppressed until this time
    /// because GitHub reported the API rate limit was exhausted. Shared via
    /// the metadata file so every jcode process on the machine backs off, not
    /// just the one that saw the 403.
    #[serde(default)]
    pub rate_limited_until: Option<SystemTime>,
}

impl Default for UpdateMetadata {
    fn default() -> Self {
        Self {
            last_check: SystemTime::UNIX_EPOCH,
            installed_version: None,
            installed_from: None,
            last_release_update_secs: None,
            last_source_update_secs: None,
            rate_limited_until: None,
        }
    }
}

impl UpdateMetadata {
    pub fn load() -> Result<Self> {
        let path = metadata_path()?;
        if path.exists() {
            let content = fs::read_to_string(&path)?;
            Ok(serde_json::from_str(&content)?)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = metadata_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        fs::write(&path, content)?;
        Ok(())
    }

    pub fn should_check(&self) -> bool {
        if self.rate_limited_backoff_remaining().is_some() {
            return false;
        }
        match self.last_check.elapsed() {
            Ok(elapsed) => elapsed > UPDATE_CHECK_INTERVAL,
            Err(_) => true,
        }
    }

    /// Remaining rate-limit backoff, if an earlier check was rate limited and
    /// the backoff window has not elapsed yet.
    pub fn rate_limited_backoff_remaining(&self) -> Option<Duration> {
        let until = self.rate_limited_until?;
        until
            .duration_since(SystemTime::now())
            .ok()
            .filter(|remaining| !remaining.is_zero())
    }
}

pub(super) fn metadata_path() -> Result<PathBuf> {
    Ok(storage::jcode_dir()?.join("update_metadata.json"))
}

pub(super) fn record_release_update_duration(duration: Duration) {
    if let Ok(mut metadata) = UpdateMetadata::load() {
        metadata.last_release_update_secs = Some(duration.as_secs_f64());
        let _ = metadata.save();
    }
}

pub(super) fn record_source_update_duration(duration: Duration) {
    if let Ok(mut metadata) = UpdateMetadata::load() {
        metadata.last_source_update_secs = Some(duration.as_secs_f64());
        let _ = metadata.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respects_rate_limit_backoff() {
        let mut metadata = UpdateMetadata {
            last_check: SystemTime::UNIX_EPOCH,
            ..Default::default()
        };
        assert!(metadata.should_check());
        metadata.rate_limited_until = Some(SystemTime::now() + Duration::from_secs(600));
        assert!(!metadata.should_check());
        assert!(metadata.rate_limited_backoff_remaining().is_some());
        // An elapsed backoff no longer blocks checks.
        metadata.rate_limited_until = Some(SystemTime::now() - Duration::from_secs(1));
        assert!(metadata.rate_limited_backoff_remaining().is_none());
        assert!(metadata.should_check());
    }

    #[test]
    fn enforces_check_interval() {
        let metadata = UpdateMetadata {
            last_check: SystemTime::now(),
            ..Default::default()
        };
        assert!(!metadata.should_check());
    }
}
