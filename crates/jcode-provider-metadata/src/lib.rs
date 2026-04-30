#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginProviderAuthKind {
    OAuth,
    ApiKey,
    DeviceCode,
    Cli,
    Hybrid,
    Local,
}

impl LoginProviderAuthKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::OAuth => "OAuth",
            Self::ApiKey => "API key",
            Self::DeviceCode => "device code",
            Self::Cli => "CLI",
            Self::Hybrid => "API key / CLI",
            Self::Local => "local endpoint",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginProviderTarget {
    AutoImport,
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
    ExternalImport,
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
    pub requires_api_key: bool,
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
    pub requires_api_key: bool,
}

pub const OPENCODE_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "opencode",
    display_name: "OpenCode Zen",
    api_base: "https://opencode.ai/zen/v1",
    api_key_env: "OPENCODE_API_KEY",
    env_file: "opencode.env",
    setup_url: "https://github.com/1jehuang/jcode#openai-compatible-providers",
    default_model: Some("qwen/qwen3-coder-plus"),
    requires_api_key: true,
};

pub const OPENCODE_GO_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "opencode-go",
    display_name: "OpenCode Go",
    api_base: "https://opencode.ai/zen/go/v1",
    api_key_env: "OPENCODE_GO_API_KEY",
    env_file: "opencode-go.env",
    setup_url: "https://github.com/1jehuang/jcode#openai-compatible-providers",
    default_model: Some("THUDM/GLM-4.9"),
    requires_api_key: true,
};

pub const ZAI_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "zai",
    display_name: "Z.AI",
    api_base: "https://api.z.ai/api/coding/paas/v4",
    api_key_env: "ZHIPU_API_KEY",
    env_file: "zai.env",
    setup_url: "https://docs.z.ai/guides/develop/openai/introduction",
    default_model: Some("glm-4.5"),
    requires_api_key: true,
};

pub const KIMI_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "kimi",
    display_name: "Kimi Code",
    api_base: "https://api.kimi.com/coding/v1",
    api_key_env: "KIMI_API_KEY",
    env_file: "kimi.env",
    setup_url: "https://www.kimi.com/coding/docs/en/more/third-party-agents.html",
    default_model: Some("kimi-for-coding"),
    requires_api_key: true,
};

pub const AI302_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "302ai",
    display_name: "302.AI",
    api_base: "https://api.302.ai/v1",
    api_key_env: "302AI_API_KEY",
    env_file: "302ai.env",
    setup_url: "https://opencode.ai/docs/providers#302ai",
    default_model: Some("qwen3-235b-a22b-instruct-2507"),
    requires_api_key: true,
};

pub const BASETEN_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "baseten",
    display_name: "Baseten",
    api_base: "https://inference.baseten.co/v1",
    api_key_env: "BASETEN_API_KEY",
    env_file: "baseten.env",
    setup_url: "https://opencode.ai/docs/providers#baseten",
    default_model: Some("zai-org/GLM-4.7"),
    requires_api_key: true,
};

pub const CORTECS_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "cortecs",
    display_name: "Cortecs",
    api_base: "https://api.cortecs.ai/v1",
    api_key_env: "CORTECS_API_KEY",
    env_file: "cortecs.env",
    setup_url: "https://opencode.ai/docs/providers#cortecs",
    default_model: Some("kimi-k2.5"),
    requires_api_key: true,
};

pub const DEEPSEEK_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "deepseek",
    display_name: "DeepSeek",
    api_base: "https://api.deepseek.com",
    api_key_env: "DEEPSEEK_API_KEY",
    env_file: "deepseek.env",
    setup_url: "https://api-docs.deepseek.com/",
    default_model: Some("deepseek-v4-flash"),
    requires_api_key: true,
};

pub const FIRMWARE_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "firmware",
    display_name: "Firmware",
    api_base: "https://app.frogbot.ai/api/v1",
    api_key_env: "FIRMWARE_API_KEY",
    env_file: "firmware.env",
    setup_url: "https://opencode.ai/docs/providers#firmware",
    default_model: Some("kimi-k2.5"),
    requires_api_key: true,
};

