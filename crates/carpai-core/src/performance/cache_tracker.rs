//! Client-side cache tracking for append-only validation
//!
//! When providers don't report cache tokens, we can still detect cache violations
//! by tracking the message prefix ourselves. If the prefix changes between requests,
//! we know the cache was invalidated.
//!
//! This is a fallback mechanism for providers like Fireworks (via OpenRouter) that
//! have automatic caching but don't report cache hit/miss metrics.

use std::collections::VecDeque;
use std::hash::{Hash, Hasher};

/// Simplified message role for cache tracking purposes
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

/// Simplified content block for cache tracking
#[derive(Debug, Clone)]
pub enum ContentBlock {
    Text {
        text: String,
        #[allow(dead_code)]
        cache_control: Option<String>,
    },
}

/// Simplified message type for cache tracking (local to avoid crate::message dependency)
#[derive(Debug, Clone)]
pub struct TrackedMessage {
    pub role: Role,
    pub content: Vec<ContentBlock>,
    #[allow(dead_code)]
    pub timestamp: Option<u64>,
    #[allow(dead_code)]
    pub tool_duration_ms: Option<u64>,
}

/// Compute a stable hash for a single message
pub fn stable_message_hash(msg: &TrackedMessage) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    msg.role.hash(&mut hasher);
    for block in &msg.content {
        if let ContentBlock::Text { text, .. } = block {
            text.hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// Extend a hash by combining with another hash value (rolling hash for prefix tracking)
pub fn extend_stable_hash(prev_hash: u64, new_hash: u64) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    prev_hash.hash(&mut hasher);
    new_hash.hash(&mut hasher);
    hasher.finish()
}

/// Maximum number of prefix hashes to remember (for detecting intermittent violations)
const MAX_HISTORY: usize = 10;

/// Tracks message prefixes to detect cache violations
#[derive(Debug, Clone, Default)]
pub struct CacheTracker {
    /// Hash of the previous message prefix
    previous_prefix_hash: Option<u64>,
    /// Number of messages in the previous request
    previous_message_count: usize,
    /// Turn counter (number of complete request/response cycles)
    turn_count: u32,
    /// History of prefix hashes for debugging
    hash_history: VecDeque<u64>,
    /// Whether append-only was violated on the last request
    last_violation: Option<CacheViolation>,
}

/// Information about a cache violation
#[derive(Debug, Clone)]
pub struct CacheViolation {
    /// Turn number when violation occurred
    pub turn: u32,
    /// Number of messages at time of violation
    pub message_count: usize,
    /// Expected prefix hash
    pub _expected_hash: String,
    /// Actual prefix hash
    pub _actual_hash: String,
    /// Human-readable reason
    pub reason: String,
}

impl CacheTracker {
    pub fn new() -> Self {
        Self::default()
    }

    fn hash_label(hash: u64) -> String {
        format!("{hash:016x}")
    }

    fn prefix_hashes_for_messages(messages: &[TrackedMessage]) -> Vec<u64> {
        let mut prefix_hashes = Vec::with_capacity(messages.len());
        for message in messages {
            let message_hash = stable_message_hash(message);
            let prefix_hash = prefix_hashes
                .last()
                .copied()
                .map(|prev| extend_stable_hash(prev, message_hash))
                .unwrap_or(message_hash);
            prefix_hashes.push(prefix_hash);
        }
        prefix_hashes
    }

    /// Record a request and check for cache violations
    ///
    /// Call this BEFORE sending each request to the provider.
    /// Returns Some(violation) if the append-only property was violated.
    pub fn record_request(&mut self, messages: &[TrackedMessage]) -> Option<CacheViolation> {
        let prefix_hashes = Self::prefix_hashes_for_messages(messages);
        self.record_prefix_hashes(&prefix_hashes)
    }

    pub fn record_prefix_hashes(&mut self, prefix_hashes: &[u64]) -> Option<CacheViolation> {
        let current_count = prefix_hashes.len();
        let current_full_hash = prefix_hashes.last().copied();
        let previous_count = self.previous_message_count;
        let prefix_hash_at_previous_count = if previous_count == 0 || previous_count > current_count
        {
            None
        } else {
            Some(prefix_hashes[previous_count - 1])
        };
        self.record_prefix_hash_snapshot(
            current_count,
            prefix_hash_at_previous_count,
            current_full_hash,
        )
    }

    pub fn record_prefix_hash_snapshot(
        &mut self,
        current_count: usize,
        prefix_hash_at_previous_count: Option<u64>,
        current_full_hash: Option<u64>,
    ) -> Option<CacheViolation> {
        self.turn_count += 1;

        // First turn - just record the baseline
        if self.turn_count == 1 || self.previous_prefix_hash.is_none() {
            let hash = current_full_hash.unwrap_or(0);
            self.previous_prefix_hash = Some(hash);
            self.previous_message_count = current_count;
            self.hash_history.push_back(hash);
            if self.hash_history.len() > MAX_HISTORY {
                self.hash_history.pop_front();
            }
            self.last_violation = None;
            return None;
        }

        let previous_hash = self.previous_prefix_hash.as_ref()?;
        let previous_count = self.previous_message_count;

        // For append-only caching, the current messages should START with
        // all the previous messages (same prefix)
        if current_count < previous_count {
            // Messages were removed - definite violation
            let current_hash = current_full_hash.unwrap_or(0);
            let violation = CacheViolation {
                turn: self.turn_count,
                message_count: current_count,
                _expected_hash: Self::hash_label(*previous_hash),
                _actual_hash: Self::hash_label(current_hash),
                reason: format!(
                    "Messages removed: had {} messages, now have {}",
                    previous_count, current_count
                ),
            };

            // Update state
            self.previous_prefix_hash = Some(current_hash);
            self.previous_message_count = current_count;
            self.hash_history.push_back(current_hash);
            if self.hash_history.len() > MAX_HISTORY {
                self.hash_history.pop_front();
            }
            self.last_violation = Some(violation.clone());
            return Some(violation);
        }

        // Check if the prefix (first N messages) matches
        let prefix_hash = prefix_hash_at_previous_count.unwrap_or(0);

        if prefix_hash != *previous_hash {
            // Prefix changed - violation
            let violation = CacheViolation {
                turn: self.turn_count,
                message_count: current_count,
                _expected_hash: Self::hash_label(*previous_hash),
                _actual_hash: Self::hash_label(prefix_hash),
                reason: format!(
                    "Prefix modified: first {} messages changed (hash {} -> {})",
                    previous_count,
                    Self::hash_label(*previous_hash),
                    Self::hash_label(prefix_hash)
                ),
            };

            // Update state
            let current_hash = current_full_hash.unwrap_or(0);
            self.previous_prefix_hash = Some(current_hash);
            self.previous_message_count = current_count;
            self.hash_history.push_back(current_hash);
            if self.hash_history.len() > MAX_HISTORY {
                self.hash_history.pop_front();
            }
            self.last_violation = Some(violation.clone());
            return Some(violation);
        }

        // No violation - update state with new full message list
        let full_hash = current_full_hash.unwrap_or(0);
        self.previous_prefix_hash = Some(full_hash);
        self.previous_message_count = current_count;
        self.hash_history.push_back(full_hash);
        if self.hash_history.len() > MAX_HISTORY {
            self.hash_history.pop_front();
        }
        self.last_violation = None;
        None
    }

    /// Get the current turn count
    pub fn turn_count(&self) -> u32 {
        self.turn_count
    }

    pub fn previous_message_count(&self) -> usize {
        self.previous_message_count
    }

    /// Reset the tracker (e.g., when switching models or compacting)
    pub fn reset(&mut self) {
        self.previous_prefix_hash = None;
        self.previous_message_count = 0;
        self.turn_count = 0;
        self.hash_history.clear();
        self.last_violation = None;
    }

    /// Check if we detected a violation on the last request
    pub fn had_violation(&self) -> bool {
        self.last_violation.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_message(role: Role, text: &str) -> TrackedMessage {
        TrackedMessage {
            role,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        }
    }

    #[test]
    fn test_append_only_no_violation() {
        let mut tracker = CacheTracker::new();

        // First request
        let msgs1 = vec![make_message(Role::User, "Hello")];
        assert!(tracker.record_request(&msgs1).is_none());

        // Second request - append assistant response and new user message
        let msgs2 = vec![
            make_message(Role::User, "Hello"),
            make_message(Role::Assistant, "Hi there!"),
            make_message(Role::User, "How are you?"),
        ];
        assert!(tracker.record_request(&msgs2).is_none());

        // Third request - append more
        let msgs3 = vec![
            make_message(Role::User, "Hello"),
            make_message(Role::Assistant, "Hi there!"),
            make_message(Role::User, "How are you?"),
            make_message(Role::Assistant, "I'm doing well!"),
            make_message(Role::User, "Great!"),
        ];
        assert!(tracker.record_request(&msgs3).is_none());
    }

    #[test]
    fn test_prefix_modification_violation() {
        let mut tracker = CacheTracker::new();

        // First request
        let msgs1 = vec![make_message(Role::User, "Hello")];
        assert!(tracker.record_request(&msgs1).is_none());

        // Second request - modify the first message (violation!)
        let msgs2 = vec![
            make_message(Role::User, "Hello MODIFIED"),
            make_message(Role::Assistant, "Hi there!"),
        ];
        let violation = tracker.record_request(&msgs2);
        assert!(violation.is_some());
        assert!(violation.unwrap().reason.contains("Prefix modified"));
    }

    #[test]
    fn test_message_removal_violation() {
        let mut tracker = CacheTracker::new();

        // First request with multiple messages
        let msgs1 = vec![
            make_message(Role::User, "Hello"),
            make_message(Role::Assistant, "Hi there!"),
            make_message(Role::User, "How are you?"),
        ];
        assert!(tracker.record_request(&msgs1).is_none());

        // Second request - remove messages (violation!)
        let msgs2 = vec![make_message(Role::User, "Hello")];
        let violation = tracker.record_request(&msgs2);
        assert!(violation.is_some());
        assert!(violation.unwrap().reason.contains("Messages removed"));
    }

    #[test]
    fn test_reset() {
        let mut tracker = CacheTracker::new();

        let msgs1 = vec![make_message(Role::User, "Hello")];
        tracker.record_request(&msgs1);

        // Reset and start fresh - no violation
        tracker.reset();

        let msgs2 = vec![make_message(Role::User, "Different message")];
        assert!(tracker.record_request(&msgs2).is_none());
    }

    /// Verify normal multi-turn conversation growth never triggers a false positive.
    #[test]
    fn test_no_false_positive_on_normal_growth() {
        let mut tracker = CacheTracker::new();

        let turn1 = vec![make_message(Role::User, "Q1")];
        assert!(
            tracker.record_request(&turn1).is_none(),
            "Turn 1: no violation"
        );

        let turn2 = vec![
            make_message(Role::User, "Q1"),
            make_message(Role::Assistant, "A1"),
            make_message(Role::User, "Q2"),
        ];
        assert!(
            tracker.record_request(&turn2).is_none(),
            "Turn 2: no violation"
        );

        let turn3 = vec![
            make_message(Role::User, "Q1"),
            make_message(Role::Assistant, "A1"),
            make_message(Role::User, "Q2"),
            make_message(Role::Assistant, "A2"),
            make_message(Role::User, "Q3"),
        ];
        assert!(
            tracker.record_request(&turn3).is_none(),
            "Turn 3: no violation"
        );

        let turn4 = vec![
            make_message(Role::User, "Q1"),
            make_message(Role::Assistant, "A1"),
            make_message(Role::User, "Q2"),
            make_message(Role::Assistant, "A2"),
            make_message(Role::User, "Q3"),
            make_message(Role::Assistant, "A3"),
            make_message(Role::User, "Q4"),
        ];
        assert!(
            tracker.record_request(&turn4).is_none(),
            "Turn 4: no violation"
        );
    }

    /// Verify memory injection does NOT cause false positives
    #[test]
    fn test_no_false_positive_when_memory_excluded() {
        let mut tracker = CacheTracker::new();

        let base1 = vec![make_message(Role::User, "Q1")];
        assert!(tracker.record_request(&base1).is_none());

        let base2 = vec![
            make_message(Role::User, "Q1"),
            make_message(Role::Assistant, "A1"),
            make_message(Role::User, "Q2"),
        ];
        assert!(tracker.record_request(&base2).is_none());

        let base3 = vec![
            make_message(Role::User, "Q1"),
            make_message(Role::Assistant, "A1"),
            make_message(Role::User, "Q2"),
            make_message(Role::Assistant, "A2"),
            make_message(Role::User, "Q3"),
        ];
        assert!(
            tracker.record_request(&base3).is_none(),
            "Should NOT flag a violation — memory suffix from turn 2 is NOT tracked here"
        );
    }
}
