//! Shared guards that bound an assistant turn. Kept separate from the large
//! turn driver so the production file-size budget stays flat.

use super::{App, DisplayMessage};

/// Start the fixed deadline and retry budget together.
pub(super) fn start_bounds() -> (crate::provider::TurnDeadline, crate::network_retry::RetryBudget) {
    let cfg = crate::config::config();
    (crate::provider::TurnDeadline::start(),
     crate::network_retry::RetryBudget::new(cfg.provider.max_retries, cfg.provider.retry_backoff_cap_secs))
}

/// Error message shown when the fixed total-turn deadline fires.
pub(super) fn turn_deadline_message() -> String {
    format!(
        "Turn timed out after {} seconds. No further provider activity will be accepted; try again with a shorter request.",
        crate::provider::turn_deadline().as_secs()
    )
}

/// Push the visible deadline error and fail the turn. The caller's normal
/// error path clears `is_processing` via `handle_turn_error` and `finish_turn`.
pub(super) fn fail_turn_deadline(app: &mut App) -> anyhow::Result<()> {
    app.push_display_message(DisplayMessage::error(turn_deadline_message()));
    Err(anyhow::anyhow!("Turn deadline exceeded"))
}

/// Push the visible retry-limit error and fail the turn.
pub(super) fn fail_retry_limit(app: &mut App) -> anyhow::Result<()> {
    app.push_display_message(DisplayMessage::error(
        "Network retry limit reached; ending turn. Check connectivity and try again.",
    ));
    Err(anyhow::anyhow!("Network retry limit exceeded"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_error_text_is_visible() {
        let message = turn_deadline_message();
        assert!(message.contains("Turn timed out"));
        assert!(message.contains("1800"));
    }
}
