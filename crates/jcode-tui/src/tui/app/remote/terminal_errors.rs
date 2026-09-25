//! Remote turn errors that no resend can fix, so every automatic retry path
//! must stop at once instead of spending its budget on a guaranteed failure.

use super::*;
use crate::tui::app as app_mod;

/// Fail fast on a deterministic request failure. Returns `true` when the error
/// was handled here and the caller must not schedule any retry.
pub(super) fn fail_fast_on_deterministic_error(
    app: &mut App,
    message: &str,
    failed_fallback_payload: Option<app_mod::FallbackResendPayload>,
) -> bool {
    let (notice, status) = if let Some(label) =
        jcode_provider_core::failover::content_filter_block_label(message)
    {
        // A gateway guardrail matched text in the conversation. Every turn
        // resends the full history, so the same block recurs until that text
        // leaves the context or the guardrail changes.
        (
            format!(
                "🛑 Not retrying: the provider's content filter blocked this request as {label}. \
                 The match is somewhere in the conversation history, so every resend is blocked too. \
                 Compact or start a new session without that text, switch provider with /model, \
                 or adjust the guardrail (for OpenRouter: workspace guardrail content filters)."
            ),
            "Stopped: content filter block",
        )
    } else if crate::tui::app::commands::is_fatal_model_endpoint_error(message) {
        // Deterministic model/endpoint-capability failures (e.g. Volcengine
        // Ark's coding-plan endpoint returning 404 UnsupportedModel, or a
        // model-not-found) can never succeed by resending the identical
        // request (#387).
        (
            "🛑 Not retrying: the model is not valid for the configured endpoint (e.g. an Ark coding-plan endpoint rejecting a model without the coding plan feature, or a model-not-found). Check the model name and base URL (the coding endpoint `/api/coding/v3` only accepts coding-plan models; use `/api/v3` otherwise), then send again.".to_string(),
            "Stopped: model/endpoint mismatch",
        )
    } else {
        return false;
    };

    app.clear_pending_remote_retry();
    if app.auto_poke_incomplete_todos {
        crate::tui::app::commands::stop_auto_poke_for_non_retryable_error(app, message);
    }
    app.push_display_message(DisplayMessage::system(notice));
    app.set_status_notice(status);
    app.restore_failed_input_to_box();
    // Switching route is a real fix for both: another model/endpoint, or a
    // provider without the blocking guardrail.
    app.offer_fallback_after_error_with_payload(message, failed_fallback_payload);
    true
}
