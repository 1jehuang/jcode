use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginProviderAuthKind {
    OAuth,
    ApiKey,
    DeviceCode,
    Cli,
    Hybrid,
}

impl LoginProviderAuthKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::OAuth => "OAuth",
            Self::ApiKey => "API key",
            Self::DeviceCode => "device code",
            Self::Cli => "CLI",
            Self::Hybrid => "API key / CLI",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginProviderTarget {
    Jcode,
    Claude,
    OpenAi,
    OpenRouter,
    Azure,
    OpenAiCompatible(OpenAiCompatibleProfile),
    Cursor,
    Copilot,
    Gemini,
    Antigravity,
    Google,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginProviderAuthStateKey {
    Jcode,
    Anthropic,
    OpenAi,
    Azure,
    OpenRouterLike,
    Copilot,
    Gemini,
    Antigravity,
    Cursor,
    Google,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginProviderSurface {
    CliLogin,
    TuiLogin,
    ServerBootstrap,
    AutoInit,
    AuthStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoginProviderSurfaceOrder {
    pub cli_login: Option<u8>,
    pub tui_login: Option<u8>,
    pub server_bootstrap: Option<u8>,
    pub auto_init: Option<u8>,
    pub auth_status: Option<u8>,
}

impl LoginProviderSurfaceOrder {
    pub const fn new(
        cli_login: Option<u8>,
        tui_login: Option<u8>,
        server_bootstrap: Option<u8>,
        auto_init: Option<u8>,
        auth_status: Option<u8>,
    ) -> Self {
        Self {
            cli_login,
            tui_login,
            server_bootstrap,
            auto_init,
            auth_status,
        }
    }

    pub const fn for_surface(self, surface: LoginProviderSurface) -> Option<u8> {
        match surface {
            LoginProviderSurface::CliLogin => self.cli_login,
            LoginProviderSurface::TuiLogin => self.tui_login,
            LoginProviderSurface::ServerBootstrap => self.server_bootstrap,
            LoginProviderSurface::AutoInit => self.auto_init,
            LoginProviderSurface::AuthStatus => self.auth_status,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoginProviderDescriptor {
    pub id: &'static str,
    pub display_name: &'static str,
    pub auth_kind: LoginProviderAuthKind,
    pub auth_state_key: LoginProviderAuthStateKey,
    pub auth_status_method: &'static str,
    pub aliases: &'static [&'static str],
    pub menu_detail: &'static str,
    pub recommended: bool,
    pub target: LoginProviderTarget,
    pub order: LoginProviderSurfaceOrder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenAiCompatibleProfile {
    pub id: &'static str,
    pub display_name: &'static str,
    pub api_base: &'static str,
    pub api_key_env: &'static str,
    pub env_file: &'static str,
    pub setup_url: &'static str,
    pub default_model: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedOpenAiCompatibleProfile {
    pub id: String,
    pub display_name: String,
    pub api_base: String,
    pub api_key_env: String,
    pub env_file: String,
    pub setup_url: String,
    pub default_model: Option<String>,
}

pub const OPENCODE_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "opencode",
    display_name: "OpenCode Zen",
    api_base: "https://opencode.ai/zen/v1",
    api_key_env: "OPENCODE_API_KEY",
    env_file: "opencode.env",
    setup_url: "https://opencode.ai/docs/providers#opencode-zen",
    default_model: Some("qwen/qwen3-coder-plus"),
};

pub const OPENCODE_GO_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "opencode-go",
    display_name: "OpenCode Go",
    api_base: "https://opencode.ai/zen/go/v1",
    api_key_env: "OPENCODE_GO_API_KEY",
    env_file: "opencode-go.env",
    setup_url: "https://opencode.ai/docs/providers#opencode-go",
    default_model: Some("THUDM/GLM-4.5"),
};

pub const ZAI_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "zai",
    display_name: "Z.AI Coding",
    api_base: "https://api.z.ai/api/coding/paas/v4",
    api_key_env: "ZAI_API_KEY",
    env_file: "zai.env",
    setup_url: "https://docs.z.ai/guides/develop/openai/introduction",
    default_model: Some("glm-4.5"),
};

pub const CHUTES_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "chutes",
    display_name: "Chutes",
    api_base: "https://llm.chutes.ai/v1",
    api_key_env: "CHUTES_API_KEY",
    env_file: "chutes.env",
    setup_url: "https://chutes.ai",
    default_model: Some("Qwen/Qwen3-Coder-480B-A35B-Instruct"),
};

pub const CEREBRAS_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "cerebras",
    display_name: "Cerebras",
    api_base: "https://api.cerebras.ai/v1",
    api_key_env: "CEREBRAS_API_KEY",
    env_file: "cerebras.env",
    setup_url: "https://inference-docs.cerebras.ai/introduction",
    default_model: Some("qwen-3-coder-480b"),
};

pub const OPENAI_COMPAT_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "openai-compatible",
    display_name: "OpenAI-compatible",
    api_base: "https://api.openai.com/v1",
    api_key_env: "OPENAI_COMPAT_API_KEY",
    env_file: "openai-compatible.env",
    setup_url: "https://opencode.ai/docs/providers#custom-providers",
    default_model: None,
};

const OPENAI_COMPAT_PROFILES: [OpenAiCompatibleProfile; 6] = [
    OPENCODE_PROFILE,
    OPENCODE_GO_PROFILE,
    ZAI_PROFILE,
    CHUTES_PROFILE,
    CEREBRAS_PROFILE,
    OPENAI_COMPAT_PROFILE,
];

pub const CLAUDE_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "claude",
    display_name: "Anthropic/Claude",
    auth_kind: LoginProviderAuthKind::OAuth,
    auth_state_key: LoginProviderAuthStateKey::Anthropic,
    auth_status_method: "OAuth / API key",
    aliases: &["anthropic"],
    menu_detail: "requires Claude Pro or Max subscription",
    recommended: true,
    target: LoginProviderTarget::Claude,
    order: LoginProviderSurfaceOrder::new(Some(1), Some(1), Some(1), Some(1), Some(1)),
};

