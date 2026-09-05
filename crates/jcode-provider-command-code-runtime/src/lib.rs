//! Command Code provider runtime: auth import/verify/OAuth (plan 01),
//! discovery/efforts (plan 02), NDJSON transport (plan 03), quota/failover
//! (plan 04) and the composed Provider (plan 05).

pub mod auth;
pub mod efforts;
pub mod failover;
pub mod integration;
pub mod models;
pub mod ndjson;
pub mod project_context;
pub mod quota;
pub mod serializer;

use futures::StreamExt as _;
use futures::TryStreamExt as _;
use ndjson::decode_ndjson_stream;
use serializer::serialize_messages;

#[cfg(test)]
mod command_code_auth_tests;
#[cfg(test)]
mod command_code_integration_tests;
#[cfg(test)]
mod command_code_models_tests;
#[cfg(test)]
mod command_code_quota_tests;
#[cfg(test)]
mod command_code_streaming_tests;

use anyhow::Result;
use jcode_message_types::{Message, StreamEvent, ToolDefinition};
use jcode_provider_command_code::{
    COMMAND_CODE_VERSION, COMMAND_CODE_VERSION_HEADER, GENERATE_URL, SESSION_ID_HEADER, USER_AGENT,
};
use jcode_provider_core::{EventStream, Provider};
use serde_json::json;
use std::sync::{Arc, RwLock};

/// Active transport connection label surfaced to the status bar hook.
pub const CONNECTION: &str = "HTTP/2";

fn project_slug(path: &std::path::Path) -> String {
    let slug = path
        .display()
        .to_string()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    slug.trim_matches('-').chars().take(64).collect::<String>()
}

fn response_retry_after(body: &str) -> Option<u64> {
    body.lines().find_map(|line| {
        let lower = line.to_ascii_lowercase();
        lower
            .strip_prefix("retry-after:")
            .and_then(|v| v.trim().parse().ok())
    })
}

/// Minimal native /alpha/generate text streaming provider (tracer slice;
/// full event mapping and composition land in plans 03/05).
pub struct CommandCodeProvider {
    pub client: reqwest::Client,
    pub api_key: String,
    active_key: Arc<RwLock<String>>,
    pool: Arc<Vec<(String, String)>>,
    pub session_id: String,
    model: Arc<RwLock<String>>,
    pub(crate) catalog: Arc<models::CommandCodeCatalog>,
    reasoning: Arc<efforts::CommandCodeReasoningCapability>,
    quota: Arc<quota::CommandCodeQuotaCache>,
}

impl CommandCodeProvider {
    pub fn new(api_key: String, session_id: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            active_key: Arc::new(RwLock::new(String::new())),
            pool: Arc::new(Vec::new()),
            session_id,
            model: Arc::new(RwLock::new(model)),
            catalog: Arc::new(models::CommandCodeCatalog::new()),
            reasoning: Arc::new(efforts::CommandCodeReasoningCapability::default()),
            quota: Arc::new(quota::CommandCodeQuotaCache::new()),
        }
    }

    pub fn with_pool(mut self, accounts: Vec<(String, String)>) -> Self {
        self.active_key = Arc::new(RwLock::new(self.api_key.clone()));
        self.pool = Arc::new(accounts);
        self
    }

    /// Build the canonical /alpha/generate POST (headers + stream:true).
    pub fn generate_request(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
    ) -> Result<reqwest::RequestBuilder> {
        let context =
            project_context::project_context_cache(std::env::current_dir().unwrap_or_default());
        let model = self.model();
        let reasoning = (!self.reasoning.reasoning_denied(&model)).then_some("max");
        let mut body = json!({
            "config": context,
            "memory": "", "taste": null, "skills": null, "permissionMode": "standard", "mode": "agent",
            "params": {"model": model, "messages": serialize_messages(messages), "tools": tools, "system": system, "max_tokens": 64000, "stream": true, "reasoning_effort": reasoning},
        });
        if reasoning.is_none() {
            body["params"]
                .as_object_mut()
                .map(|params| params.remove("reasoning_effort"));
        }
        Ok(self
            .client
            .post(GENERATE_URL)
            .bearer_auth(
                self.active_key
                    .read()
                    .ok()
                    .filter(|key| !key.is_empty())
                    .map(|key| key.clone())
                    .unwrap_or_else(|| self.api_key.clone()),
            )
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(COMMAND_CODE_VERSION_HEADER, COMMAND_CODE_VERSION)
            .header(SESSION_ID_HEADER, &self.session_id)
            .header("x-cli-environment", "production")
            .header("x-taste-learning", "false")
            .header("x-co-flag", "false")
            .header(
                "x-project-slug",
                project_slug(&std::env::current_dir().unwrap_or_default()),
            )
            .json(&body))
    }
}