pub const HUGGING_FACE_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "huggingface",
    display_name: "Hugging Face",
    api_base: "https://router.huggingface.co/v1",
    api_key_env: "HF_TOKEN",
    env_file: "huggingface.env",
    setup_url: "https://opencode.ai/docs/providers#hugging-face",
    default_model: Some("zai-org/GLM-4.7"),
    requires_api_key: true,
};

pub const MOONSHOT_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "moonshotai",
    display_name: "Moonshot AI",
    api_base: "https://api.moonshot.ai/v1",
    api_key_env: "MOONSHOT_API_KEY",
    env_file: "moonshotai.env",
    setup_url: "https://opencode.ai/docs/providers#moonshot-ai",
    default_model: Some("kimi-k2.5"),
    requires_api_key: true,
};

pub const NEBIUS_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "nebius",
    display_name: "Nebius Token Factory",
    api_base: "https://api.tokenfactory.nebius.com/v1",
    api_key_env: "NEBIUS_API_KEY",
    env_file: "nebius.env",
    setup_url: "https://opencode.ai/docs/providers#nebius-token-factory",
    default_model: Some("openai/gpt-oss-120b"),
    requires_api_key: true,
};

pub const SCALEWAY_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "scaleway",
    display_name: "Scaleway",
    api_base: "https://api.scaleway.ai/v1",
    api_key_env: "SCALEWAY_API_KEY",
    env_file: "scaleway.env",
    setup_url: "https://opencode.ai/docs/providers#scaleway",
    default_model: Some("qwen3-coder-30b-a3b-instruct"),
    requires_api_key: true,
};

pub const STACKIT_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "stackit",
    display_name: "STACKIT",
    api_base: "https://api.openai-compat.model-serving.eu01.onstackit.cloud/v1",
    api_key_env: "STACKIT_API_KEY",
    env_file: "stackit.env",
    setup_url: "https://opencode.ai/docs/providers#stackit",
    default_model: Some("openai/gpt-oss-120b"),
    requires_api_key: true,
};

pub const GROQ_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "groq",
    display_name: "Groq",
    api_base: "https://api.groq.com/openai/v1",
    api_key_env: "GROQ_API_KEY",
    env_file: "groq.env",
    setup_url: "https://console.groq.com/docs/openai",
    default_model: Some("llama-3.1-8b-instant"),
    requires_api_key: true,
};

pub const MISTRAL_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "mistral",
    display_name: "Mistral",
    api_base: "https://api.mistral.ai/v1",
    api_key_env: "MISTRAL_API_KEY",
    env_file: "mistral.env",
    setup_url: "https://docs.mistral.ai/getting-started/models/",
    default_model: Some("devstral-medium-2507"),
    requires_api_key: true,
};

pub const PERPLEXITY_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "perplexity",
    display_name: "Perplexity",
    api_base: "https://api.perplexity.ai",
    api_key_env: "PERPLEXITY_API_KEY",
    env_file: "perplexity.env",
    setup_url: "https://docs.perplexity.ai/docs/agent-api/openai-compatibility",
    default_model: Some("sonar"),
    requires_api_key: true,
};

pub const TOGETHER_AI_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "togetherai",
    display_name: "Together AI",
    api_base: "https://api.together.xyz/v1",
    api_key_env: "TOGETHER_API_KEY",
    env_file: "togetherai.env",
    setup_url: "https://docs.together.ai/docs/openai-api-compatibility",
    default_model: Some("moonshotai/Kimi-K2-Instruct"),
    requires_api_key: true,
};

pub const DEEPINFRA_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "deepinfra",
    display_name: "Deep Infra",
    api_base: "https://api.deepinfra.com/v1/openai",
    api_key_env: "DEEPINFRA_API_KEY",
    env_file: "deepinfra.env",
    setup_url: "https://deepinfra.com/docs/api-reference",
    default_model: Some("moonshotai/Kimi-K2-Instruct"),
    requires_api_key: true,
};

pub const FIREWORKS_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "fireworks",
    display_name: "Fireworks",
    api_base: "https://api.fireworks.ai/inference/v1",
    api_key_env: "FIREWORKS_API_KEY",
    env_file: "fireworks.env",
    setup_url: "https://docs.fireworks.ai/tools-sdks/openai-compatibility",
    default_model: Some("accounts/fireworks/routers/kimi-k2p5-turbo"),
    requires_api_key: true,
};

