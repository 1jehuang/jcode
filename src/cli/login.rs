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
mod google;
mod jcode_device;
mod native_reuse;
use native_reuse::reuse_claude_code_native_snapshot;
mod next_step;
mod scriptable;
use google::*;
use scriptable::*;

#[derive(Debug, Clone, Default)]
pub struct LoginOptions {
    pub no_browser: bool,
    pub claude_code: bool,
    pub print_auth_url: bool,
    pub callback_url: Option<String>,
    pub auth_code: Option<String>,
    pub json: bool,
    pub complete: bool,
    pub flow_id: Option<String>,
    pub cancel: bool,
    pub no_validate: bool,
    pub google_access_tier: Option<auth::google::GmailAccessTier>,
    pub google_services: Option<Vec<auth::google::GoogleService>>,
    /// Run the guided Google Cloud OAuth client setup even when a client is saved.
    pub google_setup: bool,
    /// OAuth client JSON path (or `auto`) to import before Google login.
    pub google_client_json: Option<String>,
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
        /// Missing in pending files written by older builds, which only
        /// requested Gmail.
        #[serde(default = "default_pending_google_services")]
        services: Vec<auth::google::GoogleService>,
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
    if options.claude_code && login_provider_for_choice(choice).is_none() {
        anyhow::bail!("--claude-code requires --provider claude.");
    }
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
                eprintln!("\nImported {} existing auth source(s).", imported);
                notify_running_server_auth_changed_best_effort(None).await;
                return Ok(());
            }
            match super::provider_init::prompt_login_provider_selection_optional(
                &providers,
                "Choose a provider to log in:",
            )? {
                Some(provider) => run_login_provider(provider, account_label, options).await?,
                None => eprintln!("Login skipped."),
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
    validate_claude_code_method(provider, account_label, &options)?;
    if options.cancel {
        return cancel_scriptable_login(provider, &options);
    }
    if provider.target == LoginProviderTarget::Google
        && let Some(source) = options.google_client_json.as_deref()
    {
        import_google_client_json(source, options.json)?;
    }
    crate::telemetry::record_provider_selected(provider.id);
    crate::telemetry::record_auth_started(provider.id, provider.auth_kind.label());
    let explicit_scriptable_flow = options.uses_scriptable_flow()?;
    // Google setup is an interactive wizard. On a terminal it must run even
    // with --no-browser (it prints each URL), otherwise a first-time user
    // with no OAuth client is sent to a flow that can only fail.
    let google_wizard = provider.target == LoginProviderTarget::Google
        && !explicit_scriptable_flow
        && io::stdin().is_terminal()
        && (options.google_setup || auth::google::load_credentials().is_err());
    let auto_scriptable_reason = if explicit_scriptable_flow || google_wizard {
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
            eprintln!(
                "Detected a manual-safe login environment for {}. Starting the auth URL flow instead of browser-first login.",
                provider.display_name
            );
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
                eprintln!("Imported {} existing auth source(s).", imported);
                Ok(LoginFlowOutcome::Completed)
            }
            LoginProviderTarget::Jcode => login_jcode_flow(options.no_browser)
                .await
                .map(|_| LoginFlowOutcome::Completed),
            LoginProviderTarget::Claude if options.claude_code => {
                login_claude_code_flow().map(|_| LoginFlowOutcome::Completed)
            }
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
            LoginProviderTarget::ClaudeCode => {
                login_claude_code_instance_flow(account_label).map(|_| LoginFlowOutcome::Completed)
            }
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
            LoginProviderTarget::Google => login_google_flow(
                options.no_browser,
                options.google_access_tier,
                options.google_services.clone(),
                options.google_setup,
            )
            .await
            .map(|_| LoginFlowOutcome::Completed),
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
        eprintln!("Skipping post-login provider validation (--no-validate).");
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

/// Resolved `claude auth login` invocation for one Claude Code instance:
/// (binary, `CLAUDE_CONFIG_DIR` when the instance has its own home, instance id).
pub(crate) fn claude_code_login_command(
    settings: &crate::config::ClaudeCodeConfig,
    account_label: Option<&str>,
) -> Result<(String, Option<PathBuf>, String)> {
    let instance_id = account_label
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| settings.resolved_default_instance());
    let Some(instance) = settings.instance(&instance_id) else {
        let known = settings
            .effective_instances()
            .into_iter()
            .map(|instance| instance.id)
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!(
            "No Claude Code instance '{instance_id}' in [provider.claude_code]. Known instances: {known}. Add it under [[provider.claude_code.instances]] in config.toml first."
        );
    };
    Ok((
        settings.resolved_binary(),
        instance.resolved_home(),
        instance.id,
    ))
}

