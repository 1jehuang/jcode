use super::{
    LoginProviderAuthKind, LoginProviderAuthStateKey, LoginProviderDescriptor,
    LoginProviderSurfaceOrder, LoginProviderTarget, OpenAiCompatibleProfile,
};

pub const OPENCODE_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "opencode",
    display_name: "OpenCode Zen",
    api_base: "https://opencode.ai/zen/v1",
    api_key_env: "OPENCODE_API_KEY",
    env_file: "opencode.env",
    setup_url: "https://github.com/1jehuang/jcode#openai-compatible-providers",
    default_model: Some("minimax-m2.7"),
    requires_api_key: true,
};

pub const OPENCODE_GO_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "opencode-go",
    display_name: "OpenCode Go",
    api_base: "https://opencode.ai/zen/go/v1",
    api_key_env: "OPENCODE_GO_API_KEY",
    env_file: "opencode-go.env",
    setup_url: "https://github.com/1jehuang/jcode#openai-compatible-providers",
    default_model: Some("kimi-k2.5"),
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
    setup_url: "https://github.com/1jehuang/jcode#openai-compatible-providers",
    default_model: Some("qwen3-235b-a22b-instruct-2507"),
    requires_api_key: true,
};

pub const BASETEN_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "baseten",
    display_name: "Baseten",
    api_base: "https://inference.baseten.co/v1",
    api_key_env: "BASETEN_API_KEY",
    env_file: "baseten.env",
    setup_url: "https://github.com/1jehuang/jcode#openai-compatible-providers",
    default_model: Some("zai-org/GLM-4.7"),
    requires_api_key: true,
};

pub const CORTECS_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "cortecs",
    display_name: "Cortecs",
    api_base: "https://api.cortecs.ai/v1",
    api_key_env: "CORTECS_API_KEY",
    env_file: "cortecs.env",
    setup_url: "https://github.com/1jehuang/jcode#openai-compatible-providers",
    default_model: Some("kimi-k2.5"),
    requires_api_key: true,
};

// OpenRouter also has a dedicated provider implementation elsewhere, but it
// speaks the standard OpenAI-compatible /api/v1 endpoint, so it can be driven
// by `provider-doctor` / `provider-test-coverage` like any other
// OpenAI-compatible provider. `default_model` is None so the doctor selects the
// live catalog's first model unless `--model` is passed.
pub const OPENROUTER_OPENAI_COMPAT_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "openrouter",
    display_name: "OpenRouter",
    api_base: "https://openrouter.ai/api/v1",
    api_key_env: "OPENROUTER_API_KEY",
    env_file: "openrouter.env",
    setup_url: "https://openrouter.ai/keys",
    default_model: None,
    requires_api_key: true,
};

// Anthropic and OpenAI also expose OpenAI-compatible `/v1/chat/completions`
// endpoints, so they can be driven by `provider-doctor` /
// `provider-test-coverage` as OpenAI-compatible profiles. These profile ids
// alias the native login-provider ids (`anthropic-api`, `openai-api`); auth
// activation deliberately routes them through the native runtime, while the
// live HTTP probes hit these hosts (Anthropic needs `x-api-key` +
// `anthropic-version`, handled in the probe layer). `default_model` is None so
// the doctor selects from the live catalog unless `--model` is passed.
pub const ANTHROPIC_OPENAI_COMPAT_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "anthropic-api",
    display_name: "Anthropic API",
    api_base: "https://api.anthropic.com/v1",
    api_key_env: "ANTHROPIC_API_KEY",
    env_file: "anthropic.env",
    setup_url: "https://docs.anthropic.com/en/api/openai-sdk",
    default_model: None,
    requires_api_key: true,
};

pub const OPENAI_NATIVE_OPENAI_COMPAT_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "openai-api",
    display_name: "OpenAI API",
    api_base: "https://api.openai.com/v1",
    api_key_env: "OPENAI_API_KEY",
    env_file: "openai.env",
    setup_url: "https://platform.openai.com/api-keys",
    default_model: None,
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

pub const COMTEGRA_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "comtegra",
    display_name: "Comtegra GPU Cloud",
    api_base: "https://llm.comtegra.cloud/v1",
    api_key_env: "COMTEGRA_API_KEY",
    env_file: "comtegra.env",
    setup_url: "https://docs.cgc.comtegra.cloud/llm-api",
    default_model: Some("glm-51-nvfp4"),
    requires_api_key: true,
};

pub const FPT_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "fpt",
    display_name: "FPT AI Marketplace",
    api_base: "https://mkp-api.fptcloud.com",
    api_key_env: "FPT_API_KEY",
    env_file: "fpt.env",
    setup_url: "https://ai-docs.fptcloud.com/api-reference/ai-marketplace/api-reference/api-integration-large-language-model-md",
    default_model: Some("GLM-5.1"),
    requires_api_key: true,
};

pub const FIRMWARE_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "firmware",
    display_name: "Firmware",
    api_base: "https://app.frogbot.ai/api/v1",
    api_key_env: "FIRMWARE_API_KEY",
    env_file: "firmware.env",
    setup_url: "https://github.com/1jehuang/jcode#openai-compatible-providers",
    default_model: Some("kimi-k2.5"),
    requires_api_key: true,
};

