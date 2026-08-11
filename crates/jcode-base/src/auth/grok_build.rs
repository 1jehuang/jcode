//! Local Grok CLI discovery for the delegated Grok Build provider.

use std::path::PathBuf;

pub const CLI_PATH_ENV: &str = "JCODE_GROK_CLI_PATH";

pub fn cli_path() -> PathBuf {
    std::env::var_os(CLI_PATH_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("grok"))
}

pub fn cli_available() -> bool {
    super::command_exists(cli_path().to_string_lossy().as_ref())
}
