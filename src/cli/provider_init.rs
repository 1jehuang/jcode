use anyhow::Result;
use std::io::{self, IsTerminal, Write};
use std::sync::Arc;

use crate::auth;
use crate::provider;
use crate::provider::Provider;
use crate::provider_catalog::{
    LoginProviderDescriptor, LoginProviderTarget, OpenAiCompatibleProfile,
    apply_openai_compatible_profile_env, is_safe_env_file_name, is_safe_env_key_name,
    resolve_login_selection, resolve_openai_compatible_profile,
};
use crate::tool;

use super::login::run_login_provider;
use super::output;

mod external_auth;
use external_auth::*;
pub(crate) use external_auth::{
    ExternalAuthReviewCandidate, format_external_auth_review_candidates_markdown,
    maybe_run_external_auth_auto_import_flow, parse_external_auth_review_selection,
    pending_external_auth_review_candidates, run_external_auth_auto_import_candidates,
};

#[derive(Debug, Clone, PartialEq, Eq, clap::ValueEnum)]
pub enum ProviderChoice {
    Jcode,
    Claude,
    #[deprecated(
        note = "Claude Code CLI subprocess transport is deprecated; use ProviderChoice::Claude for native Anthropic OAuth/API transport"
    )]
    #[value(alias = "claude-subprocess", hide = true)]
    ClaudeSubprocess,
    Openai,
    #[value(
        alias = "openai-key",
        alias = "openai-apikey",
        alias = "openai-platform"
    )]
    OpenaiApi,
    Openrouter,
    #[value(alias = "azure-openai", alias = "aoai")]
    Azure,
    #[value(alias = "opencode-zen", alias = "zen")]
    Opencode,
    #[value(alias = "opencodego")]
    OpencodeGo,
    #[value(alias = "z.ai", alias = "z-ai", alias = "zai-coding")]
    Zai,
    #[value(
        alias = "kimi-code",
        alias = "kimi-coding",
        alias = "kimi-coding-plan",
        alias = "kimi-for-coding",
        alias = "moonshot-coding"
    )]
    Kimi,
    #[value(alias = "302.ai")]
    Ai302,
    Baseten,
    Cortecs,
    #[value(alias = "cgc", alias = "comtegra-gpu-cloud")]
    Comtegra,
    Deepseek,
    Firmware,
    #[value(alias = "hugging-face", alias = "hf")]
    HuggingFace,
    #[value(alias = "moonshot")]
    MoonshotAi,
    Nebius,
    Scaleway,
    Stackit,
    Groq,
    #[value(alias = "mistralai")]
    Mistral,
    #[value(alias = "pplx")]
    Perplexity,
    #[value(alias = "together", alias = "together-ai")]
    TogetherAi,
    #[value(alias = "deep-infra")]
    Deepinfra,
    #[value(alias = "fireworks-ai", alias = "fireworks.ai")]
    Fireworks,
    #[value(alias = "minimax-ai", alias = "minimaxi")]
    Minimax,
    #[value(alias = "minimax-coding-plan")]
    MinimaxTokenPlan,
    #[value(alias = "x.ai", alias = "x-ai", alias = "grok")]
    Xai,
    #[value(alias = "lm-studio")]
    Lmstudio,
    Ollama,
    Chutes,
    #[value(alias = "cerebrascode", alias = "cerberascode")]
    Cerebras,
    #[value(
        alias = "bailian",
        alias = "aliyun-bailian",
        alias = "coding-plan",
        alias = "alibaba-coding"
    )]
    AlibabaCodingPlan,
    #[value(alias = "compat", alias = "custom")]
    OpenaiCompatible,
    Cursor,
    Copilot,
    Gemini,
    Antigravity,
    Google,
    Auto,
}

impl ProviderChoice {
    #[allow(deprecated)]
    pub fn as_arg_value(&self) -> &'static str {
        match self {
            Self::Jcode => "jcode",
            Self::Claude => "claude",
            Self::ClaudeSubprocess => "claude-subprocess",
            Self::Openai => "openai",
            Self::OpenaiApi => "openai-api",
            Self::Openrouter => "openrouter",
            Self::Azure => "azure",
            Self::Opencode => "opencode",
            Self::OpencodeGo => "opencode-go",
            Self::Zai => "zai",
            Self::Kimi => "kimi",
            Self::Ai302 => "302ai",
            Self::Baseten => "baseten",
            Self::Cortecs => "cortecs",
            Self::Comtegra => "comtegra",
            Self::Deepseek => "deepseek",
            Self::Firmware => "firmware",
            Self::HuggingFace => "huggingface",
            Self::MoonshotAi => "moonshotai",
            Self::Nebius => "nebius",
            Self::Scaleway => "scaleway",
            Self::Stackit => "stackit",
            Self::Groq => "groq",
            Self::Mistral => "mistral",
            Self::Perplexity => "perplexity",
            Self::TogetherAi => "togetherai",
            Self::Deepinfra => "deepinfra",
            Self::Fireworks => "fireworks",
            Self::Minimax => "minimax",
            Self::MinimaxTokenPlan => "minimax-token-plan",
            Self::Xai => "xai",
            Self::Lmstudio => "lmstudio",
            Self::Ollama => "ollama",
            Self::Chutes => "chutes",
            Self::Cerebras => "cerebras",
            Self::AlibabaCodingPlan => "alibaba-coding-plan",
            Self::OpenaiCompatible => "openai-compatible",
            Self::Cursor => "cursor",
            Self::Copilot => "copilot",
            Self::Gemini => "gemini",
            Self::Antigravity => "antigravity",
            Self::Google => "google",
            Self::Auto => "auto",
        }
    }
}

