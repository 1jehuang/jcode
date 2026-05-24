//! # Mock Implementations
//!
//! Mock implementations of all core traits for testing and development.
//! Activated via the `mock` feature gate.
//!
//! ## Usage by Other Teams
//!
//! - **ma-guoyang**: Use `MockInferenceBackend` to test gRPC handlers without a real LLM
//! - **Paw-brave**: Use `MockSessionStore` to test TUI rendering without real session persistence

pub mod session_store;
pub mod tool_executor;
pub mod inference;
pub mod filesystem;
pub mod event_bus;
pub mod memory;

use std::sync::Arc;
use carpai_internal::*;

/// Build a complete mock AgentContext for testing
pub fn build_mock_agent_context() -> AgentContext {
    AgentContextBuilder::new(AppConfig::default())
        .with_sessions(Arc::new(session_store::MockSessionStore::default()))
        .with_tools(Arc::new(tool_executor::MockToolExecutor::default()))
        .with_inference(Arc::new(inference::MockInferenceBackend))
        .with_fs(Arc::new(filesystem::MockFileSystem::new()))
        .with_events(Arc::new(event_bus::MockEventBus::default()))
        .with_memory(Arc::new(memory::MockMemoryBackend::default()))
        .build()
        .expect("Mock AgentContext assembly")
}
