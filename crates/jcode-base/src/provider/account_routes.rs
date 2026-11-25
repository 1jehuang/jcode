use crate::{provider::Provider, session::Session};
use anyhow::{Context, Result};

/// A retained pin alone does not mean this request uses subscription billing.
pub fn observed_oauth_account_route(provider: &dyn Provider) -> Option<(&'static str, String)> {
    if provider.active_resolved_credential() != Some(jcode_provider_core::ResolvedCredential::Oauth)
    {
        return None;
    }
    let method = match provider.name().to_ascii_lowercase().as_str() {
        "openai" => "openai-oauth",
        "claude" | "anthropic" => "claude-oauth",
        _ => return None,
    };
    Some((method, provider.oauth_account_label()?))
}

/// Capture identity only after an explicit route selection has succeeded.
pub fn record_account_pin(provider: &dyn Provider, session: &mut Session) {
    let Some(mut pin) = provider.oauth_account_pin() else {
        return;
    };
    let Some(method) = session.route_api_method.as_deref() else {
        return;
    };
    let Some((method, label)) = method.split_once('@') else {
        return;
    };
    if pin.label != label && provider.oauth_account_label().as_deref() != Some(label) {
        return;
    }
    let key = match method {
        "claude-oauth" => "claude",
        "openai-oauth" => "openai",
        _ => return,
    };
    pin.label = label.to_owned();
    session.account_pins.insert(key.into(), pin);
}

/// Resolve a restored label through its historical identity before selecting a runtime.
pub fn restore_account_pin_route(session: &mut Session) -> Result<()> {
    let Some(route) = session.route_api_method.clone() else {
        return Ok(());
    };
    let Some((method, label)) = route.split_once('@') else {
        return Ok(());
    };
    let (key, resolve): (&str, fn(&jcode_provider_core::AccountPin) -> Result<String>) =
        match method {
            "claude-oauth" => ("claude", crate::auth::account_pins::resolve_claude),
            "openai-oauth" => ("openai", crate::auth::account_pins::resolve_openai),
            _ => return Ok(()),
        };
    let Some(pin) = session.account_pins.get_mut(key) else {
        return Ok(());
    };
    anyhow::ensure!(
        pin.label == label,
        "Saved account route disagrees with its identity pin"
    );
    let resolved = resolve(pin)?;
    if resolved != label {
        session.route_api_method = Some(format!("{method}@{resolved}"));
        if session.provider_key.as_deref() == Some(&route) {
            session.provider_key = session.route_api_method.clone();
        }
        if let Some(model) = session.model.as_mut()
            && let Some(suffix) = model.strip_prefix(&format!("{route}:"))
        {
            *model = format!("{method}@{resolved}:{suffix}");
        }
        pin.label = resolved;
    }
    Ok(())
}

pub fn apply_restored_account_pin(provider: &dyn Provider, session: &mut Session) -> Result<()> {
    let Some(method) = session.route_api_method.as_deref() else {
        return Ok(());
    };
    let key = if method.starts_with("claude-oauth@") {
        "claude"
    } else if method.starts_with("openai-oauth@") {
        "openai"
    } else {
        return Ok(());
    };
    if let Some(pin) = session.account_pins.get(key) {
        provider.set_oauth_account_pin(pin)?;
        record_account_pin(provider, session);
    }
    Ok(())
}

pub fn ensure_account_identity(provider: &dyn Provider, session: &Session) -> Result<bool> {
    let Some(route) = session.route_api_method.as_deref() else {
        return Ok(false);
    };
    let key = if route.starts_with("claude-oauth@") {
        "claude"
    } else if route.starts_with("openai-oauth@") {
        "openai"
    } else {
        return Ok(false);
    };
    if let Some(expected) = session.account_pins.get(key) {
        let actual = provider.oauth_account_pin().context(
            "Saved account identity could not be restored; select an account before sending",
        )?;
        anyhow::ensure!(
            actual.identity == expected.identity
                || expected
                    .record_id
                    .as_ref()
                    .is_some_and(|record_id| actual.record_id.as_ref() == Some(record_id)),
            "Saved account identity changed; select an account before sending"
        );
        let label = if key == "claude" {
            crate::auth::account_pins::resolve_claude(expected)?
        } else {
            crate::auth::account_pins::resolve_openai(expected)?
        };
        anyhow::ensure!(
            provider.oauth_account_label().as_deref() == Some(label.as_str()),
            "Saved account identity is bound to another route"
        );
        return Ok(true);
    }
    Ok(false)
}

#[cfg(test)]
#[path = "account_route_tests.rs"]
mod tests;
