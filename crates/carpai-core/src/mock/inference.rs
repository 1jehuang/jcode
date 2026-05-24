use std::sync::Arc;
use async_trait::async_trait;
use carpai_internal::inference_backend::*;

pub struct MockInferenceBackend;

fn extract_text(content: &ChatContent) -> String {
    match content {
        ChatContent::Text(s) => s.clone(),
        ChatContent::Parts(parts) => parts.iter().filter_map(|p| p.text.clone()).collect::<Vec<_>>().join(" "),
    }
}

#[async_trait]
impl InferenceBackend for MockInferenceBackend {
    async fn complete_chat(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, InferenceError> {
        let last_msg = request.messages.last();
        let reply = last_msg
            .map(|m| format!("Mock reply to: {}", extract_text(&m.content)))
            .unwrap_or_else(|| "Mock: no messages".to_string());

        Ok(ChatCompletionResponse {
            id: "mock-id".into(),
            object: "chat.completion".into(),
            created: 0,
            model: request.model,
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: ChatRole::Assistant,
                    content: ChatContent::Text(reply),
                    name: None,
                },
                finish_reason: FinishReason::Stop,
                logprobs: None,
            }],
            usage: CompletionTokenUsage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            },
            provider: None,
            fallback_info: None,
        })
    }

    async fn stream_chat(
        &self,
        _request: ChatCompletionRequest,
    ) -> Result<Box<dyn tokio_stream::Stream<Item = Result<StreamChunk, InferenceError>> + Send>, InferenceError> {
        Ok(Box::new(tokio_stream::iter(vec![])))
    }

    async fn list_models_with_routing(
        &self,
    ) -> Result<Vec<RoutedModelInfo>, InferenceError> {
        Ok(vec![])
    }

    async fn select_model(
        &self,
        _constraints: &ModelSelectionConstraints,
    ) -> Result<String, InferenceError> {
        Ok("mock-model".into())
    }

    async fn get_quota_usage(&self, user_id: &str) -> Result<QuotaUsage, InferenceError> {
        Ok(QuotaUsage {
            user_id: user_id.into(),
            tokens_used: 0,
            token_limit: 100000,
            requests_used: 0,
            request_limit: 1000,
            period_start: chrono::Utc::now(),
            period_end: chrono::Utc::now() + chrono::Duration::hours(24),
            reset_in_secs: 86400,
        })
    }

    async fn record_usage(
        &self,
        _user_id: &str,
        _usage: &CompletionTokenUsage,
        _model: &str,
    ) -> Result<(), InferenceError> {
        Ok(())
    }

    fn base_engine(&self) -> Arc<dyn carpai_internal::inference::InferenceEngine> {
        unimplemented!("Mock base_engine")
    }
}
