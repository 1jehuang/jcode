use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use crate::auth;
use crate::provider_catalog::{
    LoginProviderDescriptor, LoginProviderTarget, OPENAI_COMPAT_LOCAL_ENABLED_ENV,
    OpenAiCompatibleProfile, resolve_openai_compatible_profile,
};

use super::provider_init::{ProviderChoice, login_provider_for_choice};
use crate::provider_catalog::save_named_api_key;

mod existing_key_notice;
mod jcode_device;
mod next_step;
mod scriptable;
use scriptable::*;

#[derive(Debug, Clone, Default)]
pub struct LoginOptions {
    pub no_browser: bool,
    pub print_auth_url: bool,
    pub callback_url: Option<String>,
    pub auth_code: Option<String>,
    pub json: bool,
    pub complete: bool,
    pub flow_id: Option<String>,
    pub cancel: bool,
    pub no_validate: bool,
    pub google_access_tier: Option<auth::google::GmailAccessTier>,
    pub openai_compatible_api_base: Option<String>,
    pub openai_compatible_api_key: Option<String>,
    pub openai_compatible_api_key_env: Option<String>,
    pub openai_compatible_default_model: Option<String>,
}

impl LoginOptions {
    fn validate(&self) -> Result<()> {
        if let Some(flow_id) = &self.flow_id {
            parse_login_flow_id(flow_id).map_err(anyhow::Error::msg)?;
        }
        if self.cancel {
            anyhow::ensure!(self.flow_id.is_some(), "--cancel requires --flow-id.");
            anyhow::ensure!(
                !self.print_auth_url && !self.complete && !self.has_provided_input(),
                "--cancel cannot be combined with login begin or completion flags."
            );
        }
        Ok(())
    }

    fn has_provided_input(&self) -> bool {
        self.callback_url.is_some() || self.auth_code.is_some()
    }

    fn resolve_provided_input(&self) -> Result<Option<ProvidedAuthInput>> {
        match (&self.callback_url, &self.auth_code) {
            (Some(_), Some(_)) => {
                anyhow::bail!("Specify only one of --callback-url or --auth-code.")
            }
            (Some(value), None) => Ok(Some(ProvidedAuthInput::CallbackUrl(resolve_auth_input(
                value,
            )?))),
            (None, Some(value)) => Ok(Some(ProvidedAuthInput::AuthCode(resolve_auth_input(
                value,
            )?))),
            (None, None) => Ok(None),
        }
    }

    fn uses_scriptable_flow(&self) -> Result<bool> {
        Ok(self.print_auth_url
            || self.complete
            || self.has_provided_input()
            || self.flow_id.is_some()
            || self.cancel)
    }
}

pub(crate) fn parse_login_flow_id(value: &str) -> std::result::Result<String, String> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(
            "--flow-id must contain 1-64 ASCII letters, digits, underscores or hyphens.".into(),
        );
    }
    Ok(value.to_string())
}