pub fn profile_for_choice(choice: &ProviderChoice) -> Option<OpenAiCompatibleProfile> {
    match choice {
        ProviderChoice::Opencode => Some(crate::provider_catalog::OPENCODE_PROFILE),
        ProviderChoice::OpencodeGo => Some(crate::provider_catalog::OPENCODE_GO_PROFILE),
        ProviderChoice::Zai => Some(crate::provider_catalog::ZAI_PROFILE),
        ProviderChoice::Kimi => Some(crate::provider_catalog::KIMI_PROFILE),
        ProviderChoice::Ai302 => Some(crate::provider_catalog::AI302_PROFILE),
        ProviderChoice::Baseten => Some(crate::provider_catalog::BASETEN_PROFILE),
        ProviderChoice::Cortecs => Some(crate::provider_catalog::CORTECS_PROFILE),
        ProviderChoice::Comtegra => Some(crate::provider_catalog::COMTEGRA_PROFILE),
        ProviderChoice::Deepseek => Some(crate::provider_catalog::DEEPSEEK_PROFILE),
        ProviderChoice::Firmware => Some(crate::provider_catalog::FIRMWARE_PROFILE),
        ProviderChoice::HuggingFace => Some(crate::provider_catalog::HUGGING_FACE_PROFILE),
        ProviderChoice::MoonshotAi => Some(crate::provider_catalog::MOONSHOT_PROFILE),
        ProviderChoice::Nebius => Some(crate::provider_catalog::NEBIUS_PROFILE),
        ProviderChoice::Scaleway => Some(crate::provider_catalog::SCALEWAY_PROFILE),
        ProviderChoice::Stackit => Some(crate::provider_catalog::STACKIT_PROFILE),
        ProviderChoice::Groq => Some(crate::provider_catalog::GROQ_PROFILE),
        ProviderChoice::Mistral => Some(crate::provider_catalog::MISTRAL_PROFILE),
        ProviderChoice::Perplexity => Some(crate::provider_catalog::PERPLEXITY_PROFILE),
        ProviderChoice::TogetherAi => Some(crate::provider_catalog::TOGETHER_AI_PROFILE),
        ProviderChoice::Deepinfra => Some(crate::provider_catalog::DEEPINFRA_PROFILE),
        ProviderChoice::Fireworks => Some(crate::provider_catalog::FIREWORKS_PROFILE),
        ProviderChoice::Minimax => Some(crate::provider_catalog::MINIMAX_PROFILE),
        ProviderChoice::MinimaxTokenPlan => {
            Some(crate::provider_catalog::MINIMAX_TOKEN_PLAN_PROFILE)
        }
        ProviderChoice::Xai => Some(crate::provider_catalog::XAI_PROFILE),
        ProviderChoice::Lmstudio => Some(crate::provider_catalog::LMSTUDIO_PROFILE),
        ProviderChoice::Ollama => Some(crate::provider_catalog::OLLAMA_PROFILE),
        ProviderChoice::Chutes => Some(crate::provider_catalog::CHUTES_PROFILE),
        ProviderChoice::Cerebras => Some(crate::provider_catalog::CEREBRAS_PROFILE),
        ProviderChoice::AlibabaCodingPlan => {
            Some(crate::provider_catalog::ALIBABA_CODING_PLAN_PROFILE)
        }
        ProviderChoice::OpenaiCompatible => Some(crate::provider_catalog::OPENAI_COMPAT_PROFILE),
        _ => None,
    }
}

#[allow(deprecated)]
pub fn login_provider_for_choice(choice: &ProviderChoice) -> Option<LoginProviderDescriptor> {
    match choice {
        ProviderChoice::Jcode => Some(crate::provider_catalog::JCODE_LOGIN_PROVIDER),
        ProviderChoice::Claude | ProviderChoice::ClaudeSubprocess => {
            Some(crate::provider_catalog::CLAUDE_LOGIN_PROVIDER)
        }
        ProviderChoice::Openai => Some(crate::provider_catalog::OPENAI_LOGIN_PROVIDER),
        ProviderChoice::OpenaiApi => Some(crate::provider_catalog::OPENAI_API_LOGIN_PROVIDER),
        ProviderChoice::Openrouter => Some(crate::provider_catalog::OPENROUTER_LOGIN_PROVIDER),
        ProviderChoice::Azure => Some(crate::provider_catalog::AZURE_LOGIN_PROVIDER),
        ProviderChoice::Opencode => Some(crate::provider_catalog::OPENCODE_LOGIN_PROVIDER),
        ProviderChoice::OpencodeGo => Some(crate::provider_catalog::OPENCODE_GO_LOGIN_PROVIDER),
        ProviderChoice::Zai => Some(crate::provider_catalog::ZAI_LOGIN_PROVIDER),
        ProviderChoice::Kimi => Some(crate::provider_catalog::KIMI_LOGIN_PROVIDER),
        ProviderChoice::Ai302 => Some(crate::provider_catalog::AI302_LOGIN_PROVIDER),
        ProviderChoice::Baseten => Some(crate::provider_catalog::BASETEN_LOGIN_PROVIDER),
        ProviderChoice::Cortecs => Some(crate::provider_catalog::CORTECS_LOGIN_PROVIDER),
        ProviderChoice::Comtegra => Some(crate::provider_catalog::COMTEGRA_LOGIN_PROVIDER),
        ProviderChoice::Deepseek => Some(crate::provider_catalog::DEEPSEEK_LOGIN_PROVIDER),
        ProviderChoice::Firmware => Some(crate::provider_catalog::FIRMWARE_LOGIN_PROVIDER),
        ProviderChoice::HuggingFace => Some(crate::provider_catalog::HUGGING_FACE_LOGIN_PROVIDER),
        ProviderChoice::MoonshotAi => Some(crate::provider_catalog::MOONSHOT_LOGIN_PROVIDER),
        ProviderChoice::Nebius => Some(crate::provider_catalog::NEBIUS_LOGIN_PROVIDER),
        ProviderChoice::Scaleway => Some(crate::provider_catalog::SCALEWAY_LOGIN_PROVIDER),
        ProviderChoice::Stackit => Some(crate::provider_catalog::STACKIT_LOGIN_PROVIDER),
        ProviderChoice::Groq => Some(crate::provider_catalog::GROQ_LOGIN_PROVIDER),
        ProviderChoice::Mistral => Some(crate::provider_catalog::MISTRAL_LOGIN_PROVIDER),
        ProviderChoice::Perplexity => Some(crate::provider_catalog::PERPLEXITY_LOGIN_PROVIDER),
        ProviderChoice::TogetherAi => Some(crate::provider_catalog::TOGETHER_AI_LOGIN_PROVIDER),
        ProviderChoice::Deepinfra => Some(crate::provider_catalog::DEEPINFRA_LOGIN_PROVIDER),
        ProviderChoice::Fireworks => Some(crate::provider_catalog::FIREWORKS_LOGIN_PROVIDER),
        ProviderChoice::Minimax => Some(crate::provider_catalog::MINIMAX_LOGIN_PROVIDER),
        ProviderChoice::MinimaxTokenPlan => {
            Some(crate::provider_catalog::MINIMAX_TOKEN_PLAN_LOGIN_PROVIDER)
        }
        ProviderChoice::Xai => Some(crate::provider_catalog::XAI_LOGIN_PROVIDER),
        ProviderChoice::Lmstudio => Some(crate::provider_catalog::LMSTUDIO_LOGIN_PROVIDER),
        ProviderChoice::Ollama => Some(crate::provider_catalog::OLLAMA_LOGIN_PROVIDER),
        ProviderChoice::Chutes => Some(crate::provider_catalog::CHUTES_LOGIN_PROVIDER),
        ProviderChoice::Cerebras => Some(crate::provider_catalog::CEREBRAS_LOGIN_PROVIDER),
        ProviderChoice::AlibabaCodingPlan => {
            Some(crate::provider_catalog::ALIBABA_CODING_PLAN_LOGIN_PROVIDER)
        }
        ProviderChoice::OpenaiCompatible => {
            Some(crate::provider_catalog::OPENAI_COMPAT_LOGIN_PROVIDER)
        }
        ProviderChoice::Cursor => Some(crate::provider_catalog::CURSOR_LOGIN_PROVIDER),
        ProviderChoice::Copilot => Some(crate::provider_catalog::COPILOT_LOGIN_PROVIDER),
        ProviderChoice::Gemini => Some(crate::provider_catalog::GEMINI_LOGIN_PROVIDER),
        ProviderChoice::Antigravity => Some(crate::provider_catalog::ANTIGRAVITY_LOGIN_PROVIDER),
        ProviderChoice::Google => Some(crate::provider_catalog::GOOGLE_LOGIN_PROVIDER),
        ProviderChoice::Auto => None,
    }
}