/// `jcode login --provider claude-code [--account <instance>]`: run the
/// official `claude auth login` for one configured instance. The CLI owns the
/// resulting credentials; nothing is copied into jcode's auth store.
fn login_claude_code_instance_flow(account_label: Option<&str>) -> Result<()> {
    let settings = crate::auth::claude_code::settings();
    let (binary, home, instance_id) = claude_code_login_command(&settings, account_label)?;
    anyhow::ensure!(
        io::stdin().is_terminal(),
        "`claude auth login` needs an interactive terminal. Run `jcode login --provider claude-code{}` in a terminal.",
        account_label
            .map(|label| format!(" --account {label}"))
            .unwrap_or_default()
    );
    anyhow::ensure!(
        crate::auth::claude_code::command_available(&binary),
        "Claude Code CLI `{binary}` was not found. Install Claude Code, or set [provider.claude_code].binary / JCODE_CLAUDE_CODE_BIN."
    );
    let mut command = std::process::Command::new(&binary);
    command.args(["auth", "login"]);
    match &home {
        Some(home) => {
            std::fs::create_dir_all(home)
                .with_context(|| format!("Failed to create {}", home.display()))?;
            eprintln!(
                "Signing in Claude Code instance '{instance_id}' (CLAUDE_CONFIG_DIR={})…",
                home.display()
            );
            command.env("CLAUDE_CONFIG_DIR", home);
        }
        None => {
            eprintln!(
                "Signing in Claude Code instance '{instance_id}' (Claude Code default login)…"
            );
            // The default instance uses the CLI's own default home. Never
            // inherit a CLAUDE_CONFIG_DIR from this shell into it.
            command.env_remove("CLAUDE_CONFIG_DIR");
        }
    }
    let status = command
        .status()
        .with_context(|| format!("Failed to start `{binary} auth login`"))?;
    anyhow::ensure!(
        status.success(),
        "`{binary} auth login` exited with status {status}"
    );
    eprintln!(
        "Claude Code instance '{instance_id}' is signed in. Pick a `Claude Code` model in /model or run with --provider claude-code."
    );
    crate::telemetry::record_auth_success("claude-code", "claude_code_cli");
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
    eprintln!("\nGrok Build login (xAI)");
    eprintln!("  Open: {url}");
    eprintln!("  Code: {}\n", authorization.user_code);
    maybe_open_browser(url, no_browser);
    eprintln!("Waiting for authorization...");
    crate::auth::grok_build::complete_device_login(&client, &authorization).await?;
    eprintln!("Grok Build login complete.");
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
    // Wait for the server's verdict. It used to reject this lone request
    // (no Subscribe) while login still logged it as sent.
    match client
        .notify_auth_changed_and_wait(provider, std::time::Duration::from_secs(10))
        .await
    {
        Ok(()) => crate::logging::auth_event("auth_changed_notify_applied", "server", &[]),
        Err(err) => {
            let reason = err.to_string();
            crate::logging::auth_event(
                "auth_changed_notify_failed",
                "server",
                &[("reason", reason.as_str())],
            );
            eprintln!(
                "Warning: the running jcode server did not confirm the new login ({reason}). \
                 Open sessions may keep the previous account until restarted."
            );
        }
    }
}

