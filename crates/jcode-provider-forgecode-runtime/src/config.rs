#[derive(Clone)]
pub(crate) struct ForgeCodeCliConfig {
    pub(crate) cli_path: String,
    pub(crate) model: String,
    pub(crate) permission_mode: Option<String>,
}

impl ForgeCodeCliConfig {
    pub(crate) fn from_env() -> Self {
        let cli_path = std::env::var("JCODE_FORGECODE_CLI_PATH")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "forge".to_string());

        let model = std::env::var("JCODE_FORGECODE_CLI_MODEL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| super::DEFAULT_MODEL.to_string());

        let permission_mode = std::env::var("JCODE_FORGECODE_CLI_PERMISSION_MODE")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| Some("bypassPermissions".to_string()));

        Self {
            cli_path,
            model,
            permission_mode,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_env_defaults() {
        unsafe {
            std::env::set_var("JCODE_FORGECODE_CLI_PATH", "");
            std::env::set_var("JCODE_FORGECODE_CLI_MODEL", "");
            std::env::set_var("JCODE_FORGECODE_CLI_PERMISSION_MODE", "");
        }
        let config = ForgeCodeCliConfig::from_env();
        assert_eq!(config.cli_path, "forge");
        assert_eq!(config.model, "default");
        assert_eq!(config.permission_mode.as_deref(), Some("bypassPermissions"));
    }

    #[test]
    fn test_from_env_custom_values() {
        unsafe {
            std::env::set_var("JCODE_FORGECODE_CLI_PATH", "/usr/local/bin/forge");
            std::env::set_var("JCODE_FORGECODE_CLI_MODEL", "claude-sonnet-4");
            std::env::set_var("JCODE_FORGECODE_CLI_PERMISSION_MODE", "strict");
        }
        let config = ForgeCodeCliConfig::from_env();
        assert_eq!(config.cli_path, "/usr/local/bin/forge");
        assert_eq!(config.model, "claude-sonnet-4");
        assert_eq!(config.permission_mode.as_deref(), Some("strict"));
    }

    #[test]
    fn test_from_env_whitespace_uses_default() {
        unsafe {
            std::env::set_var("JCODE_FORGECODE_CLI_PATH", "  ");
            std::env::set_var("JCODE_FORGECODE_CLI_MODEL", "  ");
            std::env::set_var("JCODE_FORGECODE_CLI_PERMISSION_MODE", "  ");
        }
        let config = ForgeCodeCliConfig::from_env();
        assert_eq!(config.cli_path, "forge");
        assert_eq!(config.model, "default");
        assert_eq!(config.permission_mode.as_deref(), Some("bypassPermissions"));
    }
}
