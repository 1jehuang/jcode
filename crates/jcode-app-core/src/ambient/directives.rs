use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::paths::ambient_dir;
use crate::storage;

// ---------------------------------------------------------------------------
// User Directives (from email replies)
// ---------------------------------------------------------------------------

/// A user directive received via email reply to an ambient cycle notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserDirective {
    pub id: String,
    pub text: String,
    pub received_at: DateTime<Utc>,
    pub in_reply_to_cycle: String,
    pub consumed: bool,
}

fn directives_path() -> Result<PathBuf> {
    Ok(ambient_dir()?.join("directives.json"))
}

pub fn load_directives() -> Vec<UserDirective> {
    directives_path()
        .ok()
        .and_then(|p| {
            if p.exists() {
                storage::read_json(&p).ok()
            } else {
                None
            }
        })
        .unwrap_or_default()
}

fn save_directives(directives: &[UserDirective]) -> Result<()> {
    storage::write_json(&directives_path()?, directives)
}

/// Store a new directive from an email reply.
pub fn add_directive(text: String, in_reply_to: String) -> Result<()> {
    let mut directives = load_directives();
    directives.push(UserDirective {
        id: format!("dir_{:08x}", rand::random::<u32>()),
        text,
        received_at: Utc::now(),
        in_reply_to_cycle: in_reply_to,
        consumed: false,
    });
    save_directives(&directives)
}

/// Take all unconsumed directives, marking them as consumed.
pub fn take_pending_directives() -> Vec<UserDirective> {
    let mut all = load_directives();
    let pending: Vec<_> = all.iter().filter(|d| !d.consumed).cloned().collect();
    if pending.is_empty() {
        return pending;
    }
    for d in &mut all {
        if !d.consumed {
            d.consumed = true;
        }
    }
    let _ = save_directives(&all);
    pending
}

/// Check if there are any unconsumed directives.
pub fn has_pending_directives() -> bool {
    load_directives().iter().any(|d| !d.consumed)
}
