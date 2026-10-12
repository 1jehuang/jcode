//! Request details shared by models with mandatory, conversation-bound thinking.
use super::AnthropicProvider;
use jcode_provider_anthropic::{ApiRequest, ApiThinking, ApiThinkingBlockBinding};
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

/// Preserve caller preferences, not the original model's resolved defaults.
/// Both fallback paths use this snapshot to rebuild the next wire request.
pub(super) struct RetrySettings {
    effort: Option<String>,
    show_thinking: bool,
    max_tokens_override: Option<u32>,
    service_tier: Option<String>,
}

impl RetrySettings {
    pub(super) fn from_provider(provider: &AnthropicProvider) -> Self {
        Self {
            effort: provider.stored_reasoning_effort(),
            show_thinking: jcode_base::config::config().display.show_thinking,
            max_tokens_override: provider.max_tokens_override,
            service_tier: provider
                .service_tier
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        }
    }

    pub(super) fn reshape(&self, request: &mut ApiRequest, model: &str, is_oauth: bool) {
        request.model = super::strip_1m_suffix(model).to_string();
        request.max_tokens = self
            .max_tokens_override
            .unwrap_or_else(|| jcode_provider_core::anthropic::anthropic_max_output_tokens(model));
        let effort = self
            .effort
            .clone()
            .or_else(|| AnthropicProvider::default_reasoning_effort_for_model(model));
        let resolved = effort.as_deref().map(|effort| {
            jcode_base::prompt::swarm_root_reasoning_effort(effort).unwrap_or(effort)
        });
        (request.thinking, request.output_config, request.temperature) =
            AnthropicProvider::build_reasoning_request_parts_for_budget(
                model,
                is_oauth,
                self.show_thinking,
                resolved,
                request.max_tokens,
            );
        request.service_tier = self
            .service_tier
            .clone()
            .filter(|_| AnthropicProvider::model_supports_priority_service_tier(model));
    }
}

/// Retry at most once by dropping unsupported effort. Always-on models retain
/// their progress display and binding policy. If that payload itself is rejected,
/// surface the error instead of silently losing required request shaping.
pub(super) fn recover_rejected_reasoning(
    request: &mut ApiRequest,
    model: &str,
    is_oauth: bool,
) -> bool {
    if anthropic_thinking_always_on(model) {
        if request.output_config.take().is_none() {
            return false;
        }
        request.thinking = Some(adaptive_thinking(model));
        request.temperature = None;
    } else {
        if request.thinking.is_none() && request.output_config.is_none() {
            return false;
        }
        request.thinking = None;
        request.output_config = None;
        request.temperature = fallback_temperature(model, is_oauth);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request() -> ApiRequest {
        ApiRequest {
            model: "unused".into(),
            max_tokens: 1,
            system: None,
            messages: vec![],
            tools: None,
            metadata: None,
            thinking: None,
            output_config: None,
            temperature: None,
            service_tier: None,
            stream: true,
        }
    }

    fn settings(effort: Option<&str>) -> RetrySettings {
        RetrySettings {
            effort: effort.map(str::to_string),
            show_thinking: false,
            max_tokens_override: None,
            service_tier: Some("auto".into()),
        }
    }

    #[test]
    fn model_not_found_retry_rebuilds_mandatory_thinking_wire_payload() {
        for oauth in [false, true] {
            for effort in ["none", "low"] {
                let settings = settings(Some(effort));
                let mut request = request();
                settings.reshape(&mut request, "claude-opus-4-5", oauth);
                if effort == "none" {
                    assert!(request.thinking.is_none());
                } else {
                    assert!(matches!(
                        request.thinking,
                        Some(ApiThinking::Enabled { .. })
                    ));
                }
                settings.reshape(&mut request, "claude-opus-5-5", oauth);
                let wire = serde_json::to_value(&request).unwrap();
                assert_eq!(wire["model"], "claude-opus-5-5");
                assert_eq!(wire["thinking"]["type"], "adaptive");
                assert_eq!(wire["thinking"]["display"], "summarized");
                assert_eq!(
                    wire["thinking"]["block_binding"]["prefix_mismatch_behavior"],
                    "drop_block"
                );
                assert_eq!(wire["output_config"]["effort"], "low");
                assert_eq!(wire["max_tokens"], 128_000);
                assert!(wire.get("temperature").is_none());
                assert!(wire.get("service_tier").is_none());
                assert_eq!(with_binding_beta("", &request.thinking), BINDING_BETA);
            }
        }
    }

    #[test]
    fn fable_quota_retry_rebuilds_target_defaults_and_removes_binding() {
        let settings = settings(None);
        let mut request = request();
        settings.reshape(&mut request, "claude-fable-5-1", true);
        assert_eq!(
            serde_json::to_value(&request).unwrap()["output_config"]["effort"],
            "high"
        );
        settings.reshape(&mut request, "claude-opus-4-8", true);
        let wire = serde_json::to_value(&request).unwrap();
        assert_eq!(wire["output_config"]["effort"], "xhigh");
        assert_eq!(wire["service_tier"], "auto");
        assert!(wire["thinking"].get("block_binding").is_none());
        assert_eq!(with_binding_beta("existing", &request.thinking), "existing");
        settings.reshape(&mut request, "claude-haiku-4-5", true);
        assert_eq!(request.max_tokens, 64_000);
        assert!(request.service_tier.is_none());
    }

    #[test]
    fn retry_preserves_explicit_budget_and_effort_preferences() {
        let mut settings = settings(Some("low"));
        settings.max_tokens_override = Some(4096);
        let mut request = request();
        for model in ["claude-fable-5-1", "claude-opus-5-5", "claude-opus-4-5"] {
            settings.reshape(&mut request, model, false);
            assert_eq!(request.max_tokens, 4096);
            if let Some(ApiThinking::Enabled { budget_tokens }) = request.thinking {
                assert!(budget_tokens < request.max_tokens);
            }
        }
    }

    #[test]
    fn always_on_rejection_keeps_bound_summaries_and_retries_only_once() {
        for model in ["claude-opus-5-5", "claude-fable-5-1"] {
            for oauth in [false, true] {
                let mut request = request();
                settings(Some("high")).reshape(&mut request, model, oauth);
                assert!(recover_rejected_reasoning(&mut request, model, oauth));
                let wire = serde_json::to_value(&request).unwrap();
                assert_eq!(wire["thinking"]["type"], "adaptive");
                assert_eq!(wire["thinking"]["display"], "summarized");
                assert_eq!(
                    wire["thinking"]["block_binding"]["prefix_mismatch_behavior"],
                    "drop_block"
                );
                assert!(wire.get("output_config").is_none());
                assert!(wire.get("temperature").is_none());
                assert!(!recover_rejected_reasoning(&mut request, model, oauth));
                assert_eq!(serde_json::to_value(&request).unwrap(), wire);
            }
        }
    }

    #[test]
    fn legacy_rejection_still_removes_unsupported_reasoning_once() {
        for oauth in [false, true] {
            let mut request = request();
            settings(Some("high")).reshape(&mut request, "claude-opus-4-5", oauth);
            assert!(recover_rejected_reasoning(
                &mut request,
                "claude-opus-4-5",
                oauth
            ));
            assert!(request.thinking.is_none());
            assert!(request.output_config.is_none());
            assert_eq!(request.temperature, oauth.then_some(1.0));
            assert!(!recover_rejected_reasoning(
                &mut request,
                "claude-opus-4-5",
                oauth
            ));
        }
    }

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
