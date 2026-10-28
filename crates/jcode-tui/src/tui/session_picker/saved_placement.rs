//! Saved bookmarks outlive their temporary placement above the All list.
use super::SessionInfo;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const BOOST_DAYS: i64 = 7;

#[derive(Default, Deserialize, Serialize)]
pub(super) struct SavedPlacement {
    #[serde(default)]
    last_picked: HashMap<String, DateTime<Utc>>,
}

impl SavedPlacement {
    fn path() -> anyhow::Result<std::path::PathBuf> {
        Ok(crate::storage::jcode_dir()?.join("session-picker-saved-placement.json"))
    }

    pub(super) fn load() -> Self {
        Self::path()
            .ok()
            .and_then(|path| crate::storage::read_json(&path).ok())
            .unwrap_or_default()
    }

    pub(super) fn boosted(&self, session: &SessionInfo, now: DateTime<Utc>) -> bool {
        self.boosted_at(&session.id, session.saved, session.last_message_time, now)
    }

    fn boosted_at(
        &self,
        id: &str,
        saved: bool,
        legacy_time: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> bool {
        let last_picked = self.last_picked.get(id).copied().unwrap_or(legacy_time);
        saved && now.signed_duration_since(last_picked) < chrono::Duration::days(BOOST_DAYS)
    }

    pub(super) fn record_picks(ids: impl Iterator<Item = String>) {
        let result = (|| -> anyhow::Result<()> {
            let mut state = Self::load();
            let now = Utc::now();
            let mut changed = false;
            for id in ids {
                state.last_picked.insert(id, now);
                changed = true;
            }
            if changed {
                crate::storage::write_json(&Self::path()?, &state)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            crate::logging::warn(&format!(
                "Could not persist saved session placement: {error}"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_boost_expires_without_removing_bookmark() {
        let now = Utc::now();
        let mut state = SavedPlacement::default();
        let old = now - chrono::Duration::days(8);
        assert!(!state.boosted_at("saved", true, old, now));
        assert!(state.boosted_at("saved", true, now, now));
        assert!(!state.boosted_at("saved", false, now, now));
        state.last_picked.insert("saved".into(), now);
        assert!(state.boosted_at("saved", true, old, now));
        assert!(!state.boosted_at("saved", true, now, now + chrono::Duration::days(7)));
        // Background transcript updates must not renew an expired explicit pick.
        state.last_picked.insert("saved".into(), old);
        assert!(!state.boosted_at("saved", true, now, now));
    }
}
