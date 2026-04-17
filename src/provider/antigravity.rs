use super::cli_common::{build_cli_prompt, run_cli_text_command};
use super::{EventStream, Provider};
use crate::auth::antigravity as antigravity_auth;
use crate::message::{Message, ToolDefinition};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

const DEFAULT_MODEL: &str = "default";
const AVAILABLE_MODELS: &[&str] = &[
    "default",
    "claude-opus-4-6-thinking",
    "claude-sonnet-4-6",
    "gemini-3-pro-high",
    "gemini-3-pro-low",
    "gemini-3-flash",
    "gemini-3.1-pro-high",
    "gemini-3.1-pro-low",
    "gemini-3-flash-agent",
    "gpt-oss-120b-medium",
];
const FETCH_MODELS_API_URL: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels";
const VERSION_ENV: &str = "JCODE_ANTIGRAVITY_VERSION";
const ANTIGRAVITY_VERSION: &str = "1.18.3";
const X_GOOG_API_CLIENT: &str = "google-cloud-sdk vscode_cloudshelleditor/0.1";
const CATALOG_REFRESH_TTL_HOURS: i64 = 6;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
struct PersistedCatalog {
    models: Vec<CatalogModel>,
    fetched_at_rfc3339: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
struct CatalogModel {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reset_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tag_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u64>,
    #[serde(default)]
    recommended: bool,
    #[serde(default)]
    available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    remaining_fraction_milli: Option<u16>,
}

#[derive(Debug, Deserialize)]
struct FetchAvailableModelsResponse {
    #[serde(default)]
    models: HashMap<String, FetchAvailableModelEntry>,
    #[serde(default)]
    default_agent_model_id: Option<String>,
    #[serde(default)]
    command_model_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FetchAvailableModelEntry {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    model_name: Option<String>,
    #[serde(default)]
    quota_info: Option<FetchAvailableQuotaInfo>,
    #[serde(default)]
    recommended: bool,
    #[serde(default)]
    tag_title: Option<String>,
    #[serde(default)]
    model_provider: Option<String>,
    #[serde(default)]
    max_tokens: Option<u64>,
    #[serde(default)]
    max_output_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FetchAvailableQuotaInfo {
    #[serde(default)]
    remaining_fraction: Option<f64>,
    #[serde(default)]
    reset_time: Option<String>,
}

fn metadata_platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "WINDOWS"
    } else {
        "MACOS"
    }
}

fn antigravity_version() -> String {
    std::env::var(VERSION_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| ANTIGRAVITY_VERSION.to_string())
}

fn antigravity_user_agent() -> String {
    if cfg!(target_os = "windows") {
        format!("antigravity/{} windows/amd64", antigravity_version())
    } else if cfg!(target_arch = "aarch64") {
        format!("antigravity/{} darwin/arm64", antigravity_version())
    } else {
        format!("antigravity/{} darwin/amd64", antigravity_version())
    }
}

fn client_metadata_header() -> String {
    format!(
        "{{\"ideType\":\"ANTIGRAVITY\",\"platform\":\"{}\",\"pluginType\":\"GEMINI\"}}",
        metadata_platform()
    )
}

fn remaining_fraction_to_milli(value: Option<f64>) -> Option<u16> {
    let value = value?;
    if !value.is_finite() {
        return None;
    }
    let clamped = value.clamp(0.0, 1.0);
    Some((clamped * 1000.0).round() as u16)
}

fn merge_antigravity_model_ids(models: impl IntoIterator<Item = String>) -> Vec<String> {
    let models: Vec<String> = models
        .into_iter()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect();

    let mut seen = HashSet::new();
    let mut preferred = Vec::new();

    for known in AVAILABLE_MODELS {
        if models.iter().any(|model| model == known) && seen.insert((*known).to_string()) {
            preferred.push((*known).to_string());
        }
    }

    let mut extras: Vec<String> = models
        .into_iter()
        .filter(|model| seen.insert(model.clone()))
        .collect();
    extras.sort();
    preferred.extend(extras);
    preferred
}

pub(crate) fn is_known_model(model: &str) -> bool {
    let normalized = model.trim();
    !normalized.is_empty() && AVAILABLE_MODELS.contains(&normalized)
}

fn parse_fetch_available_models_response(
    response: &FetchAvailableModelsResponse,
) -> Vec<CatalogModel> {
    let mut preferred_ids = Vec::new();
    if let Some(default_agent_model_id) = response.default_agent_model_id.as_deref() {
        preferred_ids.push(default_agent_model_id.trim().to_string());
    }
    preferred_ids.extend(
        response
            .command_model_ids
            .iter()
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty()),
    );
    preferred_ids.extend(response.models.keys().map(|id| id.trim().to_string()));

