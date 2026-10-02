//! Grok Build (Grok CLI subscription) provider over direct HTTPS.
//!
//! Requests go straight to the Grok CLI chat proxy
//! (`https://cli-chat-proxy.grok.com/v1`, OpenAI-compatible chat completions)
//! using the Grok CLI OIDC session from `$GROK_HOME/auth.json` /
//! `~/.grok/auth.json`, presenting the official Grok CLI client identity (see
//! `jcode_base::auth::grok_build`). No `grok` subprocess is ever launched.
//!
//! Streaming, request building, and SSE parsing reuse the OpenAI-compatible
//! runtime. Jcode owns tool execution. This wrapper adds the Grok Build route
//! identity, model selection, and a single refresh-and-retry on HTTP 401.

use anyhow::{Result, bail};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use jcode_message_types::{Message, StreamEvent, ToolDefinition};
use jcode_provider_core::{EventStream, ModelRoute, Provider};
use jcode_provider_openrouter_runtime::{GROK_BUILD_MODELS, OpenRouterProvider};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};

pub const DEFAULT_MODEL: &str = "grok-4.6";
const ROUTE_PREFIX: &str = "grok-build:";
/// Stable route api_method (kept for saved sessions and `/model` routing).
pub const ROUTE_API_METHOD: &str = "grok-build-acp";

#[derive(Clone)]
pub struct GrokBuildProvider {
    inner: Arc<OpenRouterProvider>,
}

impl GrokBuildProvider {
    pub fn new() -> Self {
        Self::with_model(DEFAULT_MODEL)
    }

    pub fn with_model(model: &str) -> Self {
        Self {
            inner: Arc::new(OpenRouterProvider::new_grok_build_subscription(
                normalize_model(model).unwrap_or(DEFAULT_MODEL),
            )),
        }
    }

    async fn complete_once(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
    ) -> Result<EventStream> {
        // The chat proxy is stateless per request: always send full history.
        self.inner.complete(messages, tools, system, None).await
    }
}

impl Default for GrokBuildProvider {
    fn default() -> Self {
        Self::new()
    }
}

fn normalize_model(model: &str) -> Option<&str> {
    let model = model.trim();
    let model = model.strip_prefix(ROUTE_PREFIX).unwrap_or(model).trim();
    (!model.is_empty()).then_some(model)
}

/// Whether an error from the chat proxy means the bearer token was rejected.
fn is_unauthorized_text(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("status: 401")
        || (text.contains("401")
            && (text.contains("unauthorized")
                || text.contains("invalid or expired credentials")
                || text.contains("unauthenticated")))
}

fn is_unauthorized_item(item: &Result<StreamEvent>) -> bool {
    match item {
        Err(error) => is_unauthorized_text(&format!("{error:#}")),
        Ok(StreamEvent::Error { message, .. }) => is_unauthorized_text(message),
        _ => false,
    }
}

/// Emits the inner stream, but if the first error is a 401 before any model
/// output, forces a token refresh and replays the request once.
struct AuthRetryStream {
    inner: EventStream,
    retry: Option<Box<dyn FnOnce() -> RetryFuture + Send>>,
    pending: Option<RetryFuture>,
    saw_output: bool,
}

type RetryFuture = Pin<Box<dyn Future<Output = Result<EventStream>> + Send>>;

fn produces_output(event: &StreamEvent) -> bool {
    matches!(
        event,
        StreamEvent::TextDelta(_)
            | StreamEvent::ThinkingDelta(_)
            | StreamEvent::ToolUseStart { .. }
            | StreamEvent::ToolInputDelta(_)
    )
}

impl Stream for AuthRetryStream {
    type Item = Result<StreamEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(pending) = this.pending.as_mut() {
                match pending.as_mut().poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Ok(stream)) => {
                        this.pending = None;
                        this.inner = stream;
                    }
                    Poll::Ready(Err(error)) => {
                        this.pending = None;
                        return Poll::Ready(Some(Err(error)));
                    }
                }
            }
            match this.inner.poll_next_unpin(cx) {
                Poll::Ready(Some(item)) => {
                    if !this.saw_output
                        && is_unauthorized_item(&item)
                        && let Some(retry) = this.retry.take()
                    {
                        this.pending = Some(retry());
                        continue;
                    }
                    if let Ok(event) = &item
                        && produces_output(event)
                    {
                        this.saw_output = true;
                    }
                    return Poll::Ready(Some(item));
                }
                other => return other,
            }
        }
    }
}