pub fn choice_for_login_provider(provider: LoginProviderDescriptor) -> Option<ProviderChoice> {
    match provider.target {
        LoginProviderTarget::AutoImport => None,
        LoginProviderTarget::Jcode => Some(ProviderChoice::Jcode),
        LoginProviderTarget::Claude => Some(ProviderChoice::Claude),
        LoginProviderTarget::OpenAi => Some(ProviderChoice::Openai),
        LoginProviderTarget::OpenAiApiKey => Some(ProviderChoice::OpenaiApi),
        LoginProviderTarget::OpenRouter => Some(ProviderChoice::Openrouter),
        LoginProviderTarget::Azure => Some(ProviderChoice::Azure),
        LoginProviderTarget::OpenAiCompatible(profile) => [
            ProviderChoice::Opencode,
            ProviderChoice::OpencodeGo,
            ProviderChoice::Zai,
            ProviderChoice::Kimi,
            ProviderChoice::Ai302,
            ProviderChoice::Baseten,
            ProviderChoice::Cortecs,
            ProviderChoice::Comtegra,
            ProviderChoice::Deepseek,
            ProviderChoice::Firmware,
            ProviderChoice::HuggingFace,
            ProviderChoice::MoonshotAi,
            ProviderChoice::Nebius,
            ProviderChoice::Scaleway,
            ProviderChoice::Stackit,
            ProviderChoice::Groq,
            ProviderChoice::Mistral,
            ProviderChoice::Perplexity,
            ProviderChoice::TogetherAi,
            ProviderChoice::Deepinfra,
            ProviderChoice::Fireworks,
            ProviderChoice::Minimax,
            ProviderChoice::MinimaxTokenPlan,
            ProviderChoice::Xai,
            ProviderChoice::Lmstudio,
            ProviderChoice::Ollama,
            ProviderChoice::Chutes,
            ProviderChoice::Cerebras,
            ProviderChoice::AlibabaCodingPlan,
            ProviderChoice::OpenaiCompatible,
        ]
        .into_iter()
        .find(|choice| profile_for_choice(choice) == Some(profile)),
        LoginProviderTarget::Cursor => Some(ProviderChoice::Cursor),
        LoginProviderTarget::Copilot => Some(ProviderChoice::Copilot),
        LoginProviderTarget::Gemini => Some(ProviderChoice::Gemini),
        LoginProviderTarget::Antigravity => Some(ProviderChoice::Antigravity),
        LoginProviderTarget::Google => Some(ProviderChoice::Google),
    }
}

pub fn prompt_login_provider_selection(
    providers: &[LoginProviderDescriptor],
    heading: &str,
) -> Result<LoginProviderDescriptor> {
    prompt_login_provider_selection_optional(providers, heading)?.ok_or_else(|| {
        anyhow::anyhow!("Login skipped. Run `jcode login` when you're ready to authenticate.")
    })
}

pub fn prompt_login_provider_selection_optional(
    providers: &[LoginProviderDescriptor],
    heading: &str,
) -> Result<Option<LoginProviderDescriptor>> {
    let status = auth::AuthStatus::check_fast();
    eprint!(
        "{}",
        render_login_provider_selection_menu(heading, providers, &status)
    );
    eprint!(
        "\nEnter 1-{}, provider name, or Enter=skip: ",
        providers.len()
    );
    io::stderr().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    parse_login_provider_selection_input(&input, providers)
}

pub fn parse_login_provider_selection_input(
    input: &str,
    providers: &[LoginProviderDescriptor],
) -> Result<Option<LoginProviderDescriptor>> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let normalized = trimmed.to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "s" | "skip" | "q" | "quit" | "cancel" | "none"
    ) {
        return Ok(None);
    }

    resolve_login_selection(trimmed, providers)
        .map(Some)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Invalid choice '{}'. Enter 1-{}, a provider name, or 'skip'.",
                trimmed,
                providers.len()
            )
        })
}

pub fn render_login_provider_selection_menu(
    heading: &str,
    providers: &[LoginProviderDescriptor],
    status: &auth::AuthStatus,
) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out, "{heading}");
    let _ = writeln!(out);

    let detected = providers
        .iter()
        .copied()
        .filter_map(|provider| {
            let assessment = status.assessment_for_provider(provider);
            (assessment.state != auth::AuthState::NotConfigured).then(|| {
                format!(
                    "  - {}: {}",
                    provider.display_name,
                    login_provider_detection_detail(provider, &assessment)
                )
            })
        })
        .collect::<Vec<_>>();

    if detected.is_empty() {
        let _ = writeln!(out, "Autodetected auth: none found yet.");
    } else {
        let _ = writeln!(out, "Autodetected auth:");
        for line in detected {
            let _ = writeln!(out, "{line}");
        }
    }

    let _ = writeln!(out);
    for (index, provider) in providers.iter().copied().enumerate() {
        let auth_state = status.state_for_provider(provider);
        let _ = writeln!(
            out,
            "  {}. {:<22} [{:<15}] - {}",
            index + 1,
            provider.display_name,
            login_provider_state_badge(provider, auth_state),
            provider.menu_detail
        );
    }

    let recommended = providers
        .iter()
        .filter(|provider| provider.recommended)
        .map(|provider| provider.display_name)
        .collect::<Vec<_>>();
    if !recommended.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "  Recommended if you have a subscription: {}.",
            recommended.join(", ")
        );
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "  Skip: press Enter, or type `skip`.");
    out
}

fn login_provider_state_badge(
    provider: LoginProviderDescriptor,
    state: auth::AuthState,
) -> &'static str {
    match state {
        auth::AuthState::Available => {
            if matches!(provider.target, LoginProviderTarget::AutoImport) {
                "detected"
            } else {
                "configured"
            }
        }
        auth::AuthState::Expired => "needs attention",
        auth::AuthState::NotConfigured => "not configured",
    }
}

fn login_provider_detection_detail(
    provider: LoginProviderDescriptor,
    assessment: &auth::ProviderAuthAssessment,
) -> String {
    match assessment.state {
        auth::AuthState::Available => {
            let prefix = if matches!(provider.target, LoginProviderTarget::AutoImport) {
                "detected"
            } else {
                "configured"
            };
            format!("{}: {}", prefix, assessment.method_detail)
        }
        auth::AuthState::Expired => format!("needs attention: {}", assessment.method_detail),
        auth::AuthState::NotConfigured => "not configured".to_string(),
    }
}

struct AutoProviderAvailability {
    auth_status: auth::AuthStatus,
    has_claude: bool,
    has_openai: bool,
    has_copilot: bool,
    has_antigravity: bool,
    has_gemini: bool,
    has_cursor: bool,
    has_openrouter: bool,
}

impl AutoProviderAvailability {
    fn has_any_provider(&self) -> bool {
        self.has_claude
            || self.has_openai
            || self.has_copilot
            || self.has_antigravity
            || self.has_gemini
            || self.has_cursor
            || self.has_openrouter
    }
}

