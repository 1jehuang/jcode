//! Integration shim for jcode-auto-dream.
//!
//! Evaluates the three-gate dream coordinator after each ambient cycle
//! to decide whether background memory consolidation should run.

use std::path::PathBuf;

use jcode_auto_dream::{
    ConsolidationLock, DreamCoordinator, DreamTrigger, ScanThrottle, SessionGate,
};

/// Data directory for auto-dream state (locks, session counts).
fn auto_dream_data_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".jcode")
        .join("auto-dream")
}

/// Evaluate whether memory consolidation should run.
///
/// Returns `Some(())` if consolidation was triggered, `None` if gates are not met.
pub fn evaluate_and_maybe_consolidate(
    session_gate_count: u32,
    time_gate_open: bool,
) -> Option<DreamTrigger> {
    let data_dir = auto_dream_data_dir();
    let _ = std::fs::create_dir_all(&data_dir);

    let coordinator = DreamCoordinator {
        time_gate_open,
        session_gate: SessionGate::new(session_gate_count),
        lock: ConsolidationLock::new(data_dir),
    };

    let trigger = coordinator.evaluate();

    match &trigger {
        DreamTrigger::Ready => {
            crate::logging::info("[auto-dream] consolidation gates satisfied - ready to consolidate");
        }
        DreamTrigger::NotReady { reason } => {
            crate::logging::debug(&format!("[auto-dream] gates not met: {}", reason));
        }
    }

    Some(trigger)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_ready_when_time_gate_closed() {
        let result = evaluate_and_maybe_consolidate(10, false);
        assert!(result.is_some());
        assert_ne!(result.unwrap(), DreamTrigger::Ready);
    }
}
