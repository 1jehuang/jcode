use super::helpers::{
    agent_model_default_summary, agent_model_target_config_path, agent_model_target_label,
    agent_model_target_slug, load_agent_model_override, model_entry_base_name,
    model_entry_saved_spec,
};
use super::*;
use crate::tui::{
    AgentModelTarget, InlineInteractiveState, PickerAction, PickerEntry, PickerKind, PickerOption,
};

impl App {
    pub(crate) fn open_agents_picker(&mut self) {
        if crate::tui::app::commands_dispatch::ssh_local_action_blocked(
            self,
            "Agent model configuration",
        ) {
            return;
        }
        let models = [
            AgentModelTarget::Swarm,
            AgentModelTarget::Review,
            AgentModelTarget::Judge,
            AgentModelTarget::Memory,
            AgentModelTarget::Ambient,
        ]
        .into_iter()
        .map(|target| {
            let configured = if self.agent_models_global_scope {
                load_agent_model_override(target)
            } else {
                self.session.effective_agent_model(
                    agent_model_target_slug(target),
                    load_agent_model_override(target),
                )
            };
            let summary = configured
                .clone()
                .unwrap_or_else(|| agent_model_default_summary(target, self));
            PickerEntry {
                name: format!(
                    "{} [{}]",
                    agent_model_target_label(target),
                    if self.agent_models_global_scope {
                        "global"
                    } else {
                        "session"
                    }
                ),
                options: vec![PickerOption {
                    provider: summary,
                    api_method: agent_model_target_config_path(target).to_string(),
                    available: true,
                    detail: if target == AgentModelTarget::Swarm {
                        "/agents swarm · routing: /swarm-prompt".to_string()
                    } else if target == AgentModelTarget::Memory {
                        "/agents memory · extraction only, recall uses Jev".to_string()
                    } else {
                        format!("/agents {}", agent_model_target_slug(target))
                    },
                    estimated_reference_cost_micros: None,
                }],
                action: PickerAction::AgentTarget(target),
                selected_option: 0,
                is_current: false,
                is_default: configured.is_some(),
                is_favorite: false,
                recommended: false,
                recommendation_rank: usize::MAX,
                usage_score: 0,
                old: false,
                created_date: None,
                effort: None,
            }
        })
        .collect();

        self.inline_view_state = None;
        self.inline_interactive_state = Some(InlineInteractiveState {
            kind: PickerKind::Model,
            filtered: (0..5).collect(),
            entries: models,
            selected: 0,
            column: 0,
            filter: String::new(),
            preview: false,
        });
        self.input.clear();
        self.cursor_pos = 0;
    }

    pub(crate) fn open_login_picker_inline(&mut self) {
        self.open_auth_provider_picker_inline(false);
    }

    pub(crate) fn open_logout_picker_inline(&mut self) {
        self.open_auth_provider_picker_inline(true);
    }

