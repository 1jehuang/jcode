//! Main CarpAI client implementation

use crate::cache::CacheManager;
use crate::config::CarpAiConfig;
use crate::error::{CarpAiError, Result};
use crate::ide::IdeAdapter;
use crate::protocol::{ProtocolAdapter, RestAdapter};
use crate::types::*;
use futures::Stream;
use std::pin::Pin;
use std::sync::Arc;
use tracing::instrument;

/// Main CarpAI client
pub struct CarpAiClient {
    config: CarpAiConfig,
    protocol: Arc<dyn ProtocolAdapter>,
    cache: CacheManager,
    rate_limiter: Arc<tokio::sync::Semaphore>,
    ide_adapter: Option<Arc<dyn IdeAdapter>>,
    is_online: Arc<std::sync::atomic::AtomicBool>,
    request_queue: Arc<tokio::sync::Mutex<Vec<QueuedRequest>>>,
}

/// Builder for constructing CarpAiClient
pub struct ClientBuilder {
    config: CarpAiConfig,
}

impl ClientBuilder {
    pub fn new() -> Self {
        Self {
            config: CarpAiConfig::default(),
        }
    }

    pub fn with_config(mut self, config: CarpAiConfig) -> Self {
        self.config = config;
        self
    }

    pub fn with_server_url(mut self, url: impl Into<String>) -> Self {
        self.config.server.url = Some(url.into());
        self
    }

    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.config.auth.api_key = Some(api_key.into());
        self
    }

    pub fn with_ide_adapter(self, _adapter: Arc<dyn IdeAdapter>) -> Self {
        // Note: IDE adapter should be set via CarpAiClient::with_ide_adapter after creation
        self
    }

    pub fn enable_cache(mut self, enabled: bool) -> Self {
        self.config.cache.enabled = enabled;
        self
    }

    pub fn enable_offline_mode(mut self, enabled: bool) -> Self {
        self.config.offline.enabled = enabled;
        self
    }

    pub async fn build(self) -> Result<CarpAiClient> {
        CarpAiClient::new(self.config).await
    }
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Queued request for offline mode
#[allow(dead_code)]
struct QueuedRequest {
    request_type: RequestType,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[allow(dead_code)]
enum RequestType {
    Completion(CompletionRequest),
    ChatCompletion(ChatCompletionRequest),
    CodeAction(CodeActionRequest),
}

impl CarpAiClient {
    /// Create a new CarpAI client with the given configuration
    pub async fn new(config: CarpAiConfig) -> Result<Self> {
        config.validate().map_err(|msg| CarpAiError::Config {
            message: msg,
            source: None,
        })?;

        let cache = CacheManager::new(config.cache.clone())?;
        let api_key = config.auth.get_api_key();

        // Create REST adapter by default
        let base_url = config
            .server
            .rest_url
            .as_deref()
            .unwrap_or("http://localhost:8080");
        let protocol: Arc<dyn ProtocolAdapter> = Arc::new(RestAdapter::new(
            base_url.to_string(),
            api_key,
            config.server.timeout_secs,
        ));

        // Create rate limiter (semaphore-based)
        let permits = config.performance.rate_limit_per_second as usize;
        let rate_limiter = Arc::new(tokio::sync::Semaphore::new(if permits > 0 { permits } else { 100 }));

        Ok(Self {
            config,
            protocol,
            cache,
            rate_limiter,
            ide_adapter: None,
            is_online: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            request_queue: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        })
    }

    /// Set the IDE adapter
    pub fn with_ide_adapter(mut self, adapter: Arc<dyn IdeAdapter>) -> Self {
        self.ide_adapter = Some(adapter);
        self
    }

