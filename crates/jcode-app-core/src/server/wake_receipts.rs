//! Durable, idempotent acceptance of externally requested session wakes.
//!
//! A queued receipt is deliberately not an assertion that a turn consumed it.
//! Callers must keep it pending until a separate turn-entry proof is recorded.

use crate::protocol::ServerEvent;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use tokio::sync::mpsc;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum WakeState {
    Queued,
    Awakened,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct WakeReceipt {
    pub receipt_id: String,
    pub session_id: String,
    pub message: String,
    pub state: WakeState,
    /// Nanoseconds since the Unix epoch when this receipt was first accepted.
    /// Ledgers written before pruning existed default this to 0, so those
    /// entries sort oldest and are pruned first.
    #[serde(default)]
    pub created_at: u64,
}

/// Hard ceiling on pending receipts. Each accepted receipt costs a full-file
/// rewrite plus fsync, so an unbounded ledger degrades submit latency forever.
const MAX_RECEIPTS: usize = 256;

/// Ceiling on settled tombstones. Enough to keep retries of recent wakes
/// idempotent without letting the file grow without bound.
const MAX_SETTLED: usize = 1024;

/// On-disk ledger: pending receipts plus tombstones for awakened ones that were
/// evicted, so a retried receipt id can never start a second turn.
#[derive(Default, Serialize, Deserialize)]
struct Ledger {
    #[serde(default)]
    receipts: BTreeMap<String, WakeReceipt>,
    #[serde(default)]
    settled: BTreeMap<String, WakeReceipt>,
}

fn receipt_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn path() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("native-wake-receipts.json"))
}

fn load() -> Result<Ledger> {
    let path = path()?;
    if path.exists() {
        crate::storage::read_json(&path)
    } else {
        // A Windows write can be interrupted after moving the primary to
        // .bak and before installing the new file. Absence is only an empty
        // store when neither generation exists.
        let backup = path.with_extension("bak");
        if backup.exists() {
            crate::storage::read_json(&backup)
        } else {
            Ok(Ledger::default())
        }
    }
}

fn save(ledger: &Ledger) -> Result<()> {
    crate::storage::write_json(&path()?, ledger)
}

/// Move the oldest awakened receipts out of the pending ledger once it exceeds
/// its ceiling, keeping a tombstone so a retried id stays idempotent. Queued
/// receipts are never evicted: an accepted queued wake that vanished would make
/// the caller's later poll answer `Unknown wake receipt` with the wake unrun.
fn prune(ledger: &mut Ledger, keep: &str, reserve: usize) {
    let overflow = (ledger.receipts.len() + reserve).saturating_sub(MAX_RECEIPTS);
    if overflow > 0 {
        let mut candidates: Vec<(u64, String)> = ledger
            .receipts
            .values()
            .filter(|receipt| receipt.receipt_id != keep && receipt.state == WakeState::Awakened)
            .map(|receipt| (receipt.created_at, receipt.receipt_id.clone()))
            .collect();
        candidates.sort_unstable();
        for (_, receipt_id) in candidates.into_iter().take(overflow) {
            if let Some(mut receipt) = ledger.receipts.remove(&receipt_id) {
                // The message has already been delivered; the tombstone only
                // needs to identify the id and its target.
                receipt.message = String::new();
                ledger.settled.insert(receipt_id, receipt);
            }
        }
    }

    if ledger.settled.len() > MAX_SETTLED {
        let mut tombstones: Vec<(u64, String)> = ledger
            .settled
            .values()
            .map(|receipt| (receipt.created_at, receipt.receipt_id.clone()))
            .collect();
        tombstones.sort_unstable();
        for (_, receipt_id) in tombstones
            .into_iter()
            .take(ledger.settled.len() - MAX_SETTLED)
        {
            ledger.settled.remove(&receipt_id);
        }
    }
}

fn validate(receipt_id: &str, session_id: &str, message: &str) -> Result<()> {
    if receipt_id.is_empty()
        || receipt_id.len() > 128
        || !receipt_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
    {
        bail!("receipt_id must be 1-128 ASCII letters, digits, dash, underscore, dot, or colon");
    }
    if session_id.is_empty()
        || session_id.len() > 128
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
    {
        bail!("invalid target session_id");
    }
    if message.trim().is_empty() || message.len() > 64 * 1024 {
        bail!("wake message must be nonempty and at most 64 KiB");
    }
    Ok(())
}

/// Native bridge writes are opt-in and scoped to exact existing session IDs.
/// The daemon reads this from its own environment, never from request data.
pub(super) fn target_is_allowed(session_id: &str) -> bool {
    match std::env::var("JCODE_NATIVE_WAKE_SESSION_ALLOWLIST") {
        Ok(list) => list.split(',').map(str::trim).any(|id| id == session_id),
        Err(_) => false,
    }
}