#[derive(Debug, Clone)]
enum ProvidedAuthInput {
    CallbackUrl(String),
    AuthCode(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginFlowOutcome {
    Completed,
    Deferred,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
enum PendingScriptableLogin {
    Claude {
        account_label: String,
        verifier: String,
        redirect_uri: String,
    },
    Openai {
        account_label: String,
        verifier: String,
        state: String,
        redirect_uri: String,
    },
    Gemini {
        verifier: String,
        redirect_uri: String,
    },
    Antigravity {
        verifier: String,
        state: String,
        redirect_uri: String,
    },
    Google {
        verifier: String,
        state: String,
        redirect_uri: String,
        tier: auth::google::GmailAccessTier,
    },
    Copilot {
        device_code: String,
        user_code: String,
        verification_uri: String,
        interval: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingScriptableLoginRecord {
    expires_at_ms: i64,
    login: PendingScriptableLogin,
}

impl PendingScriptableLogin {
    fn key(&self) -> &'static str {
        match self {
            Self::Claude { .. } => "claude",
            Self::Openai { .. } => "openai",
            Self::Gemini { .. } => "gemini",
            Self::Antigravity { .. } => "antigravity",
            Self::Google { .. } => "google",
            Self::Copilot { .. } => "copilot",
        }
    }

    fn pending_path(&self, flow_id: Option<&str>) -> Result<PathBuf> {
        pending_login_path(self.key(), flow_id)
    }

    fn default_expires_at_ms(&self) -> i64 {
        current_time_ms() + 30 * 60 * 1000
    }
}

#[derive(Debug, Clone, Serialize)]
struct ScriptableAuthPrompt {
    status: &'static str,
    provider: String,
    auth_url: String,
    input_kind: String,
    pending_path: String,
    user_code: Option<String>,
    expires_at_ms: i64,
    resume_command: String,
}

#[derive(Debug, Clone, Serialize)]
struct ScriptableAuthSuccess {
    status: &'static str,
    provider: String,
    account_label: Option<String>,
    credentials_path: Option<String>,
    email: Option<String>,
}

#[allow(deprecated)]
pub async fn run_login(
    choice: &ProviderChoice,
    account_label: Option<&str>,
    options: LoginOptions,
) -> Result<()> {
    options.validate()?;
    if let Some(provider) = login_provider_for_choice(choice) {
        return run_login_provider(provider, account_label, options).await;
    }

    match choice {
        ProviderChoice::Auto => {
            if options.uses_scriptable_flow()? {
                anyhow::bail!(
                    "Scriptable login flags require an explicit provider. Use `jcode login --provider <provider> ...`."
                );
            }
            crate::telemetry::record_setup_step_once("login_picker_opened");
            let providers = crate::provider_catalog::cli_login_providers();
            if !io::stdin().is_terminal() {
                anyhow::bail!(
                    "`jcode login --provider auto` requires an interactive terminal. Use `jcode login --provider <provider>` in non-interactive mode."
                );
            }
            if let Some(imported) =
                super::provider_init::maybe_run_external_auth_auto_import_flow().await?
                && imported > 0
            {
                crate::console::eprintln_best_effort(&format!(
                    "\nImported {} existing auth source(s).",
                    imported
                ));
                notify_running_server_auth_changed_best_effort(None).await;
                return Ok(());
            }
            match super::provider_init::prompt_login_provider_selection_optional(
                &providers,
                "Choose a provider to log in:",
            )? {
                Some(provider) => run_login_provider(provider, account_label, options).await?,
                None => crate::cli::output::stderr_info("Login skipped."),
            }
        }
        _ => unreachable!("handled above"),
    }
    Ok(())
}

pub async fn run_login_provider(
    provider: LoginProviderDescriptor,
    account_label: Option<&str>,
    options: LoginOptions,
) -> Result<()> {
    options.validate()?;
    if options.cancel {
        return cancel_scriptable_login(provider, &options);
    }
    crate::telemetry::record_provider_selected(provider.id);
    crate::telemetry::record_auth_started(provider.id, provider.auth_kind.label());
    let explicit_scriptable_flow = options.uses_scriptable_flow()?;
    let auto_scriptable_reason = if explicit_scriptable_flow {
        None
    } else {
        auto_scriptable_flow_reason(provider, &options, io::stdin().is_terminal())
    };
    crate::logging::auth_event(
        "login_flow_resolved",
        provider.id,
        &[
            ("method", provider.auth_kind.label()),
            (
                "scriptable",
                if explicit_scriptable_flow || auto_scriptable_reason.is_some() {
                    "true"
                } else {
                    "false"
                },
            ),
            (
                "auto_scriptable_reason",
                auto_scriptable_reason.unwrap_or("none"),
            ),
            (
                "has_account_label",
                if account_label.is_some() {
                    "true"
                } else {
                    "false"
                },
            ),
        ],
    );
    let login_result = if explicit_scriptable_flow {
        run_scriptable_login_provider(provider, account_label, &options).await
    } else if let Some(reason) = auto_scriptable_reason {
        crate::telemetry::record_auth_surface_blocked_reason(
            provider.id,
            provider.auth_kind.label(),
            reason,
        );
        if !options.json {
            crate::console::eprintln_best_effort(&format!(
                "Detected a manual-safe login environment for {}. Starting the auth URL flow instead of browser-first login.",
                provider.display_name
            ));
        }
        if provider.target == LoginProviderTarget::Google
            && !auth::browser_suppressed(options.no_browser)
        {
            run_automatic_google_login(provider.id, &options).await
        } else {
            start_scriptable_login(provider, account_label, &options).await
        }
    } else {
        match provider.target {
            LoginProviderTarget::AutoImport => {
                let imported = super::provider_init::maybe_run_external_auth_auto_import_flow()
                    .await?
                    .unwrap_or(0);
                if imported == 0 {
                    anyhow::bail!(
                        "No existing logins were imported. Either none were found, nothing was approved, or validation failed."
                    );
                }
                crate::console::eprintln_best_effort(&format!(
                    "Imported {} existing auth source(s).",
                    imported
                ));
                Ok(LoginFlowOutcome::Completed)
            }
            LoginProviderTarget::Jcode => login_jcode_flow(options.no_browser)
                .await
                .map(|_| LoginFlowOutcome::Completed),
            LoginProviderTarget::Claude => login_claude_flow(account_label, options.no_browser)
                .await
                .map(|_| LoginFlowOutcome::Completed),
            LoginProviderTarget::ClaudeApiKey => {
                login_anthropic_api_key_flow().map(|_| LoginFlowOutcome::Completed)
            }
            LoginProviderTarget::OpenAi => login_openai_flow(account_label, options.no_browser)
                .await
                .map(|_| LoginFlowOutcome::Completed),
            LoginProviderTarget::OpenAiApiKey => {
                login_openai_api_key_flow().map(|_| LoginFlowOutcome::Completed)
            }
            LoginProviderTarget::GrokBuild => login_grok_build_flow(options.no_browser)
                .await
                .map(|_| LoginFlowOutcome::Completed),
            LoginProviderTarget::OpenRouter => {
                login_openrouter_flow().map(|_| LoginFlowOutcome::Completed)
            }
            LoginProviderTarget::Bedrock => {
                login_bedrock_flow().map(|_| LoginFlowOutcome::Completed)
            }
            LoginProviderTarget::Azure => login_azure_flow().map(|_| LoginFlowOutcome::Completed),
            LoginProviderTarget::OpenAiCompatible(profile) => {
                login_openai_compatible_flow(&profile, &options)
                    .map(|_| LoginFlowOutcome::Completed)
            }
            LoginProviderTarget::Cursor => login_cursor_flow().map(|_| LoginFlowOutcome::Completed),
            LoginProviderTarget::Copilot => {
                login_copilot_flow(options.no_browser).map(|_| LoginFlowOutcome::Completed)
            }
            LoginProviderTarget::Gemini => login_gemini_flow(options.no_browser)
                .await
                .map(|_| LoginFlowOutcome::Completed),
            LoginProviderTarget::Antigravity => login_antigravity_flow(options.no_browser)
                .await
                .map(|_| LoginFlowOutcome::Completed),
            LoginProviderTarget::Google => {
                login_google_flow(options.no_browser, options.google_access_tier)
                    .await
                    .map(|_| LoginFlowOutcome::Completed)
            }
        }
    };
    let outcome = match login_result {
        Ok(outcome) => outcome,
        Err(err) => {
            let reason =
                crate::auth::login_diagnostics::classify_auth_failure_message(&err.to_string());
            crate::telemetry::record_auth_failed_reason(
                provider.id,
                provider.auth_kind.label(),
                reason.label(),
            );
            crate::logging::auth_event(
                "login_flow_failed",
                provider.id,
                &[
                    ("method", provider.auth_kind.label()),
                    ("reason", reason.label()),
                ],
            );
            return Err(anyhow::anyhow!(
                crate::auth::login_diagnostics::augment_auth_error_message(
                    provider.id,
                    err.to_string(),
                )
            ));
        }
    };
    if matches!(outcome, LoginFlowOutcome::Deferred) {
        crate::logging::auth_event(
            "login_flow_deferred",
            provider.id,
            &[("method", provider.auth_kind.label())],
        );
        return Ok(());
    }
    auth::AuthStatus::invalidate_cache();
    if options.no_validate {
        crate::console::eprintln_best_effort(
            "Skipping post-login provider validation (--no-validate).",
        );
        crate::logging::auth_event(
            "post_login_validation_skipped",
            provider.id,
            &[("reason", "no_validate")],
        );
        maybe_persist_default_provider_after_login(provider, &options);
        notify_running_server_auth_changed_best_effort(Some(provider.id)).await;
        return Ok(());
    }
    // Scriptable callers require exactly one JSON object on stdout. The human
    // validation report belongs only to interactive login, including failures.
    let validation = if options.json {
        super::auth_test::run_post_login_validation_quiet(provider).await
    } else {
        super::commands::run_post_login_validation(provider).await
    };
    if let Err(err) = validation {
        let error_message = err.to_string();
        let reason = crate::auth::login_diagnostics::classify_auth_failure_message(&error_message);
        crate::telemetry::record_auth_failed_reason(
            provider.id,
            provider.auth_kind.label(),
            reason.label(),
        );
        crate::logging::auth_event(
            "post_login_validation_failed",
            provider.id,
            &[
                ("method", provider.auth_kind.label()),
                ("reason", reason.label()),
            ],
        );
        return Err(anyhow::anyhow!(
            crate::auth::login_diagnostics::augment_auth_error_message(provider.id, error_message)
        ));
    }
    auth::AuthStatus::invalidate_cache();
    crate::logging::auth_event(
        "login_flow_completed",
        provider.id,
        &[
            ("method", provider.auth_kind.label()),
            ("validated", "true"),
        ],
    );
    maybe_persist_default_provider_after_login(provider, &options);
    notify_running_server_auth_changed_best_effort(Some(provider.id)).await;
    Ok(())
}

/// Native xAI OAuth device flow with the Grok CLI client id. Tokens are stored
/// in the Grok CLI credential store (`$GROK_HOME/auth.json`), so an existing
/// `grok login` is reused and this login is visible to the Grok CLI too.
async fn login_grok_build_flow(no_browser: bool) -> Result<()> {
    let client = crate::provider::shared_http_client();
    let authorization = crate::auth::grok_build::initiate_device_login(&client).await?;
    let url = authorization
        .verification_uri_complete
        .as_deref()
        .unwrap_or(&authorization.verification_uri);
    crate::console::eprintln_best_effort("\nGrok Build login (xAI)");
    crate::console::eprintln_best_effort(&format!("  Open: {url}"));
    crate::console::eprintln_best_effort(&format!("  Code: {}\n", authorization.user_code));
    maybe_open_browser(url, no_browser);
    crate::console::eprintln_best_effort("Waiting for authorization...");
    crate::auth::grok_build::complete_device_login(&client, &authorization).await?;
    crate::console::eprintln_best_effort("Grok Build login complete.");
    Ok(())
}

fn maybe_persist_default_provider_after_login(
    provider: LoginProviderDescriptor,
    options: &LoginOptions,
) {
    let cfg = crate::config::Config::load();
    if cfg.provider.default_provider.is_some() {
        return;
    }

    let provider_id =
        crate::provider::MultiProvider::config_default_provider_for_login_provider(provider);
    let Some(provider_id) = provider_id else {
        return;
    };

    let suggested_model = match provider.target {
        LoginProviderTarget::OpenAiCompatible(profile) => options
            .openai_compatible_default_model
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .or_else(|| resolve_openai_compatible_profile(profile).default_model),
        _ => None,
    };

    let model_to_save = cfg
        .provider
        .default_model
        .as_deref()
        .or(suggested_model.as_deref());

    if let Err(err) = crate::config::Config::set_default_model(model_to_save, Some(provider_id)) {
        crate::logging::warn(&format!(
            "Failed to save {} as the default provider after login: {}",
            provider_id, err
        ));
    }
}

/// Best-effort: tell a running jcode server that on-disk auth has changed so it
/// can hot-initialize any newly-configured providers. No-op if no server is running.
async fn notify_running_server_auth_changed_best_effort(provider: Option<&str>) {
    let Ok(mut client) = crate::server::Client::connect().await else {
        crate::logging::auth_event(
            "auth_changed_notify_skipped",
            "server",
            &[("reason", "no_running_server")],
        );
        return;
    };
    match client.notify_auth_changed_for_provider(provider).await {
        Ok(_) => crate::logging::auth_event("auth_changed_notify_sent", "server", &[]),
        Err(err) => {
            let reason = err.to_string();
            crate::logging::auth_event(
                "auth_changed_notify_failed",
                "server",
                &[("reason", reason.as_str())],
            );
        }
    }
}

async fn login_jcode_flow(no_browser: bool) -> Result<()> {
    crate::console::eprintln_best_effort("Starting jcode subscription sign-in...");
    let _ = jcode_device::login_jcode_device_flow(no_browser).await?;
    Ok(())
}

pub(crate) async fn run_jcode_account_login(no_browser: bool) -> Result<()> {
    login_jcode_flow(no_browser).await
}

fn login_openai_api_key_flow() -> Result<()> {
    crate::console::eprintln_best_effort("Setting up OpenAI API key...");
    crate::console::eprintln_best_effort(
        "Get your API key from: https://platform.openai.com/api-keys\n",
    );
    crate::console::eprompt_best_effort("Paste your OpenAI API key: ");
    io::stdout().flush()?;

    let key = read_secret_line()?;
    if key.is_empty() {
        anyhow::bail!("No API key provided.");
    }
    if !key.starts_with("sk-") {
        crate::console::eprintln_best_effort(
            "Warning: OpenAI API keys usually start with 'sk-'. Saving anyway.",
        );
    }

    save_named_api_key("openai.env", "OPENAI_API_KEY", &key)?;
    crate::console::eprintln_best_effort("\nSuccessfully saved OpenAI API key!");
    crate::console::eprintln_best_effort(&format!(
        "Stored at {}",
        crate::storage::app_config_dir()?
            .join("openai.env")
            .display()
    ));
    crate::console::eprintln_best_effort("Provider: openai-api (native OpenAI Responses API)");
    crate::telemetry::record_auth_success("openai-api", "api_key");
    Ok(())
}

async fn login_claude_flow(requested_label: Option<&str>, no_browser: bool) -> Result<()> {
    let label = auth::claude::login_target_label(requested_label)?;
    crate::console::eprintln_best_effort(&format!("Logging in to Claude (account: {})...", label));
    let tokens = auth::oauth::login_claude(no_browser).await?;
    auth::oauth::save_claude_tokens_for_account(&tokens, &label)?;
    let profile_email =
        match auth::oauth::update_claude_account_profile(&label, &tokens.access_token).await {
            Ok(email) => email,
            Err(e) => {
                crate::console::eprintln_best_effort(&format!(
                    "Warning: logged in but failed to fetch profile metadata: {}",
                    e
                ));
                None
            }
        };
    crate::console::eprintln_best_effort("Successfully logged in to Claude!");
    crate::console::eprintln_best_effort(&format!(
        "Account '{}' stored at {}",
        label,
        auth::claude::jcode_path()?.display()
    ));
    if let Some(email) = profile_email {
        crate::console::eprintln_best_effort(&format!("Profile email: {}", email));
    }
    crate::telemetry::record_auth_success("claude", "oauth");
    Ok(())
}

fn login_anthropic_api_key_flow() -> Result<()> {
    crate::console::eprintln_best_effort("Setting up Anthropic API...");
    crate::console::eprintln_best_effort(
        "Get your API key from: https://console.anthropic.com/settings/keys\n",
    );
    crate::console::eprompt_best_effort("Paste your Anthropic API key: ");
    io::stdout().flush()?;

    let key = read_secret_line()?;

    if key.is_empty() {
        anyhow::bail!("No API key provided.");
    }

    if !key.starts_with("sk-ant-") {
        crate::console::eprintln_best_effort(
            "Warning: Anthropic API keys typically start with 'sk-ant-'. Saving anyway.",
        );
    }

    save_named_api_key("anthropic.env", "ANTHROPIC_API_KEY", &key)?;
    crate::console::eprintln_best_effort("\nSuccessfully saved Anthropic API key!");
    crate::console::eprintln_best_effort(&format!(
        "Stored at {}",
        crate::storage::app_config_dir()?
            .join("anthropic.env")
            .display()
    ));
    crate::console::eprintln_best_effort("Provider: claude (native Anthropic Messages API)");
    crate::telemetry::record_auth_success("anthropic-api", "api_key");
    Ok(())
}

async fn login_openai_flow(requested_label: Option<&str>, no_browser: bool) -> Result<()> {
    let label = auth::codex::login_target_label(requested_label)?;
    crate::console::eprintln_best_effort(&format!(
        "Logging in to OpenAI/Codex (account: {})...",
        label
    ));
    let tokens = auth::oauth::login_openai(no_browser).await?;
    auth::oauth::save_openai_tokens_for_account(&tokens, &label)?;
    crate::console::eprintln_best_effort(&format!(
        "Successfully logged in to OpenAI! Account '{}' saved to {}",
        label,
        crate::storage::jcode_dir()?
            .join("openai-auth.json")
            .display()
    ));
    crate::telemetry::record_auth_success("openai", "oauth");
    Ok(())
}

fn login_openrouter_flow() -> Result<()> {
    crate::console::eprintln_best_effort("Setting up OpenRouter...");
    crate::console::eprintln_best_effort("Get your API key from: https://openrouter.ai/keys\n");
    crate::console::eprompt_best_effort("Paste your OpenRouter API key: ");
    io::stdout().flush()?;

    let key = read_secret_line()?;

    if key.is_empty() {
        anyhow::bail!("No API key provided.");
    }

    if !key.starts_with("sk-or-") {
        crate::console::eprintln_best_effort(
            "Warning: OpenRouter API keys typically start with 'sk-or-'. Saving anyway.",
        );
    }

    save_named_api_key("openrouter.env", "OPENROUTER_API_KEY", &key)?;
    crate::console::eprintln_best_effort("\nSuccessfully saved OpenRouter API key!");
    crate::console::eprintln_best_effort(&format!(
        "Stored at {}",
        crate::storage::app_config_dir()?
            .join("openrouter.env")
            .display()
    ));
    crate::telemetry::record_auth_success("openrouter", "api_key");
    Ok(())
}

fn login_bedrock_flow() -> Result<()> {
    crate::console::eprintln_best_effort("Setting up AWS Bedrock...");
    crate::console::eprintln_best_effort(
        "Generate a Bedrock API key in the AWS Bedrock console: https://console.aws.amazon.com/bedrock/home#/api-keys",
    );
    crate::console::eprintln_best_effort(
        "Short-term keys are recommended for onboarding/testing.\n",
    );

    let region = read_line_trimmed("AWS region [us-east-2]: ")?;
    let region = if region.trim().is_empty() {
        "us-east-2".to_string()
    } else {
        region.trim().to_string()
    };

    crate::console::eprompt_best_effort("Paste your Bedrock API key: ");
    io::stdout().flush()?;
    let key = read_secret_line()?;
    if key.is_empty() {
        anyhow::bail!("No API key provided.");
    }

    save_named_api_key(
        crate::provider::bedrock::ENV_FILE,
        crate::provider::bedrock::API_KEY_ENV,
        &key,
    )?;
    crate::provider_catalog::save_env_value_to_env_file(
        crate::provider::bedrock::REGION_ENV,
        crate::provider::bedrock::ENV_FILE,
        Some(&region),
    )?;

    crate::console::eprintln_best_effort("\nSuccessfully saved AWS Bedrock API key!");
    crate::console::eprintln_best_effort(&format!(
        "Stored at {}",
        crate::storage::app_config_dir()?
            .join(crate::provider::bedrock::ENV_FILE)
            .display()
    ));
    crate::console::eprintln_best_effort(&format!("Region: {}", region));
    crate::console::eprintln_best_effort("Provider: bedrock (native AWS Bedrock Converse API)");
    crate::telemetry::record_auth_success("bedrock", "api_key");
    Ok(())
}

fn login_azure_flow() -> Result<()> {
    use crate::auth::azure;

    crate::console::eprintln_best_effort("Setting up Azure OpenAI...");
    crate::console::eprintln_best_effort(
        "Reference: OpenCode supports Azure OpenAI with Entra credentials. jcode uses Azure OpenAI's newer `/openai/v1` API with either Microsoft Entra ID or an API key.\n",
    );

    let endpoint_raw = read_line_trimmed(
        "Azure OpenAI endpoint (for example `https://your-resource.openai.azure.com`): ",
    )?;
    let endpoint = azure::normalize_endpoint(&endpoint_raw).ok_or_else(|| {
        anyhow::anyhow!(
            "Invalid Azure OpenAI endpoint. Use https://<resource>.openai.azure.com (or the full /openai/v1 URL)."
        )
    })?;

    let model =
        read_line_trimmed("Azure deployment/model name (required, for example `gpt-4.1-nano`): ")?;
    if model.is_empty() {
        anyhow::bail!("No deployment/model name provided.");
    }

    crate::console::eprintln_best_effort("\nAuthentication method:");
    crate::console::eprintln_best_effort("  1. Microsoft Entra ID (recommended)");
    crate::console::eprintln_best_effort("  2. API key");
    let auth_choice = read_line_trimmed("Enter 1-2 [1]: ")?;
    let use_entra = match auth_choice.trim() {
        "" | "1" => true,
        "2" => false,
        other if other.eq_ignore_ascii_case("entra") || other.eq_ignore_ascii_case("oauth") => true,
        other if other.eq_ignore_ascii_case("key") || other.eq_ignore_ascii_case("api-key") => {
            false
        }
        other => anyhow::bail!("Invalid auth choice '{}'. Use 1 or 2.", other),
    };

    let mut assignments = vec![
        (azure::ENDPOINT_ENV, endpoint),
        (azure::MODEL_ENV, model),
        (
            azure::USE_ENTRA_ENV,
            if use_entra { "1" } else { "0" }.to_string(),
        ),
    ];

    if use_entra {
        crate::console::eprintln_best_effort("");
        crate::console::eprintln_best_effort(
            "Using Microsoft Entra ID via Azure's DefaultAzureCredential chain.",
        );
        crate::console::eprintln_best_effort(
            "That means jcode can authenticate via `az login`, managed identity, or Azure environment credentials.",
        );
    } else {
        crate::console::eprompt_best_effort("Paste your Azure OpenAI API key: ");
        io::stdout().flush()?;
        let key = read_secret_line()?;
        if key.is_empty() {
            anyhow::bail!("No API key provided.");
        }
        assignments.push((azure::API_KEY_ENV, key));
    }

    save_named_env_vars(azure::ENV_FILE, &assignments)?;
    azure::apply_runtime_env()?;

    crate::console::eprintln_best_effort("\nSuccessfully saved Azure OpenAI configuration!");
    crate::console::eprintln_best_effort(&format!(
        "Stored at {}",
        crate::storage::app_config_dir()?
            .join(azure::ENV_FILE)
            .display()
    ));
    crate::console::eprintln_best_effort(&format!(
        "Base URL: {}",
        azure::load_endpoint().unwrap_or_default()
    ));
    if let Some(model) = azure::load_model() {
        crate::console::eprintln_best_effort(&format!("Default deployment/model: {}", model));
    }
    if use_entra {
        crate::console::eprintln_best_effort(
            "Next step: if you're using Azure CLI auth, run `az login` (and ensure your identity has the Cognitive Services OpenAI User role).",
        );
    }
    crate::telemetry::record_auth_success("azure", if use_entra { "entra_id" } else { "api_key" });
    Ok(())
}

fn login_openai_compatible_flow(
    profile: &OpenAiCompatibleProfile,
    options: &LoginOptions,
) -> Result<()> {
    let is_custom_profile = profile.id == crate::provider_catalog::OPENAI_COMPAT_PROFILE.id;
    let mut resolved = resolve_openai_compatible_profile(*profile);

    crate::console::eprintln_best_effort(&format!("Setting up {}...", resolved.display_name));
    let setup_url_depends_on_key = profile.id == crate::provider_catalog::MINIMAX_PROFILE.id;
    if !setup_url_depends_on_key {
        crate::console::eprintln_best_effort(&format!(
            "See setup details: {}\n",
            resolved.setup_url
        ));
    }

    if is_custom_profile {
        if !io::stdin().is_terminal()
            && options.openai_compatible_api_base.is_none()
            && options.openai_compatible_api_key.is_none()
        {
            anyhow::bail!(
                "Non-interactive OpenAI-compatible login requires --api-base and --api-key. \
                 This avoids accidentally saving a piped model name or other answer as the API key."
            );
        }
        crate::console::eprintln_best_effort(
            "You can point this at a hosted OpenAI-compatible API or a local server such as LM Studio or Ollama.",
        );
        let api_base_input = match options.openai_compatible_api_base.as_deref() {
            Some(value) => value.trim().to_string(),
            None => read_line_trimmed(&format!("API base URL [{}]: ", resolved.api_base))?,
        };
        if !api_base_input.is_empty() {
            let normalized = crate::provider_catalog::normalize_api_base(&api_base_input)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Invalid OpenAI-compatible API base. Use https://... or http://localhost..."
                    )
                })?;
            crate::provider_catalog::save_env_value_to_env_file(
                "JCODE_OPENAI_COMPAT_API_BASE",
                crate::provider_catalog::OPENAI_COMPAT_PROFILE.env_file,
                Some(&normalized),
            )?;
            resolved = resolve_openai_compatible_profile(*profile);
        }

        if let Some(api_key_env) = options
            .openai_compatible_api_key_env
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if !crate::provider_catalog::is_safe_env_key_name(api_key_env) {
                anyhow::bail!("Invalid API key environment variable name: {}", api_key_env);
            }
            crate::provider_catalog::save_env_value_to_env_file(
                "JCODE_OPENAI_COMPAT_API_KEY_NAME",
                crate::provider_catalog::OPENAI_COMPAT_PROFILE.env_file,
                Some(api_key_env),
            )?;
            resolved = resolve_openai_compatible_profile(*profile);
        }

        let default_model_input = match options.openai_compatible_default_model.as_deref() {
            Some(value) => value.trim().to_string(),
            None if !io::stdin().is_terminal() => String::new(),
            None => read_line_trimmed("Default model name (optional, press Enter to skip): ")?,
        };
        if !default_model_input.is_empty() {
            crate::provider_catalog::save_env_value_to_env_file(
                "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
                crate::provider_catalog::OPENAI_COMPAT_PROFILE.env_file,
                Some(&default_model_input),
            )?;
            resolved = resolve_openai_compatible_profile(*profile);
        }
        crate::console::eprintln_best_effort("");
    } else if let Some(model) = options
        .openai_compatible_default_model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        resolved.default_model = Some(model.to_string());
    }

    let auth_method = if resolved.requires_api_key {
        crate::console::eprintln_best_effort(&format!(
            "API key env variable: {}\n",
            resolved.api_key_env
        ));
        if options.openai_compatible_api_key.is_none() {
            existing_key_notice::announce_existing_api_key(&resolved);
        }
        let key = match options.openai_compatible_api_key.as_deref() {
            Some(value) => value.trim().to_string(),
            None => {
                crate::console::eprompt_best_effort(&format!(
                    "Paste your {} API key: ",
                    resolved.display_name
                ));
                io::stdout().flush()?;
                read_secret_line()?
            }
        };
        if key.is_empty() {
            anyhow::bail!("No API key provided.");
        }
        resolved = crate::provider_catalog::resolve_openai_compatible_profile_with_api_key_hint(
            *profile,
            Some(&key),
        );
        crate::console::eprintln_best_effort(&format!("Endpoint: {}", resolved.api_base));
        if setup_url_depends_on_key {
            crate::console::eprintln_best_effort(&format!(
                "See setup details: {}",
                resolved.setup_url
            ));
        }

        crate::provider_catalog::save_env_value_to_env_file(
            OPENAI_COMPAT_LOCAL_ENABLED_ENV,
            &resolved.env_file,
            None,
        )?;
        save_named_api_key(&resolved.env_file, &resolved.api_key_env, &key)?;
        crate::console::eprintln_best_effort(&format!(
            "\nSuccessfully saved {} API key!",
            resolved.display_name
        ));
        "api_key"
    } else {
        crate::console::eprintln_best_effort(&format!("Endpoint: {}", resolved.api_base));
        if setup_url_depends_on_key {
            crate::console::eprintln_best_effort(&format!(
                "See setup details: {}",
                resolved.setup_url
            ));
        }
        crate::console::eprintln_best_effort(
            "This provider uses a local OpenAI-compatible endpoint.",
        );
        crate::console::eprintln_best_effort(
            "An API key is optional here. Press Enter to skip if your local server does not require one.\n",
        );
        let key = match options.openai_compatible_api_key.as_deref() {
            Some(value) => value.trim().to_string(),
            None => {
                crate::console::eprompt_best_effort(&format!(
                    "Optional {} API key: ",
                    resolved.display_name
                ));
                io::stdout().flush()?;
                read_secret_line()?
            }
        };
        crate::provider_catalog::save_env_value_to_env_file(
            OPENAI_COMPAT_LOCAL_ENABLED_ENV,
            &resolved.env_file,
            Some("1"),
        )?;
        if key.trim().is_empty() {
            crate::provider_catalog::save_env_value_to_env_file(
                &resolved.api_key_env,
                &resolved.env_file,
                None,
            )?;
            crate::console::eprintln_best_effort(&format!(
                "\nSaved {} local endpoint setup.",
                resolved.display_name
            ));
            "local_endpoint"
        } else {
            crate::provider_catalog::save_named_api_key(
                &resolved.env_file,
                &resolved.api_key_env,
                key.trim(),
            )?;
            crate::console::eprintln_best_effort(&format!(
                "\nSaved {} local endpoint setup and optional API key.",
                resolved.display_name
            ));
            "local_endpoint_with_optional_api_key"
        }
    };

    if !resolved.requires_api_key && resolved.default_model.is_none() {
        crate::console::eprintln_best_effort(&format!(
            "{}",
            next_step::local_endpoint_hint(&resolved.id)
        ));
    }

    crate::console::eprintln_best_effort(&format!(
        "Stored at {}",
        crate::storage::app_config_dir()?
            .join(&resolved.env_file)
            .display()
    ));
    if let Some(default_model) = resolved.default_model {
        crate::console::eprintln_best_effort(&format!("Default model hint: {}", default_model));
    }
    crate::telemetry::record_auth_success(&resolved.id, auth_method);
    Ok(())
}