pub const JCODE_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "jcode",
    display_name: "Jcode Subscription",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::Jcode,
    auth_status_method: "API key",
    aliases: &["subscription", "jcode-subscription"],
    menu_detail: "curated jcode subscription models",
    recommended: false,
    target: LoginProviderTarget::Jcode,
    order: LoginProviderSurfaceOrder::new(Some(3), Some(3), Some(3), Some(3), Some(3)),
};

pub const OPENAI_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "openai",
    display_name: "OpenAI",
    auth_kind: LoginProviderAuthKind::OAuth,
    auth_state_key: LoginProviderAuthStateKey::OpenAi,
    auth_status_method: "OAuth / API key",
    aliases: &[],
    menu_detail: "requires ChatGPT Plus or Pro subscription",
    recommended: true,
    target: LoginProviderTarget::OpenAi,
    order: LoginProviderSurfaceOrder::new(Some(2), Some(2), Some(2), Some(2), Some(2)),
};

pub const OPENROUTER_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "openrouter",
    display_name: "OpenRouter",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &[],
    menu_detail: "API key, pay-per-token, 200+ models",
    recommended: false,
    target: LoginProviderTarget::OpenRouter,
    order: LoginProviderSurfaceOrder::new(Some(4), Some(3), Some(4), Some(3), Some(3)),
};

pub const AZURE_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "azure",
    display_name: "Azure OpenAI",
    auth_kind: LoginProviderAuthKind::Hybrid,
    auth_state_key: LoginProviderAuthStateKey::Azure,
    auth_status_method: "Entra ID / API key",
    aliases: &["azure-openai", "azure_openai", "aoai"],
    menu_detail: "Microsoft Entra ID or Azure OpenAI API key",
    recommended: false,
    target: LoginProviderTarget::Azure,
    order: LoginProviderSurfaceOrder::new(Some(5), None, None, None, Some(4)),
};

pub const OPENCODE_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "opencode",
    display_name: "OpenCode Zen",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["opencode-zen", "zen"],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(OPENCODE_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(5), Some(4), Some(5), Some(4), Some(4)),
};

pub const OPENCODE_GO_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "opencode-go",
    display_name: "OpenCode Go",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["opencodego"],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(OPENCODE_GO_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(6), Some(5), Some(6), Some(5), Some(5)),
};

pub const ZAI_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "zai",
    display_name: "Z.AI Coding",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["z.ai", "z-ai", "zai-coding"],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(ZAI_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(7), Some(6), Some(7), Some(6), Some(6)),
};

