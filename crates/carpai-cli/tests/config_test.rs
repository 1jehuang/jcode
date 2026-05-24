//! CliConfig 集成测试
//!
//! 验证三层配置加载: 默认值 → 文件 → 环境变量

use std::path::PathBuf;
use std::fs;

use tempfile::tempdir;

/// Helper: create a minimal config TOML file
fn write_test_config(dir: &std::path::Path, content: &str) -> PathBuf {
    let path = dir.join("carpai.toml");
    fs::write(&path, content).expect("Failed to write test config");
    path
}

#[test]
fn test_load_defaults() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("nonexistent.toml");
    let config = carpai_cli::CliConfig::load(&path).unwrap();

    // Default theme
    assert_eq!(config.theme.syntax_theme, "base16-dark");
    assert_eq!(config.keybinds.send_message, "Enter");
    assert!(!config.clipboard.auto_copy_response);
    assert!(config.startup.show_banner);
    assert!(config.remote_server_url.is_none());
    assert_eq!(config.remote_timeout_secs, 30);
}

#[test]
fn test_load_toml_config() {
    let dir = tempdir().unwrap();
    let toml_content = r#"
mode = "cli"
working_dir = "/tmp/test"

[theme]
syntax_theme = "monokai"
ui_color = "green"

[keybinds]
send_message = "Ctrl-Enter"
"#;
    let path = write_test_config(dir.path(), toml_content);
    let config = carpai_cli::CliConfig::load(&path).unwrap();

    assert_eq!(config.theme.syntax_theme, "monokai");
    assert_eq!(config.theme.ui_color, "green");
    assert_eq!(config.keybinds.send_message, "Ctrl-Enter");
    // Check through CoreConfig -> AppConfig
    assert_eq!(config.core.base.working_dir.to_string_lossy(), "/tmp/test");
}

#[test]
fn test_env_var_override() {
    let dir = tempdir().unwrap();
    let path = write_test_config(dir.path(), r#"mode = "cli""#);

    // Set env var
    std::env::set_var("CARPAI_REMOTE_URL", "https://carpai.example.com:8080");

    let config = carpai_cli::CliConfig::load(&path).unwrap();

    assert_eq!(
        config.remote_server_url,
        Some("https://carpai.example.com:8080".into())
    );

    // Cleanup
    std::env::remove_var("CARPAI_REMOTE_URL");
}

#[test]
fn test_cli_default_builder() {
    let working_dir = PathBuf::from("/home/user/projects");
    let config = carpai_cli::CliConfig::cli_default(working_dir.clone());

    assert_eq!(config.core.base.working_dir, working_dir);
    assert_eq!(config.core.base.mode, carpai_internal::AppMode::Cli);
    assert!(config.remote_server_url.is_none());
}

#[test]
fn test_parse_error_returns_io_error() {
    let dir = tempdir().unwrap();
    let path = write_test_config(dir.path(), "invalid toml [[[");
    let result = carpai_cli::CliConfig::load(&path);
    assert!(result.is_err());
    // Should be a Parse error, not panic
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(err_str.contains("Parse error") || err_str.contains("error"));
}

#[test]
fn test_invalid_env_override_uses_default() {
    let dir = tempdir().unwrap();
    let path = write_test_config(dir.path(), r#"mode = "cli""#);

    // Set invalid env var (should not crash)
    std::env::set_var("CARPAI_REMOTE_URL", "");

    let config = carpai_cli::CliConfig::load(&path).unwrap();
    // Empty string env var should result in Some("")
    assert_eq!(config.remote_server_url, Some("".into()));

    std::env::remove_var("CARPAI_REMOTE_URL");
}
