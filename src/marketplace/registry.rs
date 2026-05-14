use std::collections::HashMap;
use super::types::{MarketplacePlugin, Category, Review};

pub struct MarketplaceRegistry {
    plugins: HashMap<String, MarketplacePlugin>,
    categories: HashMap<Category, Vec<String>>,
    reviews: HashMap<String, Vec<Review>>,
}

impl MarketplaceRegistry {
    pub fn new() -> Self {
        MarketplaceRegistry {
            plugins: HashMap::new(),
            categories: HashMap::new(),
            reviews: HashMap::new(),
        }
    }

    pub fn register_plugin(&mut self, plugin: MarketplacePlugin) -> Result<(), String> {
        if self.plugins.contains_key(&plugin.id) {
            return Err(format!("Plugin '{}' already registered", plugin.id));
        }

        let category = plugin.category.clone();
        self.plugins.insert(plugin.id.clone(), plugin.clone());

        self.categories
            .entry(category)
            .or_insert_with(Vec::new)
            .push(plugin.id);

        Ok(())
    }

    pub fn get_plugin(&self, id: &str) -> Option<&MarketplacePlugin> {
        self.plugins.get(id)
    }

    pub fn search_plugins(
        &self,
        query: Option<&str>,
        category: Option<&Category>,
        page: usize,
        per_page: usize,
    ) -> SearchResult {
        let mut results: Vec<MarketplacePlugin> = self
            .plugins
            .values()
            .filter(|plugin| {
                if let Some(cat) = category {
                    if &plugin.category != cat {
                        return false;
                    }
                }

                if let Some(q) = query {
                    let q_lower = q.to_lowercase();
                    plugin.name.to_lowercase().contains(&q_lower)
                        || plugin.description.to_lowercase().contains(&q_lower)
                        || plugin.tags.iter().any(|t| t.to_lowercase().contains(&q_lower))
                } else {
                    true
                }
            })
            .cloned()
            .collect();

        results.sort_by(|a, b| b.rating.partial_cmp(&a.rating).unwrap_or(std::cmp::Ordering::Equal));

        let total = results.len();
        let start = (page - 1) * per_page;
        let end = (start + per_page).min(total);
        let paginated = if start < total { results[start..end].to_vec() } else { vec![] };

        SearchResult {
            plugins: paginated,
            total_count: total,
            page,
            per_page,
            query: query.map(|s| s.to_string()),
            categories: if category.is_some() { vec![category.unwrap().clone()] } else { vec![] },
        }
    }

    pub fn get_by_category(&self, category: &Category) -> Vec<&MarketplacePlugin> {
        match self.categories.get(category) {
            Some(ids) => ids
                .iter()
                .filter_map(|id| self.plugins.get(id))
                .collect(),
            None => vec![],
        }
    }

    pub fn get_popular(&self, limit: usize) -> Vec<&MarketplacePlugin> {
        let mut plugins: Vec<_> = self.plugins.values().collect();
        plugins.sort_by(|a, b| b.total_downloads.cmp(&a.total_downloads));
        plugins.into_iter().take(limit).collect()
    }

    pub fn get_recent(&self, limit: usize) -> Vec<&MarketplacePlugin> {
        let mut plugins: Vec<_> = self.plugins.values().collect();
        plugins.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        plugins.into_iter().take(limit).collect()
    }

    pub fn get_top_rated(&self, limit: usize) -> Vec<&MarketplacePlugin> {
        let mut plugins: Vec<_> = self.plugins.values()
            .filter(|p| p.review_count >= 5)
            .collect();
        plugins.sort_by(|a, b| b.rating.partial_cmp(&a.rating).unwrap_or(std::cmp::Ordering::Equal));
        plugins.into_iter().take(limit).collect()
    }

    pub fn add_review(&mut self, plugin_id: &str, review: Review) -> Result<(), String> {
        if !self.plugins.contains_key(plugin_id) {
            return Err(format!("Plugin '{}' not found", plugin_id));
        }

        self.reviews
            .entry(plugin_id.to_string())
            .or_insert_with(Vec::new)
            .push(review);

        Ok(())
    }

    pub fn get_reviews(&self, plugin_id: &str) -> Option<&Vec<Review>> {
        self.reviews.get(plugin_id)
    }

    pub fn count(&self) -> usize { self.plugins.len() }
}
