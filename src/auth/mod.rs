pub mod claude;
pub mod codex;
pub mod oauth;
use serde_json::Value as JsonValue;
use serde_yaml::Value as YamlValue;
use std::path::Path;

/// Authentication status for all supported providers
#[derive(Debug, Clone, Default)]
pub struct AuthStatus {
    /// Anthropic provider (Claude models) - via OAuth or API key
    pub anthropic: ProviderAuth,
    /// OpenRouter provider - via API key
    pub openrouter: AuthState,
    /// OpenAI provider - via OAuth or API key
    pub openai: AuthState,
    /// OpenAI has OAuth credentials
    pub openai_has_oauth: bool,
    /// OpenAI has API key available
    pub openai_has_api_key: bool,
    /// Cursor CLI provider status
    pub cursor: CliProviderAuth,
    /// GitHub Copilot CLI provider status
    pub copilot: CliProviderAuth,
    /// Antigravity CLI provider status
    pub antigravity: CliProviderAuth,
}

/// Auth state for Anthropic which has multiple auth methods
#[derive(Debug, Clone, Copy, Default)]
pub struct ProviderAuth {
    /// Overall state (best of available methods)
    pub state: AuthState,
    /// Has OAuth credentials
    pub has_oauth: bool,
    /// Has API key
    pub has_api_key: bool,
}

/// Auth state for CLI-backed providers
#[derive(Debug, Clone, Copy, Default)]
pub struct CliProviderAuth {
    /// Overall state (available / partial / not configured)
    pub state: AuthState,
    /// Whether the CLI binary is available
    pub has_cli: bool,
    /// Whether auth/session material appears available
    pub has_auth: bool,
}

/// State of a single auth credential
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AuthState {
    /// Credential is available and valid
    Available,
    /// Partial configuration exists (or OAuth may be expired)
    Expired,
    /// Credential is not configured
    #[default]
    NotConfigured,
}

fn command_exists(command: &str) -> bool {
    let path = Path::new(command);
    if path.is_absolute() || command.contains('/') {
        return path.exists();
    }

    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(command).exists()))
        .unwrap_or(false)
}

fn command_exists_with_env_override(env_var: &str, default_cmd: &str) -> bool {
    if let Ok(value) = std::env::var(env_var) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return command_exists(trimmed);
        }
    }
    command_exists(default_cmd)
}

fn non_empty_env(var: &str) -> bool {
    std::env::var(var)
        .ok()
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

fn read_json(path: &Path) -> Option<JsonValue> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<JsonValue>(&content).ok()
}

fn json_has_non_empty_string(value: &JsonValue, keys: &[&str]) -> bool {
    keys.iter().any(|key| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    })
}

fn cli_auth_state(has_cli: bool, has_auth: bool) -> AuthState {
    match (has_cli, has_auth) {
        (true, true) => AuthState::Available,
        (false, false) => AuthState::NotConfigured,
        _ => AuthState::Expired,
    }
}

fn detect_cursor_auth() -> CliProviderAuth {
    let has_cli = command_exists_with_env_override("JCODE_CURSOR_CLI_PATH", "cursor-agent");

    let env_auth = non_empty_env("CURSOR_API_KEY");
    let file_auth = dirs::home_dir()
        .map(|home| home.join(".config").join("cursor").join("auth.json"))
        .and_then(|path| read_json(&path))
        .map(|json| json_has_non_empty_string(&json, &["accessToken", "refreshToken", "token"]))
        .unwrap_or(false);

    let has_auth = env_auth || file_auth;
    CliProviderAuth {
        state: cli_auth_state(has_cli, has_auth),
        has_cli,
        has_auth,
    }
}

