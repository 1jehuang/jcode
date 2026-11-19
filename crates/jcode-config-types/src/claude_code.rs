//! `[provider.claude_code]`: Claude via the local Claude Code CLI.
//!
//! This mode runs turns through the official `claude` binary instead of
//! jcode's native Anthropic transport. Each configured instance is one Claude
//! Code login, isolated by its own `CLAUDE_CONFIG_DIR` (`home`). With no
//! instances configured a single implicit `default` instance uses the CLI's
//! default login.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Instance id used when no instances are configured.
pub const CLAUDE_CODE_DEFAULT_INSTANCE_ID: &str = "default";
/// Default binary name, resolved on `PATH`.
pub const CLAUDE_CODE_DEFAULT_BINARY: &str = "claude";
/// Env var that overrides `[provider.claude_code].binary`.
pub const CLAUDE_CODE_BIN_ENV: &str = "JCODE_CLAUDE_CODE_BIN";

/// Configuration for the Claude Code CLI provider mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeCodeConfig {
    /// Path to the `claude` binary, or a name resolved on `PATH`.
    pub binary: String,
    /// Claude Code permission mode: bypassPermissions (default, like the rest
    /// of jcode) | default | acceptEdits | plan | auto | dontAsk.
    pub permission_mode: String,
    /// Value passed to `--setting-sources`.
    pub setting_sources: Vec<String>,
    /// Offer jcode-only tools to Claude Code through an in-process MCP server.
    pub expose_jcode_tools: bool,
    /// Instance used when a session has no explicit pin.
    pub default_instance: String,
    /// Claude Code logins. Empty means one implicit `default` instance.
    pub instances: Vec<ClaudeCodeInstanceConfig>,
    /// Saved jcode Claude accounts that use jcode's native API instead of the
    /// CLI. Every other saved account runs through the CLI by default.
    pub native_accounts: Vec<String>,
    /// Saved jcode Claude accounts that run through the CLI, each as an extra
    /// instance using that account's jcode-managed token. Derived at runtime
    /// from the saved accounts minus `native_accounts`; never persisted.
    #[serde(skip)]
    pub jcode_accounts: Vec<String>,
}

impl Default for ClaudeCodeConfig {
    fn default() -> Self {
        Self {
            binary: CLAUDE_CODE_DEFAULT_BINARY.to_string(),
            // jcode runs tools without per-call prompts on every provider.
            permission_mode: "bypassPermissions".to_string(),
            setting_sources: vec!["user".into(), "project".into(), "local".into()],
            expose_jcode_tools: true,
            default_instance: CLAUDE_CODE_DEFAULT_INSTANCE_ID.to_string(),
            instances: Vec::new(),
            native_accounts: Vec::new(),
            jcode_accounts: Vec::new(),
        }
    }
}

/// One Claude Code login.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeCodeInstanceConfig {
    /// Slug, `^[a-zA-Z][a-zA-Z0-9_-]*$`.
    pub id: String,
    /// Label shown in the model picker and `/accounts`.
    pub display_name: Option<String>,
    /// `CLAUDE_CONFIG_DIR` for this instance. Empty or unset = CLI default (`~/.claude`).
    pub home: Option<String>,
    /// Extra environment variables for the child process.
    pub env: BTreeMap<String, String>,
    /// Extra CLI arguments, whitespace separated.
    pub launch_args: Option<String>,
    /// Saved jcode Anthropic account whose OAuth token the CLI uses
    /// (`CLAUDE_CODE_OAUTH_TOKEN`). jcode keeps refreshing it.
    pub jcode_account: Option<String>,
}

impl ClaudeCodeInstanceConfig {
    /// The implicit instance used when none are configured.
    pub fn implicit_default() -> Self {
        Self {
            id: CLAUDE_CODE_DEFAULT_INSTANCE_ID.to_string(),
            ..Self::default()
        }
    }

    /// Display name, falling back to the id.
    pub fn display_label(&self) -> String {
        self.display_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.id.clone())
    }

    /// `CLAUDE_CONFIG_DIR` with `~` expanded. `None` = CLI default home.
    pub fn resolved_home(&self) -> Option<PathBuf> {
        let raw = self.home.as_deref()?.trim();
        if raw.is_empty() {
            return None;
        }
        Some(expand_tilde(raw))
    }
}

