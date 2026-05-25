//! Update functionality stub — migrated to build.rs
//!
//! This module is kept for backward compatibility.
//! Actual update logic has been moved to `crate::build`.

pub fn spawn_background_session_update(_session_id: &str) {
    tracing::info!("Session update requested (stub - no-op)");
}