pub use crate::secret_input::read_secret_line;

fn read_line_trimmed(prompt: &str) -> Result<String> {
    print!("{}", prompt);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

fn save_named_env_vars(env_file: &str, vars: &[(&str, String)]) -> Result<()> {
    if !crate::provider_catalog::is_safe_env_file_name(env_file) {
        anyhow::bail!("Invalid env file name: {}", env_file);
    }

    for (key, _) in vars {
        if !crate::provider_catalog::is_safe_env_key_name(key) {
            anyhow::bail!("Invalid API key variable name: {}", key);
        }
    }

    let config_dir = crate::storage::app_config_dir()?;
    std::fs::create_dir_all(&config_dir)?;
    crate::platform::set_directory_permissions_owner_only(&config_dir)?;

    let file_path = config_dir.join(env_file);
    let mut content = String::new();
    for (key, value) in vars {
        content.push_str(&format!("{}={}\n", key, value));
    }
    std::fs::write(&file_path, &content)?;
    crate::platform::set_permissions_owner_only(&file_path)?;

    // File only: these assignments can include secrets (the Azure API key),
    // and a process env copy would shadow later file edits and leak into
    // child processes (#1386). Readers fall back to the env file.
    Ok(())
}

fn login_cursor_flow() -> Result<()> {
    crate::console::eprintln_best_effort("Starting Cursor API key setup...");

    crate::console::eprintln_best_effort("Get your API key from: https://cursor.com/settings");
    crate::console::eprintln_best_effort("(Dashboard > Integrations > User API Keys)\n");
    crate::console::eprompt_best_effort("Paste your Cursor API key: ");
    io::stdout().flush()?;

    let key = read_secret_line()?;
    if key.is_empty() {
        anyhow::bail!("No API key provided.");
    }

    save_named_api_key("cursor.env", "CURSOR_API_KEY", &key)?;
    crate::auth::AuthStatus::invalidate_cache();
    crate::console::eprintln_best_effort("\nSuccessfully saved Cursor API key!");
    crate::console::eprintln_best_effort(&format!(
        "Stored at {}",
        crate::storage::app_config_dir()?
            .join("cursor.env")
            .display()
    ));
    crate::console::eprintln_best_effort("jcode will use the native Cursor HTTPS transport.");
    crate::telemetry::record_auth_success("cursor", "api_key");
    Ok(())
}

fn login_copilot_flow(no_browser: bool) -> Result<()> {
    crate::console::eprintln_best_effort("Starting GitHub Copilot login...");

    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(login_copilot_device_flow(no_browser))
    })
}

