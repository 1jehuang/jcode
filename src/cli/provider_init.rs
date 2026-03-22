use anyhow::Result;
use std::io::{self, Write};
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

    let config_dir = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("No config directory found"))?
        .join("jcode");
    std::fs::create_dir_all(&config_dir)?;
    crate::platform::set_directory_permissions_owner_only(&config_dir)?;

    let file_path = config_dir.join(env_file);
    let content = format!("{}={}\n", key_name, key);
    std::fs::write(&file_path, &content)?;
    crate::platform::set_permissions_owner_only(&file_path)?;

    crate::env::set_var(key_name, key);
    Ok(())
}

pub async fn init_provider(
    choice: &ProviderChoice,
    model: Option<&str>,
) -> Result<Arc<dyn provider::Provider>> {
    init_provider_with_options(choice, model, true).await
}

pub async fn init_provider_quiet(
    choice: &ProviderChoice,
    model: Option<&str>,
) -> Result<Arc<dyn provider::Provider>> {
    init_provider_with_options(choice, model, false).await
}

async fn init_provider_with_options(
    choice: &ProviderChoice,
    model: Option<&str>,
    show_init_messages: bool,
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
            init_notice("Using Claude (provider locked)");
            lock_model_provider("claude");
            Arc::new(provider::MultiProvider::with_preference_fast(false))
        }
        ProviderChoice::ClaudeSubprocess => {
            disable_subscription_runtime_mode();
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
            init_notice("Using OpenAI (provider locked)");
            lock_model_provider("openai");
            Arc::new(provider::MultiProvider::with_preference_fast(true))
        }
        ProviderChoice::Cursor => {
            disable_subscription_runtime_mode();
            init_notice("Using Cursor CLI provider (experimental)");
            unlock_model_provider();
            crate::env::set_var("JCODE_ACTIVE_PROVIDER", "cursor");
            Arc::new(provider::cursor::CursorCliProvider::new())
        }
        ProviderChoice::Copilot => {
            disable_subscription_runtime_mode();
            init_notice("Using GitHub Copilot API provider (provider locked)");
            lock_model_provider("copilot");
            Arc::new(provider::MultiProvider::new_fast())
        }
        ProviderChoice::Gemini => {
            disable_subscription_runtime_mode();
            init_notice("Using Gemini provider (native Google Code Assist OAuth)");
            unlock_model_provider();
            crate::env::set_var("JCODE_ACTIVE_PROVIDER", "gemini");
            Arc::new(provider::gemini::GeminiProvider::new())
        }
        ProviderChoice::Openrouter => {
            disable_subscription_runtime_mode();
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
        | ProviderChoice::Chutes
        | ProviderChoice::Cerebras
        | ProviderChoice::AlibabaCodingPlan
        | ProviderChoice::OpenaiCompatible => {
            disable_subscription_runtime_mode();
            let profile = profile_for_choice(choice)
                .ok_or_else(|| anyhow::anyhow!("missing provider profile for choice"))?;
            let resolved = resolve_openai_compatible_profile(profile);
            init_notice(&format!(
                "Using {} via OpenAI-compatible API (provider locked)",
                resolved.display_name
            ));
            lock_model_provider("openrouter");
            Arc::new(provider::MultiProvider::new_fast())
        }
        ProviderChoice::Antigravity => {
            disable_subscription_runtime_mode();
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
            let (has_claude, has_openai) = tokio::join!(
                tokio::task::spawn_blocking(|| auth::claude::load_credentials().is_ok()),
                tokio::task::spawn_blocking(|| auth::codex::load_credentials().is_ok()),
            );
            let has_claude = has_claude.unwrap_or(false);
            let has_openai = has_openai.unwrap_or(false);
            let auth_status = auth::AuthStatus::check_fast();
            let has_copilot = auth_status.copilot_has_api_token;
            let has_gemini = auth_status.gemini == auth::AuthState::Available;
            let has_openrouter = provider::openrouter::OpenRouterProvider::has_credentials();

            if has_claude || has_openai || has_copilot || has_gemini || has_openrouter {
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

                let provider_desc = prompt_login_provider_selection(
                    &crate::provider_catalog::auto_init_login_providers(),
                    "No credentials found. Let's log in!\n\nChoose a provider:",
                )?;
                login_and_bootstrap_provider(provider_desc, None).await?
            }
        }
    };

    if model.is_none() {
        if let Some(profile) = profile_for_choice(choice) {
            let resolved = resolve_openai_compatible_profile(profile);
            if let Some(default_model) = resolved.default_model {
                if provider.set_model(&default_model).is_ok() {
                    init_notice(&format!(
                        "Using default model for {}: {}",
                        resolved.display_name, default_model
                    ));
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_catalog::{
        self, resolve_login_selection, resolve_openai_compatible_profile,
    };
    use std::sync::{Mutex, OnceLock};

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
}
