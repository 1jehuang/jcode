#[path = "auth_account_picker_input.rs"]
mod input;

#[path = "auth_account_add_replace.rs"]
mod add_replace;
#[path = "auth_account_picker_inline_claude.rs"]
mod inline_claude;
#[path = "auth_account_picker_workflow.rs"]
mod workflow;

use super::auth_account_picker_saved_accounts::{
    account_picker_display_name, anthropic_account_use,
};
use super::*;

impl App {
    pub(crate) fn open_account_center(&mut self, provider_filter: Option<&str>) {
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

    pub(crate) fn poll_account_picker_usage(&mut self) -> bool {
        let Some(picker) = self.account_picker_overlay.as_ref() else {
            return false;
        };
        if !super::auth_account_picker_saved_accounts::poll_account_usage_refresh() {
            return false;
        }
        picker.borrow_mut().update_account_details(
            super::auth_account_picker_saved_accounts::decorate_account_usage_items,
        );
        true
    }

    pub(crate) fn open_account_picker(&mut self, provider_filter: Option<&str>) {
        let Some(scope_key) = self.inline_account_picker_scope_key(provider_filter) else {
            if let Some(provider_id) = provider_filter {
                self.push_display_message(DisplayMessage::system(format!(
                    "Inline /accounts picker is only available for Claude and OpenAI accounts. Use /accounts {} settings for provider details.",
                    provider_id
                )));
            } else {
                self.push_display_message(DisplayMessage::system(
                    "Inline /accounts picker is available for Claude and OpenAI accounts. Use /accounts claude or /accounts openai to choose explicitly.".to_string(),
                ));
            }
            self.set_status_notice("Account picker unavailable");
            return;
        };

        let current_provider = if self.is_remote {
            self.remote_provider_name.clone().unwrap_or_default()
        } else {
            self.provider.name().to_string()
        };
        let current_is_claude_code = super::window_account::account_family(&current_provider)
            == Some(super::window_account::CLAUDE_CODE_FAMILY);
        let include_claude_code =
            current_is_claude_code || !crate::auth::claude_code::settings().instances.is_empty();
        let provider_label = match scope_key.as_str() {
            "all" if include_claude_code => "Claude + OpenAI + Claude Code",
            "all" => "Claude + OpenAI",
            "claude" => "Claude",
            "openai" => "OpenAI",
            super::window_account::CLAUDE_CODE_FAMILY => "Claude Code",
            _ => scope_key.as_str(),
        };

        let (models, selected) = match scope_key.as_str() {
            "all" => {
                let (mut models, mut selected) = self.build_all_inline_account_picker();
                // Include the implicit instance when this window uses Claude
                // Code, and focus its current instance instead of another family.
                if !include_claude_code {
                    (models, selected)
                } else {
                    let (cc, cc_selected) = self.build_claude_code_inline_account_picker();
                    if current_is_claude_code && !cc.is_empty() {
                        selected = models.len() + cc_selected;
                    }
                    models.extend(cc);
                    (models, selected)
                }
            }
            "claude" => self.build_claude_inline_account_picker(),
            "openai" => self.build_openai_inline_account_picker(),
            super::window_account::CLAUDE_CODE_FAMILY => {
                self.build_claude_code_inline_account_picker()
            }
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
            scoped_route_restore: Vec::new(),
            effort_step: None,
        });
        self.input.clear();
        self.cursor_pos = 0;
        self.set_status_notice(format!(
            "Account → {} (Enter: use in this window · Ctrl+D: make default)",
            provider_label
        ));
    }

