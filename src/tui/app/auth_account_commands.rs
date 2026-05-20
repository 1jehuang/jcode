use super::*;

pub(crate) fn handle_auth_command(app: &mut App, trimmed: &str) -> bool {
    if trimmed == "/auth" {
        app.show_auth_status();
        return true;
    }

    if let Some(rest) = trimmed.strip_prefix("/auth doctor") {
        let provider_id = (!rest.trim().is_empty()).then(|| rest.trim().to_string());
        app.push_display_message(DisplayMessage::system(render_auth_doctor_markdown(
            provider_id.as_deref(),
        )));
        return true;
    }

    if trimmed == "/login" {
        app.show_interactive_login();
        return true;
    }

    if let Some(provider) = trimmed
        .strip_prefix("/login ")
        .or_else(|| trimmed.strip_prefix("/auth "))
    {
        let providers = crate::provider_catalog::tui_login_providers();
        if let Some(provider) =
            crate::provider_catalog::resolve_login_selection(provider, &providers)
        {
            app.start_login_provider(provider);
        } else {
            let valid = providers
                .iter()
                .map(|provider| provider.id)
                .collect::<Vec<_>>()
                .join(", ");
            app.push_display_message(DisplayMessage::error(format!(
                "Unknown provider '{}'. Use: {}",
                provider.trim(),
                valid
            )));
        }
        return true;
    }

    if trimmed == "/subscription" || trimmed == "/subscription status" {
        app.show_jcode_subscription_status();
        return true;
    }

    if let Some(parsed) = parse_account_command(trimmed) {
        match parsed {
            Ok(command) => execute_account_command_local(app, command),
            Err(message) => app.push_display_message(DisplayMessage::error(message)),
        }
        return true;
    }

    false
}

pub(crate) async fn handle_account_command_remote(
    app: &mut App,
    trimmed: &str,
    remote: &mut crate::tui::backend::RemoteConnection,
) -> anyhow::Result<bool> {
    let Some(parsed) = parse_account_command(trimmed) else {
        return Ok(false);
    };
    match parsed {
        Ok(command) => execute_account_command_remote(app, command, remote).await?,
        Err(message) => app.push_display_message(DisplayMessage::error(message)),
    }
    Ok(true)
}

