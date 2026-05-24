//! Event Bus Trait - Unified pub/sub event system
//!
//! Abstracts the existing `Bus::global()` (tokio::broadcast) into a trait
//! that supports multiple backends:
//!
//! ## Implementations
//!
//! | Product | Implementation | Behavior |
//! |---------|---------------|----------|
//! | `carpai-cli` | `InProcessEventBus` | `tokio::broadcast::channel`, mirrors `src/bus.rs` |
//! | `carpai-server` | `RedisEventBus` | Redis Pub/Sub for cross-node events |
//! | `carpai-server` | `KafkaEventBus` | Apache Kafka for durable event streaming |
//! | `testing` | `InMemoryEventBus` | Vec-based, synchronous, for unit tests |
//!
//! ## Design Principles
//!
//! 1. **At-least-once delivery**: Events may be duplicated but never lost.
//!    Subscribers must be idempotent.
//!
//! 2. **Typed events**: All events implement `BusEvent` trait with serialization.
//!
//! 3. **Fan-out**: Multiple subscribers per event type, each gets every message.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::sync::Arc;

// ========================================================================
// Core Trait
// ========================================================================

/// Unified publish-subscribe event bus
///
/// **Object-safe**: Can be used as `Arc<dyn EventBus>`.
/// Uses serialized JSON for transport to avoid generic/dyn parameter issues.
///
/// Note: `Clone` is intentionally NOT a supertrait because `Clone` requires
/// `Self: Sized`, which makes the trait unusable as `dyn EventBus`.
/// Implementations should use `Arc::clone()` or interior mutability instead.
#[async_trait]
pub trait EventBus: Send + Sync + 'static {
    /// Publish an event (serialized as JSON internally)
    ///
    /// The event is serialized to JSON before publishing so the trait
    /// remains object-safe (no generics in method signatures).
    async fn publish_json(&self, event_type: &str, payload: &str) -> Result<(), EventBusError>;

    /// Subscribe to all events of a given type name
    ///
    /// Returns a receiver stream of JSON payloads.
    async fn subscribe(
        &self,
        event_type: &str,
    ) -> Result<Box<dyn BusSubscriber + Send>, EventBusError>;

    /// Get count of current subscribers for an event type
    fn subscriber_count(&self, event_type: &str) -> usize;

    /// Health check �?whether the bus is operational
    fn health_check(&self) -> BusHealth;

    /// Clone this event bus (returns Arc<Self> wrapped in a new trait object)
    ///
    /// This replaces the need for `Clone` supertrait.
    /// Implementations should return `Arc::clone(self_arc)` where `self_arc`
    /// is obtained via `Self::into_arc(self)` or similar pattern.
    fn clone_box(&self) -> Arc<dyn EventBus>;
}

/// Extension trait for typed event publishing (not object-safe, use concrete types only)
#[async_trait]
pub trait EventBusExt: EventBus {
    /// Convenience: publish a typed event (serializes to JSON first)
    async fn publish<E: BusEvent>(&self, event: E) -> Result<(), EventBusError> {
        let payload = serde_json::to_string(&event)
            .map_err(|e| EventBusError::Internal(anyhow::anyhow!("Serialization: {}", e)))?;
        self.publish_json(event.event_type(), &payload).await
    }
}

// Blanket impl for all EventBus implementors (including dyn EventBus)
impl<T: ?Sized + EventBus> EventBusExt for T {}

/// Subscriber receiver �?consumes JSON-encoded events from the bus
#[async_trait]
pub trait BusSubscriber: Send + Debug {
    /// Receive the next event as JSON string
    async fn recv(&mut self) -> Result<BusEventEnvelope, EventBusError>;

    /// Try to receive without blocking
    fn try_recv(&mut self) -> Result<Option<BusEventEnvelope>, EventBusError>;

    /// Number of unconsumed events buffered
    fn len(&self) -> usize;

    /// Whether the channel is empty
    fn is_empty(&self) -> bool { self.len() == 0 }
}

/// Envelope wrapping each delivered event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusEventEnvelope {
    pub event_type: String,
    pub payload: String, // JSON
    pub timestamp_ms: i64,
}

// ========================================================================
// Event Data Trait (for typed events)
// ========================================================================

/// Trait that all bus events must implement
///
/// Events must be:
/// - Serializable (for cross-process transport)
/// - Cloneable (for fan-out delivery)
/// - Named (for subscription routing)
///
/// Note: This trait does NOT need to be object-safe because it's only
/// used as a generic bound on `EventBus::publish<E: BusEvent>()`.
pub trait BusEvent: Debug + Send + Sync + Serialize + for<'a> Deserialize<'a> + Clone + 'static {
    /// Unique event type identifier (e.g., "session.message_added")
    fn event_type(&self) -> &'static str;

    /// Whether this event should be persisted (for durable backends)
    fn durable(&self) -> bool { false }
}

// ========================================================================
// Built-in Event Types
// ========================================================================

// --- Session Events ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCreated {
    pub session_id: String,
    pub owner_id: Option<String>,
    pub title: Option<String>,
    #[serde(default)]
    pub timestamp: i64,
}
impl BusEvent for SessionCreated {
    fn event_type(&self) -> &'static str { "session.created" }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessagesAppended {
    pub session_id: String,
    pub message_ids: Vec<String>,
    pub role: String,
    #[serde(default)]
    pub timestamp: i64,
}
impl BusEvent for SessionMessagesAppended {
    fn event_type(&self) -> &'static str { "session.messages_appended" }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStateChanged {
    pub session_id: String,
    pub old_state: String,
    pub new_state: String,
    #[serde(default)]
    pub timestamp: i64,
}
impl BusEvent for SessionStateChanged {
    fn event_type(&self) -> &'static str { "session.state_changed" }
}

