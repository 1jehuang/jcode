use super::*;

pub(super) fn render_provider_settings_markdown(app: &App, provider_id: &str) -> String {
    let status = crate::auth::AuthStatus::check();
    let cfg = crate::config::Config::load();
    let Some(provider) = resolve_account_provider_descriptor(provider_id) else {
        return format!("Unknown provider {}.", provider_id);
    };
    let assessment = status.assessment_for_provider(provider);
    let mut lines = vec![format!("{}\n", provider.display_name)];
    lines.push(format!("  - Status: {:?}", assessment.state));
    lines.push(format!(
        "  - Auth: {} ({})",
        provider.auth_kind.label(),
        assessment.method_detail.as_str()
    ));
    lines.push(format!(
        "  - Validation: {}",
        assessment
            .last_validation
            .as_ref()
            .map(crate::auth::validation::format_record_label)
            .unwrap_or_else(|| "not validated".to_string())
    ));
    lines.push(format!("  - Login command: /account {} login", provider.id));
    lines.push(format!(
        "  - Doctor command: /account {} doctor",
        provider.id
    ));
    lines.push(String::new());

    let recommended_actions = crate::auth::doctor::recommended_actions(provider, &assessment, None);
    if !recommended_actions.is_empty() {
        lines.push("Recommended next steps".to_string());
        for action in recommended_actions {
            lines.push(format!("  - {}", action));
        }
        lines.push(String::new());
    }

    match provider.id {
        "claude" => {
            lines.push(app.render_anthropic_accounts_markdown());
            lines.push(String::new());
            lines.push("Commands:".to_string());
            lines.push("  - /account claude add".to_string());
            lines.push("  - /account claude switch <label>".to_string());
            lines.push("  - /account claude remove <label>".to_string());
        }
        "openai" => {
            lines.push(app.render_openai_accounts_markdown());
            lines.push(String::new());
            lines.push("Settings".to_string());
            lines.push(format!(
                "  - Transport: {}",
                cfg.provider.openai_transport.as_deref().unwrap_or("auto")
            ));
            lines.push(format!(
                "  - Reasoning effort: {}",
                cfg.provider
                    .openai_reasoning_effort
                    .as_deref()
                    .unwrap_or("(provider default)")
            ));
            lines.push(format!(
                "  - Fast mode: {}",
                if cfg.provider.openai_service_tier.as_deref() == Some("priority") {
                    "on"
                } else {
                    "off"
                }
            ));
            lines.push("  - /account openai transport <auto|https|websocket>".to_string());
            lines.push(
                "  - /account openai effort <none|minimal|low|medium|high|xhigh|max|clear>"
                    .to_string(),
            );
            lines.push("  - /account openai fast <on|off>".to_string());
        }
        "copilot" => {
            lines.push("Settings".to_string());
            lines.push(format!(
                "  - Premium mode: {}",
                cfg.provider.copilot_premium.as_deref().unwrap_or("normal")
            ));
            lines.push("  - /account copilot premium <normal|one|zero>".to_string());
        }
        "openai-compatible" => {
            let compat = crate::provider_catalog::resolve_openai_compatible_profile(
                crate::provider_catalog::OPENAI_COMPAT_PROFILE,
            );
            lines.push("Settings".to_string());
            lines.push("Configure custom OpenAI-compatible endpoints in this order: base URL first, then API key variable/key.".to_string());
            lines.push(format!("  - Step 1, API base URL: {}", compat.api_base));
            lines.push(format!(
                "  - Step 2, API key variable: {}",
                compat.api_key_env
            ));
            lines.push(format!("  - Env file: {}", compat.env_file));
            lines.push(format!(
                "  - Default model hint: {}",
                compat.default_model.as_deref().unwrap_or("(unset)")
            ));
            lines.push("  - /account openai-compatible api-base <url|clear>".to_string());
            lines.push("  - /account openai-compatible api-key-name <ENV_VAR|clear>".to_string());
            lines.push("  - /account openai-compatible env-file <file.env|clear>".to_string());
            lines.push("  - /account openai-compatible default-model <model|clear>".to_string());
        }
        _ => {
            lines.push("No provider-specific settings are exposed here yet. Use /login to configure credentials.".to_string());
        }
    }

    if provider_id != "defaults" {
        lines.push(String::new());
        lines.push("Global defaults".to_string());
        lines.push(format!(
            "  - Default provider: {}",
            cfg.provider.default_provider.as_deref().unwrap_or("auto")
        ));
        lines.push(format!(
            "  - Default model: {}",
            cfg.provider
                .default_model
                .as_deref()
                .unwrap_or("(provider default)")
        ));
        lines.push(
            "  - /account default-provider <claude|openai|copilot|gemini|openrouter|auto>"
                .to_string(),
        );
        lines.push("  - /account default-model <model|clear>".to_string());
    }

    lines.join("\n")
}

pub(super) fn render_auth_doctor_markdown(provider_filter: Option<&str>) -> String {
    let status = crate::auth::AuthStatus::check();
    let validation = crate::auth::validation::load_all();
    let providers = match provider_filter {
        Some(provider_id) => match resolve_account_provider_descriptor(provider_id) {
            Some(provider) => vec![provider],
            None => {
                return format!(
                    "Unknown provider {}. Use /account <provider> doctor with a valid provider id.",
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

        let mut lines = vec![format!("{} ({})", provider.display_name, provider.id)];
        lines.push(format!(
            "  - Status: {}",
            match assessment.state {
                crate::auth::AuthState::Available => "ready",
                crate::auth::AuthState::Expired => "needs attention",
                crate::auth::AuthState::NotConfigured => "setup needed",
            }
        ));
        lines.push(format!("  - Method: {}", assessment.method_detail));
        lines.push(format!("  - Health: {}", assessment.health_summary()));
        lines.push(format!(
            "  - Credential source: {} ({})",
            assessment.credential_source.label(),
            assessment.credential_source_detail
        ));
        lines.push(format!(
            "  - Refresh: {}",
            assessment.refresh_support.label()
        ));
        lines.push(format!(
            "  - Validation method: {}",
            assessment.validation_method.label()
        ));
        lines.push(format!(
            "  - Last refresh: {}",
            assessment
                .last_refresh
                .as_ref()
                .map(crate::auth::refresh_state::format_record_label)
                .as_deref()
                .unwrap_or("not recorded")
        ));
        lines.push(format!(
            "  - Validation: {}",
            validation_label.as_deref().unwrap_or("not validated")
        ));
        lines.push(format!(
            "  - Needs attention: {}",
            if needs_attention { "yes" } else { "no" }
        ));
        if !diagnostics.is_empty() {
            lines.push(String::new());
            lines.push("Diagnostics".to_string());
            for diagnostic in diagnostics {
                lines.push(format!("  - {}", diagnostic));
            }
        }
        if !recommended_actions.is_empty() {
            lines.push(String::new());
            lines.push("Next steps".to_string());
            for action in recommended_actions {
                lines.push(format!("  - {}", action));
            }
        }
        sections.push(lines.join("\n"));
    }

    sections.join("\n\n")
}