fn parse_account_command(trimmed: &str) -> Option<Result<AccountCommand, String>> {
    let rest = trimmed
        .strip_prefix("/account")
        .or_else(|| trimmed.strip_prefix("/accounts"))?;
    let rest = rest.trim();
    if rest.is_empty() {
        return Some(Ok(AccountCommand::OpenOverlay {
            provider_filter: None,
        }));
    }

    let mut parts = rest.split_whitespace();
    let first = parts.next()?;
    let remainder = parts.collect::<Vec<_>>().join(" ");
    let remainder = remainder.trim();

    match first {
        "doctor" => {
            return Some(Ok(AccountCommand::Doctor { provider_id: None }));
        }
        "list" | "ls" => {
            return Some(Ok(AccountCommand::OpenOverlay {
                provider_filter: None,
            }));
        }
        "switch" | "use" => {
            if remainder.is_empty() {
                return Some(Err("Usage: `/account switch <label>`".to_string()));
            }
            return Some(Ok(AccountCommand::SwitchShorthand {
                label: remainder.to_string(),
            }));
        }
        "add" | "login" => {
            return Some(Ok(AccountCommand::Add {
                provider_id: "claude".to_string(),
                label: (!remainder.is_empty()).then(|| remainder.to_string()),
            }));
        }
        "remove" | "rm" | "delete" => {
            if remainder.is_empty() {
                return Some(Err("Usage: `/account remove <label>`".to_string()));
            }
            return Some(Ok(AccountCommand::Remove {
                provider_id: "claude".to_string(),
                label: remainder.to_string(),
            }));
        }
        "default-provider" => {
            if remainder.is_empty() {
                return Some(Err(
                    "Usage: `/account default-provider <claude|openai|copilot|gemini|openrouter|auto>`"
                        .to_string(),
                ));
            }
            return Some(Ok(AccountCommand::SetDefaultProvider(
                normalize_clearish_value(remainder),
            )));
        }
        "default-model" => {
            if remainder.is_empty() {
                return Some(Err(
                    "Usage: `/account default-model <model|clear>`".to_string()
                ));
            }
            return Some(Ok(AccountCommand::SetDefaultModel(
                normalize_clearish_value(remainder),
            )));
        }
        _ => {}
    }

    if let Some(provider) = resolve_account_provider_descriptor(first) {
        let provider_id = provider.id.to_string();
        if remainder.is_empty() {
            return Some(Ok(AccountCommand::OpenOverlay {
                provider_filter: Some(provider_id),
            }));
        }

        let mut provider_parts = remainder.split_whitespace();
        let subcommand = provider_parts.next().unwrap_or_default();
        let value = provider_parts.collect::<Vec<_>>().join(" ");
        let value = value.trim();

        let parsed = match subcommand {
            "doctor" => AccountCommand::Doctor {
                provider_id: Some(provider.id.to_string()),
            },
            "list" | "ls" => AccountCommand::OpenOverlay {
                provider_filter: Some(provider.id.to_string()),
            },
            "settings" => AccountCommand::ShowSettings {
                provider_id: provider.id.to_string(),
            },
            "login" => AccountCommand::Login {
                provider_id: provider.id.to_string(),
            },
            "add" => AccountCommand::Add {
                provider_id: provider.id.to_string(),
                label: (!value.is_empty()).then(|| value.to_string()),
            },
            "switch" | "use" => {
                if value.is_empty() {
                    return Some(Err(format!(
                        "Usage: `/account {} switch <label>`",
                        provider.id
                    )));
                }
                AccountCommand::Switch {
                    provider_id: provider.id.to_string(),
                    label: value.to_string(),
                }
            }
            "remove" | "rm" | "delete" => {
                if value.is_empty() {
                    return Some(Err(format!(
                        "Usage: `/account {} remove <label>`",
                        provider.id
                    )));
                }
                AccountCommand::Remove {
                    provider_id: provider.id.to_string(),
                    label: value.to_string(),
                }
            }
            "transport" if provider.id == "openai" => {
                if value.is_empty() {
                    return Some(Err(
                        "Usage: `/account openai transport <auto|https|websocket>`".to_string(),
                    ));
                }
                AccountCommand::SetOpenAiTransport(normalize_clearish_value(value))
            }
            "effort" if provider.id == "openai" => {
                if value.is_empty() {
                    return Some(Err(
                        "Usage: `/account openai effort <none|low|medium|high|xhigh|clear>`"
                            .to_string(),
                    ));
                }
                AccountCommand::SetOpenAiEffort(normalize_clearish_value(value))
            }
            "fast" if provider.id == "openai" => match value.to_ascii_lowercase().as_str() {
                "on" => AccountCommand::SetOpenAiFast(true),
                "off" => AccountCommand::SetOpenAiFast(false),
                _ => {
                    return Some(Err("Usage: `/account openai fast <on|off>`".to_string()));
                }
            },
            "premium" if provider.id == "copilot" => {
                if value.is_empty() {
                    return Some(Err(
                        "Usage: `/account copilot premium <normal|one|zero>`".to_string()
                    ));
                }
                AccountCommand::SetCopilotPremium(normalize_normal_mode_value(value))
            }
            "api-base" if provider.id == "openai-compatible" => {
                if value.is_empty() {
                    return Some(Err(
                        "Usage: `/account openai-compatible api-base <url|clear>`".to_string(),
                    ));
                }
                AccountCommand::SetOpenAiCompatApiBase(normalize_clearish_value(value))
            }
            "api-key-name" if provider.id == "openai-compatible" => {
                if value.is_empty() {
                    return Some(Err(
                        "Usage: `/account openai-compatible api-key-name <ENV_VAR|clear>`"
                            .to_string(),
                    ));
                }
                AccountCommand::SetOpenAiCompatApiKeyName(normalize_clearish_value(value))
            }
            "env-file" if provider.id == "openai-compatible" => {
                if value.is_empty() {
                    return Some(Err(
                        "Usage: `/account openai-compatible env-file <file.env|clear>`".to_string(),
                    ));
                }
                AccountCommand::SetOpenAiCompatEnvFile(normalize_clearish_value(value))
            }
            "default-model" if provider.id == "openai-compatible" => {
                if value.is_empty() {
                    return Some(Err(
                        "Usage: `/account openai-compatible default-model <model|clear>`"
                            .to_string(),
                    ));
                }
                AccountCommand::SetOpenAiCompatDefaultModel(normalize_clearish_value(value))
            }
            other => {
                if matches!(provider.id, "claude" | "openai") {
                    return Some(Ok(AccountCommand::Switch {
                        provider_id: provider.id.to_string(),
                        label: other.to_string(),
                    }));
                }
                return Some(Err(format!(
                    "Unknown `/account {}` subcommand '{}'. Try `/account {} settings` or `/account {} login`.",
                    provider.id, other, provider.id, provider.id
                )));
            }
        };

        return Some(Ok(parsed));
    }

    Some(Ok(AccountCommand::SwitchShorthand {
        label: first.to_string(),
    }))
}

