//! Session garbage collection and compaction
//!
//! Periodically cleans up expired sessions, compacted context windows,
//! and stale resources to prevent memory leaks and maintain system health.

use std::sync::Arc;
use std::time::{Instant, Duration};
use tokio::sync::RwLock;
use tracing::{info, warn, debug};
use chrono::{DateTime, Utc};

/// Configuration for session GC behavior
#[derive(Debug, Clone)]
pub struct SessionGcConfig {
    /// How often to run GC (seconds)
    pub gc_interval_secs: u64,
    /// Session idle timeout - sessions inactive longer than this are candidates for cleanup (seconds)
    pub session_idle_timeout_secs: u64,
    /// Maximum age of a session before forced cleanup (seconds)
    pub session_max_age_secs: u64,
    /// Compact context window if message count exceeds this threshold
    pub context_compact_threshold: usize,
    /// Keep at most this many messages after compaction
    pub context_keep_messages: usize,
    /// Enable automatic compaction
    pub enable_auto_compact: bool,
}

impl Default for SessionGcConfig {
    fn default() -> Self {
        Self {
            gc_interval_secs: 3600,           // Run every hour
            session_idle_timeout_secs: 86400,  // 24 hours
            session_max_age_secs: 604800,      // 7 days
            context_compact_threshold: 100,    // Compact if >100 messages
            context_keep_messages: 50,         // Keep last 50 messages
            enable_auto_compact: true,
        }
    }
}

/// Statistics from a GC run
#[derive(Debug, Clone, Default)]
pub struct GcStats {
    pub sessions_scanned: usize,
    pub sessions_expired: usize,
    pub sessions_idle_removed: usize,
    pub contexts_compacted: usize,
    pub memory_freed_bytes: usize,
    pub duration_ms: u64,
}

/// Session metadata for GC decisions
#[derive(Debug, Clone)]
pub struct SessionMetadata {
    pub session_id: String,
    pub created_at: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
    pub message_count: usize,
    pub context_size_bytes: usize,
    pub is_active: bool,
}

/// Session garbage collector
pub struct SessionGc {
    config: SessionGcConfig,
    stats: Arc<RwLock<GcStats>>,
}

impl SessionGc {
    /// Create a new session GC instance
    pub fn new(config: SessionGcConfig) -> Self {
        Self {
            config,
            stats: Arc::new(RwLock::new(GcStats::default())),
        }
    }

    /// Start the GC loop as a background task
    pub async fn start_background_gc<A: GcAgent + 'static>(&self, agent: Arc<A>) {
        let gc = self.clone_inner();
        let agent_clone = Arc::clone(&agent);

        tokio::spawn(async move {
            info!(
                "Session GC started: interval={}s, idle_timeout={}s, max_age={}s",
                gc.config.gc_interval_secs,
                gc.config.session_idle_timeout_secs,
                gc.config.session_max_age_secs
            );

            loop {
                tokio::time::sleep(Duration::from_secs(gc.config.gc_interval_secs)).await;

                match gc.run_gc_cycle(&*agent_clone).await {
                    Ok(stats) => {
                        if stats.sessions_expired > 0 || stats.contexts_compacted > 0 {
                            info!(
                                "GC cycle complete: scanned={}, expired={}, idle_removed={}, compacted={}, freed={}KB, took={}ms",
                                stats.sessions_scanned,
                                stats.sessions_expired,
                                stats.sessions_idle_removed,
                                stats.contexts_compacted,
                                stats.memory_freed_bytes / 1024,
                                stats.duration_ms
                            );
                        } else {
                            debug!(
                                "GC cycle complete (no action needed): scanned={} sessions, took={}ms",
                                stats.sessions_scanned, stats.duration_ms
                            );
                        }
                    }
                    Err(e) => {
                        warn!("GC cycle failed: {}", e);
                    }
                }
            }
        });
    }

    /// Run a single GC cycle
    pub async fn run_gc_cycle<A: GcAgent>(&self, agent: &A) -> Result<GcStats, String> {
        let start = Instant::now();
        let now = Utc::now();

        let mut stats = GcStats::default();

        // Get all active sessions
        let sessions = agent.list_sessions().await
            .map_err(|e| format!("Failed to list sessions: {}", e))?;

        stats.sessions_scanned = sessions.len();

        for session_meta in sessions {
            let idle_duration = now.signed_duration_since(session_meta.last_activity);
            let session_age = now.signed_duration_since(session_meta.created_at);

            let idle_secs = idle_duration.num_seconds() as u64;
            let age_secs = session_age.num_seconds() as u64;

            // Check if session has exceeded max age
            if age_secs > self.config.session_max_age_secs {
                if let Err(e) = agent.remove_session(&session_meta.session_id, "max_age_exceeded").await {
                    warn!("Failed to remove expired session {}: {}", session_meta.session_id, e);
                } else {
                    stats.sessions_expired += 1;
                    stats.memory_freed_bytes += session_meta.context_size_bytes;
                    debug!("Removed expired session: {} (age={}s)", session_meta.session_id, age_secs);
                }
                continue;
            }

            // Check if session has been idle too long (and is not active)
            if !session_meta.is_active && idle_secs > self.config.session_idle_timeout_secs {
                if let Err(e) = agent.remove_session(&session_meta.session_id, "idle_timeout").await {
                    warn!("Failed to remove idle session {}: {}", session_meta.session_id, e);
                } else {
                    stats.sessions_idle_removed += 1;
                    stats.memory_freed_bytes += session_meta.context_size_bytes;
                    debug!("Removed idle session: {} (idle={}s)", session_meta.session_id, idle_secs);
                }
                continue;
            }

            // Check if context needs compaction
            if self.config.enable_auto_compact
                && session_meta.message_count > self.config.context_compact_threshold
            {
                if let Err(e) = agent.compact_context(
                    &session_meta.session_id,
                    self.config.context_keep_messages,
                ).await {
                    warn!("Failed to compact session {}: {}", session_meta.session_id, e);
                } else {
                    stats.contexts_compacted += 1;
                    debug!("Compacted session context: {} (messages={})", session_meta.session_id, session_meta.message_count);
                }
            }
        }

        stats.duration_ms = start.elapsed().as_millis() as u64;

        // Update stats
        {
            let mut current_stats = self.stats.write().await;
            *current_stats = stats.clone();
        }

        Ok(stats)
    }

    /// Get current GC statistics
    pub async fn get_stats(&self) -> GcStats {
        self.stats.read().await.clone()
    }

    /// Trigger an immediate GC cycle (for testing or manual intervention)
    pub async fn trigger_manual_gc<A: GcAgent>(&self, agent: &A) -> Result<GcStats, String> {
        info!("Manual GC triggered");
        self.run_gc_cycle(agent).await
    }

    fn clone_inner(&self) -> Self {
        Self {
            config: self.config.clone(),
            stats: Arc::clone(&self.stats),
        }
    }
}

