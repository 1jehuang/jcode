//! Keys of a named Anthropic-compatible profile. A session pins one pool
//! entry and keeps its key across turns.

use std::sync::RwLock;

use anyhow::Result;
use jcode_base::provider::anthropic::{
    anthropic_api_key_pool_names, load_anthropic_api_key, load_anthropic_profile_key,
};
use jcode_provider_core::fingerprint::pool_index;

/// Keys for one profile. They resolve at construction because the profile env
/// is cleared once the runtime starts.
pub(crate) struct KeyPool {
    /// `None` loads the key per request, as before. Each entry keeps its own
    /// error. One bad entry fails only the session that uses it.
    keys: Option<Vec<std::result::Result<String, String>>>,
    /// Profile name for log lines. Set only for a pool from `JCODE_ANTHROPIC_API_KEY_NAMES`.
    pool_profile: Option<String>,
    session: RwLock<Option<String>>,
}

impl KeyPool {
    pub(crate) fn from_env() -> Self {
        let pool = anthropic_api_key_pool_names();
        let keys = std::env::var_os("JCODE_ANTHROPIC_API_BASE").map(|_| match &pool {
            Some(names) => names
                .iter()
                .map(|name| load_anthropic_profile_key(name).map_err(|err| format!("{err:#}")))
                .collect::<Vec<_>>(),
            None => vec![load_anthropic_api_key().map_err(|err| format!("{err:#}"))],
        });
        let pool_profile = pool.map(|_| {
            std::env::var("JCODE_NAMED_PROVIDER_PROFILE")
                .unwrap_or_else(|_| "anthropic".to_string())
        });
        Self {
            keys,
            pool_profile,
            session: RwLock::new(None),
        }
    }

    /// Key for the bound session. Unbound calls take the first entry that resolves.
    pub(crate) fn key(&self) -> Result<String> {
        let Some(keys) = &self.keys else {
            return load_anthropic_api_key();
        };
        let session = self
            .session
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let index = match session.as_deref() {
            // Unbound calls take the first entry that resolves, else the error of entry 0.
            None | Some("") => keys.iter().position(Result::is_ok).unwrap_or(0),
            id => pool_index(id, keys.len()),
        };
        match keys.get(index) {
            Some(Ok(key)) => Ok(key.clone()),
            Some(Err(err)) => anyhow::bail!(err.clone()),
            None => anyhow::bail!("Anthropic key pool is empty"),
        }
    }

    /// Pins a session and logs its key index on each change. Empty ids are ignored.
    pub(crate) fn bind(&self, session_id: &str) {
        if session_id.is_empty() {
            return;
        }
        let mut pinned = self
            .session
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if pinned.as_deref() == Some(session_id) {
            return;
        }
        *pinned = Some(session_id.to_string());
        if let (Some(profile), Some(keys)) = (&self.pool_profile, &self.keys) {
            jcode_base::logging::info(&format!(
                "key pool {profile}: session {session_id} uses key {}/{}",
                pool_index(Some(session_id), keys.len()) + 1,
                keys.len()
            ));
        }
    }

    /// Copy for a forked provider. The fork keeps the parent's pin.
    pub(crate) fn fork(&self) -> Self {
        let session = self
            .session
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        Self {
            keys: self.keys.clone(),
            pool_profile: self.pool_profile.clone(),
            session: RwLock::new(session),
        }
    }
}

#[cfg(test)]
#[path = "key_pool_tests.rs"]
mod tests;