async fn login_jcode_flow(no_browser: bool) -> Result<()> {
    eprintln!("Starting jcode subscription sign-in...");
    let _ = jcode_device::login_jcode_device_flow(no_browser).await?;
    Ok(())
}

pub(crate) async fn run_jcode_account_login(no_browser: bool) -> Result<()> {
    login_jcode_flow(no_browser).await
}

fn login_openai_api_key_flow() -> Result<()> {
    eprintln!("Setting up OpenAI API key...");
    eprintln!("Get your API key from: https://platform.openai.com/api-keys\n");
    eprint!("Paste your OpenAI API key: ");
    io::stdout().flush()?;

    let key = read_secret_line()?;
    if key.is_empty() {
        anyhow::bail!("No API key provided.");
    }
    if !key.starts_with("sk-") {
        eprintln!("Warning: OpenAI API keys usually start with 'sk-'. Saving anyway.");
    }

    save_named_api_key("openai.env", "OPENAI_API_KEY", &key)?;
    eprintln!("\nSuccessfully saved OpenAI API key!");
    eprintln!(
        "Stored at {}",
        crate::storage::app_config_dir()?
            .join("openai.env")
            .display()
    );
    eprintln!("Provider: openai-api (native OpenAI Responses API)");
    crate::telemetry::record_auth_success("openai-api", "api_key");
    Ok(())
}

async fn login_claude_flow(requested_label: Option<&str>, no_browser: bool) -> Result<()> {
    let label = auth::claude::login_target_label(requested_label)?;
    eprintln!("Logging in to Claude (account: {})...", label);
    let tokens = auth::oauth::login_claude(no_browser).await?;
    auth::oauth::save_claude_tokens_for_account(&tokens, &label)?;
    let profile_email =
        match auth::oauth::update_claude_account_profile(&label, &tokens.access_token).await {
            Ok(email) => email,
            Err(e) => {
                eprintln!(
                    "Warning: logged in but failed to fetch profile metadata: {}",
                    e
                );
                None
            }
        };
    eprintln!("Successfully logged in to Claude!");
    eprintln!(
        "Account '{}' stored at {}",
        label,
        auth::claude::jcode_path()?.display()
    );
    if let Some(email) = profile_email {
        eprintln!("Profile email: {}", email);
    }
    crate::telemetry::record_auth_success("claude", "oauth");
    Ok(())
}

fn validate_claude_code_method(
    provider: LoginProviderDescriptor,
    account_label: Option<&str>,
    options: &LoginOptions,
) -> Result<()> {
    if !options.claude_code {
        return Ok(());
    }
    anyhow::ensure!(
        provider.target == LoginProviderTarget::Claude,
        "--claude-code is only available with --provider claude."
    );
    anyhow::ensure!(
        account_label.is_none(),
        "--claude-code cannot select a Jcode account label; Claude Code chooses its own account."
    );
    anyhow::ensure!(
        !options.no_browser && !options.json && !options.uses_scriptable_flow()?,
        "--claude-code requires an interactive terminal and cannot use scriptable or --no-browser login flags."
    );
    anyhow::ensure!(
        crate::external_auth::can_prompt_for_external_auth(),
        "--claude-code requires an interactive terminal on this machine. For SSH, run the command on the remote host."
    );
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// Test-only answer for the Claude Code reuse prompt.
    static CLAUDE_CODE_REUSE_TEST_APPROVAL: std::cell::Cell<Option<bool>> =
        const { std::cell::Cell::new(None) };
}