#[async_trait::async_trait]
impl Provider for CommandCodeProvider {
    fn name(&self) -> &str {
        "command-code"
    }

    fn model(&self) -> String {
        self.model
            .read()
            .map(|model| model.clone())
            .unwrap_or_else(|_| jcode_provider_command_code::DEFAULT_MODEL.to_string())
    }

    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let session = resume_session_id.unwrap_or(&self.session_id);
        let mut attempt = 0;
        let response = loop {
            let mut request = self.generate_request(messages, tools, system)?;
            request = request.header(SESSION_ID_HEADER, session);
            let response = jcode_provider_core::transport::send_with_initial_response_timeout(
                request,
                std::time::Duration::from_secs(120),
            )
            .await?;
            if response.status().is_success() {
                break response;
            }
            let status = response.status().as_u16();
            let retry_after_header = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            let head = response.text().await.unwrap_or_default();
            if attempt == 0
                && self
                    .reasoning
                    .classify_pre_stream_rejection(&self.model(), status, &head)
                    .is_some()
            {
                attempt += 1;
                continue;
            }
            if attempt == 0 && status == 429 {
                let active_key = self
                    .active_key
                    .read()
                    .ok()
                    .filter(|key| !key.is_empty())
                    .map(|key| key.clone())
                    .unwrap_or_else(|| self.api_key.clone());
                let _ = quota::command_code_credits(
                    &self.client,
                    &active_key,
                    None,
                    session,
                    &self.quota,
                )
                .await;
                let retry_after = retry_after_header
                    .or_else(|| response_retry_after(&head))
                    .unwrap_or(0);
                let current_label = self
                    .pool
                    .iter()
                    .find(|(_, key)| key == &active_key)
                    .map(|(label, _)| label.clone())
                    .unwrap_or_else(|| session.to_string());
                let labels = self
                    .pool
                    .iter()
                    .map(|(label, _)| label.clone())
                    .collect::<Vec<_>>();
                let candidates =
                    failover::command_code_pool_candidates(&labels, Some(&current_label));
                match failover::handle_command_code_error_failover(
                    &current_label,
                    status,
                    Some(std::time::Duration::from_secs(retry_after)),
                    &candidates,
                    false,
                    attempt,
                ) {
                    failover::CommandCodeFailoverAction::StickWait { delay } => {
                        attempt += 1;
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    failover::CommandCodeFailoverAction::Rotate { next_account, .. } => {
                        let Some((_, key)) =
                            self.pool.iter().find(|(label, _)| label == &next_account)
                        else {
                            anyhow::bail!("Command Code failover selected an unknown account");
                        };
                        if let Ok(mut active) = self.active_key.write() {
                            *active = key.clone();
                        }
                        attempt += 1;
                        continue;
                    }
                    failover::CommandCodeFailoverAction::AllExhausted { message } => {
                        anyhow::bail!(message)
                    }
                    failover::CommandCodeFailoverAction::NoAction => {}
                }
                anyhow::bail!(
                    "Command Code rate limited; account cooled for {}s",
                    retry_after
                );
            }
            anyhow::bail!(
                "Command Code generate failed: {} (head: {})",
                status,
                head.chars().take(512).collect::<String>()
            );
        };
        let byte_stream = response
            .bytes_stream()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
        let text_lines = FramedStringStream::new(byte_stream);
        decode_ndjson_stream(Box::pin(text_lines))
    }

