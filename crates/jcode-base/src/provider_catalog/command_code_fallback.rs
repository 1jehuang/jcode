//! Command Code catalog fallbacks and context limits.

pub(crate) fn static_models() -> Vec<String> {
    jcode_provider_metadata::command_code::static_model_ids()
}

pub(crate) fn context_limit(model: &str) -> Option<usize> {
    jcode_provider_metadata::command_code::static_context_limit(model)
}
