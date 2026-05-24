//! CliConfig — Layer 2b configuration (CLI-specific settings)
//!
//! Extends CoreConfig with TUI, theme, keybinds, and remote mode settings.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use carpai_core::config::CoreConfig;

// ========================================================================
// Theme Configuration
// ========================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfig {
    #[serde(default = "default_syntax_theme")]
    pub syntax_theme: String,
    #[serde(default = "default_ui_color")]
    pub ui_color: String,
    #[serde(default)]
    pub enable_bold: bool,
}

fn default_syntax_theme() -> String { "base16-dark".into() }
fn default_ui_color() -> String { "blue".into() }

impl Default for ThemeConfig {
    fn default() -> Self {
        Self { syntax_theme: default_syntax_theme(), ui_color: default_ui_color(), enable_bold: true }
    }
}

// ========================================================================
// Keybind Configuration
// ========================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeybindConfig {
    #[serde(default = "default_send_key")]
    pub send_message: String,
    #[serde(default = "default_interrupt_key")]
    pub interrupt: String,
    #[serde(default = "default_help_key")]
    pub toggle_help: String,
    #[serde(default = "default_file_tree_key")]
    pub toggle_file_tree: String,
}

fn default_send_key() -> String { "Enter".into() }
fn default_interrupt_key() -> String { "Escape".into() }
fn default_help_key() -> String { "?".into() }
fn default_file_tree_key() -> String { "Ctrl-f".into() }

impl Default for KeybindConfig {
    fn default() -> Self {
        Self {
            send_message: default_send_key(),
            interrupt: default_interrupt_key(),
            toggle_help: default_help_key(),
            toggle_file_tree: default_file_tree_key(),
        }
    }
}

// ========================================================================
// Clipboard Configuration
// ========================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardConfig {
    #[serde(default)]
    pub auto_copy_response: bool,
    pub external_editor: Option<String>,
}

impl Default for ClipboardConfig {
    fn default() -> Self { Self { auto_copy_response: false, external_editor: None } }
}

// ========================================================================
// Startup Configuration
// ========================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartupConfig {
    #[serde(default)]
    pub show_banner: bool,
    #[serde(default = "default_startup_timeout")]
    pub model_load_timeout_secs: u64,
}

fn default_startup_timeout() -> u64 { 30 }

impl Default for StartupConfig {
    fn default() -> Self { Self { show_banner: true, model_load_timeout_secs: default_startup_timeout() } }
}

// ========================================================================
// CliConfig — Full CLI Configuration
// ========================================================================

/// CLI-specific configuration extending CoreConfig
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    #[serde(flatten)]
    pub core: CoreConfig,

    // === UI ===
    #[serde(default)]
    pub theme: ThemeConfig,
    #[serde(default)]
    pub keybinds: KeybindConfig,

    // === Editor Integration ===
    #[serde(default)]
    pub clipboard: ClipboardConfig,

    // === Startup ===
    #[serde(default)]
    pub startup: StartupConfig,

    // === Remote Mode ===
    pub remote_server_url: Option<String>,
    #[serde(default = "default_remote_timeout")]
    pub remote_timeout_secs: u64,
}

fn default_remote_timeout() -> u64 { 30 }

impl CliConfig {
    /// Load from a TOML file with environment variable overrides
    pub fn load(path: &PathBuf) -> Result<Self, ConfigError> {
        let mut config = Self::default();
        if path.exists() {
            let content = std::fs::read_to_string(path).map_err(ConfigError::Io)?;
            config = toml::from_str(&content).map_err(ConfigError::Parse)?;
        }
        // Environment variable overrides
        if let Ok(v) = std::env::var("CARPAI_REMOTE_URL") { config.remote_server_url = Some(v); }
        Ok(config)
    }

    /// Create a sensible default for local development
    pub fn cli_default(working_dir: PathBuf) -> Self {
        let mut core = CoreConfig::default();
        core.base.working_dir = working_dir;
        core.base.mode = carpai_internal::AppMode::Cli;
        Self {
            core,
            theme: ThemeConfig::default(),
            keybinds: KeybindConfig::default(),
            clipboard: ClipboardConfig::default(),
            startup: StartupConfig::default(),
            remote_server_url: None,
            remote_timeout_secs: default_remote_timeout(),
        }
    }
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            core: CoreConfig::default(),
            theme: ThemeConfig::default(),
            keybinds: KeybindConfig::default(),
            clipboard: ClipboardConfig::default(),
            startup: StartupConfig::default(),
            remote_server_url: None,
            remote_timeout_secs: default_remote_timeout(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(#[from] toml::de::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_defaults() {
        let theme = ThemeConfig::default();
        assert_eq!(theme.syntax_theme, "base16-dark");
        assert_eq!(theme.ui_color, "blue");
        assert!(theme.enable_bold);
    }

    #[test]
    fn test_keybind_defaults() {
        let kb = KeybindConfig::default();
        assert_eq!(kb.send_message, "Enter");
        assert_eq!(kb.interrupt, "Escape");
        assert_eq!(kb.toggle_help, "?");
        assert_eq!(kb.toggle_file_tree, "Ctrl-f");
    }

    #[test]
    fn test_clipboard_defaults() {
        let cb = ClipboardConfig::default();
        assert!(!cb.auto_copy_response);
        assert!(cb.external_editor.is_none());
    }

    #[test]
    fn test_startup_defaults() {
        let su = StartupConfig::default();
        assert!(su.show_banner);
        assert_eq!(su.model_load_timeout_secs, 30);
    }

    #[test]
    fn test_cli_config_default() {
        let config = CliConfig::default();
        assert!(config.remote_server_url.is_none());
        assert_eq!(config.remote_timeout_secs, 30);
    }

    #[test]
    fn test_config_error_display() {
        let io_err = ConfigError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"));
        assert!(io_err.to_string().contains("IO error"));

        let parse_err = ConfigError::Parse(toml::de::Error::custom("bad toml"));
        assert!(parse_err.to_string().contains("Parse error"));
    }
}
