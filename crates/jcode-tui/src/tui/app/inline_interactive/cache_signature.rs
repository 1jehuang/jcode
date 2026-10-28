use super::*;

impl App {
    pub(super) fn model_route_cache_marker(route: &crate::provider::ModelRoute) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
            route.model, route.provider, route.api_method, route.available, route.detail
        )
    }

    pub(super) fn model_picker_cache_signature(
        &self,
        current_model: &str,
        config_default_model: Option<String>,
        config_default_provider: Option<String>,
        current_effort: Option<String>,
        available_efforts: &[&str],
    ) -> ModelPickerCacheSignature {
        ModelPickerCacheSignature {
            is_remote: self.is_remote,
            provider_name: if self.is_remote {
                self.remote_provider_name
                    .clone()
                    .unwrap_or_else(|| "remote".to_string())
            } else {
                self.provider.name().to_string()
            },
            current_model: current_model.to_string(),
            config_default_model,
            config_default_provider,
            reasoning_effort: current_effort,
            available_efforts: available_efforts
                .iter()
                .map(|effort| (*effort).to_string())
                .collect(),
            simplified_model_picker: crate::perf::tui_policy().simplified_model_picker,
            catalog_revision: self.model_picker_catalog_revision,
            remote_provider_name: self.remote_provider_name.clone(),
            remote_available_len: self.remote_available_entries.len(),
            remote_available_first: self.remote_available_entries.first().cloned(),
            remote_available_last: self.remote_available_entries.last().cloned(),
            remote_routes_len: self.remote_model_options.len(),
            remote_routes_first: self
                .remote_model_options
                .first()
                .map(Self::model_route_cache_marker),
            remote_routes_last: self
                .remote_model_options
                .last()
                .map(Self::model_route_cache_marker),
        }
    }
}
