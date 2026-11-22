//! Antigravity provider runtime (Google Cloud Code backend multiplexing
//! Gemini/Claude/gpt-oss upstreams), moved out of `jcode-base` so provider
//! edits compile only this crate plus a binary relink instead of rebuilding
//! the base -> app-core -> tui spine. The binary's composition root registers
//! [`AntigravityProvider`] with `jcode_base::provider::external` at startup.

use anyhow::{Context, Result};
use async_trait::async_trait;
use jcode_base::auth::antigravity as antigravity_auth;
use jcode_message_types::{ConnectionPhase, Message, StreamEvent, ToolDefinition};
use jcode_provider_antigravity::{
    AVAILABLE_MODELS, CatalogModel, CatalogSnapshot, DEFAULT_FALLBACK_MODEL, PersistedCatalog,
    X_GOOG_API_CLIENT, antigravity_compatible_schema, antigravity_user_agent, catalog_is_stale,
    catalog_model_detail, client_metadata_header, merge_antigravity_model_ids,
    remap_unsupported_model, stream_generate_content_api_url,
};
#[cfg(test)]
use jcode_provider_antigravity::{
    flatten_schema_combiners, generate_content_api_url, is_retryable_empty_turn, metadata_platform,
    model_is_claude, model_is_gemini, strip_numeric_schema_bounds,
};
use jcode_provider_core::{EventStream, Provider};
#[cfg(test)]
use jcode_provider_gemini::CodeAssistGenerateResponse;
use jcode_provider_gemini::stream::{
    GeminiSseEnvelope, GeminiSseParser, GeminiStreamAccumulator, GeminiStreamFinish,
    decode_sse_chunk,
};
use jcode_provider_gemini::{
    CodeAssistGenerateRequest, GeminiFunctionCallingConfig, GeminiThinkingRequestMode,
    GeminiToolConfig, VertexGenerateContentRequest, build_generation_config_for_thoughts_with_mode,
};
use serde_json::{Value, json};
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
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
    /// Whether Jcode's `swarm` orchestration sentinel is active. Antigravity
    /// encodes real reasoning effort in sibling model IDs (`-low`, `-medium`,
    /// `-high`, `-xhigh`), while `swarm` retains the sentinel in UI/session
    /// state and routes requests to the configured swarm root effort sibling.
    swarm_mode: Arc<RwLock<bool>>,
}

impl Clone for AntigravityProvider {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            model: self.model.clone(),
            fetched_catalog: self.fetched_catalog.clone(),
            backend_default_model: self.backend_default_model.clone(),
            swarm_mode: self.swarm_mode.clone(),
        }
    }
}

impl AntigravityProvider {
    fn load_persisted_catalog() -> Option<PersistedCatalog> {
        jcode_base::provider::antigravity::load_persisted_catalog()
    }

    fn persist_catalog(snapshot: &CatalogSnapshot) {
        jcode_base::provider::antigravity::persist_catalog(snapshot);
    }