/// Trait for types that can perform GC actions on sessions
#[async_trait::async_trait]
pub trait GcAgent: Send + Sync {
    /// List all active sessions with metadata
    async fn list_sessions(&self) -> Result<Vec<SessionMetadata>, Box<dyn std::error::Error + Send + Sync>>;

    /// Remove a session and free its resources
    async fn remove_session(&self, session_id: &str, reason: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// Compact a session's context window
    async fn compact_context(&self, session_id: &str, keep_messages: usize) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockGcAgent {
        sessions: Arc<RwLock<Vec<SessionMetadata>>>,
    }

    #[async_trait::async_trait]
    impl GcAgent for MockGcAgent {
        async fn list_sessions(&self) -> Result<Vec<SessionMetadata>, Box<dyn std::error::Error + Send + Sync>> {
            Ok(self.sessions.read().await.clone())
        }

        async fn remove_session(&self, session_id: &str, _reason: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            let mut sessions = self.sessions.write().await;
            sessions.retain(|s| s.session_id != session_id);
            Ok(())
        }

        async fn compact_context(&self, _session_id: &str, _keep_messages: usize) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_gc_removes_expired_sessions() {
        let config = SessionGcConfig {
            gc_interval_secs: 3600,
            session_max_age_secs: 100, // Short for testing
            ..Default::default()
        };
        let gc = SessionGc::new(config);

        let old_session = SessionMetadata {
            session_id: "old-session".to_string(),
            created_at: Utc::now() - chrono::Duration::seconds(200),
            last_activity: Utc::now(),
            message_count: 10,
            context_size_bytes: 1024,
            is_active: false,
        };

        let agent = Arc::new(MockGcAgent {
            sessions: Arc::new(RwLock::new(vec![old_session])),
        });

        let stats = gc.run_gc_cycle(&*agent).await.unwrap();
        assert_eq!(stats.sessions_expired, 1);
        assert_eq!(stats.sessions_scanned, 1);
    }

    #[tokio::test]
    async fn test_gc_keeps_active_sessions() {
        let config = SessionGcConfig {
            session_idle_timeout_secs: 100,
            ..Default::default()
        };
        let gc = SessionGc::new(config);

        let active_session = SessionMetadata {
            session_id: "active-session".to_string(),
            created_at: Utc::now() - chrono::Duration::seconds(200),
            last_activity: Utc::now(),
            message_count: 10,
            context_size_bytes: 1024,
            is_active: true, // Active sessions should not be removed
        };

        let agent = Arc::new(MockGcAgent {
            sessions: Arc::new(RwLock::new(vec![active_session])),
        });

        let stats = gc.run_gc_cycle(&*agent).await.unwrap();
        assert_eq!(stats.sessions_idle_removed, 0);
    }
}
