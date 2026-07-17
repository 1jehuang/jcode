#[path = "auth_account_commands.rs"]
mod auth_account_commands;
#[path = "auth_account_picker.rs"]
mod auth_account_picker;
#[path = "auth_types.rs"]
mod auth_types;
pub(crate) use self::auth_account_commands::{
    account_command_from_picker, execute_account_command_local, execute_account_command_remote,
    handle_account_command_remote, handle_auth_command, resolve_account_provider_descriptor,
    save_openai_fast_setting_local,
};
pub(super) use self::auth_types::{AccountCommand, PendingAccountInput, PendingLogin};

use super::*;
use crossterm::event::{KeyCode, KeyModifiers};
use std::sync::Arc;

impl App {
    fn open_auth_browser(url: &str) -> bool {
        // Honors --no-browser/NO_BROWSER/JCODE_NO_BROWSER and never opens real
        // browser windows from test binaries (login flows are exercised by TUI
        // tests; without this guard a test run pops OAuth pages on the
        // developer's desktop).
        super::helpers::open_path_or_url_detached(url).is_ok()
    }

    fn record_oauth_preflight(
        provider_id: &str,
        browser_opened: bool,
        callback_target: Option<&str>,
        callback_available: Option<bool>,
    ) -> String {
        let mut notices = Vec::new();
        if !browser_opened {
            crate::telemetry::record_auth_surface_blocked_reason(
                provider_id,
                "oauth",
                crate::auth::login_diagnostics::AuthFailureReason::BrowserOpenFailed.label(),
            );
            notices.push("This machine could not open a browser automatically.".to_string());
        }
        if matches!(callback_available, Some(false)) {
            crate::telemetry::record_auth_surface_blocked_reason(
                provider_id,
                "oauth",
                crate::auth::login_diagnostics::AuthFailureReason::CallbackPortUnavailable.label(),
            );
            if let Some(target) = callback_target {
                notices.push(format!(
                    "Local callback target {} is unavailable, so jcode is using manual-safe paste completion instead.",
                    target
                ));
            } else {
                notices.push(
                    "The local callback listener is unavailable, so jcode is using manual-safe paste completion instead."
                        .to_string(),
                );
            }
        }
        if !notices.is_empty() {
            notices.push(format!(
                "If login still fails, run jcode auth doctor {} for a guided diagnosis.",
                provider_id
            ));
        }
        notices.join("\n")
    }

