use crate::ModelRoute;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

type CatalogRouteKey = (String, String, String);
type CatalogRouteSnapshot = (bool, String, Option<u64>);
type CatalogRouteMap = BTreeMap<CatalogRouteKey, CatalogRouteSnapshot>;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelCatalogRefreshSummary {
    pub model_count_before: usize,
    pub model_count_after: usize,
    pub models_added: usize,
    pub models_removed: usize,
    pub route_count_before: usize,
    pub route_count_after: usize,
    pub routes_added: usize,
    pub routes_removed: usize,
    pub routes_changed: usize,
}

pub fn summarize_model_catalog_refresh(
    before_models: Vec<String>,
    after_models: Vec<String>,
    before_routes: Vec<ModelRoute>,
    after_routes: Vec<ModelRoute>,
) -> ModelCatalogRefreshSummary {
    fn is_display_only_age_suffix(detail: &str) -> bool {
        let detail = detail.trim();
        ["m ago", "h ago", "d ago"]
            .iter()
            .find_map(|suffix| detail.strip_suffix(suffix))
            .is_some_and(|prefix| !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()))
    }

    fn normalize_route_refresh_detail(detail: &str) -> String {
        let detail = detail.trim();
        if detail.is_empty() {
            return String::new();
        }
        if is_display_only_age_suffix(detail) {
            return String::new();
        }
        if let Some((prefix, suffix)) = detail.rsplit_once(',')
            && is_display_only_age_suffix(suffix)
        {
            return prefix.trim_end().trim_end_matches(',').trim().to_string();
        }
        detail.to_string()
    }

    let before_model_set: BTreeSet<String> = before_models.into_iter().collect();
    let after_model_set: BTreeSet<String> = after_models.into_iter().collect();

    let before_route_map: CatalogRouteMap = before_routes
        .into_iter()
        .map(|route| {
            let estimated_cost = route.estimated_reference_cost_micros();
            (
                (route.model, route.provider, route.api_method),
                (
                    route.available,
                    normalize_route_refresh_detail(&route.detail),
                    estimated_cost,
                ),
            )
        })
        .collect();
    let after_route_map: CatalogRouteMap = after_routes
        .into_iter()
        .map(|route| {
            let estimated_cost = route.estimated_reference_cost_micros();
            (
                (route.model, route.provider, route.api_method),
                (
                    route.available,
                    normalize_route_refresh_detail(&route.detail),
                    estimated_cost,
                ),
            )
        })
        .collect();

    let models_added = after_model_set.difference(&before_model_set).count();
    let models_removed = before_model_set.difference(&after_model_set).count();
    let routes_added = after_route_map
        .keys()
        .filter(|key| !before_route_map.contains_key(*key))
        .count();
    let routes_removed = before_route_map
        .keys()
        .filter(|key| !after_route_map.contains_key(*key))
        .count();
    let routes_changed = after_route_map
        .iter()
        .filter(|(key, value)| {
            before_route_map
                .get(*key)
                .is_some_and(|before| before != *value)
        })
        .count();

    ModelCatalogRefreshSummary {
        model_count_before: before_model_set.len(),
        model_count_after: after_model_set.len(),
        models_added,
        models_removed,
        route_count_before: before_route_map.len(),
        route_count_after: after_route_map.len(),
        routes_added,
        routes_removed,
        routes_changed,
    }
}
