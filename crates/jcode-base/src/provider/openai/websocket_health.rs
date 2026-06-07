pub(super) use jcode_provider_openai::websocket_health::{
    WEBSOCKET_COMPLETION_TIMEOUT_SECS, WEBSOCKET_FALLBACK_NOTICE,
    WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS, classify_websocket_fallback_reason,
    is_stream_activity_event, is_websocket_activity_payload, is_websocket_fallback_notice,
    is_websocket_first_activity_payload, record_websocket_fallback, record_websocket_success,
    summarize_websocket_fallback_reason, websocket_activity_timeout_kind,
    websocket_cooldown_remaining, websocket_next_activity_timeout_secs,
};

#[cfg(test)]
pub(super) use jcode_provider_openai::websocket_health::{
    WEBSOCKET_MODEL_COOLDOWN_BASE_SECS, WEBSOCKET_MODEL_COOLDOWN_MAX_SECS, WebsocketFallbackReason,
    clear_websocket_cooldown, normalize_transport_model, set_websocket_cooldown,
    websocket_cooldown_for_streak, websocket_remaining_timeout_secs,
};
