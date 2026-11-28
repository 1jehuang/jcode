//! `anthropic-beta` values for direct API-key requests.
//!
//! Prompt caching is generally available and requires no beta value. AWS Bedrock
//! rejects the old `prompt-caching-2024-07-31` value with 400.
use crate::reasoning_request::with_binding_beta;
use jcode_provider_anthropic::ApiThinking;

const CONTEXT_1M_BETA: &str = "context-1m-2025-08-07";
const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";

/// The header value for a direct API-key request, or `None` when the request
/// requires no beta.
pub(super) fn header(model: &str, thinking: &Option<ApiThinking>) -> Option<String> {
    let mut betas = Vec::new();
    if jcode_provider_core::anthropic_is_1m_model(model) {
        betas.push(CONTEXT_1M_BETA);
    }
    if thinking.is_some() {
        betas.push(INTERLEAVED_THINKING_BETA);
    }
    let header = with_binding_beta(&betas.join(","), thinking);
    (!header.is_empty()).then_some(header)
}

#[cfg(test)]
mod tests {
    use super::header;
    use crate::reasoning_request::adaptive_thinking;

    #[test]
    fn plain_model_sends_no_beta() {
        assert_eq!(header("claude-sonnet-5-5", &None), None);
    }

    #[test]
    fn long_context_model_sends_only_the_context_beta() {
        assert_eq!(
            header("claude-sonnet-4-6[1m]", &None).as_deref(),
            Some("context-1m-2025-08-07")
        );
    }

    #[test]
    fn thinking_sends_interleaved_thinking() {
        let thinking = Some(adaptive_thinking("claude-sonnet-5-5"));
        assert_eq!(
            header("claude-sonnet-5-5", &thinking).as_deref(),
            Some("interleaved-thinking-2025-05-14")
        );
    }

    #[test]
    fn always_on_thinking_adds_the_binding_beta() {
        let thinking = Some(adaptive_thinking("claude-opus-5-5"));
        assert_eq!(
            header("claude-opus-5-5", &thinking).as_deref(),
            Some("interleaved-thinking-2025-05-14,thinking-binding-controls-2026-08-01")
        );
    }
}
