use super::{EventStream, Provider};
use crate::auth::gemini as gemini_auth;
use crate::message::{ConnectionPhase, Message, Role, StreamEvent, ToolDefinition};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
pub use jcode_provider_gemini::{
    AVAILABLE_MODELS, CODE_ASSIST_API_VERSION, CODE_ASSIST_ENDPOINT, ClientMetadata,
    CodeAssistGenerateRequest, CodeAssistGenerateResponse, DEFAULT_MODEL, GeminiCandidate,
    GeminiContent, GeminiFunctionCall, GeminiFunctionCallingConfig, GeminiFunctionDeclaration,
    GeminiFunctionResponse, GeminiPart, GeminiPromptFeedback, GeminiRuntimeState, GeminiTool,
    GeminiToolConfig, GeminiUsageMetadata, GeminiUserTier, IneligibleTier, InlineData,
    LoadCodeAssistRequest, LoadCodeAssistResponse, LongRunningOperationResponse,
    OnboardUserRequest, OnboardUserResponse, ProjectRef, USER_TIER_FREE,
    VertexGenerateContentRequest, VertexGenerateContentResponse, choose_onboard_tier,
    client_metadata, extract_gemini_model_ids, gemini_fallback_models,
    google_cloud_project_from_env, ineligible_or_project_error, is_gemini_model_id,
    load_code_assist_request, merge_gemini_model_lists, validate_load_code_assist_response,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct PersistedCatalog {
    models: Vec<String>,
    fetched_at_rfc3339: String,
}

pub struct GeminiProvider {
    client: reqwest::Client,
    model: Arc<RwLock<String>>,
    state: Arc<Mutex<Option<GeminiRuntimeState>>>,
    fetched_models: Arc<RwLock<Vec<String>>>,
}

impl GeminiProvider {
    fn persisted_catalog_path() -> Result<std::path::PathBuf> {
        Ok(crate::storage::app_config_dir()?.join("gemini_models_cache.json"))
    }

    fn load_persisted_catalog() -> Option<PersistedCatalog> {
        let path = Self::persisted_catalog_path().ok()?;
        crate::storage::read_json(&path)
            .ok()
            .filter(|catalog: &PersistedCatalog| !catalog.models.is_empty())
    }

    fn persist_catalog(models: &[String]) {
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
                "Failed to persist Gemini model catalog {}: {}",
                path.display(),
                error
            ));
        }
    }

    fn seed_cached_catalog(&self) {
        if let Some(catalog) = Self::load_persisted_catalog()
            && let Ok(mut models) = self.fetched_models.write()
        {
            *models = catalog.models;
        }
    }

    pub fn new() -> Self {
        let model = std::env::var("JCODE_GEMINI_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
        let provider = Self {
            client: gemini_http_client(),
            model: Arc::new(RwLock::new(model)),
            state: Arc::new(Mutex::new(None)),
            fetched_models: Arc::new(RwLock::new(Vec::new())),
        };
        provider.seed_cached_catalog();
        provider
    }

    fn base_url() -> String {
        let endpoint = std::env::var("CODE_ASSIST_ENDPOINT")
            .unwrap_or_else(|_| CODE_ASSIST_ENDPOINT.to_string());
        let version = std::env::var("CODE_ASSIST_API_VERSION")
            .unwrap_or_else(|_| CODE_ASSIST_API_VERSION.to_string());
        format!("{endpoint}/{version}")
    }

    async fn ensure_state(&self) -> Result<GeminiRuntimeState> {
        let mut guard = self.state.lock().await;
        if let Some(state) = guard.clone() {
            return Ok(state);
        }

        let state = self.setup_runtime_state().await?;
        *guard = Some(state.clone());
        Ok(state)
    }

    async fn setup_runtime_state(&self) -> Result<GeminiRuntimeState> {
        let project_id_env = google_cloud_project_from_env();
        let metadata = client_metadata(project_id_env.clone());
        let load_req = load_code_assist_request(project_id_env.clone(), metadata.clone());
        let load_res: LoadCodeAssistResponse =
            match self.post_json("loadCodeAssist", &load_req).await {
                Ok(response) => response,
                Err(err) if is_vpc_sc_error(&err) => LoadCodeAssistResponse {
                    current_tier: Some(GeminiUserTier {
                        id: Some("standard-tier".to_string()),
                        name: None,
                        is_default: None,
                    }),
                    allowed_tiers: None,
                    ineligible_tiers: None,
                    cloudaicompanion_project: None,
                    paid_tier: None,
                },
                Err(err) => {
                    return Err(err)
                        .context("Gemini Code Assist setup failed during loadCodeAssist");
                }
            };

        validate_load_code_assist_response(&load_res)?;

        let project_id = if load_res.current_tier.is_some() {
            if let Some(project_id) = load_res.cloudaicompanion_project.clone() {
                project_id
            } else if let Some(project_id) = project_id_env.clone() {
                project_id
            } else {
                return Err(ineligible_or_project_error(&load_res));
            }
        } else {
            let tier = choose_onboard_tier(&load_res);
            let onboard_req = if tier.id.as_deref() == Some(USER_TIER_FREE) {
                OnboardUserRequest {
                    tier_id: tier.id.clone(),
                    cloudaicompanion_project: None,
                    metadata: Some(ClientMetadata {
                        ide_type: "IDE_UNSPECIFIED",
                        platform: "PLATFORM_UNSPECIFIED",
                        plugin_type: "GEMINI",
                        duet_project: None,
                    }),
                }
            } else {
                OnboardUserRequest {
                    tier_id: tier.id.clone(),
                    cloudaicompanion_project: project_id_env.clone(),
                    metadata: Some(metadata.clone()),
                }
            };
            let mut lro: LongRunningOperationResponse = self
                .post_json("onboardUser", &onboard_req)
                .await
                .context("Gemini Code Assist onboarding failed")?;
            while !lro.done.unwrap_or(false) {
                let op_name = lro.name.clone().ok_or_else(|| {
                    anyhow::anyhow!("Gemini onboarding returned no operation name")
                })?;
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                lro = self
                    .get_operation(&op_name)
                    .await
                    .context("Gemini onboarding polling failed")?;
            }

            if let Some(project_id) = lro
                .response
                .and_then(|response| response.cloudaicompanion_project)
                .and_then(|project| project.id)
            {
                project_id
            } else if let Some(project_id) = project_id_env.clone() {
                project_id
            } else {
                return Err(ineligible_or_project_error(&load_res));
            }
        };

        Ok(GeminiRuntimeState {
            project_id,
            session_id: Uuid::new_v4().to_string(),
        })
    }

    async fn refresh_available_models(&self) -> Result<Vec<String>> {
        let project_id_env = google_cloud_project_from_env();
        let load_req = load_code_assist_request(
            project_id_env.clone(),
            client_metadata(project_id_env.clone()),
        );
        let response: Value = match self.post_json("loadCodeAssist", &load_req).await {
            Ok(response) => response,
            Err(err) if is_vpc_sc_error(&err) => Value::Null,
            Err(err) => {
                return Err(err).context("Gemini model discovery failed during loadCodeAssist");
            }
        };

        let models = extract_gemini_model_ids(&response);
        if !models.is_empty() {
            crate::logging::info(&format!(
                "Discovered Gemini Code Assist models: {}",
                models.join(", ")
            ));
            if let Ok(mut guard) = self.fetched_models.write() {
                *guard = models.clone();
            }
            Self::persist_catalog(&models);
        }
        Ok(models)
    }

    async fn post_json<T: DeserializeOwned>(
        &self,
        method: &str,
        body: &impl Serialize,
    ) -> Result<T> {
        let tokens = gemini_auth::load_or_refresh_tokens().await?;
        let url = format!("{}:{method}", Self::base_url());
        let body_value =
            serde_json::to_value(body).context("Failed to serialize Gemini request body")?;
        let mut last_error: Option<anyhow::Error> = None;
        let mut resp = None;
        for attempt in 0..2 {
            let client = if attempt == 0 {
                self.client.clone()
            } else {
                gemini_http_client()
            };
            match client
                .post(&url)
                .bearer_auth(&tokens.access_token)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .json(&body_value)
                .send()
                .await
            {
                Ok(response) => {
                    resp = Some(response);
                    break;
                }
                Err(err) if attempt == 0 && is_transient_gemini_transport_error(&err) => {
                    last_error = Some(err.into());
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                Err(err) => {
                    return Err(err).with_context(|| format!("Gemini request to {} failed", url));
                }
            }
        }
        let resp = match resp {
            Some(resp) => resp,
            None => {
                let err = last_error.unwrap_or_else(|| anyhow::anyhow!("Gemini request failed"));
                return Err(err).with_context(|| format!("Gemini request to {} failed", url));
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "Gemini request {} failed (HTTP {}): {}",
                method,
                status,
                body.trim()
            );
        }

        resp.json()
            .await
            .with_context(|| format!("Failed to parse Gemini {} response", method))
    }

    async fn get_operation<T: DeserializeOwned>(&self, name: &str) -> Result<T> {
        let tokens = gemini_auth::load_or_refresh_tokens().await?;
        let url = format!("{}/{}", Self::base_url(), name.trim_start_matches('/'));
        let mut last_error: Option<anyhow::Error> = None;
        let mut resp = None;
        for attempt in 0..2 {
            let client = if attempt == 0 {
                self.client.clone()
            } else {
                gemini_http_client()
            };
            match client
                .get(&url)
                .bearer_auth(&tokens.access_token)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .send()
                .await
            {
                Ok(response) => {
                    resp = Some(response);
                    break;
                }
                Err(err) if attempt == 0 && is_transient_gemini_transport_error(&err) => {
                    last_error = Some(err.into());
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                Err(err) => {
                    return Err(err).with_context(|| format!("Gemini request to {} failed", url));
                }
            }
        }
        let resp = match resp {
            Some(resp) => resp,
            None => {
                let err =
                    last_error.unwrap_or_else(|| anyhow::anyhow!("Gemini operation lookup failed"));
                return Err(err).with_context(|| format!("Gemini request to {} failed", url));
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "Gemini operation lookup failed (HTTP {}): {}",
                status,
                body.trim()
            );
        }

        resp.json()
            .await
            .context("Failed to parse Gemini operation response")
    }

    async fn generate_content(
        &self,
        state: &GeminiRuntimeState,
        model: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<CodeAssistGenerateResponse> {
        let request = CodeAssistGenerateRequest {
            model: model.to_string(),
            project: state.project_id.clone(),
            user_prompt_id: Uuid::new_v4().to_string(),
            request: VertexGenerateContentRequest {
                contents: build_contents(messages),
                system_instruction: build_system_instruction(system),
                tools: build_tools(tools),
                tool_config: if tools.is_empty() {
                    None
                } else {
                    Some(GeminiToolConfig {
                        function_calling_config: GeminiFunctionCallingConfig { mode: "AUTO" },
                    })
                },
                session_id: Some(
                    resume_session_id
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or(&state.session_id)
                        .to_string(),
                ),
            },
        };

        self.post_json("generateContent", &request)
            .await
            .context("Gemini generateContent failed")
    }
}

impl Default for GeminiProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for GeminiProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let model = self.model();
        let messages = messages.to_vec();
        let tools = tools.to_vec();
        let system = system.to_string();
        let resume_session_id = resume_session_id.map(|value| value.to_string());
        let state_cache = self.state.clone();
        let provider = self.clone();
        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);

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

            let state = {
                let provider = GeminiProvider {
                    client: provider.client.clone(),
                    model: provider.model.clone(),
                    state: state_cache.clone(),
                    fetched_models: provider.fetched_models.clone(),
                };
                match provider.ensure_state().await {
                    Ok(state) => state,
                    Err(err) => {
                        let _ = tx.send(Err(err)).await;
                        return;
                    }
                }
            };

            let _ = tx
                .send(Ok(StreamEvent::SessionId(
                    resume_session_id
                        .clone()
                        .unwrap_or_else(|| state.session_id.clone()),
                )))
                .await;
            let _ = tx
                .send(Ok(StreamEvent::ConnectionPhase {
                    phase: ConnectionPhase::Connecting,
                }))
                .await;
            let _ = tx
                .send(Ok(StreamEvent::ConnectionPhase {
                    phase: ConnectionPhase::WaitingForResponse,
                }))
                .await;

            let response = match provider
                .generate_content(
                    &state,
                    &model,
                    &messages,
                    &tools,
                    &system,
                    resume_session_id.as_deref(),
                )
                .await
            {
                Ok(response) => response,
                Err(err) if is_gemini_model_not_found_error(&err) => {
                    let mut fallback_response = None;
                    let mut last_err = err;
                    for fallback_model in gemini_fallback_models(&model) {
                        crate::logging::warn(&format!(
                            "Gemini model '{}' was not found; retrying with fallback '{}'",
                            model, fallback_model
                        ));
                        match provider
                            .generate_content(
                                &state,
                                fallback_model,
                                &messages,
                                &tools,
                                &system,
                                resume_session_id.as_deref(),
                            )
                            .await
                        {
                            Ok(response) => {
                                let _ = provider.set_model(fallback_model);
                                fallback_response = Some(response);
                                break;
                            }
                            Err(err) => {
                                last_err = err;
                            }
                        }
                    }

                    match fallback_response {
                        Some(response) => response,
                        None => {
                            let _ = tx.send(Err(last_err)).await;
                            return;
                        }
                    }
                }
                Err(err) => {
                    let _ = tx.send(Err(err)).await;
                    return;
                }
            };

            let _ = tx
                .send(Ok(StreamEvent::ConnectionPhase {
                    phase: ConnectionPhase::Streaming,
                }))
                .await;

            if let Some(usage) = response
                .response
                .as_ref()
                .and_then(|response| response.usage_metadata.as_ref())
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

            let response_body = response.response;

            let candidate = response_body
                .as_ref()
                .and_then(|response| response.candidates.as_ref())
                .and_then(|candidates| candidates.first())
                .cloned();

            if candidate.is_none() {
                if let Some(feedback) = response_body
                    .as_ref()
                    .and_then(|response| response.prompt_feedback.as_ref())
                {
                    let block_reason = feedback.block_reason.as_deref().unwrap_or("unspecified");
                    let detail = feedback
                        .block_reason_message
                        .as_deref()
                        .filter(|msg| !msg.trim().is_empty())
                        .map(|msg| format!(": {}", msg.trim()))
                        .unwrap_or_default();
                    let _ = tx
                        .send(Err(anyhow::anyhow!(
                            "Gemini blocked the prompt ({}){}",
                            block_reason,
                            detail
                        )))
                        .await;
                    return;
                }

                let _ = tx
                    .send(Err(anyhow::anyhow!(
                        "Gemini returned no candidates for generateContent"
                    )))
                    .await;
                return;
            }

            let mut stop_reason = None;
            if let Some(candidate) = candidate {
                stop_reason = candidate
                    .finish_reason
                    .clone()
                    .map(|reason| reason.to_lowercase());
                if candidate.content.is_none()
                    && matches!(
                        candidate.finish_reason.as_deref(),
                        Some("SAFETY" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "RECITATION")
                    )
                {
                    let reason = candidate.finish_reason.as_deref().unwrap_or("unknown");
                    let detail = candidate
                        .finish_message
                        .as_deref()
                        .filter(|msg| !msg.trim().is_empty())
                        .map(|msg| format!(": {}", msg.trim()))
                        .unwrap_or_default();
                    let _ = tx
                        .send(Err(anyhow::anyhow!(
                            "Gemini stopped without content ({}){}",
                            reason,
                            detail
                        )))
                        .await;
                    return;
                }
                if let Some(content) = candidate.content {
                    for part in content.parts {
                        if let Some(text) = part.text
                            && !text.is_empty()
                        {
                            let _ = tx.send(Ok(StreamEvent::TextDelta(text))).await;
                        }
                        if let Some(function_call) = part.function_call {
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
                        }
                    }
                }
            }

            let _ = tx.send(Ok(StreamEvent::MessageEnd { stop_reason })).await;
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &'static str {
        "gemini"
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
            anyhow::bail!("Gemini model cannot be empty");
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
        let discovered = self
            .fetched_models
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        if discovered.is_empty() {
            return merge_gemini_model_lists(vec![self.model()]);
        }

        merge_gemini_model_lists(
            discovered
                .into_iter()
                .chain(std::iter::once(self.model()))
                .collect(),
        )
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        self.available_models_display()
    }

    async fn prefetch_models(&self) -> Result<()> {
        let _ = self.refresh_available_models().await?;
        Ok(())
    }

    fn supports_compaction(&self) -> bool {
        false
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            client: self.client.clone(),
            model: Arc::new(RwLock::new(self.model())),
            state: self.state.clone(),
            fetched_models: self.fetched_models.clone(),
        })
    }

    async fn invalidate_credentials(&self) {
        let mut guard = self.state.lock().await;
        *guard = None;
    }
}

