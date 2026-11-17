//! Gemini provider runtime (Google Code Assist OAuth + official Developer API
//! key), moved out of `jcode-base` so provider edits compile only this crate
//! plus a binary relink instead of rebuilding the base -> app-core -> tui
//! spine. The binary's composition root registers [`GeminiProvider`] with
//! `jcode_base::provider::external` at startup.

mod catalog;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use jcode_base::auth::gemini as gemini_auth;
use jcode_message_types::{ConnectionPhase, Message, StreamEvent, ToolDefinition};
use jcode_provider_core::{EventStream, Provider};
pub use jcode_provider_gemini::{
    AVAILABLE_MODELS, CODE_ASSIST_API_VERSION, CODE_ASSIST_ENDPOINT, ClientMetadata,
    CodeAssistGenerateRequest, CodeAssistGenerateResponse, DEFAULT_MODEL, GEMINI_API_ENDPOINT,
    GEMINI_API_VERSION, GeminiCandidate, GeminiContent, GeminiFunctionCall,
    GeminiFunctionCallingConfig, GeminiFunctionDeclaration, GeminiFunctionResponse, GeminiPart,
    GeminiPromptFeedback, GeminiRuntimeState, GeminiTool, GeminiToolConfig, GeminiUsageMetadata,
    GeminiUserTier, IneligibleTier, InlineData, LoadCodeAssistRequest, LoadCodeAssistResponse,
    LongRunningOperationResponse, OnboardUserRequest, OnboardUserResponse, ProjectRef,
    USER_TIER_FREE, VertexGenerateContentRequest, VertexGenerateContentResponse, build_contents,
    build_system_instruction_with_tool_guard, build_tools, choose_onboard_tier, client_metadata,
    extract_gemini_model_ids, gemini_fallback_models, google_cloud_project_from_env,
    ineligible_or_project_error, is_gemini_model_id, load_code_assist_request,
    merge_gemini_model_lists, validate_load_code_assist_response,
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
    state: Arc<Mutex<Option<CachedGeminiState>>>,
    fetched_models: Arc<RwLock<Vec<String>>>,
}

