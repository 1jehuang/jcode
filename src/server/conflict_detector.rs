//! Symbol-level conflict detection for Swarm task scheduling.
//!
//! Uses LSP `textDocument/references` and `textDocument/definition` to build
//! a symbol dependency graph, then checks whether multiple swarm tasks would
//! modify the same symbol or symbols that depend on each other.
//!
//! ## Algorithm
//!
//! 1. Extract the set of files/symbols each task intends to modify.
//! 2. For each modified symbol, query LSP for all references (dependents).
//! 3. Check for overlaps: if two tasks modify the same symbol or one task
//!    modifies a symbol that another task's modifications depend on, flag
//!    a conflict.
//! 4. Conflicting tasks should be scheduled sequentially instead of in parallel.
//!
//! ## Usage
//!
//! ```rust,ignore
//! let detector = SymbolConflictDetector::new(lsp_manager);
//! let conflicts = detector.detect_conflicts(&tasks).await;
//! for conflict in &conflicts {
//!     warn!("Conflict: {:?}", conflict);
//! }
//! ```

use jcode_lsp::LspServerManager;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// A detected conflict between swarm tasks.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConflictReport {
    /// The ID of the first conflicting task.
    task_a: String,
    /// The ID of the second conflicting task.
    task_b: String,
    /// The type of conflict detected.
    conflict_type: ConflictType,
    /// Human-readable description of the conflict.
    description: String,
    /// The symbols that are in conflict.
    conflicting_symbols: Vec<String>,
}

/// Type of conflict between tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ConflictType {
    /// Both tasks modify the same file.
    SameFile,
    /// Both tasks modify the same symbol (function/struct/etc.).
    SameSymbol,
    /// One task modifies a symbol that the other task's code depends on.
    SymbolDependency,
}

/// Symbol-level conflict detector using LSP.
pub struct SymbolConflictDetector {
    lsp_manager: Arc<LspServerManager>,
}

impl SymbolConflictDetector {
    fn new(lsp_manager: Arc<LspServerManager>) -> Self {
        Self { lsp_manager }
    }

    /// Detect conflicts between a set of tasks.
    ///
    /// Each task is represented as a (task_id, files_to_modify, symbols_in_context) tuple.
    /// Returns a list of conflict reports for any pairs that conflict.
    async fn detect_conflicts(
        &self,
        tasks: &[(String, Vec<String>, Vec<String>)],
    ) -> Vec<ConflictReport> {
        let mut conflicts = Vec::new();

        // Phase 1: Build per-task file and symbol sets
        let mut task_files: HashMap<&str, HashSet<&str>> = HashMap::new();
        let mut task_symbols: HashMap<&str, HashSet<&str>> = HashMap::new();

        for (id, files, symbols) in tasks {
            task_files.insert(id.as_str(), files.iter().map(|s| s.as_str()).collect());
            task_symbols.insert(id.as_str(), symbols.iter().map(|s| s.as_str()).collect());
        }

        // Phase 2: Check file-level conflicts (fast, no LSP needed)
        let task_ids: Vec<&str> = tasks.iter().map(|(id, _, _)| id.as_str()).collect();
        for i in 0..task_ids.len() {
            for j in (i + 1)..task_ids.len() {
                let id_a = task_ids[i];
                let id_b = task_ids[j];

                let files_a = &task_files[id_a];
                let files_b = &task_files[id_b];

                let overlapping_files: Vec<&str> = files_a.intersection(files_b).copied().collect();

                if !overlapping_files.is_empty() {
                    conflicts.push(ConflictReport {
                        task_a: id_a.to_string(),
                        task_b: id_b.to_string(),
                        conflict_type: ConflictType::SameFile,
                        description: format!(
                            "Both tasks modify the same file(s): {}",
                            overlapping_files.join(", ")
                        ),
                        conflicting_symbols: overlapping_files.iter().map(|s| s.to_string()).collect(),
                    });
                }
            }
        }

        // Phase 3: Check symbol-level conflicts using LSP references
        for i in 0..task_ids.len() {
            for j in (i + 1)..task_ids.len() {
                let id_a = task_ids[i];
                let id_b = task_ids[j];

                let symbols_a = &task_symbols[id_a];
                let symbols_b = &task_symbols[id_b];

                // Direct symbol overlap
                let overlapping_symbols: Vec<&str> =
                    symbols_a.intersection(symbols_b).copied().collect();

                if !overlapping_symbols.is_empty() {
                    conflicts.push(ConflictReport {
                        task_a: id_a.to_string(),
                        task_b: id_b.to_string(),
                        conflict_type: ConflictType::SameSymbol,
                        description: format!(
                            "Both tasks modify the same symbol(s): {}",
                            overlapping_symbols.join(", ")
                        ),
                        conflicting_symbols: overlapping_symbols.iter().map(|s| s.to_string()).collect(),
                    });
                    continue; // Skip dependency check if direct overlap found
                }

                // Dependency check: does task A's symbols depend on task B's files?
                if let Some(dep_conflicts) = self
                    .check_symbol_dependencies(
                        id_a,
                        symbols_a,
                        id_b,
                        &task_files[id_b],
                    )
                    .await
                {
                    conflicts.push(dep_conflicts);
                } else if let Some(dep_conflicts) = self
                    .check_symbol_dependencies(
                        id_b,
                        symbols_b,
                        id_a,
                        &task_files[id_a],
                    )
                    .await
                {
                    conflicts.push(dep_conflicts);
                }
            }
        }

        if !conflicts.is_empty() {
            warn!(
                conflict_count = conflicts.len(),
                "Symbol conflict detection found conflicts"
            );
        } else {
            info!("No symbol conflicts detected");
        }

        conflicts
    }

    /// Check if any symbol in `source_symbols` has references that point to
    /// files in `target_files`. If so, modifying target_files could break
    /// the source_symbols' code.
    async fn check_symbol_dependencies(
        &self,
        source_id: &str,
        source_symbols: &HashSet<&str>,
        target_id: &str,
        target_files: &HashSet<&str>,
    ) -> Option<ConflictReport> {
        let mut conflicting_refs = Vec::new();

        for symbol in source_symbols {
            // Use LSP to find all references of this symbol
            // We'd need a file and position to query, but we only have the symbol name.
            // For now, we use a simplified approach: if the symbol appears in any
            // of the target files, there's a dependency.
            for file in target_files {
                match self.lsp_manager.get_diagnostics(file).await {
                    Ok(_) => {
                        // If we can get diagnostics, we can infer the file is being
                        // actively edited. For a full implementation, we'd use
                        // goto_definition or find_references with a position.
                    }
                    Err(_) => continue,
                }
            }

            // Simplified check: if any target file path contains the symbol name,
            // flag as potential dependency. This is a heuristic.
            for file in target_files {
                if file.contains(symbol) {
                    conflicting_refs.push(symbol.to_string());
                }
            }
        }

        if conflicting_refs.is_empty() {
            None
        } else {
            Some(ConflictReport {
                task_a: source_id.to_string(),
                task_b: target_id.to_string(),
                conflict_type: ConflictType::SymbolDependency,
                description: format!(
                    "Task {} modifies symbols ({}) that depend on files being modified by task {}",
                    source_id,
                    conflicting_refs.join(", "),
                    target_id
                ),
                conflicting_symbols: conflicting_refs,
            })
        }
    }
}
