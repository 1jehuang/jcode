//! Request details shared by models with mandatory, conversation-bound thinking.
use jcode_provider_anthropic::{ApiThinking, ApiThinkingBlockBinding};
use jcode_provider_core::anthropic::anthropic_thinking_always_on;

const BINDING_BETA: &str = "thinking-binding-controls-2026-08-01";

pub(super) fn adaptive_thinking(model: &str) -> ApiThinking {
    ApiThinking::Adaptive {
        // Opus 5.5 intermediate progress is thinking, not text. Request summaries
        // explicitly rather than accepting its default empty thinking blocks.
        display: Some("summarized"),
        block_binding: anthropic_thinking_always_on(model).then_some(ApiThinkingBlockBinding {
            prefix_mismatch_behavior: "drop_block",
        }),
    }
}

pub(super) fn fallback_temperature(model: &str, is_oauth: bool) -> Option<f32> {
    (is_oauth && !anthropic_thinking_always_on(model)).then_some(1.0)
}

pub(super) fn with_binding_beta(base: &str, thinking: &Option<ApiThinking>) -> String {
    let binding = matches!(
        thinking,
        Some(ApiThinking::Adaptive {
            block_binding: Some(_),
            ..
        })
    );
    if binding && !base.split(',').any(|header| header.trim() == BINDING_BETA) {
        if base.is_empty() {
            BINDING_BETA.to_string()
        } else {
            format!("{base},{BINDING_BETA}")
        }
    } else {
        base.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn opus_55_and_fable_51_request_bound_thinking_and_progress_summaries() {
        for model in ["claude-opus-5-5", "claude-fable-5-1"] {
            let thinking = adaptive_thinking(model);
            assert_eq!(
                serde_json::to_value(&thinking).unwrap(),
                json!({
                    "type": "adaptive", "display": "summarized",
                    "block_binding": {"prefix_mismatch_behavior": "drop_block"}
                })
            );
            let header = with_binding_beta("prompt-caching-2024-07-31", &Some(thinking.clone()));
            assert!(header.ends_with(BINDING_BETA));
            assert_eq!(with_binding_beta(&header, &Some(thinking)), header);
            for is_oauth in [false, true] {
                assert_eq!(fallback_temperature(model, is_oauth), None);
            }
        }
    }

    #[test]
    fn older_models_keep_original_payload_and_headers() {
        for model in ["claude-opus-5", "claude-fable-5", "claude-sonnet-4-6"] {
            let thinking = adaptive_thinking(model);
            assert_eq!(
                serde_json::to_value(&thinking).unwrap(),
                json!({
                    "type": "adaptive", "display": "summarized"
                })
            );
            assert_eq!(
                with_binding_beta("existing-beta", &Some(thinking)),
                "existing-beta"
            );
            assert_eq!(fallback_temperature(model, true), Some(1.0));
            assert_eq!(fallback_temperature(model, false), None);
        }
        assert_eq!(with_binding_beta("existing-beta", &None), "existing-beta");
    }
}
