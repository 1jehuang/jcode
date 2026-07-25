//! Streaming timeout budgets for the OpenAI Responses transports.
//!
//! Both the HTTPS/SSE and websocket paths must agree on how long a silent model
//! is allowed to think. Reasoning effort drives that budget: without summaries a
//! max-effort turn emits nothing for minutes, which is indistinguishable from a
//! dead connection unless the budget scales with the requested effort.

use serde_json::Value;

/// Build the Responses `reasoning` payload for a requested effort.
///
/// `summary` is mandatory. Without it the stream stays completely silent while
/// the model thinks, so high/xhigh/max efforts blow past the idle timeout and
/// get killed mid-thought, and no thinking ever renders for the user. Summary
/// deltas both keep the connection demonstrably alive and surface the thinking.
pub(crate) fn reasoning_payload(effort: &str) -> Value {
    serde_json::json!({ "effort": effort, "summary": "auto" })
}

/// Reasoning effort requested on this Responses payload, if any.
pub(crate) fn request_reasoning_effort(request: &Value) -> Option<&str> {
    request
        .get("reasoning")
        .and_then(|reasoning| reasoning.get("effort"))
        .and_then(|effort| effort.as_str())
}

/// Idle budget between HTTPS/SSE events for this request.
pub(crate) fn effective_https_idle_timeout(request: &Value) -> std::time::Duration {
    jcode_base::provider::stream_idle_timeout_for_effort(request_reasoning_effort(request))
}

/// Effective websocket completion budget in seconds.
///
/// Starts from the built-in default, raised to `[provider]
/// stream_idle_timeout_secs` when the user configured a larger value so one
/// transport cannot cut off sooner than another (issue #434), then scaled by the
/// request's reasoning effort.
pub(crate) fn effective_ws_completion_timeout_secs(request: &Value) -> u64 {
    let multiplier = u64::from(
        jcode_base::provider::stream_idle_timeout_multiplier_for_effort(request_reasoning_effort(
            request,
        )),
    );
    jcode_provider_openai::websocket_health::WEBSOCKET_COMPLETION_TIMEOUT_SECS
        .max(jcode_base::provider::stream_idle_timeout().as_secs())
        .saturating_mul(multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_payload_always_requests_summaries() {
        // Regression guard: dropping `summary` reintroduces silent streams that
        // trip the idle timeout on high reasoning efforts.
        for effort in ["low", "high", "xhigh", "max"] {
            let payload = reasoning_payload(effort);
            assert_eq!(payload["effort"], serde_json::json!(effort));
            assert_eq!(
                payload["summary"],
                serde_json::json!("auto"),
                "{effort} must request reasoning summaries"
            );
        }
    }

    #[test]
    fn reads_the_responses_reasoning_payload_shape() {
        assert_eq!(
            request_reasoning_effort(&serde_json::json!({"reasoning": {"effort": "high"}})),
            Some("high")
        );
        assert_eq!(request_reasoning_effort(&serde_json::json!({})), None);
        // Malformed shapes must not panic or be mistaken for an effort.
        assert_eq!(
            request_reasoning_effort(&serde_json::json!({"reasoning": "high"})),
            None
        );
        assert_eq!(
            request_reasoning_effort(&serde_json::json!({"reasoning": {"effort": 3}})),
            None
        );
    }

    #[test]
    fn ws_completion_budget_scales_with_reasoning_effort() {
        let base = effective_ws_completion_timeout_secs(&serde_json::json!({"model": "gpt-5.6"}));
        assert!(base > 0);

        // A request with an ordinary effort keeps the base budget.
        assert_eq!(
            effective_ws_completion_timeout_secs(
                &serde_json::json!({"reasoning": {"effort": "low", "summary": "auto"}})
            ),
            base
        );

        // Max effort can think silently far longer than an ordinary turn, so the
        // budget must grow rather than killing the stream mid-thought.
        let max_effort = effective_ws_completion_timeout_secs(
            &serde_json::json!({"reasoning": {"effort": "max", "summary": "auto"}}),
        );
        assert!(
            max_effort > base,
            "max effort budget {max_effort}s should exceed base {base}s"
        );
        assert!(
            effective_ws_completion_timeout_secs(
                &serde_json::json!({"reasoning": {"effort": "xhigh"}})
            ) > base
        );
    }

    #[test]
    fn https_and_ws_budgets_agree_on_effort_ordering() {
        // The two transports must not disagree about which requests get more
        // headroom, or a model that survives on HTTPS dies on websockets.
        let low = serde_json::json!({"reasoning": {"effort": "low"}});
        let max = serde_json::json!({"reasoning": {"effort": "max"}});
        assert!(effective_https_idle_timeout(&max) > effective_https_idle_timeout(&low));
        assert!(
            effective_ws_completion_timeout_secs(&max) > effective_ws_completion_timeout_secs(&low)
        );
    }
}