    fn seed_cached_catalog(&self) {
        if let Some(catalog) = Self::load_persisted_catalog() {
            if catalog_is_stale(&catalog.fetched_at_rfc3339) {
                jcode_base::logging::info(
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
            client: jcode_provider_core::shared_http_client(),
            model: Arc::new(RwLock::new(model)),
            fetched_catalog: Arc::new(RwLock::new(Vec::new())),
            backend_default_model: Arc::new(RwLock::new(None)),
            swarm_mode: Arc::new(RwLock::new(false)),
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

    fn is_swarm_mode(&self) -> bool {
        *self
            .swarm_mode
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn catalog_effort_candidates(&self) -> Vec<String> {
        let catalog = self.fetched_catalog();
        if !catalog.is_empty() {
            return catalog
                .into_iter()
                .filter(|entry| entry.available)
                .map(|entry| entry.id)
                .collect();
        }
        AVAILABLE_MODELS
            .iter()
            .map(|id| (*id).to_string())
            .collect()
    }

    fn resolve_sibling_model_for_effort(
        &self,
        effective_model: &str,
        target_effort: &str,
    ) -> Option<String> {
        let (base, _) = jcode_provider_core::split_antigravity_model_effort(effective_model)?;
        self.catalog_effort_candidates()
            .into_iter()
            .find(|candidate| {
                jcode_provider_core::split_antigravity_model_effort(candidate).is_some_and(
                    |(candidate_base, effort)| {
                        candidate_base.eq_ignore_ascii_case(base)
                            && effort.eq_ignore_ascii_case(target_effort)
                    },
                )
            })
    }

    fn resolve_swarm_request_model(&self, effective_model: &str) -> String {
        let root_effort = jcode_base::config::config().agents.root_effort_for_swarm();
        let preference: &[&str] = match root_effort {
            "none" | "minimal" | "low" => &["low", "medium", "high", "xhigh"],
            "medium" => &["medium", "low", "high", "xhigh"],
            "high" => &["high", "medium", "xhigh", "low"],
            "xhigh" | "max" => &["xhigh", "high", "medium", "low"],
            _ => &["low", "medium", "high", "xhigh"],
        };
        for &candidate_effort in preference {
            if let Some(sibling) =
                self.resolve_sibling_model_for_effort(effective_model, candidate_effort)
            {
                return sibling;
            }
        }
        effective_model.to_string()
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

    /// Live catalog snapshot for the `/usage` report: per-model quota
    /// (`remaining_fraction_milli`) and reset times. Persists the warm catalog
    /// like the runtime prefetch so the rest of the process benefits.
    pub async fn fetch_catalog_snapshot_for_usage(&self) -> Result<CatalogSnapshot> {
        let snapshot = self.fetch_available_models().await?;
        if snapshot.models.is_empty() {
            anyhow::bail!("Antigravity model catalog returned no models");
        }
        Self::persist_catalog(&snapshot);
        Ok(snapshot)
    }

    /// Resolve `"default"` / empty to a concrete model id without applying
    /// request-time remaps or swarm-mode root overrides.
    fn effective_selected_model_for(&self, model: &str) -> String {
        let trimmed = model.trim();
        if !trimmed.is_empty() && trimmed != DEFAULT_MODEL {
            return trimmed.to_string();
        }

        let catalog = self.fetched_catalog();
        if catalog
            .iter()
            .any(|model| model.available && model.id == DEFAULT_FALLBACK_MODEL)
        {
            return DEFAULT_FALLBACK_MODEL.to_string();
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

    /// Resolve a requested model id into a real backend model id. The literal
    /// alias `"default"` (and the empty string) is rejected by the
    /// `generateContent` endpoint with HTTP 404, so it is mapped to the
    /// backend-advertised default, then to a known-good catalog model, and
    /// finally to a hardcoded fallback.
    ///
    /// Note: this only resolves the `"default"` alias / empty input. An
    /// explicit model id from the user is honoured verbatim, except for ids
    /// the backend advertises but cannot actually service, which are remapped
    /// to an equivalent working id via [`remap_unsupported_model`], and when
    /// Jcode's `swarm` sentinel is active, which routes requests to the
    /// configured swarm root effort sibling.
    fn resolve_model_for_request(&self, model: &str) -> String {
        let effective = self.effective_selected_model_for(model);
        let selected = if self.is_swarm_mode() {
            self.resolve_swarm_request_model(&effective)
        } else {
            effective
        };
        remap_unsupported_model(&selected).to_string()
    }

    async fn fetch_available_models(&self) -> Result<CatalogSnapshot> {
        jcode_base::provider::antigravity::fetch_catalog_snapshot(&self.client).await
    }

    /// Recover from a tool-schema rejection by learning what the backend
    /// refused and re-sending the turn without it.
    ///
    /// Because jcode advertises every tool on every request, one construct the
    /// backend dislikes 400s the whole session rather than one tool, and the
    /// historical fix has been to ship a new deny-list entry (#754, #687, #543,
    /// #446). `jcode-schema-dialect` instead parses the construct out of the
    /// error, persists it, and normalization strips it from then on, so the
    /// user's next turn works instead of their next upgrade.
    ///
    /// Returns `None` when the error is not a recoverable schema rejection, so
    /// the caller falls through to its other error handling. The quirk store
    /// only reports a construct as newly-learned once, which is what bounds
    /// this to a single retry per distinct construct.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors stream_generate_content's explicit per-request settings so the retry re-sends an identical turn"
    )]
    async fn retry_after_schema_rejection(
        &self,
        error: &str,
        model: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
        signature_policy: jcode_provider_gemini::SignaturePolicy,
        thinking_mode: GeminiThinkingRequestMode,
    ) -> Option<Result<reqwest::Response>> {
        let resolved = self.resolve_model_for_request(model);
        let dialect = jcode_provider_antigravity::antigravity_dialect(&resolved);
        match jcode_schema_dialect::recover_from_error(error, dialect) {
            jcode_schema_dialect::RecoveryAction::NotSchemaRelated => None,
            jcode_schema_dialect::RecoveryAction::Unrecoverable { hint } => {
                jcode_base::logging::warn(&format!("Antigravity tool-schema rejection: {hint}"));
                None
            }
            jcode_schema_dialect::RecoveryAction::RetryWithoutConstruct { description } => {
                jcode_base::logging::warn(&format!("Antigravity {description}"));
                Some(
                    self.stream_generate_content(
                        model,
                        messages,
                        tools,
                        system,
                        resume_session_id,
                        false,
                        signature_policy,
                        thinking_mode,
                    )
                    .await,
                )
            }
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the Code Assist call threads explicit per-request settings, including signature and thought policies, without hidden state"
    )]
    async fn stream_generate_content(
        &self,
        model: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
        force_function_call: bool,
        signature_policy: jcode_provider_gemini::SignaturePolicy,
        thinking_mode: GeminiThinkingRequestMode,
    ) -> Result<reqwest::Response> {
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
                if let Err(error) = antigravity_auth::save_tokens(&tokens) {
                    jcode_base::logging::warn(&format!(
                        "Cannot persist discovered Antigravity project: {error}"
                    ));
                }
                project_id
            }
        };
        let resolved_model = self.resolve_model_for_request(model);
        let tools_is_empty = tools.is_empty();
        let mut tools = jcode_provider_gemini::build_tools(tools);
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
        let generation_config =
            build_generation_config_for_thoughts_with_mode(&resolved_model, thinking_mode);
        let request = CodeAssistGenerateRequest {
            model: resolved_model,
            project,
            user_prompt_id: Uuid::new_v4().to_string(),
            request: VertexGenerateContentRequest {
                contents: jcode_provider_gemini::build_contents_with_signature_policy(
                    messages,
                    signature_policy,
                ),
                system_instruction: jcode_provider_gemini::build_system_instruction_with_tool_guard(
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
                generation_config,
                session_id: resume_session_id
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string),
            },
        };

        log_canonical_input(model, &request);

        let response = self
            .client
            .post(stream_generate_content_api_url())
            .bearer_auth(&tokens.access_token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "text/event-stream")
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
            .context("Failed to send Antigravity streamGenerateContent request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = jcode_base::util::http_error_body(response, "HTTP error").await;
            anyhow::bail!(
                "Antigravity streamGenerateContent failed (HTTP {}): {}",
                status,
                body.trim()
            );
        }

        Ok(response)
    }
}

fn canonical_payload(request: &CodeAssistGenerateRequest) -> Value {
    let mut payload = json!({
        "model": &request.model,
        "tool_config": &request.request.tool_config,
    });
    let Value::Object(object) = &mut payload else {
        return Value::Null;
    };
    object.insert(
        "contents".into(),
        serde_json::to_value(&request.request.contents).unwrap_or(Value::Null),
    );
    object.insert(
        "system_instruction".into(),
        request
            .request
            .system_instruction
            .as_ref()
            .and_then(|value| serde_json::to_value(value).ok())
            .unwrap_or(Value::Null),
    );
    object.insert(
        "tools".into(),
        request
            .request
            .tools
            .as_ref()
            .and_then(|value| serde_json::to_value(value).ok())
            .unwrap_or(Value::Null),
    );
    payload
}

// Keep diagnostic JSON trees in this synchronous frame, never across HTTP awaits.
fn log_canonical_input(model: &str, request: &CodeAssistGenerateRequest) {
    let payload = canonical_payload(request);
    let items = payload["contents"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    jcode_provider_core::fingerprint::log_provider_canonical_input(
        "antigravity",
        model,
        "gemini_generate_content",
        &payload,
        items,
        payload
            .get("system_instruction")
            .filter(|value| !value.is_null()),
        payload.get("tools").filter(|value| !value.is_null()),
        request.request.tools.as_ref().map(Vec::len),
        &[
            (
                "session_id_present",
                request.request.session_id.is_some().to_string(),
            ),
            ("project_present", (!request.project.is_empty()).to_string()),
        ],
    );
}

#[cfg(test)]
mod canonical_input_tests {
    use super::*;
    use jcode_provider_gemini::{GeminiContent, GeminiFunctionDeclaration, GeminiPart, GeminiTool};

    #[test]
    fn canonical_payload_preserves_exact_schema_hashes_nulls_and_counts() {
        for variant in 0..3 {
            let content = GeminiContent {
                role: "user".into(),
                parts: vec![GeminiPart {
                    text: Some("hello 雪".into()),
                    ..Default::default()
                }],
            };
            let request = CodeAssistGenerateRequest {
                model: "fixture-model".into(),
                project: "project".into(),
                user_prompt_id: "volatile".into(),
                request: VertexGenerateContentRequest {
                    contents: vec![content.clone()],
                    system_instruction: (variant > 0).then(|| content.clone()),
                    tools: match variant {
                        0 => None,
                        1 => Some(vec![]),
                        _ => Some(vec![GeminiTool {
                            function_declarations: vec![GeminiFunctionDeclaration {
                                name: "lookup".into(),
                                description: "find".into(),
                                parameters: json!({"type": "object", "properties": {"q": {"type": "string"}}}),
                            }],
                        }]),
                    },
                    tool_config: (variant > 0).then_some(GeminiToolConfig {
                        function_calling_config: GeminiFunctionCallingConfig { mode: "AUTO" },
                    }),
                    generation_config: None,
                    session_id: Some("session".into()),
                },
            };
            let original_wire = serde_json::to_vec(&request).unwrap();
            let contents = serde_json::to_value(&request.request.contents).unwrap();
            let system = request
                .request
                .system_instruction
                .as_ref()
                .map(|value| serde_json::to_value(value).unwrap());
            let tools = request
                .request
                .tools
                .as_ref()
                .map(|value| serde_json::to_value(value).unwrap());
            let legacy = json!({"model": request.model, "contents": contents, "system_instruction": system.as_ref(), "tools": tools.as_ref(), "tool_config": request.request.tool_config});
            let payload = canonical_payload(&request);
            assert_eq!(
                serde_json::to_vec(&payload).unwrap(),
                serde_json::to_vec(&legacy).unwrap()
            );
            assert_eq!(payload.as_object().unwrap().len(), 5);
            assert!(!payload.to_string().contains("volatile"));
            assert_eq!(payload["contents"].as_array().unwrap().len(), 1);
            assert_eq!(payload["system_instruction"].is_null(), variant == 0);
            assert_eq!(payload["tools"].is_null(), variant == 0);
            assert_eq!(
                request.request.tools.as_ref().map(Vec::len),
                match variant {
                    0 => None,
                    1 => Some(0),
                    _ => Some(1),
                }
            );
            let hash = jcode_provider_core::fingerprint::stable_hash_json;
            assert_eq!(hash(&payload), hash(&legacy));
            assert_eq!(hash(&payload["contents"]), hash(&contents));
            assert_eq!(
                payload
                    .get("system_instruction")
                    .filter(|value| !value.is_null())
                    .map(hash),
                system.as_ref().map(hash)
            );
            assert_eq!(
                payload
                    .get("tools")
                    .filter(|value| !value.is_null())
                    .map(hash),
                tools.as_ref().map(hash)
            );
            log_canonical_input("fixture-model", &request);
            assert_eq!(serde_json::to_vec(&request).unwrap(), original_wire);
        }
    }
}

impl Default for AntigravityProvider {
    fn default() -> Self {
        Self::new()
    }
}

fn ensure_account_model(model: &str, ids: &[String]) -> Result<()> {
    anyhow::ensure!(
        ids.iter().any(|id| id == model),
        "Antigravity does not offer '{model}' for this account. Use /model to choose an available route. No generation request was sent."
    );
    Ok(())
}

async fn consume_antigravity_sse_stream(
    response: reqwest::Response,
    tx: &mpsc::Sender<Result<StreamEvent>>,
) -> Result<GeminiStreamFinish> {
    let mut byte_stream = response.bytes_stream();
    let mut utf8 = jcode_base::util::Utf8StreamDecoder::new();
    let mut sse = GeminiSseParser::new();
    let mut acc = GeminiStreamAccumulator::new("Antigravity", false, true);
    let mut next_call_id = || Uuid::new_v4().to_string();

    while let Some(chunk_res) = byte_stream.next().await {
        let bytes = chunk_res.context("Failed reading Antigravity SSE stream")?;
        let text = utf8.decode(&bytes);
        for data in sse.push(&text) {
            let Some(resp) = decode_sse_chunk(GeminiSseEnvelope::CodeAssist, &data)
                .context("Failed to decode Antigravity SSE chunk")?
            else {
                continue;
            };
            let result = acc.push_response(resp, &mut next_call_id);
            for event in result.events {
                if tx.send(Ok(event)).await.is_err() {
                    return Ok(GeminiStreamFinish::Completed { events: Vec::new() });
                }
            }
            if result.pseudo_tool_call_detected {
                return Ok(GeminiStreamFinish::PseudoToolCall { events: Vec::new() });
            }
        }
    }

    let trailing = utf8.flush();
    for data in sse.push(&trailing).into_iter().chain(sse.finish()) {
        let Some(resp) = decode_sse_chunk(GeminiSseEnvelope::CodeAssist, &data)
            .context("Failed to decode trailing Antigravity SSE chunk")?
        else {
            continue;
        };
        let result = acc.push_response(resp, &mut next_call_id);
        for event in result.events {
            if tx.send(Ok(event)).await.is_err() {
                return Ok(GeminiStreamFinish::Completed { events: Vec::new() });
            }
        }
        if result.pseudo_tool_call_detected {
            return Ok(GeminiStreamFinish::PseudoToolCall { events: Vec::new() });
        }
    }

    Ok(acc.finish())
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
        // A saved session or manual route can name a model from another
        // provider. Confirm cache misses against this account before sending
        // user content. A stale cache alone must not reject a new launch.
        if model != DEFAULT_MODEL
            && !model.trim().is_empty()
            && !self
                .fetched_catalog()
                .iter()
                .any(|entry| entry.id == model && entry.available)
        {
            let ids = self.fetch_live_model_ids_for_doctor().await.context(
                "Could not verify the selected Antigravity model against this account's catalog",
            )?;
            ensure_account_model(&model, &ids)?;
        }
        let tools = _tools.to_vec();
        let system = system.to_string();
        let resume_session_id = _resume_session_id.map(str::to_string);
        let provider = self.clone();
        let (tx, rx) = mpsc::channel::<Result<jcode_message_types::StreamEvent>>(100);

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
            // Gemini-3 function calls carry an opaque `thoughtSignature` that the
            // Cloud Code backend requires on replay. `build_contents` carries the
            // most recent real signature forward, but a history where *nothing*
            // was ever signed (imported/pre-signature sessions, or synthesized
            // calls from a route that never emits signatures) has none to carry,
            // and 400s on every single turn with no way out (issues #482, #518).
            // Self-heal the way the Anthropic runtime does for rejected reasoning
            // fields: on that specific error, retry once with tool calls
            // downgraded to plain text. Content is preserved, the turn completes,
            // and the model re-signs its new calls.
            let mut signature_policy = jcode_provider_gemini::SignaturePolicy::ReplayCarriedForward;
            let mut thinking_mode = GeminiThinkingRequestMode::Full;
            let mut force_function_call = false;
            let mut malformed_retries = 0u8;
            let mut pseudo_retried = false;
            const MAX_MALFORMED_RETRIES: u8 = 2;
            let mut emitted_streaming_phase = false;

            loop {
                let response = if !force_function_call {
                    match provider
                        .stream_generate_content(
                            &model,
                            &messages,
                            &tools,
                            &system,
                            resume_session_id.as_deref(),
                            false,
                            signature_policy,
                            thinking_mode,
                        )
                        .await
                    {
                        Ok(response) => response,
                        Err(err) => {
                            let err_str = format!("{err:#}");
                            let resolved_for_downgrade = provider.resolve_model_for_request(&model);
                            if let Some(next_mode) =
                                thinking_mode.downgrade_for_error(&resolved_for_downgrade, &err_str)
                            {
                                jcode_base::logging::warn(&format!(
                                    "Antigravity rejected thinkingConfig ({thinking_mode:?}); retrying with {next_mode:?}"
                                ));
                                thinking_mode = next_mode;
                                continue;
                            } else if let Some(retried) = provider
                                .retry_after_schema_rejection(
                                    &err_str,
                                    &model,
                                    &messages,
                                    &tools,
                                    &system,
                                    resume_session_id.as_deref(),
                                    signature_policy,
                                    thinking_mode,
                                )
                                .await
                            {
                                match retried {
                                    Ok(response) => response,
                                    Err(retry_err) => {
                                        let _ = tx.send(Err(retry_err)).await;
                                        return;
                                    }
                                }
                            } else if !jcode_provider_gemini::is_missing_thought_signature_error(
                                &err_str,
                            ) {
                                let _ = tx.send(Err(err)).await;
                                return;
                            } else {
                                jcode_base::logging::warn(
                                    "Antigravity rejected unsigned function calls; retrying with tool calls downgraded to text",
                                );
                                signature_policy =
                                    jcode_provider_gemini::SignaturePolicy::DowngradeToolCallsToText;
                                match provider
                                    .stream_generate_content(
                                        &model,
                                        &messages,
                                        &tools,
                                        &system,
                                        resume_session_id.as_deref(),
                                        false,
                                        signature_policy,
                                        thinking_mode,
                                    )
                                    .await
                                {
                                    Ok(response) => response,
                                    Err(retry_err) => {
                                        let _ = tx.send(Err(retry_err)).await;
                                        return;
                                    }
                                }
                            }
                        }
                    }
                } else {
                    match provider
                        .stream_generate_content(
                            &model,
                            &messages,
                            &tools,
                            &system,
                            resume_session_id.as_deref(),
                            true,
                            signature_policy,
                            thinking_mode,
                        )
                        .await
                    {
                        Ok(response) => response,
                        Err(err) => {
                            let _ = tx.send(Err(err)).await;
                            return;
                        }
                    }
                };

                if !emitted_streaming_phase {
                    emitted_streaming_phase = true;
                    let _ = tx
                        .send(Ok(StreamEvent::ConnectionPhase {
                            phase: ConnectionPhase::Streaming,
                        }))
                        .await;
                }

                let finish = match consume_antigravity_sse_stream(response, &tx).await {
                    Ok(finish) => finish,
                    Err(err) => {
                        let _ = tx.send(Err(err)).await;
                        return;
                    }
                };

                match finish {
                    GeminiStreamFinish::Completed { events } => {
                        for event in events {
                            let _ = tx.send(Ok(event)).await;
                        }
                        return;
                    }
                    GeminiStreamFinish::RetryableEmpty {
                        finish_reason,
                        finish_message,
                        events,
                    } => {
                        for event in events {
                            let _ = tx.send(Ok(event)).await;
                        }
                        if malformed_retries < MAX_MALFORMED_RETRIES {
                            malformed_retries += 1;
                            force_function_call = true;
                            continue;
                        }
                        let detail = finish_message
                            .as_deref()
                            .filter(|msg| !msg.trim().is_empty())
                            .map(|msg| {
                                format!(": {}", jcode_base::util::truncate_str(msg.trim(), 300))
                            })
                            .unwrap_or_default();
                        let _ = tx
                            .send(Err(anyhow::anyhow!(
                                "Antigravity returned no usable output (finish_reason={finish_reason}){detail}"
                            )))
                            .await;
                        return;
                    }
                    GeminiStreamFinish::PseudoToolCall { events } => {
                        for event in events {
                            let _ = tx.send(Ok(event)).await;
                        }
                        if !pseudo_retried {
                            pseudo_retried = true;
                            force_function_call = true;
                            continue;
                        }
                        let _ = tx
                            .send(Err(anyhow::anyhow!(
                                "Antigravity returned a textual pseudo-tool call after a forced native-call retry; start a new turn or choose another model"
                            )))
                            .await;
                        return;
                    }
                    GeminiStreamFinish::Error { message, events } => {
                        for event in events {
                            let _ = tx.send(Ok(event)).await;
                        }
                        let _ = tx.send(Err(anyhow::anyhow!(message))).await;
                        return;
                    }
                }
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

    fn supports_image_input(&self) -> bool {
        true
    }

    fn set_model(&self, model: &str) -> Result<()> {
        // `--provider antigravity` uses this runtime directly, so session
        // restore hands it the routing spec `antigravity:<model>` rather than a
        // bare id. See `strip_own_model_prefix`: keeping the prefix made every
        // resumed turn 404.
        let trimmed = jcode_provider_core::strip_own_model_prefix(model, "antigravity:");
        if trimmed.is_empty() {
            anyhow::bail!("Antigravity model cannot be empty");
        }
        *self
            .model
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = trimmed.to_string();
        *self
            .swarm_mode
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
        Ok(())
    }

    fn reasoning_effort(&self) -> Option<String> {
        if self.available_efforts().is_empty() {
            return None;
        }
        if self.is_swarm_mode() {
            return Some(jcode_base::prompt::SWARM_EFFORT.to_string());
        }
        let effective = self.effective_selected_model_for(&self.model());
        jcode_provider_core::split_antigravity_model_effort(&effective)
            .map(|(_, effort)| effort.to_string())
    }

    fn set_reasoning_effort(&self, effort: &str) -> Result<()> {
        let requested = effort.trim().to_ascii_lowercase();
        let effective = self.effective_selected_model_for(&self.model());
        let available = self.available_efforts();
        if available.is_empty() {
            anyhow::bail!(
                "Antigravity model '{}' does not support reasoning effort switching",
                effective
            );
        }
        if requested.is_empty() {
            *self
                .swarm_mode
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
            return Ok(());
        }
        if jcode_base::prompt::is_swarm_effort(&requested) {
            *self
                .swarm_mode
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
            return Ok(());
        }
        if !available.iter().any(|candidate| *candidate == requested) {
            let levels = available
                .into_iter()
                .filter(|candidate| !jcode_base::prompt::is_swarm_effort(candidate))
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!(
                "Invalid reasoning effort '{}' for Antigravity model '{}'. Valid values: {}, {}",
                requested,
                effective,
                levels,
                jcode_base::prompt::SWARM_EFFORT
            );
        }
        let target_model = self
            .resolve_sibling_model_for_effort(&effective, &requested)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No Antigravity model variant available for '{}' at effort '{}'",
                    effective,
                    requested
                )
            })?;
        *self
            .model
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = target_model;
        *self
            .swarm_mode
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
        Ok(())
    }

    fn available_efforts(&self) -> Vec<&'static str> {
        let effective = self.effective_selected_model_for(&self.model());
        let candidates = self.catalog_effort_candidates();
        jcode_provider_core::antigravity_efforts_from_catalog(
            &effective,
            candidates.iter().map(String::as_str),
        )
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
        let catalog = self.fetched_catalog();
        if catalog.is_empty() {
            return self.available_models_display();
        }
        merge_antigravity_model_ids(
            catalog
                .into_iter()
                .filter(|model| model.available)
                .map(|model| model.id),
        )
    }

    fn model_routes(&self) -> Vec<jcode_provider_core::ModelRoute> {
        let catalog = self.fetched_catalog();
        if !catalog.is_empty() {
            return catalog
                .into_iter()
                .map(|model| jcode_provider_core::ModelRoute {
                    model: model.id.clone(),
                    provider: "Antigravity".to_string(),
                    api_method: "https".to_string(),
                    available: model.available,
                    detail: catalog_model_detail(&model),
                    usage: None,
                    cheapness: None,
                })
                .collect();
        }

        self.available_models_display()
            .into_iter()
            .map(|model| jcode_provider_core::ModelRoute {
                model,
                provider: "Antigravity".to_string(),
                api_method: "https".to_string(),
                available: true,
                detail: "fallback catalog".to_string(),
                usage: None,
                cheapness: None,
            })
            .collect()
    }

    fn on_auth_changed(&self) {
        let provider = self.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if provider.prefetch_models().await.is_ok() {
                    jcode_base::bus::Bus::global().publish_models_updated();
                }
            });
        }
    }

    async fn prefetch_models(&self) -> Result<()> {
        match self.fetch_available_models().await {
            Ok(snapshot) => {
                if !snapshot.models.is_empty() {
                    jcode_base::logging::info(&format!(
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
                jcode_base::logging::warn(&format!(
                    "Antigravity model catalog refresh failed; keeping fallback list: {}",
                    err
                ));
                return Err(err);
            }
        }

        Ok(())
    }

    fn context_window(&self) -> usize {
        let selected = self.model();
        let resolved = self.resolve_model_for_request(&selected);
        let catalog = self.fetched_catalog();
        // Use this route's limit and resolve aliases just as inference does.
        let limit_for = |id: &str| {
            catalog
                .iter()
                .find(|entry| entry.id == id)
                .and_then(|entry| entry.max_tokens)
                .and_then(|limit| usize::try_from(limit).ok())
                .filter(|limit| *limit > 0)
        };
        limit_for(&resolved)
            .or_else(|| limit_for(&selected))
            .or_else(|| {
                jcode_provider_core::models::context_limit_for_model_with_provider(
                    &resolved,
                    Some("antigravity"),
                )
            })
            .unwrap_or(jcode_provider_core::models::DEFAULT_CONTEXT_LIMIT)
    }

    fn supports_compaction(&self) -> bool {
        // No native server-side compaction exists for this provider, so jcode's
        // own summary compaction is the only thing standing between a long
        // session and a hard context-limit rejection. Returning `false` here
        // disabled the entire compaction block in `Agent::messages_for_provider`
        // — including the emergency hard-compact and payload truncation at the
        // critical threshold — leaving these sessions with no safety net at all.
        true
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            client: self.client.clone(),
            model: Arc::new(RwLock::new(self.model())),
            fetched_catalog: self.fetched_catalog.clone(),
            backend_default_model: self.backend_default_model.clone(),
            swarm_mode: Arc::new(RwLock::new(self.is_swarm_mode())),
        })
    }
}

#[cfg(test)]
#[path = "antigravity_tests.rs"]
mod antigravity_tests;
