//! CarpAI E2E Test Suite Entry Point
//!
//! This test binary contains all end-to-end tests for the CarpAI product lines:
//!
//! # Test Chains (from THREE_TEAM_REFACTOR_PLAN_V3_FINAL.md §7.1)
//!
//! 1. **CLI Local Mode** (`cli_local_*` tests)
//!    - TUI → type message → receive reply
//!    - Session persistence validation
//!
//! 2. **Server Standalone** (`server_standalone_*` tests)
//!    - Health check endpoint
//!    - gRPC connectivity
//!    - REST API calls
//!    - Protocol consistency
//!
//! 3. **CLI Remote Mode** (`cli_remote_*` tests)
//!    - CLI → gRPC → Server → reply
//!    - Connection resilience
//!    - Large payload handling
//!
//! 4. **SDK Basic Flow** (`sdk_*` tests)
//!    - Client initialization
//!    - Chat completion API
//!    - Session CRUD operations
//!    - Error handling
//!
//! # Running Tests
//!
//! ```bash
//! # Build binaries first (required for E2E tests)
//! cargo build --release --bins
//!
//! # Run all E2E tests (requires --ignored flag)
//! cargo test --test carpai_e2e -- --include-ignored --nocapture
//!
//! # Run specific chain
//! cargo test --test carpai_e2e cli_local -- --include-ignored
//! cargo test --test carpai_e2e server -- --include-ignored
//! cargo test --test carpai_e2e sdk -- --include-ignored
//!
//! # Run with verbose output
//! cargo test --test carpai_e2e -- --include-ignored --nocapture 2>&1 | tee e2e-results.log
//! ```
//!
//! # Configuration
//!
//! All tests use temporary directories and mock providers.
//! No external services or API keys are required.
//!
//! # Timeout Policy
//!
//! Each test has a 60-second maximum execution time.
//! Individual operations have shorter timeouts (5-30 seconds).

mod carpai_e2e;

fn main() {
    println!("CarpAI E2E Test Suite");
    println!("=====================");
    println!();
    println!("This test suite validates the four main product chains:");
    println!("1. CLI Local Mode (TUI interaction)");
    println!("2. Server Standalone (gRPC + REST)");
    println!("3. CLI Remote Mode (CLI→Server proxy)");
    println!("4. SDK Basic Flow (client library)");
    println!();
    println!("Run with: cargo test --test carpai_e2e -- --include-ignored");
}
