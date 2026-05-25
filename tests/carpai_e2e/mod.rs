//! E2E Test Framework for CarpAI Product Lines
//!
//! This module provides end-to-end tests for the four main product lines:
//! - CLI Local Mode (TUI → type → receive reply)
//! - Server Standalone (health check → gRPC call → REST call)
//! - CLI Remote Mode (CLI → gRPC → Server → reply)
//! - SDK Basic Flow (client.connect → chat → receive)
//!
//! # Running Tests
//!
//! ```bash
//! # Run all E2E tests (requires --ignored flag)
//! cargo test --test carpai_e2e -- --include-ignored
//!
//! # Run specific test chain
//! cargo test --test carpai_e2e cli_local -- --include-ignored
//! cargo test --test carpai_e2e server_standalone -- --include-ignored
//! ```
//!
//! # Prerequisites
//!
//! - Built binaries: `carpai`, `carpai-server`
//! - No external service dependencies (uses mock providers)
//! - Temporary directories for test isolation

pub mod helpers;
pub mod fixtures;

mod cli_local_test;
mod server_standalone_test;
mod cli_remote_test;
mod sdk_basic_test;

pub use helpers::*;
pub use fixtures::*;
