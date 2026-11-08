//! Unix-only cloud transfer store helpers.
//!
//! Every caller lives in `#[cfg(unix)]` import/export blocks, so these are
//! compiled only on unix and never referenced by the Windows fallbacks.

use super::TransferError;
use super::TransferProvider;
use super::cloud_account;
use super::cloud_keys;
use super::valid_cloud_label;

pub(super) fn cloud_store(
    bytes: &[u8],
    provider: TransferProvider,
) -> Result<serde_json::Value, TransferError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| TransferError::InvalidStore)?;
    if !value.is_object() {
        return Err(TransferError::InvalidStore);
    }
    let (accounts, active, _, _) = cloud_keys(provider);
    if let Some(selection) = value.get(active)
        && !selection.is_null()
        && !selection.as_str().is_some_and(valid_cloud_label)
    {
        return Err(TransferError::InvalidStore);
    }
    if let Some(entries) = value.get(accounts) {
        let entries = entries.as_array().ok_or(TransferError::InvalidStore)?;
        let mut labels = std::collections::HashSet::new();
        for entry in entries {
            let label = entry["label"]
                .as_str()
                .filter(|s| valid_cloud_label(s))
                .ok_or(TransferError::InvalidStore)?;
            if !labels.insert(label) {
                return Err(TransferError::InvalidStore);
            }
            cloud_account(provider, entry).map_err(|_| TransferError::InvalidStore)?;
        }
    }
    Ok(value)
}

pub(super) fn identity_hints_match(
    provider: TransferProvider,
    old: &serde_json::Value,
    new: &serde_json::Value,
) -> bool {
    let fields: &[&str] = match provider {
        TransferProvider::OpenAi => &["account_id", "email"],
        TransferProvider::Claude => &["email"],
    };
    let present = |value: &serde_json::Value| value.as_str().is_some_and(|s| !s.trim().is_empty());
    // A hint missing on either side is not evidence of a different account:
    // older refreshes dropped `email` when the grant omitted id_token. Any hint
    // present on both sides must still match exactly, and at least one hint
    // must be shared so an identity-less entry cannot absorb another account.
    let mut shared = false;
    for key in fields {
        let (a, b) = (&old[*key], &new[*key]);
        if present(a) && present(b) {
            if a != b {
                return false;
            }
            shared = true;
        }
    }
    shared || fields.iter().all(|key| old[*key] == new[*key])
}

pub(super) fn identity_hints_present(
    provider: TransferProvider,
    account: &serde_json::Value,
) -> bool {
    let fields: &[&str] = match provider {
        TransferProvider::OpenAi => &["account_id", "email"],
        TransferProvider::Claude => &["email"],
    };
    fields
        .iter()
        .any(|key| account[*key].as_str().is_some_and(|s| !s.trim().is_empty()))
}

pub(super) fn cloud_expiry(provider: TransferProvider, account: &serde_json::Value) -> Option<i64> {
    match provider {
        TransferProvider::Claude => account["expires"].as_i64(),
        TransferProvider::OpenAi => account["expires_at"].as_i64().or_else(|| {
            account["access_token"]
                .as_str()
                .and_then(super::super::codex::expires_at_from_access_token)
        }),
    }
}
