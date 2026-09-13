//! Auto-dream integration for the ambient runner.
//!
//! After each ambient cycle, evaluates the three-gate `DreamCoordinator`
//! (time, session count, file lock) and triggers background memory
//! consolidation when all gates pass.

use std::path::PathBuf;

use jcode_auto_dream::{DreamCoordinator, DreamTrigger};

use crate::logging;

/// Evaluate the dream coordinator and, if ready, spawn a background
/// consolidation task.
///
/// `data_dir` is the jcode data directory (e.g. `~/.jcode`).
/// `session_count` is the total number of ambient cycles completed
/// (fed to the session gate).
pub fn maybe_dream(data_dir: PathBuf, session_count: u64) {
    let mut coordinator = DreamCoordinator::new(data_dir);

    // Feed the session gate with the ambient cycle count.
    // The coordinator's session gate starts unsatisfied (0 sessions).
    // We record each completed cycle so the gate can count toward its threshold.
    let mut state = coordinator.session_gate.reset();
    for _ in 0..session_count {
        state = coordinator.session_gate.record_session(state);
    }

    // Evaluate the time gate using wall-clock time.
    // The coordinator stores time_gate_open as a bool, so we check it
    // directly. The time gate is satisfied if 24h have elapsed since
    // last consolidation (tracked by the lock file's mtime).
    coordinator.time_gate_open = evaluate_time_gate(&coordinator);

    match coordinator.evaluate() {
        DreamTrigger::Ready => {
            logging::info("Auto-dream: all gates passed, spawning consolidation");
            let lock = coordinator.lock;
            tokio::spawn(async move {
                match lock.acquire().await {
                    Ok(()) => {
                        logging::info("Auto-dream: lock acquired, running consolidation");
                        // TODO: run the actual consolidation LLM call here
                        // using ConsolidationPrompt::build(entries, cluster_info)
                        // For now, release the lock after a placeholder.
                        if let Err(e) = lock.release().await {
                            logging::error(&format!("Auto-dream: failed to release lock: {}", e));
                        }
                        logging::info("Auto-dream: consolidation complete");
                    }
                    Err(e) => {
                        logging::error(&format!("Auto-dream: failed to acquire lock: {}", e));
                    }
                }
            });
        }
        DreamTrigger::NotReady { reason } => {
            logging::info(&format!("Auto-dream: not ready ({})", reason));
        }
    }
}

/// Check whether the time gate is open by inspecting the lock file mtime.
///
/// If no lock file exists, the gate is open (first run). If the lock file
/// exists and was created more than 24 hours ago, the gate is open.
fn evaluate_time_gate(coordinator: &DreamCoordinator) -> bool {
    use std::time::{Duration, SystemTime};

    let lock_path = coordinator.lock.lock_path();
    let mtime = match std::fs::metadata(lock_path)
        .ok()
        .and_then(|m| m.modified().ok())
    {
        Some(t) => t,
        None => return true, // No lock file = first run = gate open
    };
    let elapsed = SystemTime::now()
        .duration_since(mtime)
        .unwrap_or(Duration::ZERO);
    elapsed >= Duration::from_secs(jcode_auto_dream::DEFAULT_TIME_GATE_HOURS * 3600)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_gate_open_when_no_lock_file() {
        let coordinator = DreamCoordinator::new("/tmp/auto-dream-test-nonexistent");
        assert!(evaluate_time_gate(&coordinator));
    }
}