pub const HUGGING_FACE_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "huggingface",
    display_name: "Hugging Face",
    api_base: "https://router.huggingface.co/v1",
    api_key_env: "HF_TOKEN",
    env_file: "huggingface.env",
    setup_url: "https://github.com/1jehuang/jcode#openai-compatible-providers",
    default_model: Some("zai-org/GLM-4.7"),
    requires_api_key: true,
};

pub const MOONSHOT_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "moonshotai",
    display_name: "Moonshot AI",
    api_base: "https://api.moonshot.ai/v1",
    api_key_env: "MOONSHOT_API_KEY",
    env_file: "moonshotai.env",
    setup_url: "https://github.com/1jehuang/jcode#openai-compatible-providers",
    default_model: Some("kimi-k2.5"),
    requires_api_key: true,
};

pub const NEBIUS_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "nebius",
    display_name: "Nebius Token Factory",
    api_base: "https://api.tokenfactory.nebius.com/v1",
    api_key_env: "NEBIUS_API_KEY",
    env_file: "nebius.env",
    setup_url: "https://github.com/1jehuang/jcode#openai-compatible-providers",
    default_model: Some("openai/gpt-oss-120b"),
    requires_api_key: true,
};

pub const SCALEWAY_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "scaleway",
    display_name: "Scaleway",
    api_base: "https://api.scaleway.ai/v1",
    api_key_env: "SCALEWAY_API_KEY",
    env_file: "scaleway.env",
    setup_url: "https://github.com/1jehuang/jcode#openai-compatible-providers",
    default_model: Some("qwen3-coder-30b-a3b-instruct"),
    requires_api_key: true,
};

pub const STACKIT_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "stackit",
    display_name: "STACKIT",
    api_base: "https://api.openai-compat.model-serving.eu01.onstackit.cloud/v1",
    api_key_env: "STACKIT_API_KEY",
    env_file: "stackit.env",
    setup_url: "https://github.com/1jehuang/jcode#openai-compatible-providers",
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
    api_key_env: "OPENAI_API_KEY",
    env_file: "minimax.env",
    setup_url: "https://platform.minimax.io/docs/guides/text-generation",
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
    setup_url: "https://docs.ollama.com/api/openai-compatibility",
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
    // Chutes' accessible models change with capacity/key access. Do not keep a
    // static default here: post-login activation should select from the live
    // `/models` catalog instead of advertising a stale model that may 404 at
    // chat/completions time.
    default_model: None,
    requires_api_key: true,
};

pub const CEREBRAS_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "cerebras",
    display_name: "Cerebras",
    api_base: "https://api.cerebras.ai/v1",
    api_key_env: "CEREBRAS_API_KEY",
    env_file: "cerebras.env",
    setup_url: "https://inference-docs.cerebras.ai/introduction",
    default_model: Some("gpt-oss-120b"),
    requires_api_key: true,
};

pub const ALIBABA_CODING_PLAN_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "alibaba-coding-plan",
    display_name: "Alibaba Cloud Coding Plan",
    api_base: "https://coding-intl.dashscope.aliyuncs.com/v1",
    api_key_env: "BAILIAN_CODING_PLAN_API_KEY",
    env_file: "alibaba-coding-plan.env",
    setup_url: "https://www.alibabacloud.com/help/en/model-studio/coding-plan-quickstart",
    default_model: Some("qwen3-coder-plus"),
    requires_api_key: true,
};

pub const NVIDIA_NIM_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "nvidia-nim",
    display_name: "NVIDIA NIM",
    api_base: "https://integrate.api.nvidia.com/v1",
    api_key_env: "NVIDIA_API_KEY",
    env_file: "nvidia-nim.env",
    setup_url: "https://build.nvidia.com/explore/discover",
    default_model: Some("nvidia/llama-3.1-nemotron-ultra-253b-v1"),
    requires_api_key: true,
};

pub const XIAOMI_MIMO_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "xiaomi-mimo",
    display_name: "Xiaomi MiMo",
    api_base: "https://api.xiaomimimo.com/v1",
    api_key_env: "XIAOMI_MIMO_API_KEY",
    env_file: "xiaomi-mimo.env",
    setup_url: "https://platform.xiaomimimo.com",
    default_model: Some("mimo-v2.5"),
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

