use super::*;

pub(crate) fn is_chatgpt_web_model(model: &str) -> bool {
    model.trim() == CHATGPT_WEB_MODEL
}

pub(super) fn model_supports_image_generation(model_id: &str) -> bool {
    !model_id.to_ascii_lowercase().contains("codex")
}

/// Whether the hosted `image_generation` tool can be attached for `model_id`.
///
/// The Responses backend only exposes `image_generation` to general
/// ChatGPT/GPT models. Codex models (ids containing `codex`) reject unknown
/// hosted tools, so they must not receive it. See issue #369.
/// Responses `additional_tools` / deferred tool loading requires gpt-5.4 or
/// newer. Unknown model families stay eager, which is always correct.
pub(super) fn model_supports_additional_tools(model_id: &str) -> bool {
    let lower = model_id.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("gpt-") else {
        return false;
    };
    let version: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut parts = version.split('.').filter(|p| !p.is_empty());
    let Some(major) = parts.next() else {
        return false;
    };
    let Ok(major) = major.parse::<u32>() else {
        return false;
    };
    let Some(minor) = parts.next() else {
        return major > 5;
    };
    match minor.parse::<u32>() {
        Ok(minor) => major > 5 || (major == 5 && minor >= 4),
        // Unknown minor versions keep the previous major-only capability rule.
        Err(_) => major > 5,
    }
}