async fn detect_auto_provider_flags() -> AutoProviderAvailability {
    let auth_status = auth::AuthStatus::check_fast();
    AutoProviderAvailability {
        has_claude: auth_status.anthropic.has_oauth || auth_status.anthropic.has_api_key,
        has_openai: auth_status.openai_has_oauth || auth_status.openai_has_api_key,
        has_copilot: auth_status.copilot_has_api_token,
        has_antigravity: auth::antigravity::load_tokens().is_ok(),
        has_gemini: auth_status.gemini == auth::AuthState::Available,
        has_cursor: auth_status.cursor == auth::AuthState::Available,
        has_openrouter: auth_status.openrouter == auth::AuthState::Available,
        auth_status,
    }
}

fn provider_label_for_api_key_env(env_key: &str) -> String {
    if env_key == "OPENROUTER_API_KEY" {
        return "OpenRouter".to_string();
    }

    crate::provider_catalog::openai_compatible_profiles()
        .iter()
        .find_map(|profile| {
            let resolved = resolve_openai_compatible_profile(*profile);
            (resolved.api_key_env == env_key).then_some(resolved.display_name)
        })
        .unwrap_or_else(|| env_key.to_string())
}

fn provider_login_hint_for_api_key_env(env_key: &str) -> String {
    if env_key == "OPENROUTER_API_KEY" {
        return "jcode login --provider openrouter".to_string();
    }

    crate::provider_catalog::openai_compatible_profiles()
        .iter()
        .find_map(|profile| {
            let resolved = resolve_openai_compatible_profile(*profile);
            (resolved.api_key_env == env_key)
                .then(|| format!("jcode login --provider {}", resolved.id))
        })
        .unwrap_or_else(|| "jcode login".to_string())
}

fn ensure_external_api_key_auth_allowed_for_explicit_choice(env_key: &str) -> Result<()> {
    let Some(source) = auth::external::preferred_unconsented_api_key_source_for_env(env_key) else {
        return Ok(());
    };
    let path = source.path()?;
    let provider_name = provider_label_for_api_key_env(env_key);
    let login_hint = provider_login_hint_for_api_key_env(env_key);
    if !can_prompt_for_external_auth() {
        anyhow::bail!(external_auth_blocked_message(
            &provider_name,
            source.display_name(),
            &path,
            &login_hint,
        ));
    }
    if prompt_to_trust_external_auth(&provider_name, source.display_name(), &path)? {
        auth::external::trust_external_auth_source(source)?;
        return Ok(());
    }
    anyhow::bail!(
        "Skipped trusting external {} credentials. Run `{}` to authenticate jcode directly.",
        provider_name,
        login_hint
    )
}

fn maybe_enable_external_api_key_auth_for_auto(has_other_provider: bool) -> Result<bool> {
    if provider::openrouter::OpenRouterProvider::has_credentials() {
        return Ok(true);
    }
    if has_other_provider {
        return Ok(false);
    }

    for (env_key, _) in crate::provider_catalog::openrouter_like_api_key_sources() {
        let Some(source) = auth::external::preferred_unconsented_api_key_source_for_env(&env_key)
        else {
            continue;
        };
        let path = source.path()?;
        let provider_name = provider_label_for_api_key_env(&env_key);
        let login_hint = provider_login_hint_for_api_key_env(&env_key);
        if !can_prompt_for_external_auth() {
            crate::logging::warn(&external_auth_blocked_message(
                &provider_name,
                source.display_name(),
                &path,
                &login_hint,
            ));
            return Ok(false);
        }
        if prompt_to_trust_external_auth(&provider_name, source.display_name(), &path)? {
            auth::external::trust_external_auth_source(source)?;
            return Ok(provider::openrouter::OpenRouterProvider::has_credentials());
        }
        return Ok(false);
    }

    Ok(false)
}

fn maybe_prompt_for_generic_oauth_source(
    provider_name: &str,
    source: Option<auth::external::ExternalAuthSource>,
    login_hint: &str,
    auto: bool,
    validation: impl Fn() -> bool,
) -> Result<bool> {
    let Some(source) = source else {
        return Ok(false);
    };
    let path = source.path()?;
    if !can_prompt_for_external_auth() {
        if auto {
            crate::logging::warn(&external_auth_blocked_message(
                provider_name,
                source.display_name(),
                &path,
                login_hint,
            ));
            return Ok(false);
        }
        anyhow::bail!(external_auth_blocked_message(
            provider_name,
            source.display_name(),
            &path,
            login_hint,
        ));
    }
    if prompt_to_trust_external_auth(provider_name, source.display_name(), &path)? {
        auth::external::trust_external_auth_source(source)?;
        return Ok(if auto { validation() } else { true });
    }
    Ok(false)
}

fn ensure_openai_auth_allowed_for_explicit_choice() -> Result<()> {
    if auth::codex::load_credentials().is_ok() {
        return Ok(());
    }

    if maybe_prompt_for_generic_oauth_source(
        "OpenAI/Codex",
        auth::external::preferred_unconsented_openai_oauth_source(),
        "jcode login --provider openai",
        false,
        || auth::codex::load_credentials().is_ok(),
    )? {
        return Ok(());
    }

    if !auth::codex::has_unconsented_legacy_credentials() {
        return Ok(());
    }

    let path = auth::codex::legacy_auth_file_path()?;

    if !can_prompt_for_external_auth() {
        anyhow::bail!(external_auth_blocked_message(
            "OpenAI/Codex",
            "Codex",
            &path,
            "jcode login --provider openai"
        ));
    }

    if prompt_to_trust_external_auth("OpenAI/Codex", "Codex", &path)? {
        auth::codex::trust_legacy_auth_for_future_use()?;
        return Ok(());
    }

    anyhow::bail!(
        "Skipped trusting existing ~/.codex/auth.json credentials. Run `jcode login --provider openai` to authenticate jcode directly."
    )
}

fn maybe_enable_legacy_codex_auth_for_auto(has_other_provider: bool) -> Result<bool> {
    if auth::codex::load_credentials().is_ok() {
        return Ok(true);
    }

    if let Some(source) = auth::external::preferred_unconsented_openai_oauth_source() {
        if has_other_provider {
            return Ok(false);
        }
        return maybe_prompt_for_generic_oauth_source(
            "OpenAI/Codex",
            Some(source),
            "jcode login --provider openai",
            true,
            || auth::codex::load_credentials().is_ok(),
        );
    }

    if !auth::codex::has_unconsented_legacy_credentials() {
        return Ok(false);
    }

    if has_other_provider {
        return Ok(false);
    }

    let path = auth::codex::legacy_auth_file_path()?;

    if !can_prompt_for_external_auth() {
        crate::logging::warn(&external_auth_blocked_message(
            "OpenAI/Codex",
            "Codex",
            &path,
            "jcode login --provider openai",
        ));
        return Ok(false);
    }

    if prompt_to_trust_external_auth("OpenAI/Codex", "Codex", &path)? {
        auth::codex::trust_legacy_auth_for_future_use()?;
        return Ok(auth::codex::load_credentials().is_ok());
    }

    Ok(false)
}