#[async_trait]
impl Provider for GrokBuildProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let first = self.complete_once(messages, tools, system).await;
        let provider = self.clone();
        let messages = messages.to_vec();
        let tools = tools.to_vec();
        let system = system.to_string();
        let retry = move || -> RetryFuture {
            Box::pin(async move {
                jcode_base::auth::grok_build::bearer_token(true).await?;
                provider.complete_once(&messages, &tools, &system).await
            })
        };
        let inner = match first {
            Ok(stream) => stream,
            // Already retried once: return the replayed stream as-is.
            Err(error) if is_unauthorized_text(&format!("{error:#}")) => return retry().await,
            Err(error) => return Err(error),
        };
        Ok(Box::pin(AuthRetryStream {
            inner,
            retry: Some(Box::new(retry)),
            pending: None,
            saw_output: false,
        }))
    }

    fn name(&self) -> &str {
        "grok-build"
    }

    fn display_name(&self) -> String {
        "Grok Build".to_string()
    }

    fn model(&self) -> String {
        self.inner.model()
    }

    fn set_model(&self, model: &str) -> Result<()> {
        let Some(model) = normalize_model(model) else {
            bail!("Grok Build model cannot be empty");
        };
        self.inner.set_model(model)
    }

    fn available_models_display(&self) -> Vec<String> {
        let mut models: Vec<String> = GROK_BUILD_MODELS.iter().map(|m| m.to_string()).collect();
        let current = self.model();
        if !current.is_empty() && !models.contains(&current) {
            models.insert(0, current);
        }
        models
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        self.available_models_display()
    }

    fn model_routes(&self) -> Vec<ModelRoute> {
        self.available_models_display()
            .into_iter()
            .map(|model| ModelRoute {
                model,
                provider: "Grok Build".to_string(),
                api_method: ROUTE_API_METHOD.to_string(),
                available: true,
                detail: "Grok Build subscription via the Grok CLI chat proxy (HTTPS)".to_string(),
                usage: None,
                cheapness: None,
            })
            .collect()
    }

    async fn prefetch_models(&self) -> Result<()> {
        Ok(())
    }

    fn active_auth_method_label(&self) -> Option<&'static str> {
        Some("Grok Build subscription login")
    }

    fn handles_tools_internally(&self) -> bool {
        false
    }

    fn supports_compaction(&self) -> bool {
        true
    }

    fn supports_image_input(&self) -> bool {
        self.inner.supports_image_input()
    }

    fn context_window(&self) -> usize {
        self.inner.context_window()
    }

    fn transport(&self) -> Option<String> {
        Some("HTTPS (SSE)".to_string())
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self::with_model(&self.model()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn model_selection_strips_route_prefix_and_rejects_empty() {
        let provider = GrokBuildProvider::new();
        assert_eq!(provider.model(), DEFAULT_MODEL);
        provider.set_model("grok-build:grok-4.5").unwrap();
        assert_eq!(provider.model(), "grok-4.5");
        provider.set_model("grok-code-fast-1").unwrap();
        assert_eq!(provider.model(), "grok-code-fast-1");
        assert!(provider.set_model("grok-build:").is_err());
        assert!(provider.set_model("  ").is_err());
        assert_eq!(
            GrokBuildProvider::with_model("grok-build:grok-4.5").model(),
            "grok-4.5"
        );
    }

    #[test]
    fn routes_keep_stable_ids_and_jcode_owns_tools() {
        let provider = GrokBuildProvider::new();
        let routes = provider.model_routes();
        assert!(routes.iter().any(|route| route.model == "grok-4.6"));
        assert!(
            routes
                .iter()
                .all(|route| route.api_method == ROUTE_API_METHOD)
        );
        assert_eq!(
            jcode_provider_core::ModelRouteApiMethod::parse(ROUTE_API_METHOD),
            jcode_provider_core::ModelRouteApiMethod::GrokBuild
        );
        assert!(!provider.handles_tools_internally());
        assert!(provider.supports_compaction());
        assert_eq!(provider.name(), "grok-build");
        assert_eq!(provider.context_window(), 500_000);
        let fork = provider.fork();
        assert_eq!(fork.model(), provider.model());
    }

    #[test]
    fn classifies_proxy_401_messages() {
        assert!(is_unauthorized_text(
            "OpenAI-compatible chat request failed\n  status: 401 Unauthorized\n  response: {}"
        ));
        assert!(is_unauthorized_text(
            "HTTP 401: Invalid or expired credentials (auth_kind=bearer, x_xai_token_auth=xai-grok-cli)"
        ));
        assert!(!is_unauthorized_text("status: 429 Too Many Requests"));
        assert!(!is_unauthorized_text("processed 401 tokens"));
    }

    fn stream_of(items: Vec<Result<StreamEvent>>) -> EventStream {
        Box::pin(futures::stream::iter(items))
    }

    #[tokio::test]
    async fn retries_once_on_401_before_output() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let stream = AuthRetryStream {
            inner: stream_of(vec![Err(anyhow::anyhow!("status: 401 Unauthorized"))]),
            retry: Some(Box::new(move || -> RetryFuture {
                counter.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(stream_of(vec![Ok(StreamEvent::TextDelta("hi".into()))])) })
            })),
            pending: None,
            saw_output: false,
        };
        let items: Vec<_> = stream.collect().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0], Ok(StreamEvent::TextDelta(text)) if text == "hi"));
    }

    #[tokio::test]
    async fn does_not_retry_after_output_or_twice() {
        let stream = AuthRetryStream {
            inner: stream_of(vec![
                Ok(StreamEvent::TextDelta("partial".into())),
                Err(anyhow::anyhow!("status: 401 Unauthorized")),
            ]),
            retry: Some(Box::new(|| -> RetryFuture { panic!("must not retry") })),
            pending: None,
            saw_output: false,
        };
        let items: Vec<_> = stream.collect().await;
        assert_eq!(items.len(), 2);
        assert!(items[1].is_err());

        let stream = AuthRetryStream {
            inner: stream_of(vec![Err(anyhow::anyhow!("status: 401 Unauthorized"))]),
            retry: Some(Box::new(|| -> RetryFuture {
                Box::pin(async { Ok(stream_of(vec![Err(anyhow::anyhow!("status: 401 again"))])) })
            })),
            pending: None,
            saw_output: false,
        };
        let items: Vec<_> = stream.collect().await;
        assert_eq!(items.len(), 1);
        assert!(items[0].is_err());
    }
}
