//! Self-contained OSC 9 status emission for the ask_user decision chooser.
//!
//! herdr and herdr-webui detect a blocked agent through the OSC 9 progress
//! side-channel: the payload format is `jcode:<state>` (see herdr-webui's
//! `osc_progress_status_for_agent`). While the blocking ask_user chooser is
//! visible the turn is paused waiting for the human, so the agent status is
//! `blocked`; when the chooser is answered or dismissed the turn resumes, so
//! the status returns to `working`.
//!
//! This module is deliberately independent of the broader OSC 9 status
//! emission work: it only handles the ask_user chooser transitions, it writes
//! to stdout best-effort, and it dedups transitions so repeated calls with
//! the same state are safe.

use std::sync::Mutex;

/// Last ask_user-chooser OSC state emitted, for transition dedup.
static LAST_DECISION_OSC_STATE: Mutex<Option<&'static str>> = Mutex::new(None);

/// Build the OSC 9 escape sequence carrying a jcode agent-status payload.
///
/// Format: `ESC ] 9 ; jcode:<state> BEL`. Kept as a pure function so tests
/// can assert the exact bytes without touching stdout.
pub fn decision_osc9_bytes(state: &str) -> Vec<u8> {
    format!("\x1b]9;jcode:{state}\x07").into_bytes()
}

/// Emit an OSC 9 status for the ask_user chooser transition, deduped.
///
/// `true` means a decision became pending (emits `blocked`), `false` means it
/// was resolved (emits `working`, since answering resumes the turn). Repeated
/// calls with the same state are skipped, so it is safe to call from every
/// mutation site. Write failures (piped stdout, closed terminal) are ignored:
/// the payload is advisory status for multiplexers, not load-bearing data.
pub fn emit_decision_osc9(chooser_present: bool) {
    let state: &'static str = if chooser_present {
        "blocked"
    } else {
        "working"
    };
    let mut last = LAST_DECISION_OSC_STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if *last == Some(state) {
        return;
    }
    *last = Some(state);
    let bytes = decision_osc9_bytes(state);
    use std::io::Write as _;
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(&bytes);
    let _ = stdout.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_format_matches_herdr_contract() {
        assert_eq!(decision_osc9_bytes("blocked"), b"\x1b]9;jcode:blocked\x07");
        assert_eq!(decision_osc9_bytes("working"), b"\x1b]9;jcode:working\x07");
        assert_eq!(decision_osc9_bytes("idle"), b"\x1b]9;jcode:idle\x07");
    }
}
