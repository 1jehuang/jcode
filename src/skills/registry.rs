use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::skill::{SkillDefinition, SkillResult, SkillCategory};

/// A skill registered in the system with its handler
#[derive(Clone)]
pub struct RegisteredSkill {
    pub definition: SkillDefinition,
    pub handler: Option<Arc<dyn Fn(&str) -> SkillResult + Send + Sync>>,
}

/// Central registry for all skills (built-in + loaded)
pub struct SkillRegistry {
    skills: Arc<RwLock<HashMap<String, RegisteredSkill>>>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        SkillRegistry {
            skills: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register(&self, name: &str, definition: SkillDefinition, handler: Option<Arc<dyn Fn(&str) -> SkillResult + Send + Sync>>) {
        let mut skills = self.skills.write().await;
        skills.insert(name.to_string(), RegisteredSkill { definition, handler });
    }

    pub async fn unregister(&self, name: &str) {
        self.skills.write().await.remove(name);
    }

    pub async fn get(&self, name: &str) -> Option<RegisteredSkill> {
        self.skills.read().await.get(name).cloned()
    }

    pub async fn list(&self) -> Vec<RegisteredSkill> {
        self.skills.read().await.values().cloned().collect()
    }

    pub async fn list_by_category(&self, category: &SkillCategory) -> Vec<RegisteredSkill> {
        self.skills.read().await.values()
            .filter(|s| s.definition.category == *category || category.label() == "all")
            .cloned()
            .collect()
    }

    pub async fn search(&self, query: &str) -> Vec<RegisteredSkill> {
        let q = query.to_lowercase();
        self.skills.read().await.values()
            .filter(|s| {
                s.definition.name.to_lowercase().contains(&q)
                    || s.definition.description.to_lowercase().contains(&q)
                    || s.definition.tags.iter().any(|t| t.to_lowercase().contains(&q))
            })
            .cloned()
            .collect()
    }

    pub async fn count(&self) -> usize {
        self.skills.read().await.len()
    }

    pub async fn execute(&self, name: &str, args: &str) -> SkillResult {
        let skill = self.get(name).await;
        match skill {
            Some(registered) => {
                if let Some(handler) = &registered.handler {
                    handler(args)
                } else {
                    SkillResult::ok(&format!("Skill '{}' registered but has no executable handler. Prompt template available.", name))
                }
            }
            None => SkillResult::err(&format!("Skill '{}' not found", name)),
        }
    }

    pub fn get_sync(&self, name: &str) -> Option<RegisteredSkill> {
        tokio::task::block_in_place(|| {
            let handle = tokio::runtime::Handle::current();
            handle.block_on(self.get(name))
        })
    }

    pub fn list_sync(&self) -> Vec<RegisteredSkill> {
        tokio::task::block_in_place(|| {
            let handle = tokio::runtime::Handle::current();
            handle.block_on(self.list())
        })
    }

    pub fn search_sync(&self, query: &str) -> Vec<RegisteredSkill> {
        tokio::task::block_in_place(|| {
            let handle = tokio::runtime::Handle::current();
            handle.block_on(self.search(query))
        })
    }

    pub fn count_sync(&self) -> usize {
        tokio::task::block_in_place(|| {
            let handle = tokio::runtime::Handle::current();
            handle.block_on(self.count())
        })
    }
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}