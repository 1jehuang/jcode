//! Claude Code CLI mode helpers.
//!
//! In this mode the official `claude` binary owns login, token storage and
//! refresh. jcode never reads or imports those credentials. An "account" is a
//! configured instance (`[provider.claude_code].instances`), isolated by its
//! own `CLAUDE_CONFIG_DIR`. Session pins use the instance id as the label.

use crate::config::{ClaudeCodeConfig, ClaudeCodeInstanceConfig};
use anyhow::{Result, bail};
use jcode_provider_core::AccountPin;

/// Current `[provider.claude_code]` config.
pub fn settings() -> ClaudeCodeConfig {
    crate::config::config().provider.claude_code.clone()
}

/// Binary that will be spawned (`JCODE_CLAUDE_CODE_BIN` wins).
pub fn binary() -> String {
    settings().resolved_binary()
}

/// True when the configured Claude Code binary exists (on PATH or as a path).
pub fn binary_available() -> bool {
    command_available(&binary())
}

/// True when `command` exists on PATH or as a path.
pub fn command_available(command: &str) -> bool {
    crate::auth::command_exists(command)
}

/// Effective instances (implicit `default` when none are configured).
pub fn instances() -> Vec<ClaudeCodeInstanceConfig> {
    settings().effective_instances()
}

/// Look up an instance by id.
pub fn instance(id: &str) -> Option<ClaudeCodeInstanceConfig> {
    settings().instance(id)
}

/// Default instance id.
pub fn default_instance_id() -> String {
    settings().resolved_default_instance()
}

/// Instance ids in configured order.
pub fn instance_ids() -> Vec<String> {
    instances()
        .into_iter()
        .map(|instance| instance.id)
        .collect()
}

/// Pin for an instance id. Fails when the id is not configured.
pub fn pin_for_label(label: &str) -> Result<AccountPin> {
    let label = label.trim();
    match instance(label) {
        Some(instance) => Ok(AccountPin::new(instance.id, None)),
        None => bail!(
            "No Claude Code instance '{}' configured. Known instances: {}",
            label,
            instance_ids().join(", ")
        ),
    }
}

/// Current id of the instance a pin names, `None` when it was removed.
pub fn resolve_pin(pin: &AccountPin) -> Option<String> {
    instance(&pin.label).map(|instance| instance.id)
}

/// Non-secret identity Claude Code caches for an instance's login.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstanceIdentity {
    pub email: Option<String>,
    /// Organization / plan type as Claude Code reports it (e.g. `claude_max`).
    pub plan: Option<String>,
    pub organization: Option<String>,
}

/// `.claude.json` of an instance: `<home>/.claude.json` for instances with
/// their own `CLAUDE_CONFIG_DIR`, else `~/.claude.json`.
pub fn instance_profile_path(instance: &ClaudeCodeInstanceConfig) -> Option<std::path::PathBuf> {
    match instance.resolved_home() {
        Some(home) => Some(home.join(".claude.json")),
        None => dirs::home_dir().map(|home| home.join(".claude.json")),
    }
}

/// Read the cached account profile (`oauthAccount`) of an instance. Only
/// display fields are read; tokens live elsewhere and are never touched.
pub fn instance_identity(instance: &ClaudeCodeInstanceConfig) -> Option<InstanceIdentity> {
    let raw = std::fs::read_to_string(instance_profile_path(instance)?).ok()?;
    parse_instance_identity(&raw)
}

fn parse_instance_identity(raw: &str) -> Option<InstanceIdentity> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let account = value.get("oauthAccount")?;
    let field = |key: &str| {
        account
            .get(key)
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let identity = InstanceIdentity {
        email: field("emailAddress"),
        plan: field("organizationType").or_else(|| field("billingType")),
        organization: field("organizationName"),
    };
    (identity != InstanceIdentity::default()).then_some(identity)
}

/// Command that logs an instance in, for user-facing hints.
pub fn login_hint(instance: &ClaudeCodeInstanceConfig) -> String {
    let binary = binary();
    match instance.resolved_home() {
        Some(home) => format!("CLAUDE_CONFIG_DIR={} {binary} auth login", home.display()),
        None => format!("{binary} auth login"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_instance_pin_round_trips() {
        let id = default_instance_id();
        let pin = pin_for_label(&id).expect("default instance");
        assert_eq!(resolve_pin(&pin), Some(id));
        assert!(pin_for_label("definitely-not-configured-xyz").is_err());
    }

    #[test]
    fn identity_reads_only_display_fields() {
        let identity = parse_instance_identity(
            r#"{"oauthAccount":{"emailAddress":"a@b.c","organizationType":"claude_max","organizationName":"Org"},"other":1}"#,
        )
        .expect("identity");
        assert_eq!(identity.email.as_deref(), Some("a@b.c"));
        assert_eq!(identity.plan.as_deref(), Some("claude_max"));
        assert_eq!(identity.organization.as_deref(), Some("Org"));
        assert_eq!(parse_instance_identity(r#"{"numStartups":3}"#), None);
        assert_eq!(parse_instance_identity("not json"), None);
    }
}