/// Deliver a server event, logging instead of silently dropping it when the
/// client channel is already closed. Receipt handling is best-effort delivery:
/// the receipt itself is durable, so a lost event is a missed notification, not
/// lost work.
pub(super) fn send_event(tx: &mpsc::UnboundedSender<ServerEvent>, event: ServerEvent) {
    if let Err(error) = tx.send(event) {
        crate::logging::debug(&format!("wake receipt event not delivered: {error}"));
    }
}

/// A `ServerEvent::Error` naming the request that could not be answered.
pub(super) fn error_event(id: u64, message: &str) -> ServerEvent {
    ServerEvent::Error {
        id,
        message: message.to_string(),
        retry_after_secs: None,
    }
}

/// Re-read a receipt after attempting to advance it, keeping the value already
/// in hand when the ledger cannot be read.
pub(super) fn reread_or(receipt_id: &str, fallback: WakeReceipt) -> WakeReceipt {
    match get(receipt_id) {
        Ok(Some(updated)) => updated,
        Ok(None) => fallback,
        Err(error) => {
            crate::logging::warn(&format!(
                "wake receipt {receipt_id} re-read failed: {error}"
            ));
            fallback
        }
    }
}

/// Returns the existing record on retry; a reused key with changed payload fails closed.
pub(super) fn submit(receipt_id: &str, session_id: &str, message: &str) -> Result<WakeReceipt> {
    validate(receipt_id, session_id, message)?;
    let _guard = receipt_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut ledger = load()?;
    let existing = ledger
        .receipts
        .get(receipt_id)
        .or_else(|| ledger.settled.get(receipt_id));
    if let Some(existing) = existing {
        // A tombstone keeps only the id and target, so a retried settled wake
        // is compared on its target alone; its message was already delivered.
        if existing.session_id != session_id
            || (existing.state == WakeState::Queued && existing.message != message)
        {
            bail!("receipt_id already belongs to another wake payload");
        }
        return Ok(existing.clone());
    }
    // Make room by settling awakened receipts before refusing the new wake.
    prune(&mut ledger, receipt_id, 1);
    if ledger.receipts.len() >= MAX_RECEIPTS {
        bail!(
            "wake receipt ledger is full ({MAX_RECEIPTS} pending receipts); retry once existing wakes settle"
        );
    }
    let receipt = WakeReceipt {
        receipt_id: receipt_id.to_string(),
        session_id: session_id.to_string(),
        message: message.to_string(),
        state: WakeState::Queued,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or(0),
    };
    ledger
        .receipts
        .insert(receipt_id.to_string(), receipt.clone());
    prune(&mut ledger, receipt_id, 0);
    save(&ledger)?;
    Ok(receipt)
}

pub(super) fn get(receipt_id: &str) -> Result<Option<WakeReceipt>> {
    let _guard = receipt_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let ledger = load()?;
    Ok(ledger
        .receipts
        .get(receipt_id)
        .or_else(|| ledger.settled.get(receipt_id))
        .cloned())
}

/// Called only after the exact receipt's content has been saved in the target transcript.
pub(crate) fn mark_awakened(receipt_id: &str, session_id: &str) -> Result<()> {
    let _guard = receipt_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut ledger = load()?;
    if let Some(settled) = ledger.settled.get(receipt_id) {
        if settled.session_id != session_id {
            bail!("receipt target mismatch");
        }
        // Already settled and evicted: recording the proof again is a no-op.
        return Ok(());
    }
    let Some(receipt) = ledger.receipts.get_mut(receipt_id) else {
        bail!("unknown receipt_id");
    };
    if receipt.session_id != session_id {
        bail!("receipt target mismatch");
    }
    receipt.state = WakeState::Awakened;
    save(&ledger)
}

