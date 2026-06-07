use super::{EventStream, Provider};
use crate::auth::antigravity as antigravity_auth;
use crate::message::{ConnectionPhase, Message, StreamEvent, ToolDefinition};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
pub(crate) use jcode_provider_antigravity::is_known_model;
use jcode_provider_antigravity::{
    AVAILABLE_MODELS, CatalogModel, CatalogSnapshot, DEFAULT_FALLBACK_MODEL, FETCH_MODELS_API_URL,
    FetchAvailableModelsResponse, GENERATE_CONTENT_API_URL, PersistedCatalog, X_GOOG_API_CLIENT,
    antigravity_compatible_schema, antigravity_user_agent, catalog_is_stale, catalog_model_detail,
    client_metadata_header, is_retryable_empty_turn, merge_antigravity_model_ids,
    parse_fetch_available_models_response, remap_unsupported_model,
};
#[cfg(test)]
use jcode_provider_antigravity::{
    flatten_schema_combiners, metadata_platform, model_is_claude, model_is_gemini,
    strip_numeric_schema_bounds,
};
use jcode_provider_gemini::{
    CodeAssistGenerateRequest, CodeAssistGenerateResponse, GeminiFunctionCallingConfig,
    GeminiToolConfig, VertexGenerateContentRequest,
};
use serde_json::{Value, json};
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

const DEFAULT_MODEL: &str = "default";

pub struct AntigravityProvider {
    client: reqwest::Client,
    model: Arc<RwLock<String>>,
    fetched_catalog: Arc<RwLock<Vec<CatalogModel>>>,
    /// Backend-advertised default agent model id (from `fetchAvailableModels`).
    /// Used to resolve the `"default"` alias to a real model for inference.
    backend_default_model: Arc<RwLock<Option<String>>>,
}

impl Clone for AntigravityProvider {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            model: self.model.clone(),
            fetched_catalog: self.fetched_catalog.clone(),
            backend_default_model: self.backend_default_model.clone(),
        }
    }
}

impl AntigravityProvider {
    fn persisted_catalog_path() -> Result<std::path::PathBuf> {
        Ok(crate::storage::app_config_dir()?.join("antigravity_models_cache.json"))
    }

    fn load_persisted_catalog() -> Option<PersistedCatalog> {
        let path = Self::persisted_catalog_path().ok()?;
        crate::storage::read_json(&path)
            .ok()
            .filter(|catalog: &PersistedCatalog| !catalog.models.is_empty())
    }

