pub mod claude;
pub mod codex;
pub mod copilot;
pub mod cursor;
pub mod oauth;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::Instant;

static AUTH_STATUS_CACHE: std::sync::LazyLock<RwLock<Option<(AuthStatus, Instant)>>> =
    std::sync::LazyLock::new(|| RwLock::new(None));

const AUTH_STATUS_CACHE_TTL_SECS: u64 = 30;

/// Per-process cache for command existence lookups.
/// CLI tools don't get installed/uninstalled while jcode is running, so caching
/// indefinitely per process is correct and avoids repeated PATH scans.
static COMMAND_EXISTS_CACHE: std::sync::LazyLock<Mutex<HashMap<String, bool>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

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
    /// Copilot API available (GitHub OAuth token found)
    pub copilot: AuthState,
    /// Copilot has API token (from hosts.json/apps.json/GITHUB_TOKEN)
    pub copilot_has_api_token: bool,
    /// Antigravity CLI available
    pub antigravity: AuthState,
    /// Cursor CLI available (via `cursor-agent` binary or API key)
    pub cursor: AuthState,
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
    /// Check all authentication sources and return their status.
    /// Results are cached for 30 seconds to avoid expensive PATH scanning on every frame.
    pub fn check() -> Self {
        if let Ok(cache) = AUTH_STATUS_CACHE.read() {
            if let Some((ref status, ref when)) = *cache {
                if when.elapsed().as_secs() < AUTH_STATUS_CACHE_TTL_SECS {
                    return status.clone();
                }
            }
        }

        let status = Self::check_uncached();

        if let Ok(mut cache) = AUTH_STATUS_CACHE.write() {
            *cache = Some((status.clone(), Instant::now()));
        }

        status
    }

    /// Returns true if at least one provider has usable credentials.
    pub fn has_any_available(&self) -> bool {
        self.anthropic.state == AuthState::Available
            || self.openai == AuthState::Available
            || self.openrouter == AuthState::Available
            || self.copilot == AuthState::Available
            || self.antigravity == AuthState::Available
            || self.cursor == AuthState::Available
    }

    /// Invalidate the cached auth status so the next `check()` does a fresh probe.
    pub fn invalidate_cache() {
        if let Ok(mut cache) = AUTH_STATUS_CACHE.write() {
            *cache = None;
        }
    }

    fn check_uncached() -> Self {
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

        status.cursor = if cursor::has_cursor_api_key()
            || cursor::has_cursor_agent_cli()
            || cursor::has_cursor_vscdb_token()
        {
            AuthState::Available
        } else {
            AuthState::NotConfigured
        };

        status
    }
}

pub(crate) fn command_available_from_env(env_var: &str, fallback: &str) -> bool {
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

    // Absolute/relative path: direct stat, no caching needed
    let path = std::path::Path::new(command);
    if path.is_absolute() || contains_path_separator(command) {
        return explicit_command_exists(path);
    }

    // Check per-process cache first (O(1) on repeated calls)
    if let Ok(cache) = COMMAND_EXISTS_CACHE.lock() {
        if let Some(&cached) = cache.get(command) {
            return cached;
        }
    }

    let path_var = match std::env::var_os("PATH") {
        Some(p) if !p.is_empty() => p,
        _ => {
            cache_command_result(command, false);
            return false;
        }
    };

    let wsl2 = is_wsl2();
    let found = std::env::split_paths(&path_var)
        // On WSL2 skip Windows DrvFs mounts (/mnt/c, /mnt/d, …) — they are
        // accessed via the slow 9P filesystem and CLI tools are never there.
        .filter(|dir| !(wsl2 && is_wsl2_windows_path(dir)))
        .flat_map(|dir| {
            command_candidates(command)
                .into_iter()
                .map(move |c| dir.join(c))
        })
        .any(|p| p.exists());

    cache_command_result(command, found);
    found
}

fn cache_command_result(command: &str, exists: bool) {
    if let Ok(mut cache) = COMMAND_EXISTS_CACHE.lock() {
        cache.insert(command.to_string(), exists);
    }
}

/// Detect WSL2: reads `/proc/version` once and caches the result for the
/// process lifetime.  Returns false on any platform without that file.
fn is_wsl2() -> bool {
    static IS_WSL2: OnceLock<bool> = OnceLock::new();
    *IS_WSL2.get_or_init(|| {
        std::fs::read_to_string("/proc/version")
            .map(|s| s.to_ascii_lowercase().contains("microsoft"))
            .unwrap_or(false)
    })
}