fn execute_account_command_local(app: &mut App, command: AccountCommand) {
    match command {
        AccountCommand::OpenOverlay { provider_filter } => {
            if app.should_open_inline_account_picker(provider_filter.as_deref()) {
                app.open_account_picker(provider_filter.as_deref())
            } else {
                app.open_account_center(provider_filter.as_deref())
            }
        }
        AccountCommand::Doctor { provider_id } => app.push_display_message(DisplayMessage::system(
            render_auth_doctor_markdown(provider_id.as_deref()),
        )),
        AccountCommand::ShowSettings { provider_id } => app.push_display_message(
            DisplayMessage::system(render_provider_settings_markdown(app, &provider_id)),
        ),
        AccountCommand::Login { provider_id } => {
            match resolve_account_provider_descriptor(&provider_id) {
                Some(provider) => app.start_login_provider(provider),
                None => app.push_display_message(DisplayMessage::error(format!(
                    "Unknown provider `{}`.",
                    provider_id
                ))),
            }
        }
        AccountCommand::Add { provider_id, label } => {
            execute_account_add_local(app, &provider_id, label.as_deref())
        }
        AccountCommand::Switch { provider_id, label } => match provider_id.as_str() {
            "claude" => app.switch_account(&label),
            "openai" => app.switch_openai_account(&label),
            _ => app.push_display_message(DisplayMessage::error(format!(
                "Provider `{}` does not support account switching.",
                provider_id
            ))),
        },
        AccountCommand::SwitchShorthand { label } => app.switch_account_by_label(&label),
        AccountCommand::Remove { provider_id, label } => match provider_id.as_str() {
            "claude" => app.remove_account(&label),
            "openai" => app.remove_openai_account(&label),
            _ => app.push_display_message(DisplayMessage::error(format!(
                "Provider `{}` does not support account removal.",
                provider_id
            ))),
        },
        AccountCommand::SetDefaultProvider(provider) => {
            save_default_provider_setting(app, provider.as_deref())
        }
        AccountCommand::SetDefaultModel(model) => save_default_model_setting(app, model.as_deref()),
        AccountCommand::SetOpenAiTransport(value) => {
            save_openai_transport_setting_local(app, value.as_deref())
        }
        AccountCommand::SetOpenAiEffort(value) => {
            save_openai_effort_setting_local(app, value.as_deref())
        }
        AccountCommand::SetOpenAiFast(enabled) => save_openai_fast_setting_local(app, enabled),
        AccountCommand::SetCopilotPremium(mode) => {
            save_copilot_premium_setting(app, mode.as_deref())
        }
        AccountCommand::SetOpenAiCompatApiBase(value) => {
            save_openai_compat_setting(app, OpenAiCompatSetting::ApiBase, value.as_deref())
        }
        AccountCommand::SetOpenAiCompatApiKeyName(value) => {
            save_openai_compat_setting(app, OpenAiCompatSetting::ApiKeyName, value.as_deref())
        }
        AccountCommand::SetOpenAiCompatEnvFile(value) => {
            save_openai_compat_setting(app, OpenAiCompatSetting::EnvFile, value.as_deref())
        }
        AccountCommand::SetOpenAiCompatDefaultModel(value) => {
            save_openai_compat_setting(app, OpenAiCompatSetting::DefaultModel, value.as_deref())
        }
    }
}