fn ensure_claude_auth_allowed_for_explicit_choice() -> Result<()> {
    if auth::claude::load_credentials().is_ok() {
        return Ok(());
    }

    if maybe_prompt_for_generic_oauth_source(
        "Claude",
        auth::external::preferred_unconsented_anthropic_oauth_source(),
        "jcode login --provider claude",
        false,
        || auth::claude::load_credentials().is_ok(),
    )? {
        return Ok(());
    }

    let Some(source) = auth::claude::has_unconsented_external_auth() else {
        return Ok(());
    };
    let path = source.path()?;
    if !can_prompt_for_external_auth() {
        anyhow::bail!(external_auth_blocked_message(
            "Claude",
            source.display_name(),
            &path,
            "jcode login --provider claude"
        ));
    }
    if prompt_to_trust_external_auth("Claude", source.display_name(), &path)? {
        auth::claude::trust_external_auth_source(source)?;
        return Ok(());
    }
    anyhow::bail!(
        "Skipped trusting external Claude credentials. Run `jcode login --provider claude` to authenticate jcode directly."
    )
}

fn maybe_enable_claude_auth_for_auto(has_other_provider: bool) -> Result<bool> {
    if auth::claude::load_credentials().is_ok() {
        return Ok(true);
    }

    if let Some(source) = auth::external::preferred_unconsented_anthropic_oauth_source() {
        if has_other_provider {
            return Ok(false);
        }
        return maybe_prompt_for_generic_oauth_source(
            "Claude",
            Some(source),
            "jcode login --provider claude",
            true,
            || auth::claude::load_credentials().is_ok(),
        );
    }

    let Some(source) = auth::claude::has_unconsented_external_auth() else {
        return Ok(false);
    };
    if has_other_provider {
        return Ok(false);
    }
    let path = source.path()?;
    if !can_prompt_for_external_auth() {
        crate::logging::warn(&external_auth_blocked_message(
            "Claude",
            source.display_name(),
            &path,
            "jcode login --provider claude",
        ));
        return Ok(false);
    }
    if prompt_to_trust_external_auth("Claude", source.display_name(), &path)? {
        auth::claude::trust_external_auth_source(source)?;
        return Ok(auth::claude::load_credentials().is_ok());
    }
    Ok(false)
}

fn ensure_gemini_auth_allowed_for_explicit_choice() -> Result<()> {
    if auth::gemini::load_tokens().is_ok() {
        return Ok(());
    }

    if maybe_prompt_for_generic_oauth_source(
        "Gemini",
        auth::external::preferred_unconsented_gemini_oauth_source(),
        "jcode login --provider gemini",
        false,
        || auth::gemini::load_tokens().is_ok(),
    )? {
        return Ok(());
    }

    if !auth::gemini::has_unconsented_cli_auth() {
        return Ok(());
    }
    let path = auth::gemini::gemini_cli_oauth_path()?;
    if !can_prompt_for_external_auth() {
        anyhow::bail!(external_auth_blocked_message(
            "Gemini",
            "Gemini CLI",
            &path,
            "jcode login --provider gemini"
        ));
    }
    if prompt_to_trust_external_auth("Gemini", "Gemini CLI", &path)? {
        auth::gemini::trust_cli_auth_for_future_use()?;
        return Ok(());
    }
    anyhow::bail!(
        "Skipped trusting Gemini CLI credentials. Run `jcode login --provider gemini` to authenticate jcode directly."
    )
}

fn maybe_enable_gemini_auth_for_auto(has_other_provider: bool) -> Result<bool> {
    if auth::gemini::load_tokens().is_ok() {
        return Ok(true);
    }

    if let Some(source) = auth::external::preferred_unconsented_gemini_oauth_source() {
        if has_other_provider {
            return Ok(false);
        }
        return maybe_prompt_for_generic_oauth_source(
            "Gemini",
            Some(source),
            "jcode login --provider gemini",
            true,
            || auth::gemini::load_tokens().is_ok(),
        );
    }

    if !auth::gemini::has_unconsented_cli_auth() {
        return Ok(false);
    }
    if has_other_provider {
        return Ok(false);
    }
    let path = auth::gemini::gemini_cli_oauth_path()?;
    if !can_prompt_for_external_auth() {
        crate::logging::warn(&external_auth_blocked_message(
            "Gemini",
            "Gemini CLI",
            &path,
            "jcode login --provider gemini",
        ));
        return Ok(false);
    }
    if prompt_to_trust_external_auth("Gemini", "Gemini CLI", &path)? {
        auth::gemini::trust_cli_auth_for_future_use()?;
        return Ok(auth::gemini::load_tokens().is_ok());
    }
    Ok(false)
}

fn ensure_antigravity_auth_allowed_for_explicit_choice() -> Result<()> {
    if auth::antigravity::load_tokens().is_ok() {
        return Ok(());
    }

    if maybe_prompt_for_generic_oauth_source(
        "Antigravity",
        auth::external::preferred_unconsented_antigravity_oauth_source(),
        "jcode login --provider antigravity",
        false,
        || auth::antigravity::load_tokens().is_ok(),
    )? {
        return Ok(());
    }

    Ok(())
}

fn ensure_copilot_auth_allowed_for_explicit_choice() -> Result<()> {
    if auth::copilot::load_github_token().is_ok() {
        return Ok(());
    }
    let Some(source) = auth::copilot::has_unconsented_external_auth() else {
        return Ok(());
    };
    let path = source.path();
    if !can_prompt_for_external_auth() {
        anyhow::bail!(external_auth_blocked_message(
            "GitHub Copilot",
            source.display_name(),
            &path,
            "jcode login --provider copilot"
        ));
    }
    if prompt_to_trust_external_auth("GitHub Copilot", source.display_name(), &path)? {
        auth::copilot::trust_external_auth_source(source)?;
        return Ok(());
    }
    anyhow::bail!(
        "Skipped trusting external Copilot credentials. Run `jcode login --provider copilot` to authenticate jcode directly."
    )
}

fn maybe_enable_copilot_auth_for_auto(has_other_provider: bool) -> Result<bool> {
    if auth::copilot::load_github_token().is_ok() {
        return Ok(true);
    }
    let Some(source) = auth::copilot::has_unconsented_external_auth() else {
        return Ok(false);
    };
    if has_other_provider {
        return Ok(false);
    }
    let path = source.path();
    if !can_prompt_for_external_auth() {
        crate::logging::warn(&external_auth_blocked_message(
            "GitHub Copilot",
            source.display_name(),
            &path,
            "jcode login --provider copilot",
        ));
        return Ok(false);
    }
    if prompt_to_trust_external_auth("GitHub Copilot", source.display_name(), &path)? {
        auth::copilot::trust_external_auth_source(source)?;
        return Ok(auth::copilot::load_github_token().is_ok());
    }
    Ok(false)
}

fn ensure_cursor_auth_allowed_for_explicit_choice() -> Result<()> {
    if auth::cursor::has_cursor_native_auth() || auth::cursor::has_cursor_api_key() {
        return Ok(());
    }
    let Some(source) = auth::cursor::has_unconsented_external_auth() else {
        return Ok(());
    };
    let path = source.path()?;
    if !can_prompt_for_external_auth() {
        anyhow::bail!(external_auth_blocked_message(
            "Cursor",
            source.display_name(),
            &path,
            "jcode login --provider cursor"
        ));
    }
    if prompt_to_trust_external_auth("Cursor", source.display_name(), &path)? {
        auth::cursor::trust_external_auth_source(source)?;
        return Ok(());
    }
    anyhow::bail!(
        "Skipped trusting external Cursor credentials. Run `jcode login --provider cursor` to authenticate jcode directly."
    )
}