    pub(super) fn show_jcode_subscription_status(&mut self) {
        let configured_key = crate::subscription_catalog::configured_api_key().is_some();
        let configured_base = crate::subscription_catalog::configured_api_base()
            .unwrap_or_else(|| crate::subscription_catalog::DEFAULT_JCODE_API_BASE.to_string());
        let runtime_mode = crate::subscription_catalog::is_runtime_mode_enabled();
        let cached_tier = crate::subscription_catalog::cached_tier();

        let mut message = String::from("Jcode Subscription Status\n\n");
        message.push_str(&format!(
            "  - Credentials: {}\n",
            if configured_key {
                "configured"
            } else {
                "not configured (/login jcode)"
            }
        ));
        message.push_str(&format!(
            "  - Router base: {}{}\n",
            configured_base,
            if crate::subscription_catalog::has_router_base() {
                ""
            } else {
                " (default)"
            }
        ));
        message.push_str(&format!(
            "  - Tier: {}\n",
            cached_tier
                .map(|tier| tier.display_name().to_string())
                .unwrap_or_else(|| "unknown (treated as Plus)".to_string())
        ));
        message.push_str(&format!(
            "  - Runtime mode: {}\n\n",
            if runtime_mode {
                "active for this session"
            } else {
                "inactive for this session"
            }
        ));

        message.push_str("Catalog\n\n");
        for model in crate::subscription_catalog::curated_models() {
            let default_suffix = if model.default_enabled {
                " (default)"
            } else {
                ""
            };
            let tier_suffix = if model.min_tier == crate::subscription_catalog::JcodeTier::Plus {
                String::new()
            } else {
                format!(" [{}]", model.min_tier.display_name())
            };
            message.push_str(&format!(
                "  - {} - {}{}{}\n      - {}\n      - {}\n",
                model.display_name,
                model.id,
                default_suffix,
                tier_suffix,
                crate::subscription_catalog::routing_policy_detail(model),
                model.note
            ));
        }

        message.push_str("\nTiers\n\n");
        for tier in crate::subscription_catalog::JcodeTier::ALL.iter().copied() {
            message.push_str(&format!(
                "  - {} - ${}/mo retail, about ${:.2} usable inference budget\n",
                tier.display_name(),
                tier.retail_price_usd(),
                tier.usable_budget_usd()
            ));
        }

        if configured_key {
            message.push_str("\nFetching account status...");
        } else {
            message.push_str("\nLog in with /login jcode to see account usage and tier.");
        }

        self.push_display_message(DisplayMessage::system(message));

        // With credentials present, fetch live account status (/v1/me) in the
        // background and surface it via a UiActivity card. Short timeout keeps
        // this responsive; offline failures degrade to a quiet log line.
        if configured_key {
            let session_id = self.session.id.clone();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    match crate::subscription_api::fetch_subscription_me().await {
                        Ok(me) => {
                            let tier_label = me
                                .parsed_tier()
                                .map(|tier| tier.display_name().to_string())
                                .unwrap_or_else(|| me.tier.clone());
                            let resets = me
                                .usage
                                .resets_at
                                .as_deref()
                                .map(|at| format!(", resets {}", at))
                                .unwrap_or_default();
                            crate::bus::Bus::global().publish(crate::bus::BusEvent::UiActivity(
                                crate::bus::UiActivity::background(
                                    Some(session_id),
                                    format!(
                                        "Jcode Subscription Account\n\n  - Email: {}\n  - Tier: {} ({})\n  - Usage: ${:.2} of ${:.2}{}",
                                        me.email,
                                        tier_label,
                                        me.status,
                                        me.usage.used_usd,
                                        me.usage.budget_usd,
                                        resets
                                    ),
                                    Some("Subscription: account status loaded"),
                                ),
                            ));
                        }
                        Err(error) => {
                            let message = if error
                                .downcast_ref::<crate::subscription_api::AccountApiError>()
                                == Some(&crate::subscription_api::AccountApiError::Unauthorized)
                            {
                                let _ = crate::subscription_catalog::clear_account_credentials();
                                "Jcode Account Status\n\nThe saved account key was revoked or expired. Local credentials were cleared. Use /account jcode login to sign in again."
                                    .to_string()
                            } else {
                                format!(
                                    "Jcode Account Status\n\nCould not load /v1/me: {}\n\nThe local credential was retained. Retry /account jcode status, open /account jcode manage, or use /account jcode logout.",
                                    error
                                )
                            };
                            crate::bus::Bus::global().publish(crate::bus::BusEvent::UiActivity(
                                crate::bus::UiActivity::background(
                                    Some(session_id),
                                    message,
                                    Some("Jcode account status unavailable"),
                                ),
                            ));
                        }
                    }
                });
            }
        }
    }

    pub(super) fn show_auth_status(&mut self) {
        let status = crate::auth::AuthStatus::check();
        let validation = crate::auth::validation::load_all();
        let icon = |state: crate::auth::AuthState| match state {
            crate::auth::AuthState::Available => "ok",
            crate::auth::AuthState::Expired => "needs attention",
            crate::auth::AuthState::NotConfigured => "not configured",
        };
        let providers = crate::provider_catalog::auth_status_login_providers();
        let mut rows: Vec<[String; 5]> = vec![[
            "Provider".to_string(),
            "Status".to_string(),
            "Method".to_string(),
            "Health".to_string(),
            "Validation".to_string(),
        ]];
        for provider in providers {
            let assessment = status.assessment_for_provider(provider);
            rows.push([
                provider.display_name.to_string(),
                icon(assessment.state).to_string(),
                assessment.method_detail.to_string(),
                assessment.health_summary(),
                validation
                    .get(provider.id)
                    .map(crate::auth::validation::format_record_label)
                    .unwrap_or_else(|| "not validated".to_string()),
            ]);
        }
        let mut widths = [0usize; 5];
        for row in &rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
        let mut message = String::from("Authentication Status:\n\n");
        for row in &rows {
            let line = row
                .iter()
                .enumerate()
                .map(|(i, cell)| format!("{:width$}", cell, width = widths[i]))
                .collect::<Vec<_>>()
                .join("  ");
            message.push_str(line.trim_end());
            message.push('\n');
        }
        message.push_str(
            "\nUse /login <provider> to authenticate. /login jcode is for curated jcode subscription access; /account opens the provider/account management center, /account <provider> settings shows provider-specific controls, and /auth doctor or /account <provider> doctor shows recovery steps.",
        );
        self.push_display_message(DisplayMessage::system(message));
    }

    pub(super) fn show_interactive_login(&mut self) {
        crate::telemetry::record_setup_step_once("login_picker_opened");
        self.open_login_picker_inline();
        self.set_status_notice("Login: choose a provider");
    }

    pub(super) fn show_interactive_logout(&mut self) {
        self.open_logout_picker_inline();
        self.set_status_notice("Logout: choose a provider");
    }

    pub(super) fn start_logout_provider(
        &mut self,
        provider: crate::provider_catalog::LoginProviderDescriptor,
    ) {
        use crate::provider_catalog::LoginProviderTarget;

        if matches!(provider.target, LoginProviderTarget::Jcode) {
            self.start_jcode_account_logout();
            return;
        }

        let result: anyhow::Result<String> = (|| match provider.target {
            LoginProviderTarget::Jcode => unreachable!("handled above"),
            LoginProviderTarget::Claude => {
                let removed = crate::auth::claude::clear_accounts()?;
                Ok(format!("Logged out of {} Anthropic account(s).", removed))
            }
            LoginProviderTarget::ClaudeApiKey => {
                Self::clear_api_key_login("ANTHROPIC_API_KEY", "anthropic.env")?;
                Ok("Logged out of Anthropic API key.".to_string())
            }
            LoginProviderTarget::OpenAi => {
                let removed = crate::auth::codex::clear_accounts()?;
                Ok(format!("Logged out of {} OpenAI account(s).", removed))
            }
            LoginProviderTarget::OpenAiApiKey => {
                Self::clear_api_key_login("OPENAI_API_KEY", "openai.env")?;
                Ok("Logged out of OpenAI API key.".to_string())
            }
            LoginProviderTarget::OpenRouter => {
                Self::clear_api_key_login("OPENROUTER_API_KEY", "openrouter.env")?;
                Ok("Logged out of OpenRouter API key.".to_string())
            }
            LoginProviderTarget::Bedrock => {
                Self::clear_api_key_login(
                    crate::provider::bedrock::API_KEY_ENV,
                    crate::provider::bedrock::ENV_FILE,
                )?;
                Ok("Logged out of Bedrock API key.".to_string())
            }
            LoginProviderTarget::Azure => {
                Self::clear_api_key_login(
                    crate::auth::azure::API_KEY_ENV,
                    crate::auth::azure::ENV_FILE,
                )?;
                crate::provider_catalog::save_env_value_to_env_file(
                    crate::auth::azure::USE_ENTRA_ENV,
                    crate::auth::azure::ENV_FILE,
                    None,
                )?;
                Ok("Logged out of Azure OpenAI API key / Entra configuration.".to_string())
            }
            LoginProviderTarget::OpenAiCompatible(profile) => {
                let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
                Self::clear_api_key_login(&resolved.api_key_env, &resolved.env_file)?;
                crate::provider_catalog::save_env_value_to_env_file(
                    crate::provider_catalog::OPENAI_COMPAT_LOCAL_ENABLED_ENV,
                    &resolved.env_file,
                    None,
                )?;
                Ok(format!("Logged out of {} API key.", resolved.display_name))
            }
            LoginProviderTarget::Cursor => {
                crate::auth::cursor::clear_api_key()?;
                Ok("Logged out of Cursor API key.".to_string())
            }
            LoginProviderTarget::Gemini => {
                crate::auth::gemini::clear_tokens()?;
                Ok("Logged out of Gemini.".to_string())
            }
            _ => Ok(format!(
                "Logout for {} is not automated yet. Remove its saved API key or external CLI session from /account {} settings.",
                provider.display_name, provider.id
            )),
        })();

        match result {
            Ok(message) => {
                crate::auth::AuthStatus::invalidate_cache();
                self.push_display_message(DisplayMessage::system(message));
                self.set_status_notice(format!("Logout: {}", provider.display_name));
            }
            Err(err) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to log out of {}: {}",
                    provider.display_name, err
                )));
                self.set_status_notice("Logout failed");
            }
        }
    }

    pub(super) fn start_logout_all(&mut self) {
        let mut summary: Vec<String> = Vec::new();
        let mut errors: Vec<String> = Vec::new();

        match crate::auth::claude::clear_accounts() {
            Ok(removed) if removed > 0 => summary.push(format!("{} Anthropic account(s)", removed)),
            Ok(_) => {}
            Err(err) => errors.push(format!("Anthropic: {}", err)),
        }
        match crate::auth::codex::clear_accounts() {
            Ok(removed) if removed > 0 => summary.push(format!("{} OpenAI account(s)", removed)),
            Ok(_) => {}
            Err(err) => errors.push(format!("OpenAI: {}", err)),
        }

        Self::clear_api_key_logout_summary(
            &mut summary,
            &mut errors,
            "jcode subscription API key",
            crate::subscription_catalog::JCODE_API_KEY_ENV,
            crate::subscription_catalog::JCODE_ENV_FILE,
        );
        for env_key in [
            crate::subscription_catalog::JCODE_API_BASE_ENV,
            crate::subscription_catalog::JCODE_ACCOUNT_ID_ENV,
            crate::subscription_catalog::JCODE_ACCOUNT_EMAIL_ENV,
            crate::subscription_catalog::JCODE_TIER_ENV,
        ] {
            if let Err(err) = crate::provider_catalog::save_env_value_to_env_file(
                env_key,
                crate::subscription_catalog::JCODE_ENV_FILE,
                None,
            ) {
                errors.push(format!("jcode subscription {}: {}", env_key, err));
            }
        }

        Self::clear_api_key_logout_summary(
            &mut summary,
            &mut errors,
            "Anthropic API key",
            "ANTHROPIC_API_KEY",
            "anthropic.env",
        );
        Self::clear_api_key_logout_summary(
            &mut summary,
            &mut errors,
            "OpenAI API key",
            "OPENAI_API_KEY",
            "openai.env",
        );
        Self::clear_api_key_logout_summary(
            &mut summary,
            &mut errors,
            "OpenRouter API key",
            "OPENROUTER_API_KEY",
            "openrouter.env",
        );
        Self::clear_api_key_logout_summary(
            &mut summary,
            &mut errors,
            "Bedrock API key",
            crate::provider::bedrock::API_KEY_ENV,
            crate::provider::bedrock::ENV_FILE,
        );
        Self::clear_api_key_logout_summary(
            &mut summary,
            &mut errors,
            "Azure OpenAI API key",
            crate::auth::azure::API_KEY_ENV,
            crate::auth::azure::ENV_FILE,
        );
        if let Err(err) = crate::provider_catalog::save_env_value_to_env_file(
            crate::auth::azure::USE_ENTRA_ENV,
            crate::auth::azure::ENV_FILE,
            None,
        ) {
            errors.push(format!("Azure OpenAI Entra config: {}", err));
        }
        for profile in crate::provider_catalog::openai_compatible_profiles() {
            let resolved = crate::provider_catalog::resolve_openai_compatible_profile(*profile);
            Self::clear_api_key_logout_summary(
                &mut summary,
                &mut errors,
                &format!("{} API key", resolved.display_name),
                &resolved.api_key_env,
                &resolved.env_file,
            );
            if let Err(err) = crate::provider_catalog::save_env_value_to_env_file(
                crate::provider_catalog::OPENAI_COMPAT_LOCAL_ENABLED_ENV,
                &resolved.env_file,
                None,
            ) {
                errors.push(format!(
                    "{} local endpoint config: {}",
                    resolved.display_name, err
                ));
            }
        }
        let cursor_configured = crate::auth::cursor::load_api_key().is_ok();
        match crate::auth::cursor::clear_api_key() {
            Ok(()) if cursor_configured => summary.push("Cursor API key".to_string()),
            Ok(()) => {}
            Err(err) => errors.push(format!("Cursor API key: {}", err)),
        }
        match crate::auth::gemini::clear_tokens() {
            Ok(()) => summary.push("Gemini".to_string()),
            Err(err) => errors.push(format!("Gemini: {}", err)),
        }

        crate::auth::AuthStatus::invalidate_cache();

        let message = if summary.is_empty() {
            "No automated logins to clear.".to_string()
        } else {
            format!("Logged out of: {}.", summary.join(", "))
        };
        self.push_display_message(DisplayMessage::system(message));

        if errors.is_empty() {
            self.set_status_notice("Logout: all providers");
        } else {
            self.push_display_message(DisplayMessage::error(format!(
                "Some logouts failed: {}",
                errors.join("; ")
            )));
            self.set_status_notice("Logout: completed with errors");
        }
    }

    fn clear_api_key_login(env_key: &str, env_file: &str) -> anyhow::Result<()> {
        crate::provider_catalog::save_env_value_to_env_file(env_key, env_file, None)
    }

    fn clear_api_key_logout_summary(
        summary: &mut Vec<String>,
        errors: &mut Vec<String>,
        label: &str,
        env_key: &str,
        env_file: &str,
    ) {
        let configured =
            crate::provider_catalog::load_env_value_from_env_or_config(env_key, env_file).is_some();
        match Self::clear_api_key_login(env_key, env_file) {
            Ok(()) if configured => summary.push(label.to_string()),
            Ok(()) => {}
            Err(err) => errors.push(format!("{}: {}", label, err)),
        }
    }

    pub(super) fn start_login_provider(
        &mut self,
        provider: crate::provider_catalog::LoginProviderDescriptor,
    ) {
        crate::telemetry::record_provider_selected(provider.id);
        crate::logging::event_info(
            "login_started",
            vec![
                ("provider_id", provider.id.to_string()),
                ("auth_kind", provider.auth_kind.label().to_string()),
            ],
        );
        match provider.target {
            crate::provider_catalog::LoginProviderTarget::AutoImport => {
                match crate::external_auth::pending_external_auth_review_candidates() {
                    Ok(candidates) if candidates.is_empty() => {
                        self.push_display_message(DisplayMessage::system(
                            "No importable external logins were found.".to_string(),
                        ));
                        self.set_status_notice("Login: no external imports found");
                    }
                    Ok(candidates) => {
                        self.push_display_message(DisplayMessage::system(
                            crate::external_auth::format_external_auth_review_candidates_markdown(
                                &candidates,
                            ),
                        ));
                        self.set_status_notice("Login: choose sources to import");
                        self.pending_login = Some(PendingLogin::AutoImportSelection { candidates });
                    }
                    Err(err) => {
                        self.push_display_message(DisplayMessage::error(format!(
                            "Failed to inspect external login sources: {}",
                            err
                        )));
                        self.set_status_notice("Login: auto import failed");
                    }
                }
            }
            crate::provider_catalog::LoginProviderTarget::Jcode => self.start_jcode_login(),
            crate::provider_catalog::LoginProviderTarget::Claude => self.start_claude_login(),
            crate::provider_catalog::LoginProviderTarget::ClaudeApiKey => {
                self.start_anthropic_api_key_login()
            }
            crate::provider_catalog::LoginProviderTarget::OpenAi => self.start_openai_login(),
            crate::provider_catalog::LoginProviderTarget::OpenAiApiKey => {
                self.start_openai_api_key_login()
            }
            crate::provider_catalog::LoginProviderTarget::OpenRouter => {
                self.start_openrouter_login()
            }
            crate::provider_catalog::LoginProviderTarget::Bedrock => self.start_bedrock_login(),
            crate::provider_catalog::LoginProviderTarget::Azure => self.start_azure_login(),
            crate::provider_catalog::LoginProviderTarget::OpenAiCompatible(profile) => {
                self.start_openai_compatible_profile_login(profile)
            }
            crate::provider_catalog::LoginProviderTarget::Cursor => self.start_cursor_login(),
            crate::provider_catalog::LoginProviderTarget::Copilot => self.start_copilot_login(),
            crate::provider_catalog::LoginProviderTarget::Gemini => self.start_gemini_login(),
            crate::provider_catalog::LoginProviderTarget::Antigravity => {
                self.start_antigravity_login()
            }
            crate::provider_catalog::LoginProviderTarget::Google => {
                crate::telemetry::record_auth_surface_blocked(
                    provider.id,
                    provider.auth_kind.label(),
                );
                self.push_display_message(DisplayMessage::error(
                    "Google/Gmail login is only available from the CLI right now. Run jcode login --provider google."
                        .to_string(),
                ));
            }
        }
    }

    fn begin_pending_login(&mut self, pending: PendingLogin) {
        if let Some((provider, method)) = pending.telemetry_context() {
            crate::telemetry::record_auth_started(&provider, &method);
        }
        self.pending_login = Some(pending);
    }

    fn start_claude_login(&mut self) {
        let label = crate::auth::claude::login_target_label(None)
            .unwrap_or_else(|_| crate::auth::claude::primary_account_label());
        self.start_claude_login_for_account(&label);
    }

    fn start_jcode_login(&mut self) {
        self.push_display_message(DisplayMessage::system(
            "Jcode Account Login\n\nRequesting a secure browser approval flow. No email or API key will be requested in the terminal."
                .to_string(),
        ));
        self.set_status_notice("Jcode account: requesting browser approval");
        let session_id = self.session.id.clone();
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            self.push_display_message(DisplayMessage::error(
                "Jcode account login requires the async runtime.".to_string(),
            ));
            return;
        };
        handle.spawn(async move {
            use crate::subscription_api::{
                ActivationOutcome, PollingBackoff, TokenPollOutcome,
            };
            use std::time::Duration;

            let publish = |message: String, status: &'static str| {
                crate::bus::Bus::global().publish(crate::bus::BusEvent::UiActivity(
                    crate::bus::UiActivity::background(
                        Some(session_id.clone()),
                        message,
                        Some(status),
                    ),
                ));
            };
            let client = crate::provider::shared_http_client();
            let api_base = crate::subscription_api::configured_api_base();
            let device = match crate::subscription_api::request_device_authorization(
                &client,
                &api_base,
                Some(crate::subscription_catalog::JcodeTier::Pro),
            )
            .await
            {
                Ok(device) => device,
                Err(error) => {
                    publish(
                        format!(
                            "Jcode Account Login\n\nCould not start browser approval: {}\n\nRetry /account jcode login. No credential was saved.",
                            error
                        ),
                        "Jcode account login failed",
                    );
                    return;
                }
            };

            let opened = App::open_auth_browser(&device.verification_uri_complete);
            publish(
                format!(
                    "Jcode Account Login\n\n{}\n\nApprove the request in the same browser. Jcode is waiting for the single-use exchange.{}",
                    device.verification_uri_complete,
                    if opened {
                        ""
                    } else {
                        "\n\nThe browser could not be opened automatically. Open the public URL above manually."
                    }
                ),
                "Jcode account: waiting for browser approval",
            );

            let approved = {
                let deadline = tokio::time::Instant::now()
                    + Duration::from_secs(device.expires_in.max(device.interval));
                let mut backoff = PollingBackoff::new(Duration::from_secs(device.interval));
                loop {
                    let delay = backoff.delay();
                    if tokio::time::Instant::now() + delay >= deadline {
                        break Err("Browser approval timed out. No credential was saved.".to_string());
                    }
                    tokio::time::sleep(delay).await;
                    match crate::subscription_api::poll_device_token_once(
                        &client,
                        &api_base,
                        &device.device_code,
                    )
                    .await
                    {
                        Ok(TokenPollOutcome::Pending) => backoff.on_pending(),
                        Ok(TokenPollOutcome::SlowDown { retry_after }) => {
                            backoff.on_slow_down(retry_after)
                        }
                        Ok(TokenPollOutcome::Approved(key)) => break Ok(key),
                        Ok(TokenPollOutcome::Expired) => break Err(
                            "The browser approval expired or was already exchanged. Start a new login."
                                .to_string(),
                        ),
                        Ok(TokenPollOutcome::Denied) => break Err(
                            "Jcode account login was canceled or denied in the browser."
                                .to_string(),
                        ),
                        Err(error) if error.is_temporary() => backoff.on_offline_error(),
                        Err(error) => break Err(error.to_string()),
                    }
                }
            };
            let approved = match approved {
                Ok(approved) => approved,
                Err(error) => {
                    publish(
                        format!("Jcode Account Login\n\n{error}\n\nRetry /account jcode login."),
                        "Jcode account login stopped",
                    );
                    return;
                }
            };

            if let Err(error) = crate::subscription_catalog::persist_account_credentials(
                &approved.api_key,
                Some(&approved.account_id),
                Some(&approved.email),
                Some(&approved.tier),
            ) {
                publish(
                    format!("Jcode Account Login\n\nBrowser approval succeeded, but secure credential persistence failed: {error}"),
                    "Jcode account credential save failed",
                );
                return;
            }
            crate::auth::AuthStatus::invalidate_cache();
            publish(
                format!(
                    "Jcode Account Approved\n\nSigned in as {}. The key is stored with owner-only permissions. Waiting for an active paid plan on /v1/me...",
                    approved.email
                ),
                "Jcode account: waiting for plan activation",
            );

            match crate::subscription_api::poll_for_paid_activation(
                &client,
                &api_base,
                &approved.api_key,
                crate::subscription_api::ACTIVATION_TIMEOUT,
                Duration::from_secs(device.interval.max(2)),
            )
            .await
            {
                ActivationOutcome::Active(me) => publish(
                    format!(
                        "Jcode Account Ready\n\n{} is active for {}.\n\nStatus: /account jcode status\nManage: /account jcode manage\nLogout: /account jcode logout",
                        me.parsed_tier()
                            .map(|tier| tier.display_name().to_string())
                            .unwrap_or(me.tier),
                        me.email
                    ),
                    "Jcode account plan active",
                ),
                ActivationOutcome::Canceled(_) => publish(
                    "Jcode Account Login\n\nCheckout was canceled. The valid account key remains saved, but no paid plan is active.\n\nStatus: /account jcode status\nManage: /account jcode manage\nLogout: /account jcode logout".to_string(),
                    "Jcode account plan not active",
                ),
                ActivationOutcome::TimedOut { last_error_was_offline } => publish(
                    format!(
                        "Jcode Account Login\n\nPlan activation was not confirmed before timeout{}. The valid account key remains saved.\n\nStatus: /account jcode status\nManage: /account jcode manage\nLogout: /account jcode logout",
                        if last_error_was_offline { " because the API remained unreachable" } else { "" }
                    ),
                    "Jcode account activation pending",
                ),
                ActivationOutcome::Revoked | ActivationOutcome::Denied => {
                    let _ = crate::subscription_catalog::clear_account_credentials();
                    publish(
                        "Jcode Account Login\n\nThe issued key was revoked or denied during activation checks. Local credentials were cleared. Retry /account jcode login.".to_string(),
                        "Jcode account key rejected",
                    );
                }
            }
        });
    }

    pub(super) fn open_jcode_account_management(&mut self) {
        let url = crate::subscription_catalog::JCODE_ACCOUNT_URL;
        let opened = Self::open_auth_browser(url);
        self.push_display_message(DisplayMessage::system(format!(
            "Jcode Account Management\n\n{}{}",
            url,
            if opened {
                "\n\nOpened in your browser."
            } else {
                "\n\nThe browser could not be opened automatically. Open the public URL above manually."
            }
        )));
        self.set_status_notice("Jcode account management");
    }

    pub(super) fn start_jcode_account_logout(&mut self) {
        self.set_status_notice("Jcode account: logging out");
        let session_id = self.session.id.clone();
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            let result = crate::subscription_catalog::clear_account_credentials();
            match result {
                Ok(()) => self.push_display_message(DisplayMessage::system(
                    "Jcode account credentials and cache were cleared locally. Remote revocation could not be attempted without the async runtime."
                        .to_string(),
                )),
                Err(error) => self.push_display_message(DisplayMessage::error(format!(
                    "Failed to clear local Jcode account credentials: {error}"
                ))),
            }
            return;
        };
        handle.spawn(async move {
            let api_key = crate::subscription_catalog::configured_api_key();
            let remote = if let Some(api_key) = api_key.as_deref() {
                crate::subscription_api::revoke_current_key(
                    &crate::provider::shared_http_client(),
                    &crate::subscription_api::configured_api_base(),
                    api_key,
                )
                .await
            } else {
                Ok(())
            };
            let local = crate::subscription_catalog::clear_account_credentials();
            crate::auth::AuthStatus::invalidate_cache();
            let message = match local {
                Err(error) => format!(
                    "Jcode Account Logout\n\nFailed to securely clear local credentials: {error}"
                ),
                Ok(()) => match (api_key.is_some(), remote) {
                    (false, _) => "Jcode Account Logout\n\nNo local credential was present. Local account cache is clear.".to_string(),
                    (true, Ok(())) => "Jcode Account Logout\n\nThe current key was revoked. Local credentials and account cache were securely cleared.".to_string(),
                    (true, Err(crate::subscription_api::AccountApiError::Unauthorized)) => "Jcode Account Logout\n\nThe key was already revoked. Local credentials and account cache were securely cleared.".to_string(),
                    (true, Err(crate::subscription_api::AccountApiError::Offline(_))) => "Jcode Account Logout\n\nLocal credentials and account cache were securely cleared. The API was offline, so remote revocation could not be confirmed.".to_string(),
                    (true, Err(error)) => format!("Jcode Account Logout\n\nLocal credentials and account cache were securely cleared. Remote revocation could not be confirmed: {error}"),
                },
            };
            crate::bus::Bus::global().publish(crate::bus::BusEvent::UiActivity(
                crate::bus::UiActivity::background(
                    Some(session_id),
                    message,
                    Some("Jcode account logout complete"),
                ),
            ));
        });
    }

    pub(super) fn start_claude_login_for_account(&mut self, label: &str) {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        use sha2::{Digest, Sha256};

        let verifier: String = {
            use rand::Rng;
            const CHARSET: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
            let mut rng = rand::rng();
            (0..64)
                .map(|_| {
                    let idx = rng.random_range(0..CHARSET.len());
                    CHARSET[idx] as char
                })
                .collect()
        };

        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let hash = hasher.finalize();
        let challenge = URL_SAFE_NO_PAD.encode(hash);

        // Try a loopback callback first so the user never has to copy/paste the
        // authorization code (mirrors the OpenAI/Gemini flows). Claude uses the
        // PKCE verifier as the OAuth `state`, so we wait for that on the
        // listener. If binding fails we fall back to manual paste with the
        // hosted redirect page.
        let callback_listener = crate::auth::oauth::bind_callback_listener(0).ok();
        let callback_port = callback_listener
            .as_ref()
            .and_then(|l| l.local_addr().ok())
            .map(|addr| addr.port());
        let callback_available = callback_listener.is_some() && callback_port.is_some();

        let (auth_url, redirect_uri) = match callback_port {
            Some(port) if callback_available => {
                let redirect_uri = format!("http://localhost:{}/callback", port);
                let auth_url =
                    crate::auth::oauth::claude_auth_url(&redirect_uri, &challenge, &verifier);
                (auth_url, redirect_uri)
            }
            _ => {
                let redirect_uri = crate::auth::oauth::claude::REDIRECT_URI.to_string();
                let auth_url =
                    crate::auth::oauth::claude_auth_url(&redirect_uri, &challenge, &verifier);
                (auth_url, redirect_uri)
            }
        };
        let qr_section = crate::login_qr::markdown_section_for_tui(
            &auth_url,
            "Scan this on another device if this machine has no browser:",
        )
        .map(|section| format!("\n\n{section}"))
        .unwrap_or_default();

        let browser_opened = Self::open_auth_browser(&auth_url);
        let preflight = Self::record_oauth_preflight(
            "claude",
            browser_opened,
            callback_port.map(|p| format!("localhost:{}", p)).as_deref(),
            Some(callback_available),
        );

        // Spawn the loopback waiter. On success it publishes LoginCompleted just
        // like the manual paste path, so onboarding and account UI react
        // identically.
        if let (Some(listener), true) = (callback_listener, callback_available) {
            let verifier_clone = verifier.clone();
            let label_clone = label.to_string();
            let redirect_clone = redirect_uri.clone();
            tokio::spawn(async move {
                match Self::claude_login_callback(
                    verifier_clone,
                    label_clone,
                    redirect_clone,
                    listener,
                )
                .await
                {
                    Ok(msg) => {
                        crate::logging::info(&format!("Claude login: {}", msg));
                        Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                            provider: "claude".to_string(),
                            success: true,
                            message: msg,
                        }));
                    }
                    Err(e) => {
                        crate::logging::info(&format!(
                            "Claude automatic callback did not complete: {}",
                            e
                        ));
                    }
                }
            });
        }

        let callback_line = if callback_available {
            "Waiting for the browser callback... (this completes automatically)\n".to_string()
        } else {
            "After logging in, copy the callback URL or authorization code and paste it here.\n"
                .to_string()
        };

        self.push_display_message(DisplayMessage::system(format!(
            "Claude OAuth Login (account: {})\n\n\
             Opening browser for authentication...\n\n\
             If the browser didn't open, visit:\n{}\n\n\
             {}{}{}\
             Or paste the full callback URL or authorization code here to finish from another device. Type /cancel to abort.{}",
            label,
            auth_url,
            if preflight.is_empty() {
                String::new()
            } else {
                format!("{}\n", preflight)
            },
            callback_line,
            if preflight.is_empty() {
                String::new()
            } else {
                "Manual-safe fallback is already active here.\n".to_string()
            },
            qr_section
        )));
        if callback_available {
            self.set_status_notice(format!("Login [{}]: waiting...", label));
        } else {
            self.set_status_notice(format!("Login [{}]: paste code...", label));
        }
        self.begin_pending_login(PendingLogin::ClaudeAccount {
            verifier,
            label: label.to_string(),
            redirect_uri: if callback_available {
                Some(redirect_uri)
            } else {
                None
            },
        });
    }

    async fn claude_login_callback(
        verifier: String,
        label: String,
        redirect_uri: String,
        listener: tokio::net::TcpListener,
    ) -> Result<String, String> {
        // Claude uses the PKCE verifier as the OAuth `state` value.
        let code = tokio::time::timeout(
            std::time::Duration::from_secs(300),
            crate::auth::oauth::wait_for_callback_async_on_listener(listener, &verifier),
        )
        .await
        .map_err(|_| "Login timed out after 5 minutes. Please try again.".to_string())?
        .map_err(|e| format!("Callback failed: {}", e))?;

        Self::claude_token_exchange(verifier, code, &label, Some(redirect_uri)).await
    }

    pub(super) fn switch_account(&mut self, label: &str) {
        match crate::auth::claude::set_active_account(label) {
            Ok(()) => {
                {
                    let provider = self.provider.clone();
                    let label_owned = label.to_string();
                    tokio::spawn(async move {
                        provider.invalidate_credentials().await;
                        crate::logging::info(&format!(
                            "Switched to Anthropic account '{}'",
                            label_owned
                        ));
                    });
                }
                self.push_display_message(DisplayMessage::system(format!(
                    "Switched to Anthropic account {}.",
                    label
                )));
                // Keep account-sensitive UI state in sync immediately.
                crate::auth::AuthStatus::invalidate_cache();
                self.context_limit = self.provider.context_window() as u64;
                self.context_warning_shown = false;
            }
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to switch account: {}",
                    e
                )));
            }
        }
    }

    pub(super) fn switch_account_by_label(&mut self, label: &str) {
        let has_anthropic = crate::auth::claude::list_accounts()
            .unwrap_or_default()
            .iter()
            .any(|account| account.label == label);
        let has_openai = crate::auth::codex::list_accounts()
            .unwrap_or_default()
            .iter()
            .any(|account| account.label == label);

        match (has_anthropic, has_openai) {
            (true, false) => self.switch_account(label),
            (false, true) => self.switch_openai_account(label),
            (true, true) => self.push_display_message(DisplayMessage::error(format!(
                "Account label {} exists for both Anthropic and OpenAI. Use /account switch {} or /account openai switch {} explicitly.",
                label, label, label
            ))),
            (false, false) => self.push_display_message(DisplayMessage::error(format!(
                "No Anthropic or OpenAI account with label {} found.",
                label
            ))),
        }
    }

    pub(super) fn remove_account(&mut self, label: &str) {
        match crate::auth::claude::remove_account(label) {
            Ok(()) => {
                self.push_display_message(DisplayMessage::system(format!(
                    "Removed Anthropic account {}.",
                    label
                )));
            }
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to remove account: {}",
                    e
                )));
            }
        }
    }

    pub(super) fn switch_openai_account(&mut self, label: &str) {
        match crate::auth::codex::set_active_account(label) {
            Ok(()) => {
                {
                    let provider = self.provider.clone();
                    let label_owned = label.to_string();
                    tokio::spawn(async move {
                        provider.invalidate_credentials().await;
                        crate::logging::info(&format!(
                            "Switched to OpenAI account '{}'",
                            label_owned
                        ));
                    });
                }
                self.push_display_message(DisplayMessage::system(format!(
                    "Switched to OpenAI account {}.",
                    label
                )));
                crate::auth::AuthStatus::invalidate_cache();
                self.context_limit = self.provider.context_window() as u64;
                self.context_warning_shown = false;
            }
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to switch OpenAI account: {}",
                    e
                )));
            }
        }
    }

    pub(super) fn remove_openai_account(&mut self, label: &str) {
        match crate::auth::codex::remove_account(label) {
            Ok(()) => {
                self.push_display_message(DisplayMessage::system(format!(
                    "Removed OpenAI account {}.",
                    label
                )));
            }
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to remove OpenAI account: {}",
                    e
                )));
            }
        }
    }

    fn start_openai_login(&mut self) {
        let label = crate::auth::codex::login_target_label(None)
            .unwrap_or_else(|_| crate::auth::codex::primary_account_label());
        self.start_openai_login_for_account(&label);
    }

    pub(super) fn start_openai_login_for_account(&mut self, label: &str) {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        use sha2::{Digest, Sha256};

        let verifier: String = {
            use rand::Rng;
            const CHARSET: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
            let mut rng = rand::rng();
            (0..64)
                .map(|_| {
                    let idx = rng.random_range(0..CHARSET.len());
                    CHARSET[idx] as char
                })
                .collect()
        };

        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let hash = hasher.finalize();
        let challenge = URL_SAFE_NO_PAD.encode(hash);

        let state: String = {
            let bytes: [u8; 16] = rand::random();
            hex::encode(bytes)
        };

        let port = crate::auth::oauth::openai::DEFAULT_PORT;
        let redirect_uri = crate::auth::oauth::openai::redirect_uri(port);
        let auth_url = crate::auth::oauth::openai_auth_url_with_prompt(
            &redirect_uri,
            &challenge,
            &state,
            Some("login"),
        );
        let qr_section = crate::login_qr::markdown_section_for_tui(
            &auth_url,
            "Scan this on another device if this machine has no browser, then paste the full callback URL here:",
        )
        .map(|section| format!("\n\n{section}"))
        .unwrap_or_default();

        let callback_listener = crate::auth::oauth::bind_callback_listener(port).ok();
        let callback_available = callback_listener.is_some();
        let browser_opened = Self::open_auth_browser(&auth_url);
        let label_owned = label.to_string();

        if let Some(listener) = callback_listener {
            let verifier_clone = verifier.clone();
            let state_clone = state.clone();
            let label_clone = label_owned.clone();
            tokio::spawn(async move {
                match Self::openai_login_callback(
                    verifier_clone,
                    state_clone,
                    Some(label_clone),
                    listener,
                )
                .await
                {
                    Ok(msg) => {
                        crate::logging::info(&format!("OpenAI login: {}", msg));
                        Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                            provider: "openai".to_string(),
                            success: true,
                            message: msg,
                        }));
                    }
                    Err(e) => {
                        crate::logging::info(&format!(
                            "OpenAI automatic callback did not complete: {}",
                            e
                        ));
                    }
                }
            });
        }

        let callback_line = if callback_available {
            format!(
                "Waiting for callback on localhost:{}... (this will complete automatically)\n",
                port
            )
        } else {
            format!(
                "Local callback port localhost:{} is unavailable, so finish in any browser and paste the full callback URL here.\n",
                port
            )
        };
        let preflight = Self::record_oauth_preflight(
            "openai",
            browser_opened,
            Some(&format!("localhost:{}", port)),
            Some(callback_available),
        );

        self.push_display_message(DisplayMessage::system(format!(
            "OpenAI OAuth Login (account: {})\n\n\
             Opening browser for authentication...\n\n\
             If the browser didn't open, visit:\n{}\n\n\
             Note: Wait a few seconds for the page to fully load before clicking Continue. \
             OpenAI's verification system may briefly disable the button.\n\n\
             {}{}{}\
             Or paste the full callback URL or query string here to finish from another device. Type /cancel to abort.{}",
            label,
            auth_url,
            if preflight.is_empty() {
                String::new()
            } else {
                format!("{}\n", preflight)
            },
            callback_line,
            if preflight.is_empty() {
                String::new()
            } else {
                "Manual-safe fallback is already active here.\n".to_string()
            },
            qr_section
        )));
        self.set_status_notice(format!("Login [{}]: waiting...", label));
        self.begin_pending_login(PendingLogin::OpenAiAccount {
            verifier,
            label: label.to_string(),
            expected_state: state,
            redirect_uri,
        });
    }

    async fn openai_login_callback(
        verifier: String,
        expected_state: String,
        label: Option<String>,
        listener: tokio::net::TcpListener,
    ) -> Result<String, String> {
        let port = crate::auth::oauth::openai::DEFAULT_PORT;
        let redirect_uri = crate::auth::oauth::openai::redirect_uri(port);
        let code = tokio::time::timeout(
            std::time::Duration::from_secs(300),
            crate::auth::oauth::wait_for_callback_async_on_listener(listener, &expected_state),
        )
        .await
        .map_err(|_| "Login timed out after 5 minutes. Please try again.".to_string())?
        .map_err(|e| format!("Callback failed: {}", e))?;

        Self::openai_token_exchange(verifier, code, label, None, &redirect_uri).await
    }

    async fn openai_token_exchange(
        verifier: String,
        input: String,
        label: Option<String>,
        expected_state: Option<String>,
        redirect_uri: &str,
    ) -> Result<String, String> {
        let oauth_tokens = if let Some(expected_state) = expected_state {
            crate::auth::oauth::exchange_openai_callback_input(
                &verifier,
                input.trim(),
                &expected_state,
                redirect_uri,
            )
            .await
            .map_err(|e| e.to_string())?
        } else {
            crate::auth::oauth::exchange_openai_code(&input, &verifier, redirect_uri)
                .await
                .map_err(|e| e.to_string())?
        };

        let label = label.unwrap_or_else(crate::auth::codex::primary_account_label);
        crate::auth::oauth::save_openai_tokens_for_account(&oauth_tokens, &label)
            .map_err(|e| format!("Failed to save tokens: {}", e))?;

        Ok(format!(
            "Successfully logged in to OpenAI! (account: {})",
            label
        ))
    }

    fn start_gemini_login(&mut self) {
        let (verifier, challenge) = crate::auth::oauth::generate_pkce_public();
        let state = crate::auth::oauth::generate_state_public();

        let callback_listener = crate::auth::oauth::bind_callback_listener(0).ok();
        let maybe_redirect_uri = callback_listener
            .as_ref()
            .and_then(|listener| listener.local_addr().ok())
            .map(|addr| format!("http://127.0.0.1:{}/oauth2callback", addr.port()));

        let auth_setup: anyhow::Result<(String, Option<String>, String)> =
            if let Some(redirect_uri) = maybe_redirect_uri {
                crate::auth::gemini::build_web_auth_url(&redirect_uri, &challenge, &state)
                    .map(|auth_url| (auth_url, Some(state.clone()), redirect_uri))
            } else {
                crate::auth::gemini::build_manual_auth_url(
                    "https://codeassist.google.com/authcode",
                    &challenge,
                    &state,
                )
                .map(|auth_url| {
                    (
                        auth_url,
                        None,
                        "https://codeassist.google.com/authcode".to_string(),
                    )
                })
            };

        let (auth_url, pending_state, redirect_uri) = match auth_setup {
            Ok(values) => values,
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Gemini login is unavailable: {}",
                    e
                )));
                self.set_status_notice("Login: failed");
                return;
            }
        };

        let qr_section = crate::login_qr::markdown_section_for_tui(
            &auth_url,
            "Scan this on another device if this machine has no browser, then paste the callback URL or authorization code here:",
        )
        .map(|section| format!("\n\n{section}"))
        .unwrap_or_default();

        let browser_opened = Self::open_auth_browser(&auth_url);
        let callback_available = callback_listener.is_some() && pending_state.is_some();

        if let (Some(listener), Some(expected_state)) = (callback_listener, pending_state.clone()) {
            let redirect_clone = redirect_uri.clone();
            let verifier_clone = verifier.clone();
            tokio::spawn(async move {
                let code = tokio::time::timeout(
                    std::time::Duration::from_secs(300),
                    crate::auth::oauth::wait_for_callback_async_on_listener(
                        listener,
                        &expected_state,
                    ),
                )
                .await
                .map_err(|_| "Login timed out after 5 minutes. Please try again.".to_string())
                .and_then(|result| result.map_err(|e| format!("Callback failed: {}", e)));

                match code {
                    Ok(code) => {
                        match crate::auth::gemini::exchange_callback_code(
                            &code,
                            &verifier_clone,
                            &redirect_clone,
                        )
                        .await
                        {
                            Ok(tokens) => {
                                let msg = if let Some(email) = tokens.email {
                                    format!(
                                        "Successfully logged in to Gemini! (account: {})",
                                        email
                                    )
                                } else {
                                    "Successfully logged in to Gemini!".to_string()
                                };
                                Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                    provider: "gemini".to_string(),
                                    success: true,
                                    message: msg,
                                }));
                            }
                            Err(e) => {
                                let message = format!("Gemini login failed: {}", e);
                                crate::logging::info(&format!(
                                    "Gemini automatic callback did not complete: {}",
                                    e
                                ));
                                Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                    provider: "gemini".to_string(),
                                    success: false,
                                    message,
                                }));
                            }
                        }
                    }
                    Err(e) => {
                        crate::logging::info(&format!(
                            "Gemini automatic callback did not complete: {}",
                            e
                        ));
                        Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                            provider: "gemini".to_string(),
                            success: false,
                            message: format!("Gemini login failed: {}", e),
                        }));
                    }
                }
            });
        }

        let callback_line = if callback_available {
            format!(
                "Waiting for callback on {}... (this will complete automatically)\n",
                redirect_uri
            )
        } else {
            "Finish login in any browser, then paste the callback URL or authorization code here.\n"
                .to_string()
        };
        let preflight = Self::record_oauth_preflight(
            "gemini",
            browser_opened,
            Some(&redirect_uri),
            Some(callback_available),
        );

        self.push_display_message(DisplayMessage::system(format!(
            "Gemini OAuth Login\n\n\
             Opening browser for authentication...\n\n\
             If the browser didn't open, visit:\n{}\n\n\
             {}{}{}\
             Or paste the full callback URL, query string, or authorization code here to finish. Type /cancel to abort.{}",
            auth_url,
            if preflight.is_empty() {
                String::new()
            } else {
                format!("{}\n", preflight)
            },
            callback_line,
            if preflight.is_empty() {
                String::new()
            } else {
                "Manual-safe fallback is already active here.\n".to_string()
            },
            qr_section
        )));
        self.set_status_notice("Login: waiting...");
        self.begin_pending_login(PendingLogin::Gemini {
            verifier,
            expected_state: pending_state,
            redirect_uri,
        });
    }

    fn start_openrouter_login(&mut self) {
        self.start_api_key_login(
            "OpenRouter",
            "https://openrouter.ai/keys",
            "openrouter.env",
            "OPENROUTER_API_KEY",
            None,
            None,
            false,
            None,
        );
    }

    fn start_bedrock_login(&mut self) {
        self.start_api_key_login(
            "AWS Bedrock",
            "https://console.aws.amazon.com/bedrock/home#/api-keys",
            crate::provider::bedrock::ENV_FILE,
            crate::provider::bedrock::API_KEY_ENV,
            Some("us.amazon.nova-micro-v1:0"),
            Some(
                "Region: us-east-2 (default for TUI onboarding; use CLI login for another region)",
            ),
            false,
            None,
        );
    }

    fn start_openai_api_key_login(&mut self) {
        self.start_api_key_login(
            "OpenAI API",
            "https://platform.openai.com/api-keys",
            "openai.env",
            "OPENAI_API_KEY",
            Some("gpt-5.5"),
            Some("https://api.openai.com/v1"),
            false,
            None,
        );
    }

    fn start_anthropic_api_key_login(&mut self) {
        self.start_api_key_login(
            "Anthropic API",
            "https://console.anthropic.com/settings/keys",
            "anthropic.env",
            "ANTHROPIC_API_KEY",
            Some("claude-opus-4-8"),
            Some("https://api.anthropic.com"),
            false,
            None,
        );
    }

    fn start_openai_compatible_profile_login(
        &mut self,
        profile: crate::provider_catalog::OpenAiCompatibleProfile,
    ) {
        if profile.id == crate::provider_catalog::OPENAI_COMPAT_PROFILE.id {
            let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
            self.push_display_message(DisplayMessage::system(format!(
                "{} Endpoint\n\n\
                 Setup docs: {}\n\
                 Current API base: {}\n\n\
                 Paste the API base below. Press Enter to keep the current value, or type /cancel to abort.",
                resolved.display_name, resolved.setup_url, resolved.api_base
            )));
            self.set_status_notice("Login: API base...");
            self.pending_login = Some(PendingLogin::OpenAiCompatibleApiBase { profile });
            return;
        }

        self.start_openai_compatible_key_login(profile);
    }

    fn start_openai_compatible_key_login(
        &mut self,
        profile: crate::provider_catalog::OpenAiCompatibleProfile,
    ) {
        let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
        self.start_api_key_login(
            &resolved.display_name,
            &resolved.setup_url,
            &resolved.env_file,
            &resolved.api_key_env,
            resolved.default_model.as_deref(),
            Some(&resolved.api_base),
            !resolved.requires_api_key,
            Some(profile),
        );
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "API-key login setup passes provider-specific metadata assembled at call sites"
    )]
    fn start_api_key_login(
        &mut self,
        provider: &str,
        docs_url: &str,
        env_file: &str,
        key_name: &str,
        default_model: Option<&str>,
        endpoint: Option<&str>,
        api_key_optional: bool,
        openai_compatible_profile: Option<crate::provider_catalog::OpenAiCompatibleProfile>,
    ) {
        let model_hint = default_model
            .map(|m| format!("Suggested default model: {}\n\n", m))
            .unwrap_or_default();
        let endpoint_hint = endpoint
            .map(|endpoint| format!("Endpoint: {}\n", endpoint))
            .unwrap_or_default();
        let prompt = if api_key_optional {
            "Paste your API key below if your endpoint requires one. Press Enter to skip, or type /cancel to abort."
        } else {
            "Paste your API key below (it will be saved securely), or type /cancel to abort."
        };
        self.push_display_message(DisplayMessage::system(format!(
            "{} {}\n\n\
             Setup docs: {}\n\
             Stored variable: {}\n\
             {}\
             {}\n\
             {}",
            provider,
            if api_key_optional {
                "Local Endpoint"
            } else {
                "API Key"
            },
            docs_url,
            key_name,
            endpoint_hint,
            model_hint,
            prompt,
        )));
        self.set_status_notice(if api_key_optional {
            "Login: optional key..."
        } else {
            "Login: paste key..."
        });
        let provider_id = openai_compatible_profile
            .map(|profile| profile.id.to_string())
            .unwrap_or_else(|| match key_name {
                crate::subscription_catalog::JCODE_API_KEY_ENV => "jcode".to_string(),
                "OPENROUTER_API_KEY" => "openrouter".to_string(),
                _ => provider.to_ascii_lowercase().replace(' ', "-"),
            });
        let auth_method = if api_key_optional {
            "local_endpoint"
        } else {
            "api_key"
        };
        self.begin_pending_login(PendingLogin::ApiKeyProfile {
            provider_id,
            provider: provider.to_string(),
            auth_method: auth_method.to_string(),
            docs_url: docs_url.to_string(),
            env_file: env_file.to_string(),
            key_name: key_name.to_string(),
            default_model: default_model.map(|m| m.to_string()),
            endpoint: endpoint.map(|value| value.to_string()),
            api_key_optional,
            openai_compatible_profile,
        });
    }

    fn start_azure_login(&mut self) {
        self.push_display_message(DisplayMessage::system(
            "Azure OpenAI Login\n\n\
             jcode uses Azure OpenAI's /openai/v1 API with either Microsoft Entra ID or an API key.\n\n\
             Enter your Azure OpenAI endpoint, for example https://your-resource.openai.azure.com, or type /cancel to abort."
                .to_string(),
        ));
        self.set_status_notice("Login: Azure endpoint...");
        self.begin_pending_login(PendingLogin::AzureEndpoint);
    }

    fn start_cursor_login(&mut self) {
        crate::telemetry::record_auth_started("cursor", "api_key");

        self.push_display_message(DisplayMessage::system(
            "Cursor API Key\n\n\
             Get your API key from: https://cursor.com/settings\n\
             (Dashboard > Integrations > User API Keys)\n\n\
             jcode will save it securely and use the native Cursor HTTPS transport.\n\n\
             Paste your API key below, or type /cancel to abort."
                .to_string(),
        ));
        self.set_status_notice("Login: paste cursor key...");
        self.begin_pending_login(PendingLogin::CursorApiKey);
    }

    fn start_copilot_login(&mut self) {
        self.set_status_notice("Login: copilot device flow...");
        self.begin_pending_login(PendingLogin::Copilot);

        tokio::spawn(async move {
            let client = crate::provider::shared_http_client();

            let device_resp = match crate::auth::copilot::initiate_device_flow(&client).await {
                Ok(resp) => resp,
                Err(e) => {
                    Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                        provider: "copilot".to_string(),
                        success: false,
                        message: format!("Copilot device flow failed: {}", e),
                    }));
                    return;
                }
            };

            let user_code = device_resp.user_code.clone();
            let verification_uri = device_resp.verification_uri.clone();

            let clipboard_ok = copy_to_clipboard(&user_code);
            let clipboard_msg = if clipboard_ok {
                " (copied to clipboard - just paste it!)"
            } else {
                ""
            };

            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                provider: "copilot_code".to_string(),
                success: true,
                message: {
                    let qr_section = crate::login_qr::markdown_section_for_tui(
                        &verification_uri,
                        "Scan this on another device to open the GitHub verification page:",
                    )
                    .map(|section| format!("\n\n{section}"))
                    .unwrap_or_default();
                    format!(
                        "GitHub Copilot Login\n\n\
                         Your code: {}{}\n\n\
                         Opening browser to {} ...\n\
                         Paste the code there and authorize.{}\n\n\
                         Waiting for authorization... (type /cancel to abort)",
                        user_code, clipboard_msg, verification_uri, qr_section
                    )
                },
            }));

            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let _ = Self::open_auth_browser(&verification_uri);

            let token = match crate::auth::copilot::poll_for_access_token(
                &client,
                &device_resp.device_code,
                device_resp.interval,
            )
            .await
            {
                Ok(t) => t,
                Err(e) => {
                    Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                        provider: "copilot".to_string(),
                        success: false,
                        message: format!("Copilot login failed: {}", e),
                    }));
                    return;
                }
            };

            let username = crate::auth::copilot::fetch_github_username(&client, &token)
                .await
                .unwrap_or_else(|_| "unknown".to_string());

            match crate::auth::copilot::save_github_token(&token, &username) {
                Ok(()) => {
                    Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                        provider: "copilot".to_string(),
                        success: true,
                        message: format!(
                            "Authenticated as {} via GitHub Copilot.\n\n\
                             Copilot models are now available in /model.",
                            username
                        ),
                    }));
                }
                Err(e) => {
                    Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                        provider: "copilot".to_string(),
                        success: false,
                        message: format!("Failed to save Copilot token: {}", e),
                    }));
                }
            }
        });

        self.push_display_message(DisplayMessage::system(
            "GitHub Copilot Login\n\n\
             Starting device flow... please wait. Type /cancel to abort."
                .to_string(),
        ));
    }

    fn start_antigravity_login(&mut self) {
        let (verifier, challenge) = crate::auth::oauth::generate_pkce_public();
        let expected_state = crate::auth::oauth::generate_state_public();
        let port = crate::auth::antigravity::DEFAULT_PORT;
        let redirect_uri = crate::auth::antigravity::redirect_uri(port);

        let auth_url = match crate::auth::antigravity::build_auth_url(
            &redirect_uri,
            &challenge,
            &expected_state,
        ) {
            Ok(url) => url,
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Antigravity login is unavailable: {}",
                    e
                )));
                self.set_status_notice("Login: failed");
                return;
            }
        };

        let qr_section = crate::login_qr::markdown_section_for_tui(
            &auth_url,
            "Scan this on another device if this machine has no browser, then paste the full callback URL or query string here:",
        )
        .map(|section| format!("\n\n{section}"))
        .unwrap_or_default();

        let callback_listener = crate::auth::oauth::bind_callback_listener(port).ok();
        let callback_available = callback_listener.is_some();
        let browser_opened = Self::open_auth_browser(&auth_url);

        if let Some(listener) = callback_listener {
            let verifier_clone = verifier.clone();
            let expected_state_clone = expected_state.clone();
            let redirect_clone = redirect_uri.clone();
            tokio::spawn(async move {
                let code = tokio::time::timeout(
                    std::time::Duration::from_secs(300),
                    crate::auth::oauth::wait_for_callback_async_on_listener(
                        listener,
                        &expected_state_clone,
                    ),
                )
                .await
                .map_err(|_| "Login timed out after 5 minutes. Please try again.".to_string())
                .and_then(|result| result.map_err(|e| format!("Callback failed: {}", e)));

                match code {
                    Ok(code) => {
                        match Self::antigravity_token_exchange(
                            verifier_clone,
                            code,
                            Some(expected_state_clone),
                            redirect_clone,
                        )
                        .await
                        {
                            Ok(msg) => {
                                Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                    provider: "antigravity".to_string(),
                                    success: true,
                                    message: msg,
                                }));
                            }
                            Err(e) => {
                                Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                    provider: "antigravity".to_string(),
                                    success: false,
                                    message: format!("Antigravity login failed: {}", e),
                                }));
                            }
                        }
                    }
                    Err(e) => {
                        crate::logging::info(&format!(
                            "Antigravity automatic callback did not complete: {}",
                            e
                        ));
                    }
                }
            });
        }

        let callback_line = if callback_available {
            format!(
                "Waiting for callback on {}... (this will complete automatically)\n",
                redirect_uri
            )
        } else {
            format!(
                "Local callback port {} is unavailable, so finish in any browser and paste the full callback URL or query string here.\n",
                redirect_uri
            )
        };
        let preflight = Self::record_oauth_preflight(
            "antigravity",
            browser_opened,
            Some(&redirect_uri),
            Some(callback_available),
        );
        let manual_hint = "If the browser ends on a loopback/callback error page, copy the full URL from the address bar and paste it here immediately.\n";

        self.push_display_message(DisplayMessage::system(format!(
            "Antigravity OAuth Login\n\n\
             Opening browser for authentication...\n\n\
             If the browser didn't open, visit:\n{}\n\n\
             {}{}{}{}\
             Or paste the full callback URL or query string here to finish. Type /cancel to abort.{}",
            auth_url,
            if preflight.is_empty() {
                String::new()
            } else {
                format!("{}\n", preflight)
            },
            callback_line,
            manual_hint,
            if preflight.is_empty() {
                String::new()
            } else {
                "Manual-safe fallback is already active here.\n".to_string()
            },
            qr_section
        )));
        self.set_status_notice("Login: antigravity waiting...");
        self.begin_pending_login(PendingLogin::Antigravity {
            verifier,
            expected_state,
            redirect_uri,
        });
    }

    async fn antigravity_token_exchange(
        verifier: String,
        input: String,
        expected_state: Option<String>,
        redirect_uri: String,
    ) -> Result<String, String> {
        let trimmed = input.trim();
        let tokens =
            if antigravity_input_requires_state_validation(trimmed, expected_state.as_deref()) {
                crate::auth::antigravity::exchange_callback_input(
                    &verifier,
                    trimmed,
                    expected_state.as_deref(),
                    &redirect_uri,
                )
                .await
            } else {
                crate::auth::antigravity::exchange_callback_code(trimmed, &verifier, &redirect_uri)
                    .await
            }
            .map_err(|e| e.to_string())?;

        let mut msg = if let Some(email) = tokens.email.as_deref() {
            format!(
                "Successfully logged in to Antigravity! (account: {})",
                email
            )
        } else {
            "Successfully logged in to Antigravity!".to_string()
        };
        if let Some(project_id) = tokens.project_id.as_deref() {
            msg.push_str(&format!(" (project: {})", project_id));
        }
        Ok(msg)
    }

    pub(super) fn handle_login_input(&mut self, pending: PendingLogin, input: String) {
        let trimmed = input.trim();
        if trimmed == "/cancel" {
            if let Some((provider, method)) = pending.telemetry_context() {
                crate::telemetry::record_auth_cancelled(&provider, &method);
            }
            self.push_display_message(DisplayMessage::system("Login cancelled.".to_string()));
            return;
        }

        if trimmed.is_empty() {
            let help = match &pending {
                PendingLogin::AutoImportSelection { .. } => {
                    "Auto import is waiting for your selection. Reply with a to approve all, 1,3 to approve specific sources, or /cancel to abort.".to_string()
                }
                _ => "Login still in progress. Complete it in your browser, or paste the callback URL / authorization code here. Type /cancel to abort.".to_string(),
            };
            self.push_display_message(DisplayMessage::system(help));
            self.pending_login = Some(pending);
            return;
        }

        match &pending {
            PendingLogin::OpenAiAccount { .. } if !looks_like_oauth_callback_input(trimmed) => {
                self.push_display_message(DisplayMessage::system(
                    "Still waiting for the browser callback. Paste the full callback URL or query string if you want to finish manually, or keep waiting for the automatic redirect.".to_string(),
                ));
                self.pending_login = Some(pending);
                return;
            }
            PendingLogin::Antigravity { .. } if !looks_like_oauth_callback_input(trimmed) => {
                self.push_display_message(DisplayMessage::system(
                    "Still waiting for the browser callback. Paste the full callback URL or query string if you want to finish manually, or keep waiting for the automatic redirect.".to_string(),
                ));
                self.pending_login = Some(pending);
                return;
            }
            _ => {}
        }

        match pending {
            PendingLogin::ClaudeAccount {
                verifier,
                label,
                redirect_uri,
            } => {
                self.set_status_notice(format!("Login [{}]: exchanging...", label));
                let input_owned = input.clone();
                let label_clone = label.clone();
                tokio::spawn(async move {
                    match Self::claude_token_exchange(
                        verifier,
                        input_owned,
                        &label_clone,
                        redirect_uri,
                    )
                    .await
                    {
                        Ok(msg) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "claude".to_string(),
                                success: true,
                                message: msg,
                            }));
                        }
                        Err(e) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "claude".to_string(),
                                success: false,
                                message: format!("Claude login [{}] failed: {}", label_clone, e),
                            }));
                        }
                    }
                });
                self.push_display_message(DisplayMessage::system(format!(
                    "Exchanging authorization code for account {}...",
                    label
                )));
            }
            PendingLogin::OpenAiAccount {
                verifier,
                label,
                expected_state,
                redirect_uri,
            } => {
                self.set_status_notice(format!("Login [{}]: exchanging...", label));
                let input_owned = input.clone();
                let label_clone = label.clone();
                tokio::spawn(async move {
                    match Self::openai_token_exchange(
                        verifier,
                        input_owned,
                        Some(label_clone.clone()),
                        Some(expected_state),
                        &redirect_uri,
                    )
                    .await
                    {
                        Ok(msg) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "openai".to_string(),
                                success: true,
                                message: msg,
                            }));
                        }
                        Err(e) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "openai".to_string(),
                                success: false,
                                message: format!("OpenAI login [{}] failed: {}", label_clone, e),
                            }));
                        }
                    }
                });
                self.push_display_message(DisplayMessage::system(format!(
                    "Exchanging OpenAI callback for account {}...",
                    label
                )));
            }
            PendingLogin::Gemini {
                verifier,
                expected_state,
                redirect_uri,
            } => {
                self.set_status_notice("Login: exchanging...");
                let input_owned = input.clone();
                tokio::spawn(async move {
                    match crate::auth::gemini::exchange_callback_input(
                        &verifier,
                        input_owned.trim(),
                        expected_state.as_deref(),
                        &redirect_uri,
                    )
                    .await
                    {
                        Ok(tokens) => {
                            let msg = if let Some(email) = tokens.email {
                                format!("Successfully logged in to Gemini! (account: {})", email)
                            } else {
                                "Successfully logged in to Gemini!".to_string()
                            };
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "gemini".to_string(),
                                success: true,
                                message: msg,
                            }));
                        }
                        Err(e) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "gemini".to_string(),
                                success: false,
                                message: format!("Gemini login failed: {}", e),
                            }));
                        }
                    }
                });
                self.push_display_message(DisplayMessage::system(
                    "Exchanging Gemini callback for tokens...".to_string(),
                ));
            }
            PendingLogin::Antigravity {
                verifier,
                expected_state,
                redirect_uri,
            } => {
                self.set_status_notice("Login: exchanging...");
                let input_owned = input.clone();
                tokio::spawn(async move {
                    match Self::antigravity_token_exchange(
                        verifier,
                        input_owned,
                        Some(expected_state),
                        redirect_uri,
                    )
                    .await
                    {
                        Ok(msg) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "antigravity".to_string(),
                                success: true,
                                message: msg,
                            }));
                        }
                        Err(e) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "antigravity".to_string(),
                                success: false,
                                message: format!("Antigravity login failed: {}", e),
                            }));
                        }
                    }
                });
                self.push_display_message(DisplayMessage::system(
                    "Exchanging Antigravity callback for tokens...".to_string(),
                ));
            }
            PendingLogin::ApiKeyProfile {
                provider_id,
                provider,
                auth_method,
                docs_url,
                env_file,
                key_name,
                default_model,
                endpoint,
                api_key_optional,
                openai_compatible_profile,
            } => {
                let key = input.trim().to_string();
                if key.is_empty() && !api_key_optional {
                    self.push_display_message(DisplayMessage::error(
                        "API key cannot be empty.".to_string(),
                    ));
                    self.pending_login = Some(PendingLogin::ApiKeyProfile {
                        provider_id,
                        provider,
                        auth_method,
                        docs_url,
                        env_file,
                        key_name,
                        default_model,
                        endpoint,
                        api_key_optional,
                        openai_compatible_profile,
                    });
                    return;
                }
                // Real API keys are never short digit strings. Users sometimes
                // type a menu number like `1` here, trying to select from a
                // numbered list shown earlier; silently saving that as the key
                // bricks the provider until they log in again (issue #496).
                if !key.is_empty() && key.len() < 8 && key.chars().all(|c| c.is_ascii_digit()) {
                    self.push_display_message(DisplayMessage::error(format!(
                        "'{}' looks like a menu selection, not an API key. This prompt is waiting for the {} API key itself. Paste the key (see {}), or type /cancel to abort.",
                        key, provider, docs_url
                    )));
                    self.pending_login = Some(PendingLogin::ApiKeyProfile {
                        provider_id,
                        provider,
                        auth_method,
                        docs_url,
                        env_file,
                        key_name,
                        default_model,
                        endpoint,
                        api_key_optional,
                        openai_compatible_profile,
                    });
                    return;
                }
                if key_name == "OPENROUTER_API_KEY" && !key.starts_with("sk-or-") {
                    self.push_display_message(DisplayMessage::system(
                        "OpenRouter keys typically start with sk-or-. Saving anyway...".to_string(),
                    ));
                }

                let resolved_openai_compatible = openai_compatible_profile
                    .map(crate::provider_catalog::resolve_openai_compatible_profile);

                // Record the key-save attempt before touching disk. This is the
                // single most important breadcrumb for issue #312 ("paste API
                // key for OpenAI-compatible/opencode silently returns to menu"):
                // it proves the input was received and which env var/file jcode
                // tried to write, without logging the key itself.
                crate::logging::event_info(
                    "login_api_key_save_attempt",
                    vec![
                        ("provider_id", provider_id.clone()),
                        ("provider", provider.clone()),
                        ("auth_method", auth_method.clone()),
                        ("env_var", key_name.clone()),
                        ("env_file", env_file.clone()),
                        ("input_len", key.len().to_string()),
                        ("optional", api_key_optional.to_string()),
                        (
                            "openai_compatible",
                            openai_compatible_profile.is_some().to_string(),
                        ),
                    ],
                );

                let save_result: anyhow::Result<()> =
                    if let Some(resolved) = resolved_openai_compatible.as_ref() {
                        (|| {
                            if resolved.requires_api_key {
                                crate::provider_catalog::save_env_value_to_env_file(
                                    crate::provider_catalog::OPENAI_COMPAT_LOCAL_ENABLED_ENV,
                                    &resolved.env_file,
                                    None,
                                )?;
                                crate::provider_catalog::save_env_value_to_env_file(
                                    &resolved.api_key_env,
                                    &resolved.env_file,
                                    Some(key.trim()),
                                )
                            } else {
                                crate::provider_catalog::save_env_value_to_env_file(
                                    crate::provider_catalog::OPENAI_COMPAT_LOCAL_ENABLED_ENV,
                                    &resolved.env_file,
                                    Some("1"),
                                )?;
                                crate::provider_catalog::save_env_value_to_env_file(
                                    &resolved.api_key_env,
                                    &resolved.env_file,
                                    if key.trim().is_empty() {
                                        None
                                    } else {
                                        Some(key.trim())
                                    },
                                )
                            }
                        })()
                    } else if key_name == crate::subscription_catalog::JCODE_API_KEY_ENV {
                        (|| {
                            let mut content = format!("{}={}\n", key_name, key);
                            if let Some(base) = crate::subscription_catalog::configured_api_base() {
                                content.push_str(&format!(
                                    "{}={}\n",
                                    crate::subscription_catalog::JCODE_API_BASE_ENV,
                                    base
                                ));
                            }

                            let config_dir = crate::storage::app_config_dir()?;
                            std::fs::create_dir_all(&config_dir)?;
                            crate::platform::set_directory_permissions_owner_only(&config_dir)?;

                            let file_path = config_dir.join(&env_file);
                            std::fs::write(&file_path, content)?;
                            crate::platform::set_permissions_owner_only(&file_path)?;
                            crate::env::set_var(&key_name, &key);
                            Ok(())
                        })()
                    } else if key_name == crate::provider::bedrock::API_KEY_ENV {
                        (|| {
                            Self::save_named_api_key(&env_file, &key_name, &key)?;
                            crate::provider_catalog::save_env_value_to_env_file(
                                crate::provider::bedrock::REGION_ENV,
                                &env_file,
                                Some("us-east-2"),
                            )
                        })()
                    } else {
                        Self::save_named_api_key(&env_file, &key_name, &key)
                    };

                match save_result {
                    Ok(()) => {
                        crate::auth::AuthStatus::invalidate_cache();
                        crate::logging::event_info(
                            "login_api_key_saved",
                            vec![
                                ("provider_id", provider_id.clone()),
                                ("provider", provider.clone()),
                                ("env_var", key_name.clone()),
                                ("env_file", env_file.clone()),
                                (
                                    "openai_compatible",
                                    openai_compatible_profile.is_some().to_string(),
                                ),
                            ],
                        );
                        if key_name == crate::provider::bedrock::API_KEY_ENV {
                            crate::provider::activation::lock_runtime_provider_key("bedrock");
                            if let Some(default_model) = default_model.as_deref() {
                                crate::env::set_var("JCODE_BEDROCK_MODEL", default_model);
                            }
                        }

                        if let Some(profile) = openai_compatible_profile {
                            crate::provider_catalog::apply_openai_compatible_profile_env(Some(
                                profile,
                            ));
                            self.start_openai_compatible_post_login_activation(
                                profile.id.to_string(),
                                provider.clone(),
                            );
                        }

                        let effective_default_model = resolved_openai_compatible
                            .as_ref()
                            .and_then(|resolved| resolved.default_model.as_deref())
                            .or(default_model.as_deref());
                        let model_hint = effective_default_model
                            .map(|m| format!("\nSuggested default model: {}", m))
                            .unwrap_or_default();
                        let guidance = if key_name == crate::subscription_catalog::JCODE_API_KEY_ENV
                        {
                            format!(
                                "Use /login jcode to access curated models via your router. If the model list looks stale, run /refresh-model-list.\nDocs: {}",
                                docs_url
                            )
                        } else if let Some(resolved) = resolved_openai_compatible.as_ref() {
                            if resolved.requires_api_key {
                                "Fetching models now. Jcode will switch to an accessible model returned by the live catalog and show the catalog diff when discovery finishes. If the model list looks stale, run /refresh-model-list.".to_string()
                            } else {
                                format!(
                                    "Local endpoint configured at {}. Fetching models now; Jcode will switch to an accessible model returned by the live catalog and show the catalog diff when discovery finishes. If the model list looks stale, run /refresh-model-list.",
                                    endpoint.as_deref().unwrap_or(resolved.api_base.as_str()),
                                )
                            }
                        } else if key_name == crate::provider::bedrock::API_KEY_ENV {
                            "You can now use /model to switch to Bedrock models. TUI onboarding saved region us-east-2; for a different region, run jcode login --provider bedrock from a terminal.".to_string()
                        } else if key_name == "OPENROUTER_API_KEY" {
                            "You can now use /model to switch to OpenRouter models. If the model list looks stale, run /refresh-model-list.".to_string()
                        } else {
                            "API key saved. Run /refresh-model-list to refresh model discovery, then use /model to pick an accessible model.".to_string()
                        };
                        let saved_label = if let Some(resolved) =
                            resolved_openai_compatible.as_ref()
                        {
                            if resolved.requires_api_key {
                                format!("{} API key saved", provider)
                            } else if key.trim().is_empty() {
                                format!("{} local endpoint saved", provider)
                            } else {
                                format!("{} local endpoint and optional API key saved", provider)
                            }
                        } else {
                            format!("{} API key saved", provider)
                        };
                        Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                            provider: provider.clone(),
                            success: true,
                            message: format!(
                                "{}.\n\n\
                                 Stored at ~/.config/jcode/{}.\n\
                                 {}{}",
                                saved_label, env_file, guidance, model_hint
                            ),
                        }));
                    }
                    Err(e) => {
                        let reason = crate::auth::login_diagnostics::classify_auth_failure_message(
                            &e.to_string(),
                        );
                        crate::logging::event_error(
                            "login_api_key_save_failed",
                            vec![
                                ("provider_id", provider_id.clone()),
                                ("provider", provider.clone()),
                                ("env_var", key_name.clone()),
                                ("env_file", env_file.clone()),
                                ("reason", reason.label().to_string()),
                                ("error", e.to_string()),
                            ],
                        );
                        crate::telemetry::record_auth_failed_reason(
                            &provider_id,
                            &auth_method,
                            reason.label(),
                        );
                        self.push_display_message(DisplayMessage::error(format!(
                            "Failed to save {} key: {}",
                            provider, e
                        )));
                        self.pending_login = Some(PendingLogin::ApiKeyProfile {
                            provider_id,
                            provider,
                            auth_method,
                            docs_url,
                            env_file,
                            key_name,
                            default_model,
                            endpoint,
                            api_key_optional,
                            openai_compatible_profile,
                        });
                    }
                }
            }
            PendingLogin::OpenAiCompatibleApiBase { profile } => {
                let api_base = input.trim();
                if !api_base.is_empty() {
                    let normalized = match crate::provider_catalog::normalize_api_base(api_base) {
                        Some(value) => value,
                        None => {
                            self.push_display_message(DisplayMessage::error(
                                "OpenAI-compatible API base must be https://... or http://localhost."
                                    .to_string(),
                            ));
                            self.pending_login =
                                Some(PendingLogin::OpenAiCompatibleApiBase { profile });
                            return;
                        }
                    };
                    if let Err(err) = crate::provider_catalog::save_env_value_to_env_file(
                        "JCODE_OPENAI_COMPAT_API_BASE",
                        crate::provider_catalog::OPENAI_COMPAT_PROFILE.env_file,
                        Some(&normalized),
                    ) {
                        self.push_display_message(DisplayMessage::error(format!(
                            "Failed to save OpenAI-compatible API base: {}",
                            err
                        )));
                        self.pending_login =
                            Some(PendingLogin::OpenAiCompatibleApiBase { profile });
                        return;
                    }
                }
                self.start_openai_compatible_key_login(profile);
            }
            PendingLogin::AzureEndpoint => {
                let endpoint_raw = input.trim();
                let Some(endpoint) = crate::auth::azure::normalize_endpoint(endpoint_raw) else {
                    self.push_display_message(DisplayMessage::error(
                        "Invalid Azure OpenAI endpoint. Use https://<resource>.openai.azure.com or the full /openai/v1 URL."
                            .to_string(),
                    ));
                    self.pending_login = Some(PendingLogin::AzureEndpoint);
                    return;
                };
                self.push_display_message(DisplayMessage::system(
                    "Azure endpoint accepted. Now enter the Azure deployment/model name, for example gpt-4.1-nano."
                        .to_string(),
                ));
                self.set_status_notice("Login: Azure model...");
                self.pending_login = Some(PendingLogin::AzureModel { endpoint });
            }
            PendingLogin::AzureModel { endpoint } => {
                let model = input.trim().to_string();
                if model.is_empty() {
                    self.push_display_message(DisplayMessage::error(
                        "Azure deployment/model name cannot be empty.".to_string(),
                    ));
                    self.pending_login = Some(PendingLogin::AzureModel { endpoint });
                    return;
                }
                self.push_display_message(DisplayMessage::system(
                    "Authentication method:\n\n\
                     1 Microsoft Entra ID via DefaultAzureCredential, for example az login\n\
                     2 Azure OpenAI API key\n\n\
                     Enter 1 or 2 [1]."
                        .to_string(),
                ));
                self.set_status_notice("Login: Azure auth method...");
                self.pending_login = Some(PendingLogin::AzureAuthChoice { endpoint, model });
            }
            PendingLogin::AzureAuthChoice { endpoint, model } => {
                let choice = input.trim();
                let use_entra = match choice {
                    "" | "1" => true,
                    "2" => false,
                    other
                        if other.eq_ignore_ascii_case("entra")
                            || other.eq_ignore_ascii_case("oauth") =>
                    {
                        true
                    }
                    other
                        if other.eq_ignore_ascii_case("key")
                            || other.eq_ignore_ascii_case("api-key") =>
                    {
                        false
                    }
                    _ => {
                        self.push_display_message(DisplayMessage::error(
                            "Invalid auth choice. Enter 1 for Entra ID or 2 for API key."
                                .to_string(),
                        ));
                        self.pending_login =
                            Some(PendingLogin::AzureAuthChoice { endpoint, model });
                        return;
                    }
                };
                if use_entra {
                    match Self::save_azure_config(&endpoint, &model, true, None) {
                        Ok(()) => self.finish_azure_login(true),
                        Err(err) => {
                            self.push_display_message(DisplayMessage::error(format!(
                                "Failed to save Azure OpenAI configuration: {}",
                                err
                            )));
                            self.pending_login =
                                Some(PendingLogin::AzureAuthChoice { endpoint, model });
                        }
                    }
                } else {
                    self.push_display_message(DisplayMessage::system(
                        "Paste your Azure OpenAI API key, or type /cancel to abort.".to_string(),
                    ));
                    self.set_status_notice("Login: Azure API key...");
                    self.pending_login = Some(PendingLogin::AzureApiKey { endpoint, model });
                }
            }
            PendingLogin::AzureApiKey { endpoint, model } => {
                let key = input.trim().to_string();
                if key.is_empty() {
                    self.push_display_message(DisplayMessage::error(
                        "Azure OpenAI API key cannot be empty.".to_string(),
                    ));
                    self.pending_login = Some(PendingLogin::AzureApiKey { endpoint, model });
                    return;
                }
                match Self::save_azure_config(&endpoint, &model, false, Some(&key)) {
                    Ok(()) => self.finish_azure_login(false),
                    Err(err) => {
                        self.push_display_message(DisplayMessage::error(format!(
                            "Failed to save Azure OpenAI configuration: {}",
                            err
                        )));
                        self.pending_login = Some(PendingLogin::AzureApiKey { endpoint, model });
                    }
                }
            }
            PendingLogin::CursorApiKey => {
                let key = input.trim().to_string();
                if key.is_empty() {
                    self.push_display_message(DisplayMessage::error(
                        "API key cannot be empty.".to_string(),
                    ));
                    self.pending_login = Some(PendingLogin::CursorApiKey);
                    return;
                }

                match crate::auth::cursor::save_api_key(&key) {
                    Ok(()) => {
                        crate::auth::AuthStatus::invalidate_cache();
                        Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                            provider: "cursor".to_string(),
                            success: true,
                            message: "Cursor API key saved.\n\n\
                             Stored at ~/.config/jcode/cursor.env.\n\
                             jcode will use it with the native Cursor HTTPS transport."
                                .to_string(),
                        }));
                    }
                    Err(e) => {
                        let reason = crate::auth::login_diagnostics::classify_auth_failure_message(
                            &e.to_string(),
                        );
                        crate::telemetry::record_auth_failed_reason(
                            "cursor",
                            "api_key",
                            reason.label(),
                        );
                        self.push_display_message(DisplayMessage::error(format!(
                            "Failed to save Cursor API key: {}",
                            e
                        )));
                        self.pending_login = Some(PendingLogin::CursorApiKey);
                    }
                }
            }
            PendingLogin::Copilot => {
                self.push_display_message(DisplayMessage::system(
                    "Copilot login is waiting for browser authorization.\n\
                     Complete the login in your browser, or type /cancel to abort."
                        .to_string(),
                ));
                self.pending_login = Some(PendingLogin::Copilot);
            }
            PendingLogin::AutoImportSelection { candidates } => {
                let selected = match crate::external_auth::parse_external_auth_review_selection(
                    &input,
                    candidates.len(),
                ) {
                    Ok(selected) => selected,
                    Err(err) => {
                        self.push_display_message(DisplayMessage::error(err.to_string()));
                        self.pending_login = Some(PendingLogin::AutoImportSelection { candidates });
                        return;
                    }
                };

                self.set_status_notice("Login: importing approved sources...");
                tokio::spawn(async move {
                    match crate::external_auth::run_external_auth_auto_import_candidates(
                        &candidates,
                        &selected,
                    )
                    .await
                    {
                        Ok(outcome) => {
                            // Auto-import bypasses the manual `pending_login`
                            // telemetry path, so record `auth_success` for each
                            // imported provider to keep the activation funnel
                            // accurate.
                            for (provider, method) in &outcome.imported_auth_labels {
                                crate::telemetry::record_auth_success(provider, method);
                            }
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "auto-import".to_string(),
                                success: outcome.imported > 0,
                                message: outcome.render_markdown(),
                            }));
                        }
                        Err(err) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "auto-import".to_string(),
                                success: false,
                                message: format!("Auto import failed: {}", err),
                            }));
                        }
                    }
                });
            }
        }
    }

    pub(super) fn onboarding_should_prefer_strongest_model(&self) -> bool {
        if !self.onboarding_flow_active() {
            return false;
        }

        let provider_config = &crate::config::config().provider;
        let has_explicit_default = provider_config
            .default_provider
            .as_deref()
            .is_some_and(|provider| !provider.trim().is_empty())
            || provider_config
                .default_model
                .as_deref()
                .is_some_and(|model| !model.trim().is_empty());
        let runtime_provider_forced =
            std::env::var("JCODE_FORCE_PROVIDER")
                .ok()
                .is_some_and(|value| {
                    matches!(
                        value.trim().to_ascii_lowercase().as_str(),
                        "1" | "true" | "yes" | "on"
                    )
                });

        !has_explicit_default && !runtime_provider_forced
    }

    fn trigger_provider_auth_changed(
        &mut self,
        provider_hint: Option<&str>,
        prefer_strongest: bool,
    ) {
        crate::logging::auth_event(
            "auth_changed_triggered",
            self.provider.name(),
            &[("surface", "tui")],
        );
        crate::bus::Bus::global().publish(crate::bus::BusEvent::UiActivity(
            crate::bus::UiActivity::auth(
                Some(self.session.id.clone()),
                "",
                Some("Auth: refreshing model routes..."),
            ),
        ));
        // Remote mode forwards the auth change to the server immediately after
        // this handler returns. Refreshing the client-side provider as well used
        // to duplicate every catalog network request and could race the first
        // onboarding prompt with a second model switch.
        if self.is_remote {
            return;
        }
        let provider = Arc::clone(&self.provider);
        let provider_hint = provider_hint.map(str::to_string);
        let session_id = self.session.id.clone();
        let select_local_model = true;
        let auto_selection_active = Arc::clone(&self.onboarding_auto_model_selection_active);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let activation = crate::auth::lifecycle::activate_auth_change(
                    &crate::auth::lifecycle::AuthActivationRequest::new(provider_hint, None),
                );
                provider.on_auth_changed();
                if select_local_model && activation.provider_id.is_some() {
                    let model_before_catalog_wait = provider.model();
                    // Hot initialization is synchronous, but provider catalogs can
                    // arrive shortly afterward. Retry briefly so a first-run import
                    // selects the strongest live route rather than validating the
                    // stale pre-import model.
                    for delay_ms in [0_u64, 150, 350, 750, 1_500] {
                        if delay_ms > 0 {
                            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                        }
                        let routes = provider.model_routes();
                        let selection = if prefer_strongest {
                            if !auto_selection_active.load(std::sync::atomic::Ordering::Acquire)
                                || provider.model() != model_before_catalog_wait
                            {
                                break;
                            }
                            let Some(route) =
                                crate::auth::lifecycle::globally_preferred_default_route(&routes)
                            else {
                                continue;
                            };
                            let exact_route = crate::provider::RouteSelection::from_model_route(&route);
                            let default_selection =
                                crate::provider::MultiProvider::default_model_selection_from_route(
                                    &route.model,
                                    &route.api_method,
                                    &route.provider,
                                );
                            Some((
                                route.model,
                                exact_route.routed_model_spec(),
                                default_selection.provider_key,
                                Some(exact_route),
                            ))
                        } else {
                            let current_model = provider.model();
                            crate::auth::lifecycle::provider_model_to_select_after_auth(
                                &activation,
                                Some(&current_model),
                                &routes,
                            )
                            .map(|model| {
                                let model_request =
                                    activation.model_switch_request(provider.name(), &model);
                                let provider_key = crate::provider::MultiProvider::session_provider_key_for_model_request(
                                    &model_request,
                                    provider.name(),
                                );
                                (model, model_request, provider_key, None)
                            })
                        };
                        let Some((model, model_request, provider_key, exact_route)) = selection else {
                            break;
                        };
                        if prefer_strongest
                            && (!auto_selection_active
                                .load(std::sync::atomic::Ordering::Acquire)
                                || provider.model() != model_before_catalog_wait)
                        {
                            break;
                        }
                        let applied = exact_route.as_ref().map_or_else(
                            || provider.set_model(&model_request),
                            |selection| provider.set_route_selection(selection),
                        );
                        if applied.is_ok() {
                            crate::bus::Bus::global().publish_models_updated();
                            crate::bus::Bus::global().publish(
                                crate::bus::BusEvent::ProviderModelActivated {
                                    session_id: session_id.clone(),
                                    model: model.clone(),
                                    provider_key,
                                    message: format!(
                                        "Login ready. Switched to the strongest available default model: {model}."
                                    ),
                                    open_picker: false,
                                },
                            );
                            break;
                        }
                    }
                }
                // Hot provider initialization is complete even if live catalog
                // prefetches are still running. Wake the picker now so it can use
                // the newly available routes instead of the pre-login snapshot.
                crate::bus::Bus::global().publish(crate::bus::BusEvent::AuthCatalogRefreshReady);
            });
        } else {
            let activation = crate::auth::lifecycle::activate_auth_change(
                &crate::auth::lifecycle::AuthActivationRequest::new(provider_hint, None),
            );
            provider.on_auth_changed();
            if select_local_model && activation.provider_id.is_some() {
                let routes = provider.model_routes();
                if prefer_strongest {
                    if let Some(route) =
                        crate::auth::lifecycle::globally_preferred_default_route(&routes)
                    {
                        let selection = crate::provider::RouteSelection::from_model_route(&route);
                        let model_request = selection.routed_model_spec();
                        if provider.set_route_selection(&selection).is_ok() {
                            self.finalize_model_switch(&model_request);
                        }
                    }
                } else {
                    let current_model = provider.model();
                    if let Some(model) = crate::auth::lifecycle::provider_model_to_select_after_auth(
                        &activation,
                        Some(&current_model),
                        &routes,
                    ) {
                        let model_request =
                            activation.model_switch_request(provider.name(), &model);
                        if provider.set_model(&model_request).is_ok() {
                            self.finalize_model_switch(&model_request);
                        }
                    }
                }
            }
            self.finish_auth_catalog_refresh();
        }
    }

    fn login_provider_is_azure(provider: &str) -> bool {
        let provider = provider.trim();
        provider.eq_ignore_ascii_case("azure")
            || provider.eq_ignore_ascii_case("azure-openai")
            || provider.eq_ignore_ascii_case("azure openai")
    }

    fn activate_azure_runtime_model_after_login(&mut self) {
        let activated_model = match crate::provider::activation::apply_azure_openai_runtime() {
            Ok(model) => model,
            Err(error) => {
                let message = error.to_string();
                crate::logging::auth_event(
                    "auth_changed_runtime_activation_failed",
                    "azure-openai",
                    &[("surface", "tui"), ("reason", message.as_str())],
                );
                self.trigger_provider_auth_changed(Some("azure-openai"), false);
                return;
            }
        };

        // Rebuild the OpenAI-compatible transport under the Azure runtime before
        // selecting the configured deployment. This is local-only state; it does
        // not send a prompt or resume an upstream conversation.
        self.provider.on_auth_changed();

        let Some(model) = activated_model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
        else {
            crate::bus::Bus::global().publish_models_updated();
            return;
        };

        let model_request = if self.provider.name().eq_ignore_ascii_case("openrouter") {
            model.to_string()
        } else {
            format!("openrouter:{}", model)
        };

        match self.provider.set_model(&model_request) {
            Ok(()) => {
                let active_model = self.finalize_model_switch(&model_request);
                crate::bus::Bus::global().publish_models_updated();
                crate::logging::auth_event(
                    "auth_changed_runtime_model_applied",
                    "azure-openai",
                    &[("surface", "tui"), ("provider_session", "reset")],
                );
                self.set_status_notice(format!("Login: Azure OpenAI ready ({})", active_model));
            }
            Err(error) => {
                let message = error.to_string();
                crate::logging::auth_event(
                    "auth_changed_runtime_model_failed",
                    "azure-openai",
                    &[("surface", "tui"), ("reason", message.as_str())],
                );
                crate::bus::Bus::global().publish_models_updated();
            }
        }
    }

    pub(super) fn start_openai_compatible_post_login_activation(
        &mut self,
        provider_id: String,
        provider_label: String,
    ) {
        crate::logging::event_info(
            "login_post_activation_started",
            vec![
                ("provider_id", provider_id.clone()),
                ("provider", provider_label.clone()),
                ("session_id", self.session.id.clone()),
            ],
        );
        crate::bus::Bus::global().publish(crate::bus::BusEvent::UiActivity(
            crate::bus::UiActivity::catalog(
                Some(self.session.id.clone()),
                format!(
                    "{} Model Discovery Started\n\nSaved credentials are active. Jcode is fetching the live model catalog, will only switch to a model returned by that catalog, and will show what changed when discovery finishes.",
                    provider_label
                ),
                Some(format!("{}: fetching models...", provider_label)),
            ),
        ));
        self.set_status_notice(format!("{}: fetching models...", provider_label));
        self.invalidate_model_picker_cache();

        // Make the newly saved OpenAI-compatible credentials usable in this
        // session immediately. The normal LoginCompleted path also calls this,
        // but doing it here lets the refresh task see the hot-added provider
        // without requiring a restart or a second user action.
        let provider = Arc::clone(&self.provider);
        let session_id = self.session.id.clone();
        let before_routes = provider.model_routes();
        self.provider.on_auth_changed();

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let result = provider.refresh_model_catalog().await;
                match result {
                    Ok(_summary) => {
                        let routes = provider.model_routes();
                        let expected_api_method = format!("openai-compatible:{}", provider_id);
                        let route_matches_profile = |route: &crate::provider::ModelRoute| {
                            route.available
                                && crate::provider::is_listable_model_name(&route.model)
                                && (route.api_method.eq_ignore_ascii_case(&expected_api_method)
                                    || route.api_method.eq_ignore_ascii_case(&provider_id))
                        };
                        let before_provider_routes = before_routes
                            .into_iter()
                            .filter(route_matches_profile)
                            .collect::<Vec<_>>();
                        let provider_routes = routes
                            .iter()
                            .filter(|route| route_matches_profile(route))
                            .cloned()
                            .collect::<Vec<_>>();
                        let before_provider_models = before_provider_routes
                            .iter()
                            .map(|route| route.model.clone())
                            .collect::<Vec<_>>();
                        let after_provider_models = provider_routes
                            .iter()
                            .map(|route| route.model.clone())
                            .collect::<Vec<_>>();
                        let summary = crate::provider::summarize_model_catalog_refresh(
                            before_provider_models,
                            after_provider_models,
                            before_provider_routes,
                            provider_routes.clone(),
                        );
                        let selected = provider_routes
                            .iter()
                            .find(|route| {
                                route.available
                                    && route.api_method.eq_ignore_ascii_case(&expected_api_method)
                                    && crate::provider::is_listable_model_name(&route.model)
                            })
                            .or_else(|| {
                                provider_routes.iter().find(|route| {
                                    route.available
                                        && route.api_method.eq_ignore_ascii_case(&provider_id)
                                        && crate::provider::is_listable_model_name(&route.model)
                                })
                            })
                            .map(|route| route.model.clone());

                        if let Some(model) = selected {
                            let model_request = format!("{}:{}", provider_id, model);
                            crate::logging::event_info(
                                "login_post_activation_route_selected",
                                vec![
                                    ("provider_id", provider_id.clone()),
                                    ("model", model.clone()),
                                    ("provider_routes", provider_routes.len().to_string()),
                                    ("models_added", summary.models_added.to_string()),
                                    ("routes_added", summary.routes_added.to_string()),
                                ],
                            );
                            match provider.set_model(&model_request) {
                                Ok(()) => {
                                    let provider_key = crate::provider::MultiProvider::session_provider_key_for_model_request(
                                        &model_request,
                                        provider.name(),
                                    );
                                    crate::logging::event_info(
                                        "login_post_activation_model_applied",
                                        vec![
                                            ("provider_id", provider_id.clone()),
                                            ("model", model.clone()),
                                            (
                                                "session_provider",
                                                provider_key.clone().unwrap_or_default(),
                                            ),
                                        ],
                                    );
                                    crate::bus::Bus::global().publish_models_updated();
                                    crate::bus::Bus::global().publish(
                                        crate::bus::BusEvent::ProviderModelActivated {
                                            session_id,
                                            model: model.clone(),
                                            provider_key,
                                            message: format!(
                                                "{} is ready.\n\nFetched model catalog: +{} models, +{} routes, ~{} changed.{}\n\nSwitched to {}. Use /model if you want to choose a different accessible model.\n\nIf the model list ever looks stale, run /refresh-model-list.",
                                                provider_label,
                                                summary.models_added,
                                                summary.routes_added,
                                                summary.routes_changed,
                                                {
                                                    let mut details = String::new();
                                                    super::model_context::append_model_name_diff(&mut details, &summary);
                                                    if details.is_empty() { String::new() } else { format!("\n{}", details) }
                                                },
                                                model
                                            ),
                                            open_picker: false,
                                        },
                                    );
                                }
                                Err(error) => {
                                    crate::logging::event_error(
                                        "login_post_activation_model_failed",
                                        vec![
                                            ("provider_id", provider_id.clone()),
                                            ("model", model.clone()),
                                            ("error", error.to_string()),
                                        ],
                                    );
                                    crate::bus::Bus::global().publish(
                                        crate::bus::BusEvent::LoginCompleted(
                                            crate::bus::LoginCompleted {
                                                provider: provider_label,
                                                success: false,
                                                message: format!(
                                                    "Fetched models, but failed to switch to {}: {}\n\nYou can run /refresh-model-list to retry model discovery.",
                                                    model, error
                                                ),
                                            },
                                        ),
                                    );
                                }
                            }
                        } else {
                            crate::logging::event_warn(
                                "login_post_activation_no_route",
                                vec![
                                    ("provider_id", provider_id.clone()),
                                    ("provider_routes", provider_routes.len().to_string()),
                                ],
                            );
                            crate::bus::Bus::global().publish(crate::bus::BusEvent::UiActivity(
                                crate::bus::UiActivity::catalog(
                                    Some(session_id),
                                    format!(
                                        "{} Model Discovery Still Updating\n\nSaved credentials are active, but this local refresh pass did not find a selectable {} route yet. Jcode is still processing the auth-change catalog refresh and will switch once provider routes are available. If the model list still looks stale after the auth catalog update, run /refresh-model-list.",
                                        provider_label, provider_label
                                    ),
                                    Some(format!(
                                        "{}: waiting for model routes...",
                                        provider_label
                                    )),
                                ),
                            ));
                        }
                    }
                    Err(error) => {
                        crate::logging::event_error(
                            "login_post_activation_refresh_failed",
                            vec![
                                ("provider_id", provider_id.clone()),
                                ("error", error.to_string()),
                            ],
                        );
                        crate::bus::Bus::global().publish(crate::bus::BusEvent::UiActivity(
                            crate::bus::UiActivity::catalog(
                                Some(session_id),
                                format!(
                                    "{} Model Discovery Still Updating\n\nSaved credentials are active, but this local refresh pass failed before the server auth-change catalog refresh finished. Jcode is still processing the auth-change catalog refresh and will switch once provider routes are available. If the model list still looks stale after the auth catalog update, run /refresh-model-list.\n\nLocal refresh error: {}",
                                    provider_label, error
                                ),
                                Some(format!(
                                    "{}: waiting for model routes...",
                                    provider_label
                                )),
                            ),
                        ));
                    }
                }
            });
        }
    }

    pub(super) fn handle_login_completed(&mut self, login: LoginCompleted) {
        if login.provider == "copilot_code" {
            self.push_display_message(DisplayMessage::system(login.message.clone()));
            if let Some(code) = login
                .message
                .split("Your code: ")
                .nth(1)
                .and_then(|s| s.split_whitespace().next())
            {
                self.set_status_notice(format!("Login: enter {} at GitHub", code));
            }
            return;
        }
        crate::auth::AuthStatus::invalidate_cache();
        crate::logging::event_info(
            "login_completed",
            vec![
                ("provider", login.provider.clone()),
                ("success", login.success.to_string()),
            ],
        );
        if let Some((provider, method)) = self
            .pending_login
            .as_ref()
            .and_then(PendingLogin::telemetry_context)
        {
            if login.success {
                crate::telemetry::record_auth_success(&provider, &method);
            } else {
                let reason =
                    crate::auth::login_diagnostics::classify_auth_failure_message(&login.message);
                crate::telemetry::record_auth_failed_reason(&provider, &method, reason.label());
            }
        }
        if login.success {
            self.recent_authenticated_provider = Some((login.provider.clone(), Instant::now()));
            // A fresh login is exactly what the credential-failure breaker is
            // waiting for: give automatic retries a fresh budget.
            self.reset_credential_failure_breaker();
            self.auth_catalog_refresh_pending = true;
            self.invalidate_model_picker_cache();
            let suppress_first_run_login_noise =
                self.onboarding_flow_active() && !matches!(login.provider.as_str(), "copilot_code");
            if !suppress_first_run_login_noise {
                self.push_display_message(DisplayMessage::system(login.message));
            }
            self.set_status_notice(format!("Login: {} ready", login.provider));
            if Self::login_provider_is_azure(&login.provider) {
                self.activate_azure_runtime_model_after_login();
            } else {
                let prefer_strongest = self.onboarding_should_prefer_strongest_model();
                if prefer_strongest {
                    self.onboarding_auto_model_selection_active
                        .store(true, std::sync::atomic::Ordering::Release);
                }
                self.trigger_provider_auth_changed(Some(&login.provider), prefer_strongest);
            }
            // First-run onboarding: once the user has authenticated on a fresh
            // install, walk them through model selection -> continue/suggestions.
            self.maybe_begin_onboarding_flow_after_login();
        } else {
            let message = crate::auth::login_diagnostics::augment_auth_error_message(
                &login.provider,
                &login.message,
            );
            // During onboarding we route the failure to the recovery screen
            // (which explains next steps) instead of dumping a raw error message
            // and a status notice the user can miss.
            if self.onboarding_flow_active() {
                self.onboarding_handle_login_failed(Some(message));
            } else {
                self.push_display_message(DisplayMessage::error(message));
                self.set_status_notice(format!("Login: {} failed", login.provider));
                self.onboarding_handle_login_failed(None);
            }
        }
        if self.pending_login.is_some() {
            self.pending_login = None;
        }
    }

    async fn claude_token_exchange(
        verifier: String,
        input: String,
        label: &str,
        redirect_uri: Option<String>,
    ) -> Result<String, String> {
        let fallback_redirect_uri =
            redirect_uri.unwrap_or_else(|| crate::auth::oauth::claude::REDIRECT_URI.to_string());
        let redirect_uri =
            crate::auth::oauth::claude_redirect_uri_for_input(input.trim(), &fallback_redirect_uri);
        let oauth_tokens =
            crate::auth::oauth::exchange_claude_code(&verifier, input.trim(), &redirect_uri)
                .await
                .map_err(|e| e.to_string())?;

        crate::auth::oauth::save_claude_tokens_for_account(&oauth_tokens, label)
            .map_err(|e| format!("Failed to save tokens: {}", e))?;

        let profile_suffix = match crate::auth::oauth::update_claude_account_profile(
            label,
            &oauth_tokens.access_token,
        )
        .await
        {
            Ok(Some(email)) => format!(" (email: {})", mask_email(&email)),
            Ok(None) => String::new(),
            Err(e) => {
                crate::logging::warn(&format!(
                    "Claude login [{}] profile fetch failed: {}",
                    label, e
                ));
                String::new()
            }
        };

        Ok(format!(
            "Successfully logged in to Claude! (account: {}){}",
            label, profile_suffix
        ))
    }

    fn save_named_api_key(env_file: &str, key_name: &str, key: &str) -> anyhow::Result<()> {
        if !crate::provider_catalog::is_safe_env_key_name(key_name) {
            anyhow::bail!("Invalid API key variable name: {}", key_name);
        }
        if !crate::provider_catalog::is_safe_env_file_name(env_file) {
            anyhow::bail!("Invalid env file name: {}", env_file);
        }

        let config_dir = crate::storage::app_config_dir()?;
        let file_path = config_dir.join(env_file);
        crate::storage::upsert_env_file_value(&file_path, key_name, Some(key))?;
        crate::env::set_var(key_name, key);
        Ok(())
    }

    fn save_azure_config(
        endpoint: &str,
        model: &str,
        use_entra: bool,
        api_key: Option<&str>,
    ) -> anyhow::Result<()> {
        use crate::auth::azure;

        crate::provider_catalog::save_env_value_to_env_file(
            azure::ENDPOINT_ENV,
            azure::ENV_FILE,
            Some(endpoint),
        )?;
        crate::provider_catalog::save_env_value_to_env_file(
            azure::MODEL_ENV,
            azure::ENV_FILE,
            Some(model),
        )?;
        crate::provider_catalog::save_env_value_to_env_file(
            azure::USE_ENTRA_ENV,
            azure::ENV_FILE,
            Some(if use_entra { "1" } else { "0" }),
        )?;
        if let Some(api_key) = api_key {
            crate::provider_catalog::save_env_value_to_env_file(
                azure::API_KEY_ENV,
                azure::ENV_FILE,
                Some(api_key),
            )?;
        }
        azure::apply_runtime_env()?;
        Ok(())
    }

    fn finish_azure_login(&mut self, use_entra: bool) {
        crate::auth::AuthStatus::invalidate_cache();
        if let Err(err) = crate::provider::activation::apply_azure_openai_runtime() {
            self.push_display_message(DisplayMessage::error(format!(
                "Failed to activate Azure OpenAI runtime: {}",
                err
            )));
            return;
        }
        crate::telemetry::record_auth_success(
            "azure",
            if use_entra { "entra_id" } else { "api_key" },
        );
        let auth_note = if use_entra {
            "Using Microsoft Entra ID through Azure DefaultAzureCredential. If you use Azure CLI auth, run az login and make sure the identity has the Cognitive Services OpenAI User role."
        } else {
            "Using the saved Azure OpenAI API key."
        };
        Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
            provider: "Azure OpenAI".to_string(),
            success: true,
            message: format!(
                "Azure OpenAI configuration saved.\n\n\
                 Stored at ~/.config/jcode/{}.\n\
                 {}\n\n\
                 Use /model after your Azure deployment exists. If the model list looks stale, run /refresh-model-list.",
                crate::auth::azure::ENV_FILE,
                auth_note,
            ),
        }));
    }
}