/// How the Gemini provider authenticates to Google.
#[derive(Clone)]
enum GeminiAuthMode {
    /// OAuth Code Assist (cloudcode-pa, free tier). The default when no API key
    /// is configured.
    Oauth,
    /// Official Gemini Developer API key (Google AI Studio), sent as
    /// `x-goog-api-key` to `generativelanguage.googleapis.com`.
    ApiKey(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GeminiStateKey {
    api_key: bool,
    project: Option<String>,
}

#[derive(Clone)]
struct CachedGeminiState {
    key: GeminiStateKey,
    state: GeminiRuntimeState,
}

impl GeminiProvider {
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

    /// Base URL for the official Gemini Developer API (Google AI Studio).
    fn developer_api_base_url() -> String {
        let endpoint = std::env::var("GEMINI_API_ENDPOINT")
            .unwrap_or_else(|_| GEMINI_API_ENDPOINT.to_string());
        let version =
            std::env::var("GEMINI_API_VERSION").unwrap_or_else(|_| GEMINI_API_VERSION.to_string());
        format!(
            "{}/{}",
            endpoint.trim_end_matches('/'),
            version.trim_matches('/')
        )
    }

    /// Resolve the active authentication mode.
    ///
    /// An official Gemini Developer API key takes precedence over OAuth Code
    /// Assist credentials: it points at `generativelanguage.googleapis.com` with
    /// the key's own (often higher) quota, while OAuth uses the free
    /// cloudcode-pa tier. Set `JCODE_GEMINI_FORCE_OAUTH=1` to pin OAuth even when
    /// a key is present.
    fn auth_mode() -> GeminiAuthMode {
        if !gemini_auth::force_oauth()
            && let Some(api_key) = gemini_auth::api_key()
        {
            return GeminiAuthMode::ApiKey(api_key);
        }
        GeminiAuthMode::Oauth
    }

    async fn ensure_state(&self) -> Result<GeminiRuntimeState> {
        let key = Self::state_key();
        let mut guard = self.state.lock().await;
        if let Some(cached) = guard.as_ref()
            && cached.key == key
        {
            return Ok(cached.state.clone());
        }
        // The Developer API key path is stateless: there is no Code Assist
        // project/onboarding handshake, so synthesize a lightweight session.
        let state = if key.api_key {
            GeminiRuntimeState {
                project_id: String::new(),
                session_id: Uuid::new_v4().to_string(),
            }
        } else {
            self.setup_runtime_state().await?
        };
        *guard = Some(CachedGeminiState {
            key,
            state: state.clone(),
        });
        Ok(state)
    }

    fn state_key() -> GeminiStateKey {
        let api_key = matches!(Self::auth_mode(), GeminiAuthMode::ApiKey(_));
        GeminiStateKey {
            api_key,
            project: if api_key {
                None
            } else {
                gemini_auth::cloud_project()
            },
        }
    }

    async fn setup_runtime_state(&self) -> Result<GeminiRuntimeState> {
        let project_id_env = gemini_auth::cloud_project();
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

    /// Send a request with a single transient-error retry, transparently
    /// rebuilding the HTTP client on the second attempt. The `make` closure
    /// produces a fully-configured (auth + body) request builder for each try.
    async fn send_with_retry<F>(&self, make: F, url: &str) -> Result<reqwest::Response>
    where
        F: Fn(reqwest::Client) -> reqwest::RequestBuilder,
    {
        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 0..2 {
            let client = if attempt == 0 {
                self.client.clone()
            } else {
                gemini_http_client()
            };
            match make(client).send().await {
                Ok(response) => return Ok(response),
                Err(err) if attempt == 0 && is_transient_gemini_transport_error(&err) => {
                    last_error = Some(err.into());
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                Err(err) => {
                    return Err(err).with_context(|| format!("Gemini request to {} failed", url));
                }
            }
        }
        let err = last_error.unwrap_or_else(|| anyhow::anyhow!("Gemini request failed"));
        Err(err).with_context(|| format!("Gemini request to {} failed", url))
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
        // Code Assist enforces a short-window burst limiter that returns 429
        // RESOURCE_EXHAUSTED with "quota will reset after 0s" even when the
        // daily bucket has plenty left. Those clear within seconds, so retry
        // them with backoff instead of failing the turn. The same applies to
        // transient 5xx from Google's side ("Authentication backend
        // unavailable", "No capacity available"), which otherwise kill a
        // long session mid-turn.
        let mut attempt: u32 = 0;
        loop {
            let resp = self
                .send_with_retry(
                    |client| {
                        client
                            .post(&url)
                            .bearer_auth(&tokens.access_token)
                            .header(reqwest::header::CONTENT_TYPE, "application/json")
                            .json(&body_value)
                    },
                    &url,
                )
                .await?;

            if !resp.status().is_success() {
                let status = resp.status();
                let retry_after = jcode_provider_core::retry_after::retry_after(resp.headers());
                let body = jcode_base::util::http_error_body(resp, "HTTP error").await;
                if let Some(delay) = gemini_http_retry_delay(
                    method,
                    status,
                    &body,
                    attempt,
                    retry_after.map(|hint| hint.remaining()),
                ) {
                    jcode_base::logging::warn(&format!(
                        "Gemini {} hit transient HTTP {} (attempt {}/{}); retrying in {:?}",
                        method,
                        status.as_u16(),
                        attempt + 1,
                        MAX_HTTP_RETRIES,
                        delay
                    ));
                    attempt += 1;
                    tokio::time::sleep(delay).await;
                    continue;
                }
                return Err(jcode_provider_core::retry_after::error_with_retry_after(
                    format!(
                        "Gemini request {} failed (HTTP {}): {}",
                        method,
                        status,
                        body.trim()
                    ),
                    retry_after,
                ));
            }

            return resp
                .json()
                .await
                .with_context(|| format!("Failed to parse Gemini {} response", method));
        }
    }

    /// POST a JSON body to the official Gemini Developer API, authenticating
    /// with an `x-goog-api-key` header instead of an OAuth bearer token.
    async fn post_json_api_key<T: DeserializeOwned>(
        &self,
        url: &str,
        api_key: &str,
        body: &impl Serialize,
        label: &str,
    ) -> Result<T> {
        let body_value =
            serde_json::to_value(body).context("Failed to serialize Gemini request body")?;
        let resp = self
            .send_with_retry(
                |client| {
                    client
                        .post(url)
                        .header("x-goog-api-key", api_key)
                        .header(reqwest::header::CONTENT_TYPE, "application/json")
                        .json(&body_value)
                },
                url,
            )
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = jcode_base::util::http_error_body(resp, "HTTP error").await;
            anyhow::bail!(
                "Gemini request {} failed (HTTP {}): {}",
                label,
                status,
                body.trim()
            );
        }

        resp.json()
            .await
            .with_context(|| format!("Failed to parse Gemini {} response", label))
    }

    /// GET a JSON resource from the official Gemini Developer API using an
    /// `x-goog-api-key` header.
    async fn get_json_api_key<T: DeserializeOwned>(
        &self,
        url: &str,
        api_key: &str,
        label: &str,
    ) -> Result<T> {
        let resp = self
            .send_with_retry(
                |client| {
                    client
                        .get(url)
                        .header("x-goog-api-key", api_key)
                        .header(reqwest::header::CONTENT_TYPE, "application/json")
                },
                url,
            )
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = jcode_base::util::http_error_body(resp, "HTTP error").await;
            anyhow::bail!("Gemini {} failed (HTTP {}): {}", label, status, body.trim());
        }

        resp.json()
            .await
            .with_context(|| format!("Failed to parse Gemini {} response", label))
    }

    async fn get_operation<T: DeserializeOwned>(&self, name: &str) -> Result<T> {
        let tokens = gemini_auth::load_or_refresh_tokens().await?;
        let url = format!("{}/{}", Self::base_url(), name.trim_start_matches('/'));
        let resp = self
            .send_with_retry(
                |client| {
                    client
                        .get(&url)
                        .bearer_auth(&tokens.access_token)
                        .header(reqwest::header::CONTENT_TYPE, "application/json")
                },
                &url,
            )
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = jcode_base::util::http_error_body(resp, "HTTP error").await;
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

    /// Recover from a tool-schema rejection by learning what
    /// `generateContent` refused and re-sending the turn without it.
    ///
    /// jcode advertises every tool on every request, so one construct the
    /// endpoint dislikes 400s the whole session rather than one tool. The
    /// historical fix was to append the keyword to a deny-list and ship a
    /// release (#754, #655); this recovers in the same turn instead, and
    /// `jcode-schema-dialect` remembers it so later requests never send it.
    ///
    /// Returns `None` when the error is not a recoverable schema rejection,
    /// so the caller falls through to its normal error handling. The quirk
    /// store reports a construct as newly-learned only once, which is what
    /// bounds this to a single retry per distinct construct.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors generate_content so the retry re-sends an identical turn"
    )]
    async fn retry_after_schema_rejection(
        &self,
        error: &str,
        state: &GeminiRuntimeState,
        model: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Option<Result<CodeAssistGenerateResponse>> {
        let dialect = &jcode_schema_dialect::registry::GEMINI;
        match jcode_schema_dialect::recover_from_error(error, dialect) {
            jcode_schema_dialect::RecoveryAction::NotSchemaRelated => None,
            jcode_schema_dialect::RecoveryAction::Unrecoverable { hint } => {
                jcode_base::logging::warn(&format!("Gemini tool-schema rejection: {hint}"));
                None
            }
            jcode_schema_dialect::RecoveryAction::RetryWithoutConstruct { description } => {
                jcode_base::logging::warn(&format!("Gemini {description}"));
                Some(
                    self.generate_content(state, model, messages, tools, system, resume_session_id)
                        .await,
                )
            }
        }
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
                system_instruction: build_system_instruction_with_tool_guard(
                    system,
                    !tools.is_empty(),
                ),
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

        log_canonical_input(model, &request);

        match Self::auth_mode() {
            GeminiAuthMode::ApiKey(api_key) => {
                // The Developer API consumes the inner generateContent body
                // directly (no Code Assist envelope) and returns the response
                // without the `{ response: ... }` wrapper, so adapt both sides.
                let mut inner = request.request;
                inner.session_id = None;
                let url = format!(
                    "{}/models/{}:generateContent",
                    Self::developer_api_base_url(),
                    model
                );
                let response: VertexGenerateContentResponse = self
                    .post_json_api_key(&url, &api_key, &inner, "generateContent")
                    .await
                    .context("Gemini generateContent failed")?;
                Ok(CodeAssistGenerateResponse {
                    trace_id: None,
                    response: Some(response),
                })
            }
            GeminiAuthMode::Oauth => self
                .post_json("generateContent", &request)
                .await
                .context("Gemini generateContent failed"),
        }
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
        "gemini",
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

impl Default for GeminiProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[path = "lib_provider_impl.rs"]
mod provider_impl;
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

const MAX_HTTP_RETRIES: u32 = 5;

fn gemini_http_retry_delay(
    method: &str,
    status: reqwest::StatusCode,
    body: &str,
    attempt: u32,
    retry_after: Option<Duration>,
) -> Option<Duration> {
    // Onboarding mutates account state. Do not replay it automatically when a
    // server error cannot tell us whether the first request completed.
    if attempt >= MAX_HTTP_RETRIES || !matches!(method, "generateContent" | "loadCodeAssist") {
        return None;
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let lower = body.to_ascii_lowercase();
        if [
            "daily",
            "per day",
            "per_day",
            "perday",
            "billing",
            "insufficient_quota",
            "quota_exhausted",
        ]
        .iter()
        .any(|reason| lower.contains(reason))
        {
            return None;
        }
    } else if !matches!(status.as_u16(), 500 | 502 | 503 | 504) {
        return None;
    }
    Some(jcode_provider_core::retry_after::retry_delay(
        attempt,
        1500,
        retry_after,
    ))
}

fn gemini_http_client() -> reqwest::Client {
    // Code Assist (cloudcode-pa) applies the subscription quota per client
    // identity. A/B tests with the same token, project and body showed
    // `jcode/1.0 (gemini)` throttled (429 RATE_LIMIT_EXCEEDED after 1-2
    // requests) while the official Gemini CLI UA ran unlimited. Present the
    // same identity the official CLI sends.
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "x-goog-api-client",
        reqwest::header::HeaderValue::from_static("gl-node/26.4.0"),
    );
    reqwest::Client::builder()
        .user_agent(gemini_user_agent())
        .default_headers(headers)
        .http1_only()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(0)
        .tcp_keepalive(Some(Duration::from_secs(30)))
        .build()
        .unwrap_or_else(|_| jcode_provider_core::shared_http_client())
}

/// User-Agent matching the official Gemini CLI. Override with
/// `JCODE_GEMINI_USER_AGENT` if Google changes the accepted format.
fn gemini_user_agent() -> String {
    if let Ok(ua) = std::env::var("JCODE_GEMINI_USER_AGENT")
        && !ua.trim().is_empty()
    {
        return ua;
    }
    let os = std::env::consts::OS;
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    format!("GeminiCLI-tui/0.57.0/{DEFAULT_MODEL} ({os}; {arch}; terminal)")
}

fn is_transient_gemini_transport_error(err: &reqwest::Error) -> bool {
    // Delegate to the shared transport classifier so Gemini recognizes the
    // same transient faults as every other provider (close_notify, connection
    // reset, DNS, HTTP/2 stream errors, ...), plus reqwest's structured
    // connect/timeout flags which don't always surface in the message text.
    err.is_connect()
        || err.is_timeout()
        || jcode_provider_core::is_transient_transport_error(&err.to_string())
}

fn is_gemini_model_not_found_error(err: &anyhow::Error) -> bool {
    let lower = format!("{err:#}").to_ascii_lowercase();
    lower.contains("http 404")
        || lower.contains("\"status\": \"not_found\"")
        || lower.contains("requested entity was not found")
}

#[cfg(test)]
#[path = "gemini_tests.rs"]
mod tests;

#[cfg(test)]
mod catalog_tests;