pub const CHUTES_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "chutes",
    display_name: "Chutes",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &[],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(CHUTES_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(8), Some(7), Some(8), Some(7), Some(7)),
};

pub const CEREBRAS_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "cerebras",
    display_name: "Cerebras",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["cerebrascode", "cerberascode"],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(CEREBRAS_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(9), Some(8), Some(9), Some(8), Some(8)),
};

pub const OPENAI_COMPAT_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "openai-compatible",
    display_name: "OpenAI-compatible",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["openai_compatible", "compat", "custom"],
    menu_detail: "custom OpenAI-compatible endpoint",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(OPENAI_COMPAT_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(10), Some(9), None, None, Some(9)),
};

pub const CURSOR_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "cursor",
    display_name: "Cursor",
    auth_kind: LoginProviderAuthKind::Hybrid,
    auth_state_key: LoginProviderAuthStateKey::Cursor,
    auth_status_method: "API key / CLI",
    aliases: &[],
    menu_detail: "browser login or API key",
    recommended: false,
    target: LoginProviderTarget::Cursor,
    order: LoginProviderSurfaceOrder::new(Some(11), Some(12), None, Some(9), Some(12)),
};

pub const COPILOT_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "copilot",
    display_name: "GitHub Copilot",
    auth_kind: LoginProviderAuthKind::DeviceCode,
    auth_state_key: LoginProviderAuthStateKey::Copilot,
    auth_status_method: "device code",
    aliases: &[],
    menu_detail: "GitHub device flow",
    recommended: false,
    target: LoginProviderTarget::Copilot,
    order: LoginProviderSurfaceOrder::new(Some(3), Some(10), Some(3), Some(10), Some(10)),
};

pub const GEMINI_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "gemini",
    display_name: "Google Gemini",
    auth_kind: LoginProviderAuthKind::OAuth,
    auth_state_key: LoginProviderAuthStateKey::Gemini,
    auth_status_method: "OAuth",
    aliases: &[],
    menu_detail: "Google Gemini Code Assist OAuth login",
    recommended: false,
    target: LoginProviderTarget::Gemini,
    order: LoginProviderSurfaceOrder::new(Some(13), Some(11), Some(4), Some(11), Some(13)),
};

pub const ANTIGRAVITY_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "antigravity",
    display_name: "Antigravity",
    auth_kind: LoginProviderAuthKind::Cli,
    auth_state_key: LoginProviderAuthStateKey::Antigravity,
    auth_status_method: "CLI",
    aliases: &[],
    menu_detail: "CLI login",
    recommended: false,
    target: LoginProviderTarget::Antigravity,
    order: LoginProviderSurfaceOrder::new(Some(12), Some(12), None, Some(12), Some(12)),
};

pub const GOOGLE_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "google",
    display_name: "Google/Gmail",
    auth_kind: LoginProviderAuthKind::OAuth,
    auth_state_key: LoginProviderAuthStateKey::Google,
    auth_status_method: "OAuth",
    aliases: &["gmail"],
    menu_detail: "read, draft, and send emails",
    recommended: false,
    target: LoginProviderTarget::Google,
    order: LoginProviderSurfaceOrder::new(Some(13), None, None, None, None),
};

const LOGIN_PROVIDERS: [LoginProviderDescriptor; 16] = [
    CLAUDE_LOGIN_PROVIDER,
    OPENAI_LOGIN_PROVIDER,
    JCODE_LOGIN_PROVIDER,
    OPENROUTER_LOGIN_PROVIDER,
    AZURE_LOGIN_PROVIDER,
    OPENCODE_LOGIN_PROVIDER,
    OPENCODE_GO_LOGIN_PROVIDER,
    ZAI_LOGIN_PROVIDER,
    CHUTES_LOGIN_PROVIDER,
    CEREBRAS_LOGIN_PROVIDER,
    OPENAI_COMPAT_LOGIN_PROVIDER,
    CURSOR_LOGIN_PROVIDER,
    COPILOT_LOGIN_PROVIDER,
    GEMINI_LOGIN_PROVIDER,
    ANTIGRAVITY_LOGIN_PROVIDER,
    GOOGLE_LOGIN_PROVIDER,
];

