use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowlistEntry {
    pub tool_name: String,
    pub description: String,
    pub category: ToolCategory,
    pub allowed_args: Vec<String>,
    pub blocked_args: Vec<String>,
    pub requires_confirmation: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ToolCategory {
    ReadOnly,
    Write,
    Network,
    System,
    Utility,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowlistConfig {
    pub enabled: bool,
    pub default_action: DefaultAction,
    pub max_list_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DefaultAction {
    Allow,
    Deny,
    RequireConfirmation,
}

#[derive(Debug, Clone)]
pub struct AllowlistManager {
    entries: Arc<RwLock<HashMap<String, AllowlistEntry>>>,
    categories: Arc<RwLock<HashMap<ToolCategory, HashSet<String>>>>,
    config: AllowlistConfig,
}

impl Default for AllowlistConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_action: DefaultAction::Deny,
            max_list_size: 1000,
        }
    }
}

impl AllowlistManager {
    pub fn new(config: AllowlistConfig) -> Self {
        let manager = Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            categories: Arc::new(RwLock::new(HashMap::new())),
            config,
        };
        manager.initialize_defaults();
        manager
    }

    pub fn default() -> Self {
        Self::new(AllowlistConfig::default())
    }

    fn initialize_defaults(&self) {
        let defaults = Self::get_default_entries();
        let entries = self.entries.clone();
        let categories = self.categories.clone();
        
        tokio::spawn(async move {
            for entry in defaults {
                let mut entries_lock = entries.write().await;
                let _ = entries_lock.insert(entry.tool_name.clone(), entry.clone());
                
                let mut categories_lock = categories.write().await;
                categories_lock
                    .entry(entry.category)
                    .or_insert_with(HashSet::new)
                    .insert(entry.tool_name);
            }
        });
    }

    fn get_default_entries() -> Vec<AllowlistEntry> {
        let now = chrono::Utc::now();
        
        vec![
            AllowlistEntry {
                tool_name: "file_read".to_string(),
                description: "Read file contents".to_string(),
                category: ToolCategory::ReadOnly,
                allowed_args: vec!["path".to_string(), "offset".to_string(), "length".to_string()],
                blocked_args: vec![],
                requires_confirmation: false,
                created_at: now,
            },
            AllowlistEntry {
                tool_name: "read_file".to_string(),
                description: "Read entire file".to_string(),
                category: ToolCategory::ReadOnly,
                allowed_args: vec!["path".to_string()],
                blocked_args: vec![],
                requires_confirmation: false,
                created_at: now,
            },
            AllowlistEntry {
                tool_name: "grep".to_string(),
                description: "Search in files".to_string(),
                category: ToolCategory::ReadOnly,
                allowed_args: vec!["pattern".to_string(), "path".to_string()],
                blocked_args: vec![],
                requires_confirmation: false,
                created_at: now,
            },
            AllowlistEntry {
                tool_name: "glob".to_string(),
                description: "List files matching pattern".to_string(),
                category: ToolCategory::ReadOnly,
                allowed_args: vec!["pattern".to_string()],
                blocked_args: vec![],
                requires_confirmation: false,
                created_at: now,
            },
            AllowlistEntry {
                tool_name: "list_files".to_string(),
                description: "List files in directory".to_string(),
                category: ToolCategory::ReadOnly,
                allowed_args: vec!["path".to_string()],
                blocked_args: vec![],
                requires_confirmation: false,
                created_at: now,
            },
            AllowlistEntry {
                tool_name: "todo_write".to_string(),
                description: "Write to-do list".to_string(),
                category: ToolCategory::Utility,
                allowed_args: vec!["items".to_string(), "file".to_string()],
                blocked_args: vec![],
                requires_confirmation: false,
                created_at: now,
            },
            AllowlistEntry {
                tool_name: "task_list".to_string(),
                description: "List tasks".to_string(),
                category: ToolCategory::Utility,
                allowed_args: vec![],
                blocked_args: vec![],
                requires_confirmation: false,
                created_at: now,
            },
            AllowlistEntry {
                tool_name: "sleep".to_string(),
                description: "Pause execution".to_string(),
                category: ToolCategory::Utility,
                allowed_args: vec!["seconds".to_string()],
                blocked_args: vec![],
                requires_confirmation: false,
                created_at: now,
            },
            AllowlistEntry {
                tool_name: "tool_search".to_string(),
                description: "Search for tools".to_string(),
                category: ToolCategory::Utility,
                allowed_args: vec!["query".to_string()],
                blocked_args: vec![],
                requires_confirmation: false,
                created_at: now,
            },
            AllowlistEntry {
                tool_name: "ask_user".to_string(),
                description: "Ask user for input".to_string(),
                category: ToolCategory::Utility,
                allowed_args: vec!["question".to_string(), "options".to_string()],
                blocked_args: vec![],
                requires_confirmation: false,
                created_at: now,
            },
        ]
    }

    pub async fn check_tool(&self, tool_name: &str, tool_args: &serde_json::Value) -> CheckResult {
        if !self.config.enabled {
            return CheckResult {
                allowed: true,
                reason: "Allowlist is disabled".to_string(),
                requires_confirmation: false,
            };
        }

        let entries = self.entries.read().await;
        if let Some(entry) = entries.get(tool_name) {
            let args_check = self.check_args(entry, tool_args);
            
            if !args_check.allowed {
                return CheckResult {
                    allowed: false,
                    reason: args_check.reason,
                    requires_confirmation: false,
                };
            }

            return CheckResult {
                allowed: true,
                reason: format!("Tool is allowlisted: {}", entry.description),
                requires_confirmation: entry.requires_confirmation,
            };
        }

        match self.config.default_action {
            DefaultAction::Allow => CheckResult {
                allowed: true,
                reason: "Default action is allow".to_string(),
                requires_confirmation: false,
            },
            DefaultAction::Deny => CheckResult {
                allowed: false,
                reason: format!("Tool '{}' is not in allowlist", tool_name),
                requires_confirmation: false,
            },
            DefaultAction::RequireConfirmation => CheckResult {
                allowed: true,
                reason: format!("Tool '{}' requires confirmation", tool_name),
                requires_confirmation: true,
            },
        }
    }

    fn check_args(&self, entry: &AllowlistEntry, tool_args: &serde_json::Value) -> CheckResult {
        if entry.blocked_args.is_empty() && entry.allowed_args.is_empty() {
            return CheckResult {
                allowed: true,
                reason: "No arg restrictions".to_string(),
                requires_confirmation: false,
            };
        }

        let args_obj = tool_args.as_object();
        if args_obj.is_none() {
            return CheckResult {
                allowed: true,
                reason: "No arguments provided".to_string(),
                requires_confirmation: false,
            };
        }

        let args = args_obj.unwrap();

        for blocked in &entry.blocked_args {
            if args.contains_key(blocked) {
                return CheckResult {
                    allowed: false,
                    reason: format!("Argument '{}' is blocked", blocked),
                    requires_confirmation: false,
                };
            }
        }

        if !entry.allowed_args.is_empty() {
            for (key, _) in args {
                if !entry.allowed_args.contains(key) {
                    return CheckResult {
                        allowed: false,
                        reason: format!("Argument '{}' is not allowed", key),
                        requires_confirmation: false,
                    };
                }
            }
        }

        CheckResult {
            allowed: true,
            reason: "All arguments are allowed".to_string(),
            requires_confirmation: false,
        }
    }

    pub async fn add_entry(&self, entry: AllowlistEntry) -> Result<()> {
        let mut entries = self.entries.write().await;
        
        if entries.len() >= self.config.max_list_size {
            return Err(anyhow!("Allowlist is full"));
        }

        if entries.contains_key(&entry.tool_name) {
            return Err(anyhow!("Tool '{}' is already in allowlist", entry.tool_name));
        }

        entries.insert(entry.tool_name.clone(), entry.clone());
        
        let mut categories = self.categories.write().await;
        categories
            .entry(entry.category)
            .or_insert_with(HashSet::new)
            .insert(entry.tool_name);

        Ok(())
    }

    pub async fn remove_entry(&self, tool_name: &str) -> Result<()> {
        let mut entries = self.entries.write().await;
        let entry = entries.remove(tool_name).ok_or_else(|| anyhow!("Tool not found"))?;

        let mut categories = self.categories.write().await;
        if let Some(tools) = categories.get_mut(&entry.category) {
            tools.remove(tool_name);
        }

        Ok(())
    }

    pub async fn update_entry(&self, tool_name: &str, updated: AllowlistEntry) -> Result<()> {
        let mut entries = self.entries.write().await;
        let existing = entries.get(tool_name).ok_or_else(|| anyhow!("Tool not found"))?;

        let old_category = existing.category.clone();
        let new_category = updated.category.clone();

        let mut categories = self.categories.write().await;
        if old_category != new_category {
            if let Some(tools) = categories.get_mut(&old_category) {
                tools.remove(tool_name);
            }
            categories
                .entry(new_category)
                .or_insert_with(HashSet::new)
                .insert(tool_name.to_string());
        }

        entries.insert(tool_name.to_string(), updated);

        Ok(())
    }

    pub async fn get_entry(&self, tool_name: &str) -> Option<AllowlistEntry> {
        let entries = self.entries.read().await;
        entries.get(tool_name).cloned()
    }

    pub async fn get_all_entries(&self) -> Vec<AllowlistEntry> {
        let entries = self.entries.read().await;
        entries.values().cloned().collect()
    }

    pub async fn get_entries_by_category(&self, category: ToolCategory) -> Vec<AllowlistEntry> {
        let categories = self.categories.read().await;
        let entries = self.entries.read().await;

        if let Some(tools) = categories.get(&category) {
            tools
                .iter()
                .filter_map(|name| entries.get(name))
                .cloned()
                .collect()
        } else {
            Vec::new()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub allowed: bool,
    pub reason: String,
    pub requires_confirmation: bool,
}