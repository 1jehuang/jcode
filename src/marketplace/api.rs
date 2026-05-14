use actix_web::{web, HttpResponse, Result, Json};
use serde::{Deserialize, Serialize};

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

    pub async fn list_plugins(
        data: web::Data<Arc<std::sync::Mutex<MarketplaceRegistry>>>,
        web::Query(query): web::Query<ListQuery>,
    ) -> Result<HttpResponse> {
        let registry = data.lock().unwrap();
        let category = query.category.and_then(|c| Category::all().into_iter().find(|cat| format!("{:?}", cat).to_lowercase() == c.to_lowercase()));
        let result = registry.search_plugins(
            query.q.as_deref(),
            category.as_ref(),
            query.page.unwrap_or(1),
            query.per_page.unwrap_or(20),
        );

        Ok(HttpResponse::Ok().json(result))
    }

    pub async fn get_plugin_detail(
        data: web::Data<Arc<std::sync::Mutex<MarketplaceRegistry>>>,
        path: web::Path<String>,
    ) -> Result<HttpResponse> {
        let plugin_id = path.into_inner();
        let registry = data.lock().unwrap();

        match registry.get_plugin(&plugin_id) {
            Some(plugin) => Ok(HttpResponse::Ok().json(plugin)),
            None => Ok(HttpResponse::NotFound().json(serde_json::json!({
                "error": "Plugin not found",
                "id": plugin_id
            }))),
        }
    }

    pub async fn get_categories() -> Result<HttpResponse> {
        let categories = Category::all()
            .iter()
            .map(|c| serde_json::json!({
                "name": format!("{:?}", c),
                "display": c.display(),
            }))
            .collect::<Vec<_>>();

        Ok(HttpResponse::Ok().json(categories))
    }

    pub async fn search(
        data: web::Data<Arc<std::sync::Mutex<MarketplaceRegistry>>>,
        web::Query(query): web::Query<SearchQuery>,
    ) -> Result<HttpResponse> {
        let registry = data.lock().unwrap();
        let result = if let Some(ref q) = query.q {
            registry.search_plugins(Some(q), None, 1, 20)
        } else {
            registry.search_plugins(None, None, 1, 20)
        };

        Ok(HttpResponse::Ok().json(result))
    }

    pub async fn popular(
        data: web::Data<Arc<std::sync::Mutex<MarketplaceRegistry>>>,
    ) -> Result<HttpResponse> {
        let registry = data.lock().unwrap();
        let plugins = registry.get_popular(10);
        Ok(HttpResponse::Ok().json(plugins))
    }

    pub async fn register_plugin(
        data: web::Data<Arc<std::sync::Mutex<MarketplaceRegistry>>>,
        Json(plugin): Json<MarketplacePlugin>,
    ) -> Result<HttpResponse> {
        let mut registry = data.lock().unwrap();

        match registry.register_plugin(plugin.clone()) {
            Ok(_) => Ok(HttpResponse::Created().json(serde_json::json!({
                "message": "Plugin registered successfully",
                "plugin_id": plugin.id
            }))),
            Err(e) => Ok(HttpResponse::BadRequest().json(serde_json::json!({
                "error": e
            }))),
        }
    }

    pub async fn health_check() -> Result<HttpResponse> {
        Ok(HttpResponse::Ok().json(serde_json::json!({
            "status": "healthy",
            "service": "carpai-marketplace-api",
            "version": env!("CARGO_PKG_VERSION"),
        })))
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
