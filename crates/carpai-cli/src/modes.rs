//! # Modes
//!
//! CLI operation modes: local (standalone) and remote (connected to server).

use serde::{Deserialize, Serialize};

/// Operation mode for the CLI client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CliMode {
    /// Local mode: run inference locally via sidecar
    #[default]
    Local,
    /// Remote mode: connect to a carpai-server instance
    Remote,
}

impl std::fmt::Display for CliMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local => write!(f, "local"),
            Self::Remote => write!(f, "remote"),
        }
    }
}

impl std::str::FromStr for CliMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "local" => Ok(Self::Local),
            "remote" => Ok(Self::Remote),
            _ => Err(format!("Unknown mode: {s}. Expected 'local' or 'remote'")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_local() {
        assert_eq!(CliMode::default(), CliMode::Local);
    }

    #[test]
    fn test_display() {
        assert_eq!(CliMode::Local.to_string(), "local");
        assert_eq!(CliMode::Remote.to_string(), "remote");
    }

    #[test]
    fn test_from_str_valid() {
        assert_eq!("local".parse::<CliMode>().unwrap(), CliMode::Local);
        assert_eq!("LOCAL".parse::<CliMode>().unwrap(), CliMode::Local);
        assert_eq!("remote".parse::<CliMode>().unwrap(), CliMode::Remote);
    }

    #[test]
    fn test_from_str_invalid() {
        assert!("unknown".parse::<CliMode>().is_err());
        assert!("".parse::<CliMode>().is_err());
    }

    #[test]
    fn test_serde_roundtrip() {
        let json = serde_json::to_string(&CliMode::Local).unwrap();
        assert_eq!(json, "\"local\"");
        let deserialized: CliMode = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, CliMode::Local);
    }
}