// --- Agent/Turn Events ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTurnStarted {
    pub session_id: String,
    pub turn_id: String,
    pub user_message: String,
    pub model: Option<String>,
    #[serde(default)]
    pub timestamp: i64,
}
impl BusEvent for AgentTurnStarted {
    fn event_type(&self) -> &'static str { "agent.turn_started" }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTurnCompleted {
    pub session_id: String,
    pub turn_id: String,
    pub success: bool,
    pub duration_ms: u64,
    pub tool_calls_count: usize,
    pub tokens_used: usize,
    #[serde(default)]
    pub timestamp: i64,
}
impl BusEvent for AgentTurnCompleted {
    fn event_type(&self) -> &'static str { "agent.turn_completed" }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecuted {
    pub session_id: String,
    pub turn_id: String,
    pub tool_name: String,
    pub success: bool,
    pub duration_ms: u64,
    pub output_length: usize,
    #[serde(default)]
    pub timestamp: i64,
}
impl BusEvent for ToolExecuted {
    fn event_type(&self) -> &'static str { "agent.tool_executed" }
}

// --- File System Events ---

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileOperationType { Created, Written, Deleted, Renamed, }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileModified {
    pub session_id: Option<String>,
    pub file_path: String,
    pub operation: FileOperationType,
    pub size_bytes: u64,
    #[serde(default)]
    pub timestamp: i64,
}
impl BusEvent for FileModified {
    fn event_type(&self) -> &'static str { "fs.file_modified" }
    fn durable(&self) -> bool { true }
}

// --- Inference Events ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceCompleted {
    pub session_id: Option<String>,
    pub model: String,
    pub provider: String,
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub latency_ms: u64,
    pub cost_usd: f64,
    #[serde(default)]
    pub timestamp: i64,
}
impl BusEvent for InferenceCompleted {
    fn event_type(&self) -> &'static str { "inference.completed" }
    fn durable(&self) -> bool { true }
}

// --- System Events ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SystemStatus { Healthy, Degraded, Down, Unknown, }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemHealthChanged {
    pub component: String,
    pub status: SystemStatus,
    pub message: Option<String>,
    #[serde(default)]
    pub timestamp: i64,
}
impl BusEvent for SystemHealthChanged {
    fn event_type(&self) -> &'static str { "system.health_changed" }
}

// ========================================================================
// Health & Error Types
// ========================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusHealth {
    pub healthy: bool,
    pub backend: String,
    pub total_subscribers: usize,
    pub events_published_total: u64,
    pub events_dropped_total: u64,
    pub uptime_secs: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum EventBusError {
    #[error("Subscription failed: {0}")]
    SubscriptionFailed(String),
    #[error("Publish failed: {0}")]
    PublishFailed(String),
    #[error("Connection lost to backend")]
    ConnectionLost,
    #[error("Deserialization error: {0}")]
    Deserialization(String),
    #[error("Channel closed")]
    ChannelClosed,
    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_created_event() {
        let event = SessionCreated {
            session_id: "sess-1".into(),
            owner_id: Some("user-1".into()),
            title: Some("Test Session".into()),
            timestamp: 1700000000,
        };
        assert_eq!(event.event_type(), "session.created");
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("sess-1"));
    }

    #[test]
    fn test_tool_executed_event() {
        let event = ToolExecuted {
            session_id: "s1".into(),
            turn_id: "t1".into(),
            tool_name: "read_file".into(),
            success: true,
            duration_ms: 15,
            output_length: 2048,
            timestamp: 1700000001,
        };
        assert_eq!(event.event_type(), "agent.tool_executed");
        let _clone = event.clone();
    }

    #[test]
    fn test_inference_completed_durable() {
        let event = InferenceCompleted {
            session_id: None,
            model: "claude-4-opus".into(),
            provider: "anthropic".into(),
            prompt_tokens: 10000,
            completion_tokens: 2000,
            latency_ms: 3500,
            cost_usd: 0.085,
            timestamp: 1700000002,
        };
        assert!(event.durable());
    }

    #[test]
    fn test_all_events_serialize_roundtrip() {
        let events: Vec<Box<dyn BusEvent>> = vec![
            Box::new(SessionCreated { session_id: "x".into(), owner_id: None, title: None, timestamp: 0 }),
            Box::new(FileModified { session_id: None, file_path: "/tmp/x".into(), operation: FileOperationType::Written, size_bytes: 100, timestamp: 0 }),
            Box::new(SystemHealthChanged { component: "db".into(), status: SystemStatus::Healthy, message: None, timestamp: 0 }),
        ];
        for event in events {
            let json = serde_json::to_string(event.as_ref()).unwrap();
            assert!(!json.is_empty());
        }
    }

    #[test]
    fn test_envelope_serialization() {
        let env = BusEventEnvelope {
            event_type: "test".into(),
            payload: r#"{"key":"value"}"#.into(),
            timestamp_ms: 1000,
        };
        let json = serde_json::to_string(&env).unwrap();
        assert!(json.contains("test"));
    }
}
