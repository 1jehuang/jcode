//! Collaboration-Aware Completion
//!
//! This module enables completion suggestions that are aware of other team members'
//! editing activities in a Swarm session. It provides:
//! - Shared hot symbol cache across swarm members
//! - Conflict detection (suggesting alternatives when multiple people edit same area)
//! - Team pattern learning (what do teammates commonly use?)

use crate::incremental_index::{SymbolEntry, IncrementalIndex};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

/// Represents another swarm member's current editing context
#[derive(Debug, Clone)]
pub struct MemberEditingContext {
    pub member_id: String,
    pub current_file: String,
    pub cursor_line: usize,
    pub recent_symbols: Vec<String>,
    pub last_active: Instant,
}

/// Collaboration-aware completion engine
pub struct CollabAwareCompleter {
    /// Active swarm members and their editing contexts
    active_members: Arc<RwLock<HashMap<String, MemberEditingContext>>>,
    /// Shared symbol cache for the swarm
    shared_cache: Arc<RwLock<HashMap<String, Vec<String>>>>,
    /// Team usage patterns (learned from all members)
    team_patterns: Arc<RwLock<HashMap<String, u32>>>,
    /// Reference to incremental index for symbol lookup
    index: Arc<IncrementalIndex>,
}

impl CollabAwareCompleter {
    pub fn new(index: Arc<IncrementalIndex>) -> Self {
        Self {
            active_members: Arc::new(RwLock::new(HashMap::new())),
            shared_cache: Arc::new(RwLock::new(HashMap::new())),
            team_patterns: Arc::new(RwLock::new(HashMap::new())),
            index,
        }
    }

    /// Register a swarm member's editing activity
    pub fn update_member_context(&self, member_id: String, context: MemberEditingContext) {
        self.active_members.write().insert(member_id, context);
    }

    /// Remove a member when they leave the swarm
    pub fn remove_member(&self, member_id: &str) {
        self.active_members.write().remove(member_id);
    }

    /// Get symbols currently being edited by other members (to avoid conflicts)
    pub fn get_conflicting_symbols(&self, current_file: &str) -> HashSet<String> {
        let members = self.active_members.read();
        let mut conflicting = HashSet::new();

        for member in members.values() {
            if member.current_file == current_file {
                for symbol in &member.recent_symbols {
                    conflicting.insert(symbol.clone());
                }
            }
        }

        conflicting
    }

    /// Get suggested symbols based on team patterns
    pub fn get_team_suggested_symbols(&self, prefix: &str, limit: usize) -> Vec<(String, u32)> {
        let patterns = self.team_patterns.read();
        let mut suggestions: Vec<_> = patterns.iter()
            .filter(|(name, _)| name.starts_with(prefix))
            .map(|(name, count)| (name.clone(), *count))
            .collect();

        suggestions.sort_by(|a, b| b.1.cmp(&a.1));
        suggestions.truncate(limit);
        suggestions
    }

    /// Record a symbol usage to update team patterns
    pub fn record_symbol_usage(&self, symbol: &str) {
        let mut patterns = self.team_patterns.write();
        *patterns.entry(symbol.to_string()).or_insert(0) += 1;
    }

    /// Add symbols to shared cache
    pub fn add_to_shared_cache(&self, file: &str, symbols: Vec<String>) {
        let mut cache = self.shared_cache.write();
        cache.insert(file.to_string(), symbols);
    }

    /// Get symbols from shared cache
    pub fn get_from_shared_cache(&self, file: &str) -> Option<Vec<String>> {
        self.shared_cache.read().get(file).cloned()
    }

    /// Get active swarm members count
    pub fn get_active_members_count(&self) -> usize {
        self.active_members.read().len()
    }

    /// Check if anyone else is editing the same file
    pub fn is_file_being_edited_by_others(&self, file: &str, exclude_member: &str) -> bool {
        let members = self.active_members.read();
        members.values().any(|m| m.current_file == file && m.member_id != exclude_member)
    }

    /// Get collaboration statistics
    pub fn get_collab_stats(&self) -> CollabStats {
        let members = self.active_members.read();
        let patterns = self.team_patterns.read();
        let cache = self.shared_cache.read();

        CollabStats {
            active_members: members.len(),
            tracked_symbols: patterns.len(),
            cached_files: cache.len(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CollabStats {
    pub active_members: usize,
    pub tracked_symbols: usize,
    pub cached_files: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_collab_aware_completer() {
        let index = Arc::new(IncrementalIndex::new());
        let completer = CollabAwareCompleter::new(index);

        // Simulate two members editing
        let ctx1 = MemberEditingContext {
            member_id: "user1".to_string(),
            current_file: "src/main.rs".to_string(),
            cursor_line: 10,
            recent_symbols: vec!["println".to_string()],
            last_active: Instant::now(),
        };
        completer.update_member_context("user1".to_string(), ctx1);

        let ctx2 = MemberEditingContext {
            member_id: "user2".to_string(),
            current_file: "src/lib.rs".to_string(),
            cursor_line: 5,
            recent_symbols: vec!["format".to_string()],
            last_active: Instant::now(),
        };
        completer.update_member_context("user2".to_string(), ctx2);

        // Check conflict detection
        let conflicts = completer.get_conflicting_symbols("src/main.rs");
        assert!(conflicts.contains("println"));

        // Check active members
        assert_eq!(completer.get_active_members_count(), 2);
    }
}