    fn open_auth_provider_picker_inline(&mut self, logout: bool) {
        if crate::tui::app::commands_dispatch::ssh_local_action_blocked(
            self,
            "Local authentication",
        ) {
            return;
        }
        let status = crate::auth::AuthStatus::check_fast();
        let providers = crate::provider_catalog::tui_login_providers();
        let mut models = providers
            .into_iter()
            .filter(|provider| {
                // Logging out of the auto-import descriptor is meaningless: there is
                // no saved session for it, only detected external logins.
                !(logout
                    && matches!(
                        provider.target,
                        crate::provider_catalog::LoginProviderTarget::AutoImport
                    ))
            })
            .map(|provider| {
                let assessment = status.assessment_for_provider(provider);
                let auth_state = assessment.state;
                let state_label = match auth_state {
                    crate::auth::AuthState::Available => {
                        if matches!(
                            provider.target,
                            crate::provider_catalog::LoginProviderTarget::AutoImport
                        ) {
                            "detected"
                        } else {
                            "configured"
                        }
                    }
                    crate::auth::AuthState::Expired => "attention",
                    crate::auth::AuthState::NotConfigured => "setup",
                };
                let provider_label = if matches!(
                    provider.target,
                    crate::provider_catalog::LoginProviderTarget::AutoImport
                ) {
                    "import"
                } else {
                    provider.auth_kind.label()
                };
                PickerEntry {
                    name: provider.display_name.to_string(),
                    options: vec![PickerOption {
                        provider: provider_label.to_string(),
                        api_method: state_label.to_string(),
                        available: true,
                        detail: format!("{} · {}", assessment.method_detail, provider.menu_detail),
                        estimated_reference_cost_micros: None,
                    }],
                    action: if logout {
                        PickerAction::Logout(provider)
                    } else {
                        PickerAction::Login(provider)
                    },
                    selected_option: 0,
                    is_current: auth_state == crate::auth::AuthState::Available,
                    is_default: false,
                    is_favorite: false,
                    recommended: provider.recommended,
                    recommendation_rank: usize::MAX,
                    usage_score: 0,
                    old: false,
                    created_date: None,
                    effort: None,
                }
            })
            .collect::<Vec<_>>();

        if !logout {
            let saved_entries =
                super::super::auth_account_picker_saved_accounts::saved_login_picker_entries(
                    self.provider.name(),
                    self.provider.oauth_account_label().as_deref(),
                );
            let auto_api_key_entry = PickerEntry {
                name: "Paste an API key".to_string(),
                options: vec![PickerOption {
                    provider: "auto-detect".to_string(),
                    api_method: "API key".to_string(),
                    available: true,
                    detail: "Auto-detects Anthropic, OpenAI, OpenRouter, Gemini, xAI, Groq, Hugging Face & more".to_string(),
                    estimated_reference_cost_micros: None,
                }],
                action: PickerAction::AutoApiKey,
                selected_option: 0,
                is_current: false,
                is_default: false,
                is_favorite: false,
                recommended: true,
                recommendation_rank: usize::MAX,
                usage_score: 0,
                old: false,
                created_date: None,
                effort: None,
            };

            let mut starter_entries = Vec::new();
            let mut configured_hidden_entries = Vec::new();
            let mut unconfigured_hidden_entries = Vec::new();
            let mut openai_entry = None;
            let mut claude_entry = None;
            let mut gemini_entry = None;
            let mut auto_import_entry = None;

            for entry in models {
                match entry.action {
                    PickerAction::Login(provider) => match provider.target {
                        crate::provider_catalog::LoginProviderTarget::OpenAi => {
                            openai_entry = Some(entry);
                        }
                        crate::provider_catalog::LoginProviderTarget::Claude => {
                            claude_entry = Some(entry);
                        }
                        crate::provider_catalog::LoginProviderTarget::Antigravity
                        | crate::provider_catalog::LoginProviderTarget::Gemini => {
                            if gemini_entry.is_none() {
                                gemini_entry = Some(entry);
                            } else {
                                unconfigured_hidden_entries.push(entry);
                            }
                        }
                        crate::provider_catalog::LoginProviderTarget::AutoImport => {
                            auto_import_entry = Some(entry);
                        }
                        _ => {
                            let is_configured_or_attention = entry.is_current
                                || entry
                                    .options
                                    .first()
                                    .is_some_and(|opt| opt.api_method == "attention");
                            if is_configured_or_attention {
                                configured_hidden_entries.push(entry);
                            } else {
                                unconfigured_hidden_entries.push(entry);
                            }
                        }
                    },
                    _ => configured_hidden_entries.push(entry),
                }
            }

            if let Some(entry) = openai_entry {
                starter_entries.push(entry);
            }
            if let Some(entry) = claude_entry {
                starter_entries.push(entry);
            }
            if let Some(entry) = gemini_entry {
                starter_entries.push(entry);
            }
            starter_entries.push(auto_api_key_entry);
            if let Some(entry) = auto_import_entry {
                starter_entries.push(entry);
            }

            let mut hidden_entries = saved_entries;
            hidden_entries.extend(configured_hidden_entries);
            hidden_entries.extend(unconfigured_hidden_entries);

            if !hidden_entries.is_empty() {
                let hidden_count = hidden_entries.len();
                starter_entries.push(PickerEntry {
                    name: format!("More providers ({})…", hidden_count),
                    options: vec![PickerOption {
                        provider: "all".to_string(),
                        api_method: "expand".to_string(),
                        available: true,
                        detail: "OpenRouter, Ollama, DeepSeek, Groq, Bedrock, Custom · or type to filter".to_string(),
                        estimated_reference_cost_micros: None,
                    }],
                    action: PickerAction::ExpandLoginProviders,
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
            }

            starter_entries.extend(hidden_entries);
            models = starter_entries;
        }

        if logout {
            // Prepend a synthetic "All providers" entry that logs out everywhere.
            models.insert(
                0,
                PickerEntry {
                    name: "All providers".to_string(),
                    options: vec![PickerOption {
                        provider: "all".to_string(),
                        api_method: "logout".to_string(),
                        available: true,
                        detail: "Log out of every provider with a saved session".to_string(),
                        estimated_reference_cost_micros: None,
                    }],
                    action: PickerAction::LogoutAll,
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
                },
            );
        }

        self.inline_view_state = None;
        let mut picker = InlineInteractiveState {
            kind: PickerKind::Login,
            filtered: (0..models.len()).collect(),
            entries: models,
            selected: 0,
            column: 0,
            filter: String::new(),
            preview: false,
        };
        Self::apply_inline_interactive_filter(&mut picker);
        self.inline_interactive_state = Some(picker);
        self.input.clear();
        self.cursor_pos = 0;
    }

    pub(crate) fn open_agent_model_picker(&mut self, target: AgentModelTarget) {
        if crate::tui::app::commands_dispatch::ssh_local_action_blocked(
            self,
            "Agent model configuration",
        ) {
            return;
        }
        let global_scope = self.agent_models_global_scope;
        let configured = if global_scope {
            load_agent_model_override(target)
        } else {
            self.session
                .effective_agent_model(agent_model_target_slug(target), None)
        };
        let inherit_summary = agent_model_default_summary(target, self);
        self.open_model_picker();
        let load_started = std::time::Instant::now();
        while self.pending_model_picker_load.is_some()
            && load_started.elapsed() < std::time::Duration::from_secs(2)
        {
            if self.poll_model_picker_load() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        if let Some(ref mut picker) = self.inline_interactive_state {
            if target == AgentModelTarget::Memory {
                picker.entries.retain(|entry| {
                    matches!(
                        crate::provider::provider_for_model(&model_entry_base_name(entry)),
                        Some("openai" | "claude")
                    )
                });
            }

            for entry in &mut picker.entries {
                let matches_saved = configured.as_deref().map(|saved| {
                    let base = model_entry_base_name(entry);
                    model_entry_saved_spec(entry) == saved || base == saved
                }) == Some(true);
                entry.action = PickerAction::AgentModelChoice {
                    target,
                    clear_override: false,
                };
                entry.is_current = matches_saved;
                entry.is_default = false;
            }

            if let Some(saved) = configured.as_deref() {
                let already_present = picker.entries.iter().any(|entry| {
                    model_entry_saved_spec(entry) == saved || model_entry_base_name(entry) == saved
                });
                if !already_present {
                    picker.entries.insert(
                        0,
                        PickerEntry {
                            name: saved.to_string(),
                            options: vec![PickerOption {
                                provider: "saved override".to_string(),
                                api_method: agent_model_target_config_path(target).to_string(),
                                available: true,
                                detail: "not in current picker catalog".to_string(),
                                estimated_reference_cost_micros: None,
                            }],
                            action: PickerAction::AgentModelChoice {
                                target,
                                clear_override: false,
                            },
                            selected_option: 0,
                            is_current: true,
                            is_default: false,
                            is_favorite: false,
                            recommended: false,
                            recommendation_rank: usize::MAX,
                            usage_score: 0,
                            old: false,
                            created_date: None,
                            effort: None,
                        },
                    );
                }
            }

            picker.entries.insert(
                0,
                PickerEntry {
                    name: if global_scope {
                        format!("use global fallback ({})", inherit_summary)
                    } else {
                        "use global default [session]".to_string()
                    },
                    options: vec![PickerOption {
                        provider: "default".to_string(),
                        api_method: agent_model_target_config_path(target).to_string(),
                        available: true,
                        detail: if global_scope {
                            "clear global default".to_string()
                        } else {
                            format!("clear only this session override · {}", inherit_summary)
                        },
                        estimated_reference_cost_micros: None,
                    }],
                    action: PickerAction::AgentModelChoice {
                        target,
                        clear_override: true,
                    },
                    selected_option: 0,
                    is_current: configured.is_none(),
                    is_default: false,
                    is_favorite: false,
                    recommended: false,
                    recommendation_rank: usize::MAX,
                    usage_score: 0,
                    old: false,
                    created_date: None,
                    effort: None,
                },
            );

            let mut inherit = picker.entries[0].clone();
            inherit.name = "inherit coordinator".to_string();
            inherit.options[0].provider = "inherit".to_string();
            inherit.options[0].detail = if global_scope {
                "global default: inherit coordinator"
            } else {
                "session: bypass global default"
            }
            .to_string();
            inherit.action = PickerAction::AgentModelChoice {
                target,
                clear_override: false,
            };
            inherit.is_current = configured.as_deref() == Some("inherit");
            picker.entries.insert(1, inherit);
            for entry in &mut picker.entries {
                for option in &mut entry.options {
                    option.detail = format!(
                        "[{}] {}",
                        if global_scope { "global" } else { "session" },
                        option.detail
                    );
                }
            }

            if target == AgentModelTarget::Memory {
                for entry in &mut picker.entries {
                    for option in &mut entry.options {
                        option.detail =
                            format!("Extraction only; recall uses Jev. {}", option.detail);
                    }
                }
            }

            picker.filtered = (0..picker.entries.len()).collect();
            picker.selected = picker
                .entries
                .iter()
                .position(|entry| entry.is_current)
                .unwrap_or(0);
            picker.column = 0;
            picker.filter.clear();
        }
    }
}