/// True when `id` is a valid instance slug.
pub fn is_valid_claude_code_instance_id(id: &str) -> bool {
    let mut chars = id.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    id.len() <= 64 && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Instance id for a saved jcode account label (labels may hold spaces).
fn jcode_account_instance_id(label: &str) -> String {
    let slug: String = label
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .take(64)
        .collect();
    if is_valid_claude_code_instance_id(&slug) {
        slug
    } else {
        format!("a-{}", slug.chars().take(62).collect::<String>())
    }
}

fn expand_tilde(raw: &str) -> PathBuf {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    if raw == "~" {
        if let Some(home) = home() {
            return home;
        }
    } else if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = home()
    {
        return home.join(rest);
    }
    PathBuf::from(raw)
}

impl ClaudeCodeConfig {
    /// Configured instances with invalid or duplicate ids dropped, or the
    /// implicit `default` instance when none remain.
    pub fn effective_instances(&self) -> Vec<ClaudeCodeInstanceConfig> {
        let mut out: Vec<ClaudeCodeInstanceConfig> = Vec::new();
        for instance in &self.instances {
            let id = instance.id.trim();
            if !is_valid_claude_code_instance_id(id) || out.iter().any(|seen| seen.id == id) {
                continue;
            }
            let mut instance = instance.clone();
            instance.id = id.to_string();
            out.push(instance);
        }
        if out.is_empty() {
            out.push(ClaudeCodeInstanceConfig::implicit_default());
        }
        for label in &self.jcode_accounts {
            let label = label.trim();
            let id = jcode_account_instance_id(label);
            if label.is_empty() || out.iter().any(|seen| seen.id == id) {
                continue;
            }
            out.push(ClaudeCodeInstanceConfig {
                id,
                display_name: Some(label.to_string()),
                jcode_account: Some(label.to_string()),
                ..ClaudeCodeInstanceConfig::default()
            });
        }
        out
    }

    /// True when the saved jcode account `label` runs through the CLI.
    pub fn routes_jcode_account(&self, label: &str) -> bool {
        self.jcode_accounts
            .iter()
            .any(|item| item.trim() == label.trim())
    }

    /// True when the saved jcode account `label` was toggled to native API.
    pub fn uses_native(&self, label: &str) -> bool {
        self.native_accounts
            .iter()
            .any(|item| item.trim() == label.trim())
    }

    /// Default instance id, guaranteed to name an effective instance.
    pub fn resolved_default_instance(&self) -> String {
        let instances = self.effective_instances();
        let wanted = self.default_instance.trim();
        instances
            .iter()
            .find(|instance| instance.id == wanted)
            .unwrap_or(&instances[0])
            .id
            .clone()
    }

    /// Look up an effective instance by id.
    pub fn instance(&self, id: &str) -> Option<ClaudeCodeInstanceConfig> {
        self.effective_instances()
            .into_iter()
            .find(|instance| instance.id == id.trim())
    }

    /// Binary to run: `JCODE_CLAUDE_CODE_BIN` wins over `binary`.
    pub fn resolved_binary(&self) -> String {
        std::env::var(CLAUDE_CODE_BIN_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                let configured = self.binary.trim();
                if configured.is_empty() {
                    CLAUDE_CODE_DEFAULT_BINARY.to_string()
                } else {
                    configured.to_string()
                }
            })
    }

    /// True when `id` is the default instance (routes use the bare
    /// `claude-code` api method for it).
    pub fn is_default_instance(&self, id: &str) -> bool {
        self.resolved_default_instance() == id.trim()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Wrapper {
        claude_code: ClaudeCodeConfig,
    }

    #[test]
    fn defaults_have_implicit_instance() {
        let config = ClaudeCodeConfig::default();
        let instances = config.effective_instances();
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].id, "default");
        assert_eq!(instances[0].resolved_home(), None);
        assert_eq!(config.resolved_default_instance(), "default");
        assert_eq!(config.binary, "claude");
        assert!(config.expose_jcode_tools);
        assert_eq!(config.setting_sources, ["user", "project", "local"]);
        // Claude Code mode runs tools without prompts, like the rest of jcode.
        assert_eq!(config.permission_mode, "bypassPermissions");
        let parsed: Wrapper = toml_like(r#"{"claude_code":{"binary":"claude"}}"#);
        assert_eq!(parsed.claude_code.permission_mode, "bypassPermissions");
    }

    #[test]
    fn parses_instances_and_falls_back_to_first() {
        let parsed: Wrapper = toml_like(
            r#"{"claude_code":{"default_instance":"missing","instances":[
                {"id":"work","display_name":"Claude Work"},
                {"id":"personal","home":"~/.claude_personal","env":{"A":"b"},"launch_args":"--x"},
                {"id":"9bad"},
                {"id":"work"}
            ]}}"#,
        );
        let config = parsed.claude_code;
        let instances = config.effective_instances();
        assert_eq!(
            instances.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            ["work", "personal"]
        );
        assert_eq!(config.resolved_default_instance(), "work");
        assert_eq!(instances[0].display_label(), "Claude Work");
        assert_eq!(instances[1].display_label(), "personal");
        assert_eq!(instances[1].env.get("A").map(String::as_str), Some("b"));
        let home = instances[1].resolved_home().expect("home");
        assert!(home.ends_with(".claude_personal"));
        assert!(!home.to_string_lossy().starts_with('~') || std::env::var_os("HOME").is_none());
    }

    #[test]
    fn jcode_accounts_add_instances_and_keep_the_default() {
        let config = ClaudeCodeConfig {
            jcode_accounts: vec!["claude-fox".into(), "My Work".into(), "claude-fox".into()],
            ..ClaudeCodeConfig::default()
        };
        let instances = config.effective_instances();
        assert_eq!(
            instances.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            ["default", "claude-fox", "My-Work"]
        );
        assert_eq!(config.resolved_default_instance(), "default");
        assert_eq!(instances[2].jcode_account.as_deref(), Some("My Work"));
        assert_eq!(instances[2].display_label(), "My Work");
        assert!(config.routes_jcode_account("My Work"));
        assert!(!config.routes_jcode_account("claude-otter"));
        assert_eq!(jcode_account_instance_id("1st"), "a-1st");
        let parsed: Wrapper = toml_like(
            r#"{"claude_code":{"native_accounts":["claude-otter"],"jcode_accounts":["x"]}}"#,
        );
        assert!(parsed.claude_code.uses_native("claude-otter"));
        assert!(
            parsed.claude_code.jcode_accounts.is_empty(),
            "derived, never read"
        );
    }

    #[test]
    fn instance_id_validation() {
        assert!(is_valid_claude_code_instance_id("default"));
        assert!(is_valid_claude_code_instance_id("a_b-9"));
        assert!(!is_valid_claude_code_instance_id(""));
        assert!(!is_valid_claude_code_instance_id("9a"));
        assert!(!is_valid_claude_code_instance_id("a:b"));
    }

    fn toml_like(json: &str) -> Wrapper {
        serde_json::from_str(json).expect("parse")
    }
}
