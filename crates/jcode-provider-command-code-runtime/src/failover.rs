//! Bounded, pre-stream Command Code account failover.
use std::time::Duration;
pub const PROVIDER_KEY: &str = "command-code";
pub const MAX_FAILOVER_ATTEMPTS: usize = 1;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandCodeFailoverAction { StickWait { delay: Duration }, Rotate { previous_account: String, next_account: String }, AllExhausted { message: String }, NoAction }
/// Affinity-first ordering over daemon-owned account labels.
pub fn command_code_pool_candidates(accounts: &[String], affinity: Option<&str>) -> Vec<String> {
    let mut eligible: Vec<String> = accounts.iter().filter(|label| !jcode_provider_core::is_account_in_cooldown(PROVIDER_KEY, label)).cloned().collect();
    if let Some(affinity) = affinity { if let Some(pos) = eligible.iter().position(|label| label == affinity) { let bound = eligible.remove(pos); eligible.insert(0, bound); } }
    eligible
}
/// Classify only a pre-stream 429. Once output is visible, account selection is immutable.
pub fn handle_command_code_error_failover(account: &str, status: u16, retry_after: Option<Duration>, candidates: &[String], output_started: bool, attempt: usize) -> CommandCodeFailoverAction {
    if status != 429 || output_started || attempt >= MAX_FAILOVER_ATTEMPTS { return CommandCodeFailoverAction::NoAction; }
    if let Some(delay) = retry_after { if delay.as_secs() <= jcode_provider_core::STICK_WAIT_MAX_SECS { return CommandCodeFailoverAction::StickWait { delay }; } jcode_provider_core::set_account_cooldown(PROVIDER_KEY, account, "quota", Some(delay)); } else { jcode_provider_core::set_account_cooldown(PROVIDER_KEY, account, "quota", None); }
    candidates.iter().find(|label| label.as_str() != account).cloned().map(|next_account| CommandCodeFailoverAction::Rotate { previous_account: account.to_owned(), next_account }).unwrap_or_else(|| CommandCodeFailoverAction::AllExhausted { message: "All Command Code accounts are in cooldown or exhausted".into() })
}
