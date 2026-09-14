//! Integration configuration and feature flags.

use std::collections::HashMap;

/// Feature flags controlling which shell integration features are enabled.
#[derive(Debug, Clone)]
pub struct FeatureFlags {
    /// Enable OSC 133 prompt marking.
    pub prompt_marking: bool,
    /// Enable OSC 7 CWD reporting.
    pub cwd_reporting: bool,
    /// Track command execution timing.
    pub command_timing: bool,
    /// Show git branch in prompt.
    pub git_branch: bool,
    /// Show exit code in prompt.
    pub exit_code: bool,
    /// Wrap `ssh` to preserve terminal environment.
    pub ssh_wrapping: bool,
    /// Wrap `sudo` to preserve terminfo.
    pub sudo_wrapping: bool,
    /// Detect password prompts (secure keyboard mode).
    pub secure_keyboard: bool,
}

impl Default for FeatureFlags {
    fn default() -> Self {
        Self {
            prompt_marking: true,
            cwd_reporting: true,
            command_timing: true,
            git_branch: false,
            exit_code: true,
            ssh_wrapping: false,
            sudo_wrapping: false,
            secure_keyboard: false,
        }
    }
}

/// Optional terminal metadata (e.g. from `jcode-terminal-detect`).
#[derive(Debug, Clone, Default)]
pub struct TerminalInfo {
    pub terminal_emulator: Option<String>,
    pub color_depth: Option<u8>,
    pub is_ssh: bool,
}

/// Top-level configuration for shell integration generation.
#[derive(Debug, Clone)]
pub struct IntegrationConfig {
    pub features: FeatureFlags,
    pub terminal_info: Option<TerminalInfo>,
    pub custom_env: HashMap<String, String>,
    pub ssh_wrapping: bool,
    pub sudo_wrapping: bool,
}

impl Default for IntegrationConfig {
    fn default() -> Self {
        Self {
            features: FeatureFlags::default(),
            terminal_info: None,
            custom_env: HashMap::new(),
            ssh_wrapping: false,
            sudo_wrapping: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_flags_all_prompt_features_enabled() {
        let flags = FeatureFlags::default();
        assert!(flags.prompt_marking);
        assert!(flags.cwd_reporting);
        assert!(flags.command_timing);
        assert!(flags.exit_code);
    }

    #[test]
    fn default_config_is_sane() {
        let cfg = IntegrationConfig::default();
        assert!(cfg.features.prompt_marking);
        assert!(!cfg.ssh_wrapping);
        assert!(!cfg.sudo_wrapping);
        assert!(cfg.custom_env.is_empty());
    }
}
