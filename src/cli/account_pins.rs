//! `--account <label>`: per-window / per-run account pins.

use anyhow::{Result, bail};

/// Resolve `--account` values to `(provider, label)` pins. The provider is
/// inferred from the label prefix (`claude-fox`, `openai-otter`) or, for an
/// email or custom label, from the stored accounts. One pin per provider.
pub fn resolve_account_pins(values: &[String]) -> Result<Vec<(String, String)>> {
    let mut pins: Vec<(String, String)> = Vec::new();
    for value in values {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let (provider, label) = resolve_one(value)?;
        if pins.iter().any(|(existing, _)| *existing == provider) {
            bail!(
                "Use one --account per provider ({provider} was given twice). A window uses one account per provider."
            );
        }
        pins.push((provider, label));
    }
    Ok(pins)
}

fn resolve_one(value: &str) -> Result<(String, String)> {
    if let Some(kind) = crate::auth::AccountProviderKind::from_label(value) {
        return Ok((kind.key().to_string(), value.to_string()));
    }
    let claude = crate::auth::claude::list_accounts()
        .unwrap_or_default()
        .into_iter()
        .find(|a| a.label == value || a.email.as_deref() == Some(value))
        .map(|a| a.label);
    let openai = crate::auth::codex::list_accounts()
        .unwrap_or_default()
        .into_iter()
        .find(|a| a.label == value || a.email.as_deref() == Some(value))
        .map(|a| a.label);
    match (claude, openai) {
        (Some(label), None) => Ok(("claude".to_string(), label)),
        (None, Some(label)) => Ok(("openai".to_string(), label)),
        (Some(_), Some(_)) => bail!(
            "--account {value} matches both a Claude and an OpenAI account. Use its label (claude-… or openai-…)."
        ),
        (None, None) => bail!(
            "--account {value}: no saved Claude or OpenAI account with that label or email. Run `jcode auth status` to list accounts."
        ),
    }
}

/// Pin a standalone provider (`jcode run`) to the requested accounts. This
/// affects only this process's provider instance, never the stored default.
pub fn apply_to_provider(
    provider: &std::sync::Arc<dyn crate::provider::Provider>,
    pins: &[(String, String)],
) -> Result<()> {
    for (provider_key, label) in pins {
        let Some(kind) = crate::auth::AccountProviderKind::from_key(provider_key) else {
            continue;
        };
        let identity = match kind {
            crate::auth::AccountProviderKind::Claude => {
                crate::auth::claude::list_accounts()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|a| &a.label == label)
                    .and_then(|a| a.email)
            }
            crate::auth::AccountProviderKind::OpenAi => {
                crate::auth::codex::list_accounts()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|a| &a.label == label)
                    .and_then(|a| a.email.or(a.account_id))
            }
        };
        provider.set_account_pin(
            kind,
            Some(crate::auth::AccountPin::new(label.clone(), identity)),
        )?;
        // One-shot processes may also use the process-local override
        // (design section 0). It never writes auth.json, so the stored default
        // and other windows are unaffected.
        match kind {
            crate::auth::AccountProviderKind::Claude => {
                crate::auth::claude::set_active_account_override(Some(label.clone()))
            }
            crate::auth::AccountProviderKind::OpenAi => {
                crate::auth::codex::set_active_account_override(Some(label.clone()))
            }
        }
    }
    Ok(())
}