pub const MINIMAX_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "minimax",
    display_name: "MiniMax",
    api_base: "https://api.minimax.io/v1",
    api_key_env: "MINIMAX_API_KEY",
    env_file: "minimax.env",
    setup_url: "https://platform.minimax.io/docs/api-reference/text-openai-api",
    default_model: Some("MiniMax-M2.7"),
    requires_api_key: true,
};

pub const XAI_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "xai",
    display_name: "xAI",
    api_base: "https://api.x.ai/v1",
    api_key_env: "XAI_API_KEY",
    env_file: "xai.env",
    setup_url: "https://docs.x.ai/developers/quickstart",
    default_model: Some("grok-code-fast-1"),
    requires_api_key: true,
};

pub const LMSTUDIO_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "lmstudio",
    display_name: "LM Studio",
    api_base: "http://localhost:1234/v1",
    api_key_env: "LMSTUDIO_API_KEY",
    env_file: "lmstudio.env",
    setup_url: "https://lmstudio.ai/docs/app/api/endpoints/openai",
    default_model: None,
    requires_api_key: false,
};

pub const OLLAMA_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "ollama",
    display_name: "Ollama",
    api_base: "http://localhost:11434/v1",
    api_key_env: "OLLAMA_API_KEY",
    env_file: "ollama.env",
    setup_url: "https://ollama.com/blog/openai-compatibility",
    default_model: None,
    requires_api_key: false,
};

pub const CHUTES_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "chutes",
    display_name: "Chutes",
    api_base: "https://llm.chutes.ai/v1",
    api_key_env: "CHUTES_API_KEY",
    env_file: "chutes.env",
    setup_url: "https://chutes.ai",
    default_model: Some("Qwen/Qwen3-Coder-480B-A35B-Instruct"),
    requires_api_key: true,
};

pub const CEREBRAS_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "cerebras",
    display_name: "Cerebras",
    api_base: "https://api.cerebras.ai/v1",
    api_key_env: "CEREBRAS_API_KEY",
    env_file: "cerebras.env",
    setup_url: "https://inference-docs.cerebras.ai/introduction",
    default_model: Some("qwen-3-coder-480b"),
    requires_api_key: true,
};

pub const ALIBABA_CODING_PLAN_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "alibaba-coding-plan",
    display_name: "Alibaba Cloud Coding Plan",
    api_base: "https://coding.dashscope.aliyuncs.com/v1",
    api_key_env: "BAILIAN_CODING_PLAN_API_KEY",
    env_file: "alibaba-coding-plan.env",
    setup_url: "https://www.alibabacloud.com/help/en/model-studio/coding-plan-quickstart",
    default_model: Some("qwen3-coder-plus"),
    requires_api_key: true,
};

pub const OPENAI_COMPAT_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "openai-compatible",
    display_name: "OpenAI-compatible",
    api_base: "https://api.openai.com/v1",
    api_key_env: "OPENAI_COMPAT_API_KEY",
    env_file: "openai-compatible.env",
    setup_url: "https://github.com/1jehuang/jcode#openai-compatible-providers",
    default_model: None,
    requires_api_key: true,
};

const OPENAI_COMPAT_PROFILES: [OpenAiCompatibleProfile; 28] = [
    OPENCODE_PROFILE,
    OPENCODE_GO_PROFILE,
    ZAI_PROFILE,
    KIMI_PROFILE,
    CHUTES_PROFILE,
    CEREBRAS_PROFILE,
    ALIBABA_CODING_PLAN_PROFILE,
    AI302_PROFILE,
    BASETEN_PROFILE,
    CORTECS_PROFILE,
    DEEPSEEK_PROFILE,
    FIRMWARE_PROFILE,
    HUGGING_FACE_PROFILE,
    MOONSHOT_PROFILE,
    NEBIUS_PROFILE,
    SCALEWAY_PROFILE,
    STACKIT_PROFILE,
    GROQ_PROFILE,
    MISTRAL_PROFILE,
    PERPLEXITY_PROFILE,
    TOGETHER_AI_PROFILE,
    DEEPINFRA_PROFILE,
    FIREWORKS_PROFILE,
    MINIMAX_PROFILE,
    XAI_PROFILE,
    LMSTUDIO_PROFILE,
    OLLAMA_PROFILE,
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

pub const AUTO_IMPORT_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "auto-import",
    display_name: "Auto Import",
    auth_kind: LoginProviderAuthKind::Local,
    auth_state_key: LoginProviderAuthStateKey::ExternalImport,
    auth_status_method: "Reuse detected logins",
    aliases: &["import", "reuse", "autoimport"],
    menu_detail: "review and reuse logins from other tools",
    recommended: false,
    target: LoginProviderTarget::AutoImport,
    order: LoginProviderSurfaceOrder::new(Some(1), Some(1), None, None, None),
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
    display_name: "Z.AI",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["z.ai", "z-ai", "zai-coding", "zhipu"],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(ZAI_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(7), Some(6), Some(7), Some(6), Some(6)),
};

