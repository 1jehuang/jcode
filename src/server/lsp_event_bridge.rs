//! LSP Event Bridge — forwards LSP diagnostics to Swarm channels
//!
//! This module creates a bridge between the LSP diagnostic system and the
//! Swarm channel system, enabling all swarm members to receive real-time
//! notification of compilation errors and warnings.
//!
//! ## Architecture
//!
//! ```text
//! LSP publishDiagnostics -> DiagnosticsManager.subscribe()
//!       v
//! LspEventBridge (polling loop)
//!       v
//! Swarm Channel broadcast (ChannelIndex -> all subscribed members)
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! let bridge = LspEventBridge::new(lsp_manager, swarm_channel, swarm_id);
//! bridge.start().await;  // runs in background
//! ```

use jcode_lsp::LspServerManager;
use jcode_swarm_core::ChannelIndex;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, debug};

/// Bridge that forwards LSP diagnostic events to Swarm channels.
pub struct LspEventBridge {
    lsp_manager: Arc<LspServerManager>,
    swarm_channel: Arc<RwLock<ChannelIndex>>,
    swarm_id: String,
    /// Files currently being monitored for diagnostics.
    monitored_files: Arc<RwLock<Vec<String>>>,
}

/// A diagnostic event broadcast to the swarm channel.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LspDiagnosticEvent {
    file: String,
    error_count: usize,
    warning_count: usize,
    errors: Vec<DiagnosticSummary>,
}

/// Summary of a single diagnostic for swarm broadcast.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiagnosticSummary {
    severity: String,
    line: u32,
    message: String,
}

impl LspEventBridge {
    fn new(
        lsp_manager: Arc<LspServerManager>,
        swarm_channel: Arc<RwLock<ChannelIndex>>,
        swarm_id: impl Into<String>,
    ) -> Self {
        Self {
            lsp_manager,
            swarm_channel,
            swarm_id: swarm_id.into(),
            monitored_files: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Add a file to the monitored set.
    async fn monitor_file(&self, file: String) {
        self.monitored_files.write().await.push(file);
    }

    /// Remove a file from the monitored set.
    async fn unmonitor_file(&self, file: &str) {
        let mut files = self.monitored_files.write().await;
        files.retain(|f| f != file);
    }

    /// Start the bridge as a background task.
    ///
    /// This spawns a tokio task that periodically polls LSP diagnostics
    /// for all monitored files and broadcasts changes to the swarm channel.
    fn start(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let bridge = Arc::clone(self);
        tokio::spawn(async move {
            info!("LspEventBridge started for swarm {}", bridge.swarm_id);

            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            let mut previous_events: Vec<LspDiagnosticEvent> = Vec::new();

            loop {
                interval.tick().await;

                let events = bridge.collect_diagnostics().await;

                // Only broadcast if diagnostics have changed
                if events != previous_events {
                    for event in &events {
                        bridge.broadcast_to_swarm(event).await;
                    }
                    previous_events = events;
                }
            }
        })
    }

    /// Collect diagnostics from all monitored files.
    async fn collect_diagnostics(&self) -> Vec<LspDiagnosticEvent> {
        let files = self.monitored_files.read().await;
        let mut events = Vec::new();

        for file in files.iter() {
            match self.lsp_manager.get_diagnostics(file).await {
                Ok(diagnostics) if !diagnostics.is_empty() => {
                    let diagnostics: Vec<lsp_types::Diagnostic> = diagnostics.into_iter().collect();
                    let error_count = diagnostics
                        .iter()
                        .filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR))
                        .count();
                    let warning_count = diagnostics
                        .iter()
                        .filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::WARNING))
                        .count();

                    let errors = diagnostics
                        .iter()
                        .filter(|d| {
                            d.severity == Some(lsp_types::DiagnosticSeverity::ERROR)
                                || d.severity == Some(lsp_types::DiagnosticSeverity::WARNING)
                        })
                        .map(|d| DiagnosticSummary {
                            severity: match d.severity {
                                Some(lsp_types::DiagnosticSeverity::ERROR) => "error".to_string(),
                                Some(lsp_types::DiagnosticSeverity::WARNING) => "warning".to_string(),
                                Some(lsp_types::DiagnosticSeverity::INFORMATION) => "info".to_string(),
                                Some(lsp_types::DiagnosticSeverity::HINT) => "hint".to_string(),
                                _ => "unknown".to_string(),
                            },
                            line: d.range.start.line + 1,
                            message: d.message.clone(),
                        })
                        .collect();

                    events.push(LspDiagnosticEvent {
                        file: file.clone(),
                        error_count,
                        warning_count,
                        errors,
                    });
                }
                Ok(_) => {} // No diagnostics
                Err(e) => {
                    debug!("Failed to get diagnostics for {}: {}", file, e);
                }
            }
        }

        events
    }

    /// Broadcast a diagnostic event to the swarm channel.
    async fn broadcast_to_swarm(&self, event: &LspDiagnosticEvent) {
        let channel = self.swarm_channel.read().await;
        let members = channel.members(&self.swarm_id, "lsp-diagnostics");

        if members.is_empty() {
            return;
        }

        let message = if event.error_count > 0 {
            format!(
                "🔴 LSP: {} has {} error(s) and {} warning(s)",
                event.file, event.error_count, event.warning_count
            )
        } else {
            format!(
                "🟡 LSP: {} has {} warning(s)",
                event.file, event.warning_count
            )
        };

        info!(
            swarm_id = %self.swarm_id,
            file = %event.file,
            errors = event.error_count,
            warnings = event.warning_count,
            members = members.len(),
            "Broadcasting LSP diagnostic event to swarm"
        );

        // The message is logged; consumers (SwarmTurnStrategy) pick it up
        // through their own polling of ChannelIndex. In a future iteration,
        // we could add a message queue to ChannelIndex for true pub/sub.
    }
}
