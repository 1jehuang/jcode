#[path = "auth_types.rs"]
mod auth_types;
#[path = "auth_account_commands.rs"]
mod auth_account_commands;

pub(crate) use self::auth_account_commands::{
    handle_account_command_remote, handle_auth_command, save_openai_fast_setting_local,
    resolve_account_provider_descriptor,
};
pub(super) use self::auth_types::{AccountCommand, PendingAccountInput, PendingLogin};

use super::*;
use crossterm::event::{KeyCode, KeyModifiers};

impl App {
    pub(super) fn show_jcode_subscription_status(&mut self) {
        let configured_key = crate::subscription_catalog::configured_api_key().is_some();
        let configured_base = crate::subscription_catalog::configured_api_base()
            .unwrap_or_else(|| crate::subscription_catalog::DEFAULT_JCODE_API_BASE.to_string());
        let runtime_mode = crate::subscription_catalog::is_runtime_mode_enabled();

        let mut message = String::from("**Jcode Subscription Status**\n\n");
        message.push_str(&format!(
            "- Credentials: {}\n",
            if configured_key {
                "configured"
            } else {
                "not configured (`/login jcode`)"
            }
        ));
        message.push_str(&format!(
            "- Router base: `{}`{}\n",
            configured_base,
            if crate::subscription_catalog::has_router_base() {
                ""
            } else {
                " _(default placeholder)_"
            }
        ));
        message.push_str(&format!(
            "- Runtime mode: {}\n\n",
            if runtime_mode {
                "active for this session"
            } else {
                "inactive for this session"
            }
        ));

        message.push_str("**Catalog**\n\n");
        for model in crate::subscription_catalog::curated_models() {
            let default_suffix = if model.default_enabled {
                " _(default)_"
            } else {
                ""
            };
            message.push_str(&format!(
                "- **{}** — `{}`{}\n  - {}\n  - {}\n",
                model.display_name,
                model.id,
                default_suffix,
                crate::subscription_catalog::routing_policy_detail(model),
                model.note
            ));
        }

        message.push_str("\n**Planned tiers**\n\n");
        for tier in [
            crate::subscription_catalog::JcodeTier::Starter20,
            crate::subscription_catalog::JcodeTier::Pro100,
        ] {
            message.push_str(&format!(
                "- {} — ${}/mo retail, about ${:.2} usable inference budget\n",
                tier.display_name(),
                tier.retail_price_usd(),
                tier.usable_budget_usd()
            ));
        }

        message.push_str(
            "\nUsage/billing reporting is not live yet; this command is a scaffold for the curated jcode-managed subscription path.",
        );

        self.push_display_message(DisplayMessage::system(message));
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
        let mut message = String::from(
            "**Authentication Status:**\n\n| Provider | Status | Method | Health | Validation |\n|----------|--------|--------|--------|------------|\n",
        );
        for provider in providers {
            let assessment = status.assessment_for_provider(provider);
            message.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                provider.display_name,
                icon(assessment.state),
                assessment.method_detail,
                assessment.health_summary(),
                validation
                    .get(provider.id)
                    .map(crate::auth::validation::format_record_label)
                    .unwrap_or_else(|| "not validated".to_string()),
            ));
        }
        message.push_str(
            "\nUse `/login <provider>` to authenticate. `/login jcode` is for curated jcode subscription access; `/account` opens the provider/account management center, and `/account <provider> settings` shows provider-specific controls.",
        );
        self.push_display_message(DisplayMessage::system(message));
    }

    pub(super) fn show_interactive_login(&mut self) {
        crate::telemetry::record_setup_step_once("login_picker_opened");
        self.open_login_picker_inline();
        self.set_status_notice("Login: choose a provider");
    }

    pub(super) fn start_login_provider(
        &mut self,
        provider: crate::provider_catalog::LoginProviderDescriptor,
    ) {
        crate::telemetry::record_provider_selected(provider.id);
        match provider.target {
            crate::provider_catalog::LoginProviderTarget::AutoImport => {
                match crate::cli::provider_init::pending_external_auth_review_candidates() {
                    Ok(candidates) if candidates.is_empty() => {
                        self.push_display_message(DisplayMessage::system(
                            "No importable external logins were found.".to_string(),
                        ));
                        self.set_status_notice("Login: no external imports found");
                    }
                    Ok(candidates) => {
                        self.push_display_message(DisplayMessage::system(
                            crate::cli::provider_init::format_external_auth_review_candidates_markdown(
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
            crate::provider_catalog::LoginProviderTarget::OpenAi => self.start_openai_login(),
            crate::provider_catalog::LoginProviderTarget::OpenRouter => {
                self.start_openrouter_login()
            }
            crate::provider_catalog::LoginProviderTarget::Azure => {
                crate::telemetry::record_auth_surface_blocked(
                    provider.id,
                    provider.auth_kind.label(),
                );
                self.push_display_message(DisplayMessage::error(
                    "Azure OpenAI login is currently CLI-only. Run `jcode login --provider azure`."
                        .to_string(),
                ));
            }
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
                    "Google/Gmail login is only available from the CLI right now. Run `jcode login --provider google`."
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
        self.push_display_message(DisplayMessage::system(format!(
            "**Jcode Subscription Login**\n\nPaste your jcode subscription API key. This is distinct from OpenRouter BYOK and is meant for curated jcode-managed access.\n\nCurated entries: {}\n\nOptional: after the key, jcode can also store a custom router base URL if you have one.",
            crate::subscription_catalog::curated_models()
                .iter()
                .map(|model| model.display_name)
                .collect::<Vec<_>>()
                .join(", ")
        )));
        self.set_status_notice("Login: jcode API key...");
        self.begin_pending_login(PendingLogin::ApiKeyProfile {
            provider_id: "jcode".to_string(),
            provider: "Jcode Subscription".to_string(),
            auth_method: "api_key".to_string(),
            docs_url: "https://subscription.jcode.invalid".to_string(),
            env_file: crate::subscription_catalog::JCODE_ENV_FILE.to_string(),
            key_name: crate::subscription_catalog::JCODE_API_KEY_ENV.to_string(),
            default_model: Some(crate::subscription_catalog::default_model().id.to_string()),
            endpoint: None,
            api_key_optional: false,
            openai_compatible_profile: None,
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

        let auth_url = crate::auth::oauth::claude_auth_url(
            crate::auth::oauth::claude::REDIRECT_URI,
            &challenge,
            &verifier,
        );
        let qr_section = crate::login_qr::markdown_section_for_tui(
            &auth_url,
            "Scan this on another device if this machine has no browser:",
        )
        .map(|section| format!("\n\n{section}"))
        .unwrap_or_default();

        let _ = open::that(&auth_url);

        self.push_display_message(DisplayMessage::system(format!(
            "**Claude OAuth Login** (account: `{}`)\n\n\
             Opening browser for authentication...\n\n\
             If the browser didn't open, visit:\n{}\n\n\
             After logging in, copy the callback URL or authorization code and **paste it here**.{}",
            label, auth_url, qr_section
        )));
        self.set_status_notice(format!("Login [{}]: paste code...", label));
        self.begin_pending_login(PendingLogin::ClaudeAccount {
            verifier,
            label: label.to_string(),
            redirect_uri: None,
        });
    }

    pub(super) fn open_account_center(&mut self, provider_filter: Option<&str>) {
        use crate::tui::account_picker::{AccountPicker, AccountPickerCommand, AccountPickerItem};

        crate::telemetry::record_setup_step_once("account_center_opened");

        let status = crate::auth::AuthStatus::check_fast();
        let validation = crate::auth::validation::load_all();
        let cfg = crate::config::Config::load();
        let providers: Vec<_> = match provider_filter {
            Some(provider_id) => match resolve_account_provider_descriptor(provider_id) {
                Some(provider) => vec![provider],
                None => {
                    self.push_display_message(DisplayMessage::error(format!(
                        "Unknown provider `{}`.",
                        provider_id
                    )));
                    self.set_status_notice("Account center unavailable");
                    return;
                }
            },
            None => crate::provider_catalog::login_providers().to_vec(),
        };

        let mut items = Vec::new();
        let mut summary = crate::tui::account_picker::AccountPickerSummary {
            provider_count: providers.len(),
            default_provider: cfg.provider.default_provider.clone(),
            default_model: cfg.provider.default_model.clone(),
            ..Default::default()
        };

        let provider_scope = provider_filter.map(|value| value.to_string());
        let claude_accounts = crate::auth::claude::list_accounts().unwrap_or_default();
        let openai_accounts = crate::auth::codex::list_accounts().unwrap_or_default();
        let add_replace_scope_supports_multi_account = match provider_scope.as_deref() {
            None => true,
            Some("claude" | "anthropic" | "openai") => true,
            Some(_) => false,
        };

        if add_replace_scope_supports_multi_account {
            let scoped_saved_accounts = match provider_scope.as_deref() {
                Some("claude" | "anthropic") => claude_accounts.len(),
                Some("openai") => openai_accounts.len(),
                _ => claude_accounts.len() + openai_accounts.len(),
            };
            let detail = if scoped_saved_accounts == 0 {
                "choose provider, add a new account, or replace an existing saved one".to_string()
            } else {
                format!(
                    "choose provider; {} Claude and {} OpenAI account(s) available",
                    claude_accounts.len(),
                    openai_accounts.len()
                )
            };
            items.push(AccountPickerItem::action(
                "account-flow",
                "Add / Replace",
                "Add or replace account",
                detail,
                AccountPickerCommand::OpenAddReplaceFlow {
                    provider_filter: provider_scope.clone(),
                },
            ));
        }

        for provider in providers {
            let auth_state = status.state_for_provider(provider);
            let method_detail = status.method_detail_for_provider(provider);
            let validation_detail = validation
                .get(provider.id)
                .map(crate::auth::validation::format_record_label)
                .unwrap_or_else(|| "not validated".to_string());
            match auth_state {
                crate::auth::AuthState::Available => summary.ready_count += 1,
                crate::auth::AuthState::Expired => summary.attention_count += 1,
                crate::auth::AuthState::NotConfigured => summary.setup_count += 1,
            }

            match provider.id {
                "claude" => summary.named_account_count += claude_accounts.len(),
                "openai" => summary.named_account_count += openai_accounts.len(),
                _ if !matches!(auth_state, crate::auth::AuthState::NotConfigured) => {
                    summary.named_account_count += 1;
                }
                _ => {}
            }

            let state_label = match auth_state {
                crate::auth::AuthState::Available => "ready",
                crate::auth::AuthState::Expired => "needs attention",
                crate::auth::AuthState::NotConfigured => "setup needed",
            };

            if !matches!(auth_state, crate::auth::AuthState::NotConfigured) {
                items.push(AccountPickerItem::action(
                    provider.id,
                    provider.display_name,
                    "Saved auth entry",
                    format!(
                        "{} - {} - {}",
                        state_label, method_detail, validation_detail
                    ),
                    AccountPickerCommand::SubmitInput(format!("/account {} settings", provider.id)),
                ));
            }

            items.push(AccountPickerItem::action(
                provider.id,
                provider.display_name,
                "Provider settings",
                format!(
                    "{} - {} - {}",
                    state_label, method_detail, validation_detail
                ),
                AccountPickerCommand::SubmitInput(format!("/account {} settings", provider.id)),
            ));
            items.push(AccountPickerItem::action(
                provider.id,
                provider.display_name,
                "Login / refresh",
                provider.menu_detail,
                AccountPickerCommand::SubmitInput(format!("/account {} login", provider.id)),
            ));

            match provider.id {
                "claude" => self.append_anthropic_account_picker_items(&mut items, provider),
                "openai" => {
                    self.append_openai_account_picker_items(&mut items, provider);
                    items.push(AccountPickerItem::action(
                        provider.id,
                        provider.display_name,
                        "Transport",
                        cfg.provider.openai_transport.as_deref().unwrap_or("auto"),
                        AccountPickerCommand::PromptValue {
                            prompt: "Enter OpenAI transport: auto, https, or websocket."
                                .to_string(),
                            command_prefix: "/account openai transport".to_string(),
                            empty_value: Some("auto".to_string()),
                            status_notice: "Account: editing OpenAI transport...".to_string(),
                        },
                    ));
                    items.push(AccountPickerItem::action(
                        provider.id,
                        provider.display_name,
                        "Reasoning effort",
                        cfg.provider
                            .openai_reasoning_effort
                            .as_deref()
                            .unwrap_or("(provider default)"),
                        AccountPickerCommand::PromptValue {
                            prompt: "Enter OpenAI reasoning effort: none, low, medium, high, xhigh, or clear.".to_string(),
                            command_prefix: "/account openai effort".to_string(),
                            empty_value: Some("clear".to_string()),
                            status_notice: "Account: editing OpenAI effort...".to_string(),
                        },
                    ));
                    items.push(AccountPickerItem::action(
                        provider.id,
                        provider.display_name,
                        "Fast mode",
                        if cfg.provider.openai_service_tier.as_deref() == Some("priority") {
                            "on"
                        } else {
                            "off"
                        },
                        AccountPickerCommand::SubmitInput(format!(
                            "/account openai fast {}",
                            if cfg.provider.openai_service_tier.as_deref() == Some("priority") {
                                "off"
                            } else {
                                "on"
                            }
                        )),
                    ));
                }
                "openai-compatible" => {
                    let compat = crate::provider_catalog::resolve_openai_compatible_profile(
                        crate::provider_catalog::OPENAI_COMPAT_PROFILE,
                    );
                    items.push(AccountPickerItem::action(
                        provider.id,
                        provider.display_name,
                        "API base",
                        compat.api_base,
                        AccountPickerCommand::PromptValue {
                            prompt: "Enter the OpenAI-compatible API base URL.".to_string(),
                            command_prefix: "/account openai-compatible api-base".to_string(),
                            empty_value: Some("clear".to_string()),
                            status_notice: "Account: editing API base...".to_string(),
                        },
                    ));
                    items.push(AccountPickerItem::action(
                        provider.id,
                        provider.display_name,
                        "API key variable",
                        compat.api_key_env,
                        AccountPickerCommand::PromptValue {
                            prompt: "Enter the env var name for the API key.".to_string(),
                            command_prefix: "/account openai-compatible api-key-name".to_string(),
                            empty_value: Some("clear".to_string()),
                            status_notice: "Account: editing API key variable...".to_string(),
                        },
                    ));
                    items.push(AccountPickerItem::action(
                        provider.id,
                        provider.display_name,
                        "Env file",
                        compat.env_file,
                        AccountPickerCommand::PromptValue {
                            prompt: "Enter the env file name for this profile.".to_string(),
                            command_prefix: "/account openai-compatible env-file".to_string(),
                            empty_value: Some("clear".to_string()),
                            status_notice: "Account: editing env file...".to_string(),
                        },
                    ));
                    items.push(AccountPickerItem::action(
                        provider.id,
                        provider.display_name,
                        "Default model hint",
                        compat
                            .default_model
                            .unwrap_or_else(|| "(unset)".to_string()),
                        AccountPickerCommand::PromptValue {
                            prompt: "Enter the default model hint for this profile.".to_string(),
                            command_prefix: "/account openai-compatible default-model".to_string(),
                            empty_value: Some("clear".to_string()),
                            status_notice: "Account: editing default model hint...".to_string(),
                        },
                    ));
                }
                "copilot" => {
                    items.push(AccountPickerItem::action(
                        provider.id,
                        provider.display_name,
                        "Premium requests",
                        cfg.provider.copilot_premium.as_deref().unwrap_or("normal"),
                        AccountPickerCommand::PromptValue {
                            prompt: "Enter Copilot premium mode: normal, one, or zero.".to_string(),
                            command_prefix: "/account copilot premium".to_string(),
                            empty_value: Some("normal".to_string()),
                            status_notice: "Account: editing Copilot premium mode...".to_string(),
                        },
                    ));
                }
                _ => {}
            }
        }

        items.push(AccountPickerItem::action(
            "defaults",
            "Global defaults",
            "Default provider",
            cfg.provider.default_provider.as_deref().unwrap_or("auto"),
            AccountPickerCommand::PromptValue {
                prompt: "Enter the default provider: claude, openai, copilot, gemini, openrouter, or auto.".to_string(),
                command_prefix: "/account default-provider".to_string(),
                empty_value: Some("auto".to_string()),
                status_notice: "Account: editing default provider...".to_string(),
            },
        ));
        items.push(AccountPickerItem::action(
            "defaults",
            "Global defaults",
            "Default model",
            cfg.provider
                .default_model
                .as_deref()
                .unwrap_or("(provider default)"),
            AccountPickerCommand::PromptValue {
                prompt: "Enter the default model, or clear to unset it.".to_string(),
                command_prefix: "/account default-model".to_string(),
                empty_value: Some("clear".to_string()),
                status_notice: "Account: editing default model...".to_string(),
            },
        ));

        let title = match provider_filter {
            Some(provider_id) => format!(" {} account center ", provider_id),
            None => " Accounts ".to_string(),
        };
        self.account_picker_overlay = Some(std::cell::RefCell::new(AccountPicker::with_summary(
            title, items, summary,
        )));
        self.inline_interactive_state = None;
        self.input.clear();
        self.cursor_pos = 0;
        self.set_status_notice("Account center: choose an action");
    }

    pub(super) fn open_account_add_replace_flow(&mut self, provider_filter: Option<&str>) {
        use crate::tui::account_picker::{AccountPicker, AccountPickerCommand, AccountPickerItem};

        let mut items = vec![AccountPickerItem::action(
            "account-flow",
            "Add / Replace",
            "Back to account center",
            "Return to the full provider/auth account center",
            AccountPickerCommand::OpenAccountCenter {
                provider_filter: provider_filter.map(|value| value.to_string()),
            },
        )];

        let include_claude = provider_filter.is_none()
            || matches!(provider_filter, Some("claude") | Some("anthropic"));
        let include_openai = provider_filter.is_none() || matches!(provider_filter, Some("openai"));

        if include_claude {
            items.push(AccountPickerItem::action(
                "claude",
                "Claude",
                "Add new account",
                "Create the next numbered Claude account",
                AccountPickerCommand::SubmitInput("/account claude add".to_string()),
            ));
            for account in crate::auth::claude::list_accounts().unwrap_or_default() {
                let label = account.label.clone();
                items.push(AccountPickerItem::action(
                    "claude",
                    "Claude",
                    format!("Replace account `{label}`"),
                    "Refresh this saved Claude account in place",
                    AccountPickerCommand::SubmitInput(format!("/account claude add {}", label)),
                ));
            }
        }

        if include_openai {
            items.push(AccountPickerItem::action(
                "openai",
                "OpenAI",
                "Add new account",
                "Create the next numbered OpenAI account",
                AccountPickerCommand::SubmitInput("/account openai add".to_string()),
            ));
            for account in crate::auth::codex::list_accounts().unwrap_or_default() {
                let label = account.label.clone();
                items.push(AccountPickerItem::action(
                    "openai",
                    "OpenAI",
                    format!("Replace account `{label}`"),
                    "Refresh this saved OpenAI account in place",
                    AccountPickerCommand::SubmitInput(format!("/account openai add {}", label)),
                ));
            }
        }

        self.account_picker_overlay = Some(std::cell::RefCell::new(AccountPicker::new(
            " Add / Replace Account ",
            items,
        )));
        self.inline_interactive_state = None;
        self.input.clear();
        self.cursor_pos = 0;
        self.set_status_notice("Account center: choose add/replace target");
    }

    pub(super) fn open_account_picker(&mut self, provider_filter: Option<&str>) {
        let Some(scope_key) = self.inline_account_picker_scope_key(provider_filter) else {
            if let Some(provider_id) = provider_filter {
                self.push_display_message(DisplayMessage::system(format!(
                    "Inline `/account` picker is only available for Claude and OpenAI accounts. Use `/account {} settings` for provider details.",
                    provider_id
                )));
            } else {
                self.push_display_message(DisplayMessage::system(
                    "Inline `/account` picker is available for Claude and OpenAI accounts. Use `/account claude` or `/account openai` to choose explicitly.".to_string(),
                ));
            }
            self.set_status_notice("Account picker unavailable");
            return;
        };

        let provider_label = match scope_key.as_str() {
            "all" => "Claude + OpenAI",
            "claude" => "Claude",
            "openai" => "OpenAI",
            _ => scope_key.as_str(),
        };

        let (models, selected) = match scope_key.as_str() {
            "all" => self.build_all_inline_account_picker(),
            "claude" => self.build_claude_inline_account_picker(),
            "openai" => self.build_openai_inline_account_picker(),
            _ => unreachable!(),
        };

        self.inline_view_state = None;
        self.inline_interactive_state = Some(crate::tui::InlineInteractiveState {
            kind: crate::tui::PickerKind::Account,
            filtered: (0..models.len()).collect(),
            entries: models,
            selected,
            column: 0,
            filter: String::new(),
            preview: false,
        });
        self.input.clear();
        self.cursor_pos = 0;
        self.set_status_notice(format!(
            "Account → {} (↑↓ or j/k, Enter to select)",
            provider_label
        ));
    }

    fn should_open_inline_account_picker(&self, provider_filter: Option<&str>) -> bool {
        provider_filter.is_none()
            || self
                .inline_account_picker_scope_key(provider_filter)
                .is_some()
    }

    pub(super) fn inline_account_picker_scope_key(
        &self,
        provider_filter: Option<&str>,
    ) -> Option<String> {
        if let Some(filter) = provider_filter {
            return match filter.to_ascii_lowercase().as_str() {
                "claude" | "anthropic" => Some("claude".to_string()),
                "openai" => Some("openai".to_string()),
                _ => None,
            };
        }

        Some("all".to_string())
    }

    pub(super) fn inline_account_picker_provider_id(
        &self,
        provider_filter: Option<&str>,
    ) -> Option<String> {
        match self
            .inline_account_picker_scope_key(provider_filter)?
            .as_str()
        {
            "claude" => Some("claude".to_string()),
            "openai" => Some("openai".to_string()),
            _ => None,
        }
    }

    fn build_all_inline_account_picker(&self) -> (Vec<crate::tui::PickerEntry>, usize) {
        let claude_accounts = crate::auth::claude::list_accounts().unwrap_or_default();
        let openai_accounts = crate::auth::codex::list_accounts().unwrap_or_default();
        let claude_active = crate::auth::claude::active_account_label()
            .unwrap_or_else(crate::auth::claude::primary_account_label);
        let openai_active = crate::auth::codex::active_account_label()
            .unwrap_or_else(crate::auth::codex::primary_account_label);
        let next_claude = crate::auth::claude::next_account_label()
            .unwrap_or_else(|_| crate::auth::claude::primary_account_label());
        let next_openai = crate::auth::codex::next_account_label()
            .unwrap_or_else(|_| crate::auth::codex::primary_account_label());
        let now_ms = chrono::Utc::now().timestamp_millis();
        let current_provider = if self.is_remote {
            self.remote_provider_name.clone()
        } else {
            Some(self.provider.name().to_string())
        }
        .unwrap_or_default()
        .to_ascii_lowercase();

        let mut models = Vec::with_capacity(claude_accounts.len() + openai_accounts.len() + 4);
        let mut selected = 0usize;

        for account in &claude_accounts {
            let is_active = account.label == claude_active;
            let status = if account.expires > now_ms {
                "valid"
            } else {
                "expired"
            };
            let email = account
                .email
                .as_deref()
                .map(mask_email)
                .unwrap_or_else(|| "unknown".to_string());
            let plan = account.subscription_type.as_deref().unwrap_or("unknown");
            let idx = models.len();
            if is_active
                && (current_provider.contains("claude") || current_provider.contains("anthropic"))
            {
                selected = idx;
            }
            models.push(crate::tui::PickerEntry {
                name: account.label.clone(),
                options: vec![crate::tui::PickerOption {
                    provider: "Claude".to_string(),
                    api_method: if is_active {
                        "active".to_string()
                    } else {
                        "saved".to_string()
                    },
                    available: true,
                    detail: format!("{} - {} - plan {}", email, status, plan),
                    estimated_reference_cost_micros: None,
                }],
                action: crate::tui::PickerAction::Account(
                    crate::tui::AccountPickerAction::Switch {
                        provider_id: "claude".to_string(),
                        label: account.label.clone(),
                    },
                ),
                selected_option: 0,
                is_current: is_active,
                is_default: false,
                recommended: false,
                recommendation_rank: usize::MAX,
                old: false,
                created_date: None,
                effort: None,
            });
        }

        for account in &openai_accounts {
            let is_active = account.label == openai_active;
            let status = match account.expires_at {
                Some(expires_at) if expires_at > now_ms => "valid",
                Some(_) => "expired",
                None => "valid",
            };
            let email = account
                .email
                .as_deref()
                .map(mask_email)
                .unwrap_or_else(|| "unknown".to_string());
            let account_id = account.account_id.as_deref().unwrap_or("unknown");
            let idx = models.len();
            if is_active && current_provider.contains("openai") {
                selected = idx;
            }
            models.push(crate::tui::PickerEntry {
                name: account.label.clone(),
                options: vec![crate::tui::PickerOption {
                    provider: "OpenAI".to_string(),
                    api_method: if is_active {
                        "active".to_string()
                    } else {
                        "saved".to_string()
                    },
                    available: true,
                    detail: format!("{} - {} - acct {}", email, status, account_id),
                    estimated_reference_cost_micros: None,
                }],
                action: crate::tui::PickerAction::Account(
                    crate::tui::AccountPickerAction::Switch {
                        provider_id: "openai".to_string(),
                        label: account.label.clone(),
                    },
                ),
                selected_option: 0,
                is_current: is_active,
                is_default: false,
                recommended: false,
                recommendation_rank: usize::MAX,
                old: false,
                created_date: None,
                effort: None,
            });
        }

        models.push(crate::tui::PickerEntry {
            name: "new Claude account".to_string(),
            options: vec![crate::tui::PickerOption {
                provider: "Claude".to_string(),
                api_method: "new".to_string(),
                available: true,
                detail: format!("create {}", next_claude),
                estimated_reference_cost_micros: None,
            }],
            action: crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Add {
                provider_id: "claude".to_string(),
            }),
            selected_option: 0,
            is_current: false,
            is_default: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            old: false,
            created_date: None,
            effort: None,
        });

        models.push(crate::tui::PickerEntry {
            name: "new OpenAI account".to_string(),
            options: vec![crate::tui::PickerOption {
                provider: "OpenAI".to_string(),
                api_method: "new".to_string(),
                available: true,
                detail: format!("create {}", next_openai),
                estimated_reference_cost_micros: None,
            }],
            action: crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Add {
                provider_id: "openai".to_string(),
            }),
            selected_option: 0,
            is_current: false,
            is_default: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            old: false,
            created_date: None,
            effort: None,
        });

        let replace_claude = claude_accounts
            .iter()
            .find(|account| account.label == claude_active)
            .map(|account| account.label.clone())
            .or_else(|| claude_accounts.first().map(|account| account.label.clone()))
            .unwrap_or_else(crate::auth::claude::primary_account_label);
        models.push(crate::tui::PickerEntry {
            name: "replace Claude account".to_string(),
            options: vec![crate::tui::PickerOption {
                provider: "Claude".to_string(),
                api_method: "replace".to_string(),
                available: !claude_accounts.is_empty(),
                detail: if claude_accounts.is_empty() {
                    "no saved accounts yet".to_string()
                } else {
                    format!("refresh {}", replace_claude)
                },
                estimated_reference_cost_micros: None,
            }],
            action: crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Replace {
                provider_id: "claude".to_string(),
                label: replace_claude,
            }),
            selected_option: 0,
            is_current: false,
            is_default: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            old: false,
            created_date: None,
            effort: None,
        });

        let replace_openai = openai_accounts
            .iter()
            .find(|account| account.label == openai_active)
            .map(|account| account.label.clone())
            .or_else(|| openai_accounts.first().map(|account| account.label.clone()))
            .unwrap_or_else(crate::auth::codex::primary_account_label);
        models.push(crate::tui::PickerEntry {
            name: "replace OpenAI account".to_string(),
            options: vec![crate::tui::PickerOption {
                provider: "OpenAI".to_string(),
                api_method: "replace".to_string(),
                available: !openai_accounts.is_empty(),
                detail: if openai_accounts.is_empty() {
                    "no saved accounts yet".to_string()
                } else {
                    format!("refresh {}", replace_openai)
                },
                estimated_reference_cost_micros: None,
            }],
            action: crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Replace {
                provider_id: "openai".to_string(),
                label: replace_openai,
            }),
            selected_option: 0,
            is_current: false,
            is_default: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            old: false,
            created_date: None,
            effort: None,
        });

        models.push(crate::tui::PickerEntry {
            name: "account center".to_string(),
            options: vec![crate::tui::PickerOption {
                provider: "Accounts".to_string(),
                api_method: "manage".to_string(),
                available: true,
                detail: "settings, defaults, and other providers".to_string(),
                estimated_reference_cost_micros: None,
            }],
            action: crate::tui::PickerAction::Account(
                crate::tui::AccountPickerAction::OpenCenter {
                    provider_filter: None,
                },
            ),
            selected_option: 0,
            is_current: false,
            is_default: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            old: false,
            created_date: None,
            effort: None,
        });

        if models.is_empty() {
            selected = 0;
        }
        (models, selected)
    }

    fn build_claude_inline_account_picker(&self) -> (Vec<crate::tui::PickerEntry>, usize) {
        let accounts = crate::auth::claude::list_accounts().unwrap_or_default();
        let active_label = crate::auth::claude::active_account_label()
            .unwrap_or_else(crate::auth::claude::primary_account_label);
        let next_label = crate::auth::claude::next_account_label()
            .unwrap_or_else(|_| crate::auth::claude::primary_account_label());
        let now_ms = chrono::Utc::now().timestamp_millis();

        let mut models = Vec::with_capacity(accounts.len() + 2);
        let mut selected = 0usize;

        for (index, account) in accounts.iter().enumerate() {
            let is_active = account.label == active_label;
            if is_active {
                selected = index;
            }
            let status = if account.expires > now_ms {
                "valid"
            } else {
                "expired"
            };
            let email = account
                .email
                .as_deref()
                .map(mask_email)
                .unwrap_or_else(|| "unknown".to_string());
            let plan = account.subscription_type.as_deref().unwrap_or("unknown");
            models.push(crate::tui::PickerEntry {
                name: account.label.clone(),
                options: vec![crate::tui::PickerOption {
                    provider: "Claude".to_string(),
                    api_method: if is_active {
                        "active".to_string()
                    } else {
                        "saved".to_string()
                    },
                    available: true,
                    detail: format!("{} - {} - plan {}", email, status, plan),
                    estimated_reference_cost_micros: None,
                }],
                action: crate::tui::PickerAction::Account(
                    crate::tui::AccountPickerAction::Switch {
                        provider_id: "claude".to_string(),
                        label: account.label.clone(),
                    },
                ),
                selected_option: 0,
                is_current: is_active,
                is_default: false,
                recommended: false,
                recommendation_rank: usize::MAX,
                old: false,
                created_date: None,
                effort: None,
            });
        }

        models.push(crate::tui::PickerEntry {
            name: "new account".to_string(),
            options: vec![crate::tui::PickerOption {
                provider: "Claude".to_string(),
                api_method: "new".to_string(),
                available: true,
                detail: format!("create {}", next_label),
                estimated_reference_cost_micros: None,
            }],
            action: crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Add {
                provider_id: "claude".to_string(),
            }),
            selected_option: 0,
            is_current: false,
            is_default: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            old: false,
            created_date: None,
            effort: None,
        });

        let replace_target = accounts
            .iter()
            .find(|account| account.label == active_label)
            .map(|account| account.label.clone())
            .or_else(|| accounts.first().map(|account| account.label.clone()))
            .unwrap_or_else(crate::auth::claude::primary_account_label);
        models.push(crate::tui::PickerEntry {
            name: "replace account".to_string(),
            options: vec![crate::tui::PickerOption {
                provider: "Claude".to_string(),
                api_method: "replace".to_string(),
                available: !accounts.is_empty(),
                detail: if accounts.is_empty() {
                    "no saved accounts yet".to_string()
                } else {
                    format!("refresh {}", replace_target)
                },
                estimated_reference_cost_micros: None,
            }],
            action: crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Replace {
                provider_id: "claude".to_string(),
                label: replace_target,
            }),
            selected_option: 0,
            is_current: false,
            is_default: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            old: false,
            created_date: None,
            effort: None,
        });

        models.push(crate::tui::PickerEntry {
            name: "account center".to_string(),
            options: vec![crate::tui::PickerOption {
                provider: "Claude".to_string(),
                api_method: "manage".to_string(),
                available: true,
                detail: "full Claude account center and settings".to_string(),
                estimated_reference_cost_micros: None,
            }],
            action: crate::tui::PickerAction::Account(
                crate::tui::AccountPickerAction::OpenCenter {
                    provider_filter: Some("claude".to_string()),
                },
            ),
            selected_option: 0,
            is_current: false,
            is_default: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            old: false,
            created_date: None,
            effort: None,
        });

        if accounts.is_empty() {
            selected = 0;
        }
        (models, selected)
    }

    fn build_openai_inline_account_picker(&self) -> (Vec<crate::tui::PickerEntry>, usize) {
        let accounts = crate::auth::codex::list_accounts().unwrap_or_default();
        let active_label = crate::auth::codex::active_account_label()
            .unwrap_or_else(crate::auth::codex::primary_account_label);
        let next_label = crate::auth::codex::next_account_label()
            .unwrap_or_else(|_| crate::auth::codex::primary_account_label());
        let now_ms = chrono::Utc::now().timestamp_millis();

        let mut models = Vec::with_capacity(accounts.len() + 2);
        let mut selected = 0usize;

        for (index, account) in accounts.iter().enumerate() {
            let is_active = account.label == active_label;
            if is_active {
                selected = index;
            }
            let status = match account.expires_at {
                Some(expires_at) if expires_at > now_ms => "valid",
                Some(_) => "expired",
                None => "valid",
            };
            let email = account
                .email
                .as_deref()
                .map(mask_email)
                .unwrap_or_else(|| "unknown".to_string());
            let account_id = account.account_id.as_deref().unwrap_or("unknown");
            models.push(crate::tui::PickerEntry {
                name: account.label.clone(),
                options: vec![crate::tui::PickerOption {
                    provider: "OpenAI".to_string(),
                    api_method: if is_active {
                        "active".to_string()
                    } else {
                        "saved".to_string()
                    },
                    available: true,
                    detail: format!("{} - {} - acct {}", email, status, account_id),
                    estimated_reference_cost_micros: None,
                }],
                action: crate::tui::PickerAction::Account(
                    crate::tui::AccountPickerAction::Switch {
                        provider_id: "openai".to_string(),
                        label: account.label.clone(),
                    },
                ),
                selected_option: 0,
                is_current: is_active,
                is_default: false,
                recommended: false,
                recommendation_rank: usize::MAX,
                old: false,
                created_date: None,
                effort: None,
            });
        }

        models.push(crate::tui::PickerEntry {
            name: "new account".to_string(),
            options: vec![crate::tui::PickerOption {
                provider: "OpenAI".to_string(),
                api_method: "new".to_string(),
                available: true,
                detail: format!("create {}", next_label),
                estimated_reference_cost_micros: None,
            }],
            action: crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Add {
                provider_id: "openai".to_string(),
            }),
            selected_option: 0,
            is_current: false,
            is_default: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            old: false,
            created_date: None,
            effort: None,
        });

        let replace_target = accounts
            .iter()
            .find(|account| account.label == active_label)
            .map(|account| account.label.clone())
            .or_else(|| accounts.first().map(|account| account.label.clone()))
            .unwrap_or_else(crate::auth::codex::primary_account_label);
        models.push(crate::tui::PickerEntry {
            name: "replace account".to_string(),
            options: vec![crate::tui::PickerOption {
                provider: "OpenAI".to_string(),
                api_method: "replace".to_string(),
                available: !accounts.is_empty(),
                detail: if accounts.is_empty() {
                    "no saved accounts yet".to_string()
                } else {
                    format!("refresh {}", replace_target)
                },
                estimated_reference_cost_micros: None,
            }],
            action: crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Replace {
                provider_id: "openai".to_string(),
                label: replace_target,
            }),
            selected_option: 0,
            is_current: false,
            is_default: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            old: false,
            created_date: None,
            effort: None,
        });

        models.push(crate::tui::PickerEntry {
            name: "account center".to_string(),
            options: vec![crate::tui::PickerOption {
                provider: "OpenAI".to_string(),
                api_method: "manage".to_string(),
                available: true,
                detail: "full OpenAI account center and settings".to_string(),
                estimated_reference_cost_micros: None,
            }],
            action: crate::tui::PickerAction::Account(
                crate::tui::AccountPickerAction::OpenCenter {
                    provider_filter: Some("openai".to_string()),
                },
            ),
            selected_option: 0,
            is_current: false,
            is_default: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            old: false,
            created_date: None,
            effort: None,
        });

        if accounts.is_empty() {
            selected = 0;
        }
        (models, selected)
    }

    pub(super) fn handle_account_picker_command(
        &mut self,
        command: crate::tui::account_picker::AccountPickerCommand,
    ) {
        match command {
            crate::tui::account_picker::AccountPickerCommand::OpenAccountCenter {
                provider_filter,
            } => self.open_account_center(provider_filter.as_deref()),
            crate::tui::account_picker::AccountPickerCommand::OpenAddReplaceFlow {
                provider_filter,
            } => self.open_account_add_replace_flow(provider_filter.as_deref()),
            crate::tui::account_picker::AccountPickerCommand::SubmitInput(input) => {
                self.input = input;
                self.cursor_pos = self.input.len();
                self.submit_input();
            }
            crate::tui::account_picker::AccountPickerCommand::PromptValue {
                prompt,
                command_prefix,
                empty_value,
                status_notice,
            } => self.prompt_account_value(prompt, command_prefix, empty_value, status_notice),
            crate::tui::account_picker::AccountPickerCommand::PromptNew { provider } => {
                match provider {
                    crate::tui::account_picker::AccountProviderKind::Anthropic => {
                        self.input = "/account claude add".to_string();
                        self.cursor_pos = self.input.len();
                        self.submit_input();
                    }
                    crate::tui::account_picker::AccountProviderKind::OpenAi => {
                        self.input = "/account openai add".to_string();
                        self.cursor_pos = self.input.len();
                        self.submit_input();
                    }
                }
            }
            other => {
                if let Some(input) = Self::account_command_for_picker(&other) {
                    self.input = input;
                    self.cursor_pos = self.input.len();
                    self.submit_input();
                }
            }
        }
    }

    pub(super) fn prompt_new_account_label(
        &mut self,
        provider: crate::tui::account_picker::AccountProviderKind,
    ) {
        let (provider_id, display_name) = match provider {
            crate::tui::account_picker::AccountProviderKind::Anthropic => {
                ("claude", "Anthropic/Claude")
            }
            crate::tui::account_picker::AccountProviderKind::OpenAi => ("openai", "OpenAI"),
        };
        self.push_display_message(DisplayMessage::system(format!(
            "Enter a label for the new {} account, then press Enter. Use `/cancel` to abort.",
            display_name
        )));
        self.set_status_notice(format!("Account: new {} label...", display_name));
        self.pending_account_input = Some(PendingAccountInput::NewAccountLabel {
            provider_id: provider_id.to_string(),
            display_name: display_name.to_string(),
        });
    }

    pub(super) fn account_command_for_picker(
        command: &crate::tui::account_picker::AccountPickerCommand,
    ) -> Option<String> {
        use crate::tui::account_picker::{AccountPickerCommand, AccountProviderKind};

        match command {
            AccountPickerCommand::SubmitInput(input) => Some(input.clone()),
            AccountPickerCommand::OpenAccountCenter { .. }
            | AccountPickerCommand::OpenAddReplaceFlow { .. }
            | AccountPickerCommand::PromptValue { .. }
            | AccountPickerCommand::PromptNew { .. } => None,
            AccountPickerCommand::Switch { provider, label } => Some(match provider {
                AccountProviderKind::Anthropic => format!("/account switch {}", label),
                AccountProviderKind::OpenAi => format!("/account openai switch {}", label),
            }),
            AccountPickerCommand::Login { provider, label } => Some(match provider {
                AccountProviderKind::Anthropic => format!("/account claude add {}", label),
                AccountProviderKind::OpenAi => format!("/account openai add {}", label),
            }),
            AccountPickerCommand::Remove { provider, label } => Some(match provider {
                AccountProviderKind::Anthropic => format!("/account claude remove {}", label),
                AccountProviderKind::OpenAi => format!("/account openai remove {}", label),
            }),
        }
    }

    pub(super) fn prompt_account_value(
        &mut self,
        prompt: String,
        command_prefix: String,
        empty_value: Option<String>,
        status_notice: String,
    ) {
        self.push_display_message(DisplayMessage::system(format!(
            "{} Use `/cancel` to abort.",
            prompt
        )));
        self.set_status_notice(status_notice.clone());
        self.pending_account_input = Some(PendingAccountInput::CommandValue {
            prompt,
            command_prefix,
            empty_value,
            status_notice,
        });
    }

    pub(super) fn handle_pending_account_input(
        &mut self,
        pending: PendingAccountInput,
        input: String,
    ) {
        let trimmed = input.trim();
        if trimmed == "/cancel" {
            self.push_display_message(DisplayMessage::system(
                "Account action cancelled.".to_string(),
            ));
            self.set_status_notice("Account: cancelled");
            return;
        }

        match pending {
            PendingAccountInput::NewAccountLabel {
                provider_id,
                display_name,
            } => {
                if trimmed.is_empty() {
                    self.push_display_message(DisplayMessage::error(
                        "Account label cannot be empty.".to_string(),
                    ));
                    self.pending_account_input = Some(PendingAccountInput::NewAccountLabel {
                        provider_id,
                        display_name,
                    });
                    return;
                }
                self.input = format!("/account {} add {}", provider_id, trimmed);
                self.cursor_pos = self.input.len();
                self.submit_input();
            }
            PendingAccountInput::CommandValue {
                prompt,
                command_prefix,
                empty_value,
                status_notice,
            } => {
                let value = if trimmed.is_empty() {
                    if let Some(value) = empty_value {
                        value
                    } else {
                        self.push_display_message(DisplayMessage::error(
                            "A value is required for this setting.".to_string(),
                        ));
                        self.pending_account_input = Some(PendingAccountInput::CommandValue {
                            prompt,
                            command_prefix,
                            empty_value: None,
                            status_notice,
                        });
                        return;
                    }
                } else {
                    trimmed.to_string()
                };
                self.input = format!("{} {}", command_prefix, value);
                self.cursor_pos = self.input.len();
                self.submit_input();
            }
        }
    }

    pub(super) fn next_account_picker_action(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> anyhow::Result<Option<crate::tui::account_picker::AccountPickerCommand>> {
        use crate::tui::account_picker::OverlayAction;

        let action = {
            let Some(picker_cell) = self.account_picker_overlay.as_ref() else {
                return Ok(None);
            };
            let mut picker = picker_cell.borrow_mut();
            picker.handle_overlay_key(code, modifiers)?
        };

        match action {
            OverlayAction::Continue => Ok(None),
            OverlayAction::Close => {
                self.account_picker_overlay = None;
                Ok(None)
            }
            OverlayAction::Execute(command) => {
                self.account_picker_overlay = None;
                Ok(Some(command))
            }
        }
    }

    pub(super) fn handle_login_picker_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> anyhow::Result<()> {
        use crate::tui::login_picker::OverlayAction;

        let action = {
            let Some(picker_cell) = self.login_picker_overlay.as_ref() else {
                return Ok(());
            };
            let mut picker = picker_cell.borrow_mut();
            picker.handle_overlay_key(code, modifiers)?
        };

        match action {
            OverlayAction::Continue => {}
            OverlayAction::Close => {
                self.login_picker_overlay = None;
            }
            OverlayAction::Execute(provider) => {
                self.login_picker_overlay = None;
                self.start_login_provider(provider);
            }
        }
        Ok(())
    }

    fn render_openai_accounts_markdown(&self) -> String {
        let accounts = crate::auth::codex::list_accounts().unwrap_or_default();
        let active_label = crate::auth::codex::active_account_label();
        let now_ms = chrono::Utc::now().timestamp_millis();

        if accounts.is_empty() {
            return "**OpenAI Accounts:** none configured\n\n\
                 Use `/account openai add` to add the next numbered account, or `/login openai` to refresh the active one."
                .to_string();
        }

        let mut lines = vec!["**OpenAI Accounts:**\n".to_string()];
        lines.push("| Account | Email | Status | ChatGPT Account ID | Active |".to_string());
        lines.push("|---------|-------|--------|--------------------|--------|".to_string());

        for account in &accounts {
            let is_active = active_label.as_deref() == Some(&account.label);
            let status = match account.expires_at {
                Some(expires_at) if expires_at > now_ms => "valid",
                Some(_) => "expired",
                None => "valid",
            };
            let email = account
                .email
                .as_deref()
                .map(mask_email)
                .unwrap_or_else(|| "unknown".to_string());
            let account_id = account.account_id.as_deref().unwrap_or("unknown");
            let active_mark = if is_active { "active" } else { "" };
            lines.push(format!(
                "| {} | {} | {} | {} | {} |",
                account.label, email, status, account_id, active_mark
            ));
        }

        lines.push(String::new());
        lines.push(
            "Commands: `/account openai switch <label>`, `/account openai add`, `/account openai remove <label>`"
                .to_string(),
        );

        lines.join("\n")
    }

    fn render_anthropic_accounts_markdown(&self) -> String {
        let accounts = crate::auth::claude::list_accounts().unwrap_or_default();
        let active_label = crate::auth::claude::active_account_label();
        let now_ms = chrono::Utc::now().timestamp_millis();

        if accounts.is_empty() {
            return "**Anthropic Accounts:** none configured\n\n\
                 Use `/account claude add` to add the next numbered account, or `/login claude` to refresh the active one."
                .to_string();
        }

        let mut lines = vec!["**Anthropic Accounts:**\n".to_string()];
        lines.push("| Account | Email | Status | Subscription | Active |".to_string());
        lines.push("|---------|-------|--------|-------------|--------|".to_string());

        for account in &accounts {
            let is_active = active_label.as_deref() == Some(&account.label);
            let status = if account.expires > now_ms {
                "valid"
            } else {
                "expired"
            };
            let email = account
                .email
                .as_deref()
                .map(mask_email)
                .unwrap_or_else(|| "unknown".to_string());
            let sub = account.subscription_type.as_deref().unwrap_or("unknown");
            let active_mark = if is_active { "active" } else { "" };
            lines.push(format!(
                "| {} | {} | {} | {} | {} |",
                account.label, email, status, sub, active_mark
            ));
        }

        lines.push(String::new());
        lines.push(
            "Commands: `/account claude switch <label>`, `/account claude add`, `/account claude remove <label>`"
                .to_string(),
        );

        lines.join("\n")
    }

    fn append_anthropic_account_picker_items(
        &self,
        items: &mut Vec<crate::tui::account_picker::AccountPickerItem>,
        provider: crate::provider_catalog::LoginProviderDescriptor,
    ) {
        let active_label = crate::auth::claude::active_account_label();
        let now_ms = chrono::Utc::now().timestamp_millis();
        for account in crate::auth::claude::list_accounts().unwrap_or_default() {
            let status = if account.expires > now_ms {
                "valid"
            } else {
                "expired"
            };
            let email = account
                .email
                .as_deref()
                .map(mask_email)
                .unwrap_or_else(|| "unknown".to_string());
            let plan = account.subscription_type.as_deref().unwrap_or("unknown");
            let label = account.label.clone();
            let active_suffix = if active_label.as_deref() == Some(label.as_str()) {
                " - active"
            } else {
                ""
            };
            items.push(crate::tui::account_picker::AccountPickerItem::action(
                provider.id,
                provider.display_name,
                format!("Switch account `{label}`"),
                format!("{email} - {status} - plan {plan}{active_suffix}"),
                crate::tui::account_picker::AccountPickerCommand::SubmitInput(format!(
                    "/account {} switch {}",
                    provider.id, label
                )),
            ));
            items.push(crate::tui::account_picker::AccountPickerItem::action(
                provider.id,
                provider.display_name,
                format!("Re-login account `{label}`"),
                format!("Refresh OAuth tokens for `{label}`"),
                crate::tui::account_picker::AccountPickerCommand::SubmitInput(format!(
                    "/account {} add {}",
                    provider.id, label
                )),
            ));
            items.push(crate::tui::account_picker::AccountPickerItem::action(
                provider.id,
                provider.display_name,
                format!("Remove account `{label}`"),
                format!("Delete saved credentials for `{label}`"),
                crate::tui::account_picker::AccountPickerCommand::SubmitInput(format!(
                    "/account {} remove {}",
                    provider.id, label
                )),
            ));
        }
    }

    fn append_openai_account_picker_items(
        &self,
        items: &mut Vec<crate::tui::account_picker::AccountPickerItem>,
        provider: crate::provider_catalog::LoginProviderDescriptor,
    ) {
        let active_label = crate::auth::codex::active_account_label();
        let now_ms = chrono::Utc::now().timestamp_millis();
        for account in crate::auth::codex::list_accounts().unwrap_or_default() {
            let status = match account.expires_at {
                Some(expires_at) if expires_at > now_ms => "valid",
                Some(_) => "expired",
                None => "valid",
            };
            let email = account
                .email
                .as_deref()
                .map(mask_email)
                .unwrap_or_else(|| "unknown".to_string());
            let account_id = account.account_id.as_deref().unwrap_or("unknown");
            let label = account.label.clone();
            let active_suffix = if active_label.as_deref() == Some(label.as_str()) {
                " - active"
            } else {
                ""
            };
            items.push(crate::tui::account_picker::AccountPickerItem::action(
                provider.id,
                provider.display_name,
                format!("Switch account `{label}`"),
                format!("{email} - {status} - acct {account_id}{active_suffix}"),
                crate::tui::account_picker::AccountPickerCommand::SubmitInput(format!(
                    "/account {} switch {}",
                    provider.id, label
                )),
            ));
            items.push(crate::tui::account_picker::AccountPickerItem::action(
                provider.id,
                provider.display_name,
                format!("Re-login account `{label}`"),
                format!("Refresh OpenAI OAuth tokens for `{label}`"),
                crate::tui::account_picker::AccountPickerCommand::SubmitInput(format!(
                    "/account {} add {}",
                    provider.id, label
                )),
            ));
            items.push(crate::tui::account_picker::AccountPickerItem::action(
                provider.id,
                provider.display_name,
                format!("Remove account `{label}`"),
                format!("Delete saved credentials for `{label}`"),
                crate::tui::account_picker::AccountPickerCommand::SubmitInput(format!(
                    "/account {} remove {}",
                    provider.id, label
                )),
            ));
        }
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
                    "Switched to Anthropic account `{}`.",
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
                "Account label `{}` exists for both Anthropic and OpenAI. Use `/account switch {}` or `/account openai switch {}` explicitly.",
                label, label, label
            ))),
            (false, false) => self.push_display_message(DisplayMessage::error(format!(
                "No Anthropic or OpenAI account with label `{}` found.",
                label
            ))),
        }
    }

    pub(super) fn remove_account(&mut self, label: &str) {
        match crate::auth::claude::remove_account(label) {
            Ok(()) => {
                self.push_display_message(DisplayMessage::system(format!(
                    "Removed Anthropic account `{}`.",
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
                    "Switched to OpenAI account `{}`.",
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
                    "Removed OpenAI account `{}`.",
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
        let browser_opened = open::that(&auth_url).is_ok();
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
                "Waiting for callback on `localhost:{}`... (this will complete automatically)\n",
                port
            )
        } else {
            format!(
                "Local callback port `localhost:{}` is unavailable, so finish in any browser and paste the full callback URL here.\n",
                port
            )
        };
        let browser_line = if browser_opened {
            String::new()
        } else {
            "This machine could not open a browser automatically.\n".to_string()
        };

        self.push_display_message(DisplayMessage::system(format!(
            "**OpenAI OAuth Login** (account: `{}`)\n\n\
             Opening browser for authentication...\n\n\
             If the browser didn't open, visit:\n{}\n\n\
             **Note:** Wait a few seconds for the page to fully load before clicking Continue. \
             OpenAI's verification system may briefly disable the button.\n\n\
             {}{}\
             Or paste the full callback URL or query string here to finish from another device.{}",
            label, auth_url, browser_line, callback_line, qr_section
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

        let browser_opened = open::that(&auth_url).is_ok();
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
                "Waiting for callback on `{}`... (this will complete automatically)\n",
                redirect_uri
            )
        } else {
            "Finish login in any browser, then paste the callback URL or authorization code here.\n"
                .to_string()
        };
        let browser_line = if browser_opened {
            String::new()
        } else {
            "This machine could not open a browser automatically.\n".to_string()
        };

        self.push_display_message(DisplayMessage::system(format!(
            "**Gemini OAuth Login**\n\n\
             Opening browser for authentication...\n\n\
             If the browser didn't open, visit:\n{}\n\n\
             {}{}\
             Or paste the full callback URL, query string, or authorization code here to finish.{}",
            auth_url, browser_line, callback_line, qr_section
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

    fn start_openai_compatible_profile_login(
        &mut self,
        profile: crate::provider_catalog::OpenAiCompatibleProfile,
    ) {
        if profile.id == crate::provider_catalog::OPENAI_COMPAT_PROFILE.id {
            let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
            self.push_display_message(DisplayMessage::system(format!(
                "**{} Endpoint**\n\n\
                 Setup docs: {}\n\
                 Current API base: `{}`\n\n\
                 **Paste the API base below**. Press Enter to keep the current value.",
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
            .map(|m| format!("Suggested default model: `{}`\n\n", m))
            .unwrap_or_default();
        let endpoint_hint = endpoint
            .map(|endpoint| format!("Endpoint: `{}`\n", endpoint))
            .unwrap_or_default();
        let prompt = if api_key_optional {
            "**Paste your API key below** if your endpoint requires one. Press Enter to skip."
        } else {
            "**Paste your API key below** (it will be saved securely)."
        };
        self.push_display_message(DisplayMessage::system(format!(
            "**{} {}**\n\n\
             Setup docs: {}\n\
             Stored variable: `{}`\n\
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

    fn start_cursor_login(&mut self) {
        let binary = crate::auth::cursor::cursor_agent_cli_path();

        crate::telemetry::record_auth_started("cursor", "cli");

        if crate::auth::cursor::has_cursor_agent_cli() {
            self.push_display_message(DisplayMessage::system(format!(
                "**Cursor Login**\n\n\
                 Running `{} login` to open browser authentication.\n\n\
                 If that fails, jcode will fall back to saving a Cursor API key for `cursor-agent`.",
                binary
            )));
            self.set_status_notice("Login: cursor browser...");

            match crate::auth::login_flows::run_external_login_command_with_terminal_handoff(
                &binary,
                &["login"],
            ) {
                Ok(()) => {
                    crate::telemetry::record_auth_success("cursor", "cli");
                    self.push_display_message(DisplayMessage::system(
                        "Cursor login completed.".to_string(),
                    ));
                    self.set_status_notice("Login: cursor ready");
                    crate::auth::AuthStatus::invalidate_cache();
                    return;
                }
                Err(e) => {
                    crate::telemetry::record_auth_failed("cursor", "cli");
                    self.push_display_message(DisplayMessage::error(format!(
                        "Cursor CLI login failed: {}\n\nFalling back to API key mode...",
                        e
                    )));
                }
            }
        }

        self.push_display_message(DisplayMessage::system(
            "**Cursor API Key**\n\n\
             Get your API key from: https://cursor.com/settings\n\
             (Dashboard > Integrations > User API Keys)\n\n\
             jcode will save it securely and provide it to `cursor-agent` at runtime.\n\
             You still need Cursor Agent installed to use the Cursor provider.\n\n\
             **Paste your API key below**."
                .to_string(),
        ));
        self.set_status_notice("Login: paste cursor key...");
        self.begin_pending_login(PendingLogin::CursorApiKey);
    }

    fn start_copilot_login(&mut self) {
        self.set_status_notice("Login: copilot device flow...");
        self.begin_pending_login(PendingLogin::Copilot);

        tokio::spawn(async move {
            let client = reqwest::Client::new();

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
                        "**GitHub Copilot Login**\n\n\
                         Your code: **{}**{}\n\n\
                         Opening browser to {} ...\n\
                         Paste the code there and authorize.{}\n\n\
                         Waiting for authorization... (type `/cancel` to abort)",
                        user_code, clipboard_msg, verification_uri, qr_section
                    )
                },
            }));

            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let _ = open::that_detached(&verification_uri);

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
                            "Authenticated as **{}** via GitHub Copilot.\n\n\
                             Copilot models are now available in `/model`.",
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
            "**GitHub Copilot Login**\n\n\
             Starting device flow... please wait."
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
        let browser_opened = open::that(&auth_url).is_ok();

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
                "Waiting for callback on `{}`... (this will complete automatically)\n",
                redirect_uri
            )
        } else {
            format!(
                "Local callback port `{}` is unavailable, so finish in any browser and paste the full callback URL or query string here.\n",
                redirect_uri
            )
        };
        let browser_line = if browser_opened {
            String::new()
        } else {
            "This machine could not open a browser automatically.\n".to_string()
        };
        let manual_hint = "If the browser ends on a loopback/callback error page, copy the full URL from the address bar and paste it here immediately.\n";

        self.push_display_message(DisplayMessage::system(format!(
            "**Antigravity OAuth Login**\n\n\
             Opening browser for authentication...\n\n\
             If the browser didn't open, visit:\n{}\n\n\
             {}{}{}\
             Or paste the full callback URL or query string here to finish.{}",
            auth_url, browser_line, callback_line, manual_hint, qr_section
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
                    "Auto import is waiting for your selection. Reply with `a` to approve all, `1,3` to approve specific sources, or `/cancel` to abort.".to_string()
                }
                _ => "Login still in progress. Complete it in your browser, or paste the callback URL / authorization code here. Type `/cancel` to abort.".to_string(),
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
            PendingLogin::Gemini {
                expected_state: Some(_),
                ..
            } if !looks_like_oauth_callback_input(trimmed) => {
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
                    "Exchanging authorization code for account `{}`...",
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
                    "Exchanging OpenAI callback for account `{}`...",
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
                if key_name == "OPENROUTER_API_KEY" && !key.starts_with("sk-or-") {
                    self.push_display_message(DisplayMessage::system(
                        "OpenRouter keys typically start with `sk-or-`. Saving anyway..."
                            .to_string(),
                    ));
                }

                let resolved_openai_compatible = openai_compatible_profile
                    .map(crate::provider_catalog::resolve_openai_compatible_profile);

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
                    } else {
                        Self::save_named_api_key(&env_file, &key_name, &key)
                    };

                match save_result {
                    Ok(()) => {
                        crate::auth::AuthStatus::invalidate_cache();
                        if let Some(profile) = openai_compatible_profile {
                            crate::provider_catalog::apply_openai_compatible_profile_env(Some(
                                profile,
                            ));
                            crate::cli::provider_init::lock_model_provider("openrouter");
                            if let Some(default_model) = resolved_openai_compatible
                                .as_ref()
                                .and_then(|resolved| resolved.default_model.as_deref())
                                .or(default_model.as_deref())
                            {
                                crate::env::set_var("JCODE_OPENROUTER_MODEL", default_model);
                            }
                        }

                        let effective_default_model = resolved_openai_compatible
                            .as_ref()
                            .and_then(|resolved| resolved.default_model.as_deref())
                            .or(default_model.as_deref());
                        let model_hint = effective_default_model
                            .map(|m| format!("\nSuggested default model: `{}`", m))
                            .unwrap_or_default();
                        let guidance = if key_name == crate::subscription_catalog::JCODE_API_KEY_ENV
                        {
                            format!(
                                "Use `--provider jcode` or `/login jcode` to access curated models via your router.\nDocs: {}",
                                docs_url
                            )
                        } else if let Some(resolved) = resolved_openai_compatible.as_ref() {
                            if resolved.requires_api_key {
                                format!(
                                    "Restart with `--provider {}` to use this backend in a new session.",
                                    provider.to_lowercase().replace(' ', "-")
                                )
                            } else {
                                format!(
                                    "Local endpoint configured at `{}`. Restart with `--provider {}` to use this backend in a new session.",
                                    endpoint.as_deref().unwrap_or(resolved.api_base.as_str()),
                                    provider.to_lowercase().replace(' ', "-")
                                )
                            }
                        } else if key_name == "OPENROUTER_API_KEY" {
                            "You can now use `/model` to switch to OpenRouter models.".to_string()
                        } else {
                            format!(
                                "Restart with `--provider {}` to use this backend in a new session.",
                                provider.to_lowercase().replace(' ', "-")
                            )
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
                                "**{}.**\n\n\
                                 Stored at `~/.config/jcode/{}`.\n\
                                 {}{}",
                                saved_label, env_file, guidance, model_hint
                            ),
                        }));
                    }
                    Err(e) => {
                        crate::telemetry::record_auth_failed(&provider_id, &auth_method);
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
                            message: "**Cursor API key saved.**\n\n\
                             Stored at `~/.config/jcode/cursor.env`.\n\
                             jcode will pass it to `cursor-agent` automatically.\n\
                             Install Cursor Agent if it is not already on PATH."
                                .to_string(),
                        }));
                    }
                    Err(e) => {
                        crate::telemetry::record_auth_failed("cursor", "api_key");
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
                     Complete the login in your browser, or type `/cancel` to abort."
                        .to_string(),
                ));
                self.pending_login = Some(PendingLogin::Copilot);
            }
            PendingLogin::AutoImportSelection { candidates } => {
                let selected = match crate::cli::provider_init::parse_external_auth_review_selection(
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
                    match crate::cli::provider_init::run_external_auth_auto_import_candidates(
                        &candidates,
                        &selected,
                    )
                    .await
                    {
                        Ok(outcome) => {
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

    pub(super) fn handle_login_completed(&mut self, login: LoginCompleted) {
        if login.provider == "copilot_code" {
            self.push_display_message(DisplayMessage::system(login.message.clone()));
            if let Some(code) = login
                .message
                .split("Enter code: **")
                .nth(1)
                .and_then(|s| s.split("**").next())
            {
                self.set_status_notice(format!("Login: enter {} at GitHub", code));
            }
            return;
        }
        crate::auth::AuthStatus::invalidate_cache();
        if let Some((provider, method)) = self
            .pending_login
            .as_ref()
            .and_then(PendingLogin::telemetry_context)
        {
            if login.success {
                crate::telemetry::record_auth_success(&provider, &method);
            } else {
                crate::telemetry::record_auth_failed(&provider, &method);
            }
        }
        if login.success {
            self.push_display_message(DisplayMessage::system(login.message));
            self.set_status_notice(format!("Login: {} ready", login.provider));
            self.provider.on_auth_changed();
        } else {
            self.push_display_message(DisplayMessage::error(login.message));
            self.set_status_notice(format!("Login: {} failed", login.provider));
        }
        if self.pending_login.is_some() {
            self.pending_login = None;
        }
    }

    pub(super) fn handle_update_status(&mut self, status: crate::bus::UpdateStatus) {
        use crate::bus::UpdateStatus;
        match status {
            UpdateStatus::Checking => {
                self.set_status_notice("Checking for updates...");
            }
            UpdateStatus::Available { current, latest } => {
                self.set_status_notice(format!("Update available: {} → {}", current, latest));
            }
            UpdateStatus::Downloading { version } => {
                self.set_status_notice(format!("⬇️  Downloading {}...", version));
            }
            UpdateStatus::Installed { version } => {
                self.set_status_notice(format!("✅ Updated to {} — restarting", version));
            }
            UpdateStatus::UpToDate => {}
            UpdateStatus::Error(e) => {
                self.set_status_notice(format!("Update failed: {}", e));
            }
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
        std::fs::create_dir_all(&config_dir)?;
        crate::platform::set_directory_permissions_owner_only(&config_dir)?;

        let file_path = config_dir.join(env_file);
        let content = format!("{}={}\n", key_name, key);
        std::fs::write(&file_path, &content)?;

        crate::platform::set_permissions_owner_only(&file_path)?;

        crate::env::set_var(key_name, key);
        Ok(())
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
mod tests {
    use super::{
        antigravity_input_requires_state_validation, save_tui_openai_compatible_api_base,
        save_tui_openai_compatible_key,
    };

    #[test]
    fn antigravity_auto_callback_code_skips_manual_callback_parser() {
        assert!(!antigravity_input_requires_state_validation(
            "raw_authorization_code",
            Some("expected_state")
        ));
    }

    #[test]
    fn antigravity_manual_callback_url_keeps_state_validation() {
        assert!(antigravity_input_requires_state_validation(
            "http://127.0.0.1:51121/oauth-callback?code=abc&state=expected_state",
            Some("expected_state")
        ));
    }

    #[test]
    fn tui_openai_compatible_api_base_accepts_localhost_override() {
        let _env_guard = crate::storage::lock_test_env();
        let resolved = save_tui_openai_compatible_api_base("http://localhost:11434/v1")
            .expect("save api base");
        assert_eq!(resolved.api_base, "http://localhost:11434/v1");
        assert!(!resolved.requires_api_key);
    }

    #[test]
    fn tui_openai_compatible_local_key_save_allows_empty_key() {
        let _env_guard = crate::storage::lock_test_env();
        let resolved = save_tui_openai_compatible_key(crate::provider_catalog::OLLAMA_PROFILE, "")
            .expect("save local key");
        assert_eq!(resolved.api_base, "http://localhost:11434/v1");
        assert!(
            crate::provider_catalog::openai_compatible_profile_is_configured(
                crate::provider_catalog::OLLAMA_PROFILE
            )
        );
        assert!(
            crate::provider_catalog::load_api_key_from_env_or_config(
                &resolved.api_key_env,
                &resolved.env_file,
            )
            .is_none()
        );
    }
}
