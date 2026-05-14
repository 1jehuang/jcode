use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplacePlugin {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub category: Category,
    pub versions: Vec<PluginVersion>,
    pub latest_version: String,
    pub total_downloads: u64,
    pub rating: f64,
    pub review_count: usize,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub repository_url: Option<String>,
    pub homepage_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Category {
    Development,
    Productivity,
    Integration,
    Theme,
    Language,
    Tool,
    AI,
    Security,
    Other(String),
}

impl Category {
    pub fn display(&self) -> &'static str {
        match self {
            Category::Development => "🛠️ Development",
            Category::Productivity => "⚡ Productivity",
            Category::Integration => "🔗 Integration",
            Category::Theme => "🎨 Theme",
            Category::Language => "💬 Language",
            Category::Tool => "🔧 Tool",
            Category::AI => "🤖 AI & ML",
            Category::Security => "🔒 Security",
            Category::Other(name) => name.as_str(),
        }
    }

    pub fn all() -> Vec<Category> {
        vec![
            Category::Development,
            Category::Productivity,
            Category::Integration,
            Category::Theme,
            Category::Language,
            Category::Tool,
            Category::AI,
            Category::Security,
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginVersion {
    pub version: String,
    pub changelog: Vec<String>,
    pub download_url: String,
    pub size_bytes: u64,
    pub published_at: DateTime<Utc>,
    pub min_carpai_version: String,
    pub dependencies: Vec<PluginDependency>,
    pub checksum_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDependency {
    pub plugin_id: String,
    pub version_requirement: String,
    pub optional: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Review {
    pub id: String,
    pub user_id: String,
    pub username: String,
    pub rating: u8,
    pub title: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub helpful_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub plugins: Vec<MarketplacePlugin>,
    pub total_count: usize,
    pub page: usize,
    pub per_page: usize,
    pub query: Option<String>,
    pub categories: Vec<Category>,
}
