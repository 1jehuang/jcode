//! Claude Code CLI mode helpers.
//!
//! In this mode the official `claude` binary owns login, token storage and
//! refresh. jcode never reads or imports those credentials. An "account" is a
//! configured instance (`[provider.claude_code].instances`), isolated by its
//! own `CLAUDE_CONFIG_DIR`, or a saved jcode Claude account (the default for
//! every saved account) whose jcode-managed token is handed to the CLI.
//! Session pins use the instance id as the label.

use crate::config::{ClaudeCodeConfig, ClaudeCodeInstanceConfig};
use anyhow::{Result, bail};
use jcode_provider_core::AccountPin;

/// Current `[provider.claude_code]` config, with every saved Claude account
/// not toggled to native API added as a CLI instance (when the CLI exists).
pub fn settings() -> ClaudeCodeConfig {
    let mut settings = crate::config::config().provider.claude_code.clone();
    if cli_default() && command_available(&settings.resolved_binary()) {
        settings.jcode_accounts = crate::auth::claude::list_accounts()
            .unwrap_or_default()
            .into_iter()
            .map(|account| account.label)
            .filter(|label| !settings.uses_native(label))
            .collect();
    }
    settings
}

/// jcode-base's own unit tests cover native Claude routing and must not depend
/// on whether `claude` is on the test host's PATH; they opt in explicitly.
#[cfg(test)]
pub(crate) static TEST_CLI_DEFAULT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn cli_default() -> bool {
    #[cfg(test)]
    return TEST_CLI_DEFAULT.load(std::sync::atomic::Ordering::SeqCst);
    #[cfg(not(test))]
    true
}

/// Binary that will be spawned (`JCODE_CLAUDE_CODE_BIN` wins).
pub fn binary() -> String {
    crate::config::config()
        .provider
        .claude_code
        .resolved_binary()
}

/// CLI instance id serving saved Claude account `label`, `None` when the
/// account uses jcode's native API (or the CLI is not installed).
pub fn instance_for_account(label: &str) -> Option<String> {
    settings()
        .effective_instances()
        .into_iter()
        .find(|instance| instance.jcode_account.as_deref() == Some(label.trim()))
        .map(|instance| instance.id)
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

/// Run saved Claude account `label` through the Claude Code CLI (`cli`) or
/// jcode's native API. Returns a confirmation for the user.
pub fn set_account_transport(label: &str, cli: bool) -> Result<String> {
    let label = label.trim();
    if !crate::auth::claude::list_accounts()?
        .iter()
        .any(|account| account.label == label)
    {
        bail!("No saved Claude account `{label}`.");
    }
    if cli && !binary_available() {
        bail!(
            "Claude Code CLI `{}` not found. Install it, or set [provider.claude_code].binary.",
            binary()
        );
    }
    crate::config::Config::set_claude_account_native(label, !cli, None)?;
    Ok(if cli {
        format!(
            "Claude account `{label}` now runs through the Claude Code CLI. Its Claude models are listed as Claude Code routes in /model."
        )
    } else {
        format!("Claude account `{label}` now uses jcode's native API.")
    })
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
    if let Some(label) = instance.jcode_account.as_deref() {
        // Uses a saved jcode account, not a Claude Code login.
        let account = crate::auth::claude::list_accounts()
            .ok()?
            .into_iter()
            .find(|account| account.label == label)?;
        return Some(InstanceIdentity {
            email: account.email,
            plan: account.subscription_type,
            organization: None,
        });
    }
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
    if let Some(label) = &instance.jcode_account {
        return format!("/accounts claude login (re-login account `{label}`)");
    }
    let binary = binary();
    match instance.resolved_home() {
        Some(home) => format!("CLAUDE_CONFIG_DIR={} {binary} auth login", home.display()),
        None => format!("{binary} auth login"),
    }
}

/// Access token of saved account `label`, refreshed (and saved) when it
/// expires within 5 minutes. Used to hand the token to the Claude Code CLI.
pub async fn fresh_access_token_for_account(label: &str) -> Result<String> {
    let creds = crate::auth::claude::load_credentials_for_account(label)?;
    let now = chrono::Utc::now().timestamp_millis();
    if creds.expires_at > now + 300_000 || creds.refresh_token.is_empty() {
        return Ok(creds.access_token);
    }
    Ok(
        crate::auth::oauth::refresh_claude_tokens_for_account(&creds.refresh_token, label)
            .await?
            .access_token,
    )
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
