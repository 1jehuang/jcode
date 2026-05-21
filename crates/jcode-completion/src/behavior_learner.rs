//! User Behavior Learning System for Personalized Completions
//!
//! This module learns from user editing patterns to provide personalized
//! completion suggestions. It tracks:
//! 1. Which completions users accept/reject
//! 2. Common code patterns and templates used
//! 3. Time-of-day coding habits
//! 4. Project-specific conventions
//!
//! Architecture:
//! ```text
//! User Action -> EventCollector -> PatternAnalyzer -> PreferenceModel
//!                                              |
//!                                              v
//!                                       Personalized Ranking
//! ```

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::fs;
use tracing::{debug, info};

use chrono::Timelike;

/// Maximum number of events to keep in memory
const MAX_EVENT_HISTORY: usize = 1000;

/// Decay factor for old preferences (0.95 = slow decay)
const DECAY_FACTOR: f64 = 0.95;

/// Represents a user interaction with a completion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionEvent {
    pub timestamp: u64, // Unix timestamp in milliseconds
    pub file_path: String,
    pub context: CompletionContextSnapshot,
    pub offered_completions: Vec<String>,
    pub accepted_index: Option<usize>, // None if rejected all
    pub time_to_decision_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionContextSnapshot {
    pub prefix: String,
    pub suffix: String,
    pub line_content: String,
    pub scope: Option<String>,
    pub expected_type: Option<String>,
}

/// Learned user preferences
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPreferences {
    /// Preferred naming conventions: e.g., "snake_case" vs "camelCase"
    pub naming_convention: HashMap<String, f64>, // pattern -> weight
    /// Preferred code structures: e.g., "for loop" vs "iterator"
    pub structure_preferences: HashMap<String, f64>,
    /// Frequently used libraries/modules
    pub library_usage: HashMap<String, u32>,
    /// Time-based patterns: hour_of_day -> activity_level
    pub temporal_patterns: [f64; 24],
    /// File type preferences: ".rs" -> preference_score
    pub file_type_preferences: HashMap<String, f64>,
}

impl Default for UserPreferences {
    fn default() -> Self {
        Self {
            naming_convention: HashMap::new(),
            structure_preferences: HashMap::new(),
            library_usage: HashMap::new(),
            temporal_patterns: [0.5; 24], // Uniform distribution initially
            file_type_preferences: HashMap::new(),
        }
    }
}

/// Behavior learner that adapts to user patterns
pub struct BehaviorLearner {
    /// Recent completion events
    events: Arc<RwLock<VecDeque<CompletionEvent>>>,
    /// Learned preferences
    preferences: Arc<RwLock<UserPreferences>>,
    /// Session start time
    session_start: Instant,
    /// Persistence path
    storage_path: Option<PathBuf>,
}

impl BehaviorLearner {
    pub fn new(storage_path: Option<PathBuf>) -> Self {
        let learner = Self {
            events: Arc::new(RwLock::new(VecDeque::with_capacity(MAX_EVENT_HISTORY))),
            preferences: Arc::new(RwLock::new(UserPreferences::default())),
            session_start: Instant::now(),
            storage_path,
        };

        // Load existing preferences if available
        if let Some(path) = &learner.storage_path {
            let prefs_path = path.join("user_preferences.json");
            let learner_clone = learner.clone();
            tokio::spawn(async move {
                if let Ok(prefs) = learner_clone.load_preferences(&prefs_path).await {
                    *learner_clone.preferences.write() = prefs;
                    info!("Loaded user preferences from {:?}", prefs_path);
                }
            });
        }

        learner
    }

    /// Record a completion interaction
    pub async fn record_completion_event(&self, event: CompletionEvent) {
        // Add to event history
        {
            let mut events = self.events.write();
            events.push_back(event.clone());
            if events.len() > MAX_EVENT_HISTORY {
                events.pop_front();
            }
        }

        // Update preferences asynchronously
        self.update_preferences_from_event(&event).await;

        // Periodically save preferences
        if self.events.read().len() % 50 == 0 {
            self.save_preferences_async().await;
        }
    }

    /// Get personalization score for a completion candidate
    pub fn get_personalization_score(&self, candidate_text: &str, file_path: &str) -> f64 {
        let prefs = self.preferences.read();
        let mut score = 0.0;

        // Check naming convention match
        if self.matches_naming_convention(candidate_text, &prefs.naming_convention) {
            score += 0.2;
        }

        // Check file type preference
        if let Some(ext) = std::path::Path::new(file_path).extension() {
            if let Some(ext_str) = ext.to_str() {
                if let Some(pref) = prefs.file_type_preferences.get(ext_str) {
                    score += pref * 0.1;
                }
            }
        }

        // Check temporal relevance (current hour activity)
        let current_hour = chrono::Local::now().hour() as usize;
        score += prefs.temporal_patterns[current_hour] * 0.05;

        score.min(1.0)
    }

