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

/// `jcode --account X run --account Y`: the top-level values apply to `run`
/// too. A `run` value wins for the same provider.
pub fn merge_account_values(
    global: &[String],
    subcommand: &[String],
) -> Result<Vec<(String, String)>> {
    let mut pins = resolve_account_pins(subcommand)?;
    for (provider, label) in resolve_account_pins(global)? {
        if !pins.iter().any(|(existing, _)| *existing == provider) {
            pins.push((provider, label));
        }
    }
    Ok(pins)
}

/// Stored (label, email) pairs for one provider family.
fn stored(kind: crate::auth::AccountProviderKind) -> Vec<(String, Option<String>)> {
    match kind {
        crate::auth::AccountProviderKind::Claude => crate::auth::claude::list_accounts()
            .unwrap_or_default()
            .into_iter()
            .map(|a| (a.label, a.email))
            .collect(),
        crate::auth::AccountProviderKind::OpenAi => crate::auth::codex::list_accounts()
            .unwrap_or_default()
            .into_iter()
            .map(|a| (a.label, a.email))
            .collect(),
        // Only explicitly configured Claude Code instances are addressable,
        // so a bare `--account default` keeps its old meaning.
        crate::auth::AccountProviderKind::ClaudeCode => {
            if crate::config::config()
                .provider
                .claude_code
                .instances
                .is_empty()
            {
                Vec::new()
            } else {
                crate::auth::claude_code::instance_ids()
                    .into_iter()
                    .map(|id| (id, None))
                    .collect()
            }
        }
    }
}

fn known_labels_hint() -> String {
    let labels: Vec<String> = crate::auth::AccountProviderKind::ALL
        .into_iter()
        .flat_map(stored)
        .map(|(label, _)| label)
        .collect();
    if labels.is_empty() {
        "No Claude or OpenAI accounts are saved. Log in with `jcode login claude` or `jcode login openai`.".to_string()
    } else {
        format!(
            "Saved accounts: {}. Run `jcode auth status` for details.",
            labels.join(", ")
        )
    }
}

/// A value must name a stored account (by label or email). A known prefix
/// alone (`claude-typo`) is not enough.
fn resolve_one(value: &str) -> Result<(String, String)> {
    let matches: Vec<(crate::auth::AccountProviderKind, String)> =
        crate::auth::AccountProviderKind::ALL
            .into_iter()
            .filter_map(|kind| {
                stored(kind)
                    .into_iter()
                    .find(|(label, email)| {
                        label == value
                            || email
                                .as_deref()
                                .is_some_and(|email| email.eq_ignore_ascii_case(value))
                    })
                    .map(|(label, _)| (kind, label))
            })
            .collect();
    match matches.as_slice() {
        [(kind, label)] => Ok((kind.key().to_string(), label.clone())),
        [] => bail!(
            "--account {value}: no saved Claude or OpenAI account with that label or email. {}",
            known_labels_hint()
        ),
        _ => bail!(
            "--account {value} matches accounts of more than one provider. Use its unique label (claude-…, openai-…, or a Claude Code instance id)."
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
        let pin = match kind {
            crate::auth::AccountProviderKind::Claude => crate::auth::claude::pin_for_label(label)?,
            crate::auth::AccountProviderKind::OpenAi => crate::auth::codex::pin_for_label(label)?,
            crate::auth::AccountProviderKind::ClaudeCode => {
                crate::auth::claude_code::pin_for_label(label)?
            }
        };
        provider.set_account_pin(kind, Some(pin))?;
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
            // Claude Code instances live in config; the pin alone selects one.
            crate::auth::AccountProviderKind::ClaudeCode => {}
        }
    }
    Ok(())
}