async fn login_copilot_device_flow(no_browser: bool) -> Result<()> {
    let client = crate::provider::shared_http_client();

    let device_resp = crate::auth::copilot::initiate_device_flow(&client).await?;

    crate::console::eprintln_best_effort("");
    crate::console::eprintln_best_effort("  Open this URL in your browser:");
    crate::console::eprintln_best_effort(&format!("    {}", device_resp.verification_uri));
    crate::console::eprintln_best_effort("");
    if let Some(qr) = crate::login_qr::indented_section(
        &device_resp.verification_uri,
        "  Or scan this QR on another device to open the verification page:",
        "    ",
        crate::auth::browser_suppressed(no_browser),
    ) {
        crate::console::eprintln_best_effort(&format!("{qr}"));
        crate::console::eprintln_best_effort("");
    }
    crate::console::eprintln_best_effort(&format!("  Enter code: {}", device_resp.user_code));
    crate::console::eprintln_best_effort("");
    crate::console::eprintln_best_effort("  Waiting for authorization...");

    maybe_open_browser(&device_resp.verification_uri, no_browser);

    let token = crate::auth::copilot::poll_for_access_token(
        &client,
        &device_resp.device_code,
        device_resp.interval,
    )
    .await?;

    let username = crate::auth::copilot::fetch_github_username(&client, &token)
        .await
        .unwrap_or_else(|_| "unknown".to_string());

    crate::auth::copilot::save_github_token(&token, &username)?;

    crate::console::eprintln_best_effort(&format!(
        "  ✓ Authenticated as {} via GitHub Copilot",
        username
    ));
    crate::telemetry::record_auth_success("copilot", "oauth_device_code");
    Ok(())
}

