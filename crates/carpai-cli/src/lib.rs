//! # carpai-cli
//!
//! **TUI Client** of the CarpAI monorepo.
//!
//! ## Architecture
//!
//! ```
//! ┌─────────────────────────────────────────────┐
//! │              carpai-cli                     │  ← THIS CRATE: TUI + CLI commands
//! ├─────────────────────────────────────────────┤
//! │              carpai-core                    │  ← Business logic (execute_agent_turn)
//! ├─────────────────────────────────────────────┤
//! │            carpai-internal                  │  ← Trait definitions + DI container
//! └─────────────────────────────────────────────┘
//! ```
//!
//! ## Key Design Principle
//!
//! **TUI is a pure rendering layer.** All agent business logic is delegated to
//! `carpai-core::execute_agent_turn()` via `agent_bridge.rs`.

pub mod config;
pub mod cli;
pub mod tui;
pub mod agent_bridge;
pub mod ambient;
pub mod modes;
pub mod notifications;
pub mod retry;
pub mod config_watch;
pub mod grpc_client;

// Re-exports for convenience
pub use config::CliConfig;
pub use agent_bridge::{AgentBridge, BridgeMode, AgentTurnOutput};
pub use ambient::runner::BackgroundRunner;
pub use ambient::scheduler::TaskScheduler;
pub use notifications::{BrowserOpener, GmailNotifier, TelegramNotifier};
pub use retry::{RetryConfig, retry_default};