pub fn openai_compatible_profiles() -> &'static [OpenAiCompatibleProfile] {
    &OPENAI_COMPAT_PROFILES
}

pub fn login_providers() -> &'static [LoginProviderDescriptor] {
    &LOGIN_PROVIDERS
}

fn login_providers_for_surface(surface: LoginProviderSurface) -> Vec<LoginProviderDescriptor> {
    let mut providers = login_providers()
        .iter()
        .copied()
        .filter(|provider| provider.order.for_surface(surface).is_some())
        .collect::<Vec<_>>();
    providers.sort_by_key(|provider| provider.order.for_surface(surface).unwrap_or(u8::MAX));
    providers
}

pub fn cli_login_providers() -> Vec<LoginProviderDescriptor> {
    login_providers_for_surface(LoginProviderSurface::CliLogin)
}

pub fn tui_login_providers() -> Vec<LoginProviderDescriptor> {
    login_providers_for_surface(LoginProviderSurface::TuiLogin)
}

pub fn server_bootstrap_login_providers() -> Vec<LoginProviderDescriptor> {
    login_providers_for_surface(LoginProviderSurface::ServerBootstrap)
}

pub fn auto_init_login_providers() -> Vec<LoginProviderDescriptor> {
    login_providers_for_surface(LoginProviderSurface::AutoInit)
}

pub fn auth_status_login_providers() -> Vec<LoginProviderDescriptor> {
    login_providers_for_surface(LoginProviderSurface::AuthStatus)
}

pub fn resolve_login_provider(input: &str) -> Option<LoginProviderDescriptor> {
    let normalized = normalize_provider_input(input)?;
    login_providers().iter().copied().find(|provider| {
        provider.id == normalized || provider.aliases.iter().any(|alias| *alias == normalized)
    })
}

pub fn resolve_login_selection(
    input: &str,
    providers: &[LoginProviderDescriptor],
) -> Option<LoginProviderDescriptor> {
    let trimmed = input.trim();
    if let Ok(index) = trimmed.parse::<usize>() {
        return index
            .checked_sub(1)
            .and_then(|idx| providers.get(idx))
            .copied();
    }

    let provider = resolve_login_provider(trimmed)?;
    providers
        .iter()
        .copied()
        .find(|candidate| candidate.id == provider.id)
}

pub fn resolve_openai_compatible_profile(
    profile: OpenAiCompatibleProfile,
) -> ResolvedOpenAiCompatibleProfile {
    let mut resolved = ResolvedOpenAiCompatibleProfile {
        id: profile.id.to_string(),
        display_name: profile.display_name.to_string(),
        api_base: profile.api_base.to_string(),
        api_key_env: profile.api_key_env.to_string(),
        env_file: profile.env_file.to_string(),
        setup_url: profile.setup_url.to_string(),
        default_model: profile.default_model.map(ToString::to_string),
    };

    if profile.id != OPENAI_COMPAT_PROFILE.id {
        return resolved;
    }

    if let Some(base) = env_override("JCODE_OPENAI_COMPAT_API_BASE") {
        if let Some(normalized) = normalize_api_base(&base) {
            resolved.api_base = normalized;
        } else {
            eprintln!(
                "Warning: ignoring invalid JCODE_OPENAI_COMPAT_API_BASE '{}'. Use https://... (or http://localhost).",
                base
            );
        }
    }

    if let Some(key_name) = env_override("JCODE_OPENAI_COMPAT_API_KEY_NAME") {
        if is_safe_env_key_name(&key_name) {
            resolved.api_key_env = key_name;
        } else {
            eprintln!(
                "Warning: ignoring invalid JCODE_OPENAI_COMPAT_API_KEY_NAME '{}'.",
                key_name
            );
        }
    }

    if let Some(env_file) = env_override("JCODE_OPENAI_COMPAT_ENV_FILE") {
        if is_safe_env_file_name(&env_file) {
            resolved.env_file = env_file;
        } else {
            eprintln!(
                "Warning: ignoring invalid JCODE_OPENAI_COMPAT_ENV_FILE '{}'.",
                env_file
            );
        }
    }

    if let Some(setup_url) = env_override("JCODE_OPENAI_COMPAT_SETUP_URL") {
        resolved.setup_url = setup_url;
    }

    if let Some(model) = env_override("JCODE_OPENAI_COMPAT_DEFAULT_MODEL") {
        resolved.default_model = Some(model);
    }

    resolved
}

