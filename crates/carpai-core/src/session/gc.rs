//! Session Garbage Collection - Automatic cleanup of old sessions

use crate::session::core_types::{GcConfig, GcResult};
use chrono::{Duration, Utc};
use std::collections::HashMap;

/// Session entry for GC tracking
#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub id: String,
    pub created_at: chrono::DateTime<Utc>,
    pub last_accessed: chrono::DateTime<Utc>,
    pub size_bytes: u64,
}

/// Session garbage collector
pub struct SessionGc {
    config: GcConfig,
    sessions: HashMap<String, SessionEntry>,
}

impl SessionGc {
    pub fn new(config: GcConfig) -> Self {
        Self {
            config,
            sessions: HashMap::new(),
        }
    }

    /// Register a session for GC tracking
    pub fn register_session(&mut self, entry: SessionEntry) {
        self.sessions.insert(entry.id.clone(), entry);
    }

    /// Run garbage collection
    pub fn run_gc(&mut self) -> GcResult {
        let now = Utc::now();
        let max_age = Duration::days(self.config.max_age_days as i64);
        
        let mut deleted_ids = Vec::new();
        let mut space_freed = 0u64;
        
        // Find sessions to delete
        let to_delete: Vec<String> = self.sessions.iter()
            .filter(|(_, entry)| {
                let age = now.signed_duration_since(entry.last_accessed);
                age > max_age
            })
            .map(|(id, _)| id.clone())
            .collect();
        
        // Don't delete below minimum
        let remaining_after_delete = self.sessions.len().saturating_sub(to_delete.len());
        let can_delete = if remaining_after_delete < self.config.min_keep_sessions {
            let max_deletable = self.sessions.len().saturating_sub(self.config.min_keep_sessions);
            to_delete.into_iter().take(max_deletable).collect::<Vec<_>>()
        } else {
            to_delete
        };
        
        // Execute deletion
        for id in &can_delete {
            if let Some(entry) = self.sessions.remove(id) {
                space_freed += entry.size_bytes;
                deleted_ids.push(id.clone());
            }
        }
        
        let sessions_scanned = self.sessions.len() + deleted_ids.len();
        
        GcResult {
            sessions_scanned,
            sessions_deleted: deleted_ids.len(),
            space_freed_bytes: space_freed,
            deleted_session_ids: can_delete,
        }
    }

    /// Get current session count
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }
}

impl Default for SessionGc {
    fn default() -> Self {
        Self::new(GcConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gc_old_sessions() {
        let mut gc = SessionGc::new(GcConfig {
            max_age_days: 7,
            max_sessions: 100,
            min_keep_sessions: 2,
            dry_run: false,
        });
        
        // Add old session
        let old_entry = SessionEntry {
            id: "old-session".to_string(),
            created_at: Utc::now() - Duration::days(30),
            last_accessed: Utc::now() - Duration::days(30),
            size_bytes: 1000,
        };
        gc.register_session(old_entry);
        
        // Add recent session
        let new_entry = SessionEntry {
            id: "new-session".to_string(),
            created_at: Utc::now(),
            last_accessed: Utc::now(),
            size_bytes: 500,
        };
        gc.register_session(new_entry);
        
        let result = gc.run_gc();
        assert_eq!(result.sessions_deleted, 1);
        assert!(result.deleted_session_ids.contains(&"old-session".to_string()));
        assert_eq!(gc.session_count(), 1);
    }
}