    pub(crate) fn inline_account_picker_scope_key(
        &self,
        provider_filter: Option<&str>,
    ) -> Option<String> {
        if let Some(filter) = provider_filter {
            return match filter.to_ascii_lowercase().as_str() {
                "claude" | "anthropic" => Some("claude".to_string()),
                "openai" => Some("openai".to_string()),
                "claude-code" | "claude_code" => {
                    Some(super::window_account::CLAUDE_CODE_FAMILY.to_string())
                }
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
            super::window_account::CLAUDE_CODE_FAMILY => {
                Some(super::window_account::CLAUDE_CODE_FAMILY.to_string())
            }
            _ => None,
        }
    }

    /// Claude Code CLI instances from `[provider.claude_code]`: one row per
    /// instance with the email/plan Claude Code cached for its login. Enter
    /// pins the instance for this window; logins happen in a terminal.
    fn build_claude_code_inline_account_picker(&self) -> (Vec<crate::tui::PickerEntry>, usize) {
        let family = super::window_account::CLAUDE_CODE_FAMILY;
        let instances = crate::auth::claude_code::instances();
        let active = self
            .window_account_label(family)
            .unwrap_or_else(crate::auth::claude_code::default_instance_id);
        let mut models = Vec::with_capacity(instances.len());
        let mut selected = 0usize;
        for instance in &instances {
            let is_active = instance.id == active;
            if is_active {
                selected = models.len();
            }
            let identity = crate::auth::claude_code::instance_identity(instance);
            let email = identity
                .as_ref()
                .and_then(|identity| identity.email.as_deref())
                .map(mask_email)
                .unwrap_or_else(|| "not signed in".to_string());
            let plan = identity
                .as_ref()
                .and_then(|identity| identity.plan.clone())
                .unwrap_or_else(|| "unknown".to_string());
            let home = instance
                .resolved_home()
                .map(|home| home.display().to_string())
                .unwrap_or_else(|| "default login".to_string());
            models.push(crate::tui::PickerEntry {
                name: format!("Claude Code · {}", instance.display_label()),
                options: vec![crate::tui::PickerOption {
                    provider: "Claude Code".to_string(),
                    api_method: self.account_row_badge(family, &instance.id).0.to_string(),
                    available: true,
                    detail: format!("{} - plan {} - {} - {}", email, plan, instance.id, home),
                    estimated_reference_cost_micros: None,
                }],
                action: crate::tui::PickerAction::Account(
                    crate::tui::AccountPickerAction::Switch {
                        provider_id: family.to_string(),
                        label: instance.id.clone(),
                    },
                ),
                selected_option: 0,
                is_current: is_active,
                is_default: false,
                is_favorite: false,
                recommended: false,
                recommendation_rank: usize::MAX,
                usage_score: 0,
                old: false,
                created_date: None,
                effort: None,
            });
        }
        (models, selected)
    }

    fn build_all_inline_account_picker(&self) -> (Vec<crate::tui::PickerEntry>, usize) {
        let claude_accounts = crate::auth::claude::list_accounts().unwrap_or_default();
        let openai_accounts = crate::auth::codex::list_accounts().unwrap_or_default();
        // "Active" here means the account THIS window uses.
        let claude_active = self
            .window_account_label("claude")
            .unwrap_or_else(crate::auth::claude::primary_account_label);
        let openai_active = self
            .window_account_label("openai")
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
                name: account_picker_display_name(
                    "Claude",
                    &account.label,
                    claude_accounts.len(),
                    account.email.as_deref(),
                ),
                options: vec![crate::tui::PickerOption {
                    provider: "Claude".to_string(),
                    api_method: self
                        .account_row_badge("claude", &account.label)
                        .0
                        .to_string(),
                    available: true,
                    detail: format!(
                        "{} - {} - {} - plan {}",
                        email,
                        anthropic_account_use(account.subscription_type.as_deref()),
                        status,
                        plan
                    ),
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
                is_favorite: false,
                recommended: false,
                recommendation_rank: usize::MAX,
                usage_score: 0,
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
                name: account_picker_display_name(
                    "OpenAI",
                    &account.label,
                    openai_accounts.len(),
                    account.email.as_deref(),
                ),
                options: vec![crate::tui::PickerOption {
                    provider: "OpenAI".to_string(),
                    api_method: self
                        .account_row_badge("openai", &account.label)
                        .0
                        .to_string(),
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
                is_favorite: false,
                recommended: false,
                recommendation_rank: usize::MAX,
                usage_score: 0,
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
            is_favorite: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            usage_score: 0,
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
            is_favorite: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            usage_score: 0,
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
            is_favorite: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            usage_score: 0,
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
            is_favorite: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            usage_score: 0,
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
            is_favorite: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            usage_score: 0,
            old: false,
            created_date: None,
            effort: None,
        });

        if models.is_empty() {
            selected = 0;
        }
        if !openai_accounts.is_empty() {
            let mut usage_entry = models.last().expect("account center entry").clone();
            usage_entry.name = "OpenAI usage details".to_string();
            usage_entry.options[0].detail =
                "Today / lifetime API-equivalent cost and tokens by account".to_string();
            usage_entry.options[0].provider = "OpenAI".to_string();
            usage_entry.action = crate::tui::PickerAction::Usage {
                id: "openai-oauth-accounts".to_string(),
                title: "ChatGPT OAuth account usage".to_string(),
                subtitle: "Today / lifetime API-equivalent estimates, not a bill".to_string(),
                status: crate::tui::usage_overlay::UsageOverlayStatus::Info,
                detail_lines: self
                    .render_openai_accounts_markdown()
                    .lines()
                    .map(str::to_string)
                    .collect(),
            };
            models.push(usage_entry);
        }
        (models, selected)
    }

    fn build_openai_inline_account_picker(&self) -> (Vec<crate::tui::PickerEntry>, usize) {
        let accounts = crate::auth::codex::list_accounts().unwrap_or_default();
        let active_label = self
            .window_account_label("openai")
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
                name: account_picker_display_name(
                    "OpenAI",
                    &account.label,
                    accounts.len(),
                    account.email.as_deref(),
                ),
                options: vec![crate::tui::PickerOption {
                    provider: "OpenAI".to_string(),
                    api_method: self
                        .account_row_badge("openai", &account.label)
                        .0
                        .to_string(),
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
                is_favorite: false,
                recommended: false,
                recommendation_rank: usize::MAX,
                usage_score: 0,
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
            is_favorite: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            usage_score: 0,
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
            is_favorite: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            usage_score: 0,
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
            is_favorite: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            usage_score: 0,
            old: false,
            created_date: None,
            effort: None,
        });

        if accounts.is_empty() {
            selected = 0;
        }
        if !accounts.is_empty() {
            let mut usage_entry = models.last().expect("account center entry").clone();
            usage_entry.name = "OpenAI usage details".to_string();
            usage_entry.options[0].detail =
                "Today / lifetime API-equivalent cost and tokens by account".to_string();
            usage_entry.options[0].provider = "OpenAI".to_string();
            usage_entry.action = crate::tui::PickerAction::Usage {
                id: "openai-oauth-accounts".to_string(),
                title: "ChatGPT OAuth account usage".to_string(),
                subtitle: "Today / lifetime API-equivalent estimates, not a bill".to_string(),
                status: crate::tui::usage_overlay::UsageOverlayStatus::Info,
                detail_lines: self
                    .render_openai_accounts_markdown()
                    .lines()
                    .map(str::to_string)
                    .collect(),
            };
            models.push(usage_entry);
        }
        (models, selected)
    }

    pub(crate) fn handle_account_picker_command(
        &mut self,
        command: crate::tui::account_picker::AccountPickerCommand,
    ) {
        let navigation = matches!(
            &command,
            crate::tui::account_picker::AccountPickerCommand::OpenAccountCenter { .. }
                | crate::tui::account_picker::AccountPickerCommand::OpenAddReplaceFlow { .. }
        );
        let message_start = self.display_messages.len();
        let selected = Self::account_manager_action_target(&command);
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
                        self.input = "/accounts claude add".to_string();
                        self.cursor_pos = self.input.len();
                        self.submit_input();
                    }
                    crate::tui::account_picker::AccountProviderKind::OpenAi => {
                        self.input = "/accounts openai add".to_string();
                        self.cursor_pos = self.input.len();
                        self.submit_input();
                    }
                }
            }
            other => {
                if let Some(command) = crate::tui::app::auth::account_command_from_picker(&other) {
                    crate::tui::app::auth::execute_account_command_local(self, command);
                }
            }
        }
        if !navigation {
            self.finish_account_manager_action(message_start, selected);
        }
    }
}
