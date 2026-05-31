//! `jcode-base`: foundational layer of the jcode application core.
//!
//! This crate holds the downward-closed set of modules that the upper
//! server/tool/agent layer (`jcode-app-core`) depends on: provider, auth,
//! config, session, message, memory, telemetry, and their supporting leaves.
//! Splitting it out lets the two halves compile as separate rustc units so the
//! largest compilation unit (and its peak memory) is roughly halved.
//!
//! `jcode-app-core` re-exports this crate via `pub use jcode_base::*`, so every
//! existing `crate::<module>` path in the upper layers keeps resolving.

#![allow(
    unknown_lints,
    clippy::collapsible_match,
    clippy::manual_checked_ops,
    clippy::unnecessary_sort_by,
    clippy::useless_conversion
)]

pub mod auth;
pub mod background;
pub mod browser;
pub mod build;
pub mod bus;
pub mod cache_tracker;
pub mod client_input;
pub mod compaction;
pub mod config;
pub mod copilot_usage;
pub mod dictation;
#[cfg(feature = "embeddings")]
pub mod embedding;
#[cfg(not(feature = "embeddings"))]
pub mod embedding_stub;
pub mod env;
pub mod gateway;
pub mod generated_image;
pub mod gmail;
pub mod goal;
pub mod id;
pub mod import;
pub mod live_tests;
pub mod logging;
pub mod login_qr;
pub mod mcp;
pub mod memory;
pub mod memory_agent;
pub mod memory_graph;
pub mod memory_log;
pub mod memory_types;
pub mod message;
pub mod plan;
pub mod platform;
pub mod process_memory;
pub mod process_title;
pub mod prompt;
pub mod protocol;
pub mod provider;
pub mod provider_catalog;
pub mod registry;
pub mod runtime_memory_log;
pub mod safety;
pub mod secret_input;
pub mod session;
pub mod session_list_cache;
pub mod side_panel;
pub mod sidecar;
pub mod skill;
pub mod soft_interrupt_store;
pub mod stdin_detect;
pub mod storage;
pub mod subscription_catalog;
pub mod telegram;
pub mod telemetry;
pub mod terminal_launch;
pub mod todo;
pub mod transport;
pub mod usage;
pub mod util;
#[cfg(not(feature = "embeddings"))]
pub use embedding_stub as embedding;
