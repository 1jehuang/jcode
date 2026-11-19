//! LLMTR (https://llmtr.com) OpenAI-compatible gateway.
//!
//! Chat goes to `{api_base}/chat/completions`. Model ids such as
//! `llmtr/qwen3-8-27b` are the ids the API expects; only a colon prefix
//! (`llmtr:<id>`) selects this profile. `GET /v1/models` is public and
//! answers 200 for any bearer, so it is a catalog, not a key check.
//! The free default caps completion at 16384 tokens (`top_provider.max_completion_tokens`).

use super::{
    LoginProviderAuthKind, LoginProviderAuthStateKey, LoginProviderDescriptor,
    LoginProviderSurfaceOrder, LoginProviderTarget, OpenAiCompatibleProfile,
};

pub const LLMTR_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "llmtr",
    display_name: "LLMTR",
    api_base: "https://llmtr.com/v1",
    api_key_env: "LLMTR_API_KEY",
    env_file: "llmtr.env",
    setup_url: "https://llmtr.com/llms.txt",
    default_model: Some("qwen/qwen3.8-27b-free"),
    requires_api_key: true,
};

pub const LLMTR_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "llmtr",
    display_name: "LLMTR",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["llmtr.com"],
    menu_detail: "OpenAI-compatible chat API",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(LLMTR_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(38), Some(38), Some(38), Some(38), Some(38)),
};
