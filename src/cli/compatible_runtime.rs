//! OpenAI-compatible profile runtime init, including the native Muse
//! subscription (native device flow).
//!
//! Extracted from `provider_init.rs` so compatible-profile runtime wiring
//! stays in one cohesive place. The Muse subscription branch reuses the same
//! compatible runtime but skips the API-key consent prompt: its credential
//! lives in `auth::muse`'s own JSON store and is resolved by the coordinator,
//! never pasted here.

use super::provider_init::{
    ProviderChoice, disable_subscription_runtime_mode,
    ensure_external_api_key_auth_allowed_for_explicit_choice, profile_for_choice,
};
use crate::provider;
use crate::provider_catalog::{
    force_apply_openai_compatible_profile_env, resolve_openai_compatible_profile,
};
use anyhow::Result;
use std::sync::Arc;

/// Build the initial runtime for an explicit compatible-profile choice.
///
/// `notify` mirrors the parent's `init_notice` closure: notices print only on
/// the interactive path.
pub(super) async fn init_compatible_runtime(
    choice: &ProviderChoice,
    notify: &dyn Fn(&str),
) -> Result<Arc<dyn provider::Provider>> {
    if matches!(choice, ProviderChoice::MuseSubscription) {
        return init_muse_subscription_runtime(choice, notify).await;
    }
    disable_subscription_runtime_mode();
    let profile = profile_for_choice(choice)
        .ok_or_else(|| anyhow::anyhow!("missing provider profile for choice"))?;
    if std::env::var_os("JCODE_NAMED_PROVIDER_PROFILE").is_none() {
        // An explicit `--provider <compatible>` selection should win over
        // any stale active-profile marker inherited from a previous
        // bootstrap/login flow. Named provider profiles still take
        // precedence when explicitly configured.
        force_apply_openai_compatible_profile_env(Some(profile));
    }
    let mut runtime_model_hint = None;
    let display_name = if let Ok(named) = std::env::var("JCODE_NAMED_PROVIDER_PROFILE") {
        if let Some(profile) = crate::config::config().providers.get(&named) {
            runtime_model_hint = profile.default_model.clone();
        }
        named
    } else {
        let resolved = resolve_openai_compatible_profile(profile);
        if resolved.requires_api_key {
            ensure_external_api_key_auth_allowed_for_explicit_choice(&resolved.api_key_env)?;
        }
        runtime_model_hint = resolved.default_model.clone();
        resolved.display_name
    };
    notify(&format!(
        "Using {} via OpenAI-compatible API as the initial provider",
        display_name
    ));
    crate::provider::activation::apply_openai_compatible_runtime(runtime_model_hint)?;
    if std::env::var_os("JCODE_NAMED_PROVIDER_PROFILE").is_some() {
        let profile_name = std::env::var("JCODE_NAMED_PROVIDER_PROFILE")?;
        let cfg = crate::config::config();
        let profile = cfg
            .providers
            .get(&profile_name)
            .ok_or_else(|| anyhow::anyhow!("Unknown provider profile '{}'", profile_name))?;
        Ok(Arc::new(
            jcode_provider_openrouter_runtime::OpenRouterProvider::new_named_openai_compatible(
                &profile_name,
                profile,
            )?,
        ))
    } else {
        Ok(Arc::new(
            jcode_provider_openrouter_runtime::OpenRouterProvider::new()?,
        ))
    }
}

async fn init_muse_subscription_runtime(
    choice: &ProviderChoice,
    notify: &dyn Fn(&str),
) -> Result<Arc<dyn provider::Provider>> {
    disable_subscription_runtime_mode();
    // Native Muse subscription (generally available). No API-key consent
    // prompt: the credential lives in auth::muse's own JSON store and is
    // resolved by the coordinator, never pasted here.
    notify("Using Muse subscription provider (native device flow; no local Muse required)");
    let profile = profile_for_choice(choice)
        .ok_or_else(|| anyhow::anyhow!("missing provider profile for choice"))?;
    if std::env::var_os("JCODE_NAMED_PROVIDER_PROFILE").is_none() {
        force_apply_openai_compatible_profile_env(Some(profile));
    }
    let resolved = resolve_openai_compatible_profile(profile);
    crate::provider::activation::apply_openai_compatible_runtime(resolved.default_model.clone())?;
    // The requested/default model is applied by the caller's post-match model
    // policy, exactly like the inlined arm this replaces.
    Ok(Arc::new(provider::MultiProvider::new_fast()))
}