async fn execute_account_command_remote(
    app: &mut App,
    command: AccountCommand,
    remote: &mut crate::tui::backend::RemoteConnection,
) -> anyhow::Result<()> {
    match command {
        AccountCommand::OpenOverlay { provider_filter } => {
            if app.should_open_inline_account_picker(provider_filter.as_deref()) {
                app.open_account_picker(provider_filter.as_deref());
            } else {
                app.open_account_center(provider_filter.as_deref());
            }
        }
        AccountCommand::Doctor { provider_id } => {
            execute_account_command_local(app, AccountCommand::Doctor { provider_id })
        }
        AccountCommand::Switch { provider_id, label } => match provider_id.as_str() {
            "claude" => {
                if let Err(e) = crate::auth::claude::set_active_account(&label) {
                    app.push_display_message(DisplayMessage::error(format!(
                        "Failed to switch account: {}",
                        e
                    )));
                    return Ok(());
                }
                crate::auth::AuthStatus::invalidate_cache();
                app.context_limit = app.provider.context_window() as u64;
                app.context_warning_shown = false;
                remote.switch_anthropic_account(&label).await?;
                app.push_display_message(DisplayMessage::system(format!(
                    "Switched to Anthropic account `{}`.",
                    label
                )));
                app.set_status_notice(format!("Account: switched to {}", label));
            }
            "openai" => {
                if let Err(e) = crate::auth::codex::set_active_account(&label) {
                    app.push_display_message(DisplayMessage::error(format!(
                        "Failed to switch OpenAI account: {}",
                        e
                    )));
                    return Ok(());
                }
                crate::auth::AuthStatus::invalidate_cache();
                app.context_limit = app.provider.context_window() as u64;
                app.context_warning_shown = false;
                remote.switch_openai_account(&label).await?;
                app.push_display_message(DisplayMessage::system(format!(
                    "Switched to OpenAI account `{}`.",
                    label
                )));
                app.set_status_notice(format!("OpenAI account: switched to {}", label));
            }
            _ => execute_account_command_local(app, AccountCommand::Switch { provider_id, label }),
        },
        AccountCommand::SwitchShorthand { label } => {
            let has_anthropic = crate::auth::claude::list_accounts()
                .unwrap_or_default()
                .iter()
                .any(|account| account.label == label);
            let has_openai = crate::auth::codex::list_accounts()
                .unwrap_or_default()
                .iter()
                .any(|account| account.label == label);
            match (has_anthropic, has_openai) {
                (true, false) => {
                    if let Err(e) = crate::auth::claude::set_active_account(&label) {
                        app.push_display_message(DisplayMessage::error(format!(
                            "Failed to switch account: {}",
                            e
                        )));
                        return Ok(());
                    }
                    crate::auth::AuthStatus::invalidate_cache();
                    app.context_limit = app.provider.context_window() as u64;
                    app.context_warning_shown = false;
                    remote.switch_anthropic_account(&label).await?;
                    app.push_display_message(DisplayMessage::system(format!(
                        "Switched to Anthropic account `{}`.",
                        label
                    )));
                    app.set_status_notice(format!("Account: switched to {}", label));
                }
                (false, true) => {
                    if let Err(e) = crate::auth::codex::set_active_account(&label) {
                        app.push_display_message(DisplayMessage::error(format!(
                            "Failed to switch OpenAI account: {}",
                            e
                        )));
                        return Ok(());
                    }
                    crate::auth::AuthStatus::invalidate_cache();
                    app.context_limit = app.provider.context_window() as u64;
                    app.context_warning_shown = false;
                    remote.switch_openai_account(&label).await?;
                    app.push_display_message(DisplayMessage::system(format!(
                        "Switched to OpenAI account `{}`.",
                        label
                    )));
                    app.set_status_notice(format!("OpenAI account: switched to {}", label));
                }
                _ => execute_account_command_local(app, AccountCommand::SwitchShorthand { label }),
            }
        }
        AccountCommand::SetOpenAiTransport(value) => {
            save_openai_transport_setting_local(app, value.as_deref());
            remote
                .set_transport(value.as_deref().unwrap_or("auto"))
                .await?;
        }
        AccountCommand::SetOpenAiEffort(value) => {
            save_openai_effort_setting_local(app, value.as_deref());
            if let Some(value) = value.as_deref() {
                remote.set_reasoning_effort(value).await?;
            }
        }
        AccountCommand::SetOpenAiFast(enabled) => {
            save_openai_fast_setting_local(app, enabled);
            remote
                .set_service_tier(if enabled { "priority" } else { "off" })
                .await?;
        }
        other => execute_account_command_local(app, other),
    }
    Ok(())
}

fn execute_account_add_local(app: &mut App, provider_id: &str, label: Option<&str>) {
    match provider_id {
        "claude" => {
            let target = match label.map(str::trim).filter(|label| !label.is_empty()) {
                Some(existing) => existing.to_string(),
                None => match crate::auth::claude::next_account_label() {
                    Ok(label) => label,
                    Err(e) => {
                        app.push_display_message(DisplayMessage::error(format!(
                            "Failed to prepare Claude account: {}",
                            e
                        )));
                        return;
                    }
                },
            };
            app.start_claude_login_for_account(&target)
        }
        "openai" => {
            let target = match label.map(str::trim).filter(|label| !label.is_empty()) {
                Some(existing) => existing.to_string(),
                None => match crate::auth::codex::next_account_label() {
                    Ok(label) => label,
                    Err(e) => {
                        app.push_display_message(DisplayMessage::error(format!(
                            "Failed to prepare OpenAI account: {}",
                            e
                        )));
                        return;
                    }
                },
            };
            app.start_openai_login_for_account(&target)
        }
        other => match resolve_account_provider_descriptor(other) {
            Some(provider) => app.start_login_provider(provider),
            None => app.push_display_message(DisplayMessage::error(format!(
                "Unknown provider `{}`.",
                other
            ))),
        },
    }
}

