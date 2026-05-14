//! Core type definitions: config, workspace, ambient, auth, gateway
//!
//! Merged from: jcode-config-types, jcode-ambient-types, jcode-auth-types, jcode-gateway-types

pub mod config;
pub mod ambient;
pub mod auth;
pub mod gateway;

// Re-export all types at crate root for backward compatibility
pub use config::*;
pub use ambient::*;
pub use auth::*;
pub use gateway::*;
