//! Claude Code CLI model routes.

use super::*;

/// Claude Code CLI routes: one per configured instance per known Claude model.
/// The default instance uses the bare `claude-code` api method, other
/// instances `claude-code:<id>`, so picking a route also picks the account.
pub(super) fn append_claude_code_routes(provider: &MultiProvider, routes: &mut Vec<ModelRoute>) {
    if let Some(claude_code) = provider.claude_code_provider() {
        routes.extend(claude_code.model_routes());
    }
}

/// Provider label for a Claude Code instance route.
pub(crate) fn claude_code_route_provider_label(
    settings: &crate::config::ClaudeCodeConfig,
    instance: &crate::config::ClaudeCodeInstanceConfig,
) -> String {
    if let Some(label) = &instance.jcode_account {
        return format!("Claude Code ({label})");
    }
    match instance.display_name.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => name.to_string(),
        _ if settings.is_default_instance(&instance.id) => "Claude Code".to_string(),
        _ => format!("Claude Code ({})", instance.id),
    }
}

/// Routes for every configured instance, each from its own discovered models:
/// instances have separate logins, so their model access can differ.
pub fn claude_code_routes_for(
    settings: &crate::config::ClaudeCodeConfig,
    models_for: impl Fn(&crate::config::ClaudeCodeInstanceConfig) -> Vec<String>,
) -> Vec<ModelRoute> {
    let mut routes = Vec::new();
    let accounts = if settings.jcode_accounts.is_empty() {
        Vec::new()
    } else {
        crate::auth::claude::list_accounts().unwrap_or_default()
    };
    for instance in settings.effective_instances() {
        let api_method = jcode_provider_core::claude_code_api_method(
            (!settings.is_default_instance(&instance.id)).then_some(instance.id.as_str()),
        );
        let provider_label = claude_code_route_provider_label(settings, &instance);
        let mut models = models_for(&instance);
        let detail = match (&instance.jcode_account, instance.resolved_home()) {
            (Some(label), _) => {
                // The CLI only reports its few aliases; the account's own
                // Anthropic catalog is what it can run. shortcut: 1M-context
                // variants are left out because their extra-usage gate is
                // not known per account; list them once that is tracked.
                let mut catalog: Vec<String> =
                    crate::provider::known_anthropic_model_ids_for_scope(&format!(
                        "oauth::{label}"
                    ))
                    .into_iter()
                    .filter(|model| !model.ends_with("[1m]"))
                    .collect();
                for model in models {
                    if !catalog.contains(&model) {
                        catalog.push(model);
                    }
                }
                models = catalog;
                match accounts
                    .iter()
                    .find(|account| &account.label == label)
                    .and_then(|account| account.email.as_deref())
                {
                    Some(email) => format!("account {label} · {email} · via Claude Code CLI"),
                    None => format!("account {label} · via Claude Code CLI"),
                }
            }
            (None, Some(home)) => {
                format!("Claude Code CLI, CLAUDE_CONFIG_DIR={}", home.display())
            }
            (None, None) => "Claude Code CLI, default login".to_string(),
        };
        for model in models {
            routes.push(ModelRoute {
                catalog_metadata: None,
                model,
                provider: provider_label.clone(),
                api_method: api_method.clone(),
                available: true,
                detail: detail.clone(),
                usage: None,
                cheapness: None,
            });
        }
    }
    routes
}
