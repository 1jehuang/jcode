// TODO: This module is scaffolding — types will be aligned with carpai-internal in Phase 1C
//! Memory Prompt - Memory-aware prompt construction

#[allow(dead_code)]

use crate::memory::core_types::{EnhancedMemoryEntry, MemoryScope};

/// Inject memory context into prompts
pub struct MemoryPromptBuilder {
    max_memories: usize,
    include_scope: Vec<MemoryScope>,
}

impl MemoryPromptBuilder {
    pub fn new() -> Self {
        Self {
            max_memories: 5,
            include_scope: vec![MemoryScope::Global, MemoryScope::User, MemoryScope::Session],
        }
    }

    pub fn with_max_memories(mut self, max: usize) -> Self {
        self.max_memories = max;
        self
    }

    pub fn with_scopes(mut self, scopes: Vec<MemoryScope>) -> Self {
        self.include_scope = scopes;
        self
    }

    /// Build prompt with memory context
    pub fn build_with_memories(&self, base_prompt: &str, memories: &[EnhancedMemoryEntry]) -> String {
        let mut prompt = base_prompt.to_string();
        
        if !memories.is_empty() {
            prompt.push_str("\n\n## Relevant Context from Memory\n\n");
            
            let relevant: Vec<&EnhancedMemoryEntry> = memories.iter()
                .filter(|m| self.include_scope.contains(&m.scope))
                .take(self.max_memories)
                .collect();
            
            for (i, memory) in relevant.iter().enumerate() {
                prompt.push_str(&format!(
                    "{}. [{}] {}\n",
                    i + 1,
                    format!("{:?}", memory.trust_level),
                    memory.content
                ));
            }
        }
        
        prompt
    }
}

impl Default for MemoryPromptBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::collections::HashMap;

    #[test]
    fn test_build_prompt_with_memories() {
        let builder = MemoryPromptBuilder::new();
        
        let memories = vec![
            EnhancedMemoryEntry {
                id: "mem1".to_string(),
                content: "Important fact 1".to_string(),
                embedding: None,
                metadata: HashMap::new(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                scope: MemoryScope::Global,
                trust_level: crate::memory::core_types::TrustLevel::High,
                access_count: 0,
                last_accessed: None,
            },
        ];
        
        let prompt = builder.build_with_memories("Base question", &memories);
        assert!(prompt.contains("Base question"));
        assert!(prompt.contains("Important fact 1"));
    }
}