    let ordered_ids = merge_antigravity_model_ids(preferred_ids);
    let mut by_id: HashMap<String, CatalogModel> = HashMap::new();

    for (model_id, entry) in &response.models {
        let id = model_id.trim();
        if id.is_empty() {
            continue;
        }
        let available = entry
            .quota_info
            .as_ref()
            .and_then(|quota| quota.remaining_fraction)
            .map(|remaining| remaining > 0.0)
            .unwrap_or(true);
        by_id.insert(
            id.to_string(),
            CatalogModel {
                id: id.to_string(),
                display_name: entry
                    .display_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                reset_time: entry
                    .quota_info
                    .as_ref()
                    .and_then(|quota| quota.reset_time.as_deref())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                tag_title: entry
                    .tag_title
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                model_provider: entry
                    .model_provider
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                max_tokens: entry.max_tokens,
                max_output_tokens: entry.max_output_tokens,
                recommended: entry.recommended,
                available,
                remaining_fraction_milli: remaining_fraction_to_milli(
                    entry
                        .quota_info
                        .as_ref()
                        .and_then(|quota| quota.remaining_fraction),
                ),
            },
        );

        if let Some(alias) = entry.model_name.as_deref().map(str::trim)
            && !alias.is_empty()
            && alias != id
        {
            by_id
                .entry(alias.to_string())
                .or_insert_with(|| CatalogModel {
                    id: alias.to_string(),
                    display_name: entry
                        .display_name
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string),
                    reset_time: entry
                        .quota_info
                        .as_ref()
                        .and_then(|quota| quota.reset_time.as_deref())
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string),
                    tag_title: entry
                        .tag_title
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string),
                    model_provider: entry
                        .model_provider
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string),
                    max_tokens: entry.max_tokens,
                    max_output_tokens: entry.max_output_tokens,
                    recommended: entry.recommended,
                    available,
                    remaining_fraction_milli: remaining_fraction_to_milli(
                        entry
                            .quota_info
                            .as_ref()
                            .and_then(|quota| quota.remaining_fraction),
                    ),
                });
        }
    }

    ordered_ids
        .into_iter()
        .map(|id| {
            by_id.remove(&id).unwrap_or(CatalogModel {
                id,
                display_name: None,
                reset_time: None,
                tag_title: None,
                model_provider: None,
                max_tokens: None,
                max_output_tokens: None,
                recommended: false,
                available: true,
                remaining_fraction_milli: None,
            })
        })
        .collect()
}

fn catalog_model_detail(model: &CatalogModel) -> String {
    let mut parts = Vec::new();
    if let Some(display_name) = model.display_name.as_deref()
        && display_name != model.id
    {
        parts.push(display_name.to_string());
    }
    if model.recommended {
        parts.push("recommended".to_string());
    }
    if let Some(tag_title) = model.tag_title.as_deref() {
        parts.push(tag_title.to_string());
    }
    if let Some(model_provider) = model.model_provider.as_deref() {
        parts.push(model_provider.to_ascii_lowercase());
    }
    if let Some(remaining) = model.remaining_fraction_milli {
        let percent = remaining as f64 / 10.0;
        parts.push(format!("quota {:.1}%", percent));
    }
    if let Some(reset_time) = model.reset_time.as_deref() {
        parts.push(format!("resets {}", reset_time));
    }
    parts.join(" · ")
}

fn catalog_is_stale(fetched_at_rfc3339: &str) -> bool {
    let Ok(fetched_at) = DateTime::parse_from_rfc3339(fetched_at_rfc3339) else {
        return true;
    };
    Utc::now()
        .signed_duration_since(fetched_at.with_timezone(&Utc))
        .num_hours()
        >= CATALOG_REFRESH_TTL_HOURS
}

pub struct AntigravityCliProvider {
    cli_path: String,
    client: reqwest::Client,
    model: Arc<RwLock<String>>,
    fetched_catalog: Arc<RwLock<Vec<CatalogModel>>>,
    prompt_flag: Option<String>,
    model_flag: Option<String>,
}