async fn login_antigravity_flow(no_browser: bool) -> Result<()> {
    crate::console::eprintln_best_effort("Starting native Antigravity login...");
    crate::console::eprintln_best_effort(
        "jcode will authenticate directly with Google Antigravity; the Antigravity desktop app is not required.",
    );
    crate::console::eprintln_best_effort(
        "If browser launch fails, or you pass `--no-browser`, jcode will prompt for the callback URL instead.",
    );
    crate::console::eprintln_best_effort(
        "If the browser later shows a loopback/callback error page, copy the full URL from the address bar and re-run with `--no-browser`.",
    );
    crate::console::eprintln_best_effort("");

    let tokens = crate::auth::antigravity::login(no_browser).await?;

    crate::console::eprintln_best_effort("Successfully logged in to Antigravity!");
    crate::console::eprintln_best_effort(&format!(
        "Tokens saved to {}",
        crate::auth::antigravity::tokens_path()?.display()
    ));
    if let Some(email) = tokens.email.as_deref() {
        crate::console::eprintln_best_effort(&format!("Google account: {}", email));
    }
    if let Some(project_id) = tokens.project_id.as_deref() {
        crate::console::eprintln_best_effort(&format!(
            "Resolved Antigravity project: {}",
            project_id
        ));
    }
    crate::telemetry::record_auth_success("antigravity", "oauth");
    Ok(())
}

