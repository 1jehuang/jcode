#![allow(unknown_lints)]

// ════════════════════════════════════════════════════════════════
//  CarpAI — Enterprise-grade AI Programming Server
//
//  Feature Gates:
//    - "server" : Headless server (gRPC + REST + WS) [default: ON]
//    - "cli"    : Terminal TUI client (local + remote) [default: ON]
//    - "enterprise": Multi-tenant, distributed, RBAC [opt-in]
//
//  All modules below are conditionally compiled based on features.
//  This allows building a pure server binary without TUI/terminal deps.
// ════════════════════════════════════════════════════════════════

// ===== Core Foundation (always included) =====
pub mod core;
pub mod id;
pub mod update;
pub mod utils;
pub use utils as util;
pub mod video_export;

// ===== Agent System (always included — core abstraction) =====
pub mod agent;
pub mod agent_runtime;
pub mod claude_agent_port;
pub mod sub_agents;
pub mod skill_system;
pub mod plan_mode;
pub mod task_planner;
pub mod task_manager;
pub mod task_cli;
pub mod task_decomposer;
pub mod task_scheduler;
pub mod plan_verifier;
pub mod ultraplan;

// ===== API Layer (server feature) =====
#[cfg(feature = "server")]
pub mod api;
#[cfg(feature = "server")]
pub mod grpc;
#[cfg(feature = "server")]
pub mod rest;
#[cfg(feature = "server")]
pub mod ws;
#[cfg(any(feature = "server", feature = "cli"))]
pub mod transport;
#[cfg(feature = "server")]
pub mod protocol;
#[cfg(feature = "server")]
pub mod bridge;

// ===== Authentication & Authorization =====
#[cfg(feature = "server")]
pub mod auth;
#[cfg(feature = "server")]
pub mod security;
#[cfg(feature = "server")]
pub mod security_scanner;
#[cfg(feature = "server")]
pub mod permission_rules;

// ===== Code Completion =====
pub mod completion;
pub mod completion_engine;
pub mod completion_quality;
pub mod auto_fallback;

// ===== Memory System =====
pub mod memory;
pub mod memory_agent;
pub mod memory_graph;
pub mod memory_log;
pub mod memory_types;
pub mod memory_prompt;
pub mod memory_advanced;
pub mod semantic_memory;
pub mod hierarchical_memory;
pub mod knowledge_graph;
pub mod knowledge;
pub mod knowledge_agents;

// ===== Tools & MCP =====
pub mod tool;
pub mod mcp;
pub mod slash_command;

// ===== Enterprise Features (enterprise feature gate) =====
#[cfg(feature = "enterprise")]
pub mod enterprise;

// ===== Observability (server feature) =====
#[cfg(feature = "server")]
pub mod observability;
#[cfg(feature = "server")]
pub mod telemetry;
#[cfg(feature = "server")]
pub mod metrics;
#[cfg(feature = "server")]
pub mod prometheus;
#[cfg(feature = "server")]
pub mod logging;
#[cfg(feature = "server")]
pub mod audit_log;
#[cfg(feature = "server")]
pub mod deny_log;

// ===== Configuration =====
pub mod config;

// ===== Session Management =====
pub mod session;
pub mod session_export;
pub mod session_cost_tracker;
pub mod session_gc;
pub mod runtime_manager;
pub mod cgroup_isolation;

// ===== File Operations =====
pub mod storage;
pub mod file_refs;
pub mod file_state_cache;
pub mod file_history;
pub mod checkpoint;
pub mod undo_redo;
pub mod undo_manager;

// ===== Git Integration =====
pub mod git;
pub mod git_workflow;
pub mod version_manager;

// ===== Refactoring Engine =====
pub mod refactor;
pub mod refactor_engine;
pub mod orchestrator;
pub mod precise_edit;
pub mod atomic_edit_coordinator;
pub mod diff_engine;
pub mod diff_integration;
pub mod streaming_diff_preview;
pub mod compilation_engine;
pub mod diagnostics;
pub mod transaction;
pub mod refactor_verify_pipeline;
pub mod delivery_pipeline;

// ===== AST & Code Analysis =====
pub mod ast;
pub mod classifier;
pub mod semantic;
pub mod context_pruner;
pub mod incremental_index;
pub mod proactive_context;
pub mod context;
pub mod reasoning;

// ===== LSP Integration =====
pub mod lsp_client;
pub mod lsp_code_actions;
pub mod lsp_server;
pub mod ide_integration;

// ===== CLI & TUI (cli feature only) =====
#[cfg(feature = "cli")]
pub mod cli;
#[cfg(feature = "cli")]
pub mod tui;
#[cfg(feature = "cli")]
pub mod terminal_launch;
#[cfg(feature = "cli")]
pub mod stdin_detect;
#[cfg(feature = "cli")]
pub mod setup_hints;

// ===== Provider & LLM Integration =====
pub mod provider;
pub mod provider_catalog;
pub mod gateway;
pub mod rest_llm;
pub mod inference_optimizer;
pub mod inference_integration;
pub mod auto_mode;
pub mod embedding;

// ===== Background Tasks =====
pub mod background;
pub mod ambient;
#[cfg(feature = "cli")]
pub mod ambient_runner;
pub mod ambient_scheduler;
pub mod overnight;
pub mod catchup;
pub mod replay;

