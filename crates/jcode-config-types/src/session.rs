//! Idle session memory policy shared by the daemon and terminal clients.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    /// Release reloadable session memory after this many seconds without work
    /// or interaction. Zero disables hibernation. Default: 30 minutes.
    pub hibernation_idle_secs: u64,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            hibernation_idle_secs: 30 * 60,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_setting_defaults_to_thirty_minutes() {
        let config: SessionConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.hibernation_idle_secs, 1800);
    }

    #[test]
    fn zero_disables_hibernation_and_roundtrips() {
        let config = SessionConfig {
            hibernation_idle_secs: 0,
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: SessionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.hibernation_idle_secs, 0);
    }
}
