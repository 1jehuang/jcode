//! Read the native picker choices without installing a view or replacing a draft.

use super::{
    App, ModelPickerBuildOutcome, model_effort_step_preselection, model_entry_base_name,
    model_route_effort_levels, picker_route_selection, reasoning_effort_picker_label,
    saved_default_reasoning_effort,
};

impl App {
    pub(in crate::tui::app) fn debug_model_options_json(&self) -> String {
        let context = self.model_picker_build_context();
        let current_model = context.current_model.clone();
        let effort = if self.is_remote {
            self.remote_reasoning_effort.clone()
        } else {
            self.provider.reasoning_effort()
        };
        let routes = if self.is_remote {
            self.remote_model_options.clone()
        } else {
            self.provider.model_routes()
        };
        let entries =
            match Self::build_model_picker_entries(context, routes, 0, std::time::Instant::now()) {
                ModelPickerBuildOutcome::Built { entries, .. } => entries,
                ModelPickerBuildOutcome::Empty => Vec::new(),
            };
        let any_current = entries.iter().any(|entry| entry.is_current);
        let mut choices = Vec::new();
        for entry in &entries {
            for route in &entry.options {
                if super::placeholder_routes::is_placeholder_route_method(&route.api_method) {
                    continue;
                }
                let selection = picker_route_selection(entry, route);
                let model = model_entry_base_name(entry);
                let levels = model_route_effort_levels(&model, route);
                let saved = saved_default_reasoning_effort(&entry.name, route);
                let current = entry.is_current || (!any_current && entry.name == current_model);
                let selected = model_effort_step_preselection(
                    &levels,
                    effort.as_deref().filter(|_| current),
                    saved.as_deref(),
                );
                let reasoning_levels: Vec<_> = levels
                    .iter()
                    .map(|level| {
                        let label = (route.api_method == "chatgpt-web")
                            .then(|| super::super::chatgpt_web_effort_label(&model, level))
                            .flatten()
                            .unwrap_or_else(|| reasoning_effort_picker_label(level).to_string());
                        serde_json::json!({ "value": level, "label": label })
                    })
                    .collect();
                choices.push(serde_json::json!({
                    "id": selection.picker_choice_id(),
                    "name": entry.name, "provider": route.provider, "api_method": route.api_method,
                    "available": route.available, "detail": route.detail, "selection": selection,
                    "reasoning_effort": levels.get(selected), "reasoning_levels": reasoning_levels,
                }));
            }
        }
        serde_json::json!({ "choices": choices, "client_pid": std::process::id(), "session_id": self.active_client_session_id() }).to_string()
    }
}
