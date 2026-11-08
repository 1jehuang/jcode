//! Pins one `api_key_envs` entry per session for a named profile. A fork keeps the pin.

use super::*;
use jcode_base::config::{NamedProviderAuth, NamedProviderConfig};
use jcode_provider_core::fingerprint::pool_index;

pub(crate) struct KeyPool {
    name: String,
    profile: Arc<NamedProviderConfig>,
    session: Arc<std::sync::RwLock<Option<String>>>,
}

impl KeyPool {
    /// Rejects an unsafe pool entry name before any request runs.
    pub(crate) fn new(name: &str, profile: &NamedProviderConfig) -> Result<Self> {
        for env_name in &profile.api_key_envs {
            if !is_safe_env_key_name(env_name) {
                anyhow::bail!("Provider profile '{name}' has invalid api_key_env '{env_name}'.");
            }
        }
        Ok(Self {
            name: name.to_string(),
            profile: Arc::new(profile.clone()),
            session: Arc::new(std::sync::RwLock::new(None)),
        })
    }

    pub(crate) fn auth(&self) -> AuthResolver {
        named_profile_auth(Arc::clone(&self.profile), Arc::clone(&self.session))
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
        let len = self.profile.api_key_envs.len();
        if len > 0 {
            let name = &self.name;
            let index = pool_index(Some(session_id), len) + 1;
            jcode_base::logging::info(&format!(
                "key pool {name}: session {session_id} uses key {index}/{len}"
            ));
        }
    }

    /// The fork copies the parent's pin into its own lock.
    pub(crate) fn fork(&self) -> Self {
        let session = self
            .session
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        Self {
            name: self.name.clone(),
            profile: Arc::clone(&self.profile),
            session: Arc::new(std::sync::RwLock::new(session)),
        }
    }
}

/// Credential resolver for a named profile. A pool picks its entry from the
/// session pin on each call. The key follows the session.
fn named_profile_auth(
    profile: Arc<NamedProviderConfig>,
    session: Arc<std::sync::RwLock<Option<String>>>,
) -> AuthResolver {
    Arc::new(move || {
        let (key_env, key) = if profile.api_key_envs.is_empty() {
            let key_env = profile
                .api_key_env
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string);
            let key = key_env
                .as_deref()
                .and_then(|name| load_named_profile_api_key(name, &profile))
                .or_else(|| profile.api_key.clone());
            (key_env, key)
        } else {
            let pinned = session
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            let pool = &profile.api_key_envs;
            let picked = pool.get(pool_index(pinned.as_deref(), pool.len())).cloned();
            let key = picked
                .as_deref()
                .and_then(|name| load_named_profile_api_key(name, &profile));
            (picked, key)
        };
        let key_label = key_env.unwrap_or_else(|| "inline api_key".to_string());
        Ok(match profile.auth {
            NamedProviderAuth::None => ProviderAuth::None {
                label: "local endpoint (no auth)".to_string(),
            },
            NamedProviderAuth::Bearer => ProviderAuth::AuthorizationBearer {
                token: key
                    .ok_or_else(|| anyhow::anyhow!("{} not found in environment", key_label))?,
                label: key_label,
            },
            NamedProviderAuth::Header => ProviderAuth::HeaderValue {
                header_name: HeaderName::from_bytes(
                    profile
                        .auth_header
                        .as_deref()
                        .unwrap_or("api-key")
                        .as_bytes(),
                )?,
                value: key
                    .ok_or_else(|| anyhow::anyhow!("{} not found in environment", key_label))?,
                label: key_label,
            },
        })
    })
}

#[cfg(test)]
#[path = "key_pool_tests.rs"]
mod tests;