pub const KIMI_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "kimi",
    display_name: "Kimi Code",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &[
        "kimi-code",
        "kimi-coding",
        "kimi-coding-plan",
        "kimi-for-coding",
        "moonshot-coding",
    ],
    menu_detail: "API key, dedicated Kimi coding endpoint",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(KIMI_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(36), Some(36), Some(36), Some(36), Some(36)),
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

pub const ALIBABA_CODING_PLAN_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "alibaba-coding-plan",
    display_name: "Alibaba Cloud Coding Plan",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["bailian", "aliyun-bailian", "coding-plan", "alibaba-coding"],
    menu_detail: "API key, dedicated Alibaba coding endpoint",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(ALIBABA_CODING_PLAN_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(10), Some(9), Some(10), Some(9), Some(9)),
};

pub const AI302_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "302ai",
    display_name: "302.AI",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["302.ai"],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(AI302_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(18), Some(18), Some(18), Some(18), Some(18)),
};

pub const BASETEN_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "baseten",
    display_name: "Baseten",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &[],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(BASETEN_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(19), Some(19), Some(19), Some(19), Some(19)),
};

pub const CORTECS_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "cortecs",
    display_name: "Cortecs",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &[],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(CORTECS_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(20), Some(20), Some(20), Some(20), Some(20)),
};

pub const DEEPSEEK_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "deepseek",
    display_name: "DeepSeek",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &[],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(DEEPSEEK_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(21), Some(21), Some(21), Some(21), Some(21)),
};

pub const FIRMWARE_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "firmware",
    display_name: "Firmware",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &[],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(FIRMWARE_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(22), Some(22), Some(22), Some(22), Some(22)),
};

pub const HUGGING_FACE_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "huggingface",
    display_name: "Hugging Face",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["hugging-face", "hf"],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(HUGGING_FACE_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(23), Some(23), Some(23), Some(23), Some(23)),
};

pub const MOONSHOT_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "moonshotai",
    display_name: "Moonshot AI",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["moonshot"],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(MOONSHOT_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(24), Some(24), Some(24), Some(24), Some(24)),
};

pub const NEBIUS_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "nebius",
    display_name: "Nebius Token Factory",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &[],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(NEBIUS_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(25), Some(25), Some(25), Some(25), Some(25)),
};

pub const SCALEWAY_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "scaleway",
    display_name: "Scaleway",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &[],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(SCALEWAY_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(26), Some(26), Some(26), Some(26), Some(26)),
};

pub const STACKIT_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "stackit",
    display_name: "STACKIT",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &[],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(STACKIT_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(27), Some(27), Some(27), Some(27), Some(27)),
};

pub const GROQ_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "groq",
    display_name: "Groq",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &[],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(GROQ_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(28), Some(28), Some(28), Some(28), Some(28)),
};

pub const MISTRAL_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "mistral",
    display_name: "Mistral",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["mistralai"],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(MISTRAL_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(29), Some(29), Some(29), Some(29), Some(29)),
};

pub const PERPLEXITY_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "perplexity",
    display_name: "Perplexity",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["pplx"],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(PERPLEXITY_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(30), Some(30), Some(30), Some(30), Some(30)),
};

pub const TOGETHER_AI_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "togetherai",
    display_name: "Together AI",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["together", "together-ai"],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(TOGETHER_AI_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(31), Some(31), Some(31), Some(31), Some(31)),
};

pub const DEEPINFRA_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "deepinfra",
    display_name: "Deep Infra",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["deep-infra"],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(DEEPINFRA_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(32), Some(32), Some(32), Some(32), Some(32)),
};