    fn persist_catalog(snapshot: &CatalogSnapshot) {
        if snapshot.models.is_empty() {
            return;
        }
        let Ok(path) = Self::persisted_catalog_path() else {
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

    fn seed_cached_catalog(&self) {
        if let Some(catalog) = Self::load_persisted_catalog() {
            if catalog_is_stale(&catalog.fetched_at_rfc3339) {
                crate::logging::info(
                    "Loaded stale persisted Antigravity model catalog; a refresh will update it on next prefetch",
                );
            }
            if let Some(default_model_id) = catalog.default_model_id.clone() {
                *self
                    .backend_default_model
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(default_model_id);
            }
            if let Ok(mut models) = self.fetched_catalog.write() {
                *models = catalog.models;
            }
        }
    }

    pub fn new() -> Self {
        let model =
            std::env::var("JCODE_ANTIGRAVITY_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());

        let provider = Self {
            client: crate::provider::shared_http_client(),
            model: Arc::new(RwLock::new(model)),
            fetched_catalog: Arc::new(RwLock::new(Vec::new())),
            backend_default_model: Arc::new(RwLock::new(None)),
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

    fn backend_default_model(&self) -> Option<String> {
        self.backend_default_model
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Resolve the live OAuth credential the runtime would use, for the
    /// provider-doctor's native Antigravity driver.
    ///
    /// Antigravity authenticates exclusively via the Google OAuth tokens minted
    /// by `jcode login --provider antigravity`; there is no API-key path. This
    /// loads (and refreshes if needed) those tokens through the exact same code
    /// path inference uses, returning only the resolved Google account email so
    /// the doctor can confirm the credential without ever surfacing the token
    /// itself.
    pub async fn resolve_account_for_doctor(&self) -> Result<String> {
        let tokens = antigravity_auth::load_or_refresh_tokens().await?;
        if tokens.access_token.trim().is_empty() {
            anyhow::bail!("resolved an empty Antigravity access token");
        }
        match antigravity_auth::fetch_email(&tokens.access_token).await {
            Ok(email) if !email.trim().is_empty() => Ok(email),
            _ => Ok(String::new()),
        }
    }

    /// Fetch the live Antigravity model catalog using the resolved credential.
    ///
    /// Mirrors [`Provider::prefetch_models`] but returns the available model ids
    /// to the caller (rather than only persisting them) so the doctor can assert
    /// the live `fetchAvailableModels` endpoint works and that the model under
    /// test is in the live catalog. The warm catalog is persisted exactly like
    /// the runtime's own prefetch so the rest of the process benefits.
    pub async fn fetch_live_model_ids_for_doctor(&self) -> Result<Vec<String>> {
        let snapshot = self.fetch_available_models().await?;
        if snapshot.models.is_empty() {
            anyhow::bail!("Antigravity model catalog returned no models");
        }
        Self::persist_catalog(&snapshot);
        if let Some(default_model_id) = snapshot.default_model_id.clone() {
            *self
                .backend_default_model
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(default_model_id);
        }
        let model_ids: Vec<String> = snapshot
            .models
            .iter()
            .filter(|model| model.available)
            .map(|model| model.id.clone())
            .collect();
        *self
            .fetched_catalog
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = snapshot.models;
        if model_ids.is_empty() {
            anyhow::bail!("Antigravity model catalog returned no available models");
        }
        Ok(model_ids)
    }

    /// Resolve a requested model id into a real backend model id. The literal
    /// alias `"default"` (and the empty string) is rejected by the
    /// `generateContent` endpoint with HTTP 404, so it is mapped to the
    /// backend-advertised default, then to a known-good catalog model, and
    /// finally to a hardcoded fallback.
    ///
    /// Note: this only resolves the `"default"` alias / empty input. An
    /// explicit model id from the user is honoured verbatim, except for ids
    /// the backend advertises but cannot actually service, which are remapped
    /// to an equivalent working id via [`remap_unsupported_model`].
    fn resolve_model_for_request(&self, model: &str) -> String {
        let trimmed = model.trim();
        if !trimmed.is_empty() && trimmed != DEFAULT_MODEL {
            return remap_unsupported_model(trimmed).to_string();
        }

        if let Some(backend_default) = self
            .backend_default_model()
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty() && id != DEFAULT_MODEL)
        {
            return backend_default;
        }

        // No backend-advertised default: pick a usable catalog model. Prefer a
        // Gemini model, which works reliably with tool use on this backend.
        // Claude models on the Cloud Code backend currently reject jcode's tool
        // schemas (they require JSON Schema draft 2020-12), so they are a poor
        // automatic default even when listed first in the catalog.
        let catalog = self.fetched_catalog();
        if let Some(gemini_model) = catalog
            .iter()
            .find(|model| {
                model.available
                    && model.id.trim() != DEFAULT_MODEL
                    && model.id.starts_with("gemini-")
            })
            .map(|model| model.id.clone())
        {
            return gemini_model;
        }
        if let Some(catalog_model) = catalog
            .iter()
            .find(|model| model.available && model.id.trim() != DEFAULT_MODEL)
            .map(|model| model.id.clone())
        {
            return catalog_model;
        }

        DEFAULT_FALLBACK_MODEL.to_string()
    }

    async fn fetch_available_models_with_project(
        &self,
        access_token: &str,
        project_id: Option<&str>,
    ) -> Result<CatalogSnapshot> {
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

    async fn fetch_available_models(&self) -> Result<CatalogSnapshot> {
        let mut tokens = antigravity_auth::load_or_refresh_tokens().await?;

        if let Some(project_id) = tokens
            .project_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            && let Ok(snapshot) = self
                .fetch_available_models_with_project(&tokens.access_token, Some(project_id))
                .await
            && !snapshot.models.is_empty()
        {
            return Ok(snapshot);
        }

        if let Ok(project_id) = antigravity_auth::fetch_project_id(&tokens.access_token).await {
            tokens.project_id = Some(project_id.clone());
            let _ = antigravity_auth::save_tokens(&tokens);
            if let Ok(snapshot) = self
                .fetch_available_models_with_project(&tokens.access_token, Some(&project_id))
                .await
                && !snapshot.models.is_empty()
            {
                return Ok(snapshot);
            }
        }

        self.fetch_available_models_with_project(&tokens.access_token, None)
            .await
    }

    async fn generate_content(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
        force_function_call: bool,
    ) -> Result<CodeAssistGenerateResponse> {
        let mut tokens = antigravity_auth::load_or_refresh_tokens().await?;
        let project = match tokens
            .project_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            Some(project_id) => project_id.to_string(),
            None => {
                let project_id = antigravity_auth::fetch_project_id(&tokens.access_token).await?;
                tokens.project_id = Some(project_id.clone());
                let _ = antigravity_auth::save_tokens(&tokens);
                project_id
            }
        };
        let resolved_model = self.resolve_model_for_request(model);
        let tools_is_empty = tools.is_empty();
        let mut tools = super::gemini::build_tools(tools);
        // Normalize each tool's JSON schema for the specific Antigravity backend
        // path the resolved model uses. The Cloud Code backend forwards each
        // model family to a different upstream (Gemini-native, Gemini->Anthropic,
        // or an OpenAI-compatible bridge), and each upstream rejects a different
        // construct. Gemini-native accepts everything jcode emits, so Gemini
        // models pass through unchanged. See `antigravity_compatible_schema`.
        if let Some(tools) = tools.as_mut() {
            for tool in tools.iter_mut() {
                for decl in tool.function_declarations.iter_mut() {
                    decl.parameters =
                        antigravity_compatible_schema(&decl.parameters, &resolved_model);
                }
            }
        }
        let request = CodeAssistGenerateRequest {
            model: resolved_model,
            project,
            user_prompt_id: Uuid::new_v4().to_string(),
            request: VertexGenerateContentRequest {
                contents: super::gemini::build_contents(messages),
                system_instruction: super::gemini::build_system_instruction_with_tool_guard(
                    system,
                    !tools_is_empty,
                ),
                tools,
                tool_config: if tools_is_empty {
                    None
                } else {
                    // On a transparent retry after a MALFORMED_FUNCTION_CALL, force
                    // function-calling mode `ANY` so the model must emit a real
                    // functionCall instead of the Python-style pseudo-code that
                    // triggered the malformed turn (the proven recovery for this
                    // failure mode). Normal turns use `AUTO`.
                    Some(GeminiToolConfig {
                        function_calling_config: GeminiFunctionCallingConfig {
                            mode: if force_function_call { "ANY" } else { "AUTO" },
                        },
                    })
                },
                session_id: resume_session_id
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string),
            },
        };

        let contents_value = serde_json::to_value(&request.request.contents).unwrap_or(Value::Null);
        let content_items = contents_value.as_array().cloned().unwrap_or_default();
        let system_value = request
            .request
            .system_instruction
            .as_ref()
            .and_then(|system| serde_json::to_value(system).ok());
        let tools_value = request
            .request
            .tools
            .as_ref()
            .and_then(|tools| serde_json::to_value(tools).ok());
        let payload = json!({
            "model": &request.model,
            "contents": contents_value,
            "system_instruction": system_value.as_ref(),
            "tools": tools_value.as_ref(),
            "tool_config": &request.request.tool_config,
        });
        super::fingerprint::log_provider_canonical_input(
            "antigravity",
            model,
            "gemini_generate_content",
            &payload,
            &content_items,
            system_value.as_ref(),
            tools_value.as_ref(),
            request.request.tools.as_ref().map(|tools| tools.len()),
            &[
                (
                    "session_id_present",
                    request.request.session_id.is_some().to_string(),
                ),
                ("project_present", (!request.project.is_empty()).to_string()),
            ],
        );

        let response = self
            .client
            .post(GENERATE_CONTENT_API_URL)
            .bearer_auth(&tokens.access_token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::USER_AGENT, antigravity_user_agent())
            .header("x-goog-api-client", X_GOOG_API_CLIENT)
            .header(
                "x-goog-request-params",
                format!("project={}", request.project),
            )
            .header("x-goog-client-metadata", client_metadata_header())
            .json(&request)
            .send()
            .await
            .context("Failed to send Antigravity generateContent request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = crate::util::http_error_body(response, "HTTP error").await;
            anyhow::bail!(
                "Antigravity generateContent failed (HTTP {}): {}",
                status,
                body.trim()
            );
        }

        response
            .json()
            .await
            .context("Failed to decode Antigravity generateContent response")
    }
}

impl Default for AntigravityProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for AntigravityProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let model = self
            .model
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let messages = messages.to_vec();
        let tools = _tools.to_vec();
        let system = system.to_string();
        let resume_session_id = _resume_session_id.map(str::to_string);
        let provider = self.clone();
        let (tx, rx) = mpsc::channel::<Result<crate::message::StreamEvent>>(100);

        tokio::spawn(async move {
            let _ = tx
                .send(Ok(StreamEvent::ConnectionType {
                    connection: "https".to_string(),
                }))
                .await;
            let _ = tx
                .send(Ok(StreamEvent::ConnectionPhase {
                    phase: ConnectionPhase::Authenticating,
                }))
                .await;
            let _ = tx
                .send(Ok(StreamEvent::ConnectionPhase {
                    phase: ConnectionPhase::WaitingForResponse,
                }))
                .await;
            let response = match provider
                .generate_content(
                    &model,
                    &messages,
                    &tools,
                    &system,
                    resume_session_id.as_deref(),
                    false,
                )
                .await
            {
                Ok(response) => response,
                Err(err) => {
                    let _ = tx.send(Err(err)).await;
                    return;
                }
            };
            // Gemini-3 thinking models intermittently return an empty
            // `MALFORMED_FUNCTION_CALL` turn (pseudo-code instead of a clean
            // functionCall). It is transient, so transparently re-request a few
            // times before surfacing it; this turns a frequent hard failure into a
            // near-always-successful turn without the agent loop seeing the blip.
            // The retries force function-calling mode `ANY` so the model must emit
            // a real functionCall rather than the pseudo-code that failed.
            let mut response = response;
            let mut malformed_retries = 0u8;
            const MAX_MALFORMED_RETRIES: u8 = 2;
            while is_retryable_empty_turn(&response) && malformed_retries < MAX_MALFORMED_RETRIES {
                malformed_retries += 1;
                match provider
                    .generate_content(
                        &model,
                        &messages,
                        &tools,
                        &system,
                        resume_session_id.as_deref(),
                        true,
                    )
                    .await
                {
                    Ok(retried) => response = retried,
                    Err(err) => {
                        let _ = tx.send(Err(err)).await;
                        return;
                    }
                }
            }
            let _ = tx
                .send(Ok(StreamEvent::ConnectionPhase {
                    phase: ConnectionPhase::Streaming,
                }))
                .await;
            if let Some(usage) = response
                .response
                .as_ref()
                .and_then(|r| r.usage_metadata.as_ref())
            {
                let _ = tx
                    .send(Ok(StreamEvent::TokenUsage {
                        input_tokens: usage.prompt_token_count,
                        output_tokens: usage.candidates_token_count,
                        cache_read_input_tokens: usage.cached_content_token_count,
                        cache_creation_input_tokens: None,
                    }))
                    .await;
            }
            let Some(candidate) = response
                .response
                .and_then(|r| r.candidates)
                .and_then(|mut c| c.drain(..).next())
            else {
                let _ = tx
                    .send(Err(anyhow::anyhow!(
                        "Antigravity returned no candidates for generateContent"
                    )))
                    .await;
                return;
            };
            // Track whether this candidate produced any usable output (text or a
            // tool call). Gemini-3 thinking models intermittently emit Python-style
            // pseudo-code instead of a clean functionCall and finish with
            // `MALFORMED_FUNCTION_CALL` (or a bare `OTHER`) and empty content. If we
            // silently end the turn the agent loop looks like it stalled with no
            // answer, so we surface an actionable error below instead.
            let mut produced_output = false;
            if let Some(content) = candidate.content {
                // Gemini 3 attaches a `thoughtSignature` to function-call parts
                // (and occasionally to a standalone preceding part). Emit tool
                // calls through the standard ToolUseStart/End path so jcode
                // drives the multi-turn loop, and replay the signature via a
                // dedicated ToolUseSignature event so it can be persisted on the
                // ToolUse block and resent on later turns (required by the
                // Cloud Code backend, which rejects function calls missing it).
                let mut pending_signature: Option<String> = None;
                for part in content.parts {
                    let part_signature = part
                        .thought_signature
                        .as_ref()
                        .filter(|sig| !sig.is_empty())
                        .cloned();
                    if let Some(text) = part.text.filter(|text| !text.is_empty()) {
                        produced_output = true;
                        let _ = tx.send(Ok(StreamEvent::TextDelta(text))).await;
                    }
                    if let Some(function_call) = part.function_call {
                        produced_output = true;
                        let signature = part_signature.clone().or_else(|| pending_signature.take());
                        let raw_call_id = function_call
                            .id
                            .clone()
                            .unwrap_or_else(|| Uuid::new_v4().to_string());
                        let call_id = crate::message::sanitize_tool_id(&raw_call_id);
                        let _ = tx
                            .send(Ok(StreamEvent::ToolUseStart {
                                id: call_id,
                                name: function_call.name,
                            }))
                            .await;
                        let _ = tx
                            .send(Ok(StreamEvent::ToolInputDelta(
                                function_call.args.to_string(),
                            )))
                            .await;
                        let _ = tx.send(Ok(StreamEvent::ToolUseEnd)).await;
                        if let Some(signature) = signature {
                            let _ = tx.send(Ok(StreamEvent::ToolUseSignature(signature))).await;
                        }
                    } else if let Some(signature) = part_signature {
                        // Standalone signature part; remember it for the next
                        // function call in this candidate.
                        pending_signature = Some(signature);
                    }
                }
                // A thought signature that was never consumed by a following
                // function call (e.g. a pure-text reasoning turn) is still an
                // opaque reasoning signal. Surface it as a ThinkingSignatureDelta
                // rather than dropping it, so reasoning-aware consumers (and the
                // provider-doctor reasoning probe) can see the model reasoned.
                if let Some(signature) = pending_signature.take() {
                    let _ = tx
                        .send(Ok(StreamEvent::ThinkingSignatureDelta(signature)))
                        .await;
                }
            }

            // An abnormal finish (typically Gemini-3's intermittent
            // `MALFORMED_FUNCTION_CALL`, where the model writes pseudo-code rather
            // than a valid functionCall) that yielded no text and no tool call is a
            // dead turn: surface it as a retryable error instead of a silent empty
            // `MessageEnd` that looks like the agent gave up. `STOP`/`MAX_TOKENS`
            // are normal terminal reasons and are left to flow through as usual.
            if !produced_output {
                let abnormal = candidate
                    .finish_reason
                    .as_deref()
                    .map(|reason| {
                        !matches!(
                            reason.to_ascii_uppercase().as_str(),
                            "STOP" | "MAX_TOKENS" | "FINISH_REASON_UNSPECIFIED" | ""
                        )
                    })
                    .unwrap_or(false);
                if abnormal {
                    let reason = candidate.finish_reason.as_deref().unwrap_or("unknown");
                    let detail = candidate
                        .finish_message
                        .as_deref()
                        .filter(|msg| !msg.trim().is_empty())
                        .map(|msg| format!(": {}", crate::util::truncate_str(msg.trim(), 300)))
                        .unwrap_or_default();
                    let _ = tx
                        .send(Err(anyhow::anyhow!(
                            "Antigravity returned no usable output (finish_reason={reason}){detail}"
                        )))
                        .await;
                    return;
                }
            }

            let _ = tx
                .send(Ok(StreamEvent::MessageEnd {
                    stop_reason: candidate.finish_reason.clone(),
                }))
                .await;
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
                    api_method: "https".to_string(),
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
                api_method: "https".to_string(),
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
                    crate::bus::Bus::global().publish_models_updated();
                }
            });
        }
    }

    async fn prefetch_models(&self) -> Result<()> {
        match self.fetch_available_models().await {
            Ok(snapshot) => {
                if !snapshot.models.is_empty() {
                    crate::logging::info(&format!(
                        "Discovered Antigravity models: {}{}",
                        snapshot
                            .models
                            .iter()
                            .map(|model| model.id.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                        snapshot
                            .default_model_id
                            .as_deref()
                            .map(|id| format!(" (default: {id})"))
                            .unwrap_or_default()
                    ));
                    Self::persist_catalog(&snapshot);
                    if let Some(default_model_id) = snapshot.default_model_id.clone() {
                        *self
                            .backend_default_model
                            .write()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                            Some(default_model_id);
                    }
                    *self
                        .fetched_catalog
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = snapshot.models;
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
            client: self.client.clone(),
            model: Arc::new(RwLock::new(self.model())),
            fetched_catalog: self.fetched_catalog.clone(),
            backend_default_model: self.backend_default_model.clone(),
        })
    }
}

#[cfg(test)]
#[path = "antigravity_tests.rs"]
mod antigravity_tests;
