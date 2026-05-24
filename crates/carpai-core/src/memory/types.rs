// TODO: This module is scaffolding — types will be aligned with carpai-internal in Phase 1C
// NOTE: This file is NOT declared in mod.rs and is currently orphaned.
//! Memory Types - Additional type definitions for memory system

#[allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Memory category classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryCategory {
    Fact,       // Factual information
    Preference, // User preferences
    Context,    // Contextual information
    Instruction,// Instructions or procedures
    Observation,// Observations from environment
}

/// Memory tag for flexible categorization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryTag {
    pub name: String,
    pub confidence: f64, // 0.0 to 1.0
}

/// Memory source tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySource {
    pub source_type: SourceType,
    pub source_id: Option<String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub reliability: f64, // 0.0 to 1.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceType {
    UserInput,
    AgentInference,
    ExternalApi,
    FileRead,
    CodeAnalysis,
}

/// Memory versioning support
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryVersion {
    pub version: u32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub changes: Vec<String>,
}

/// Memory access pattern tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessPattern {
    pub access_count: u64,
    pub last_accessed: Option<chrono::DateTime<chrono::Utc>>,
    pub frequent_queries: Vec<String>,
}

/// Memory metadata builder
pub struct MemoryMetadataBuilder {
    data: HashMap<String, String>,
}

impl MemoryMetadataBuilder {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }

    pub fn with(mut self, key: &str, value: &str) -> Self {
        self.data.insert(key.to_string(), value.to_string());
        self
    }

    pub fn build(self) -> HashMap<String, String> {
        self.data
    }
}

impl Default for MemoryMetadataBuilder {
    fn default() -> Self {
        Self::new()
    }
}