pub(crate) fn resolve_account_provider_descriptor(
    input: &str,
) -> Option<crate::provider_catalog::LoginProviderDescriptor> {
    crate::provider_catalog::resolve_login_provider(input)
}

fn normalize_clearish_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || matches!(trimmed, "clear" | "unset" | "none" | "auto") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn normalize_normal_mode_value(value: &str) -> Option<String> {
    let trimmed = value.trim().to_ascii_lowercase();
    match trimmed.as_str() {
        "" | "normal" | "clear" | "unset" => None,
        "one" | "zero" => Some(trimmed),
        _ => Some(trimmed),
    }
}

fn save_default_provider_setting(app: &mut App, provider: Option<&str>) {
    let normalized = provider.map(|provider| provider.trim().to_ascii_lowercase());
    let provider = match normalized.as_deref() {
        None => None,
        Some("auto") => None,
        Some("claude" | "openai" | "copilot" | "gemini" | "openrouter") => normalized,
        Some(other) => {
            app.push_display_message(DisplayMessage::error(format!(
                "Unsupported default provider `{}`. Use claude, openai, copilot, gemini, openrouter, or auto.",
                other
            )));
            return;
        }
    };
    match crate::config::Config::set_default_provider(provider.as_deref()) {
        Ok(()) => {
            let label = provider.as_deref().unwrap_or("auto");
            app.set_status_notice(format!("Default provider: {}", label));
            app.push_display_message(DisplayMessage::system(format!(
                "Saved default provider: **{}**. This affects future sessions.",
                label
            )));
        }
        Err(err) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to save default provider: {}",
            err
        ))),
    }
}

fn save_default_model_setting(app: &mut App, model: Option<&str>) {
    match crate::config::Config::set_default_model_only(model) {
        Ok(()) => {
            let label = model.unwrap_or("(provider default)");
            app.set_status_notice(format!("Default model: {}", label));
            app.push_display_message(DisplayMessage::system(format!(
                "Saved default model: **{}**. This affects future sessions.",
                label
            )));
        }
        Err(err) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to save default model: {}",
            err
        ))),
    }
}

fn save_openai_transport_setting_local(app: &mut App, value: Option<&str>) {
    let value = value.unwrap_or("auto");
    if !matches!(value, "auto" | "https" | "websocket") {
        app.push_display_message(DisplayMessage::error(
            "OpenAI transport must be `auto`, `https`, or `websocket`.".to_string(),
        ));
        return;
    }
    match crate::config::Config::set_openai_transport(Some(value)) {
        Ok(()) => {
            let _ = app.provider.set_transport(value);
            app.set_status_notice(format!("Transport: {}", value));
            app.push_display_message(DisplayMessage::system(format!(
                "Saved OpenAI transport preference: **{}**.",
                value
            )));
        }
        Err(err) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to save OpenAI transport: {}",
            err
        ))),
    }
}

fn save_openai_effort_setting_local(app: &mut App, value: Option<&str>) {
    if let Some(value) = value
        && !matches!(value, "none" | "low" | "medium" | "high" | "xhigh")
    {
        app.push_display_message(DisplayMessage::error(
            "OpenAI effort must be one of `none`, `low`, `medium`, `high`, or `xhigh`.".to_string(),
        ));
        return;
    }
    match crate::config::Config::set_openai_reasoning_effort(value) {
        Ok(()) => {
            if let Some(value) = value {
                let _ = app.provider.set_reasoning_effort(value);
            }
            let label = value.unwrap_or("(provider default)");
            app.set_status_notice(format!("Effort: {}", label));
            app.push_display_message(DisplayMessage::system(format!(
                "Saved OpenAI reasoning effort: **{}**.",
                label
            )));
        }
        Err(err) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to save OpenAI effort: {}",
            err
        ))),
    }
}

pub(crate) fn save_openai_fast_setting_local(app: &mut App, enabled: bool) {
    let value = if enabled { Some("priority") } else { None };
    match crate::config::Config::set_openai_service_tier(value) {
        Ok(()) => {
            let _ = app
                .provider
                .set_service_tier(if enabled { "priority" } else { "off" });
            let label = if enabled { "on" } else { "off" };
            app.set_status_notice(format!("Fast mode: {}", label));
            app.push_display_message(DisplayMessage::system(format!(
                "Saved OpenAI fast mode: **{}**.",
                label
            )));
        }
        Err(err) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to save OpenAI fast mode: {}",
            err
        ))),
    }
}

