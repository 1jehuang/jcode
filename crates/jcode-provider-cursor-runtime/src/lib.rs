//! Cursor provider runtime (direct ChatService HTTP/2 streaming), moved out
//! of `jcode-base` so provider edits compile only this crate plus a binary
//! relink instead of rebuilding the base -> app-core -> tui spine. The
//! binary's composition root registers [`CursorCliProvider`] with
//! `jcode_base::provider::external` at startup.
//!
//! The pure model-catalog data (`AVAILABLE_MODELS`, `is_known_model`) stays in
//! `jcode_base::provider::cursor` because base's model-routing logic needs it
//! without a runtime.

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use jcode_base::auth::cursor as cursor_auth;
use jcode_base::provider::cursor::{AVAILABLE_MODELS, DEFAULT_MODEL};
use jcode_message_types::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use jcode_provider_core::{EventStream, Provider};
use serde::Deserialize;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

mod agent_transport;

const MODELS_API_URL: &str = "https://api.cursor.com/v0/models";
const MAX_PROMPT_CHARS: usize = 120_000;

fn build_cli_prompt(system: &str, messages: &[Message]) -> String {
    let mut out = String::new();

    if !system.trim().is_empty() {
        out.push_str("System:\n");
        out.push_str(system.trim());
        out.push_str("\n\n");
    }

    out.push_str("Conversation:\n");

    for message in messages {
        let role = match message.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
        };
        out.push_str(role);
        out.push_str(":\n");

        for block in &message.content {
            match block {
                ContentBlock::Text { text, .. } => {
                    out.push_str(text);
                    out.push('\n');
                }
                ContentBlock::Reasoning { .. }
                | ContentBlock::ReasoningTrace { .. }
                | ContentBlock::AnthropicThinking { .. }
                | ContentBlock::OpenAIReasoning { .. } => {}
                ContentBlock::ToolUse { name, input, .. } => {
                    out.push_str("[tool_use ");
                    out.push_str(name);
                    out.push_str(" input=");
                    out.push_str(&input.to_string());
                    out.push_str("]\n");
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    out.push_str("[tool_result ");
                    out.push_str(tool_use_id);
                    out.push_str(" is_error=");
                    out.push_str(if is_error.unwrap_or(false) {
                        "true"
                    } else {
                        "false"
                    });
                    out.push_str("]\n");
                    out.push_str(content);
                    out.push('\n');
                }
                ContentBlock::Image { .. } => {
                    out.push_str("[image]\n");
                }
                ContentBlock::OpenAICompaction { .. } => {
                    out.push_str("[openai native compaction]\n");
                }
            }
        }
        out.push('\n');
    }

    out.push_str("Assistant:\n");

    if out.chars().count() <= MAX_PROMPT_CHARS {
        return out;
    }

    let mut kept = out.chars().rev().take(MAX_PROMPT_CHARS).collect::<Vec<_>>();
    kept.reverse();
    let tail: String = kept.into_iter().collect();
    format!(
        "[Earlier conversation truncated to fit prompt limits]\n\n{}",
        tail
    )
}

#[derive(Debug, Deserialize)]
struct CursorModelsResponse {
    #[serde(default)]
    models: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PersistedCatalog {
    models: Vec<String>,
    fetched_at_rfc3339: String,
}

fn merge_cursor_models(dynamic: &[String], current: &str) -> Vec<String> {
    let mut merged = Vec::new();

    for model in dynamic {
        let trimmed = model.trim();
        if !trimmed.is_empty() && !merged.iter().any(|known| known == trimmed) {
            merged.push(trimmed.to_string());
        }
    }

    for model in AVAILABLE_MODELS {
        let trimmed = model.trim();
        if !trimmed.is_empty() && !merged.iter().any(|known| known == trimmed) {
            merged.push(trimmed.to_string());
        }
    }

    let current = current.trim();
    if !current.is_empty() && !merged.iter().any(|known| known == current) {
        merged.push(current.to_string());
    }

    merged
}

async fn fetch_available_models(client: &reqwest::Client, api_key: &str) -> Result<Vec<String>> {
    let response = client
        .get(MODELS_API_URL)
        .basic_auth(api_key, Some(""))
        .send()
        .await
        .context("Failed to fetch Cursor model catalog")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = jcode_base::util::http_error_body(response, "HTTP error").await;
        anyhow::bail!(
            "Cursor model catalog request failed ({}): {}",
            status,
            body.trim()
        );
    }

    let parsed: CursorModelsResponse = response
        .json()
        .await
        .context("Failed to decode Cursor model catalog response")?;
    Ok(parsed
        .models
        .into_iter()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect())
}

fn runtime_cursor_api_key() -> Option<String> {
    jcode_base::auth::cursor::load_api_key().ok()
}

pub struct CursorCliProvider {
    client: reqwest::Client,
    model: Arc<RwLock<String>>,
    fetched_models: Arc<RwLock<Vec<String>>>,
}

impl CursorCliProvider {
    fn persisted_catalog_path() -> Result<std::path::PathBuf> {
        Ok(jcode_base::storage::app_config_dir()?.join("cursor_models_cache.json"))
    }