pub const FIREWORKS_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "fireworks",
    display_name: "Fireworks",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["fireworks-ai", "fireworks.ai"],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(FIREWORKS_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(37), Some(37), Some(37), Some(37), Some(37)),
};

pub const MINIMAX_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "minimax",
    display_name: "MiniMax",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["minimaxi", "minimax-ai"],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(MINIMAX_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(38), Some(38), Some(38), Some(38), Some(38)),
};

pub const XAI_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "xai",
    display_name: "xAI",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["x.ai", "x-ai", "grok"],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(XAI_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(33), Some(33), Some(33), Some(33), Some(33)),
};

pub const LMSTUDIO_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "lmstudio",
    display_name: "LM Studio",
    auth_kind: LoginProviderAuthKind::Local,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "local endpoint",
    aliases: &["lm-studio"],
    menu_detail: "local OpenAI-compatible endpoint",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(LMSTUDIO_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(34), Some(34), Some(34), Some(34), Some(34)),
};

pub const OLLAMA_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "ollama",
    display_name: "Ollama",
    auth_kind: LoginProviderAuthKind::Local,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "local endpoint",
    aliases: &[],
    menu_detail: "local OpenAI-compatible endpoint",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(OLLAMA_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(35), Some(35), Some(35), Some(35), Some(35)),
};

pub const OPENAI_COMPAT_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "openai-compatible",
    display_name: "OpenAI-compatible",
    auth_kind: LoginProviderAuthKind::Hybrid,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key / local endpoint",
    aliases: &["openai_compatible", "compat", "custom"],
    menu_detail: "custom hosted or local OpenAI-compatible endpoint",
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
    auth_kind: LoginProviderAuthKind::OAuth,
    auth_state_key: LoginProviderAuthStateKey::Antigravity,
    auth_status_method: "OAuth",
    aliases: &[],
    menu_detail: "Google Antigravity OAuth login",
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

const LOGIN_PROVIDERS: [LoginProviderDescriptor; 39] = [
    AUTO_IMPORT_LOGIN_PROVIDER,
    CLAUDE_LOGIN_PROVIDER,
    OPENAI_LOGIN_PROVIDER,
    JCODE_LOGIN_PROVIDER,
    OPENROUTER_LOGIN_PROVIDER,
    AZURE_LOGIN_PROVIDER,
    OPENCODE_LOGIN_PROVIDER,
    OPENCODE_GO_LOGIN_PROVIDER,
    ZAI_LOGIN_PROVIDER,
    KIMI_LOGIN_PROVIDER,
    CHUTES_LOGIN_PROVIDER,
    CEREBRAS_LOGIN_PROVIDER,
    ALIBABA_CODING_PLAN_LOGIN_PROVIDER,
    AI302_LOGIN_PROVIDER,
    BASETEN_LOGIN_PROVIDER,
    CORTECS_LOGIN_PROVIDER,
    DEEPSEEK_LOGIN_PROVIDER,
    FIRMWARE_LOGIN_PROVIDER,
    HUGGING_FACE_LOGIN_PROVIDER,
    MOONSHOT_LOGIN_PROVIDER,
    NEBIUS_LOGIN_PROVIDER,
    SCALEWAY_LOGIN_PROVIDER,
    STACKIT_LOGIN_PROVIDER,
    GROQ_LOGIN_PROVIDER,
    MISTRAL_LOGIN_PROVIDER,
    PERPLEXITY_LOGIN_PROVIDER,
    TOGETHER_AI_LOGIN_PROVIDER,
    DEEPINFRA_LOGIN_PROVIDER,
    FIREWORKS_LOGIN_PROVIDER,
    MINIMAX_LOGIN_PROVIDER,
    XAI_LOGIN_PROVIDER,
    LMSTUDIO_LOGIN_PROVIDER,
    OLLAMA_LOGIN_PROVIDER,
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

    let parsed = url::Url::parse(trimmed).ok()?;
    let scheme = parsed.scheme();
    if scheme != "https" && scheme != "http" {
        return None;
    }

    if scheme == "http" {
        let host = parsed.host_str()?;
        if !allows_insecure_http_host(host) {
            return None;
        }
    }

    Some(trimmed.trim_end_matches('/').to_string())
}

