use super::*;

impl App {
    pub(crate) fn open_account_center(&mut self, provider_filter: Option<&str>) {
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
                Some(provider_id) => format!("/account {} doctor", provider_id),
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
            items.push(AccountPickerItem::action(
                provider.id,
                provider.display_name,
                "Diagnose login",
                format!("{} - {}", state_label, validation_detail),
                AccountPickerCommand::SubmitInput(format!("/account {} doctor", provider.id)),
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
                        "Step 1: API base URL",
                        compat.api_base,
                        AccountPickerCommand::PromptValue {
                            prompt: "Step 1/2: enter the OpenAI-compatible API base URL, for example https://llm.example.com/v1.".to_string(),
                            command_prefix: "/account openai-compatible api-base".to_string(),
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
                            command_prefix: "/account openai-compatible api-key-name".to_string(),
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

    pub(crate) fn open_account_add_replace_flow(&mut self, provider_filter: Option<&str>) {
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

    pub(crate) fn open_account_picker(&mut self, provider_filter: Option<&str>) {
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

    pub(crate) fn should_open_inline_account_picker(&self, provider_filter: Option<&str>) -> bool {
        provider_filter.is_none()
            || self
                .inline_account_picker_scope_key(provider_filter)
                .is_some()
    }

    pub(crate) fn inline_account_picker_scope_key(
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

    pub(crate) fn inline_account_picker_provider_id(
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

    pub(crate) fn handle_account_picker_command(
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

    pub(crate) fn prompt_new_account_label(
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

    pub(crate) fn account_command_for_picker(
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

    pub(crate) fn prompt_account_value(
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

    pub(crate) fn handle_pending_account_input(
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

    pub(crate) fn next_account_picker_action(
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
}