fn save_copilot_premium_setting(app: &mut App, mode: Option<&str>) {
    use crate::provider::copilot::PremiumMode;

    let premium_mode = match mode.unwrap_or("normal") {
        "normal" => PremiumMode::Normal,
        "one" => PremiumMode::OnePerSession,
        "zero" => PremiumMode::Zero,
        other => {
            app.push_display_message(DisplayMessage::error(format!(
                "Copilot premium mode must be `normal`, `one`, or `zero` (got `{}`).",
                other
            )));
            return;
        }
    };
    app.provider.set_premium_mode(premium_mode);
    let result = match mode {
        None | Some("normal") => crate::config::Config::set_copilot_premium(None),
        Some(value) => crate::config::Config::set_copilot_premium(Some(value)),
    };
    match result {
        Ok(()) => {
            let label = match premium_mode {
                PremiumMode::Normal => "normal",
                PremiumMode::OnePerSession => "one premium per session",
                PremiumMode::Zero => "zero premium requests",
            };
            app.set_status_notice(format!("Premium: {}", label));
            app.push_display_message(DisplayMessage::system(format!(
                "Saved Copilot premium mode: **{}**.",
                label
            )));
        }
        Err(err) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to save Copilot premium mode: {}",
            err
        ))),
    }
}

#[derive(Clone, Copy)]
enum OpenAiCompatSetting {
    ApiBase,
    ApiKeyName,
    EnvFile,
    DefaultModel,
}

fn save_openai_compat_setting(app: &mut App, setting: OpenAiCompatSetting, value: Option<&str>) {
    let old = crate::provider_catalog::resolve_openai_compatible_profile(
        crate::provider_catalog::OPENAI_COMPAT_PROFILE,
    );
    let current_key =
        crate::provider_catalog::load_api_key_from_env_or_config(&old.api_key_env, &old.env_file);
    let (env_key, normalized_value) = match setting {
        OpenAiCompatSetting::ApiBase => {
            let normalized = match value {
                Some(value) => match crate::provider_catalog::normalize_api_base(value) {
                    Some(value) => Some(value),
                    None => {
                        app.push_display_message(DisplayMessage::error(
                            "OpenAI-compatible API base must be https://... or http://localhost."
                                .to_string(),
                        ));
                        return;
                    }
                },
                None => None,
            };
            ("JCODE_OPENAI_COMPAT_API_BASE", normalized)
        }
        OpenAiCompatSetting::ApiKeyName => {
            if let Some(value) = value
                && !crate::provider_catalog::is_safe_env_key_name(value)
            {
                app.push_display_message(DisplayMessage::error(
                    "API key variable must be uppercase letters, digits, and underscores only."
                        .to_string(),
                ));
                return;
            }
            (
                "JCODE_OPENAI_COMPAT_API_KEY_NAME",
                value.map(ToString::to_string),
            )
        }
        OpenAiCompatSetting::EnvFile => {
            if let Some(value) = value
                && !crate::provider_catalog::is_safe_env_file_name(value)
            {
                app.push_display_message(DisplayMessage::error(
                    "Env file must be a simple file name like `groq.env`.".to_string(),
                ));
                return;
            }
            (
                "JCODE_OPENAI_COMPAT_ENV_FILE",
                value.map(ToString::to_string),
            )
        }
        OpenAiCompatSetting::DefaultModel => (
            "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
            value.map(ToString::to_string),
        ),
    };

    if let Err(err) = crate::provider_catalog::save_env_value_to_env_file(
        env_key,
        crate::provider_catalog::OPENAI_COMPAT_PROFILE.env_file,
        normalized_value.as_deref(),
    ) {
        app.push_display_message(DisplayMessage::error(format!(
            "Failed to save OpenAI-compatible setting: {}",
            err
        )));
        return;
    }

    let new = crate::provider_catalog::resolve_openai_compatible_profile(
        crate::provider_catalog::OPENAI_COMPAT_PROFILE,
    );
    if let Some(key) = current_key
        && (old.api_key_env != new.api_key_env || old.env_file != new.env_file)
        && crate::provider_catalog::save_env_value_to_env_file(
            &new.api_key_env,
            &new.env_file,
            Some(&key),
        )
        .is_err()
    {
        crate::logging::warn("Failed to migrate OpenAI-compatible API key to new source");
    }
    crate::auth::AuthStatus::invalidate_cache();
    let label = match setting {
        OpenAiCompatSetting::ApiBase => format!("API base → {}", new.api_base),
        OpenAiCompatSetting::ApiKeyName => format!("API key variable → {}", new.api_key_env),
        OpenAiCompatSetting::EnvFile => format!("Env file → {}", new.env_file),
        OpenAiCompatSetting::DefaultModel => format!(
            "Default model hint → {}",
            new.default_model.as_deref().unwrap_or("(unset)")
        ),
    };
    app.set_status_notice(label.clone());
    app.push_display_message(DisplayMessage::system(format!(
        "Saved OpenAI-compatible setting: **{}**.",
        label
    )));
}