pub(super) fn status_event(id: u64, receipt: WakeReceipt) -> crate::protocol::ServerEvent {
    crate::protocol::ServerEvent::WakeReceipt {
        id,
        receipt_id: receipt.receipt_id,
        session_id: receipt.session_id,
        status: match receipt.state {
            WakeState::Queued => "queued",
            WakeState::Awakened => "awakened",
        }
        .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_receipt_survives_reload_and_retries_without_duplicate() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("temp dir");
        let old_home = std::env::var_os("JCODE_HOME");
        let old_allowlist = std::env::var_os("JCODE_NATIVE_WAKE_SESSION_ALLOWLIST");
        crate::env::set_var("JCODE_HOME", temp.path());
        crate::env::set_var(
            "JCODE_NATIVE_WAKE_SESSION_ALLOWLIST",
            "ses_target,ses_second",
        );
        assert!(target_is_allowed("ses_target"));
        assert!(!target_is_allowed("ses_target_extra"));

        let first = submit("discord:event:1", "ses_target", "wake now").expect("submit");
        assert_eq!(first.state, WakeState::Queued);
        assert_eq!(get("discord:event:1").expect("reload"), Some(first.clone()));
        assert_eq!(
            submit("discord:event:1", "ses_target", "wake now")
                .expect("retry")
                .state,
            WakeState::Queued
        );
        assert!(submit("discord:event:1", "ses_other", "wake now").is_err());
        assert!(submit("discord:event:1", "ses_target", "changed").is_err());
        assert!(submit("../escape", "ses_target", "wake now").is_err());

        mark_awakened("discord:event:1", "ses_target").expect("transcript committed");
        assert_eq!(
            submit("discord:event:1", "ses_target", "wake now")
                .expect("retry after wake")
                .state,
            WakeState::Awakened
        );

        if let Some(old_home) = old_home {
            crate::env::set_var("JCODE_HOME", old_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
        if let Some(old_allowlist) = old_allowlist {
            crate::env::set_var("JCODE_NATIVE_WAKE_SESSION_ALLOWLIST", old_allowlist);
        } else {
            crate::env::remove_var("JCODE_NATIVE_WAKE_SESSION_ALLOWLIST");
        }
    }

    #[test]
    fn interrupted_replace_recovers_backup_without_erasing_history() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("temp dir");
        let old_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        let first = submit("bridge:event:old", "ses_target", "first").expect("submit");
        let primary = path().expect("receipt path");
        let backup = primary.with_extension("bak");
        std::fs::rename(&primary, &backup).expect("simulate interrupted Windows replace");
        assert_eq!(
            get(&first.receipt_id).expect("recover absent primary"),
            Some(first)
        );
        submit("bridge:event:new", "ses_target", "second").expect("preserve recovered history");
        assert!(get("bridge:event:old").expect("reload").is_some());
        assert!(get("bridge:event:new").expect("reload").is_some());

        // A corrupt primary must recover the prior generation as well.
        std::fs::write(&primary, b"{").expect("simulate torn primary");
        assert!(
            get("bridge:event:old")
                .expect("recover corrupt primary")
                .is_some()
        );
        std::fs::write(&backup, b"{").expect("simulate corrupt backup");
        std::fs::write(&primary, b"{").expect("simulate corrupt primary again");
        assert!(get("bridge:event:old").is_err());
        assert!(submit("bridge:event:third", "ses_target", "third").is_err());

        if let Some(old_home) = old_home {
            crate::env::set_var("JCODE_HOME", old_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn ledger_rejects_new_wakes_instead_of_evicting_queued_ones() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("temp dir");
        let old_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        for index in 0..MAX_RECEIPTS {
            submit(&format!("bound:event:{index}"), "ses_target", "wake").expect("submit");
        }
        // The ledger is full of queued receipts: a new wake is refused rather
        // than silently dropping an already-accepted one.
        let rejected = submit("bound:event:new", "ses_target", "wake");
        assert!(rejected.is_err(), "a full ledger must refuse new wakes");

        let ledger = load().expect("reload");
        assert_eq!(ledger.receipts.len(), MAX_RECEIPTS);
        for index in 0..MAX_RECEIPTS {
            assert!(
                ledger
                    .receipts
                    .contains_key(&format!("bound:event:{index}")),
                "queued receipt {index} must survive"
            );
        }

        // Settling one makes room again.
        mark_awakened("bound:event:0", "ses_target").expect("settle");
        submit("bound:event:new", "ses_target", "wake").expect("room after settle");

        if let Some(old_home) = old_home {
            crate::env::set_var("JCODE_HOME", old_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn evicted_awakened_receipt_still_blocks_a_second_turn() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("temp dir");
        let old_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        for index in 0..MAX_RECEIPTS {
            submit(&format!("prune:event:{index}"), "ses_target", "wake").expect("submit");
        }
        mark_awakened("prune:event:0", "ses_target").expect("settle");
        // Over the ceiling again: the awakened entry is the only eviction
        // candidate, so it becomes a tombstone instead of disappearing.
        submit("prune:event:extra", "ses_target", "wake").expect("submit");

        let ledger = load().expect("reload");
        assert!(!ledger.receipts.contains_key("prune:event:0"));
        assert!(ledger.settled.contains_key("prune:event:0"));

        let retried = get("prune:event:0").expect("reload").expect("tombstone");
        assert_eq!(retried.state, WakeState::Awakened);
        assert_eq!(
            submit("prune:event:0", "ses_target", "wake")
                .expect("retry stays settled")
                .state,
            WakeState::Awakened
        );
        assert!(
            submit("prune:event:0", "ses_other", "wake").is_err(),
            "a tombstone still pins its target"
        );
        mark_awakened("prune:event:0", "ses_target").expect("re-settling is a no-op");

        if let Some(old_home) = old_home {
            crate::env::set_var("JCODE_HOME", old_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}