async fn login_gemini_flow(no_browser: bool) -> Result<()> {
    // Offer the auth-method choice only on an interactive terminal so scripted
    // / piped invocations preserve the historical OAuth-only behavior.
    if io::stdin().is_terminal() {
        crate::console::eprintln_best_effort("Gemini login. Choose an authentication method:");
        crate::console::eprintln_best_effort(
            "  [1] Google account OAuth (free Code Assist tier, default)",
        );
        crate::console::eprintln_best_effort(
            "  [2] Gemini Developer API key (Google AI Studio, generativelanguage.googleapis.com)",
        );
        crate::console::eprintln_best_effort("");
        let choice = read_line_trimmed("Enter 1-2 [1]: ")?;
        if choice == "2" {
            return login_gemini_api_key_flow();
        }
    }

    crate::console::eprintln_best_effort("Starting native Gemini login...");
    crate::console::eprintln_best_effort(
        "If your student/education plan is attached to your Google account, use that account in the browser flow.",
    );
    crate::console::eprintln_best_effort(
        "If browser launch fails, or you pass `--no-browser`, jcode will prompt for the manual authorization code.",
    );
    crate::console::eprintln_best_effort(
        "Note: school / Workspace Google accounts may also require GOOGLE_CLOUD_PROJECT and GOOGLE_CLOUD_LOCATION for Code Assist entitlement checks.",
    );
    crate::console::eprintln_best_effort("");

    let tokens = crate::auth::gemini::login(no_browser).await?;

    crate::console::eprintln_best_effort("Successfully logged in to Gemini!");
    crate::console::eprintln_best_effort(&format!(
        "Tokens saved to {}",
        crate::auth::gemini::tokens_path()?.display()
    ));
    if let Some(email) = tokens.email.as_deref() {
        crate::console::eprintln_best_effort(&format!("Google account: {}", email));
    }
    crate::telemetry::record_auth_success("gemini", "oauth");
    Ok(())
}