fn allows_insecure_http_host(host: &str) -> bool {
    let host = host.trim();
    let host = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host);
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }

    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => {
                v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()
            }
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.is_unique_local()
                    || v6.is_unicast_link_local()
                    || v6.is_unspecified()
            }
        };
    }

    false
}

fn normalize_provider_input(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

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
    fn normalize_api_base_accepts_private_http_hosts() {
        assert_eq!(
            normalize_api_base("http://192.168.1.25:8000/v1/").as_deref(),
            Some("http://192.168.1.25:8000/v1")
        );
        assert_eq!(
            normalize_api_base("http://10.0.0.8:11434/v1").as_deref(),
            Some("http://10.0.0.8:11434/v1")
        );
        assert_eq!(
            normalize_api_base("http://[fd00::1]:8080/v1").as_deref(),
            Some("http://[fd00::1]:8080/v1")
        );
    }

    #[test]
    fn normalize_api_base_rejects_public_http_hosts() {
        assert_eq!(normalize_api_base("http://example.com/v1"), None);
        assert_eq!(normalize_api_base("http://8.8.8.8/v1"), None);
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
            resolve_login_provider("zhipu").map(|provider| provider.id),
            Some("zai")
        );
        assert_eq!(
            resolve_login_provider("kimi").map(|provider| provider.id),
            Some("kimi")
        );
        assert_eq!(
            resolve_login_provider("kimi-for-coding").map(|provider| provider.id),
            Some("kimi")
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
            resolve_login_provider("bailian").map(|provider| provider.id),
            Some("alibaba-coding-plan")
        );
        assert_eq!(
            resolve_login_provider("302.ai").map(|provider| provider.id),
            Some("302ai")
        );
        assert_eq!(
            resolve_login_provider("hf").map(|provider| provider.id),
            Some("huggingface")
        );
        assert_eq!(
            resolve_login_provider("moonshot").map(|provider| provider.id),
            Some("moonshotai")
        );
        assert_eq!(
            resolve_login_provider("mistralai").map(|provider| provider.id),
            Some("mistral")
        );
        assert_eq!(
            resolve_login_provider("pplx").map(|provider| provider.id),
            Some("perplexity")
        );
        assert_eq!(
            resolve_login_provider("together").map(|provider| provider.id),
            Some("togetherai")
        );
        assert_eq!(
            resolve_login_provider("deep-infra").map(|provider| provider.id),
            Some("deepinfra")
        );
        assert_eq!(
            resolve_login_provider("fireworks.ai").map(|provider| provider.id),
            Some("fireworks")
        );
        assert_eq!(
            resolve_login_provider("minimax-ai").map(|provider| provider.id),
            Some("minimax")
        );
        assert_eq!(
            resolve_login_provider("grok").map(|provider| provider.id),
            Some("xai")
        );
        assert_eq!(
            resolve_login_provider("lm-studio").map(|provider| provider.id),
            Some("lmstudio")
        );
        assert_eq!(
            resolve_login_provider("gmail").map(|provider| provider.id),
            Some("google")
        );
    }

    #[test]
    fn matrix_login_provider_ids_and_aliases_are_unique() {
        let mut seen = HashSet::new();
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
            Some("auto-import")
        );
        assert_eq!(
            resolve_login_selection("2", &providers).map(|provider| provider.id),
            Some("claude")
        );
        assert_eq!(
            resolve_login_selection("15", &providers).map(|provider| provider.id),
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
            resolve_login_selection("1", &providers).map(|provider| provider.id),
            Some("auto-import")
        );
        assert_eq!(
            resolve_login_selection("4", &providers).map(|provider| provider.id),
            Some("jcode")
        );
        assert_eq!(
            resolve_login_selection("5", &providers).map(|provider| provider.id),
            Some("copilot")
        );
        assert_eq!(
            resolve_login_selection("6", &providers).map(|provider| provider.id),
            Some("openrouter")
        );
        assert_eq!(
            resolve_login_selection("7", &providers).map(|provider| provider.id),
            Some("azure")
        );
        assert_eq!(
            resolve_login_selection("17", &providers).map(|provider| provider.id),
            Some("gemini")
        );
        assert_eq!(
            resolve_login_selection("18", &providers).map(|provider| provider.id),
            Some("google")
        );
    }
}