// ===== Notifications & Communication =====
pub mod notifications;
pub mod message_notifications;
#[cfg(feature = "cli")]
pub mod telegram;
#[cfg(feature = "cli")]
pub mod gmail;
#[cfg(feature = "cli")]
pub mod browser;
#[cfg(feature = "cli")]
pub mod browser_bridge;
pub mod copilot_usage;

// ===== Build & CI/CD =====
pub mod build;
pub mod build_module;
pub mod ci;
pub mod sandbox;
pub mod hooks_system;

// ===== Performance & Optimization =====
pub mod perf;
pub mod cache_tracker;
pub mod cache_optimizer;
pub mod cache_integration;
pub mod cache_break_detector;
pub mod concurrency_optimizer;
#[cfg(feature = "cli")]
pub mod render_optimizer;
pub mod compression;
pub mod circuit_breaker;
pub mod backpressure;
pub mod token_budget;
pub mod denial_tracking;

// ===== Error Handling =====
pub mod error_recovery;
pub mod error_types;
pub mod network_retry;
pub mod allowlist;

// ===== Plugins & Extensions =====
pub mod plugins;
pub mod plugin_market;
pub mod marketplace;
#[cfg(feature = "cli")]
pub mod dashboard;
#[cfg(feature = "cli")]
pub mod debug_panel;
#[cfg(feature = "cli")]
pub mod side_panel;
pub mod i18n;

// ===== Advanced Features =====
#[cfg(feature = "server")]
pub mod distributed;
pub mod ai_optimization;
pub mod ab_testing;
pub mod ai_enhanced;
pub mod codereview;
pub mod workflow;
#[cfg(feature = "cli")]
pub mod buddy;
#[cfg(feature = "cli")]
pub mod voice;
#[cfg(feature = "cli")]
pub mod vim;
pub mod memdir;
pub mod nlp;
pub mod prototype;
pub mod retrieval;
pub mod mab;
pub mod tdd;
pub mod performance_advanced;

// ===== SSH & Remote =====
pub mod ssh;

// ===== Registry & Skills =====
pub mod registry;
pub mod skill;
pub mod skills;

// ===== Message & Channel =====
pub mod message;
pub mod channel;
pub mod bus;

// ===== Legacy/Deprecated Modules =====
//
// DEAD CODE ANALYSIS (2025-05-25):
// The following modules were flagged as potential dead code but analysis shows
// they are ACTIVELY USED and cannot be safely removed:
//
// ✅ HIGH-ACTIVITY MODULES (keep - heavily referenced):
//   - process_memory: 19 refs | Core memory profiling with jemalloc support
//   - plan:          19 refs | Plan types re-exported from jcode-plan crate
//   - runtime_memory_log: 16 refs | Server-side memory logging & attribution
//   - safety:        15 refs | Action classification & permission checking
//
// 🟡 MEDIUM-ACTIVITY MODULES (keep - moderately referenced):
//   - goal:          12 refs | Goal display modes, used by task_planner + TUI
//   - import:        11 refs | Claude Code session import (CLI feature)
//   - todo:          10 refs | TODO item persistence (CLI feature)
//   - login_qr:       9 refs | QR code rendering for OAuth login (CLI)
//   - process_title:  9 refs | Process title management (Unix/Windows)
//   - prompt:         8 refs | System prompt templates & SplitSystemPrompt
//
// 🟢 LOW-ACTIVITY MODULES (keep - lightly referenced but functional):
//   - usage:          6 refs | Subscription usage tracking (Anthropic/OpenAI)
//   - workspace_manager: 6 refs | Multi-project workspace management
//   - restart_snapshot: 5 refs | Session state snapshot for crash recovery
//   - scheduler:       4 refs | Task scheduling with dependencies
//
// ❌ REMOVED MODULES:
//   - update: DELETED on 2025-05-25 | Was dead code (only 2 refs in lib.rs/lib_minimal.rs)
//     Functionality covered by build.rs and scripts/install.sh
//
pub mod crdt;
#[cfg(feature = "cli")]
pub mod dictation;
pub mod env;
pub mod goal;
pub mod import;
#[cfg(feature = "cli")]
pub mod login_qr;
pub mod process_memory;
pub mod process_title;
pub mod prompt;
pub mod restart_snapshot;
pub mod runtime_memory_log;
pub mod safety;
#[cfg(feature = "server")]
pub mod server;
pub mod sidecar;
pub mod soft_interrupt_store;
#[cfg(feature = "cli")]
pub mod startup_profile;
pub mod subscription_catalog;
#[cfg(feature = "cli")]
pub mod todo;
// NOTE: update module removed - dead code (only referenced in lib.rs and lib_minimal.rs)
// Functionality covered by build.rs and scripts/install.sh
pub mod usage;
pub mod scheduler;
pub mod external;
pub mod dap;
pub mod workspace_manager;
pub mod compaction;
pub mod plan;

// ===== P2 Integration =====
pub mod p2_integration;

// ===== Protocol Memory (Legacy) =====
pub mod protocol_memory;

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

/// Main entry point — dispatches to CLI or server based on active features.
///
/// - With `cli` feature: launches TUI interactive client (`cli::startup::run`)
/// - With `server` only (no cli): launches headless server
/// - Default (both): runs CLI (backward compatible)
#[cfg(feature = "cli")]
pub async fn run() -> Result<()> {
    cli::startup::run().await
}

#[cfg(all(not(feature = "cli"), feature = "server"))]
pub async fn run() -> Result<()> {
    // Pure server mode — no TUI, no terminal interaction
    // Starts gRPC + REST + WebSocket servers
    server::startup::run().await
}
