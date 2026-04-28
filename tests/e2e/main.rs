//! End-to-end tests for jcode using a mock provider
//!
//! These tests verify the full flow from user input to response
//! without making actual API calls.

mod mock_provider;
mod test_support;

mod ambient;
mod binary_integration;
mod burst_spawn;
mod provider_behavior;
mod safety;
mod session_flow;
mod transport;
#[cfg(windows)]
mod windows_lifecycle;