    /// Get learned code templates for a given context
    pub fn get_common_templates(&self, context_prefix: &str) -> Vec<String> {
        let prefs = self.preferences.read();
        let mut templates = Vec::new();

        // Look for common patterns in structure preferences
        for (pattern, weight) in &prefs.structure_preferences {
            if *weight > 0.7 && pattern.starts_with(context_prefix) {
                templates.push(pattern.clone());
            }
        }

        templates.sort_by(|a, b| {
            let weight_a = prefs.structure_preferences.get(a).unwrap_or(&0.0);
            let weight_b = prefs.structure_preferences.get(b).unwrap_or(&0.0);
            weight_b.partial_cmp(weight_a).unwrap_or(std::cmp::Ordering::Equal)
        });

        templates.truncate(5);
        templates
    }

    /// Get statistics about learning progress
    pub fn get_learning_stats(&self) -> LearningStatistics {
        let events = self.events.read();
        let prefs = self.preferences.read();

        let total_events = events.len();
        let acceptance_count = events.iter().filter(|e| e.accepted_index.is_some()).count();
        let acceptance_rate = if total_events > 0 {
            acceptance_count as f64 / total_events as f64
        } else {
            0.0
        };

        LearningStatistics {
            total_events,
            acceptance_rate,
            session_duration_secs: self.session_start.elapsed().as_secs(),
            unique_patterns_learned: prefs.structure_preferences.len(),
            top_libraries: self.get_top_libraries(5),
        }
    }

    /// Update preferences based on a single event
    async fn update_preferences_from_event(&self, event: &CompletionEvent) {
        let mut prefs = self.preferences.write();

        // Update naming convention preferences
        if let Some(accepted_idx) = event.accepted_index {
            if let Some(accepted_text) = event.offered_completions.get(accepted_idx) {
                self.extract_and_update_naming_pattern(&mut prefs, accepted_text);
                self.extract_and_update_structure_pattern(&mut prefs, &event.context, accepted_text);
            }
        }

        // Update temporal patterns
        let hour = chrono::DateTime::from_timestamp_millis(event.timestamp as i64)
            .map(|dt| dt.hour() as usize)
            .unwrap_or(0);
        if hour < 24 {
            prefs.temporal_patterns[hour] = (prefs.temporal_patterns[hour] + 0.1).min(1.0);
        }

        // Update file type preferences
        if let Some(ext) = PathBuf::from(&event.file_path).extension() {
            if let Some(ext_str) = ext.to_str() {
                let pref = prefs.file_type_preferences.entry(ext_str.to_string()).or_insert(0.5);
                if event.accepted_index.is_some() {
                    *pref = (*pref + 0.05).min(1.0);
                } else {
                    *pref = (*pref - 0.02).max(0.0);
                }
            }
        }

        // Apply decay to all preferences to forget old patterns
        self.apply_decay(&mut prefs);
    }

    /// Extract naming convention from accepted text
    fn extract_and_update_naming_pattern(&self, prefs: &mut UserPreferences, text: &str) {
        // Detect snake_case
        if text.contains('_') && text.chars().all(|c| c.is_lowercase() || c == '_' || c.is_digit(10)) {
            let weight = prefs.naming_convention.entry("snake_case".to_string()).or_insert(0.5);
            *weight = (*weight + 0.1).min(1.0);
        }

        // Detect camelCase
        if text.chars().any(|c| c.is_uppercase()) && !text.starts_with(char::is_uppercase) {
            let weight = prefs.naming_convention.entry("camelCase".to_string()).or_insert(0.5);
            *weight = (*weight + 0.1).min(1.0);
        }

        // Detect PascalCase
        if text.starts_with(char::is_uppercase) {
            let weight = prefs.naming_convention.entry("PascalCase".to_string()).or_insert(0.5);
            *weight = (*weight + 0.1).min(1.0);
        }
    }

    /// Extract code structure patterns
    fn extract_and_update_structure_pattern(
        &self,
        prefs: &mut UserPreferences,
        context: &CompletionContextSnapshot,
        accepted_text: &str,
    ) {
        // Simple pattern extraction (in real implementation, use AST parsing)
        if accepted_text.contains("for ") && accepted_text.contains(" in ") {
            let pattern = format!("{} for-in loop", context.prefix);
            let weight = prefs.structure_preferences.entry(pattern).or_insert(0.5);
            *weight = (*weight + 0.1).min(1.0);
        }

        if accepted_text.contains(".map(") || accepted_text.contains(".filter(") {
            let pattern = format!("{} iterator chain", context.prefix);
            let weight = prefs.structure_preferences.entry(pattern).or_insert(0.5);
            *weight = (*weight + 0.1).min(1.0);
        }
    }