fn maybe_enable_cursor_auth_for_auto(has_other_provider: bool) -> Result<bool> {
    if auth::cursor::has_cursor_native_auth() || auth::cursor::has_cursor_api_key() {
        return Ok(true);
    }
    let Some(source) = auth::cursor::has_unconsented_external_auth() else {
        return Ok(false);
    };
    if has_other_provider {
        return Ok(false);
    }
    let path = source.path()?;
    if !can_prompt_for_external_auth() {
        crate::logging::warn(&external_auth_blocked_message(
            "Cursor",
            source.display_name(),
            &path,
            "jcode login --provider cursor",
        ));
        return Ok(false);
    }
    if prompt_to_trust_external_auth("Cursor", source.display_name(), &path)? {
        auth::cursor::trust_external_auth_source(source)?;
        return Ok(auth::cursor::has_cursor_native_auth());
    }
    Ok(false)
}

pub fn lock_model_provider(provider_key: &str) {
    crate::env::set_var("JCODE_ACTIVE_PROVIDER", provider_key);
    crate::env::set_var("JCODE_FORCE_PROVIDER", "1");
}

pub fn unlock_model_provider() {
    crate::env::remove_var("JCODE_FORCE_PROVIDER");
}

fn disable_subscription_runtime_mode() {
    crate::subscription_catalog::clear_runtime_env();
}

pub fn apply_login_provider_profile_env(provider: LoginProviderDescriptor) {
    if let LoginProviderTarget::OpenAiCompatible(profile) = provider.target {
        apply_openai_compatible_profile_env(Some(profile));
    }
}

fn resolved_profile_default_model(profile: OpenAiCompatibleProfile) -> Option<String> {
    resolve_openai_compatible_profile(profile).default_model
}

pub async fn login_and_bootstrap_provider(
    provider: LoginProviderDescriptor,
    account_label: Option<&str>,
) -> Result<Arc<dyn provider::Provider>> {
    run_login_provider(
        provider,
        account_label,
        crate::cli::login::LoginOptions::default(),
    )
    .await?;
    eprintln!();

    let runtime: Arc<dyn provider::Provider> = match provider.target {
        LoginProviderTarget::AutoImport => {
            disable_subscription_runtime_mode();
            Arc::new(provider::MultiProvider::new())
        }
        LoginProviderTarget::Jcode => Arc::new(provider::jcode::JcodeProvider::new()),
        LoginProviderTarget::Claude => {
            disable_subscription_runtime_mode();
            Arc::new(provider::MultiProvider::new())
        }
        LoginProviderTarget::OpenAi => {
            disable_subscription_runtime_mode();
            Arc::new(provider::MultiProvider::with_preference(true))
        }
        LoginProviderTarget::OpenAiApiKey => {
            disable_subscription_runtime_mode();
            lock_model_provider("openai");
            Arc::new(provider::MultiProvider::with_preference(true))
        }
        LoginProviderTarget::OpenRouter => {
            disable_subscription_runtime_mode();
            Arc::new(provider::MultiProvider::new())
        }
        LoginProviderTarget::Azure => {
            disable_subscription_runtime_mode();
            crate::auth::azure::apply_runtime_env()?;
            lock_model_provider("openrouter");
            let multi = provider::MultiProvider::new();
            if let Some(model) = crate::auth::azure::load_model() {
                let _ = multi.set_model(&model);
            }
            Arc::new(multi)
        }
        LoginProviderTarget::OpenAiCompatible(profile) => {
            disable_subscription_runtime_mode();
            apply_openai_compatible_profile_env(Some(profile));
            lock_model_provider("openrouter");
            let multi = provider::MultiProvider::new();
            let resolved = resolve_openai_compatible_profile(profile);
            if let Some(model) = resolved.default_model.as_deref() {
                let _ = multi.set_model(model);
            }
            Arc::new(multi)
        }
        LoginProviderTarget::Cursor => {
            disable_subscription_runtime_mode();
            unlock_model_provider();
            crate::env::set_var("JCODE_ACTIVE_PROVIDER", "cursor");
            Arc::new(provider::cursor::CursorCliProvider::new())
        }
        LoginProviderTarget::Copilot => {
            disable_subscription_runtime_mode();
            Arc::new(provider::MultiProvider::new())
        }
        LoginProviderTarget::Gemini => {
            disable_subscription_runtime_mode();
            unlock_model_provider();
            crate::env::set_var("JCODE_ACTIVE_PROVIDER", "gemini");
            Arc::new(provider::gemini::GeminiProvider::new())
        }
        LoginProviderTarget::Antigravity => {
            disable_subscription_runtime_mode();
            unlock_model_provider();
            crate::env::set_var("JCODE_ACTIVE_PROVIDER", "antigravity");
            Arc::new(provider::antigravity::AntigravityProvider::new())
        }
        LoginProviderTarget::Google => {
            anyhow::bail!("Google login cannot be used as a model provider bootstrap");
        }
    };

    Ok(runtime)
}

pub fn save_named_api_key(env_file: &str, key_name: &str, key: &str) -> Result<()> {
    if !is_safe_env_key_name(key_name) {
        anyhow::bail!("Invalid API key variable name: {}", key_name);
    }
    if !is_safe_env_file_name(env_file) {
        anyhow::bail!("Invalid env file name: {}", env_file);
    }

    let config_dir = crate::storage::app_config_dir()?;
    let file_path = config_dir.join(env_file);
    crate::storage::upsert_env_file_value(&file_path, key_name, Some(key))?;

    crate::env::set_var(key_name, key);
    Ok(())
}

pub async fn init_provider(
    choice: &ProviderChoice,
    model: Option<&str>,
) -> Result<Arc<dyn provider::Provider>> {
    init_provider_with_options(choice, model, true, true).await
}

pub async fn init_provider_quiet(
    choice: &ProviderChoice,
    model: Option<&str>,
) -> Result<Arc<dyn provider::Provider>> {
    init_provider_with_options(choice, model, false, true).await
}

pub async fn init_provider_for_validation(
    choice: &ProviderChoice,
    model: Option<&str>,
) -> Result<Arc<dyn provider::Provider>> {
    init_provider_with_options(choice, model, false, false).await
}

