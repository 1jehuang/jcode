//! Antigravity pure protocol types plus the credentialed catalog fetch
//! (compatibility shim).
//!
//! The Antigravity provider *runtime* (`AntigravityProvider`) now lives in the
//! downstream `jcode-provider-antigravity-runtime` crate so provider edits do
//! not rebuild the base -> app-core -> tui spine. The binary's composition
//! root registers it via [`crate::provider::external`].
//!
//! Base keeps two things here:
//! - re-exports of the pure request/response types and helpers (from
//!   `jcode-provider-antigravity`) at their historical paths, and
//! - the credentialed `fetchAvailableModels` catalog snapshot, because base's
//!   `/usage` report needs per-model quota data without a runtime instance.

use crate::auth::antigravity as antigravity_auth;
use anyhow::{Context, Result};
use chrono::Utc;

pub use jcode_provider_antigravity::is_known_model;
pub use jcode_provider_antigravity::{
    AVAILABLE_MODELS, CatalogModel, CatalogSnapshot, DEFAULT_FALLBACK_MODEL, FETCH_MODELS_API_URL,
    FetchAvailableModelsResponse, GENERATE_CONTENT_API_URL, PersistedCatalog, X_GOOG_API_CLIENT,
    antigravity_compatible_schema, antigravity_user_agent, catalog_is_stale, catalog_model_detail,
    client_metadata_header, is_retryable_empty_turn, merge_antigravity_model_ids,
    parse_fetch_available_models_response, remap_unsupported_model,
};

/// Path of the persisted warm-catalog cache shared by the runtime crate and
/// base's `/usage` report.
pub fn persisted_catalog_path() -> Result<std::path::PathBuf> {
    Ok(crate::storage::app_config_dir()?.join("antigravity_models_cache.json"))
}

/// Load the persisted warm catalog, if present and non-empty.
pub fn load_persisted_catalog() -> Option<PersistedCatalog> {
    let path = persisted_catalog_path().ok()?;
    crate::storage::read_json(&path)
        .ok()
        .filter(|catalog: &PersistedCatalog| !catalog.models.is_empty())
}

/// Persist the warm catalog so later processes skip the cold fetch.
pub fn persist_catalog(snapshot: &CatalogSnapshot) {
    if snapshot.models.is_empty() {
        return;
    }
    let Ok(path) = persisted_catalog_path() else {
        return;
    };
    let payload = PersistedCatalog {
        models: snapshot.models.clone(),
        fetched_at_rfc3339: Utc::now().to_rfc3339(),
        default_model_id: snapshot.default_model_id.clone(),
    };
    if let Err(error) = crate::storage::write_json(&path, &payload) {
        crate::logging::warn(&format!(
            "Failed to persist Antigravity model catalog {}: {}",
            path.display(),
            error
        ));
    }
}

async fn fetch_available_models_with_project(
    client: &reqwest::Client,
    access_token: &str,
    project_id: Option<&str>,
) -> Result<CatalogSnapshot> {
    let request = if let Some(project_id) = project_id.filter(|value| !value.trim().is_empty()) {
        serde_json::json!({ "project": project_id })
    } else {
        serde_json::json!({})
    };

    let response = client
        .post(FETCH_MODELS_API_URL)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", access_token),
        )
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::USER_AGENT, antigravity_user_agent())
        .header(
            reqwest::header::HeaderName::from_static("x-goog-api-client"),
            X_GOOG_API_CLIENT,
        )
        .header(
            reqwest::header::HeaderName::from_static("client-metadata"),
            client_metadata_header(),
        )
        .json(&request)
        .send()
        .await
        .context("Failed to fetch Antigravity model catalog")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = crate::util::http_error_body(response, "HTTP error").await;
        anyhow::bail!(
            "Antigravity model catalog request failed ({}): {}",
            status,
            body.trim()
        );
    }

    let parsed: FetchAvailableModelsResponse = response
        .json()
        .await
        .context("Failed to decode Antigravity model catalog response")?;
    Ok(parse_fetch_available_models_response(&parsed))
}