#[cfg(test)]
fn save_tui_openai_compatible_api_base(
    api_base: &str,
) -> anyhow::Result<crate::provider_catalog::ResolvedOpenAiCompatibleProfile> {
    let trimmed = api_base.trim();
    if !trimmed.is_empty() {
        let normalized = crate::provider_catalog::normalize_api_base(trimmed).ok_or_else(|| {
            anyhow::anyhow!("OpenAI-compatible API base must be https://... or http://localhost.")
        })?;
        crate::provider_catalog::save_env_value_to_env_file(
            "JCODE_OPENAI_COMPAT_API_BASE",
            crate::provider_catalog::OPENAI_COMPAT_PROFILE.env_file,
            Some(&normalized),
        )?;
    }
    Ok(crate::provider_catalog::resolve_openai_compatible_profile(
        crate::provider_catalog::OPENAI_COMPAT_PROFILE,
    ))
}

#[cfg(test)]
fn save_tui_openai_compatible_key(
    profile: crate::provider_catalog::OpenAiCompatibleProfile,
    key: &str,
) -> anyhow::Result<crate::provider_catalog::ResolvedOpenAiCompatibleProfile> {
    let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
    if resolved.requires_api_key {
        crate::provider_catalog::save_env_value_to_env_file(
            crate::provider_catalog::OPENAI_COMPAT_LOCAL_ENABLED_ENV,
            &resolved.env_file,
            None,
        )?;
        crate::provider_catalog::save_env_value_to_env_file(
            &resolved.api_key_env,
            &resolved.env_file,
            Some(key.trim()),
        )?;
    } else {
        crate::provider_catalog::save_env_value_to_env_file(
            crate::provider_catalog::OPENAI_COMPAT_LOCAL_ENABLED_ENV,
            &resolved.env_file,
            Some("1"),
        )?;
        crate::provider_catalog::save_env_value_to_env_file(
            &resolved.api_key_env,
            &resolved.env_file,
            if key.trim().is_empty() {
                None
            } else {
                Some(key.trim())
            },
        )?;
    }
    Ok(resolved)
}

fn looks_like_oauth_callback_input(input: &str) -> bool {
    let input = input.trim();
    input.starts_with("http://")
        || input.starts_with("https://")
        || input.starts_with('?')
        || input.contains("code=")
        || input.contains("state=")
}

fn antigravity_input_requires_state_validation(input: &str, expected_state: Option<&str>) -> bool {
    expected_state.is_some() && looks_like_oauth_callback_input(input)
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