fn render_provider_settings_markdown(app: &App, provider_id: &str) -> String {
    let status = crate::auth::AuthStatus::check();
    let cfg = crate::config::Config::load();
    let Some(provider) = resolve_account_provider_descriptor(provider_id) else {
        return format!("Unknown provider `{}`.", provider_id);
    };
    let assessment = status.assessment_for_provider(provider);
    let mut lines = vec![format!("**{}**\n", provider.display_name)];
    lines.push(format!("- Status: **{:?}**", assessment.state));
    lines.push(format!(
        "- Auth: {} ({})",
        provider.auth_kind.label(),
        assessment.method_detail.as_str()
    ));
    lines.push(format!(
        "- Validation: {}",
        assessment
            .last_validation
            .as_ref()
            .map(crate::auth::validation::format_record_label)
            .unwrap_or_else(|| "not validated".to_string())
    ));
    lines.push(format!("- Login command: `/account {} login`", provider.id));
    lines.push(format!(
        "- Doctor command: `/account {} doctor`",
        provider.id
    ));
    lines.push(String::new());

    let recommended_actions = crate::auth::doctor::recommended_actions(provider, &assessment, None);
    if !recommended_actions.is_empty() {
        lines.push("**Recommended next steps**".to_string());
        for action in recommended_actions {
            lines.push(format!("- {}", action));
        }
        lines.push(String::new());
    }

    match provider.id {
        "claude" => {
            lines.push(app.render_anthropic_accounts_markdown());
            lines.push(String::new());
            lines.push("Commands:".to_string());
            lines.push("- `/account claude add`".to_string());
            lines.push("- `/account claude switch <label>`".to_string());
            lines.push("- `/account claude remove <label>`".to_string());
        }
        "openai" => {
            lines.push(app.render_openai_accounts_markdown());
            lines.push(String::new());
            lines.push("**Settings**".to_string());
            lines.push(format!(
                "- Transport: `{}`",
                cfg.provider.openai_transport.as_deref().unwrap_or("auto")
            ));
            lines.push(format!(
                "- Reasoning effort: `{}`",
                cfg.provider
                    .openai_reasoning_effort
                    .as_deref()
                    .unwrap_or("(provider default)")
            ));
            lines.push(format!(
                "- Fast mode: `{}`",
                if cfg.provider.openai_service_tier.as_deref() == Some("priority") {
                    "on"
                } else {
                    "off"
                }
            ));
            lines.push("- `/account openai transport <auto|https|websocket>`".to_string());
            lines.push("- `/account openai effort <none|low|medium|high|xhigh|clear>`".to_string());
            lines.push("- `/account openai fast <on|off>`".to_string());
        }
        "copilot" => {
            lines.push("**Settings**".to_string());
            lines.push(format!(
                "- Premium mode: `{}`",
                cfg.provider.copilot_premium.as_deref().unwrap_or("normal")
            ));
            lines.push("- `/account copilot premium <normal|one|zero>`".to_string());
        }
        "openai-compatible" => {
            let compat = crate::provider_catalog::resolve_openai_compatible_profile(
                crate::provider_catalog::OPENAI_COMPAT_PROFILE,
            );
            lines.push("**Settings**".to_string());
            lines.push("Configure custom OpenAI-compatible endpoints in this order: base URL first, then API key variable/key.".to_string());
            lines.push(format!("- Step 1, API base URL: `{}`", compat.api_base));
            lines.push(format!(
                "- Step 2, API key variable: `{}`",
                compat.api_key_env
            ));
            lines.push(format!("- Env file: `{}`", compat.env_file));
            lines.push(format!(
                "- Default model hint: `{}`",
                compat.default_model.as_deref().unwrap_or("(unset)")
            ));
            lines.push("- `/account openai-compatible api-base <url|clear>`".to_string());
            lines.push("- `/account openai-compatible api-key-name <ENV_VAR|clear>`".to_string());
            lines.push("- `/account openai-compatible env-file <file.env|clear>`".to_string());
            lines.push("- `/account openai-compatible default-model <model|clear>`".to_string());
        }
        _ => {
            lines.push("No provider-specific settings are exposed here yet. Use `/login` to configure credentials.".to_string());
        }
    }

    if provider_id != "defaults" {
        lines.push(String::new());
        lines.push("**Global defaults**".to_string());
        lines.push(format!(
            "- Default provider: `{}`",
            cfg.provider.default_provider.as_deref().unwrap_or("auto")
        ));
        lines.push(format!(
            "- Default model: `{}`",
            cfg.provider
                .default_model
                .as_deref()
                .unwrap_or("(provider default)")
        ));
        lines.push(
            "- `/account default-provider <claude|openai|copilot|gemini|openrouter|auto>`"
                .to_string(),
        );
        lines.push("- `/account default-model <model|clear>`".to_string());
    }

    lines.join("\n")
}