/// Fetch a model/quota catalog for a specific OAuth credential without
/// changing the process-wide active account or the legacy token file.
///
/// Managed account polling uses this boundary so each account can be measured
/// independently. The returned snapshot contains only model metadata and
/// quota/reset fields, never credentials.
pub async fn fetch_catalog_snapshot_for_credentials(
    client: &reqwest::Client,
    access_token: &str,
    project_id: Option<&str>,
) -> Result<CatalogSnapshot> {
    if let Some(project_id) = project_id.filter(|value| !value.trim().is_empty())
        && let Ok(snapshot) =
            fetch_available_models_with_project(client, access_token, Some(project_id)).await
        && !snapshot.models.is_empty()
    {
        return Ok(snapshot);
    }

    if let Ok(project_id) = antigravity_auth::fetch_project_id(access_token).await
        && let Ok(snapshot) =
            fetch_available_models_with_project(client, access_token, Some(&project_id)).await
        && !snapshot.models.is_empty()
    {
        return Ok(snapshot);
    }

    fetch_available_models_with_project(client, access_token, None).await
}

/// Refresh quota snapshots for managed accounts other than the active account.
/// The active account is still fetched by the existing usage path so its report
/// remains authoritative and this sweep does not issue a duplicate request.
pub async fn refresh_managed_account_quotas(client: &reqwest::Client) {
    let active = crate::auth::provider_pool::active_account("antigravity")
        .ok()
        .flatten()
        .map(|account| account.label);

    let accounts = crate::auth::provider_pool::list_accounts("antigravity").unwrap_or_default();
    for account in accounts {
        if active.as_deref() == Some(account.label.as_str())
            || crate::auth::provider_pool::account_on_cooldown("antigravity", &account.label)
        {
            continue;
        }

        let mut tokens = antigravity_auth::AntigravityTokens {
            access_token: account.access_token,
            refresh_token: account.refresh_token,
            expires_at: account.expires_at,
            email: account.email,
            project_id: account.project_id,
        };
        if tokens.is_expired() {
            tokens = match antigravity_auth::refresh_tokens(&tokens).await {
                Ok(tokens) => tokens,
                Err(error) => {
                    crate::logging::info(&format!(
                        "Skipping Antigravity quota refresh for account '{}': {}",
                        account.label, error
                    ));
                    continue;
                }
            };
        }

        match fetch_catalog_snapshot_for_credentials(
            client,
            &tokens.access_token,
            tokens.project_id.as_deref(),
        )
        .await
        {
            Ok(snapshot) => {
                let quotas = snapshot
                    .models
                    .into_iter()
                    .map(|model| (model.id, model.remaining_fraction_milli, model.reset_time))
                    .collect::<Vec<_>>();
                crate::auth::provider_pool::record_account_quotas(
                    "antigravity",
                    &account.label,
                    &quotas,
                );
            }
            Err(error) => crate::logging::info(&format!(
                "Antigravity quota refresh failed for account '{}': {}",
                account.label, error
            )),
        }
    }
}

/// Fetch the live Antigravity model catalog using the resolved Google OAuth
/// credential, trying the stored project id, then a freshly-resolved project
/// id, then no project.
///
/// Shared by the runtime crate's prefetch/doctor paths and base's `/usage`
/// report (per-model `remaining_fraction` quota + reset times).
pub async fn fetch_catalog_snapshot(client: &reqwest::Client) -> Result<CatalogSnapshot> {
    let mut tokens = antigravity_auth::load_or_refresh_tokens().await?;

    if let Some(project_id) = tokens
        .project_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        && let Ok(snapshot) =
            fetch_available_models_with_project(client, &tokens.access_token, Some(project_id))
                .await
        && !snapshot.models.is_empty()
    {
        return Ok(snapshot);
    }

    if let Ok(project_id) = antigravity_auth::fetch_project_id(&tokens.access_token).await {
        tokens.project_id = Some(project_id.clone());
        let _ = antigravity_auth::save_tokens(&tokens);
        if let Ok(snapshot) =
            fetch_available_models_with_project(client, &tokens.access_token, Some(&project_id))
                .await
            && !snapshot.models.is_empty()
        {
            return Ok(snapshot);
        }
    }

    fetch_available_models_with_project(client, &tokens.access_token, None).await
}
