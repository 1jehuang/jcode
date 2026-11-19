//! Per-model wire protocol selection for OpenAI-compatible endpoints.
//!
//! A single endpoint can expose several protocols at once. OpenCode Go is the
//! motivating case (issue #1224): most of its catalog answers on
//! `/chat/completions`, a small allowlist answers on `/responses`, and the
//! MiniMax/Qwen models only answer on the Anthropic `/messages` route. The
//! protocol therefore has to be chosen per request from the configured profile
//! and model, not hard-coded per runtime.

use super::opencode_go_responses::model_uses_responses_api;

/// The streaming protocol used for one request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ApiProtocol {
    /// OpenAI-compatible `POST {base}/chat/completions`.
    #[default]
    ChatCompletions,
    /// OpenAI Responses `POST {base}/responses` (OpenCode Go / Azure subset).
    Responses,
    /// Anthropic Messages `POST {base}/messages`.
    Messages,
}

impl ApiProtocol {
    /// Path appended to the configured api_base for this protocol.
    pub(crate) fn route(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat/completions",
            Self::Responses => "responses",
            Self::Messages => "messages",
        }
    }

    /// Short label used by the canonical-input fingerprint log.
    pub(crate) fn fingerprint_label(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::Responses => "responses",
            Self::Messages => "anthropic_messages",
        }
    }

    /// User-facing request description used in transport errors.
    pub(crate) fn request_kind(self) -> &'static str {
        match self {
            Self::ChatCompletions => "OpenAI-compatible chat",
            Self::Responses => "OpenCode Go Responses",
            Self::Messages => "OpenCode Go Messages",
        }
    }
}

/// Choose the protocol for `model` on the endpoint described by `api_base` and
/// `profile_id`.
///
/// `/responses` wins over `/messages` so the native Muse route keeps its
/// existing behavior; `/chat/completions` stays the fallback for every model
/// that is not explicitly listed, which is what keeps ordinary OpenRouter and
/// user-declared OpenAI-compatible profiles unchanged.
pub(crate) fn select_protocol(
    api_base: &str,
    profile_id: Option<&str>,
    model: &str,
) -> ApiProtocol {
    if model_uses_responses_api(api_base, profile_id, model) {
        return ApiProtocol::Responses;
    }
    if model_uses_messages_api(api_base, profile_id, model) {
        return ApiProtocol::Messages;
    }
    ApiProtocol::ChatCompletions
}

/// Whether this endpoint/model pair speaks the Anthropic Messages protocol.
///
/// Protocol identity belongs to the model allowlist, but only for the OpenCode
/// Go gateway: the same model id served by another aggregator must keep using
/// the route that profile is configured for.
fn model_uses_messages_api(api_base: &str, profile_id: Option<&str>, model: &str) -> bool {
    let is_go_profile = profile_id.is_some_and(|id| id.eq_ignore_ascii_case("opencode-go"));
    let is_go_endpoint = api_base
        .to_ascii_lowercase()
        .contains("opencode.ai/zen/go/");
    (is_go_profile || is_go_endpoint)
        && jcode_base::provider_catalog::model_uses_messages_api(model)
}

/// Path this endpoint serves `model` on, without the configured api_base.
///
/// Public so callers outside this crate (notably `jcode-base`'s provider
/// tests) can assert the route a runtime selects instead of duplicating the
/// model allowlists. Production code picks the route through
/// [`select_protocol`] directly.
pub fn openai_compatible_route_path(
    api_base: &str,
    profile_id: Option<&str>,
    model: &str,
) -> &'static str {
    select_protocol(api_base, profile_id, model).route()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GO_BASE: &str = "https://opencode.ai/zen/go/v1";

    /// Issue #1224: each OpenCode Go model must land on the endpoint its
    /// gateway actually serves, with chat completions as the untouched default.
    #[test]
    fn opencode_go_protocol_selection_is_per_model() {
        assert_eq!(
            select_protocol(GO_BASE, Some("opencode-go"), "muse-spark-1.3-contributor"),
            ApiProtocol::Responses
        );
        assert_eq!(
            select_protocol(GO_BASE, Some("opencode-go"), "minimax-m2.7"),
            ApiProtocol::Messages
        );
        assert_eq!(
            select_protocol(GO_BASE, Some("opencode-go"), "glm-5"),
            ApiProtocol::ChatCompletions
        );
    }

    /// The endpoint alone is enough: a restored session can lose the profile id
    /// but not the api_base, and that must not silently downgrade the route.
    #[test]
    fn opencode_go_messages_route_survives_a_missing_profile_id() {
        assert_eq!(
            select_protocol(GO_BASE, None, "qwen3.8-max"),
            ApiProtocol::Messages
        );
    }

    /// Non-Go endpoints never get the Messages route, even for a model id that
    /// appears in the OpenCode Go allowlist. OpenCode Zen (non-Go) is included:
    /// only `/zen/go/` is multi-protocol.
    #[test]
    fn opencode_go_messages_allowlist_is_scoped_to_the_go_gateway() {
        for (api_base, profile_id) in [
            ("https://openrouter.ai/api/v1", None),
            ("https://api.somewhere.test/v1", Some("custom")),
            ("https://opencode.ai/zen/v1", Some("opencode")),
        ] {
            assert_eq!(
                select_protocol(api_base, profile_id, "minimax-m2.7"),
                ApiProtocol::ChatCompletions,
                "{api_base} must not use /messages"
            );
        }
    }

    #[test]
    fn protocol_routes_and_fingerprint_labels_are_stable() {
        assert_eq!(ApiProtocol::ChatCompletions.route(), "chat/completions");
        assert_eq!(ApiProtocol::Responses.route(), "responses");
        assert_eq!(ApiProtocol::Messages.route(), "messages");
        assert_eq!(
            ApiProtocol::Messages.fingerprint_label(),
            "anthropic_messages"
        );
        assert_eq!(ApiProtocol::default(), ApiProtocol::ChatCompletions);
    }
}
