//! Skill System - Manages agent skills and capabilities
//!
//! TODO: Full implementation pending migration from src/skill_system.rs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
}

pub struct SkillRegistry;

impl SkillRegistry {
    pub fn new() -> Self {
        Self
    }

    pub fn list(&self) -> Vec<Skill> {
        vec![]
    }
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}