    /// Apply exponential decay to preferences
    fn apply_decay(&self, prefs: &mut UserPreferences) {
        for value in prefs.naming_convention.values_mut() {
            *value *= DECAY_FACTOR;
        }
        for value in prefs.structure_preferences.values_mut() {
            *value *= DECAY_FACTOR;
        }
        for value in prefs.temporal_patterns.iter_mut() {
            *value *= DECAY_FACTOR;
        }
        for value in prefs.file_type_preferences.values_mut() {
            *value *= DECAY_FACTOR;
        }
    }

    /// Get top used libraries
    fn get_top_libraries(&self, limit: usize) -> Vec<(String, u32)> {
        let prefs = self.preferences.read();
        let mut libs: Vec<_> = prefs.library_usage.iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();

        libs.sort_by(|a, b| b.1.cmp(&a.1));
        libs.truncate(limit);
        libs
    }

    /// Save preferences to disk
    async fn save_preferences_async(&self) {
        if let Some(base_path) = &self.storage_path {
            let prefs_path = base_path.join("user_preferences.json");
            let prefs = self.preferences.read().clone();

            if let Ok(json) = serde_json::to_string_pretty(&prefs) {
                if let Err(e) = fs::write(&prefs_path, json).await {
                    debug!("Failed to save preferences: {}", e);
                }
            }
        }
    }

    /// Load preferences from disk
    async fn load_preferences(&self, path: &PathBuf) -> Result<UserPreferences, Box<dyn std::error::Error>> {
        let content = fs::read_to_string(path).await?;
        let prefs: UserPreferences = serde_json::from_str(&content)?;
        Ok(prefs)
    }

    /// Check if text matches learned naming conventions
    fn matches_naming_convention(&self, text: &str, conventions: &HashMap<String, f64>) -> bool {
        for (pattern, weight) in conventions {
            if *weight < 0.6 {
                continue; // Ignore weak patterns
            }

            match pattern.as_str() {
                "snake_case" => {
                    if text.contains('_') && text.chars().all(|c| c.is_lowercase() || c == '_' || c.is_digit(10)) {
                        return true;
                    }
                }
                "camelCase" => {
                    if text.chars().any(|c| c.is_uppercase()) && !text.starts_with(char::is_uppercase) {
                        return true;
                    }
                }
                "PascalCase" => {
                    if text.starts_with(char::is_uppercase) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }
}

impl Clone for BehaviorLearner {
    fn clone(&self) -> Self {
        Self {
            events: self.events.clone(),
            preferences: self.preferences.clone(),
            session_start: self.session_start,
            storage_path: self.storage_path.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LearningStatistics {
    pub total_events: usize,
    pub acceptance_rate: f64,
    pub session_duration_secs: u64,
    pub unique_patterns_learned: usize,
    pub top_libraries: Vec<(String, u32)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_behavior_learner_records_events() {
        let learner = BehaviorLearner::new(None);

        let event = CompletionEvent {
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
            file_path: "src/main.rs".to_string(),
            context: CompletionContextSnapshot {
                prefix: "let x = ".to_string(),
                suffix: ";".to_string(),
                line_content: "let x = hello".to_string(),
                scope: Some("function".to_string()),
                expected_type: Some("String".to_string()),
            },
            offered_completions: vec!["hello_world".to_string(), "hello_name".to_string()],
            accepted_index: Some(0),
            time_to_decision_ms: 500,
        };

        learner.record_completion_event(event).await;

        let stats = learner.get_learning_stats();
        assert_eq!(stats.total_events, 1);
        assert_eq!(stats.acceptance_rate, 1.0);
    }

    #[tokio::test]
    async fn test_personalization_scoring() {
        let learner = BehaviorLearner::new(None);

        // Should return non-negative score even with no data
        let score = learner.get_personalization_score("test_function", "src/main.rs");
        assert!(score >= 0.0);
        assert!(score <= 1.0);
    }

    #[tokio::test]
    async fn test_learning_stats_initial_state() {
        let learner = BehaviorLearner::new(None);
        let stats = learner.get_learning_stats();

        assert_eq!(stats.total_events, 0);
        assert_eq!(stats.acceptance_rate, 0.0);
    }
}