impl Clone for GeminiProvider {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            model: self.model.clone(),
            state: self.state.clone(),
            fetched_models: self.fetched_models.clone(),
        }
    }
}

fn is_vpc_sc_error(err: &anyhow::Error) -> bool {
    err.to_string().contains("SECURITY_POLICY_VIOLATED")
}

fn gemini_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("jcode/1.0 (gemini)")
        .http1_only()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(0)
        .tcp_keepalive(Some(Duration::from_secs(30)))
        .build()
        .unwrap_or_else(|_| crate::provider::shared_http_client())
}

fn is_transient_gemini_transport_error(err: &reqwest::Error) -> bool {
    let lower = err.to_string().to_ascii_lowercase();
    err.is_connect()
        || err.is_timeout()
        || lower.contains("unexpected eof")
        || lower.contains("connection reset")
        || lower.contains("broken pipe")
        || lower.contains("tls handshake eof")
}

fn is_gemini_model_not_found_error(err: &anyhow::Error) -> bool {
    let lower = format!("{err:#}").to_ascii_lowercase();
    lower.contains("http 404")
        || lower.contains("\"status\": \"not_found\"")
        || lower.contains("requested entity was not found")
}

fn build_system_instruction(system: &str) -> Option<GeminiContent> {
    let trimmed = system.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(GeminiContent {
            role: "user".to_string(),
            parts: vec![GeminiPart {
                text: Some(trimmed.to_string()),
                ..Default::default()
            }],
        })
    }
}

