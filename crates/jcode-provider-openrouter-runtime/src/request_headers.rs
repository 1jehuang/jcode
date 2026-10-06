//! Per-host request headers for OpenAI-compatible coding endpoints (Kimi
//! coding agents, OpenCode Go/Zen session ids).

pub(crate) const KIMI_CODING_USER_AGENT: &str = "claude-cli/1.0.0";
pub(crate) const KIMI_CODING_X_APP: &str = "cli";

pub(crate) fn is_kimi_coding_api_base(api_base: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(api_base) else {
        return false;
    };
    matches!(url.host_str(), Some("api.kimi.com"))
        && url.path().trim_end_matches('/').starts_with("/coding")
}

pub(crate) fn is_coding_agent_api_base(api_base: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(api_base) else {
        return false;
    };
    let host = url.host_str().unwrap_or_default();
    let path = url.path().trim_end_matches('/');
    is_kimi_coding_api_base(api_base)
        || host == "coding.dashscope.aliyuncs.com"
        || host == "coding-intl.dashscope.aliyuncs.com"
        || (host == "api.z.ai" && path.starts_with("/api/coding/paas"))
}

pub(crate) fn is_kimi_model_name(model: &str) -> bool {
    model.to_ascii_lowercase().contains("kimi")
}

pub(crate) fn should_send_kimi_coding_agent_headers(api_base: &str, model: Option<&str>) -> bool {
    is_coding_agent_api_base(api_base) || model.map(is_kimi_model_name).unwrap_or(false)
}

pub(crate) fn apply_kimi_coding_agent_headers(
    req: reqwest::RequestBuilder,
    api_base: &str,
    model: Option<&str>,
) -> reqwest::RequestBuilder {
    if should_send_kimi_coding_agent_headers(api_base, model) {
        req.header("User-Agent", KIMI_CODING_USER_AGENT)
            .header("x-app", KIMI_CODING_X_APP)
    } else {
        req
    }
}

/// Hosts that require the `x-opencode-session` header (issue #1167).
pub(crate) fn is_opencode_api_base(api_base: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(api_base) else {
        return false;
    };
    matches!(
        url.host_str(),
        Some(host) if host == "opencode.ai" || host.ends_with(".opencode.ai")
    )
}

pub(crate) fn new_conversation_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// OpenCode Go/Zen require a stable per-conversation `x-opencode-session`
/// header on inference requests (rejected from 2026-09-05 without it).
pub(crate) fn apply_opencode_session_header(
    req: reqwest::RequestBuilder,
    api_base: &str,
    conversation_id: &str,
) -> reqwest::RequestBuilder {
    if is_opencode_api_base(api_base) {
        req.header(OPENCODE_SESSION_HEADER, conversation_id)
    } else {
        req
    }
}

pub(crate) const OPENCODE_SESSION_HEADER: &str = "x-opencode-session";
