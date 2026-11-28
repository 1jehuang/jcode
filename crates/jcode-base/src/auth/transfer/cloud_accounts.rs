use super::*;

pub(super) fn cloud_keys(
    provider: TransferProvider,
) -> (&'static str, &'static str, &'static str, &'static str) {
    match provider {
        TransferProvider::OpenAi => (
            "openai_accounts",
            "active_openai_account",
            "access_token",
            "refresh_token",
        ),
        TransferProvider::Claude => (
            "anthropic_accounts",
            "active_anthropic_account",
            "access",
            "refresh",
        ),
    }
}

pub(super) fn valid_cloud_label(label: &str) -> bool {
    !label.trim().is_empty() && label.len() <= 256 && !label.chars().any(char::is_control)
}

// Normalize only known account fields, never copy arbitrary store contents.
// Expired access tokens are intentional: the receiving runtime may refresh them.
pub(super) fn cloud_account(
    provider: TransferProvider,
    value: &serde_json::Value,
) -> Result<serde_json::Value, TransferError> {
    let mut value = value.clone();
    if let Some(object) = value.as_object_mut() {
        object.remove("record_id");
    }
    let normalized = match provider {
        TransferProvider::OpenAi => {
            let account: crate::auth::codex::OpenAiAccount =
                serde_json::from_value(value).map_err(|_| TransferError::InvalidPayload)?;
            serde_json::to_value(account)
        }
        TransferProvider::Claude => {
            let account: crate::auth::claude::AnthropicAccount =
                serde_json::from_value(value).map_err(|_| TransferError::InvalidPayload)?;
            serde_json::to_value(account)
        }
    }
    .map_err(|_| TransferError::InvalidPayload)?;
    let (_, _, access, refresh) = cloud_keys(provider);
    if !normalized["label"].as_str().is_some_and(valid_cloud_label)
        || !normalized[access]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty())
        || !normalized[refresh]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty())
    {
        return Err(TransferError::InvalidPayload);
    }
    Ok(normalized)
}

pub(super) fn cloud_record_lineage_proven(
    provider: TransferProvider,
    old: &serde_json::Value,
    new: &serde_json::Value,
    baseline: bool,
) -> bool {
    let (_, _, _, refresh) = cloud_keys(provider);
    if baseline || old[refresh] == new[refresh] {
        return true;
    }
    let key = match provider {
        TransferProvider::OpenAi => "account_id",
        TransferProvider::Claude => "email",
    };
    old[key]
        .as_str()
        .is_some_and(|identity| !identity.trim().is_empty() && new[key].as_str() == Some(identity))
}