impl Clone for AntigravityCliProvider {
    fn clone(&self) -> Self {
        Self {
            cli_path: self.cli_path.clone(),
            client: self.client.clone(),
            model: self.model.clone(),
            fetched_catalog: self.fetched_catalog.clone(),
            prompt_flag: self.prompt_flag.clone(),
            model_flag: self.model_flag.clone(),
        }
    }
}

impl AntigravityCliProvider {
    fn persisted_catalog_path() -> Result<std::path::PathBuf> {
        Ok(crate::storage::app_config_dir()?.join("antigravity_models_cache.json"))
    }

    fn load_persisted_catalog() -> Option<PersistedCatalog> {
        let path = Self::persisted_catalog_path().ok()?;
        crate::storage::read_json(&path)
            .ok()
            .filter(|catalog: &PersistedCatalog| !catalog.models.is_empty())
    }

    fn persist_catalog(models: &[CatalogModel]) {
        if models.is_empty() {
            return;
        }
        let Ok(path) = Self::persisted_catalog_path() else {
            return;
        };
        let payload = PersistedCatalog {
            models: models.to_vec(),
            fetched_at_rfc3339: Utc::now().to_rfc3339(),
        };
        if let Err(error) = crate::storage::write_json(&path, &payload) {
            crate::logging::warn(&format!(
                "Failed to persist Antigravity model catalog {}: {}",
                path.display(),
                error
            ));
        }
    }

    fn seed_cached_catalog(&self) {
        if let Some(catalog) = Self::load_persisted_catalog()
            && let Ok(mut models) = self.fetched_catalog.write()
        {
            if catalog_is_stale(&catalog.fetched_at_rfc3339) {
                crate::logging::info(
                    "Loaded stale persisted Antigravity model catalog; a refresh will update it on next prefetch",
                );
            }
            *models = catalog.models;
        }
    }