    fn set_model(&self, model: &str) -> Result<()> {
        self.model
            .write()
            .map(|mut target| {
                *target = model.to_string();
            })
            .map_err(|_| anyhow::anyhow!("model lock poisoned"))
    }

    fn available_models(&self) -> Vec<&'static str> {
        self.catalog
            .model_ids()
            .into_iter()
            .map(|model| Box::leak(model.into_boxed_str()) as &'static str)
            .collect()
    }

    fn fork(&self) -> Arc<dyn Provider> {
        let active_key = self
            .active_key
            .read()
            .map(|key| key.clone())
            .unwrap_or_else(|_| self.api_key.clone());
        Arc::new(CommandCodeProvider {
            client: self.client.clone(),
            api_key: self.api_key.clone(),
            active_key: Arc::new(RwLock::new(active_key)),
            pool: self.pool.clone(),
            session_id: self.session_id.clone(),
            model: Arc::new(RwLock::new(self.model())),
            catalog: self.catalog.clone(),
            reasoning: self.reasoning.clone(),
            quota: self.quota.clone(),
        })
    }
}

/// Incremental splitter: byte chunks -> complete NDJSON lines.
pub struct FramedStringStream<S> {
    inner: S,
    buffer: Vec<u8>,
}

impl<S> FramedStringStream<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
        }
    }
}

impl<S> futures::Stream for FramedStringStream<S>
where
    S: futures::Stream<Item = std::io::Result<bytes::Bytes>> + Unpin,
{
    type Item = Result<String>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        loop {
            if let Some(pos) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let mut line: Vec<u8> = self.buffer.drain(..=pos).collect();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                let text = String::from_utf8_lossy(&line).to_string();
                return std::task::Poll::Ready(Some(Ok(text)));
            }
            match self.inner.poll_next_unpin(cx) {
                std::task::Poll::Ready(Some(Ok(chunk))) => self.buffer.extend_from_slice(&chunk),
                std::task::Poll::Ready(Some(Err(e))) => {
                    return std::task::Poll::Ready(Some(Err(anyhow::anyhow!(e))));
                }
                std::task::Poll::Ready(None) => {
                    if self.buffer.is_empty() {
                        return std::task::Poll::Ready(None);
                    }
                    let rest = std::mem::take(&mut self.buffer);
                    let text = String::from_utf8_lossy(&rest).to_string();
                    return std::task::Poll::Ready(Some(Ok(text)));
                }
                std::task::Poll::Pending => return std::task::Poll::Pending,
            }
        }
    }
}

/// Minimal NDJSON smoke decoder used by plan 01 tests; single-pass, maps only
/// text-delta and error records, drops unknown/blank lines and strips data:
/// prefixes. A protocol/EOF with no terminal record still ends cleanly.
pub fn decode_text_only_stream(
    lines: std::pin::Pin<Box<dyn futures::Stream<Item = Result<String>> + Send>>,
) -> Result<EventStream> {
    use futures::StreamExt;
    use jcode_provider_command_code::{GenerateRecord, decode_record_line};
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<StreamEvent>>(64);
    tokio::spawn(async move {
        let mut lines = lines;
        while let Some(line) = lines.next().await {
            let line = match line {
                Ok(line) => line,
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    break;
                }
            };
            for line in line.split('\n').map(str::trim).filter(|l| !l.is_empty()) {
                if line.is_empty() {
                    continue;
                }
                match decode_record_line(line) {
                    GenerateRecord::TextDelta(text) => {
                        if tx.send(Ok(StreamEvent::TextDelta(text))).await.is_err() {
                            return;
                        }
                    }
                    GenerateRecord::Error(message) => {
                        let _ = tx
                            .send(Ok(StreamEvent::Error {
                                message,
                                retry_after_secs: None,
                            }))
                            .await;
                        return;
                    }
                    GenerateRecord::Ignored => {}
                }
            }
        }
    });
    Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
}