fn gh_hosts_has_token() -> bool {
    if non_empty_env("GH_TOKEN") || non_empty_env("GITHUB_TOKEN") {
        return true;
    }

    let hosts_path = match dirs::home_dir() {
        Some(home) => home.join(".config").join("gh").join("hosts.yml"),
        None => return false,
    };

    let content = match std::fs::read_to_string(hosts_path) {
        Ok(content) => content,
        Err(_) => return false,
    };

    let yaml = match serde_yaml::from_str::<YamlValue>(&content) {
        Ok(yaml) => yaml,
        Err(_) => return false,
    };

    yaml.as_mapping()
        .map(|mapping| {
            mapping.values().any(|host_entry| {
                host_entry
                    .as_mapping()
                    .and_then(|host_map| host_map.get(YamlValue::from("oauth_token")))
                    .and_then(|token| token.as_str())
                    .map(|token| !token.trim().is_empty())
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn detect_copilot_auth() -> CliProviderAuth {
    let has_cli = command_exists_with_env_override("JCODE_COPILOT_CLI_PATH", "copilot")
        || command_exists("gh");
    let has_auth = gh_hosts_has_token();

    CliProviderAuth {
        state: cli_auth_state(has_cli, has_auth),
        has_cli,
        has_auth,
    }
}

fn detect_antigravity_auth() -> CliProviderAuth {
    let has_cli = command_exists_with_env_override("JCODE_ANTIGRAVITY_CLI_PATH", "antigravity");

    let env_auth =
        non_empty_env("JCODE_ANTIGRAVITY_API_KEY") || non_empty_env("ANTIGRAVITY_API_KEY");
    let file_auth = dirs::home_dir()
        .map(|home| {
            [
                home.join(".config").join("antigravity").join("auth.json"),
                home.join(".antigravity").join("auth.json"),
                home.join(".antigravity").join("credentials.json"),
            ]
        })
        .map(|paths| {
            paths.into_iter().any(|path| {
                read_json(&path)
                    .map(|json| {
                        json_has_non_empty_string(
                            &json,
                            &["token", "accessToken", "refreshToken", "apiKey", "api_key"],
                        )
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);

    let has_auth = env_auth || file_auth;
    CliProviderAuth {
        state: cli_auth_state(has_cli, has_auth),
        has_cli,
        has_auth,
    }
}

impl AuthStatus {
    /// Check all authentication sources and return their status
    pub fn check() -> Self {
        let mut status = Self::default();

        // Check Anthropic (OAuth or API key)
        let mut anthropic = ProviderAuth::default();

        // Check OAuth
        match claude::load_credentials() {
            Ok(creds) => {
                let now_ms = chrono::Utc::now().timestamp_millis();
                anthropic.has_oauth = true;
                if creds.expires_at > now_ms {
                    anthropic.state = AuthState::Available;
                } else {
                    anthropic.state = AuthState::Expired;
                }
            }
            Err(_) => {}
        }

        // Check API key (overrides expired OAuth)
        if std::env::var("ANTHROPIC_API_KEY").is_ok() {
            anthropic.has_api_key = true;
            anthropic.state = AuthState::Available;
        }

        status.anthropic = anthropic;

        // Check OpenRouter API key (env var or config file)
        if std::env::var("OPENROUTER_API_KEY").is_ok() {
            status.openrouter = AuthState::Available;
        } else if let Some(config_dir) = dirs::config_dir() {
            let config_path = config_dir.join("jcode").join("openrouter.env");
            if let Ok(content) = std::fs::read_to_string(config_path) {
                for line in content.lines() {
                    if let Some(key) = line.strip_prefix("OPENROUTER_API_KEY=") {
                        let key = key.trim().trim_matches('"').trim_matches('\'');
                        if !key.is_empty() {
                            status.openrouter = AuthState::Available;
                            break;
                        }
                    }
                }
            }
        }

        // Check OpenAI (Codex OAuth or API key)
        match codex::load_credentials() {
            Ok(creds) => {
                // Check if we have OAuth tokens (not just API key fallback)
                if !creds.refresh_token.is_empty() {
                    status.openai_has_oauth = true;
                    // Has OAuth - check expiry if available
                    if let Some(expires_at) = creds.expires_at {
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        if expires_at > now_ms {
                            status.openai = AuthState::Available;
                        } else {
                            status.openai = AuthState::Expired;
                        }
                    } else {
                        // No expiry info, assume available
                        status.openai = AuthState::Available;
                    }
                } else if !creds.access_token.is_empty() {
                    // API key fallback
                    status.openai_has_api_key = true;
                    status.openai = AuthState::Available;
                }
            }
            Err(_) => {}
        }

        // Fall back to env var (or combine with OAuth)
        if std::env::var("OPENAI_API_KEY")
            .ok()
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
        {
            status.openai_has_api_key = true;
            status.openai = AuthState::Available;
        }

        status.cursor = detect_cursor_auth();
        status.copilot = detect_copilot_auth();
        status.antigravity = detect_antigravity_auth();

        status
    }
}
