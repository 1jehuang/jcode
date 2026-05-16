use super::types::{MarketplacePlugin, SearchResult, Category, Review, PluginVersion};
use super::registry::MarketplaceRegistry;

pub struct MarketplaceClient {
    registry: MarketplaceRegistry,
    base_url: String,
}

impl MarketplaceClient {
    pub fn new(registry: MarketplaceRegistry) -> Self {
        MarketplaceClient {
            registry,
            base_url: "https://marketplace.carpai.dev".to_string(),
        }
    }

    pub fn with_base_url(mut self, url: &str) -> Self {
        self.base_url = url.to_string();
        self
    }

    pub fn search(&self, query: &str, page: usize) -> SearchResult {
        self.registry.search_plugins(Some(query), None, page, 20)
    }

    pub fn browse_category(&self, category: &Category, page: usize) -> SearchResult {
        self.registry.search_plugins(None, Some(category), page, 20)
    }

    pub fn get_plugin(&self, id: &str) -> Option<&MarketplacePlugin> {
        self.registry.get_plugin(id)
    }

    pub fn get_popular(&self, limit: usize) -> Vec<&MarketplacePlugin> {
        self.registry.get_popular(limit)
    }

    pub fn get_recent(&self, limit: usize) -> Vec<&MarketplacePlugin> {
        self.registry.get_recent(limit)
    }

    pub fn get_top_rated(&self, limit: usize) -> Vec<&MarketplacePlugin> {
        self.registry.get_top_rated(limit)
    }

    pub fn get_categories(&self) -> Vec<Category> {
        Category::all()
    }

    pub fn install(&self, plugin_id: &str) -> Result<String, String> {
        let plugin = self.registry.get_plugin(plugin_id)
            .ok_or_else(|| format!("Plugin '{}' not found in marketplace", plugin_id))?;

        let latest_version = plugin.versions.iter()
            .find(|v| v.version == plugin.latest_version)
            .ok_or("Latest version not found")?;

        Ok(format!(
            "Installing {} v{} from {}\nSize: {} bytes\nDownloads: {}",
            plugin.name,
            latest_version.version,
            latest_version.download_url,
            latest_version.size_bytes,
            plugin.total_downloads
        ))
    }

    pub fn check_updates(&self, installed_version: &str, plugin_id: &str) -> Option<PluginVersion> {
        if let Some(plugin) = self.registry.get_plugin(plugin_id) {
            if plugin.latest_version != installed_version {
                return plugin.versions.iter()
                    .find(|v| v.version == plugin.latest_version)
                    .cloned();
            }
        }
        None
    }

    pub fn submit_review(
        &mut self,
        plugin_id: &str,
        user_id: &str,
        username: &str,
        rating: u8,
        title: &str,
        content: &str,
    ) -> Result<(), String> {
        use chrono::Utc;
        use uuid::Uuid;

        if rating == 0 || rating > 5 {
            return Err("Rating must be between 1 and 5".to_string());
        }

        let review = Review {
            id: Uuid::new_v4().to_string(),
            user_id: user_id.to_string(),
            username: username.to_string(),
            rating,
            title: title.to_string(),
            content: content.to_string(),
            created_at: Utc::now(),
            helpful_count: 0,
        };

        self.registry.add_review(plugin_id, review)?;
        Ok(())
    }

    pub fn get_reviews(&self, plugin_id: &str) -> Option<&Vec<Review>> {
        self.registry.get_reviews(plugin_id)
    }

    pub fn total_plugins(&self) -> usize { self.registry.count() }
}
