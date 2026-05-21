#![allow(
    unknown_lints,
    clippy::collapsible_match,
    clippy::manual_checked_ops,
    clippy::unnecessary_sort_by,
    clippy::useless_conversion
)]

pub mod agent;
pub mod ambient;
pub mod ambient_runner;
pub mod ambient_scheduler;
pub mod ast;
pub mod auth;
pub mod background;
pub mod browser;
pub mod build;
pub mod bus;
pub mod cache_tracker;
pub mod catchup;
pub mod channel;
pub mod ci;
pub mod cli;
pub mod compaction;
pub mod config;
pub mod copilot_usage;
pub mod crdt;
pub mod dictation;
#[cfg(feature = "embeddings")]
pub mod embedding;
#[cfg(not(feature = "embeddings"))]
pub mod embedding_stub;
#[cfg(not(feature = "embeddings"))]
pub use embedding_stub as embedding;
pub mod env;
pub mod gateway;
pub mod gmail;
pub mod goal;
pub mod id;
pub mod import;
pub mod incremental_index;
pub mod proactive_context;
pub mod audit_log;
pub mod logging;
pub mod login_qr;
pub mod mcp;
pub mod memory;
pub mod memory_agent;
pub mod memory_graph;
pub mod memory_log;
pub mod memory_types;
pub mod message;
pub mod network_retry;
pub mod notifications;
pub mod overnight;
pub mod perf;
pub mod plan;
pub mod platform;
pub mod process_memory;
pub mod process_title;
pub mod prompt;
pub mod protocol;
pub mod provider;
pub mod provider_catalog;
pub mod registry;
pub mod replay;
pub mod restart_snapshot;
pub mod runtime_memory_log;
pub mod safety;
pub mod server;
pub mod session;
pub mod setup_hints;
pub mod side_panel;
pub mod sidecar;
pub mod skill;
pub mod skills;
pub mod soft_interrupt_store;
pub mod startup_profile;
pub mod stdin_detect;
pub mod storage;
pub mod subscription_catalog;
pub mod telegram;
pub mod telemetry;
pub mod terminal_launch;
pub mod todo;
pub mod tool;
pub mod transport;
pub mod tui;
pub mod undo_manager;
pub mod update;
pub mod usage;
pub mod util;
pub mod video_export;
pub mod grpc;
pub mod scheduler;
pub mod external;
pub mod ws;
pub mod rest;
pub mod auto_mode;
pub mod security;
pub mod dap;
pub mod debugger;
pub mod metrics;
pub mod compression;
pub mod classifier;
pub mod circuit_breaker;
pub mod deny_log;
pub mod task_scheduler;
pub mod rule_reviewer;
pub mod token_budget;
pub mod denial_tracking;
pub mod session_cost_tracker;
pub mod cache_break_detector;
pub mod allowlist;
pub mod workspace_manager;
pub mod build_module;
pub mod sandbox;
pub mod slash_command;
pub mod browser_bridge;
pub mod ide_integration;
pub mod checkpoint;
pub mod refactor_engine;
pub mod sub_agents;
pub mod hooks_system;
pub mod lsp_client;
pub mod diff_engine;
pub mod file_refs;
pub mod bridge;
pub mod error_types;
pub mod completion;
pub mod auto_test_loop;
pub mod git_workflow;
pub mod task_decomposer;
pub mod semantic_memory;
pub mod precise_edit;
pub mod permission_rules;
pub mod context_pruner;
pub mod atomic_edit_coordinator;
pub mod skill_system;
pub mod plan_verifier;
pub mod streaming_diff_preview;
pub mod workflow;
pub mod codereview;
pub mod ai_enhanced;
pub mod git;
pub mod task_planner;
pub mod team_sync;
pub mod plugins;
pub mod ssh;
pub mod task_manager;
pub mod task_cli;
pub mod plan_mode;
pub mod session_export;
pub mod version_manager;
pub mod undo_redo;
pub mod api;
pub mod utils;

// Mid-term enhancements
pub mod dashboard;
pub mod marketplace;
pub mod debug_panel;
pub mod plugin_market;
pub mod i18n;

// Long-term vision
pub mod distributed;
pub mod ai_optimization;
pub mod ab_testing;

// Reasoning & Context (Claude Code 级别)
pub mod context;
pub mod reasoning;

// NLP & Prototype (Claude Code 深度借鉴)
pub mod nlp;
pub mod prototype;

// Orphaned source files — 已存在的独立模块
pub mod completion_engine;
pub mod protocol_memory;
pub mod memory_prompt;
pub mod message_notifications;

// Knowledge Base (Rust最佳实践)
pub mod knowledge;

// Enhanced Refactoring System (跨语言迁移 & 现代化)
pub mod refactor;

use anyhow::Result;
use std::sync::Mutex;

static CURRENT_SESSION_ID: Mutex<Option<String>> = Mutex::new(None);

pub fn set_current_session(session_id: &str) {
    if let Ok(mut guard) = CURRENT_SESSION_ID.lock() {
        *guard = Some(session_id.to_string());
    }
}

pub fn get_current_session() -> Option<String> {
    CURRENT_SESSION_ID.lock().ok()?.clone()
}

pub async fn run() -> Result<()> {
    cli::startup::run().await
}