fn build_contents(messages: &[Message]) -> Vec<GeminiContent> {
    messages
        .iter()
        .filter_map(|message| {
            let role = match message.role {
                Role::User => "user",
                Role::Assistant => "model",
            };
            let mut parts = Vec::new();
            for block in &message.content {
                match block {
                    crate::message::ContentBlock::Text { text, .. } => {
                        parts.push(GeminiPart {
                            text: Some(text.clone()),
                            ..Default::default()
                        });
                    }
                    crate::message::ContentBlock::Reasoning { .. } => {}
                    crate::message::ContentBlock::ToolUse { id, name, input } => {
                        parts.push(GeminiPart {
                            function_call: Some(GeminiFunctionCall {
                                name: name.clone(),
                                args: input.clone(),
                                id: Some(id.clone()),
                            }),
                            ..Default::default()
                        });
                    }
                    crate::message::ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        parts.push(GeminiPart {
                            function_response: Some(GeminiFunctionResponse {
                                name: tool_name_from_tool_result(tool_use_id, messages),
                                response: if is_error.unwrap_or(false) {
                                    json!({ "error": content })
                                } else {
                                    json!({ "content": content })
                                },
                                id: Some(tool_use_id.clone()),
                            }),
                            ..Default::default()
                        });
                    }
                    crate::message::ContentBlock::Image { media_type, data } => {
                        parts.push(GeminiPart {
                            inline_data: Some(InlineData {
                                mime_type: media_type.clone(),
                                data: data.clone(),
                            }),
                            ..Default::default()
                        });
                    }
                    crate::message::ContentBlock::OpenAICompaction { .. } => {}
                }
            }
            if parts.is_empty() {
                None
            } else {
                Some(GeminiContent {
                    role: role.to_string(),
                    parts,
                })
            }
        })
        .collect()
}