    pub fn new() -> Self {
        let cli_path = std::env::var("JCODE_ANTIGRAVITY_CLI_PATH")
            .unwrap_or_else(|_| "antigravity".to_string());
        let model =
            std::env::var("JCODE_ANTIGRAVITY_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
        let prompt_flag = std::env::var("JCODE_ANTIGRAVITY_PROMPT_FLAG")
            .ok()
            .or_else(|| Some("-p".to_string()));
        let model_flag = std::env::var("JCODE_ANTIGRAVITY_MODEL_FLAG")
            .ok()
            .or_else(|| Some("--model".to_string()));

        let provider = Self {
            cli_path,
            client: crate::provider::shared_http_client(),
            model: Arc::new(RwLock::new(model)),
            fetched_catalog: Arc::new(RwLock::new(Vec::new())),
            prompt_flag,
            model_flag,
        };
        provider.seed_cached_catalog();
        provider
    }

    fn fetched_catalog(&self) -> Vec<CatalogModel> {
        self.fetched_catalog
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    async fn fetch_available_models_with_project(
        &self,
        access_token: &str,
        project_id: Option<&str>,
    ) -> Result<Vec<CatalogModel>> {
        let request = if let Some(project_id) = project_id.filter(|value| !value.trim().is_empty())
        {
            serde_json::json!({ "project": project_id })
        } else {
            serde_json::json!({})
        };

        let response = self
            .client
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
            let body = response.text().await.unwrap_or_default();
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

    async fn fetch_available_models(&self) -> Result<Vec<CatalogModel>> {
        let mut tokens = antigravity_auth::load_or_refresh_tokens().await?;

        if let Ok(models) = self
            .fetch_available_models_with_project(&tokens.access_token, None)
            .await
            && !models.is_empty()
        {
            return Ok(models);
        }

        if let Some(project_id) = tokens.project_id.clone()
            && !project_id.trim().is_empty()
            && let Ok(models) = self
                .fetch_available_models_with_project(&tokens.access_token, Some(&project_id))
                .await
            && !models.is_empty()
        {
            return Ok(models);
        }

        if let Ok(project_id) = antigravity_auth::fetch_project_id(&tokens.access_token).await {
            tokens.project_id = Some(project_id.clone());
            let _ = antigravity_auth::save_tokens(&tokens);
            return self
                .fetch_available_models_with_project(&tokens.access_token, Some(&project_id))
                .await;
        }

        self.fetch_available_models_with_project(&tokens.access_token, None)
            .await
    }
}

impl Default for AntigravityCliProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for AntigravityCliProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let prompt = build_cli_prompt(system, messages);
        let model = self
            .model
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let cli_path = self.cli_path.clone();
        let prompt_flag = self.prompt_flag.clone();
        let model_flag = self.model_flag.clone();
        let cwd = std::env::current_dir().ok();
        let (tx, rx) = mpsc::channel::<Result<crate::message::StreamEvent>>(100);

        tokio::spawn(async move {
            if tx
                .send(Ok(crate::message::StreamEvent::ConnectionType {
                    connection: "cli subprocess".to_string(),
                }))
                .await
                .is_err()
            {
                return;
            }
            let mut cmd = Command::new(&cli_path);
            if let Some(flag) = model_flag.as_ref().filter(|f| !f.trim().is_empty()) {
                cmd.arg(flag).arg(&model);
            }
            if let Some(flag) = prompt_flag.as_ref().filter(|f| !f.trim().is_empty()) {
                cmd.arg(flag).arg(prompt);
            } else {
                cmd.arg(prompt);
            }
            if let Some(dir) = cwd {
                cmd.current_dir(dir);
            }

            if let Err(e) = run_cli_text_command(cmd, tx.clone(), "Antigravity").await {
                let _ = tx.send(Err(e)).await;
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &'static str {
        "antigravity"
    }

    fn model(&self) -> String {
        self.model
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_model(&self, model: &str) -> Result<()> {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            anyhow::bail!("Antigravity model cannot be empty");
        }
        *self
            .model
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = trimmed.to_string();
        Ok(())
    }

    fn available_models(&self) -> Vec<&'static str> {
        AVAILABLE_MODELS.to_vec()
    }

    fn available_models_display(&self) -> Vec<String> {
        let catalog = self.fetched_catalog();
        merge_antigravity_model_ids(
            catalog
                .into_iter()
                .map(|model| model.id)
                .chain(std::iter::once(self.model())),
        )
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        self.available_models_display()
    }

    fn model_routes(&self) -> Vec<super::ModelRoute> {
        let catalog = self.fetched_catalog();
        if !catalog.is_empty() {
            return catalog
                .into_iter()
                .map(|model| super::ModelRoute {
                    model: model.id.clone(),
                    provider: "Antigravity".to_string(),
                    api_method: "cli".to_string(),
                    available: model.available,
                    detail: catalog_model_detail(&model),
                    cheapness: None,
                })
                .collect();
        }

        self.available_models_display()
            .into_iter()
            .map(|model| super::ModelRoute {
                model,
                provider: "Antigravity".to_string(),
                api_method: "cli".to_string(),
                available: true,
                detail: "fallback catalog".to_string(),
                cheapness: None,
            })
            .collect()
    }

    fn on_auth_changed(&self) {
        let provider = self.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if provider.prefetch_models().await.is_ok() {
                    crate::bus::Bus::global().publish(crate::bus::BusEvent::ModelsUpdated);
                }
            });
        }
    }

