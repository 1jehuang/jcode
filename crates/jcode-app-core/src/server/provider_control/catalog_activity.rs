use super::ModelCatalogRefreshSummary;

pub(super) fn format_auth_catalog_refresh_complete(
    provider_name: Option<&str>,
    provider_model: Option<&str>,
    summary: &ModelCatalogRefreshSummary,
    has_warning: bool,
) -> String {
    let provider_label = provider_name.unwrap_or("provider");
    let title = provider_model
        .map(|model| format!("**Model ready:** `{model}`"))
        .unwrap_or_else(|| "**Model access refreshed**".to_string());
    let changed = summary.models_added > 0
        || summary.models_removed > 0
        || summary.routes_added > 0
        || summary.routes_removed > 0
        || summary.routes_changed > 0;
    let catalog_status = if has_warning {
        if changed {
            format!("{provider_label} catalog changed; some routes missing. Use `/model`.")
        } else {
            format!("{provider_label} catalog unchanged; some routes missing. Use `/model`.")
        }
    } else if changed {
        format!(
            "{provider_label} catalog changed: models +{}/-{}, routes +{}/-{}/~{}. Use `/model`.",
            summary.models_added,
            summary.models_removed,
            summary.routes_added,
            summary.routes_removed,
            summary.routes_changed,
        )
    } else {
        format!(
            "{provider_label} catalog unchanged: {} models, {} routes. Use `/model`.",
            summary.model_count_after, summary.route_count_after,
        )
    };
    format!("{title}\n{catalog_status}")
}
