use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DenyEntry {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub tool_name: String,
    pub tool_args: serde_json::Value,
    pub reason: String,
    pub rule_id: Option<String>,
    pub user_prompt: Option<String>,
    pub context: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DenySummary {
    pub total_denied: usize,
    pub by_tool: HashMap<String, usize>,
    pub by_reason: HashMap<String, usize>,
    pub recent_denies: Vec<DenyEntry>,
}

#[derive(Debug, Clone)]
pub struct DenyLog {
    entries: Arc<RwLock<VecDeque<DenyEntry>>>,
    max_entries: usize,
    index_by_tool: Arc<RwLock<HashMap<String, Vec<String>>>>,
}

impl DenyLog {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Arc::new(RwLock::new(VecDeque::with_capacity(max_entries))),
            max_entries,
            index_by_tool: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn default() -> Self {
        Self::new(100)
    }

    pub async fn record_deny(
        &self,
        tool_name: &str,
        tool_args: serde_json::Value,
        reason: &str,
        rule_id: Option<&str>,
        user_prompt: Option<&str>,
        context: serde_json::Value,
    ) -> String {
        let entry = DenyEntry {
            id: self.generate_id(),
            timestamp: Utc::now(),
            tool_name: tool_name.to_string(),
            tool_args,
            reason: reason.to_string(),
            rule_id: rule_id.map(|s| s.to_string()),
            user_prompt: user_prompt.map(|s| s.to_string()),
            context,
        };

        let mut entries = self.entries.write().await;
        let mut index = self.index_by_tool.write().await;

        if entries.len() >= self.max_entries {
            if let Some(removed) = entries.pop_front() {
                if let Some(ids) = index.get_mut(&removed.tool_name) {
                    if let Some(pos) = ids.iter().position(|id: &String| id == &removed.id) {
                        ids.remove(pos);
                    }
                }
            }
        }

        entries.push_back(entry.clone());

        index
            .entry(entry.tool_name.clone())
            .or_insert_with(Vec::new)
            .push(entry.id.clone());

        entry.id
    }

    pub async fn get_entries(&self) -> Vec<DenyEntry> {
        self.entries.read().await.clone().into()
    }

    pub async fn get_entries_by_tool(&self, tool_name: &str) -> Vec<DenyEntry> {
        let entries = self.entries.read().await;
        let index = self.index_by_tool.read().await;

        if let Some(ids) = index.get(tool_name) {
            ids.iter()
                .filter_map(|id| entries.iter().find(|e| e.id == *id))
                .cloned()
                .collect()
        } else {
            Vec::new()
        }
    }

    pub async fn get_entry_by_id(&self, id: &str) -> Option<DenyEntry> {
        self.entries.read().await.iter().find(|e| e.id == id).cloned()
    }

    pub async fn get_summary(&self) -> DenySummary {
        let entries = self.entries.read().await;

        let mut by_tool = HashMap::new();
        let mut by_reason = HashMap::new();

        for entry in entries.iter() {
            *by_tool.entry(entry.tool_name.clone()).or_insert(0) += 1;
            *by_reason.entry(entry.reason.clone()).or_insert(0) += 1;
        }

        let recent_denies = entries.iter().rev().take(10).cloned().collect();

        DenySummary {
            total_denied: entries.len(),
            by_tool,
            by_reason,
            recent_denies,
        }
    }

    pub async fn clear(&self) {
        let mut entries = self.entries.write().await;
        let mut index = self.index_by_tool.write().await;

        entries.clear();
        index.clear();
    }

    pub async fn remove_entry(&self, id: &str) -> bool {
        let mut entries = self.entries.write().await;
        let mut index = self.index_by_tool.write().await;

        if let Some((pos, entry)) = entries.iter().enumerate().find(|(_, e)| e.id == id) {
            let tool_name = entry.tool_name.clone();
            entries.remove(pos);

            if let Some(ids) = index.get_mut(&tool_name) {
                if let Some(idx) = ids.iter().position(|i| i == id) {
                    ids.remove(idx);
                }
            }

            true
        } else {
            false
        }
    }

    pub async fn get_recent(&self, limit: usize) -> Vec<DenyEntry> {
        let entries = self.entries.read().await;
        entries.iter().rev().take(limit).cloned().collect()
    }

    fn generate_id(&self) -> String {
        let timestamp = Utc::now().timestamp_millis();
        let random: u32 = rand::random();
        format!("deny_{}_{:x}", timestamp, random)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_record_deny() {
        let log = DenyLog::default();

        let id = log
            .record_deny(
                "test_tool",
                json!({"path": "/etc/passwd"}),
                "Security violation",
                Some("rule_1"),
                Some("test prompt"),
                json!({}),
            )
            .await;

        assert!(!id.is_empty());

        let entries = log.get_entries().await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, id);
        assert_eq!(entries[0].tool_name, "test_tool");
    }

    #[tokio::test]
    async fn test_get_summary() {
        let log = DenyLog::new(10);

        for i in 0..5 {
            log.record_deny(
                if i % 2 == 0 { "tool_a" } else { "tool_b" },
                json!({}),
                if i % 3 == 0 { "Reason A" } else { "Reason B" },
                None,
                None,
                json!({}),
            )
            .await;
        }

        let summary = log.get_summary().await;
        assert_eq!(summary.total_denied, 5);
        assert_eq!(summary.by_tool.get("tool_a"), Some(&3));
        assert_eq!(summary.by_tool.get("tool_b"), Some(&2));
    }

    #[tokio::test]
    async fn test_max_entries() {
        let log = DenyLog::new(3);

        for i in 0..5 {
            log.record_deny(
                "tool",
                json!({ "i": i }),
                "test",
                None,
                None,
                json!({}),
            )
            .await;
        }

        let entries = log.get_entries().await;
        assert_eq!(entries.len(), 3);
    }
}