    /// Check if client is online
    pub fn is_online(&self) -> bool {
        self.is_online.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Send a completion request
    #[instrument(skip(self), fields(prompt = %request.prompt))]
    pub async fn complete(&self, mut request: CompletionRequest) -> Result<CompletionResponse> {
        if !self.is_online() && self.config.offline.enabled {
            return self.handle_offline_completion(request).await;
        }

        // Check rate limit
        let _permit = self.rate_limiter.acquire().await;

        // Check cache first
        if self.config.performance.enable_cache {
            if let Some(cached) = self.cache.get(&request) {
                tracing::debug!("Cache hit for completion request");
                let mut response = cached;
                response.cached = true;
                return Ok(response);
            }
        }

        // Add request ID if not present
        if request.session_id.is_none() {
            request.session_id = Some(SessionId::new());
        }

        // Make the request
        let result = self.protocol.complete(request.clone()).await;

        match result {
            Ok(response) => {
                // Cache successful responses
                if self.config.performance.enable_cache {
                    if let Err(e) = self.cache.put(&request, response.clone()) {
                        tracing::warn!(error = %e, "Failed to cache response");
                    }
                }
                Ok(response)
            }
            Err(e) => {
                if e.is_recoverable() && self.config.offline.enabled && self.config.offline.queue_requests_when_offline {
                    self.queue_request(RequestType::Completion(request)).await?;
                    Err(CarpAiError::Offline {
                        message: "Server unavailable, request queued".to_string(),
                        queued: true,
                        suggestion: Some("Request will be sent when connection is restored".to_string()),
                    })
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Send a chat completion request
    pub async fn chat_complete(&self, request: ChatCompletionRequest) -> Result<ChatCompletionResponse> {
        if !self.is_online() && self.config.offline.enabled {
            return Err(CarpAiError::Offline {
                message: "Chat completion not available offline".to_string(),
                queued: false,
                suggestion: Some("Check your internet connection".to_string()),
            });
        }

        let _permit = self.rate_limiter.acquire().await;
        self.protocol.chat_complete(request).await
    }

    /// Stream a completion response
    #[allow(clippy::result_large_err)]
    pub fn stream_complete(
        &self,
        request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send + 'static>>> {
        if !self.is_online() && self.config.offline.enabled {
            return Err(CarpAiError::Offline {
                message: "Streaming not available offline".to_string(),
                queued: false,
                suggestion: Some("Check your internet connection".to_string()),
            });
        }

        self.protocol.stream_complete(request)
    }

    /// Execute a code action
    pub async fn code_action(&self, request: CodeActionRequest) -> Result<CodeActionResponse> {
        if !self.is_online() && self.config.offline.enabled {
            return Err(CarpAiError::Offline {
                message: "Code actions not available offline".to_string(),
                queued: false,
                suggestion: Some("Check your internet connection".to_string()),
            });
        }

        let _permit = self.rate_limiter.acquire().await;
        self.protocol.code_action(request).await
    }

    /// Check server health
    pub async fn health_check(&self) -> Result<HealthCheckResponse> {
        let result = self.protocol.health_check().await;

        match &result {
            Ok(_) => {
                self.is_online.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            Err(_) => {
                self.is_online.store(false, std::sync::atomic::Ordering::Relaxed);
            }
        }

        result
    }

    /// Handle completion in offline mode
    async fn handle_offline_completion(&self, request: CompletionRequest) -> Result<CompletionResponse> {
        if let Some(cached) = self.cache.get(&request) {
            tracing::info!("Using cached response for offline request");
            let mut response = cached;
            response.cached = true;
            Ok(response)
        } else if self.config.offline.queue_requests_when_offline {
            self.queue_request(RequestType::Completion(request)).await?;
            Err(CarpAiError::Offline {
                message: "No cached response available, request queued".to_string(),
                queued: true,
                suggestion: Some("Request will be processed when back online".to_string()),
            })
        } else {
            Err(CarpAiError::Offline {
                message: "No cached response available and queuing disabled".to_string(),
                queued: false,
                suggestion: Some("Enable request queuing or check connection".to_string()),
            })
        }
    }

    /// Queue a request for later processing
    async fn queue_request(&self, request: RequestType) -> Result<()> {
        let mut queue = self.request_queue.lock().await;

        if queue.len() >= self.config.offline.max_queued_requests {
            return Err(CarpAiError::Offline {
                message: "Request queue is full".to_string(),
                queued: false,
                suggestion: Some("Some requests may have been dropped. Try again later.".to_string()),
            });
        }

        queue.push(QueuedRequest {
            request_type: request,
            created_at: chrono::Utc::now(),
        });

        tracing::info!(
            queue_len = queue.len(),
            "Request queued for offline processing"
        );

        Ok(())
    }

    /// Process queued requests when coming back online
    pub async fn process_queue(&self) -> Result<u32> {
        let mut queue = self.request_queue.lock().await;
        let count = queue.len() as u32;

        for queued in queue.drain(..) {
            match queued.request_type {
                RequestType::Completion(req) => {
                    if let Err(e) = self.complete(req).await {
                        tracing::warn!(error = %e, "Failed to process queued completion");
                    }
                }
                RequestType::ChatCompletion(req) => {
                    if let Err(e) = self.chat_complete(req).await {
                        tracing::warn!(error = %e, "Failed to process queued chat completion");
                    }
                }
                RequestType::CodeAction(req) => {
                    if let Err(e) = self.code_action(req).await {
                        tracing::warn!(error = %e, "Failed to process queued code action");
                    }
                }
            }
        }

        tracing::info!(count = count, "Processed queued requests");
        Ok(count)
    }

    /// Get cache statistics
    pub fn cache_stats(&self) -> crate::cache::CacheStats {
        self.cache.stats()
    }

    /// Clear the cache
    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    /// Get configuration reference
    pub fn config(&self) -> &CarpAiConfig {
        &self.config
    }
}