    fn load_persisted_catalog() -> Option<PersistedCatalog> {
        let path = Self::persisted_catalog_path().ok()?;
        jcode_base::storage::read_json(&path)
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
        if let Err(error) = jcode_base::storage::write_json(&path, &payload) {
            jcode_base::logging::warn(&format!(
                "Failed to persist Cursor model catalog {}: {}",
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
        let model = std::env::var("JCODE_CURSOR_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
        let provider = Self {
            client: jcode_provider_core::shared_http_client(),
            model: Arc::new(RwLock::new(model)),
            fetched_models: Arc::new(RwLock::new(Vec::new())),
        };
        provider.seed_cached_catalog();
        provider
    }
}

impl Default for CursorCliProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for CursorCliProvider {
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
        let prompt_items = vec![Value::String(prompt.clone())];
        let system_value = (!system.trim().is_empty()).then(|| Value::String(system.to_string()));
        let payload = json!({
            "model": &model,
            "system": system_value.as_ref(),
            "prompt": &prompt,
        });
        jcode_provider_core::fingerprint::log_provider_canonical_input(
            "cursor",
            &model,
            "cursor_cli_prompt",
            &payload,
            &prompt_items,
            system_value.as_ref(),
            None,
            Some(0),
            &[
                ("logical_message_count", messages.len().to_string()),
                ("ignored_tool_count", _tools.len().to_string()),
            ],
        );
        let client = self.client.clone();
        let (tx, rx) = mpsc::channel::<Result<jcode_message_types::StreamEvent>>(100);

        tokio::spawn(async move {
            let result = run_native_text_command(client, tx.clone(), &prompt, &model).await;

            if let Err(err) = result {
                let _ = tx.send(Err(err)).await;
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &'static str {
        "cursor"
    }

    fn model(&self) -> String {
        self.model
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_model(&self, model: &str) -> Result<()> {
        // See `strip_own_model_prefix`: `--provider cursor` routes through this
        // runtime directly, so session restore hands it `cursor:<model>`.
        let trimmed = jcode_provider_core::strip_own_model_prefix(model, "cursor:");
        if trimmed.is_empty() {
            anyhow::bail!("Cursor model cannot be empty");
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

    fn available_models_for_switching(&self) -> Vec<String> {
        self.available_models_display()
    }

    fn available_models_display(&self) -> Vec<String> {
        let dynamic = self
            .fetched_models
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        merge_cursor_models(&dynamic, &self.model())
    }

    fn model_routes(&self) -> Vec<jcode_provider_core::ModelRoute> {
        self.available_models_display()
            .into_iter()
            .map(|model| jcode_provider_core::ModelRoute {
                model,
                provider: "Cursor".to_string(),
                api_method: "cursor".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            })
            .collect()
    }

    async fn prefetch_models(&self) -> Result<()> {
        let Some(api_key) = runtime_cursor_api_key() else {
            return Ok(());
        };

        match fetch_available_models(&self.client, &api_key).await {
            Ok(models) => {
                if !models.is_empty() {
                    jcode_base::logging::info(&format!(
                        "Discovered Cursor models: {}",
                        models.join(", ")
                    ));
                    Self::persist_catalog(&models);
                    *self
                        .fetched_models
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = models;
                }
            }
            Err(err) => {
                jcode_base::logging::warn(&format!(
                    "Cursor model catalog refresh failed; keeping fallback list: {}",
                    err
                ));
            }
        }

        Ok(())
    }

    fn handles_tools_internally(&self) -> bool {
        false
    }

    fn supports_compaction(&self) -> bool {
        false
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            client: self.client.clone(),
            model: Arc::new(RwLock::new(self.model())),
            fetched_models: self.fetched_models.clone(),
        })
    }
}

async fn run_native_text_command(
    client: reqwest::Client,
    tx: mpsc::Sender<Result<StreamEvent>>,
    prompt: &str,
    model: &str,
) -> Result<()> {
    let tokens = cursor_auth::resolve_direct_tokens(&client).await?;

    // The current Cursor agent transport (`agent.v1.AgentService/Run`) is a
    // paced bidirectional Connect/HTTP2 stream. The old
    // `ChatService/StreamUnifiedChatWithTools` endpoint was decommissioned for
    // API-key / CLI tokens and now returns "Update Required"/payment errors.
    let first_result =
        crate::agent_transport::run_agent_turn(&tokens.access_token, prompt, model, tx.clone())
            .await;

    match first_result {
        Ok(()) => Ok(()),
        Err(err) if cursor_auth::error_indicates_not_logged_in(&err) => {
            let refreshed = cursor_auth::refresh_resolved_tokens(&client, &tokens)
                .await
                .with_context(|| {
                    format!("Cursor token was rejected and refresh also failed after: {err:#}")
                })?;
            crate::agent_transport::run_agent_turn(&refreshed.access_token, prompt, model, tx).await
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
#[path = "cursor_tests.rs"]
mod cursor_tests;
