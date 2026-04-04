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

#[derive(Debug, Clone, PartialEq, Eq, clap::ValueEnum)]
pub enum ProviderChoice {
    Jcode,
    Claude,
    #[value(alias = "claude-subprocess", hide = true)]
    ClaudeSubprocess,
    Openai,
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
    pub fn as_arg_value(&self) -> &'static str {
        match self {
            Self::Jcode => "jcode",
            Self::Claude => "claude",
            Self::ClaudeSubprocess => "claude-subprocess",
            Self::Openai => "openai",
            Self::Openrouter => "openrouter",
            Self::Azure => "azure",
            Self::Opencode => "opencode",
            Self::OpencodeGo => "opencode-go",
            Self::Zai => "zai",
            Self::Kimi => "kimi",
            Self::Ai302 => "302ai",
            Self::Baseten => "baseten",
            Self::Cortecs => "cortecs",
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

pub fn login_provider_for_choice(choice: &ProviderChoice) -> Option<LoginProviderDescriptor> {
    match choice {
        ProviderChoice::Jcode => Some(crate::provider_catalog::JCODE_LOGIN_PROVIDER),
        ProviderChoice::Claude | ProviderChoice::ClaudeSubprocess => {
            Some(crate::provider_catalog::CLAUDE_LOGIN_PROVIDER)
        }
        ProviderChoice::Openai => Some(crate::provider_catalog::OPENAI_LOGIN_PROVIDER),
        ProviderChoice::Openrouter => Some(crate::provider_catalog::OPENROUTER_LOGIN_PROVIDER),
        ProviderChoice::Azure => Some(crate::provider_catalog::AZURE_LOGIN_PROVIDER),
        ProviderChoice::Opencode => Some(crate::provider_catalog::OPENCODE_LOGIN_PROVIDER),
        ProviderChoice::OpencodeGo => Some(crate::provider_catalog::OPENCODE_GO_LOGIN_PROVIDER),
        ProviderChoice::Zai => Some(crate::provider_catalog::ZAI_LOGIN_PROVIDER),
        ProviderChoice::Kimi => Some(crate::provider_catalog::KIMI_LOGIN_PROVIDER),
        ProviderChoice::Ai302 => Some(crate::provider_catalog::AI302_LOGIN_PROVIDER),
        ProviderChoice::Baseten => Some(crate::provider_catalog::BASETEN_LOGIN_PROVIDER),
        ProviderChoice::Cortecs => Some(crate::provider_catalog::CORTECS_LOGIN_PROVIDER),
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
    eprintln!("{heading}");
    for (index, provider) in providers.iter().enumerate() {
        eprintln!(
            "  {}. {:<16} - {}",
            index + 1,
            provider.display_name,
            provider.menu_detail
        );
    }
    eprintln!();
    let recommended = providers
        .iter()
        .filter(|provider| provider.recommended)
        .map(|provider| provider.display_name)
        .collect::<Vec<_>>();
    if !recommended.is_empty() {
        eprintln!(
            "  Recommended if you have a subscription: {}.",
            recommended.join(", ")
        );
    }
    eprint!("\nEnter 1-{}: ", providers.len());
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    resolve_login_selection(input.trim(), providers)
        .ok_or_else(|| anyhow::anyhow!("Invalid choice. Run 'jcode login' to try again."))
}

fn can_prompt_for_external_auth() -> bool {
    std::io::stdin().is_terminal()
        && std::io::stderr().is_terminal()
        && std::env::var("JCODE_NON_INTERACTIVE").is_err()
}

fn external_auth_blocked_message(
    provider_name: &str,
    source_name: &str,
    path: &std::path::Path,
    login_hint: &str,
) -> String {
    format!(
        "Found existing {} credentials from {} at {} but jcode will not read them without confirmation. Re-run in an interactive terminal to approve this auth source for future jcode sessions, or run `{}`.",
        provider_name,
        source_name,
        path.display(),
        login_hint
    )
}

fn prompt_to_trust_external_auth(
    provider_name: &str,
    source_name: &str,
    path: &std::path::Path,
) -> Result<bool> {
    eprintln!();
    eprintln!(
        "Found existing {} credentials from {} at {}.",
        provider_name,
        source_name,
        path.display()
    );
    eprintln!("jcode will only read that source in place after you approve it.");
    eprintln!("It will not move, delete, or rewrite the original auth there.");
    eprint!("Trust this auth source for future jcode sessions? [y/N]: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ExternalAuthReviewAction {
    SharedExternal(auth::external::ExternalAuthSource),
    CodexLegacy,
    ClaudeCode,
    GeminiCli,
    Copilot(auth::copilot::ExternalCopilotAuthSource),
    Cursor(auth::cursor::ExternalCursorAuthSource),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExternalAuthReviewCandidate {
    pub(crate) provider_summary: String,
    pub(crate) source_name: String,
    pub(crate) path: std::path::PathBuf,
    action: ExternalAuthReviewAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExternalAuthAutoImportOutcome {
    pub imported: usize,
    pub messages: Vec<String>,
}

impl ExternalAuthAutoImportOutcome {
    pub(crate) fn render_markdown(&self) -> String {
        if self.messages.is_empty() {
            return "No external auth sources were imported.".to_string();
        }
        let mut out = format!("**Auto Import**\n\nImported {} source(s).", self.imported);
        for line in &self.messages {
            out.push_str("\n- ");
            out.push_str(line);
        }
        out
    }
}

pub(crate) fn pending_external_auth_review_candidates() -> Result<Vec<ExternalAuthReviewCandidate>>
{
    let mut candidates = Vec::new();

    for source in auth::external::unconsented_sources() {
        let provider_summary = auth::external::source_provider_labels(source).join(", ");
        if provider_summary.is_empty() {
            continue;
        }
        candidates.push(ExternalAuthReviewCandidate {
            provider_summary,
            source_name: source.display_name().to_string(),
            path: source.path()?,
            action: ExternalAuthReviewAction::SharedExternal(source),
        });
    }

    if auth::codex::has_unconsented_legacy_credentials() {
        candidates.push(ExternalAuthReviewCandidate {
            provider_summary: "OpenAI/Codex".to_string(),
            source_name: "Codex auth.json".to_string(),
            path: auth::codex::legacy_auth_file_path()?,
            action: ExternalAuthReviewAction::CodexLegacy,
        });
    }

    if let Some(source) = auth::claude::has_unconsented_external_auth()
        && matches!(source, auth::claude::ExternalClaudeAuthSource::ClaudeCode)
    {
        candidates.push(ExternalAuthReviewCandidate {
            provider_summary: "Claude".to_string(),
            source_name: source.display_name().to_string(),
            path: source.path()?,
            action: ExternalAuthReviewAction::ClaudeCode,
        });
    }

    if auth::gemini::has_unconsented_cli_auth() {
        candidates.push(ExternalAuthReviewCandidate {
            provider_summary: "Gemini".to_string(),
            source_name: "Gemini CLI".to_string(),
            path: auth::gemini::gemini_cli_oauth_path()?,
            action: ExternalAuthReviewAction::GeminiCli,
        });
    }

    if let Some(source) = auth::copilot::has_unconsented_external_auth()
        && !matches!(
            source,
            auth::copilot::ExternalCopilotAuthSource::OpenCodeAuth
                | auth::copilot::ExternalCopilotAuthSource::PiAuth
        )
    {
        candidates.push(ExternalAuthReviewCandidate {
            provider_summary: "GitHub Copilot".to_string(),
            source_name: source.display_name().to_string(),
            path: source.path(),
            action: ExternalAuthReviewAction::Copilot(source),
        });
    }

    if let Some(source) = auth::cursor::has_unconsented_external_auth() {
        candidates.push(ExternalAuthReviewCandidate {
            provider_summary: "Cursor".to_string(),
            source_name: source.display_name().to_string(),
            path: source.path()?,
            action: ExternalAuthReviewAction::Cursor(source),
        });
    }

    Ok(candidates)
}

pub(crate) fn parse_external_auth_review_selection(
    input: &str,
    count: usize,
) -> Result<Vec<usize>> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if matches!(trimmed.to_ascii_lowercase().as_str(), "a" | "all") {
        return Ok((0..count).collect());
    }

    let mut selected = Vec::new();
    for part in trimmed.split(',') {
        let value = part.trim();
        if value.is_empty() {
            continue;
        }
        let index: usize = value.parse().map_err(|_| {
            anyhow::anyhow!(
                "Invalid selection '{}'. Enter numbers like 1,3 or 'a' for all.",
                value
            )
        })?;
        if index == 0 || index > count {
            anyhow::bail!(
                "Selection '{}' is out of range. Enter 1-{} or 'a' for all.",
                index,
                count
            );
        }
        let zero_based = index - 1;
        if !selected.contains(&zero_based) {
            selected.push(zero_based);
        }
    }
    Ok(selected)
}

fn prompt_to_review_external_auth_sources(
    candidates: &[ExternalAuthReviewCandidate],
) -> Result<Vec<usize>> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    eprintln!();
    eprintln!("Found existing logins that jcode can reuse.");
    eprintln!("Nothing has been imported yet.");
    eprintln!(
        "Approve the sources you want jcode to read in place; rejected sources stay untouched."
    );
    eprintln!();

    for (index, candidate) in candidates.iter().enumerate() {
        eprintln!(
            "  {}. {:<22} via {}",
            index + 1,
            candidate.provider_summary,
            candidate.source_name
        );
        eprintln!("     {}", candidate.path.display());
    }

    eprintln!();
    eprint!("Approve sources [a=all, Enter=skip, example: 1,3]: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    parse_external_auth_review_selection(&input, candidates.len())
}

fn approve_external_auth_review_candidate(candidate: &ExternalAuthReviewCandidate) -> Result<()> {
    match candidate.action {
        ExternalAuthReviewAction::SharedExternal(source) => {
            auth::external::trust_external_auth_source(source)?
        }
        ExternalAuthReviewAction::CodexLegacy => auth::codex::trust_legacy_auth_for_future_use()?,
        ExternalAuthReviewAction::ClaudeCode => auth::claude::trust_external_auth_source(
            auth::claude::ExternalClaudeAuthSource::ClaudeCode,
        )?,
        ExternalAuthReviewAction::GeminiCli => auth::gemini::trust_cli_auth_for_future_use()?,
        ExternalAuthReviewAction::Copilot(source) => {
            auth::copilot::trust_external_auth_source(source)?
        }
        ExternalAuthReviewAction::Cursor(source) => {
            auth::cursor::trust_external_auth_source(source)?
        }
    }
    Ok(())
}

fn revoke_external_auth_review_candidate(candidate: &ExternalAuthReviewCandidate) -> Result<()> {
    match candidate.action {
        ExternalAuthReviewAction::SharedExternal(source) => {
            crate::config::Config::revoke_external_auth_source_for_path(
                source.source_id(),
                &candidate.path,
            )?
        }
        ExternalAuthReviewAction::CodexLegacy => {
            crate::config::Config::revoke_external_auth_source_for_path(
                auth::codex::LEGACY_CODEX_AUTH_SOURCE_ID,
                &candidate.path,
            )?
        }
        ExternalAuthReviewAction::ClaudeCode => {
            crate::config::Config::revoke_external_auth_source_for_path(
                auth::claude::CLAUDE_CODE_AUTH_SOURCE_ID,
                &candidate.path,
            )?
        }
        ExternalAuthReviewAction::GeminiCli => {
            crate::config::Config::revoke_external_auth_source_for_path(
                auth::gemini::GEMINI_CLI_AUTH_SOURCE_ID,
                &candidate.path,
            )?
        }
        ExternalAuthReviewAction::Copilot(source) => {
            crate::config::Config::revoke_external_auth_source_for_path(
                source.source_id(),
                &candidate.path,
            )?
        }
        ExternalAuthReviewAction::Cursor(source) => {
            crate::config::Config::revoke_external_auth_source_for_path(
                source.source_id(),
                &candidate.path,
            )?
        }
    }
    Ok(())
}

async fn validate_claude_import() -> Result<String> {
    let creds = auth::claude::load_credentials()?;
    let refreshed = crate::auth::oauth::refresh_claude_tokens(&creds.refresh_token).await?;
    Ok(format!(
        "Claude refresh probe succeeded (expires_at={}).",
        refreshed.expires_at
    ))
}

async fn validate_openai_import() -> Result<String> {
    let creds = auth::codex::load_credentials()?;
    if creds.refresh_token.trim().is_empty() {
        Ok("Loaded OpenAI API key credentials.".to_string())
    } else {
        let refreshed = crate::auth::oauth::refresh_openai_tokens(&creds.refresh_token).await?;
        Ok(format!(
            "OpenAI refresh probe succeeded (expires_at={}).",
            refreshed.expires_at
        ))
    }
}

async fn validate_gemini_import() -> Result<String> {
    let tokens = auth::gemini::load_or_refresh_tokens().await?;
    Ok(format!(
        "Gemini load/refresh probe succeeded (expires_at={}).",
        tokens.expires_at
    ))
}

async fn validate_antigravity_import() -> Result<String> {
    let tokens = auth::antigravity::load_or_refresh_tokens().await?;
    Ok(format!(
        "Antigravity load/refresh probe succeeded (expires_at={}).",
        tokens.expires_at
    ))
}

async fn validate_copilot_import() -> Result<String> {
    let github_token = auth::copilot::load_github_token()?;
    let api_token =
        auth::copilot::exchange_github_token(&reqwest::Client::new(), &github_token).await?;
    Ok(format!(
        "Copilot exchange probe succeeded (expires_at={}).",
        api_token.expires_at
    ))
}

async fn validate_cursor_import() -> Result<String> {
    let has_agent_auth = auth::cursor::has_cursor_agent_auth();
    let has_api_key = auth::cursor::has_cursor_api_key();
    let has_vscdb = auth::cursor::has_cursor_vscdb_token();
    if has_agent_auth || has_api_key || has_vscdb {
        Ok(format!(
            "Cursor source loaded (agent_session={}, api_key={}, vscdb_token={}).",
            has_agent_auth, has_api_key, has_vscdb
        ))
    } else {
        anyhow::bail!("Cursor source did not expose a usable auth token.")
    }
}

fn validate_openrouter_like_import() -> Result<String> {
    for (env_key, env_file) in crate::provider_catalog::openrouter_like_api_key_sources() {
        if crate::provider_catalog::load_api_key_from_env_or_config(&env_key, &env_file).is_some() {
            return Ok(format!("Loaded API key for `{}`.", env_key));
        }
    }
    anyhow::bail!("No reusable API key became available after import.")
}

async fn validate_shared_external_import(
    source: auth::external::ExternalAuthSource,
) -> Result<String> {
    let mut errors = Vec::new();
    for label in auth::external::source_provider_labels(source) {
        let result = match label {
            "OpenAI/Codex" => validate_openai_import().await,
            "Claude" => validate_claude_import().await,
            "Gemini" => validate_gemini_import().await,
            "Antigravity" => validate_antigravity_import().await,
            "GitHub Copilot" => validate_copilot_import().await,
            "OpenRouter/API-key providers" => validate_openrouter_like_import(),
            _ => continue,
        };
        match result {
            Ok(detail) => return Ok(detail),
            Err(err) => errors.push(format!("{}: {}", label, err)),
        }
    }
    anyhow::bail!(errors.join("; "))
}

async fn validate_external_auth_review_candidate(
    candidate: &ExternalAuthReviewCandidate,
) -> Result<String> {
    match candidate.action {
        ExternalAuthReviewAction::SharedExternal(source) => {
            validate_shared_external_import(source).await
        }
        ExternalAuthReviewAction::CodexLegacy => validate_openai_import().await,
        ExternalAuthReviewAction::ClaudeCode => validate_claude_import().await,
        ExternalAuthReviewAction::GeminiCli => validate_gemini_import().await,
        ExternalAuthReviewAction::Copilot(_) => validate_copilot_import().await,
        ExternalAuthReviewAction::Cursor(_) => validate_cursor_import().await,
    }
}

pub(crate) async fn maybe_run_external_auth_auto_import_flow() -> Result<Option<usize>> {
    if !can_prompt_for_external_auth() {
        return Ok(None);
    }

    let candidates = pending_external_auth_review_candidates()?;
    if candidates.is_empty() {
        return Ok(None);
    }

    let selected = prompt_to_review_external_auth_sources(&candidates)?;
    let outcome = run_external_auth_auto_import_candidates(&candidates, &selected).await?;
    for line in &outcome.messages {
        eprintln!("{}", line);
    }
    auth::AuthStatus::invalidate_cache();
    Ok(Some(outcome.imported))
}

pub(crate) fn format_external_auth_review_candidates_markdown(
    candidates: &[ExternalAuthReviewCandidate],
) -> String {
    let mut message = String::from(
        "**Auto Import Existing Logins**\n\nFound existing logins that jcode can reuse. Nothing has been imported yet.\n\nReply with `a` to approve all, `1,3` to approve specific sources, or `/cancel` to abort.\n",
    );
    for (index, candidate) in candidates.iter().enumerate() {
        message.push_str(&format!(
            "\n{}. **{}** via {}\n   - `{}`\n",
            index + 1,
            candidate.provider_summary,
            candidate.source_name,
            candidate.path.display()
        ));
    }
    message
}

pub(crate) async fn run_external_auth_auto_import_candidates(
    candidates: &[ExternalAuthReviewCandidate],
    selected: &[usize],
) -> Result<ExternalAuthAutoImportOutcome> {
    let mut outcome = ExternalAuthAutoImportOutcome {
        imported: 0,
        messages: Vec::new(),
    };

    for &index in selected {
        let Some(candidate) = candidates.get(index) else {
            continue;
        };
        approve_external_auth_review_candidate(candidate)?;
        match validate_external_auth_review_candidate(candidate).await {
            Ok(detail) => {
                outcome.imported += 1;
                outcome.messages.push(format!(
                    "✓ Imported {} from {}. {}",
                    candidate.provider_summary, candidate.source_name, detail
                ));
            }
            Err(err) => {
                let _ = revoke_external_auth_review_candidate(candidate);
                outcome.messages.push(format!(
                    "✕ Skipped {} from {}: {}",
                    candidate.provider_summary, candidate.source_name, err
                ));
            }
        }
    }

    auth::AuthStatus::invalidate_cache();
    Ok(outcome)
}

async fn detect_auto_provider_flags() -> (bool, bool, bool, bool, bool, bool) {
    let (has_claude, has_openai) = tokio::join!(
        tokio::task::spawn_blocking(|| auth::claude::load_credentials().is_ok()),
        tokio::task::spawn_blocking(|| auth::codex::load_credentials().is_ok()),
    );
    let auth_status = auth::AuthStatus::check_fast();
    (
        has_claude.unwrap_or(false),
        has_openai.unwrap_or(false),
        auth_status.copilot_has_api_token,
        auth_status.gemini == auth::AuthState::Available,
        auth_status.cursor == auth::AuthState::Available,
        provider::openrouter::OpenRouterProvider::has_credentials(),
    )
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
            anyhow::bail!(external_auth_blocked_message(
                &provider_name,
                source.display_name(),
                &path,
                &login_hint,
            ));
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
        anyhow::bail!(external_auth_blocked_message(
            "OpenAI/Codex",
            "Codex",
            &path,
            "jcode login --provider openai"
        ));
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
        anyhow::bail!(external_auth_blocked_message(
            "Claude",
            source.display_name(),
            &path,
            "jcode login --provider claude"
        ));
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
        anyhow::bail!(external_auth_blocked_message(
            "Gemini",
            "Gemini CLI",
            &path,
            "jcode login --provider gemini"
        ));
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
        anyhow::bail!(external_auth_blocked_message(
            "GitHub Copilot",
            source.display_name(),
            &path,
            "jcode login --provider copilot"
        ));
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
        anyhow::bail!(external_auth_blocked_message(
            "Cursor",
            source.display_name(),
            &path,
            "jcode login --provider cursor"
        ));
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

pub async fn login_and_bootstrap_provider(
    provider: LoginProviderDescriptor,
    account_label: Option<&str>,
) -> Result<Arc<dyn provider::Provider>> {
    run_login_provider(provider, account_label).await?;
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
            if let Some(model) = profile.default_model {
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
            Arc::new(provider::antigravity::AntigravityCliProvider::new())
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

async fn init_provider_with_options(
    choice: &ProviderChoice,
    model: Option<&str>,
    show_init_messages: bool,
    allow_login_bootstrap: bool,
) -> Result<Arc<dyn provider::Provider>> {
    if let Some(profile) = profile_for_choice(choice) {
        apply_openai_compatible_profile_env(Some(profile));
    } else {
        apply_openai_compatible_profile_env(None);
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
        ProviderChoice::Cursor => {
            disable_subscription_runtime_mode();
            ensure_cursor_auth_allowed_for_explicit_choice()?;
            init_notice("Using Cursor CLI provider (experimental)");
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
            let resolved = resolve_openai_compatible_profile(profile);
            if resolved.requires_api_key {
                ensure_external_api_key_auth_allowed_for_explicit_choice(&resolved.api_key_env)?;
            }
            init_notice(&format!(
                "Using {} via OpenAI-compatible API (provider locked)",
                resolved.display_name
            ));
            lock_model_provider("openrouter");
            Arc::new(provider::MultiProvider::new_fast())
        }
        ProviderChoice::Antigravity => {
            disable_subscription_runtime_mode();
            ensure_antigravity_auth_allowed_for_explicit_choice()?;
            init_notice("Using Antigravity CLI provider (experimental)");
            unlock_model_provider();
            crate::env::set_var("JCODE_ACTIVE_PROVIDER", "antigravity");
            Arc::new(provider::antigravity::AntigravityCliProvider::new())
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
            let (
                mut has_claude,
                mut has_openai,
                mut has_copilot,
                mut has_gemini,
                mut has_cursor,
                mut has_openrouter,
            ) = detect_auto_provider_flags().await;

            let reviewed_external_auth = if !(has_claude
                || has_openai
                || has_copilot
                || has_gemini
                || has_cursor
                || has_openrouter)
            {
                maybe_run_external_auth_auto_import_flow().await?.is_some()
            } else {
                false
            };

            if reviewed_external_auth {
                (
                    has_claude,
                    has_openai,
                    has_copilot,
                    has_gemini,
                    has_cursor,
                    has_openrouter,
                ) = detect_auto_provider_flags().await;
            } else {
                let mut has_other_provider =
                    has_claude || has_copilot || has_gemini || has_cursor || has_openrouter;

                if !has_openai {
                    has_openai = maybe_enable_legacy_codex_auth_for_auto(has_other_provider)?;
                }
                has_other_provider = has_openai
                    || has_claude
                    || has_copilot
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
                    || has_gemini
                    || has_cursor
                    || has_openrouter;

                if !has_openrouter {
                    has_openrouter = maybe_enable_external_api_key_auth_for_auto(
                        has_other_provider && !has_openrouter,
                    )?;
                }
            }

            if has_claude || has_openai || has_copilot || has_gemini || has_cursor || has_openrouter
            {
                let multi = provider::MultiProvider::new_fast();
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

    if model.is_none()
        && let Some(profile) = profile_for_choice(choice)
    {
        let resolved = resolve_openai_compatible_profile(profile);
        if let Some(default_model) = resolved.default_model
            && provider.set_model(&default_model).is_ok()
        {
            init_notice(&format!(
                "Using default model for {}: {}",
                resolved.display_name, default_model
            ));
        }
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
mod tests {
    use super::*;
    use crate::provider_catalog::{
        self, resolve_login_selection, resolve_openai_compatible_profile,
    };
    use std::sync::{Mutex, OnceLock};
    use tempfile::TempDir;

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        let mutex = ENV_LOCK.get_or_init(|| Mutex::new(()));
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[test]
    fn test_provider_choice_arg_values() {
        assert_eq!(ProviderChoice::Jcode.as_arg_value(), "jcode");
        assert_eq!(ProviderChoice::Claude.as_arg_value(), "claude");
        assert_eq!(
            ProviderChoice::ClaudeSubprocess.as_arg_value(),
            "claude-subprocess"
        );
        assert_eq!(ProviderChoice::Openai.as_arg_value(), "openai");
        assert_eq!(ProviderChoice::Openrouter.as_arg_value(), "openrouter");
        assert_eq!(ProviderChoice::Azure.as_arg_value(), "azure");
        assert_eq!(ProviderChoice::Opencode.as_arg_value(), "opencode");
        assert_eq!(ProviderChoice::OpencodeGo.as_arg_value(), "opencode-go");
        assert_eq!(ProviderChoice::Zai.as_arg_value(), "zai");
        assert_eq!(ProviderChoice::Groq.as_arg_value(), "groq");
        assert_eq!(ProviderChoice::Mistral.as_arg_value(), "mistral");
        assert_eq!(ProviderChoice::Perplexity.as_arg_value(), "perplexity");
        assert_eq!(ProviderChoice::TogetherAi.as_arg_value(), "togetherai");
        assert_eq!(ProviderChoice::Deepinfra.as_arg_value(), "deepinfra");
        assert_eq!(ProviderChoice::Fireworks.as_arg_value(), "fireworks");
        assert_eq!(ProviderChoice::Minimax.as_arg_value(), "minimax");
        assert_eq!(ProviderChoice::Xai.as_arg_value(), "xai");
        assert_eq!(ProviderChoice::Lmstudio.as_arg_value(), "lmstudio");
        assert_eq!(ProviderChoice::Ollama.as_arg_value(), "ollama");
        assert_eq!(ProviderChoice::Chutes.as_arg_value(), "chutes");
        assert_eq!(ProviderChoice::Cerebras.as_arg_value(), "cerebras");
        assert_eq!(
            ProviderChoice::AlibabaCodingPlan.as_arg_value(),
            "alibaba-coding-plan"
        );
        assert_eq!(
            ProviderChoice::OpenaiCompatible.as_arg_value(),
            "openai-compatible"
        );
        assert_eq!(ProviderChoice::Cursor.as_arg_value(), "cursor");
        assert_eq!(ProviderChoice::Copilot.as_arg_value(), "copilot");
        assert_eq!(ProviderChoice::Gemini.as_arg_value(), "gemini");
        assert_eq!(ProviderChoice::Antigravity.as_arg_value(), "antigravity");
        assert_eq!(ProviderChoice::Google.as_arg_value(), "google");
        assert_eq!(ProviderChoice::Auto.as_arg_value(), "auto");
    }

    #[test]
    fn test_server_bootstrap_login_selection_preserves_order() {
        let providers = provider_catalog::server_bootstrap_login_providers();
        assert_eq!(
            resolve_login_selection("1", &providers).map(|provider| provider.id),
            Some("claude")
        );
        assert_eq!(
            resolve_login_selection("3", &providers).map(|provider| provider.id),
            Some("jcode")
        );
        assert_eq!(
            resolve_login_selection("4", &providers).map(|provider| provider.id),
            Some("copilot")
        );
        assert_eq!(
            resolve_login_selection("10", &providers).map(|provider| provider.id),
            Some("chutes")
        );
        assert_eq!(
            resolve_login_selection("11", &providers).map(|provider| provider.id),
            Some("cerebras")
        );
        assert_eq!(
            resolve_login_selection("12", &providers).map(|provider| provider.id),
            Some("alibaba-coding-plan")
        );
    }

    #[test]
    fn test_auto_init_login_selection_preserves_order() {
        let providers = provider_catalog::auto_init_login_providers();
        assert_eq!(
            resolve_login_selection("1", &providers).map(|provider| provider.id),
            Some("claude")
        );
        assert_eq!(
            resolve_login_selection("10", &providers).map(|provider| provider.id),
            Some("alibaba-coding-plan")
        );
        assert_eq!(
            resolve_login_selection("11", &providers).map(|provider| provider.id),
            Some("cursor")
        );
        assert_eq!(
            resolve_login_selection("12", &providers).map(|provider| provider.id),
            Some("copilot")
        );
        assert_eq!(
            resolve_login_selection("13", &providers).map(|provider| provider.id),
            Some("gemini")
        );
        assert_eq!(
            resolve_login_selection("14", &providers).map(|provider| provider.id),
            Some("antigravity")
        );
    }

    #[test]
    fn test_init_provider_jcode_delegates_runtime_profile_to_wrapper() {
        let _guard = lock_env();
        let _env_guard = crate::storage::lock_test_env();
        crate::subscription_catalog::clear_runtime_env();
        crate::env::remove_var("JCODE_OPENROUTER_MODEL");
        crate::env::remove_var("JCODE_ACTIVE_PROVIDER");
        crate::env::remove_var("JCODE_FORCE_PROVIDER");

        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let provider = runtime
            .block_on(init_provider(&ProviderChoice::Jcode, None))
            .expect("init jcode provider");

        assert_eq!(provider.name(), "Jcode Subscription");
        assert!(crate::subscription_catalog::is_runtime_mode_enabled());
        assert_eq!(
            std::env::var("JCODE_OPENROUTER_MODEL").ok().as_deref(),
            Some(crate::subscription_catalog::default_model().id)
        );
        assert_eq!(
            std::env::var("JCODE_ACTIVE_PROVIDER").ok().as_deref(),
            Some("openrouter")
        );
        assert_eq!(
            std::env::var("JCODE_FORCE_PROVIDER").ok().as_deref(),
            Some("1")
        );

        crate::subscription_catalog::clear_runtime_env();
        crate::env::remove_var("JCODE_OPENROUTER_MODEL");
        crate::env::remove_var("JCODE_ACTIVE_PROVIDER");
        crate::env::remove_var("JCODE_FORCE_PROVIDER");
    }

    #[test]
    fn test_openai_compatible_profile_overrides() {
        let _guard = lock_env();
        let keys = [
            "JCODE_OPENAI_COMPAT_API_BASE",
            "JCODE_OPENAI_COMPAT_API_KEY_NAME",
            "JCODE_OPENAI_COMPAT_ENV_FILE",
            "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
        ];
        let saved: Vec<(String, Option<String>)> = keys
            .iter()
            .map(|k| (k.to_string(), std::env::var(k).ok()))
            .collect();

        crate::env::set_var(
            "JCODE_OPENAI_COMPAT_API_BASE",
            "https://api.groq.com/openai/v1/",
        );
        crate::env::set_var("JCODE_OPENAI_COMPAT_API_KEY_NAME", "GROQ_API_KEY");
        crate::env::set_var("JCODE_OPENAI_COMPAT_ENV_FILE", "groq.env");
        crate::env::set_var("JCODE_OPENAI_COMPAT_DEFAULT_MODEL", "openai/gpt-oss-120b");

        let resolved = resolve_openai_compatible_profile(provider_catalog::OPENAI_COMPAT_PROFILE);
        assert_eq!(resolved.api_base, "https://api.groq.com/openai/v1");
        assert_eq!(resolved.api_key_env, "GROQ_API_KEY");
        assert_eq!(resolved.env_file, "groq.env");
        assert_eq!(
            resolved.default_model.as_deref(),
            Some("openai/gpt-oss-120b")
        );

        for (key, value) in saved {
            if let Some(value) = value {
                crate::env::set_var(&key, value);
            } else {
                crate::env::remove_var(&key);
            }
        }
    }

    #[test]
    fn test_openai_compatible_profile_rejects_invalid_overrides() {
        let _guard = lock_env();
        let keys = [
            "JCODE_OPENAI_COMPAT_API_BASE",
            "JCODE_OPENAI_COMPAT_API_KEY_NAME",
            "JCODE_OPENAI_COMPAT_ENV_FILE",
        ];
        let saved: Vec<(String, Option<String>)> = keys
            .iter()
            .map(|k| (k.to_string(), std::env::var(k).ok()))
            .collect();

        crate::env::set_var("JCODE_OPENAI_COMPAT_API_BASE", "http://example.com/v1");
        crate::env::set_var("JCODE_OPENAI_COMPAT_API_KEY_NAME", "bad-key-name");
        crate::env::set_var("JCODE_OPENAI_COMPAT_ENV_FILE", "../bad.env");

        let resolved = resolve_openai_compatible_profile(provider_catalog::OPENAI_COMPAT_PROFILE);
        assert_eq!(
            resolved.api_base,
            provider_catalog::OPENAI_COMPAT_PROFILE.api_base
        );
        assert_eq!(
            resolved.api_key_env,
            provider_catalog::OPENAI_COMPAT_PROFILE.api_key_env
        );
        assert_eq!(
            resolved.env_file,
            provider_catalog::OPENAI_COMPAT_PROFILE.env_file
        );

        for (key, value) in saved {
            if let Some(value) = value {
                crate::env::set_var(&key, value);
            } else {
                crate::env::remove_var(&key);
            }
        }
    }

    #[test]
    fn parse_external_auth_review_selection_supports_all_and_deduped_indices() {
        assert_eq!(
            parse_external_auth_review_selection("", 3).unwrap(),
            Vec::<usize>::new()
        );
        assert_eq!(
            parse_external_auth_review_selection("a", 3).unwrap(),
            vec![0, 1, 2]
        );
        assert_eq!(
            parse_external_auth_review_selection("2,1,2", 3).unwrap(),
            vec![1, 0]
        );
        assert!(parse_external_auth_review_selection("4", 3).is_err());
        assert!(parse_external_auth_review_selection("nope", 3).is_err());
    }

    #[test]
    fn choice_for_login_provider_round_trips_core_targets() {
        assert_eq!(
            choice_for_login_provider(provider_catalog::JCODE_LOGIN_PROVIDER),
            Some(ProviderChoice::Jcode)
        );
        assert_eq!(
            choice_for_login_provider(provider_catalog::OPENROUTER_LOGIN_PROVIDER),
            Some(ProviderChoice::Openrouter)
        );
        assert_eq!(
            choice_for_login_provider(provider_catalog::AZURE_LOGIN_PROVIDER),
            Some(ProviderChoice::Azure)
        );
        assert_eq!(
            choice_for_login_provider(provider_catalog::CURSOR_LOGIN_PROVIDER),
            Some(ProviderChoice::Cursor)
        );
        assert_eq!(
            choice_for_login_provider(provider_catalog::AUTO_IMPORT_LOGIN_PROVIDER),
            None
        );
    }

    #[test]
    fn choice_for_login_provider_round_trips_openai_compatible_profiles() {
        assert_eq!(
            choice_for_login_provider(provider_catalog::OPENCODE_LOGIN_PROVIDER),
            Some(ProviderChoice::Opencode)
        );
        assert_eq!(
            choice_for_login_provider(provider_catalog::LMSTUDIO_LOGIN_PROVIDER),
            Some(ProviderChoice::Lmstudio)
        );
        assert_eq!(
            choice_for_login_provider(provider_catalog::OPENAI_COMPAT_LOGIN_PROVIDER),
            Some(ProviderChoice::OpenaiCompatible)
        );
    }

    #[test]
    fn pending_external_auth_review_candidates_include_shared_and_legacy_sources() {
        let _guard = lock_env();
        let _env_guard = crate::storage::lock_test_env();
        let dir = TempDir::new().expect("temp dir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", dir.path());

        let opencode_path = crate::auth::external::ExternalAuthSource::OpenCode
            .path()
            .expect("opencode path");
        std::fs::create_dir_all(opencode_path.parent().expect("opencode parent"))
            .expect("create opencode dir");
        std::fs::write(
            &opencode_path,
            serde_json::json!({
                "openai": {
                    "type": "oauth",
                    "access": "sk-openai",
                    "refresh": "refresh",
                    "expires": chrono::Utc::now().timestamp_millis() + 60_000
                }
            })
            .to_string(),
        )
        .expect("write opencode auth");

        let codex_path = crate::auth::codex::legacy_auth_file_path().expect("codex path");
        std::fs::create_dir_all(codex_path.parent().expect("codex parent"))
            .expect("create codex dir");
        std::fs::write(
            &codex_path,
            serde_json::json!({
                "tokens": {
                    "access_token": "sk-codex",
                    "refresh_token": "refresh",
                    "expires_at": chrono::Utc::now().timestamp_millis() + 60_000
                }
            })
            .to_string(),
        )
        .expect("write codex auth");

        let candidates = pending_external_auth_review_candidates().expect("candidates");
        assert!(candidates.iter().any(|candidate| {
            candidate.source_name == "OpenCode auth.json"
                && candidate.provider_summary.contains("OpenAI/Codex")
        }));
        assert!(candidates.iter().any(|candidate| {
            candidate.source_name == "Codex auth.json"
                && candidate.provider_summary == "OpenAI/Codex"
        }));

        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}