    async fn prefetch_models(&self) -> Result<()> {
        match self.fetch_available_models().await {
            Ok(models) => {
                if !models.is_empty() {
                    crate::logging::info(&format!(
                        "Discovered Antigravity models: {}",
                        models
                            .iter()
                            .map(|model| model.id.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                    Self::persist_catalog(&models);
                    *self
                        .fetched_catalog
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = models;
                }
            }
            Err(err) => {
                crate::logging::warn(&format!(
                    "Antigravity model catalog refresh failed; keeping fallback list: {}",
                    err
                ));
            }
        }

        Ok(())
    }

    fn supports_compaction(&self) -> bool {
        false
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            cli_path: self.cli_path.clone(),
            client: self.client.clone(),
            model: Arc::new(RwLock::new(self.model())),
            fetched_catalog: self.fetched_catalog.clone(),
            prompt_flag: self.prompt_flag.clone(),
            model_flag: self.model_flag.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fetch_available_models_response_discovers_metadata_and_priority_order() {
        let response: FetchAvailableModelsResponse = serde_json::from_value(serde_json::json!({
            "defaultAgentModelId": "gemini-3.1-pro-high",
            "commandModelIds": ["gemini-3-flash"],
            "models": {
                "claude-opus-4-6-thinking": {
                    "displayName": "Claude Opus 4.6 (Thinking)",
                    "quotaInfo": { "remainingFraction": 1, "resetTime": "2026-04-24T20:53:26Z" },
                    "recommended": true,
                    "modelProvider": "MODEL_PROVIDER_ANTHROPIC"
                },
                "gemini-3.1-pro-high": {
                    "displayName": "Gemini 3.1 Pro (High)",
                    "quotaInfo": { "remainingFraction": 0.25 }
                },
                "gpt-oss-120b-medium": {}
            }
        }))
        .expect("parse response");

        let parsed = parse_fetch_available_models_response(&response);
        assert_eq!(parsed[0].id, "claude-opus-4-6-thinking");
        assert_eq!(parsed[1].id, "gemini-3.1-pro-high");
        assert_eq!(parsed[2].id, "gpt-oss-120b-medium");
        assert_eq!(
            parsed[0].display_name.as_deref(),
            Some("Claude Opus 4.6 (Thinking)")
        );
        assert_eq!(parsed[1].remaining_fraction_milli, Some(250));
    }

    #[test]
    fn available_models_display_includes_dynamic_cache_and_current_override() {
        let provider = AntigravityCliProvider::new();
        *provider.fetched_catalog.write().expect("catalog lock") = vec![
            CatalogModel {
                id: "claude-opus-4-6-thinking".to_string(),
                display_name: None,
                reset_time: None,
                tag_title: None,
                model_provider: None,
                max_tokens: None,
                max_output_tokens: None,
                recommended: true,
                available: true,
                remaining_fraction_milli: Some(1000),
            },
            CatalogModel {
                id: "gemini-3-pro-high".to_string(),
                display_name: None,
                reset_time: None,
                tag_title: None,
                model_provider: None,
                max_tokens: None,
                max_output_tokens: None,
                recommended: false,
                available: true,
                remaining_fraction_milli: Some(1000),
            },
        ];
        provider
            .set_model("custom-antigravity-model")
            .expect("set custom model");

        let models = provider.available_models_display();

        assert!(models.contains(&"claude-opus-4-6-thinking".to_string()));
        assert!(models.contains(&"gemini-3-pro-high".to_string()));
        assert!(models.contains(&"custom-antigravity-model".to_string()));
    }

    #[test]
    fn available_models_display_seeds_from_persisted_catalog() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("temp dir");
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        let path = AntigravityCliProvider::persisted_catalog_path().expect("catalog path");
        crate::storage::write_json(
            &path,
            &PersistedCatalog {
                models: vec![CatalogModel {
                    id: "claude-opus-4-6-thinking".to_string(),
                    display_name: Some("Claude Opus 4.6 (Thinking)".to_string()),
                    reset_time: None,
                    tag_title: None,
                    model_provider: None,
                    max_tokens: None,
                    max_output_tokens: None,
                    recommended: true,
                    available: true,
                    remaining_fraction_milli: Some(1000),
                }],
                fetched_at_rfc3339: Utc::now().to_rfc3339(),
            },
        )
        .expect("write persisted catalog");

        let provider = AntigravityCliProvider::new();
        assert!(
            provider
                .available_models_display()
                .contains(&"claude-opus-4-6-thinking".to_string())
        );

        if let Some(previous) = previous {
            crate::env::set_var("JCODE_HOME", previous);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn catalog_detail_mentions_quota_and_reset() {
        let detail = catalog_model_detail(&CatalogModel {
            id: "claude-opus-4-6-thinking".to_string(),
            display_name: Some("Claude Opus 4.6 (Thinking)".to_string()),
            reset_time: Some("2026-04-24T20:53:26Z".to_string()),
            tag_title: Some("New".to_string()),
            model_provider: Some("MODEL_PROVIDER_ANTHROPIC".to_string()),
            max_tokens: Some(250_000),
            max_output_tokens: Some(64_000),
            recommended: true,
            available: true,
            remaining_fraction_milli: Some(1000),
        });

        assert!(detail.contains("recommended"));
        assert!(detail.contains("quota 100.0%"));
        assert!(detail.contains("resets 2026-04-24T20:53:26Z"));
    }

    #[test]
    fn catalog_stale_handles_invalid_timestamp() {
        assert!(catalog_is_stale("not-a-time"));
    }
}