pub(crate) const OPENAI_COMPAT_PROFILES: [OpenAiCompatibleProfile; 35] = [
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
    OPENROUTER_OPENAI_COMPAT_PROFILE,
    ANTHROPIC_OPENAI_COMPAT_PROFILE,
    OPENAI_NATIVE_OPENAI_COMPAT_PROFILE,
    DEEPSEEK_PROFILE,
    COMTEGRA_PROFILE,
    FPT_PROFILE,
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
    NVIDIA_NIM_PROFILE,
    XIAOMI_MIMO_PROFILE,
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

pub const ANTHROPIC_API_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "anthropic-api",
    display_name: "Anthropic API",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::Anthropic,
    auth_status_method: "API key",
    aliases: &["claude-api", "anthropic-key", "claude-key"],
    menu_detail: "direct Anthropic Messages API",
    recommended: false,
    target: LoginProviderTarget::ClaudeApiKey,
    order: LoginProviderSurfaceOrder::new(Some(2), Some(2), Some(2), Some(2), Some(2)),
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

pub const OPENAI_API_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "openai-api",
    display_name: "OpenAI API",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenAi,
    auth_status_method: "API key",
    aliases: &[
        "openai-key",
        "openai-apikey",
        "openai-platform",
        "platform-openai",
    ],
    menu_detail: "native OpenAI API key, pay-per-token",
    recommended: false,
    target: LoginProviderTarget::OpenAiApiKey,
    order: LoginProviderSurfaceOrder::new(Some(99), Some(99), Some(99), Some(99), Some(99)),
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

pub const BEDROCK_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "bedrock",
    display_name: "AWS Bedrock",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::Bedrock,
    auth_status_method: "API key / AWS credentials",
    aliases: &["aws-bedrock", "aws_bedrock"],
    menu_detail: "Bedrock API key or AWS credentials, pay-per-token",
    recommended: false,
    target: LoginProviderTarget::Bedrock,
    order: LoginProviderSurfaceOrder::new(Some(5), Some(4), None, None, Some(4)),
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
    order: LoginProviderSurfaceOrder::new(Some(5), Some(5), None, None, Some(4)),
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

pub const COMTEGRA_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "comtegra",
    display_name: "Comtegra GPU Cloud",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["cgc", "comtegra-gpu-cloud"],
    menu_detail: "OpenAI-compatible LLM API",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(COMTEGRA_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(22), Some(22), Some(22), Some(22), Some(22)),
};

pub const FPT_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "fpt",
    display_name: "FPT AI Marketplace",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["fpt-ai", "fptcloud", "fpt-cloud"],
    menu_detail: "OpenAI-compatible FPT AI Marketplace API",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(FPT_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(23), Some(23), Some(23), Some(23), Some(23)),
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
    order: LoginProviderSurfaceOrder::new(Some(24), Some(24), Some(24), Some(24), Some(24)),
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
    order: LoginProviderSurfaceOrder::new(Some(25), Some(25), Some(25), Some(25), Some(25)),
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
    order: LoginProviderSurfaceOrder::new(Some(26), Some(26), Some(26), Some(26), Some(26)),
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
    order: LoginProviderSurfaceOrder::new(Some(27), Some(27), Some(27), Some(27), Some(27)),
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
    order: LoginProviderSurfaceOrder::new(Some(28), Some(28), Some(28), Some(28), Some(28)),
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
    order: LoginProviderSurfaceOrder::new(Some(29), Some(29), Some(29), Some(29), Some(29)),
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
    order: LoginProviderSurfaceOrder::new(Some(30), Some(30), Some(30), Some(30), Some(30)),
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

pub const NVIDIA_NIM_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "nvidia-nim",
    display_name: "NVIDIA NIM",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["nvidia", "nim"],
    menu_detail: "API key",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(NVIDIA_NIM_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(34), Some(34), Some(34), Some(34), Some(34)),
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
    menu_detail: "custom endpoint setup: base URL first, then API key",
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

pub const XIAOMI_MIMO_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "xiaomi-mimo",
    display_name: "Xiaomi MiMo",
    auth_kind: LoginProviderAuthKind::ApiKey,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["xiaomi", "mimo", "xiaomi-mimo-api"],
    menu_detail: "OpenAI-compatible Xiaomi MiMo API",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(XIAOMI_MIMO_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(37), Some(37), Some(37), Some(37), Some(37)),
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

pub(crate) const LOGIN_PROVIDERS: [LoginProviderDescriptor; 46] = [
    AUTO_IMPORT_LOGIN_PROVIDER,
    CLAUDE_LOGIN_PROVIDER,
    ANTHROPIC_API_LOGIN_PROVIDER,
    OPENAI_LOGIN_PROVIDER,
    OPENAI_API_LOGIN_PROVIDER,
    JCODE_LOGIN_PROVIDER,
    OPENROUTER_LOGIN_PROVIDER,
    BEDROCK_LOGIN_PROVIDER,
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
    COMTEGRA_LOGIN_PROVIDER,
    FPT_LOGIN_PROVIDER,
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
    NVIDIA_NIM_LOGIN_PROVIDER,
    XIAOMI_MIMO_LOGIN_PROVIDER,
    LMSTUDIO_LOGIN_PROVIDER,
    OLLAMA_LOGIN_PROVIDER,
    OPENAI_COMPAT_LOGIN_PROVIDER,
    CURSOR_LOGIN_PROVIDER,
    COPILOT_LOGIN_PROVIDER,
    GEMINI_LOGIN_PROVIDER,
    ANTIGRAVITY_LOGIN_PROVIDER,
    GOOGLE_LOGIN_PROVIDER,
];