#[allow(deprecated)]
async fn init_provider_with_options(
    choice: &ProviderChoice,
    model: Option<&str>,
    show_init_messages: bool,
    allow_login_bootstrap: bool,
) -> Result<Arc<dyn provider::Provider>> {
    if let Ok(profile_name) = std::env::var("JCODE_PROVIDER_PROFILE_NAME")
        && !profile_name.trim().is_empty()
    {
        crate::provider_catalog::apply_named_provider_profile_env(profile_name.trim())?;
        crate::env::set_var("JCODE_PROVIDER_PROFILE_ACTIVE", "1");
    }

    if std::env::var_os("JCODE_PROVIDER_PROFILE_ACTIVE").is_none()
        && std::env::var_os("JCODE_NAMED_PROVIDER_PROFILE").is_none()
    {
        if let Some(profile) = profile_for_choice(choice) {
            apply_openai_compatible_profile_env(Some(profile));
        } else {
            apply_openai_compatible_profile_env(None);
        }
    }

    let init_notice = |message: &str| {
        if show_init_messages {
            output::stderr_info(message);
        }
    };

    let provider: Arc<dyn provider::Provider> = match choice {
        ProviderChoice::Jcode => {
            init_notice("Using Jcode subscription provider (provider locked)");
            Arc::new(provider::jcode::JcodeProvider::new())
        }
        ProviderChoice::Claude => {
            disable_subscription_runtime_mode();
            ensure_claude_auth_allowed_for_explicit_choice()?;
            init_notice("Using Claude (provider locked)");
            lock_model_provider("claude");
            Arc::new(provider::MultiProvider::with_preference_fast(false))
        }
        ProviderChoice::ClaudeSubprocess => {
            disable_subscription_runtime_mode();
            ensure_claude_auth_allowed_for_explicit_choice()?;
            crate::logging::warn(
                "Using --provider claude-subprocess is deprecated and will be removed. Prefer `--provider claude`.",
            );
            crate::env::set_var("JCODE_USE_CLAUDE_CLI", "1");
            init_notice(
                "Using deprecated Claude subprocess transport (legacy compatibility mode; provider locked)",
            );
            lock_model_provider("claude");
            Arc::new(provider::MultiProvider::with_preference_fast(false))
        }
        ProviderChoice::Openai => {
            disable_subscription_runtime_mode();
            ensure_openai_auth_allowed_for_explicit_choice()?;
            init_notice("Using OpenAI (provider locked)");
            lock_model_provider("openai");
            Arc::new(provider::MultiProvider::with_preference_fast(true))
        }
        ProviderChoice::OpenaiApi => {
            disable_subscription_runtime_mode();
            ensure_external_api_key_auth_allowed_for_explicit_choice("OPENAI_API_KEY")?;
            init_notice("Using OpenAI API key provider (provider locked)");
            lock_model_provider("openai");
            Arc::new(provider::MultiProvider::with_preference_fast(true))
        }
        ProviderChoice::Cursor => {
            disable_subscription_runtime_mode();
            ensure_cursor_auth_allowed_for_explicit_choice()?;
            init_notice("Using Cursor native HTTPS provider (experimental)");
            unlock_model_provider();
            crate::env::set_var("JCODE_ACTIVE_PROVIDER", "cursor");
            Arc::new(provider::cursor::CursorCliProvider::new())
        }
        ProviderChoice::Copilot => {
            disable_subscription_runtime_mode();
            ensure_copilot_auth_allowed_for_explicit_choice()?;
            init_notice("Using GitHub Copilot API provider (provider locked)");
            lock_model_provider("copilot");
            Arc::new(provider::MultiProvider::new_fast())
        }
        ProviderChoice::Gemini => {
            disable_subscription_runtime_mode();
            ensure_gemini_auth_allowed_for_explicit_choice()?;
            init_notice("Using Gemini provider (native Google Code Assist OAuth)");
            unlock_model_provider();
            crate::env::set_var("JCODE_ACTIVE_PROVIDER", "gemini");
            Arc::new(provider::gemini::GeminiProvider::new())
        }
        ProviderChoice::Openrouter => {
            disable_subscription_runtime_mode();
            ensure_external_api_key_auth_allowed_for_explicit_choice("OPENROUTER_API_KEY")?;
            init_notice("Using OpenRouter provider (provider locked)");
            lock_model_provider("openrouter");
            Arc::new(provider::MultiProvider::new_fast())
        }
        ProviderChoice::Azure => {
            disable_subscription_runtime_mode();
            crate::auth::azure::apply_runtime_env()?;
            init_notice("Using Azure OpenAI provider (provider locked)");
            lock_model_provider("openrouter");
            let multi = provider::MultiProvider::new_fast();
            if let Some(model) = crate::auth::azure::load_model() {
                let _ = multi.set_model(&model);
            }
            Arc::new(multi)
        }
        ProviderChoice::Opencode
        | ProviderChoice::OpencodeGo
        | ProviderChoice::Zai
        | ProviderChoice::Ai302
        | ProviderChoice::Baseten
        | ProviderChoice::Cortecs
        | ProviderChoice::Comtegra
        | ProviderChoice::Deepseek
        | ProviderChoice::Firmware
        | ProviderChoice::HuggingFace
        | ProviderChoice::MoonshotAi
        | ProviderChoice::Kimi
        | ProviderChoice::Nebius
        | ProviderChoice::Scaleway
        | ProviderChoice::Stackit
        | ProviderChoice::Groq
        | ProviderChoice::Mistral
        | ProviderChoice::Perplexity
        | ProviderChoice::TogetherAi
        | ProviderChoice::Deepinfra
        | ProviderChoice::Fireworks
        | ProviderChoice::Minimax
        | ProviderChoice::MinimaxTokenPlan
        | ProviderChoice::Xai
        | ProviderChoice::Lmstudio
        | ProviderChoice::Ollama
        | ProviderChoice::Chutes
        | ProviderChoice::Cerebras
        | ProviderChoice::AlibabaCodingPlan
        | ProviderChoice::OpenaiCompatible => {
            disable_subscription_runtime_mode();
            let profile = profile_for_choice(choice)
                .ok_or_else(|| anyhow::anyhow!("missing provider profile for choice"))?;
            if std::env::var_os("JCODE_PROVIDER_PROFILE_ACTIVE").is_none()
                && std::env::var_os("JCODE_NAMED_PROVIDER_PROFILE").is_none()
            {
                apply_openai_compatible_profile_env(Some(profile));
            }
            let display_name = if let Ok(named) = std::env::var("JCODE_NAMED_PROVIDER_PROFILE") {
                named
            } else {
                let resolved = resolve_openai_compatible_profile(profile);
                if resolved.requires_api_key {
                    ensure_external_api_key_auth_allowed_for_explicit_choice(
                        &resolved.api_key_env,
                    )?;
                }
                resolved.display_name
            };
            init_notice(&format!(
                "Using {} via OpenAI-compatible API (provider locked)",
                display_name
            ));
            lock_model_provider("openrouter");
            if std::env::var_os("JCODE_PROVIDER_PROFILE_ACTIVE").is_some()
                || std::env::var_os("JCODE_NAMED_PROVIDER_PROFILE").is_some()
            {
                let profile_name = std::env::var("JCODE_NAMED_PROVIDER_PROFILE")?;
                let cfg = crate::config::config();
                let profile = cfg.providers.get(&profile_name).ok_or_else(|| {
                    anyhow::anyhow!("Unknown provider profile '{}'", profile_name)
                })?;
                Arc::new(
                    provider::openrouter::OpenRouterProvider::new_named_openai_compatible(
                        &profile_name,
                        profile,
                    )?,
                )
            } else {
                Arc::new(provider::openrouter::OpenRouterProvider::new()?)
            }
        }
        ProviderChoice::Antigravity => {
            disable_subscription_runtime_mode();
            ensure_antigravity_auth_allowed_for_explicit_choice()?;
            init_notice("Using Antigravity provider (experimental)");
            unlock_model_provider();
            crate::env::set_var("JCODE_ACTIVE_PROVIDER", "antigravity");
            Arc::new(provider::antigravity::AntigravityProvider::new())
        }
        ProviderChoice::Google => {
            disable_subscription_runtime_mode();
            init_notice(
                "Note: Google/Gmail is not a model provider. Using auto-detect for model provider.",
            );
            init_notice("Gmail tool is available if you've run `jcode login google`.");
            unlock_model_provider();
            Arc::new(provider::MultiProvider::new_fast())
        }
        ProviderChoice::Auto => {
            disable_subscription_runtime_mode();
            unlock_model_provider();
            let auto_detect_start = std::time::Instant::now();
            let mut availability = detect_auto_provider_flags().await;

            let reviewed_external_auth = if !availability.has_any_provider() {
                maybe_run_external_auth_auto_import_flow().await?.is_some()
            } else {
                false
            };

            if reviewed_external_auth {
                availability = detect_auto_provider_flags().await;
            }

            let auto_detect_ms = auto_detect_start.elapsed().as_millis();

            if !availability.has_any_provider() {
                let supplemental_start = std::time::Instant::now();
                let mut has_claude = availability.has_claude;
                let mut has_openai = availability.has_openai;
                let mut has_copilot = availability.has_copilot;
                let has_antigravity = availability.has_antigravity;
                let mut has_gemini = availability.has_gemini;
                let mut has_cursor = availability.has_cursor;
                let mut has_openrouter = availability.has_openrouter;
                let mut has_other_provider = has_claude
                    || has_copilot
                    || has_antigravity
                    || has_gemini
                    || has_cursor
                    || has_openrouter;

                if !has_openai {
                    has_openai = maybe_enable_legacy_codex_auth_for_auto(has_other_provider)?;
                }
                has_other_provider = has_openai
                    || has_claude
                    || has_copilot
                    || has_antigravity
                    || has_gemini
                    || has_cursor
                    || has_openrouter;

                if !has_claude {
                    has_claude =
                        maybe_enable_claude_auth_for_auto(has_other_provider && !has_claude)?;
                }
                has_other_provider = has_openai
                    || has_claude
                    || has_copilot
                    || has_antigravity
                    || has_gemini
                    || has_cursor
                    || has_openrouter;

                if !has_copilot {
                    has_copilot =
                        maybe_enable_copilot_auth_for_auto(has_other_provider && !has_copilot)?;
                }
                has_other_provider = has_openai
                    || has_claude
                    || has_copilot
                    || has_antigravity
                    || has_gemini
                    || has_cursor
                    || has_openrouter;

                if !has_gemini {
                    has_gemini =
                        maybe_enable_gemini_auth_for_auto(has_other_provider && !has_gemini)?;
                }
                has_other_provider = has_openai
                    || has_claude
                    || has_copilot
                    || has_antigravity
                    || has_gemini
                    || has_cursor
                    || has_openrouter;

                if !has_cursor {
                    has_cursor =
                        maybe_enable_cursor_auth_for_auto(has_other_provider && !has_cursor)?;
                }

                has_other_provider = has_openai
                    || has_claude
                    || has_copilot
                    || has_antigravity
                    || has_gemini
                    || has_cursor
                    || has_openrouter;

                if !has_openrouter {
                    has_openrouter = maybe_enable_external_api_key_auth_for_auto(
                        has_other_provider && !has_openrouter,
                    )?;
                }

                availability = AutoProviderAvailability {
                    auth_status: auth::AuthStatus::check_fast(),
                    has_claude,
                    has_openai,
                    has_copilot,
                    has_antigravity,
                    has_gemini,
                    has_cursor,
                    has_openrouter,
                };
                crate::logging::info(&format!(
                    "[TIMING] auto_provider_bootstrap: detect={}ms, external_import={}, supplemental={}ms, final_has_any={}",
                    auto_detect_ms,
                    reviewed_external_auth,
                    supplemental_start.elapsed().as_millis(),
                    availability.has_any_provider()
                ));
            } else {
                crate::logging::info(&format!(
                    "[TIMING] auto_provider_bootstrap: detect={}ms, external_import={}, supplemental=skipped, final_has_any=true",
                    auto_detect_ms, reviewed_external_auth
                ));
            }

            if availability.has_any_provider() {
                let multi = provider::MultiProvider::from_auth_status(availability.auth_status);
                init_notice(&format!(
                    "Using {} (use /model to switch models)",
                    multi.name()
                ));
                crate::env::set_var("JCODE_ACTIVE_PROVIDER", multi.name().to_lowercase());
                Arc::new(multi)
            } else {
                let non_interactive = std::env::var("JCODE_NON_INTERACTIVE").is_ok();
                if non_interactive {
                    anyhow::bail!(
                        "No credentials configured. Run 'jcode login' or set ANTHROPIC_API_KEY to authenticate."
                    );
                }

                if !allow_login_bootstrap {
                    anyhow::bail!(
                        "No credentials configured for provider auto-detection; automatic login/bootstrap is disabled during validation."
                    );
                }

                let provider_desc = prompt_login_provider_selection(
                    &crate::provider_catalog::auto_init_login_providers(),
                    "No credentials found. Let's log in!\n\nChoose a provider:",
                )?;
                Box::pin(login_and_bootstrap_provider(provider_desc, None)).await?
            }
        }
    };

    if std::env::var_os("JCODE_PROVIDER_PROFILE_ACTIVE").is_none()
        && std::env::var_os("JCODE_NAMED_PROVIDER_PROFILE").is_none()
        && model.is_none()
        && let Some(profile) = profile_for_choice(choice)
        && let Some(default_model) = resolved_profile_default_model(profile)
        && provider.set_model(&default_model).is_ok()
    {
        let resolved = resolve_openai_compatible_profile(profile);
        init_notice(&format!(
            "Using default model for {}: {}",
            resolved.display_name, default_model
        ));
    }

    if let Some(model_name) = model {
        if let Err(e) = provider.set_model(model_name) {
            init_notice(&format!(
                "Warning: failed to set model '{}': {}",
                model_name, e
            ));
        } else {
            init_notice(&format!("Using model: {}", model_name));
        }
    }

    Ok(provider)
}

pub async fn init_provider_and_registry(
    choice: &ProviderChoice,
    model: Option<&str>,
) -> Result<(Arc<dyn provider::Provider>, tool::Registry)> {
    let provider = init_provider(choice, model).await?;
    let registry = tool::Registry::new(provider.clone()).await;
    Ok((provider, registry))
}

pub async fn init_provider_and_registry_for_validation(
    choice: &ProviderChoice,
    model: Option<&str>,
) -> Result<(Arc<dyn provider::Provider>, tool::Registry)> {
    let provider = init_provider_for_validation(choice, model).await?;
    let registry = tool::Registry::new(provider.clone()).await;
    Ok((provider, registry))
}

#[cfg(test)]
#[path = "provider_init_tests.rs"]
mod tests;
