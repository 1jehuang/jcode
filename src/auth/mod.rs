pub mod claude;
pub mod codex;
pub mod copilot;
pub mod oauth;

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
    /// Cursor CLI available (via `cursor-agent` binary)
    pub cursor: AuthState,
    /// Copilot API available (GitHub OAuth token found)
    pub copilot: AuthState,
    /// Copilot has API token (from hosts.json/apps.json/GITHUB_TOKEN)
    pub copilot_has_api_token: bool,
    /// Antigravity CLI available
    pub antigravity: AuthState,
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

        // Check external/CLI auth providers (presence of installed CLI tooling)
        status.cursor = if command_available_from_env("JCODE_CURSOR_CLI_PATH", "cursor-agent") {
            AuthState::Available
        } else {
            AuthState::NotConfigured
        };

        status.copilot = if copilot::has_copilot_credentials() {
            status.copilot_has_api_token = true;
            AuthState::Available
        } else {
            AuthState::NotConfigured
        };

        status.antigravity =
            if command_available_from_env("JCODE_ANTIGRAVITY_CLI_PATH", "antigravity") {
                AuthState::Available
            } else {
                AuthState::NotConfigured
            };

        status
    }
}

fn command_available_from_env(env_var: &str, fallback: &str) -> bool {
    if let Ok(cmd) = std::env::var(env_var) {
        let trimmed = cmd.trim();
        if !trimmed.is_empty() && command_exists(trimmed) {
            return true;
        }
    }

    command_exists(fallback)
}

fn command_exists(command: &str) -> bool {
    let command = command.trim();
    if command.is_empty() {
        return false;
    }

    let path = std::path::Path::new(command);
    if path.is_absolute() || contains_path_separator(command) {
        return explicit_command_exists(path);
    }

    let path_var = match std::env::var_os("PATH") {
        Some(path) if !path.is_empty() => path,
        _ => return false,
    };

    for dir in std::env::split_paths(&path_var) {
        for candidate in command_candidates(command) {
            if dir.join(candidate).exists() {
                return true;
            }
        }
    }

    false
}

fn explicit_command_exists(path: &std::path::Path) -> bool {
    if path.exists() {
        return true;
    }

    if has_extension(path) {
        return false;
    }

    #[cfg(windows)]
    {
        let pathext =
            std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
        for ext in pathext
            .split(';')
            .map(str::trim)
            .filter(|ext| !ext.is_empty())
        {
            let candidate = path.with_extension(ext.trim_start_matches('.'));
            if candidate.exists() {
                return true;
            }
        }
    }

    false
}

fn command_candidates(command: &str) -> Vec<std::ffi::OsString> {
    let path = std::path::Path::new(command);
    let file_name = match path.file_name() {
        Some(name) => name.to_os_string(),
        None => return Vec::new(),
    };

    if has_extension(path) {
        return vec![file_name];
    }

    let mut candidates = vec![file_name.clone()];

    #[cfg(windows)]
    {
        let pathext =
            std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
        let exts: Vec<&str> = pathext
            .split(';')
            .map(str::trim)
            .filter(|ext| !ext.is_empty())
            .collect();

        for ext in exts {
            let ext_no_dot = ext.trim_start_matches('.');
            if ext_no_dot.is_empty() {
                continue;
            }
            let mut candidate = path.to_path_buf();
            candidate.set_extension(ext_no_dot);
            if let Some(cand_name) = candidate.file_name() {
                candidates.push(cand_name.to_os_string());
            }
        }
    }

    dedup_preserve_order(candidates)
}

fn contains_path_separator(command: &str) -> bool {
    command.contains('/')
        || command.contains('\\')
        || std::path::Path::new(command).components().count() > 1
}

fn has_extension(path: &std::path::Path) -> bool {
    path.extension().is_some()
}

fn dedup_preserve_order(mut values: Vec<std::ffi::OsString>) -> Vec<std::ffi::OsString> {
    let mut out = Vec::new();
    for value in values.drain(..) {
        if !out.iter().any(|v| v == &value) {
            out.push(value);
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_candidates_adds_extension_on_windows() {
        let _ = std::env::set_var("PATHEXT", ".EXE;.BAT");
        let candidates = command_candidates("testcmd");
        if cfg!(windows) {
            let normalized: Vec<String> = candidates
                .iter()
                .map(|c| c.to_string_lossy().to_ascii_lowercase())
                .collect();
            assert!(normalized.iter().any(|c| c == "testcmd"));
            assert!(normalized.iter().any(|c| c == "testcmd.exe"));
            assert!(normalized.iter().any(|c| c == "testcmd.bat"));
        } else {
            assert_eq!(candidates.len(), 1);
            assert!(candidates.iter().any(|c| c == "testcmd"));
        }
    }
}