pub fn apply_openai_compatible_profile_env(profile: Option<OpenAiCompatibleProfile>) {
    let vars = [
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_MODEL_CATALOG",
        "JCODE_OPENROUTER_AUTH_HEADER",
        "JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER",
        "JCODE_OPENROUTER_PROVIDER",
        "JCODE_OPENROUTER_NO_FALLBACK",
    ];

    for var in vars {
        std::env::remove_var(var);
    }

    if let Some(profile) = profile {
        let resolved = resolve_openai_compatible_profile(profile);
        std::env::set_var("JCODE_OPENROUTER_API_BASE", &resolved.api_base);
        std::env::set_var("JCODE_OPENROUTER_API_KEY_NAME", &resolved.api_key_env);
        std::env::set_var("JCODE_OPENROUTER_ENV_FILE", &resolved.env_file);
        std::env::set_var("JCODE_OPENROUTER_CACHE_NAMESPACE", &resolved.id);
        std::env::set_var("JCODE_OPENROUTER_PROVIDER_FEATURES", "0");
    }
}

pub fn openrouter_like_api_key_sources() -> Vec<(String, String)> {
    let mut sources = Vec::with_capacity(10);
    sources.push((
        "OPENROUTER_API_KEY".to_string(),
        "openrouter.env".to_string(),
    ));

    for profile in openai_compatible_profiles() {
        sources.push((
            profile.api_key_env.to_string(),
            profile.env_file.to_string(),
        ));
    }

    if let Some(source) = configured_api_key_source(
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "OPENROUTER_API_KEY",
        "openrouter.env",
    ) {
        sources.push(source);
    }

    if let Some(source) = configured_api_key_source(
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_ENV_FILE",
        OPENAI_COMPAT_PROFILE.api_key_env,
        OPENAI_COMPAT_PROFILE.env_file,
    ) {
        sources.push(source);
    }

    dedup_sources(sources)
}

pub fn configured_api_key_source(
    key_var: &str,
    file_var: &str,
    default_key: &str,
    default_file: &str,
) -> Option<(String, String)> {
    if std::env::var_os(key_var).is_none() && std::env::var_os(file_var).is_none() {
        return None;
    }

    let env_key = std::env::var(key_var)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default_key.to_string());
    let file_name = std::env::var(file_var)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default_file.to_string());

    if !is_safe_env_key_name(&env_key) {
        crate::logging::warn(&format!(
            "Ignoring invalid {}='{}' while probing auth status",
            key_var, env_key
        ));
        return None;
    }
    if !is_safe_env_file_name(&file_name) {
        crate::logging::warn(&format!(
            "Ignoring invalid {}='{}' while probing auth status",
            file_var, file_name
        ));
        return None;
    }

    Some((env_key, file_name))
}

pub fn load_api_key_from_env_or_config(env_key: &str, file_name: &str) -> Option<String> {
    if !is_safe_env_key_name(env_key) {
        crate::logging::warn(&format!(
            "Ignoring invalid API key variable name '{}' while loading credentials",
            env_key
        ));
        return None;
    }
    if !is_safe_env_file_name(file_name) {
        crate::logging::warn(&format!(
            "Ignoring invalid env file name '{}' while loading credentials",
            file_name
        ));
        return None;
    }

    if let Ok(key) = std::env::var(env_key) {
        let key = key.trim();
        if !key.is_empty() {
            return Some(key.to_string());
        }
    }

    let config_path = dirs::config_dir()?.join("jcode").join(file_name);
    crate::storage::harden_secret_file_permissions(&config_path);
    let content = std::fs::read_to_string(config_path).ok()?;
    let prefix = format!("{}=", env_key);

    for line in content.lines() {
        if let Some(key) = line.strip_prefix(&prefix) {
            let key = key.trim().trim_matches('"').trim_matches('\'');
            if !key.is_empty() {
                return Some(key.to_string());
            }
        }
    }

    None
}