fn render_auth_doctor_markdown(provider_filter: Option<&str>) -> String {
    let status = crate::auth::AuthStatus::check();
    let validation = crate::auth::validation::load_all();
    let providers = match provider_filter {
        Some(provider_id) => match resolve_account_provider_descriptor(provider_id) {
            Some(provider) => vec![provider],
            None => {
                return format!(
                    "Unknown provider `{}`. Use `/account <provider> doctor` with a valid provider id.",
                    provider_id
                );
            }
        },
        None => {
            let configured = crate::provider_catalog::auth_status_login_providers()
                .into_iter()
                .filter(|provider| status.assessment_for_provider(*provider).is_configured())
                .collect::<Vec<_>>();
            if configured.is_empty() {
                crate::provider_catalog::auth_status_login_providers().to_vec()
            } else {
                configured
            }
        }
    };

    let mut sections = Vec::new();
    for provider in providers {
        let assessment = status.assessment_for_provider(provider);
        let validation_label = validation
            .get(provider.id)
            .map(crate::auth::validation::format_record_label);
        let recommended_actions =
            crate::auth::doctor::recommended_actions(provider, &assessment, None);
        let diagnostics = crate::auth::doctor::diagnostics(provider, &assessment, None);
        let needs_attention = crate::auth::doctor::needs_attention(&assessment, None);

        let mut lines = vec![format!("**{}** (`{}`)", provider.display_name, provider.id)];
        lines.push(format!(
            "- Status: {}",
            match assessment.state {
                crate::auth::AuthState::Available => "ready",
                crate::auth::AuthState::Expired => "needs attention",
                crate::auth::AuthState::NotConfigured => "setup needed",
            }
        ));
        lines.push(format!("- Method: {}", assessment.method_detail));
        lines.push(format!("- Health: {}", assessment.health_summary()));
        lines.push(format!(
            "- Credential source: {} ({})",
            assessment.credential_source.label(),
            assessment.credential_source_detail
        ));
        lines.push(format!("- Refresh: {}", assessment.refresh_support.label()));
        lines.push(format!(
            "- Validation method: {}",
            assessment.validation_method.label()
        ));
        lines.push(format!(
            "- Last refresh: {}",
            assessment
                .last_refresh
                .as_ref()
                .map(crate::auth::refresh_state::format_record_label)
                .as_deref()
                .unwrap_or("not recorded")
        ));
        lines.push(format!(
            "- Validation: {}",
            validation_label.as_deref().unwrap_or("not validated")
        ));
        lines.push(format!(
            "- Needs attention: {}",
            if needs_attention { "yes" } else { "no" }
        ));
        if !diagnostics.is_empty() {
            lines.push(String::new());
            lines.push("**Diagnostics**".to_string());
            for diagnostic in diagnostics {
                lines.push(format!("- {}", diagnostic));
            }
        }
        if !recommended_actions.is_empty() {
            lines.push(String::new());
            lines.push("**Next steps**".to_string());
            for action in recommended_actions {
                lines.push(format!("- {}", action));
            }
        }
        sections.push(lines.join("\n"));
    }

    sections.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_account_doctor_subcommands() {
        assert!(matches!(
            parse_account_command("/account doctor"),
            Some(Ok(AccountCommand::Doctor { provider_id: None }))
        ));
        assert!(matches!(
            parse_account_command("/account openai doctor"),
            Some(Ok(AccountCommand::Doctor { provider_id: Some(provider_id) })) if provider_id == "openai"
        ));
    }

    #[test]
    fn render_auth_doctor_markdown_includes_recovery_steps() {
        let _guard = crate::storage::lock_test_env();
        let markdown = render_auth_doctor_markdown(Some("openai"));
        assert!(markdown.contains("**OpenAI** (`openai`)"));
        assert!(markdown.contains("**Next steps**"));
        assert!(markdown.contains("jcode login --provider openai"));
        assert!(markdown.contains("Review current state: `jcode auth status --json`"));
    }
}
