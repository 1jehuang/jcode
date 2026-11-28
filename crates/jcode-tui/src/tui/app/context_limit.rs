pub(in crate::tui::app) fn is_context_limit_error(error: &str) -> bool {
    if crate::provider::openai_request::is_openai_encrypted_content_too_large_error(error) {
        return true;
    }
    let lower = error.to_lowercase();
    lower.contains("context length")
        || lower.contains("context_length_exceeded")
        || lower.contains("prompt_too_large")
        || lower.contains("context is too large")
        || lower.contains("context window")
        || lower.contains("maximum context")
        || lower.contains("max context")
        || lower.contains("token limit")
        || lower.contains("too many tokens")
        || lower.contains("prompt is too long")
        || lower.contains("input is too long")
        || lower.contains("request too large")
        || lower.contains("length limit")
        || lower.contains("maximum tokens")
        || (lower.contains("exceeded") && lower.contains("tokens"))
}

#[cfg(test)]
#[path = "context_limit_tests.rs"]
mod tests;