pub fn load_env_value_from_env_or_config(env_key: &str, file_name: &str) -> Option<String> {
    if !is_safe_env_key_name(env_key) {
        crate::logging::warn(&format!(
            "Ignoring invalid variable name '{}' while loading config value",
            env_key
        ));
        return None;
    }
    if !is_safe_env_file_name(file_name) {
        crate::logging::warn(&format!(
            "Ignoring invalid env file name '{}' while loading config value",
            file_name
        ));
        return None;
    }

    if let Ok(value) = std::env::var(env_key) {
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }

    let config_path = dirs::config_dir()?.join("jcode").join(file_name);
    crate::storage::harden_secret_file_permissions(&config_path);
    let content = std::fs::read_to_string(config_path).ok()?;
    let prefix = format!("{}=", env_key);

    for line in content.lines() {
        if let Some(value) = line.strip_prefix(&prefix) {
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }

    None
}

pub fn is_safe_env_key_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

pub fn is_safe_env_file_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

pub fn normalize_api_base(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let parsed = reqwest::Url::parse(trimmed).ok()?;
    let scheme = parsed.scheme();
    if scheme != "https" && scheme != "http" {
        return None;
    }

    if scheme == "http" {
        let host = parsed.host_str()?.to_ascii_lowercase();
        if host != "localhost" && host != "127.0.0.1" && host != "::1" {
            return None;
        }
    }

    Some(trimmed.trim_end_matches('/').to_string())
}

fn env_override(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn normalize_provider_input(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

fn dedup_sources(sources: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::with_capacity(sources.len());
    for (env_key, env_file) in sources {
        if seen.insert((env_key.clone(), env_file.clone())) {
            deduped.push((env_key, env_file));
        }
    }
    deduped
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        vars: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        fn save(keys: &[&str]) -> Self {
            let vars = keys
                .iter()
                .map(|key| (key.to_string(), std::env::var(key).ok()))
                .collect();
            Self { vars }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.vars {
                if let Some(value) = value {
                    std::env::set_var(key, value);
                } else {
                    std::env::remove_var(key);
                }
            }
        }
    }

    #[test]
    fn matrix_profiles_have_unique_ids_and_safe_metadata() {
        let mut ids = HashSet::new();
        for profile in openai_compatible_profiles() {
            assert!(
                ids.insert(profile.id),
                "duplicate provider profile id: {}",
                profile.id
            );
            assert!(is_safe_env_key_name(profile.api_key_env));
            assert!(is_safe_env_file_name(profile.env_file));
            assert_eq!(
                normalize_api_base(profile.api_base).as_deref(),
                Some(profile.api_base)
            );
        }
    }

    #[test]
    fn matrix_login_provider_aliases_resolve_to_canonical_ids() {
        assert_eq!(
            resolve_login_provider("subscription").map(|provider| provider.id),
            Some("jcode")
        );
        assert_eq!(
            resolve_login_provider("anthropic").map(|provider| provider.id),
            Some("claude")
        );
        assert_eq!(
            resolve_login_provider("opencodego").map(|provider| provider.id),
            Some("opencode-go")
        );
        assert_eq!(
            resolve_login_provider("z.ai").map(|provider| provider.id),
            Some("zai")
        );
        assert_eq!(
            resolve_login_provider("compat").map(|provider| provider.id),
            Some("openai-compatible")
        );
        assert_eq!(
            resolve_login_provider("aoai").map(|provider| provider.id),
            Some("azure")
        );
        assert_eq!(
            resolve_login_provider("cerberascode").map(|provider| provider.id),
            Some("cerebras")
        );
        assert_eq!(
            resolve_login_provider("gmail").map(|provider| provider.id),
            Some("google")
        );
    }

    #[test]
    fn matrix_login_provider_ids_and_aliases_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for provider in login_providers() {
            assert!(
                seen.insert(provider.id),
                "duplicate login provider identifier: {}",
                provider.id
            );
            for alias in provider.aliases {
                assert!(
                    seen.insert(*alias),
                    "duplicate login provider alias: {}",
                    alias
                );
            }
        }
    }

    #[test]
    fn matrix_tui_login_selection_supports_numbers_and_names() {
        let providers = tui_login_providers();
        assert_eq!(
            resolve_login_selection("1", &providers).map(|provider| provider.id),
            Some("claude")
        );
        assert_eq!(
            resolve_login_selection("13", &providers).map(|provider| provider.id),
            Some("cursor")
        );
        assert_eq!(
            resolve_login_selection("compat", &providers).map(|provider| provider.id),
            Some("openai-compatible")
        );
        assert!(resolve_login_selection("google", &providers).is_none());
    }

    #[test]
    fn matrix_cli_login_selection_preserves_existing_order() {
        let providers = cli_login_providers();
        assert_eq!(
            resolve_login_selection("3", &providers).map(|provider| provider.id),
            Some("jcode")
        );
        assert_eq!(
            resolve_login_selection("4", &providers).map(|provider| provider.id),
            Some("copilot")
        );
        assert_eq!(
            resolve_login_selection("5", &providers).map(|provider| provider.id),
            Some("openrouter")
        );
        assert_eq!(
            resolve_login_selection("6", &providers).map(|provider| provider.id),
            Some("azure")
        );
        assert_eq!(
            resolve_login_selection("15", &providers).map(|provider| provider.id),
            Some("gemini")
        );
        assert_eq!(
            resolve_login_selection("16", &providers).map(|provider| provider.id),
            Some("google")
        );
    }

    #[test]
    fn matrix_openrouter_like_sources_include_all_static_profiles() {
        let _lock = ENV_LOCK.lock().unwrap();
        let guard = EnvGuard::save(&[
            "JCODE_OPENROUTER_API_KEY_NAME",
            "JCODE_OPENROUTER_ENV_FILE",
            "JCODE_OPENAI_COMPAT_API_KEY_NAME",
            "JCODE_OPENAI_COMPAT_ENV_FILE",
        ]);
        std::env::remove_var("JCODE_OPENROUTER_API_KEY_NAME");
        std::env::remove_var("JCODE_OPENROUTER_ENV_FILE");
        std::env::remove_var("JCODE_OPENAI_COMPAT_API_KEY_NAME");
        std::env::remove_var("JCODE_OPENAI_COMPAT_ENV_FILE");

        let sources = openrouter_like_api_key_sources();
        drop(guard);

        assert!(sources.contains(&(
            "OPENROUTER_API_KEY".to_string(),
            "openrouter.env".to_string()
        )));
        for profile in openai_compatible_profiles() {
            assert!(sources.contains(&(
                profile.api_key_env.to_string(),
                profile.env_file.to_string()
            )));
        }
    }

    #[test]
    fn matrix_openrouter_like_sources_accept_valid_overrides() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::save(&[
            "JCODE_OPENROUTER_API_KEY_NAME",
            "JCODE_OPENROUTER_ENV_FILE",
            "JCODE_OPENAI_COMPAT_API_KEY_NAME",
            "JCODE_OPENAI_COMPAT_ENV_FILE",
        ]);

        std::env::set_var("JCODE_OPENROUTER_API_KEY_NAME", "ALT_OPENROUTER_KEY");
        std::env::set_var("JCODE_OPENROUTER_ENV_FILE", "alt-openrouter.env");
        std::env::set_var("JCODE_OPENAI_COMPAT_API_KEY_NAME", "ALT_COMPAT_KEY");
        std::env::set_var("JCODE_OPENAI_COMPAT_ENV_FILE", "alt-compat.env");

        let sources = openrouter_like_api_key_sources();
        assert!(sources.contains(&(
            "ALT_OPENROUTER_KEY".to_string(),
            "alt-openrouter.env".to_string()
        )));
        assert!(sources.contains(&("ALT_COMPAT_KEY".to_string(), "alt-compat.env".to_string())));
    }

    #[test]
    fn matrix_openrouter_like_sources_reject_invalid_overrides() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::save(&[
            "JCODE_OPENROUTER_API_KEY_NAME",
            "JCODE_OPENROUTER_ENV_FILE",
            "JCODE_OPENAI_COMPAT_API_KEY_NAME",
            "JCODE_OPENAI_COMPAT_ENV_FILE",
        ]);

        std::env::set_var("JCODE_OPENROUTER_API_KEY_NAME", "bad-key-name");
        std::env::set_var("JCODE_OPENROUTER_ENV_FILE", "../bad.env");
        std::env::set_var("JCODE_OPENAI_COMPAT_API_KEY_NAME", "bad key");
        std::env::set_var("JCODE_OPENAI_COMPAT_ENV_FILE", "../bad-compat.env");

        let sources = openrouter_like_api_key_sources();
        assert!(!sources
            .iter()
            .any(|(key, _)| key == "bad-key-name" || key == "bad key"));
        assert!(!sources
            .iter()
            .any(|(_, file)| file == "../bad.env" || file == "../bad-compat.env"));
    }

    #[test]
    fn matrix_openai_compatible_profile_overrides_apply_when_valid() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::save(&[
            "JCODE_OPENAI_COMPAT_API_BASE",
            "JCODE_OPENAI_COMPAT_API_KEY_NAME",
            "JCODE_OPENAI_COMPAT_ENV_FILE",
            "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
        ]);

        std::env::set_var(
            "JCODE_OPENAI_COMPAT_API_BASE",
            "https://api.groq.com/openai/v1/",
        );
        std::env::set_var("JCODE_OPENAI_COMPAT_API_KEY_NAME", "GROQ_API_KEY");
        std::env::set_var("JCODE_OPENAI_COMPAT_ENV_FILE", "groq.env");
        std::env::set_var("JCODE_OPENAI_COMPAT_DEFAULT_MODEL", "openai/gpt-oss-120b");

        let resolved = resolve_openai_compatible_profile(OPENAI_COMPAT_PROFILE);
        assert_eq!(resolved.api_base, "https://api.groq.com/openai/v1");
        assert_eq!(resolved.api_key_env, "GROQ_API_KEY");
        assert_eq!(resolved.env_file, "groq.env");
        assert_eq!(
            resolved.default_model.as_deref(),
            Some("openai/gpt-oss-120b")
        );
    }

    #[test]
    fn matrix_openai_compatible_profile_overrides_reject_invalid_values() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::save(&[
            "JCODE_OPENAI_COMPAT_API_BASE",
            "JCODE_OPENAI_COMPAT_API_KEY_NAME",
            "JCODE_OPENAI_COMPAT_ENV_FILE",
        ]);

        std::env::set_var("JCODE_OPENAI_COMPAT_API_BASE", "http://example.com/v1");
        std::env::set_var("JCODE_OPENAI_COMPAT_API_KEY_NAME", "bad-key-name");
        std::env::set_var("JCODE_OPENAI_COMPAT_ENV_FILE", "../bad.env");

        let resolved = resolve_openai_compatible_profile(OPENAI_COMPAT_PROFILE);
        assert_eq!(resolved.api_base, OPENAI_COMPAT_PROFILE.api_base);
        assert_eq!(resolved.api_key_env, OPENAI_COMPAT_PROFILE.api_key_env);
        assert_eq!(resolved.env_file, OPENAI_COMPAT_PROFILE.env_file);
    }

    #[test]
    fn matrix_load_api_key_from_env_or_config_prefers_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().expect("tempdir");
        let config_root = temp.path().join("config");
        std::fs::create_dir_all(config_root.join("jcode")).expect("config dir");

        let _guard = EnvGuard::save(&["XDG_CONFIG_HOME", "OPENCODE_API_KEY"]);
        std::env::set_var("XDG_CONFIG_HOME", &config_root);
        std::env::set_var("OPENCODE_API_KEY", "env-secret");
        std::fs::write(
            config_root.join("jcode").join("opencode.env"),
            "OPENCODE_API_KEY=file-secret\n",
        )
        .expect("env file");

        assert_eq!(
            load_api_key_from_env_or_config("OPENCODE_API_KEY", "opencode.env").as_deref(),
            Some("env-secret")
        );
    }

    #[test]
    fn matrix_load_api_key_from_env_or_config_reads_config_file() {
        let _lock = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().expect("tempdir");
        let config_root = temp.path().join("config");
        std::fs::create_dir_all(config_root.join("jcode")).expect("config dir");

        let _guard = EnvGuard::save(&["XDG_CONFIG_HOME", "OPENCODE_API_KEY"]);
        std::env::set_var("XDG_CONFIG_HOME", &config_root);
        std::env::remove_var("OPENCODE_API_KEY");
        std::fs::write(
            config_root.join("jcode").join("opencode.env"),
            "OPENCODE_API_KEY=file-secret\n",
        )
        .expect("env file");

        assert_eq!(
            load_api_key_from_env_or_config("OPENCODE_API_KEY", "opencode.env").as_deref(),
            Some("file-secret")
        );
    }
}