fn confirm_claude_code_reuse(native: bool) -> Result<()> {
    #[cfg(test)]
    if let Some(approved) = CLAUDE_CODE_REUSE_TEST_APPROVAL.with(std::cell::Cell::get) {
        anyhow::ensure!(
            approved,
            "Claude Code signed in, but Jcode was not granted access to its credentials."
        );
        return Ok(());
    }
    eprintln!(
        "\nClaude Code signed in. Jcode makes direct Anthropic requests, so it needs permission to reuse the CLI's OAuth credentials."
    );
    if native {
        eprintln!(
            "Jcode copies macOS Keychain credentials into its own store. Both tools may refresh the same token, so either login may need renewal."
        );
    } else {
        eprintln!("Jcode will read Claude Code's credentials file for future requests.");
    }
    eprint!("Allow Jcode to use this Claude Code login? [y/N]: ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    anyhow::ensure!(
        matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
        "Claude Code signed in, but Jcode was not granted access to its credentials."
    );
    Ok(())
}

fn usable_claude_code_credentials(credentials: &auth::claude::ClaudeCredentials) -> bool {
    !credentials.refresh_token.trim().is_empty()
        || (!credentials.access_token.trim().is_empty()
            && credentials.expires_at > chrono::Utc::now().timestamp_millis())
}

fn ensure_claude_code_credentials_selected(
    expected: &auth::claude::ClaudeCredentials,
) -> Result<()> {
    let selected = auth::claude::load_credentials()?;
    anyhow::ensure!(
        selected.access_token == expected.access_token
            && selected.refresh_token == expected.refresh_token,
        "Another trusted Claude credential source is taking precedence over this login. Review Jcode's trusted external auth sources before retrying."
    );
    Ok(())
}

fn reuse_claude_code_file(
    source: auth::claude::ExternalClaudeAuthSource,
    path: &Path,
) -> Result<bool> {
    let already_trusted =
        crate::config::Config::external_auth_source_allowed_for_path(source.source_id(), path);
    if !already_trusted {
        confirm_claude_code_reuse(false)?;
    }
    let credentials = match auth::claude::load_claude_code_credentials() {
        Ok(credentials) if usable_claude_code_credentials(&credentials) => credentials,
        _ => return Ok(false),
    };
    if !already_trusted {
        auth::claude::trust_external_auth_source(source)?;
    }
    if let Err(err) = ensure_claude_code_credentials_selected(&credentials) {
        // Do not leave a file trusted by a login that did not take effect: a
        // later update could otherwise make Jcode select it without approval.
        if !already_trusted {
            crate::config::Config::revoke_external_auth_source_for_path(source.source_id(), path)?;
            auth::AuthStatus::invalidate_cache();
        }
        return Err(err);
    }
    Ok(true)
}

fn reuse_claude_code_native(file_path: &Path) -> Result<bool> {
    if !auth::claude::native_credentials_present() {
        return Ok(false);
    }
    if !auth::claude::native_source_allowed() {
        confirm_claude_code_reuse(true)?;
    }
    // Reading Keychain secrets is permitted only after approval. The item may
    // exist but be locked, in which case the file source can still be tried.
    let credentials = match auth::claude::load_native_credentials() {
        Ok(credentials) if usable_claude_code_credentials(&credentials) => credentials,
        _ => return Ok(false),
    };

    reuse_claude_code_native_snapshot(file_path, credentials, || {})
}

fn configured_claude_file_is_unambiguous(
    path: &Path,
    existed_before_login: bool,
    modified_before_login: Option<std::time::SystemTime>,
    native_present: bool,
) -> Result<()> {
    if existed_before_login && native_present {
        let modified_after_login = std::fs::metadata(path)
            .and_then(|file| file.modified())
            .ok();
        anyhow::ensure!(
            modified_before_login
                .zip(modified_after_login)
                .is_some_and(|(before, after)| before != after),
            "Claude Code did not update the configured credentials file, while native credentials also exist. Jcode cannot tell which account just signed in. Refresh or remove the stale file, or use Jcode's OAuth login."
        );
    }
    Ok(())
}

