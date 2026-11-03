use super::*;

impl App {
    pub(crate) fn open_account_center(&mut self, provider_filter: Option<&str>) {
        if crate::tui::is_ssh_remote() {
            self.open_remote_account_picker(provider_filter);
            return;
        }
        use crate::tui::account_picker::{AccountPicker, AccountPickerCommand, AccountPickerItem};

        crate::telemetry::record_setup_step_once("account_center_opened");

        super::auth_account_picker_saved_accounts::start_account_usage_refresh();

        let status = crate::auth::AuthStatus::check_fast();
        let validation = crate::auth::validation::load_all();
        let cfg = crate::config::Config::load();
        let providers: Vec<_> = match provider_filter {
            Some(provider_id) => match resolve_account_provider_descriptor(provider_id) {
                Some(provider) => vec![provider],
                None => {
                    self.push_display_message(DisplayMessage::error(format!(
                        "Unknown provider {}.",
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
                "Choose a provider to add another saved login. Existing accounts are kept."
                    .to_string()
            } else if provider_scope.is_some() {
                format!("{scoped_saved_accounts} saved logins. Sign in to add another account.")
            } else {
                format!(
                    "Saved logins: Claude {}, OpenAI {}. Choose a provider to add another account.",
                    claude_accounts.len(),
                    openai_accounts.len()
                )
            };
            items.push(AccountPickerItem::action(
                "account-flow",
                "Accounts",
                "Add account",
                detail,
                AccountPickerCommand::OpenAddReplaceFlow {
                    provider_filter: provider_scope.clone(),
                },
            ));
        }

        items.push(AccountPickerItem::action(
            provider_scope.as_deref().unwrap_or("auth-doctor"),
            provider_scope
                .as_deref()
                .unwrap_or("Authentication")
                .to_string(),
            "Diagnose login",
            if provider_scope.is_some() {
                "review status, validation, and recommended next steps".to_string()
            } else {
                "review all configured providers and recovery steps".to_string()
            },
            AccountPickerCommand::SubmitInput(match provider_scope.as_deref() {
                Some(provider_id) => format!("/accounts {} doctor", provider_id),
                None => "/auth doctor".to_string(),
            }),
        ));

        for provider in providers {
            let assessment = status.assessment_for_provider(provider);
            let auth_state = assessment.state;
            let method_detail = assessment.method_detail.as_str();
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

            items.push(AccountPickerItem::action(
                provider.id,
                provider.display_name,
                if provider.id == "openai" {
                    "Usage & provider settings"
                } else {
                    "Provider settings"
                },
                format!(
                    "{} - {} - {}",
                    state_label, method_detail, validation_detail
                ),
                AccountPickerCommand::SubmitInput(format!("/accounts {} settings", provider.id)),
            ));
            items.push(AccountPickerItem::action(
                provider.id,
                provider.display_name,
                if matches!(auth_state, crate::auth::AuthState::NotConfigured) {
                    "Sign in"
                } else {
                    "Refresh login"
                },
                provider.menu_detail,
                AccountPickerCommand::SubmitInput(format!("/accounts {} login", provider.id)),
            ));
            if provider_scope.is_none() {
                items.push(AccountPickerItem::action(
                    provider.id,
                    provider.display_name,
                    "Diagnose login",
                    format!("{} - {}", state_label, validation_detail),
                    AccountPickerCommand::SubmitInput(format!("/accounts {} doctor", provider.id)),
                ));
            }

            match provider.id {
                "claude-code" => {
                    for instance in crate::auth::claude_code::instances() {
                        let identity = crate::auth::claude_code::instance_identity(&instance);
                        let email = identity
                            .as_ref()
                            .and_then(|value| value.email.as_deref())
                            .unwrap_or("not signed in");
                        let plan = identity
                            .as_ref()
                            .and_then(|value| value.plan.as_deref())
                            .unwrap_or("unknown plan");
                        items.push(AccountPickerItem::action(
                            provider.id, provider.display_name,
                            format!("Use {} in this window", instance.display_label()),
                            format!("{email} - {plan}{}", self.account_row_scope_suffix(provider.id, &instance.id)),
                            AccountPickerCommand::SubmitInput(format!("/accounts claude-code switch {}", instance.id)),
                        ).with_details(vec![("Usage".to_string(),
                            "Managed by the Claude Code instance. Remaining quota is not reported here.".to_string())]));
                    }
                }
                "claude" => {
                    self.append_anthropic_account_picker_items(&mut items, provider);
                    items.push(AccountPickerItem::action(
                        provider.id,
                        provider.display_name,
                        "Add new account",
                        "Sign in to another Claude account. Existing saved accounts are kept.",
                        AccountPickerCommand::SubmitInput("/accounts claude add".to_string()),
                    ));
                }
                "openai" => {
                    self.append_openai_account_picker_items(&mut items, provider);
                    items.push(AccountPickerItem::action(
                        provider.id,
                        provider.display_name,
                        "Add new account",
                        "Sign in to another OpenAI account. Existing saved accounts are kept.",
                        AccountPickerCommand::SubmitInput("/accounts openai add".to_string()),
                    ));
                    items.push(AccountPickerItem::action(
                        provider.id,
                        provider.display_name,
                        "Transport",
                        cfg.provider.openai_transport.as_deref().unwrap_or("auto"),
                        AccountPickerCommand::PromptValue {
                            prompt: "Enter OpenAI transport: auto, https, or websocket."
                                .to_string(),
                            command_prefix: "/accounts openai transport".to_string(),
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
                            prompt: "Enter OpenAI reasoning effort: none, minimal, low, medium, high, xhigh, max, or clear.".to_string(),
                            command_prefix: "/accounts openai effort".to_string(),
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
                            "/accounts openai fast {}",
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
                        "Step 1: API base URL",
                        compat.api_base,
                        AccountPickerCommand::PromptValue {
                            prompt: "Step 1/2: enter the OpenAI-compatible API base URL, for example https://llm.example.com/v1.".to_string(),
                            command_prefix: "/accounts openai-compatible api-base".to_string(),
                            empty_value: Some("clear".to_string()),
                            status_notice: "Account: editing OpenAI-compatible API base URL (step 1/2)...".to_string(),
                        },
                    ));
                    items.push(AccountPickerItem::action(
                        provider.id,
                        provider.display_name,
                        "Step 2: API key variable",
                        compat.api_key_env,
                        AccountPickerCommand::PromptValue {
                            prompt: "Step 2/2: enter the env var name that stores the API key, for example OPENAI_API_KEY.".to_string(),
                            command_prefix: "/accounts openai-compatible api-key-name".to_string(),
                            empty_value: Some("clear".to_string()),
                            status_notice: "Account: editing OpenAI-compatible API key variable (step 2/2)...".to_string(),
                        },
                    ));
                    items.push(AccountPickerItem::action(
                        provider.id,
                        provider.display_name,
                        "Env file",
                        compat.env_file,
                        AccountPickerCommand::PromptValue {
                            prompt: "Enter the env file name for this profile.".to_string(),
                            command_prefix: "/accounts openai-compatible env-file".to_string(),
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
                            command_prefix: "/accounts openai-compatible default-model".to_string(),
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
                            command_prefix: "/accounts copilot premium".to_string(),
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
                command_prefix: "/accounts default-provider".to_string(),
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
                command_prefix: "/accounts default-model".to_string(),
                empty_value: Some("clear".to_string()),
                status_notice: "Account: editing default model...".to_string(),
            },
        ));

        let title = match provider_filter {
            Some(provider_id) => format!(" {} account center ", provider_id),
            None => " Accounts ".to_string(),
        };
        let mut picker = AccountPicker::with_summary(title, items, summary)
            .with_account_menus()
            .with_manager_scope(provider_scope);
        let current_provider = if self.is_remote {
            self.remote_provider_name.as_deref().unwrap_or_default()
        } else {
            self.provider.name()
        };
        let provider_id =
            super::window_account::account_family(current_provider).unwrap_or(current_provider);
        picker.select_account(
            provider_id,
            self.window_account_label(provider_id).as_deref(),
        );
        self.account_picker_overlay = Some(std::cell::RefCell::new(picker));
        self.inline_interactive_state = None;
        self.input.clear();
        self.cursor_pos = 0;
        self.set_status_notice("Accounts: choose an account to manage, or add an account");
    }
}