fn tool_name_from_tool_result(tool_use_id: &str, messages: &[Message]) -> String {
    for message in messages.iter().rev() {
        for block in &message.content {
            if let crate::message::ContentBlock::ToolUse { id, name, .. } = block
                && id == tool_use_id
            {
                return name.clone();
            }
        }
    }
    "tool".to_string()
}

fn build_tools(tools: &[ToolDefinition]) -> Option<Vec<GeminiTool>> {
    if tools.is_empty() {
        return None;
    }

    Some(vec![GeminiTool {
        function_declarations: tools
            .iter()
            .map(|tool| GeminiFunctionDeclaration {
                name: tool.name.clone(),
                // Prompt-visible. Approximate token cost for this field:
                // tool.description_token_estimate().
                description: tool.description.clone(),
                parameters: gemini_compatible_schema(&tool.input_schema),
            })
            .collect(),
    }])
}

fn gemini_compatible_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                if key == "const" {
                    out.insert(
                        "enum".to_string(),
                        Value::Array(vec![gemini_compatible_schema(value)]),
                    );
                } else {
                    out.insert(key.clone(), gemini_compatible_schema(value));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(gemini_compatible_schema).collect()),
        _ => schema.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ContentBlock, Message, Role};
    use crate::provider::{EventStream, Provider};
    use crate::tool::Registry;
    use async_trait::async_trait;
    use std::sync::Arc;

    struct MockProvider;

    #[async_trait]
    impl Provider for MockProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> anyhow::Result<EventStream> {
            Err(anyhow::anyhow!(
                "Mock provider should not be used for streaming completions in Gemini tests"
            ))
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(MockProvider)
        }
    }

    #[test]
    fn available_models_include_gemini_defaults() {
        let provider = GeminiProvider::new();
        let models = provider.available_models();
        assert!(models.contains(&"gemini-3-pro-preview"));
        assert!(models.contains(&"gemini-3.1-pro-preview"));
        assert!(models.contains(&"gemini-2.5-pro"));
        assert!(models.contains(&"gemini-2.5-flash"));
    }

    #[test]
    fn set_model_accepts_gemini_models() {
        let provider = GeminiProvider::new();
        provider.set_model("gemini-2.5-flash").unwrap();
        assert_eq!(provider.model(), "gemini-2.5-flash");
    }

    #[test]
    fn detects_model_not_found_errors() {
        let err = anyhow::anyhow!(
            "Gemini request generateContent failed (HTTP 404 Not Found): {{\"error\":{{\"status\":\"NOT_FOUND\",\"message\":\"Requested entity was not found.\"}}}}"
        );
        assert!(is_gemini_model_not_found_error(&err));
    }

    #[test]
    fn fallback_models_skip_current_model() {
        assert_eq!(
            gemini_fallback_models("gemini-2.5-flash"),
            vec![
                "gemini-3.1-pro-preview",
                "gemini-3-pro-preview",
                "gemini-2.5-pro",
                "gemini-3-flash-preview",
                "gemini-2.0-flash",
            ]
        );
    }

    #[test]
    fn extract_gemini_model_ids_discovers_nested_models() {
        let response = json!({
            "routing": {
                "manual": {
                    "models": [
                        {"id": "gemini-3-pro-preview"},
                        {"name": "gemini-3.1-pro-preview"}
                    ]
                },
                "auto": ["gemini-3-flash-preview", "not-a-model"]
            }
        });

        assert_eq!(
            extract_gemini_model_ids(&response),
            vec![
                "gemini-3.1-pro-preview".to_string(),
                "gemini-3-pro-preview".to_string(),
                "gemini-3-flash-preview".to_string(),
            ]
        );
    }

    #[test]
    fn available_models_display_prefers_discovered_models_and_current_model() {
        let provider = GeminiProvider::new();
        provider.set_model("gemini-4-pro-preview").unwrap();
        *provider.fetched_models.write().unwrap() = vec![
            "gemini-3-flash-preview".to_string(),
            "gemini-3-pro-preview".to_string(),
        ];

        assert_eq!(
            provider.available_models_display(),
            vec![
                "gemini-3-pro-preview".to_string(),
                "gemini-3-flash-preview".to_string(),
                "gemini-4-pro-preview".to_string(),
            ]
        );
    }

    #[test]
    fn available_models_display_without_discovery_uses_current_model_only() {
        let provider = GeminiProvider::new();
        provider.set_model("gemini-4-pro-preview").unwrap();

        assert_eq!(
            provider.available_models_display(),
            vec!["gemini-4-pro-preview".to_string()]
        );
    }

    #[test]
    fn available_models_display_seeds_from_persisted_catalog() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("tempdir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        let path = GeminiProvider::persisted_catalog_path().expect("catalog path");
        crate::storage::write_json(
            &path,
            &PersistedCatalog {
                models: vec!["gemini-3-pro-preview".to_string()],
                fetched_at_rfc3339: chrono::Utc::now().to_rfc3339(),
            },
        )
        .expect("write persisted catalog");

        let provider = GeminiProvider::new();
        assert!(
            provider
                .available_models_display()
                .contains(&"gemini-3-pro-preview".to_string())
        );

        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn build_contents_preserves_tool_calls_and_results() {
        let messages = vec![
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "read".to_string(),
                    input: json!({"path":"README.md"}),
                }],
                timestamp: None,
                tool_duration_ms: None,
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call_1".to_string(),
                    content: "ok".to_string(),
                    is_error: None,
                }],
                timestamp: None,
                tool_duration_ms: None,
            },
        ];

        let contents = build_contents(&messages);
        assert_eq!(contents.len(), 2);
        assert_eq!(contents[0].role, "model");
        assert_eq!(contents[1].role, "user");
        assert_eq!(
            contents[0].parts[0].function_call.as_ref().unwrap().name,
            "read"
        );
        assert_eq!(
            contents[1].parts[0]
                .function_response
                .as_ref()
                .unwrap()
                .name,
            "read"
        );
    }

    #[test]
    fn build_tools_uses_function_declarations() {
        let defs = vec![ToolDefinition {
            name: "read".to_string(),
            description: "Read a file".to_string(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"}}}),
        }];

        let built = build_tools(&defs).unwrap();
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].function_declarations[0].name, "read");
    }

    fn schema_contains_key(schema: &Value, key: &str) -> bool {
        match schema {
            Value::Object(map) => {
                map.contains_key(key) || map.values().any(|value| schema_contains_key(value, key))
            }
            Value::Array(items) => items.iter().any(|value| schema_contains_key(value, key)),
            _ => false,
        }
    }

    #[test]
    fn build_tools_rewrites_const_for_gemini_schema_compatibility() {
        let defs = vec![ToolDefinition {
            name: "batch".to_string(),
            description: "Batch tools".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "tool_calls": {
                        "type": "array",
                        "items": {
                            "oneOf": [
                                {
                                    "type": "object",
                                    "properties": {
                                        "tool": { "type": "string", "const": "read" },
                                        "file_path": { "type": "string" }
                                    },
                                    "required": ["tool", "file_path"]
                                }
                            ]
                        }
                    }
                }
            }),
        }];

        let built = build_tools(&defs).expect("gemini tools");
        let parameters = &built[0].function_declarations[0].parameters;

        assert!(!schema_contains_key(parameters, "const"));
        assert_eq!(
            parameters["properties"]["tool_calls"]["items"]["oneOf"][0]["properties"]["tool"]["enum"],
            json!(["read"])
        );
    }

    #[tokio::test]
    async fn build_tools_from_registry_definitions_omits_const_keywords() {
        let provider: Arc<dyn Provider> = Arc::new(MockProvider);
        let registry = Registry::new(provider).await;
        let defs = registry.definitions(None).await;

        let built = build_tools(&defs).expect("gemini tools");
        let parameters = &built[0].function_declarations;

        assert!(!schema_contains_key(&json!(parameters), "const"));
    }

    #[test]
    fn parses_prompt_feedback_block_reason() {
        let response: VertexGenerateContentResponse = serde_json::from_value(json!({
            "promptFeedback": {
                "blockReason": "PROHIBITED_CONTENT",
                "blockReasonMessage": "Prompt violated policy"
            }
        }))
        .expect("parse prompt feedback");

        let feedback = response.prompt_feedback.expect("missing prompt feedback");
        assert_eq!(feedback.block_reason.as_deref(), Some("PROHIBITED_CONTENT"));
        assert_eq!(
            feedback.block_reason_message.as_deref(),
            Some("Prompt violated policy")
        );
    }

    #[test]
    fn parses_candidate_finish_message() {
        let response: VertexGenerateContentResponse = serde_json::from_value(json!({
            "candidates": [
                {
                    "finishReason": "SAFETY",
                    "finishMessage": "Response blocked by safety filters"
                }
            ]
        }))
        .expect("parse candidate");

        let candidate = response
            .candidates
            .expect("missing candidates")
            .into_iter()
            .next()
            .expect("missing first candidate");
        assert_eq!(candidate.finish_reason.as_deref(), Some("SAFETY"));
        assert_eq!(
            candidate.finish_message.as_deref(),
            Some("Response blocked by safety filters")
        );
    }
}