fn login_claude_code_flow() -> Result<()> {
    let source = auth::claude::ExternalClaudeAuthSource::ClaudeCode;
    let file_path = source.path()?;
    let configured_file = std::env::var_os("CLAUDE_CONFIG_DIR").is_some_and(|dir| !dir.is_empty());
    let file_existed_before_login = file_path.exists();
    let file_modified_before_login = std::fs::metadata(&file_path)
        .and_then(|file| file.modified())
        .ok();
    eprintln!("Signing in with Claude Code on this machine (`claude auth login`)…");
    let status = std::process::Command::new("claude")
        .args(["auth", "login"])
        .status()
        .context("Failed to start command: claude auth login")?;
    anyhow::ensure!(
        status.success(),
        "Command exited with non-zero status: claude auth login ({status})"
    );

    // Jcode makes direct Anthropic requests, so a CLI login by itself is not a
    // Jcode login. Keep the existing external-source consent boundary intact.
    let mut reused = false;
    if configured_file && file_path.exists() {
        configured_claude_file_is_unambiguous(
            &file_path,
            file_existed_before_login,
            file_modified_before_login,
            auth::claude::native_credentials_present(),
        )?;
        reused = reuse_claude_code_file(source, &file_path)?;
    }
    if !reused {
        reused = reuse_claude_code_native(&file_path)?;
    }
    if !reused && !configured_file && file_path.exists() {
        reused = reuse_claude_code_file(source, &file_path)?;
    }
    anyhow::ensure!(
        reused,
        "Claude Code signed in, but no reusable OAuth credentials were found. Check `claude auth status`, or use `jcode login --provider claude` for Jcode's OAuth flow."
    );
    eprintln!("Claude Code login is now available to Jcode.");
    crate::telemetry::record_auth_success("claude", "claude_code_cli");
    Ok(())
}

fn login_anthropic_api_key_flow() -> Result<()> {
    eprintln!("Setting up Anthropic API...");
    eprintln!("Get your API key from: https://console.anthropic.com/settings/keys\n");
    eprint!("Paste your Anthropic API key: ");
    io::stdout().flush()?;

    let key = read_secret_line()?;

    if key.is_empty() {
        anyhow::bail!("No API key provided.");
    }

    if !key.starts_with("sk-ant-") {
        eprintln!("Warning: Anthropic API keys typically start with 'sk-ant-'. Saving anyway.");
    }

    save_named_api_key("anthropic.env", "ANTHROPIC_API_KEY", &key)?;
    eprintln!("\nSuccessfully saved Anthropic API key!");
    eprintln!(
        "Stored at {}",
        crate::storage::app_config_dir()?
            .join("anthropic.env")
            .display()
    );
    eprintln!("Provider: claude (native Anthropic Messages API)");
    crate::telemetry::record_auth_success("anthropic-api", "api_key");
    Ok(())
}

async fn login_openai_flow(requested_label: Option<&str>, no_browser: bool) -> Result<()> {
    let label = auth::codex::login_target_label(requested_label)?;
    eprintln!("Logging in to OpenAI/Codex (account: {})...", label);
    let tokens = auth::oauth::login_openai(no_browser).await?;
    auth::oauth::save_openai_tokens_for_account(&tokens, &label)?;
    eprintln!(
        "Successfully logged in to OpenAI! Account '{}' saved to {}",
        label,
        crate::storage::jcode_dir()?
            .join("openai-auth.json")
            .display()
    );
    crate::telemetry::record_auth_success("openai", "oauth");
    Ok(())
}

fn login_openrouter_flow() -> Result<()> {
    eprintln!("Setting up OpenRouter...");
    eprintln!("Get your API key from: https://openrouter.ai/keys\n");
    eprint!("Paste your OpenRouter API key: ");
    io::stdout().flush()?;

    let key = read_secret_line()?;

    if key.is_empty() {
        anyhow::bail!("No API key provided.");
    }

    if !key.starts_with("sk-or-") {
        eprintln!("Warning: OpenRouter API keys typically start with 'sk-or-'. Saving anyway.");
    }

    save_named_api_key("openrouter.env", "OPENROUTER_API_KEY", &key)?;
    eprintln!("\nSuccessfully saved OpenRouter API key!");
    eprintln!(
        "Stored at {}",
        crate::storage::app_config_dir()?
            .join("openrouter.env")
            .display()
    );
    crate::telemetry::record_auth_success("openrouter", "api_key");
    Ok(())
}

