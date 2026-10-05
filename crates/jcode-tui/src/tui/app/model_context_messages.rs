//! User-facing status/error messages for model listing and switching.

pub(in crate::tui::app) fn is_refresh_model_list_command(trimmed: &str) -> bool {
    trimmed == "/refresh-model-list"
}

pub(in crate::tui::app) fn format_model_refresh_summary(
    summary: &crate::provider::ModelCatalogRefreshSummary,
) -> String {
    let mut message = format!(
        "Model List Refresh Complete\n\nModels: {} → {}  (+{} / -{})\nRoutes: {} → {}  (+{} / -{} / ~{})",
        summary.model_count_before,
        summary.model_count_after,
        summary.models_added,
        summary.models_removed,
        summary.route_count_before,
        summary.route_count_after,
        summary.routes_added,
        summary.routes_removed,
        summary.routes_changed,
    );
    append_model_name_diff(&mut message, summary);
    message
}

pub(in crate::tui::app) fn append_model_name_diff(
    message: &mut String,
    summary: &crate::provider::ModelCatalogRefreshSummary,
) {
    if !summary.models_added_names.is_empty() {
        message.push_str("\nAdded models: ");
        message.push_str(&format_model_name_list(&summary.models_added_names, 12));
    }
    if !summary.models_removed_names.is_empty() {
        message.push_str("\nRemoved models: ");
        message.push_str(&format_model_name_list(&summary.models_removed_names, 12));
    }
}

pub(in crate::tui::app) fn format_model_name_list(models: &[String], limit: usize) -> String {
    let shown = models
        .iter()
        .take(limit)
        .map(|model| model.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    if models.len() > limit {
        format!("{} … and {} more", shown, models.len() - limit)
    } else {
        shown
    }
}

pub(in crate::tui::app) fn no_models_available_message(is_remote: bool) -> String {
    let mut lines = vec![
        "No models are available right now.".to_string(),
        String::new(),
        "Next steps:".to_string(),
        "  - Run /login to connect or refresh a provider".to_string(),
        "  - Run /account to inspect or switch credentials".to_string(),
        "  - If you just logged in, wait a moment and try /model again".to_string(),
    ];

    if is_remote {
        lines.push(
            "  - If this is a remote session, reconnect if the server model list looks stale"
                .to_string(),
        );
    }

    lines.join("\n")
}

pub(in crate::tui::app) fn model_switch_failure_message(error: &str, is_remote: bool) -> String {
    let mut lines = vec![
        format!("Failed to switch model: {}", error),
        String::new(),
        "Next steps:".to_string(),
        "  - Use /model to choose another available route".to_string(),
        "  - Run /login to add or refresh credentials".to_string(),
        "  - Run /account to inspect or switch accounts".to_string(),
    ];

    if is_remote {
        lines.push(
            "  - If this is a remote session and the list looks stale, reconnect and try again"
                .to_string(),
        );
    }

    lines.join("\n")
}

pub(in crate::tui::app) fn unavailable_model_route_message(
    model: &str,
    provider: &str,
    detail: &str,
    is_remote: bool,
) -> String {
    let reason = if detail.trim().is_empty() {
        "This route is not currently available.".to_string()
    } else {
        format!("This route is not currently available: {}", detail.trim())
    };

    let mut lines = vec![
        format!("Cannot use {} via {} right now.", model, provider),
        String::new(),
        reason,
        String::new(),
        "Next steps:".to_string(),
        "  - Pick another available row in /model".to_string(),
        "  - Run /login to add or refresh credentials".to_string(),
        "  - Run /account to inspect or switch accounts".to_string(),
    ];

    if is_remote {
        lines.push(
            "  - If this is a remote session, wait a moment or reconnect if the catalog looks stale"
                .to_string(),
        );
    }

    lines.join("\n")
}