/// Returns true for paths like `/mnt/c`, `/mnt/d`, … that are Windows drive
/// mounts under WSL2 (DrvFs via 9P).
fn is_wsl2_windows_path(dir: &std::path::Path) -> bool {
    use std::path::Component;
    let mut it = dir.components();
    if !matches!(it.next(), Some(Component::RootDir)) {
        return false;
    }
    if !matches!(it.next(), Some(Component::Normal(s)) if s == "mnt") {
        return false;
    }
    if let Some(Component::Normal(drive)) = it.next() {
        let s = drive.to_string_lossy();
        return s.len() == 1 && s.chars().next().map_or(false, |c| c.is_ascii_alphabetic());
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

    #[test]
    fn auth_state_default_is_not_configured() {
        let state = AuthState::default();
        assert_eq!(state, AuthState::NotConfigured);
    }

    #[test]
    fn auth_status_default_all_not_configured() {
        let status = AuthStatus::default();
        assert_eq!(status.anthropic.state, AuthState::NotConfigured);
        assert_eq!(status.openrouter, AuthState::NotConfigured);
        assert_eq!(status.openai, AuthState::NotConfigured);
        assert_eq!(status.copilot, AuthState::NotConfigured);
        assert_eq!(status.cursor, AuthState::NotConfigured);
        assert_eq!(status.antigravity, AuthState::NotConfigured);
        assert!(!status.openai_has_oauth);
        assert!(!status.openai_has_api_key);
        assert!(!status.copilot_has_api_token);
        assert!(!status.anthropic.has_oauth);
        assert!(!status.anthropic.has_api_key);
    }

    #[test]
    fn provider_auth_default() {
        let auth = ProviderAuth::default();
        assert_eq!(auth.state, AuthState::NotConfigured);
        assert!(!auth.has_oauth);
        assert!(!auth.has_api_key);
    }

    #[test]
    fn command_exists_for_known_binary() {
        if cfg!(windows) {
            assert!(command_exists("cmd") || command_exists("cmd.exe"));
        } else {
            assert!(command_exists("ls"));
        }
    }

    #[test]
    fn command_exists_empty_string() {
        assert!(!command_exists(""));
        assert!(!command_exists("   "));
    }

    #[test]
    fn command_exists_nonexistent() {
        assert!(!command_exists("surely_this_binary_does_not_exist_xyz"));
    }

    #[test]
    fn command_exists_absolute_path() {
        if cfg!(windows) {
            assert!(command_exists(r"C:\Windows\System32\cmd.exe"));
        } else {
            assert!(command_exists("/bin/ls") || command_exists("/usr/bin/ls"));
        }
    }

    #[test]
    fn command_exists_absolute_nonexistent() {
        assert!(!command_exists("/nonexistent/path/to/binary"));
    }

    #[test]
    fn contains_path_separator_detection() {
        assert!(contains_path_separator("/usr/bin/test"));
        assert!(contains_path_separator("./test"));
        assert!(!contains_path_separator("test"));
    }

    #[test]
    fn has_extension_detection() {
        assert!(has_extension(std::path::Path::new("test.exe")));
        assert!(!has_extension(std::path::Path::new("test")));
        assert!(has_extension(std::path::Path::new("test.sh")));
    }

    #[test]
    fn dedup_preserves_order() {
        let input = vec![
            std::ffi::OsString::from("a"),
            std::ffi::OsString::from("b"),
            std::ffi::OsString::from("a"),
            std::ffi::OsString::from("c"),
        ];
        let result = dedup_preserve_order(input);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], "a");
        assert_eq!(result[1], "b");
        assert_eq!(result[2], "c");
    }

    #[test]
    fn auth_state_equality() {
        assert_eq!(AuthState::Available, AuthState::Available);
        assert_eq!(AuthState::Expired, AuthState::Expired);
        assert_eq!(AuthState::NotConfigured, AuthState::NotConfigured);
        assert_ne!(AuthState::Available, AuthState::Expired);
        assert_ne!(AuthState::Available, AuthState::NotConfigured);
    }

    #[test]
    fn is_wsl2_windows_path_matches_drive_mounts() {
        assert!(is_wsl2_windows_path(std::path::Path::new("/mnt/c")));
        assert!(is_wsl2_windows_path(std::path::Path::new("/mnt/d")));
        assert!(is_wsl2_windows_path(std::path::Path::new("/mnt/z")));
        assert!(is_wsl2_windows_path(std::path::Path::new(
            "/mnt/c/Windows/System32"
        )));
    }

    #[test]
    fn is_wsl2_windows_path_rejects_non_drives() {
        // /mnt/wsl is a WSL-internal mount, not a Windows drive
        assert!(!is_wsl2_windows_path(std::path::Path::new("/mnt/wsl")));
        // /usr/bin is a plain Linux directory
        assert!(!is_wsl2_windows_path(std::path::Path::new("/usr/bin")));
        // /mnt alone is not a drive
        assert!(!is_wsl2_windows_path(std::path::Path::new("/mnt")));
        // empty
        assert!(!is_wsl2_windows_path(std::path::Path::new("")));
    }

    #[test]
    fn command_exists_cached_on_second_call() {
        // Clear cache first to isolate this test
        if let Ok(mut cache) = COMMAND_EXISTS_CACHE.lock() {
            cache.remove("surely_this_binary_does_not_exist_xyz_cache_test");
        }
        // First call populates the cache
        let result1 = command_exists("surely_this_binary_does_not_exist_xyz_cache_test");
        assert!(!result1);
        // Second call must return same result (from cache)
        let result2 = command_exists("surely_this_binary_does_not_exist_xyz_cache_test");
        assert_eq!(result1, result2);
    }

    #[test]
    fn auth_status_check_returns_valid_struct() {
        let status = AuthStatus::check();
        // Just verify it runs without panicking and has coherent state
        match status.anthropic.state {
            AuthState::Available | AuthState::Expired | AuthState::NotConfigured => {}
        }
        match status.openai {
            AuthState::Available | AuthState::Expired | AuthState::NotConfigured => {}
        }
        // If copilot has api token, state should be Available
        if status.copilot_has_api_token {
            assert_eq!(status.copilot, AuthState::Available);
        }
    }
}
