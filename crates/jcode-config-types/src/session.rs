//! Idle session memory policy shared by the daemon and terminal clients.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    /// Release reloadable session memory after this many seconds without work
    /// or interaction. Zero disables hibernation. Default: 30 minutes.
    pub hibernation_idle_secs: u64,
    /// How much an idle terminal client releases when it hibernates.
    pub hibernation_mode: HibernationMode,
}

/// Depth of idle TUI hibernation, from cheapest wake to largest savings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HibernationMode {
    /// Drop rebuildable render caches only. Instant, lossless wake.
    #[default]
    Caches,
    /// Also drop the in-memory transcript and reload it from the server on
    /// wake. Local-only notices and the exact scroll position are not kept.
    Transcript,
    /// Exit the client process entirely, leaving a tiny waiter that resumes
    /// the session on the next key press. Largest savings, ~1s wake.
    Exit,
}

impl HibernationMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "caches" | "cache" => Some(Self::Caches),
            "transcript" => Some(Self::Transcript),
            "exit" | "process" => Some(Self::Exit),
            _ => None,
        }
    }
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            hibernation_idle_secs: 30 * 60,
            hibernation_mode: HibernationMode::Caches,
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
        assert_eq!(config.hibernation_mode, HibernationMode::Caches);
    }

    #[test]
    fn zero_disables_hibernation_and_roundtrips() {
        let config = SessionConfig {
            hibernation_idle_secs: 0,
            hibernation_mode: HibernationMode::Exit,
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"exit\""));
        let restored: SessionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.hibernation_idle_secs, 0);
        assert_eq!(restored.hibernation_mode, HibernationMode::Exit);
    }

    #[test]
    fn mode_parses_aliases() {
        assert_eq!(
            HibernationMode::parse(" Transcript "),
            Some(HibernationMode::Transcript)
        );
        assert_eq!(
            HibernationMode::parse("process"),
            Some(HibernationMode::Exit)
        );
        assert_eq!(HibernationMode::parse("bogus"), None);
    }
}
