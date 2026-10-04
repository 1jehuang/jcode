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
    crate::auth::command_exists(&binary())
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
}
