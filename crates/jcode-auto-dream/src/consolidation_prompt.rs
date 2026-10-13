//! Consolidation prompt -- builds the LLM prompt that performs memory merge,
//! deduplication, and strengthening.
//!
//! This module is responsible for assembling the context window with relevant
//! memory entries, cluster metadata, and the instruction set that guides the
//! consolidation pass.

use serde::{Deserialize, Serialize};

/// Role of a memory entry in the consolidation context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryRole {
    /// A core memory that should be preserved.
    Core,
    /// A working memory that may be promoted or pruned.
    Working,
    /// An episodic memory that provides context.
    Episodic,
}

/// A memory entry included in the consolidation prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptEntry {
    /// Unique memory ID.
    pub id: String,
    /// The memory content text.
    pub content: String,
    /// The role/category of this entry.
    pub role: EntryRole,
    /// Confidence score (0.0 -- 1.0).
    pub confidence: f32,
    /// Number of times this memory has been reinforced.
    pub strength: u32,
    /// Tags associated with this memory.
    pub tags: Vec<String>,
}

/// The assembled prompt for a consolidation pass.
#[derive(Debug, Clone)]
pub struct ConsolidationPrompt {
    /// System instruction for the LLM.
    pub system: String,
    /// User message containing the consolidation task.
    pub user_message: String,
    /// Memory entries to consolidate.
    pub entries: Vec<PromptEntry>,
}

impl ConsolidationPrompt {
    /// Build a consolidation prompt from a set of memory entries.
    pub fn build(entries: Vec<PromptEntry>, cluster_info: Option<&str>) -> Self {
        let system = CONSOLIDATION_SYSTEM_PROMPT.to_string();

        let entry_count = entries.len();
        let mut user_message = format!(
            "Consolidate {entry_count} memory entries. \
             Merge duplicates, promote high-confidence working memories, \
             and decay low-strength entries."
        );

        if let Some(info) = cluster_info {
            user_message.push_str(&format!("\n\nCluster context:\n{info}"));
        }

        Self {
            system,
            user_message,
            entries,
        }
    }

    /// Serialize the prompt entries to JSON for the API call.
    pub fn entries_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self.entries)
    }
}

/// The system prompt for the consolidation LLM call.
const CONSOLIDATION_SYSTEM_PROMPT: &str = "\
You are a memory consolidation assistant. Your task is to review a set of
memory entries and perform the following operations:

1. **Merge duplicates**: Combine entries that express the same fact or
   preference. Keep the richer version.
2. **Promote memories**: Working memories with high confidence (>0.8) and
   sufficient strength (>=3) should be promoted to core.
3. **Decay weak entries**: Memories with strength < 2 and confidence < 0.3
   that haven't been accessed recently should be marked for archival.
4. **Update tags**: Ensure tags are consistent and non-redundant.

Output a JSON array of operations: [{\"op\": \"merge\"|\"promote\"|\"decay\"|\"tag\",
\"ids\": [...], \"reason\": \"...\"}]
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_prompt_with_entries() {
        let entries = vec![
            PromptEntry {
                id: "mem_1".into(),
                content: "User prefers dark mode".into(),
                role: EntryRole::Core,
                confidence: 0.95,
                strength: 5,
                tags: vec!["preference".into()],
            },
            PromptEntry {
                id: "mem_2".into(),
                content: "User likes dark theme".into(),
                role: EntryRole::Working,
                confidence: 0.7,
                strength: 2,
                tags: vec!["preference".into(), "ui".into()],
            },
        ];

        let prompt = ConsolidationPrompt::build(entries, None);
        assert_eq!(prompt.entries.len(), 2);
        assert!(prompt.system.contains("consolidation"));
        assert!(prompt.user_message.contains("2 memory entries"));
    }

    #[test]
    fn build_prompt_with_cluster_info() {
        let entries = vec![];
        let prompt = ConsolidationPrompt::build(entries, Some("Cluster: preferences"));
        assert!(prompt.user_message.contains("Cluster: preferences"));
    }
}
