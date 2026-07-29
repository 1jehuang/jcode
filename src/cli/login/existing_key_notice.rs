//! Tell the user when `jcode login` found a credential they already saved.
//!
//! Split out of `login.rs` so the notice can carry its rationale and tests
//! without growing an already-oversized file.

use crate::provider_catalog::ResolvedOpenAiCompatibleProfile;

/// Build the notice shown when a key is already configured, or `None` when
/// nothing is configured yet.
///
/// Without this the prompt looks identical whether jcode found an existing
/// credential or not, so someone who wrote their provider env file by hand
/// cannot tell the key was picked up, and reasonably reads the silent prompt as
/// a hang (issue #660). Returning the text rather than printing it keeps the
/// source-resolution logic testable.
pub(super) fn existing_api_key_notice(
    resolved: &ResolvedOpenAiCompatibleProfile,
) -> Option<String> {
    crate::provider_catalog::load_api_key_from_env_or_config(
        &resolved.api_key_env,
        &resolved.env_file,
    )?;
    Some(format!(
        "A {} API key is already configured from {}.\nPaste a new key to replace it, or press Ctrl+C to keep it.\n",
        resolved.display_name,
        credential_source(resolved)
    ))
}

/// Where the resolved key came from, mirroring the precedence in
/// `load_api_key_from_env_or_config`: the process environment wins over the
/// saved config file.
fn credential_source(resolved: &ResolvedOpenAiCompatibleProfile) -> String {
    if std::env::var(&resolved.api_key_env).is_ok_and(|value| !value.trim().is_empty()) {
        return format!("the {} environment variable", resolved.api_key_env);
    }
    match crate::storage::app_config_dir() {
        Ok(dir) => dir.join(&resolved.env_file).display().to_string(),
        Err(_) => resolved.env_file.clone(),
    }
}

pub(super) fn announce_existing_api_key(resolved: &ResolvedOpenAiCompatibleProfile) {
    if let Some(notice) = existing_api_key_notice(resolved) {
        eprintln!("{}", notice);
    }
}

#[cfg(test)]
#[path = "existing_key_notice_tests.rs"]
mod existing_key_notice_tests;
