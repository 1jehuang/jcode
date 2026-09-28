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
    AVAILABLE_MODELS, CatalogModel, CatalogSnapshot, DEFAULT_FALLBACK_MODEL,
    FetchAvailableModelsResponse, PersistedCatalog, X_GOOG_API_CLIENT,
    antigravity_compatible_schema, antigravity_user_agent, catalog_is_stale, catalog_model_detail,
    client_metadata_header, fetch_models_api_url, is_retryable_empty_turn,
    merge_antigravity_model_ids, parse_fetch_available_models_response, remap_unsupported_model,
};

/// Path of the persisted warm-catalog cache shared by the runtime crate and
/// base's `/usage` report.
pub fn persisted_catalog_path() -> Result<std::path::PathBuf> {
    Ok(crate::storage::app_config_dir()?.join("antigravity_models_cache.json"))
}

/// Load the persisted warm catalog, if present, non-empty, and fetched from
/// the currently resolved Cloud Code endpoint. A catalog cached from a
/// different endpoint (e.g. after an endpoint default change or a new
/// `JCODE_ANTIGRAVITY_ENDPOINT`) is discarded rather than served stale: the
/// set of models an endpoint serves is endpoint-specific, so reusing it could
/// offer a model id the current endpoint cannot actually serve.
pub fn load_persisted_catalog() -> Option<PersistedCatalog> {
    let path = persisted_catalog_path().ok()?;
    crate::storage::read_json(&path)
        .ok()
        .filter(|catalog: &PersistedCatalog| {
            !catalog.models.is_empty()
                && jcode_provider_antigravity::catalog_matches_current_endpoint(catalog)
        })
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
        endpoint: Some(jcode_provider_antigravity::antigravity_endpoint()),
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
        .post(fetch_models_api_url())
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
