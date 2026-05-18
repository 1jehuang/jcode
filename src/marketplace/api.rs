use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use super::types::*;
use super::registry::MarketplaceRegistry;
use std::sync::Arc;

pub struct MarketplaceApi {
    registry: Arc<std::sync::Mutex<MarketplaceRegistry>>,
}

impl MarketplaceApi {
    pub fn new(registry: MarketplaceRegistry) -> Self {
        MarketplaceApi {
            registry: Arc::new(std::sync::Mutex::new(registry)),
        }
    }

    pub fn registry(&self) -> Arc<std::sync::Mutex<MarketplaceRegistry>> {
        Arc::clone(&self.registry)
    }

    pub async fn list_plugins(
        State(registry): State<Arc<std::sync::Mutex<MarketplaceRegistry>>>,
        Query(query): Query<ListQuery>,
    ) -> Response {
        let reg = registry.lock().unwrap_or_else(|e| e.into_inner());
        let category = query.category.as_ref().and_then(|c| {
            Category::all().into_iter().find(|cat| format!("{:?}", cat).to_lowercase() == c.to_lowercase())
        });
        let result = reg.search_plugins(
            query.q.as_deref(),
            category.as_ref(),
            query.page.unwrap_or(1),
            query.per_page.unwrap_or(20),
        );

        (StatusCode::OK, axum::Json(result)).into_response()
    }

    pub async fn get_plugin_detail(
        State(registry): State<Arc<std::sync::Mutex<MarketplaceRegistry>>>,
        Path(plugin_id): Path<String>,
    ) -> Response {
        let reg = registry.lock().unwrap_or_else(|e| e.into_inner());

        match reg.get_plugin(&plugin_id) {
            Some(plugin) => axum::Json(plugin).into_response(),
            None => (
                StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({
                    "error": "Plugin not found",
                    "id": plugin_id
                })),
            )
                .into_response(),
        }
    }

    pub async fn get_categories() -> Response {
        let categories = Category::all()
            .iter()
            .map(|c| serde_json::json!({
                "name": format!("{:?}", c),
                "display": c.display(),
            }))
            .collect::<Vec<_>>();

        axum::Json(categories).into_response()
    }

    pub async fn search(
        State(registry): State<Arc<std::sync::Mutex<MarketplaceRegistry>>>,
        Query(query): Query<SearchQuery>,
    ) -> Response {
        let reg = registry.lock().unwrap_or_else(|e| e.into_inner());
        let result = if let Some(ref q) = query.q {
            reg.search_plugins(Some(q), None, 1, 20)
        } else {
            reg.search_plugins(None, None, 1, 20)
        };

        axum::Json(result).into_response()
    }

    pub async fn popular(
        State(registry): State<Arc<std::sync::Mutex<MarketplaceRegistry>>>,
    ) -> Response {
        let reg = registry.lock().unwrap_or_else(|e| e.into_inner());
        let plugins = reg.get_popular(10);
        axum::Json(plugins).into_response()
    }

    pub async fn register_plugin(
        State(registry): State<Arc<std::sync::Mutex<MarketplaceRegistry>>>,
        Json(plugin): Json<MarketplacePlugin>,
    ) -> Response {
        let mut reg = registry.lock().unwrap_or_else(|e| e.into_inner());

        match reg.register_plugin(plugin.clone()) {
            Ok(_) => (
                StatusCode::CREATED,
                axum::Json(serde_json::json!({
                    "message": "Plugin registered successfully",
                    "plugin_id": plugin.id
                })),
            )
                .into_response(),
            Err(e) => (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({
                    "error": e
                })),
            )
                .into_response(),
        }
    }

    pub async fn health_check() -> Response {
        axum::Json(serde_json::json!({
            "status": "healthy",
            "service": "carpai-marketplace-api",
            "version": env!("CARGO_PKG_VERSION"),
        }))
        .into_response()
    }
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub q: Option<String>,
    pub category: Option<String>,
    pub page: Option<usize>,
    pub per_page: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
}