fn login_gemini_api_key_flow() -> Result<()> {
    crate::console::eprintln_best_effort("Setting up Gemini Developer API key...");
    crate::console::eprintln_best_effort(
        "Get your API key from: https://aistudio.google.com/apikey\n",
    );
    crate::console::eprompt_best_effort("Paste your Gemini API key: ");
    io::stdout().flush()?;

    let key = read_secret_line()?;
    if key.is_empty() {
        anyhow::bail!("No API key provided.");
    }

    crate::auth::gemini::save_api_key(&key)?;
    crate::console::eprintln_best_effort("\nSuccessfully saved Gemini Developer API key!");
    crate::console::eprintln_best_effort(&format!(
        "Stored at {}",
        crate::storage::app_config_dir()?
            .join(crate::auth::gemini::GEMINI_API_KEY_ENV_FILE)
            .display()
    ));
    crate::console::eprintln_best_effort(
        "Provider: gemini (official Gemini Developer API, generativelanguage.googleapis.com)",
    );
    crate::telemetry::record_auth_success("gemini", "api_key");
    Ok(())
}

async fn login_google_flow(
    no_browser: bool,
    access_tier: Option<auth::google::GmailAccessTier>,
) -> Result<()> {
    use auth::google::{GmailAccessTier, GoogleCredentials};

    crate::console::eprintln_best_effort("╔══════════════════════════════════════════╗");
    crate::console::eprintln_best_effort("║       Gmail Integration Setup            ║");
    crate::console::eprintln_best_effort("╚══════════════════════════════════════════╝\n");

    let _creds = match auth::google::load_credentials() {
        Ok(creds) => {
            crate::console::eprintln_best_effort(&format!(
                "✓ Google credentials found (client_id: {}...)\n",
                &creds.client_id[..20.min(creds.client_id.len())]
            ));
            creds
        }
        Err(_) => {
            crate::console::eprintln_best_effort(
                "No Google credentials found. Let's set them up.\n",
            );
            crate::console::eprintln_best_effort(
                "You need OAuth credentials from Google Cloud Console.",
            );
            crate::console::eprintln_best_effort("How would you like to provide them?\n");
            crate::console::eprintln_best_effort(
                "  [1] Paste client ID and secret directly (easiest)",
            );
            crate::console::eprintln_best_effort(
                "  [2] Provide path to downloaded JSON credentials file",
            );
            crate::console::eprintln_best_effort(
                "  [3] I need help creating credentials (opens setup guide)\n",
            );
            crate::console::eprompt_best_effort("Choose [1/2/3]: ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;

            match input.trim() {
                "1" => {
                    crate::console::eprintln_best_effort("\nPaste your Google OAuth Client ID:");
                    crate::console::eprintln_best_effort(
                        "  (looks like: 123456789-abc.apps.googleusercontent.com)\n",
                    );
                    crate::console::eprompt_best_effort("> ");
                    io::stdout().flush()?;
                    let mut client_id = String::new();
                    io::stdin().read_line(&mut client_id)?;
                    let client_id = client_id.trim().to_string();

                    if client_id.is_empty() {
                        anyhow::bail!("No client ID provided.");
                    }

                    crate::console::eprintln_best_effort(
                        "\nPaste your Google OAuth Client Secret:",
                    );
                    crate::console::eprintln_best_effort("  (looks like: GOCSPX-...)\n");
                    crate::console::eprompt_best_effort("> ");
                    io::stdout().flush()?;
                    let mut client_secret = String::new();
                    io::stdin().read_line(&mut client_secret)?;
                    let client_secret = client_secret.trim().to_string();

                    if client_secret.is_empty() {
                        anyhow::bail!("No client secret provided.");
                    }

                    let creds = GoogleCredentials {
                        client_id,
                        client_secret,
                    };
                    auth::google::save_credentials(&creds)?;
                    crate::console::eprintln_best_effort(&format!(
                        "\n✓ Credentials saved to {}\n",
                        auth::google::credentials_path()?.display()
                    ));
                    creds
                }
                "2" => {
                    crate::console::eprintln_best_effort(
                        "\nPaste the path to your downloaded JSON file:\n",
                    );
                    crate::console::eprompt_best_effort("> ");
                    io::stdout().flush()?;
                    let mut path_input = String::new();
                    io::stdin().read_line(&mut path_input)?;
                    let path_str = path_input.trim();

                    let path_str = if let Some(stripped) = path_str.strip_prefix("~/") {
                        if let Some(home) = dirs::home_dir() {
                            home.join(stripped).to_string_lossy().to_string()
                        } else {
                            path_str.to_string()
                        }
                    } else {
                        path_str.to_string()
                    };

                    let data = std::fs::read_to_string(&path_str)
                        .with_context(|| format!("Could not read file: {}", path_str))?;

                    let dest = auth::google::credentials_path()?;
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent)?;
                        crate::platform::set_directory_permissions_owner_only(parent)?;
                    }
                    std::fs::write(&dest, &data)?;
                    crate::platform::set_permissions_owner_only(&dest)?;

                    let creds = auth::google::load_credentials()
                        .context("Could not parse the credentials file. Make sure it's the OAuth client JSON from Google Cloud Console.")?;

                    crate::console::eprintln_best_effort(&format!(
                        "\n✓ Credentials imported to {}\n",
                        dest.display()
                    ));
                    creds
                }
                "3" => {
                    crate::console::eprintln_best_effort(
                        "\n── Step-by-step Google Cloud setup ──\n",
                    );

                    crate::console::eprintln_best_effort(
                        "1. Open Google Cloud Console and create a project:",
                    );
                    crate::console::eprintln_best_effort(
                        "   Opening: https://console.cloud.google.com/projectcreate\n",
                    );
                    maybe_open_browser(
                        "https://console.cloud.google.com/projectcreate",
                        no_browser,
                    );
                    crate::console::eprompt_best_effort(
                        "   Press Enter when your project is created...",
                    );
                    io::stdout().flush()?;
                    let mut wait = String::new();
                    io::stdin().read_line(&mut wait)?;

                    crate::console::eprintln_best_effort("\n2. Enable the Gmail API:");
                    crate::console::eprintln_best_effort("   Opening: Gmail API library page\n");
                    maybe_open_browser(
                        "https://console.cloud.google.com/apis/library/gmail.googleapis.com",
                        no_browser,
                    );
                    crate::console::eprintln_best_effort("   Click the blue 'Enable' button.");
                    crate::console::eprompt_best_effort("   Press Enter when done...");
                    io::stdout().flush()?;
                    io::stdin().read_line(&mut wait)?;

                    crate::console::eprintln_best_effort("\n3. Configure OAuth consent screen:");
                    crate::console::eprintln_best_effort("   Opening: OAuth consent screen\n");
                    maybe_open_browser(
                        "https://console.cloud.google.com/apis/credentials/consent",
                        no_browser,
                    );
                    crate::console::eprintln_best_effort("   - Choose 'External' user type");
                    crate::console::eprintln_best_effort(
                        "   - Fill in app name (e.g. 'jcode') and your email",
                    );
                    crate::console::eprintln_best_effort(
                        "   - Skip scopes (we'll request them during login)",
                    );
                    crate::console::eprintln_best_effort("   - Add your email as a test user");
                    crate::console::eprintln_best_effort(
                        "   - Save and continue through all steps",
                    );
                    crate::console::eprompt_best_effort("   Press Enter when done...");
                    io::stdout().flush()?;
                    io::stdin().read_line(&mut wait)?;

                    crate::console::eprintln_best_effort("\n4. Create OAuth credentials:");
                    crate::console::eprintln_best_effort("   Opening: Credentials page\n");
                    maybe_open_browser(
                        "https://console.cloud.google.com/apis/credentials",
                        no_browser,
                    );
                    crate::console::eprintln_best_effort(
                        "   - Click '+ Create Credentials' > 'OAuth client ID'",
                    );
                    crate::console::eprintln_best_effort("   - Application type: 'Desktop app'");
                    crate::console::eprintln_best_effort("   - Name: 'jcode'");
                    crate::console::eprintln_best_effort("   - Click 'Create'\n");
                    crate::console::eprintln_best_effort(
                        "   A dialog will show your Client ID and Client Secret.\n",
                    );

                    crate::console::eprintln_best_effort("Paste your Client ID:");
                    crate::console::eprompt_best_effort("> ");
                    io::stdout().flush()?;
                    let mut client_id = String::new();
                    io::stdin().read_line(&mut client_id)?;
                    let client_id = client_id.trim().to_string();

                    if client_id.is_empty() {
                        anyhow::bail!("No client ID provided.");
                    }

                    crate::console::eprintln_best_effort("\nPaste your Client Secret:");
                    crate::console::eprompt_best_effort("> ");
                    io::stdout().flush()?;
                    let mut client_secret = String::new();
                    io::stdin().read_line(&mut client_secret)?;
                    let client_secret = client_secret.trim().to_string();

                    if client_secret.is_empty() {
                        anyhow::bail!("No client secret provided.");
                    }

                    let creds = GoogleCredentials {
                        client_id,
                        client_secret,
                    };
                    auth::google::save_credentials(&creds)?;
                    crate::console::eprintln_best_effort("\n✓ Credentials saved!\n");
                    creds
                }
                _ => {
                    crate::console::eprintln_best_effort(
                        "\nInvalid choice. Please enter 1, 2, or 3.\n",
                    );
                    std::process::exit(1);
                }
            }
        }
    };

    let tier = if let Some(tier) = access_tier {
        tier
    } else {
        crate::console::eprintln_best_effort("── Gmail Access Level ──\n");
        crate::console::eprintln_best_effort("  [1] Full Access (recommended)");
        crate::console::eprintln_best_effort("      Search, read, draft, send, and manage emails.");
        crate::console::eprintln_best_effort(
            "      Send and delete always require your confirmation.\n",
        );
        crate::console::eprintln_best_effort("  [2] Read & Draft Only");
        crate::console::eprintln_best_effort(
            "      Search, read emails, create drafts. Cannot send or delete.",
        );
        crate::console::eprintln_best_effort(
            "      API-level restriction - impossible even if the AI tries.\n",
        );
        crate::console::eprompt_best_effort("Choose [1/2] (default: 1): ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        match input.trim() {
            "" | "1" => GmailAccessTier::Full,
            "2" => GmailAccessTier::ReadOnly,
            _ => {
                crate::console::eprintln_best_effort("Invalid choice, defaulting to Full Access.");
                GmailAccessTier::Full
            }
        }
    };

    crate::console::eprintln_best_effort(&format!("\nAccess level: {}", tier.label()));

    crate::console::eprintln_best_effort("\n── Logging in ──\n");

    let tokens = auth::google::login(tier, no_browser).await?;

    crate::console::eprintln_best_effort("\n╔══════════════════════════════════════════╗");
    crate::console::eprintln_best_effort("║  ✓ Gmail setup complete!                 ║");
    crate::console::eprintln_best_effort("╚══════════════════════════════════════════╝\n");
    if let Some(email) = &tokens.email {
        crate::console::eprintln_best_effort(&format!("  Account:      {}", email));
    }
    crate::console::eprintln_best_effort(&format!("  Access tier:  {}", tokens.tier.label()));
    crate::console::eprintln_best_effort(&format!(
        "  Credentials:  {}",
        auth::google::credentials_path()?.display()
    ));
    crate::console::eprintln_best_effort(&format!(
        "  Tokens:       {}\n",
        auth::google::tokens_path()?.display()
    ));
    crate::console::eprintln_best_effort(
        "The 'gmail' tool is enabled by default in the full tool profile.",
    );
    crate::console::eprintln_best_effort(
        "To hide it, add `disabled = [\"gmail\"]` to [tools] in config.toml.",
    );
    crate::console::eprintln_best_effort(
        "Then try asking: \"check my recent emails\" or \"search emails from ...\"",
    );

    crate::telemetry::record_auth_success("google", "oauth");
    Ok(())
}

fn maybe_open_browser(target: &str, no_browser: bool) -> bool {
    if crate::auth::browser_suppressed(no_browser) {
        false
    } else {
        open::that(target).is_ok()
    }
}

#[cfg(test)]
mod tests;
