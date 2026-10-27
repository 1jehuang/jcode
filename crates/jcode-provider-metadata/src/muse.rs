//! Meta Muse provider metadata: pay-as-you-go Model API and the native
//! Muse subscription device flow.
//!
//! The two entries are intentionally distinct:
//! - `meta-muse`: pay-as-you-go Meta Model API key (`META_MUSE_API_KEY`).
//! - `muse-subscription`: native subscription device flow owned by
//!   `jcode-base::auth::muse`; credentials live in that module's own JSON
//!   store and are resolved through
//!   `load_openai_compatible_profile_api_key`, which pins the exact
//!   id/api_base/api_key_env/env_file tuple so no other Meta key can masquerade
//!   as the subscription credential. The local `muse` CLI is never required,
//!   and the bare `muse` alias stays with pay-as-you-go `meta-muse`.

use super::{
    LoginProviderAuthKind, LoginProviderAuthStateKey, LoginProviderDescriptor,
    LoginProviderSurfaceOrder, LoginProviderTarget, OpenAiCompatibleProfile,
};

pub const META_MUSE_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "meta-muse",
    display_name: "Meta Model API",
    api_base: "https://api.meta.ai/v1",
    api_key_env: "META_MUSE_API_KEY",
    env_file: "meta-muse.env",
    setup_url: "https://dev.meta.ai/",
    default_model: Some("muse-spark-1.2"),
    requires_api_key: true,
};

pub const MUSE_SUBSCRIPTION_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "muse-subscription",
    display_name: "Muse subscription",
    api_base: "https://api.meta.ai/v1",
    api_key_env: "JCODE_MUSE_SUBSCRIPTION_API_KEY",
    env_file: "muse-subscription.env",
    setup_url: "https://dev.meta.ai/",
    // STANDARD tier default. The contributor variant must never be selected
    // silently (training/privacy implications); the user opts into it
    // explicitly.
    default_model: Some("muse-spark-1.3"),
    requires_api_key: true,
};

pub const META_MUSE_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "meta-muse",
    display_name: "Meta Model API",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["meta", "muse", "muse-spark", "meta-model-api", "meta-ai"],
    menu_detail: "OpenAI-compatible Meta Model API",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(META_MUSE_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(38), Some(38), Some(38), Some(38), Some(38)),
};

/// Native Muse subscription login via device flow.
///
/// Shares the `OpenAiCompatible` target so no new exhaustive enum variant is
/// needed; login dispatch special-cases on `profile.id == "muse-subscription"`
/// (CLI `login_openai_compatible_flow`, scriptable begin/complete/cancel, TUI
/// `start_openai_compatible_profile_login`). Only `muse-sub` is offered as an
/// alias here.
pub const MUSE_SUBSCRIPTION_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "muse-subscription",
    display_name: "Muse subscription",
    auth_kind: LoginProviderAuthKind::DeviceCode,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "device code",
    aliases: &["muse-sub"],
    menu_detail: "native Muse subscription device flow",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(MUSE_SUBSCRIPTION_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(41), Some(41), Some(41), Some(41), Some(41)),
};
