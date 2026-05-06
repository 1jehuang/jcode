pub(crate) use jcode_provider_openai::{
    OPENAI_ENCRYPTED_CONTENT_SAFE_MAX_CHARS, build_tools,
    is_openai_encrypted_content_too_large_error, openai_encrypted_content_fallback_summary,
    openai_encrypted_content_is_sendable,
};

use crate::message::Message as ChatMessage;
use jcode_provider_openai::OpenAiRequestLogLevel;
use serde_json::Value;

pub(crate) fn build_responses_input(messages: &[ChatMessage]) -> Vec<Value> {
    jcode_provider_openai::build_responses_input_with_logger(messages, |level, message| match level
    {
        OpenAiRequestLogLevel::Info => crate::logging::info(message),
        OpenAiRequestLogLevel::Warn => crate::logging::warn(message),
    })
}
