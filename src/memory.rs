//! Memory system for cross-session learning
//!
//! Provides persistent memory that survives across sessions, organized by:
//! - Project (per working directory)
//! - Global (user-level preferences)

use crate::storage;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// A single memory entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub category: MemoryCategory,
    pub content: String,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub access_count: u32,
    pub source: Option<String>,
}

impl MemoryEntry {
    pub fn new(category: MemoryCategory, content: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: crate::id::new_id("mem"),
            category,
            content: content.into(),
            tags: Vec::new(),
            created_at: now,
            updated_at: now,
            access_count: 0,
            source: None,
        }
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
        self.access_count += 1;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum MemoryCategory {
    Fact,
    Preference,
    Entity,
    Correction,
    Custom(String),
}

impl std::fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryCategory::Fact => write!(f, "fact"),
            MemoryCategory::Preference => write!(f, "preference"),
            MemoryCategory::Entity => write!(f, "entity"),
            MemoryCategory::Correction => write!(f, "correction"),
            MemoryCategory::Custom(s) => write!(f, "{}", s),
        }
    }
}

impl std::str::FromStr for MemoryCategory {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            "fact" => MemoryCategory::Fact,
            "preference" => MemoryCategory::Preference,
            "entity" => MemoryCategory::Entity,
            "correction" => MemoryCategory::Correction,
            other => MemoryCategory::Custom(other.to_string()),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryStore {
    pub entries: Vec<MemoryEntry>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, entry: MemoryEntry) -> String {
        let id = entry.id.clone();
        self.entries.push(entry);
        id
    }

    pub fn by_category(&self, category: &MemoryCategory) -> Vec<&MemoryEntry> {
        self.entries.iter().filter(|e| &e.category == category).collect()
    }

    pub fn search(&self, query: &str) -> Vec<&MemoryEntry> {
        let query_lower = query.to_lowercase();
        self.entries
            .iter()
            .filter(|e| {
                e.content.to_lowercase().contains(&query_lower)
                    || e.tags.iter().any(|t| t.to_lowercase().contains(&query_lower))
            })
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<&MemoryEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn remove(&mut self, id: &str) -> Option<MemoryEntry> {
        if let Some(pos) = self.entries.iter().position(|e| e.id == id) {
            Some(self.entries.remove(pos))
        } else {
            None
        }
    }

    pub fn get_relevant(&self, limit: usize) -> Vec<&MemoryEntry> {
        let mut entries: Vec<&MemoryEntry> = self.entries.iter().collect();
        entries.sort_by(|a, b| {
            let score_a = memory_score(a);
            let score_b = memory_score(b);
            score_b.partial_cmp(&score_a).unwrap_or(std::cmp::Ordering::Equal)
        });
        entries.into_iter().take(limit).collect()
    }

    pub fn format_for_prompt(&self, limit: usize) -> Option<String> {
        let relevant = self.get_relevant(limit);
        if relevant.is_empty() {
            return None;
        }

        let mut sections: HashMap<&MemoryCategory, Vec<&str>> = HashMap::new();
        for entry in &relevant {
            sections.entry(&entry.category).or_default().push(&entry.content);
        }

        let mut output = String::new();
        let order = [
            MemoryCategory::Correction,
            MemoryCategory::Fact,
            MemoryCategory::Preference,
            MemoryCategory::Entity,
        ];

        for cat in &order {
            if let Some(items) = sections.remove(cat) {
                output.push_str(&format!("\n### {}s\n", cat));
                for item in items {
                    output.push_str(&format!("- {}\n", item));
                }
            }
        }

        for (cat, items) in sections {
            output.push_str(&format!("\n### {}\n", cat));
            for item in items {
                output.push_str(&format!("- {}\n", item));
            }
        }

        if output.is_empty() { None } else { Some(output) }
    }
}

fn memory_score(entry: &MemoryEntry) -> f64 {
    let mut score = 0.0;
    let age_hours = (Utc::now() - entry.updated_at).num_hours() as f64;
    score += 100.0 / (1.0 + age_hours / 24.0);
    score += (entry.access_count as f64).sqrt() * 10.0;
    score += match entry.category {
        MemoryCategory::Correction => 50.0,
        MemoryCategory::Preference => 30.0,
        MemoryCategory::Fact => 20.0,
        MemoryCategory::Entity => 10.0,
        MemoryCategory::Custom(_) => 5.0,
    };
    score
}

pub struct MemoryManager {
    project_dir: Option<PathBuf>,
}

impl MemoryManager {
    pub fn new() -> Self {
        Self { project_dir: None }
    }