fn login_bedrock_flow() -> Result<()> {
    eprintln!("Setting up AWS Bedrock...");
    eprintln!(
        "Generate a Bedrock API key in the AWS Bedrock console: https://console.aws.amazon.com/bedrock/home#/api-keys"
    );
    eprintln!("Short-term keys are recommended for onboarding/testing.\n");

    let region = read_line_trimmed("AWS region [us-east-2]: ")?;
    let region = if region.trim().is_empty() {
        "us-east-2".to_string()
    } else {
        region.trim().to_string()
    };

    eprint!("Paste your Bedrock API key: ");
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

    eprintln!("\nSuccessfully saved AWS Bedrock API key!");
    eprintln!(
        "Stored at {}",
        crate::storage::app_config_dir()?
            .join(crate::provider::bedrock::ENV_FILE)
            .display()
    );
    eprintln!("Region: {}", region);
    eprintln!("Provider: bedrock (native AWS Bedrock Converse API)");
    crate::telemetry::record_auth_success("bedrock", "api_key");
    Ok(())
}

fn login_azure_flow() -> Result<()> {
    use crate::auth::azure;

    eprintln!("Setting up Azure OpenAI...");
    eprintln!(
        "Reference: OpenCode supports Azure OpenAI with Entra credentials. jcode uses Azure OpenAI's newer `/openai/v1` API with either Microsoft Entra ID or an API key.\n"
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

    eprintln!("\nAuthentication method:");
    eprintln!("  1. Microsoft Entra ID (recommended)");
    eprintln!("  2. API key");
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
        eprintln!();
        eprintln!("Using Microsoft Entra ID via Azure's DefaultAzureCredential chain.");
        eprintln!(
            "That means jcode can authenticate via `az login`, managed identity, or Azure environment credentials."
        );
    } else {
        eprint!("Paste your Azure OpenAI API key: ");
        io::stdout().flush()?;
        let key = read_secret_line()?;
        if key.is_empty() {
            anyhow::bail!("No API key provided.");
        }
        assignments.push((azure::API_KEY_ENV, key));
    }

    save_named_env_vars(azure::ENV_FILE, &assignments)?;
    azure::apply_runtime_env()?;

    eprintln!("\nSuccessfully saved Azure OpenAI configuration!");
    eprintln!(
        "Stored at {}",
        crate::storage::app_config_dir()?
            .join(azure::ENV_FILE)
            .display()
    );
    eprintln!("Base URL: {}", azure::load_endpoint().unwrap_or_default());
    if let Some(model) = azure::load_model() {
        eprintln!("Default deployment/model: {}", model);
    }
    if use_entra {
        eprintln!(
            "Next step: if you're using Azure CLI auth, run `az login` (and ensure your identity has the Cognitive Services OpenAI User role)."
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

    eprintln!("Setting up {}...", resolved.display_name);
    let setup_url_depends_on_key = profile.id == crate::provider_catalog::MINIMAX_PROFILE.id;
    if !setup_url_depends_on_key {
        eprintln!("See setup details: {}\n", resolved.setup_url);
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
        eprintln!(
            "You can point this at a hosted OpenAI-compatible API or a local server such as LM Studio or Ollama."
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
        eprintln!();
    } else if let Some(model) = options
        .openai_compatible_default_model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        resolved.default_model = Some(model.to_string());
    }

    let auth_method = if resolved.requires_api_key {
        eprintln!("API key env variable: {}\n", resolved.api_key_env);
        if options.openai_compatible_api_key.is_none() {
            existing_key_notice::announce_existing_api_key(&resolved);
        }
        let key = match options.openai_compatible_api_key.as_deref() {
            Some(value) => value.trim().to_string(),
            None => {
                eprint!("Paste your {} API key: ", resolved.display_name);
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
        eprintln!("Endpoint: {}", resolved.api_base);
        if setup_url_depends_on_key {
            eprintln!("See setup details: {}", resolved.setup_url);
        }

        crate::provider_catalog::save_env_value_to_env_file(
            OPENAI_COMPAT_LOCAL_ENABLED_ENV,
            &resolved.env_file,
            None,
        )?;
        save_named_api_key(&resolved.env_file, &resolved.api_key_env, &key)?;
        eprintln!("\nSuccessfully saved {} API key!", resolved.display_name);
        "api_key"
    } else {
        eprintln!("Endpoint: {}", resolved.api_base);
        if setup_url_depends_on_key {
            eprintln!("See setup details: {}", resolved.setup_url);
        }
        eprintln!("This provider uses a local OpenAI-compatible endpoint.");
        eprintln!(
            "An API key is optional here. Press Enter to skip if your local server does not require one.\n"
        );
        let key = match options.openai_compatible_api_key.as_deref() {
            Some(value) => value.trim().to_string(),
            None => {
                eprint!("Optional {} API key: ", resolved.display_name);
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
            eprintln!("\nSaved {} local endpoint setup.", resolved.display_name);
            "local_endpoint"
        } else {
            crate::provider_catalog::save_named_api_key(
                &resolved.env_file,
                &resolved.api_key_env,
                key.trim(),
            )?;
            eprintln!(
                "\nSaved {} local endpoint setup and optional API key.",
                resolved.display_name
            );
            "local_endpoint_with_optional_api_key"
        }
    };

    if !resolved.requires_api_key && resolved.default_model.is_none() {
        eprintln!("{}", next_step::local_endpoint_hint(&resolved.id));
    }

    eprintln!(
        "Stored at {}",
        crate::storage::app_config_dir()?
            .join(&resolved.env_file)
            .display()
    );
    if let Some(default_model) = resolved.default_model {
        eprintln!("Default model hint: {}", default_model);
    }
    crate::telemetry::record_auth_success(&resolved.id, auth_method);
    Ok(())
}

pub use jcode_terminal_prompt::read_secret_line;

mod presenter;
pub use presenter::install_terminal_login_presenter;
use presenter::read_line_trimmed;

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
    eprintln!("Starting Cursor API key setup...");

    eprintln!("Get your API key from: https://cursor.com/settings");
    eprintln!("(Dashboard > Integrations > User API Keys)\n");
    eprint!("Paste your Cursor API key: ");
    io::stdout().flush()?;

    let key = read_secret_line()?;
    if key.is_empty() {
        anyhow::bail!("No API key provided.");
    }

    save_named_api_key("cursor.env", "CURSOR_API_KEY", &key)?;
    crate::auth::AuthStatus::invalidate_cache();
    eprintln!("\nSuccessfully saved Cursor API key!");
    eprintln!(
        "Stored at {}",
        crate::storage::app_config_dir()?
            .join("cursor.env")
            .display()
    );
    eprintln!("jcode will use the native Cursor HTTPS transport.");
    crate::telemetry::record_auth_success("cursor", "api_key");
    Ok(())
}

fn login_copilot_flow(no_browser: bool) -> Result<()> {
    eprintln!("Starting GitHub Copilot login...");

    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(login_copilot_device_flow(no_browser))
    })
}

async fn login_copilot_device_flow(no_browser: bool) -> Result<()> {
    let client = crate::provider::shared_http_client();

    let device_resp = crate::auth::copilot::initiate_device_flow(&client).await?;

    eprintln!();
    eprintln!("  Open this URL in your browser:");
    eprintln!("    {}", device_resp.verification_uri);
    eprintln!();
    if let Some(qr) = jcode_terminal_prompt::qr::indented_section(
        &device_resp.verification_uri,
        "  Or scan this QR on another device to open the verification page:",
        "    ",
        crate::auth::browser_suppressed(no_browser),
    ) {
        eprintln!("{qr}");
        eprintln!();
    }
    eprintln!("  Enter code: {}", device_resp.user_code);
    eprintln!();
    eprintln!("  Waiting for authorization...");

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

    eprintln!("  ✓ Authenticated as {} via GitHub Copilot", username);
    crate::telemetry::record_auth_success("copilot", "oauth_device_code");
    Ok(())
}

async fn login_antigravity_flow(no_browser: bool) -> Result<()> {
    eprintln!("Starting native Antigravity login...");
    eprintln!(
        "jcode will authenticate directly with Google Antigravity; the Antigravity desktop app is not required."
    );
    eprintln!(
        "If browser launch fails, or you pass `--no-browser`, jcode will prompt for the callback URL instead."
    );
    eprintln!(
        "If the browser later shows a loopback/callback error page, copy the full URL from the address bar and re-run with `--no-browser`."
    );
    eprintln!();

    let tokens = crate::auth::antigravity::login(no_browser).await?;

    eprintln!("Successfully logged in to Antigravity!");
    eprintln!(
        "Tokens saved to {}",
        crate::auth::antigravity::tokens_path()?.display()
    );
    if let Some(email) = tokens.email.as_deref() {
        eprintln!("Google account: {}", email);
    }
    if let Some(project_id) = tokens.project_id.as_deref() {
        eprintln!("Resolved Antigravity project: {}", project_id);
    }
    crate::telemetry::record_auth_success("antigravity", "oauth");
    Ok(())
}

async fn login_gemini_flow(no_browser: bool) -> Result<()> {
    // Offer the auth-method choice only on an interactive terminal so scripted
    // / piped invocations preserve the historical OAuth-only behavior.
    if io::stdin().is_terminal() {
        eprintln!("Gemini login. Choose an authentication method:");
        eprintln!("  [1] Google account OAuth (free Code Assist tier, default)");
        eprintln!(
            "  [2] Gemini Developer API key (Google AI Studio, generativelanguage.googleapis.com)"
        );
        eprintln!();
        let choice = read_line_trimmed("Enter 1-2 [1]: ")?;
        if choice == "2" {
            return login_gemini_api_key_flow();
        }
    }

    eprintln!("Starting native Gemini login...");
    eprintln!(
        "If your student/education plan is attached to your Google account, use that account in the browser flow."
    );
    eprintln!(
        "If browser launch fails, or you pass `--no-browser`, jcode will prompt for the manual authorization code."
    );
    eprintln!(
        "Note: school / Workspace Google accounts may also require GOOGLE_CLOUD_PROJECT and GOOGLE_CLOUD_LOCATION for Code Assist entitlement checks."
    );
    eprintln!();

    let tokens = crate::auth::gemini::login(no_browser).await?;

    eprintln!("Successfully logged in to Gemini!");
    eprintln!(
        "Tokens saved to {}",
        crate::auth::gemini::tokens_path()?.display()
    );
    if let Some(email) = tokens.email.as_deref() {
        eprintln!("Google account: {}", email);
    }
    crate::telemetry::record_auth_success("gemini", "oauth");
    Ok(())
}

fn login_gemini_api_key_flow() -> Result<()> {
    eprintln!("Setting up Gemini Developer API key...");
    eprintln!("Get your API key from: https://aistudio.google.com/apikey\n");
    eprint!("Paste your Gemini API key: ");
    io::stdout().flush()?;

    let key = read_secret_line()?;
    if key.is_empty() {
        anyhow::bail!("No API key provided.");
    }

    crate::auth::gemini::save_api_key(&key)?;
    eprintln!("\nSuccessfully saved Gemini Developer API key!");
    eprintln!(
        "Stored at {}",
        crate::storage::app_config_dir()?
            .join(crate::auth::gemini::GEMINI_API_KEY_ENV_FILE)
            .display()
    );
    eprintln!(
        "Provider: gemini (official Gemini Developer API, generativelanguage.googleapis.com)"
    );
    crate::telemetry::record_auth_success("gemini", "api_key");
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

#[cfg(test)]
#[path = "login/native_stage_tests.rs"]
mod native_stage_tests;
