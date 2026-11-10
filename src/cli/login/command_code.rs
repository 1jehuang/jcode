//! Command Code login validation.
//!
//! The public model catalog is unauthenticated, so a successful `/models`
//! response does not prove that an API key is valid. Command Code exposes an
//! authenticated `/alpha/whoami` endpoint used by its own CLI, but it is not a
//! documented third-party contract. Validate best effort: reject explicit
//! authentication failures, and keep the key with a warning when the validator
//! is unavailable.

use anyhow::{Context, Result};
use std::time::Duration;

const WHOAMI_URL: &str = "https://api.commandcode.ai/alpha/whoami";

pub(super) fn save_account_key(label: Option<&str>, api_key: &str) -> Result<String> {
    crate::provider_catalog::command_code_accounts::save_account(label, api_key)
}

pub(super) fn save_profile_key(
    profile: &crate::provider_catalog::ResolvedOpenAiCompatibleProfile,
    account_label: Option<&str>,
    api_key: &str,
) -> Result<()> {
    if profile.id == "command-code" {
        let label = save_account_key(account_label, api_key)?;
        eprintln!("Stored Command Code account `{label}`.");
    } else {
        crate::provider_catalog::save_named_api_key(
            &profile.env_file,
            &profile.api_key_env,
            api_key,
        )?;
    }
    Ok(())
}

/// Switch to an already-stored account when the caller names one and does not
/// provide a replacement key. Returns false when the label is new, so the
/// normal prompt can collect a key and create it.
pub(super) fn select_stored_account(label: Option<&str>) -> Result<bool> {
    let Some(label) = label.map(str::trim).filter(|label| !label.is_empty()) else {
        return Ok(false);
    };
    let labels = crate::provider_catalog::command_code_accounts::labels();
    if !labels.iter().any(|existing| existing == label) {
        return Ok(false);
    }
    crate::provider_catalog::command_code_accounts::set_active(label)?;
    eprintln!("Using stored Command Code account `{label}`.");
    Ok(true)
}

pub(super) async fn validate_saved_key() -> Result<()> {
    let profile = crate::provider_catalog::COMMAND_CODE_PROFILE;
    let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
    let key = crate::provider_catalog::load_openai_compatible_profile_api_key(&resolved)
        .context("Command Code API key was not saved")?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("build Command Code validation client")?;
    let response = client
        .get(WHOAMI_URL)
        .bearer_auth(key)
        .header(
            reqwest::header::USER_AGENT,
            format!("jcode/{}", env!("CARGO_PKG_VERSION")),
        )
        .send()
        .await;
    match response {
        Ok(response) if response.status().is_success() => {
            let body = response.text().await.unwrap_or_default();
            let value: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let user_name = value
                .pointer("/user/userName")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if user_name.is_empty() {
                eprintln!(
                    "Warning: Command Code accepted the key but returned no user identity. The first model request will confirm access."
                );
            } else {
                eprintln!("Authenticated to Command Code as {user_name}.");
            }
            Ok(())
        }
        Ok(response)
            if response.status() == reqwest::StatusCode::UNAUTHORIZED
                || response.status() == reqwest::StatusCode::FORBIDDEN =>
        {
            anyhow::bail!(
                "Command Code rejected the API key ({}). Check the key in Command Code Studio and try again.",
                response.status()
            )
        }
        Ok(response) => {
            eprintln!(
                "Warning: Command Code key validation is unavailable ({}). The key was saved; the first model request will confirm access.",
                response.status()
            );
            Ok(())
        }
        Err(error) => {
            eprintln!(
                "Warning: could not reach Command Code to validate the key ({error}). The key was saved; the first model request will confirm access."
            );
            Ok(())
        }
    }
}