    fn get_project_dir(&self) -> Option<PathBuf> {
        self.project_dir.clone().or_else(|| std::env::current_dir().ok())
    }

    fn project_memory_path(&self) -> Result<Option<PathBuf>> {
        let project_dir = match self.get_project_dir() {
            Some(d) => d,
            None => return Ok(None),
        };

        let project_hash = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            project_dir.hash(&mut hasher);
            format!("{:016x}", hasher.finish())
        };

        let memory_dir = storage::jcode_dir()?.join("memory").join("projects");
        Ok(Some(memory_dir.join(format!("{}.json", project_hash))))
    }

    fn global_memory_path(&self) -> Result<PathBuf> {
        Ok(storage::jcode_dir()?.join("memory").join("global.json"))
    }

    pub fn load_project(&self) -> Result<MemoryStore> {
        match self.project_memory_path()? {
            Some(path) if path.exists() => storage::read_json(&path),
            _ => Ok(MemoryStore::new()),
        }
    }

    pub fn load_global(&self) -> Result<MemoryStore> {
        let path = self.global_memory_path()?;
        if path.exists() { storage::read_json(&path) } else { Ok(MemoryStore::new()) }
    }

    pub fn save_project(&self, store: &MemoryStore) -> Result<()> {
        if let Some(path) = self.project_memory_path()? {
            storage::write_json(&path, store)?;
        }
        Ok(())
    }

    pub fn save_global(&self, store: &MemoryStore) -> Result<()> {
        let path = self.global_memory_path()?;
        storage::write_json(&path, store)
    }

    pub fn remember_project(&self, entry: MemoryEntry) -> Result<String> {
        let mut store = self.load_project()?;
        let id = store.add(entry);
        self.save_project(&store)?;
        Ok(id)
    }

    pub fn remember_global(&self, entry: MemoryEntry) -> Result<String> {
        let mut store = self.load_global()?;
        let id = store.add(entry);
        self.save_global(&store)?;
        Ok(id)
    }

    pub fn get_prompt_memories(&self, limit: usize) -> Option<String> {
        let mut combined = MemoryStore::new();
        if let Ok(project) = self.load_project() {
            combined.entries.extend(project.entries);
        }
        if let Ok(global) = self.load_global() {
            combined.entries.extend(global.entries);
        }
        combined.format_for_prompt(limit)
    }

    pub fn search(&self, query: &str) -> Result<Vec<MemoryEntry>> {
        let mut results = Vec::new();
        if let Ok(project) = self.load_project() {
            results.extend(project.search(query).into_iter().cloned());
        }
        if let Ok(global) = self.load_global() {
            results.extend(global.search(query).into_iter().cloned());
        }
        Ok(results)
    }

    pub fn list_all(&self) -> Result<Vec<MemoryEntry>> {
        let mut all = Vec::new();
        if let Ok(project) = self.load_project() {
            all.extend(project.entries);
        }
        if let Ok(global) = self.load_global() {
            all.extend(global.entries);
        }
        all.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(all)
    }

    pub fn forget(&self, id: &str) -> Result<bool> {
        let mut project = self.load_project()?;
        if project.remove(id).is_some() {
            self.save_project(&project)?;
            return Ok(true);
        }
        let mut global = self.load_global()?;
        if global.remove(id).is_some() {
            self.save_global(&global)?;
            return Ok(true);
        }
        Ok(false)
    }
}

impl Default for MemoryManager {
    fn default() -> Self {
        Self::new()
    }
}
