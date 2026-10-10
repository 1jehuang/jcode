//! First-frame header hints for remote clients.
//!
//! A remote client paints its first frame long before the server's History
//! event arrives. Header rows that only appear once History lands (the `mcp:`
//! line, the self-dev badge) used to pop in ~100ms later and shove the whole
//! layout down a row or two. We seed them from facts the client already knows
//! (it will request self-dev) or saw last time it talked to this server (its
//! MCP inventory), and History overwrites them with the authoritative values.
//!
//! The same applies to the session facts on the overscroll status line: the
//! context window, provider label, billing credential, and reasoning effort
//! all come from the server. Without a hint the first frames show generic
//! placeholders (a 200k window, the raw `oauth:claude` provider key, no
//! effort) that visibly snap to the real values ~150ms later.

use super::App;
use serde::{Deserialize, Serialize};

const HINT_FILE: &str = "remote_header_hint.json";

#[derive(Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
struct RemoteHeaderHint {
    /// Socket path of the server these facts came from.
    origin: String,
    #[serde(default)]
    mcp_servers: Vec<(String, usize)>,
    /// Last session facts the server reported. Only applied when they describe
    /// the model this launch will actually run (see `apply_session_facts`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<SessionFactsHint>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
struct SessionFactsHint {
    provider_name: Option<String>,
    provider_model: String,
    context_limit: u64,
    reasoning_effort: Option<String>,
    resolved_credential: Option<jcode_provider_core::ResolvedCredential>,
}

fn hint_path() -> Option<std::path::PathBuf> {
    crate::storage::jcode_dir()
        .ok()
        .map(|dir| dir.join(HINT_FILE))
}

fn origin() -> String {
    crate::server::socket_path().to_string_lossy().into_owned()
}

fn read_hint() -> Option<RemoteHeaderHint> {
    // Unit tests share one JCODE_HOME across the whole binary; a hint written
    // by one test's History would leak into every later `new_for_remote`.
    if cfg!(test) {
        return None;
    }
    let path = hint_path()?;
    let hint = crate::storage::read_json::<RemoteHeaderHint>(&path).ok()?;
    (hint.origin == origin()).then_some(hint)
}

/// Bare model id with any `provider:` / `route:` prefix removed, lowercased.
fn bare_model(model: &str) -> String {
    model
        .rsplit(':')
        .next()
        .unwrap_or(model)
        .trim()
        .to_ascii_lowercase()
}

impl App {
    /// Seed header state before the first frame so it matches the layout the
    /// History event will settle on.
    pub(super) fn apply_remote_header_hint(&mut self) {
        if crate::tui::is_ssh_remote() {
            return;
        }
        // The subscribe request carries `selfdev` exactly when this resolves
        // true, and the server marks such sessions canary.
        if self.remote_is_canary.is_none() && crate::tui::subscribe_metadata(None).1 == Some(true) {
            self.remote_is_canary = Some(true);
        }
        let Some(hint) = read_hint() else {
            self.apply_configured_route_facts();
            return;
        };
        if self.mcp_server_names.is_empty() {
            self.mcp_server_names = hint.mcp_servers;
        }
        if let Some(session) = hint.session {
            self.apply_session_facts_hint(session);
        }
        if !self.remote_session_facts_provisional {
            self.apply_configured_route_facts();
        }
    }

    /// No usable hint: derive what we can from the route the launch is
    /// configured to use (`claude-oauth:claude-opus-5-5`). Without this the
    /// raw route id leaked into the status line (`Oauth:claude Opus 5.5`) next
    /// to a generic 200k window until History arrived. The provider, the
    /// explicitly pinned credential, and the static context window for a
    /// known model are all determined by the route itself. Effort is left
    /// unknown: only the server knows the session's effort.
    pub(super) fn apply_configured_route_facts(&mut self) {
        if self.remote_provider_model.is_some() {
            return;
        }
        let Some(model) = self.effective_remote_provider_model() else {
            return;
        };
        // Route-qualified id (`claude-oauth:claude-opus-5-5`, the config
        // default) or a bare one (`claude-opus-5-5`, what a resumed session
        // stores). For a bare id the session's provider key, then the
        // configured default provider, supply the route.
        let configured_provider = self
            .session
            .provider_key
            .clone()
            .or_else(|| self.configured_remote_provider_hint());
        let qualified;
        let model = if jcode_provider_core::selection::explicit_model_provider_prefix(model.trim())
            .is_some()
        {
            model.trim()
        } else {
            let Some(provider) = configured_provider.as_deref() else {
                return;
            };
            qualified = format!("{}:{}", provider.trim(), model.trim());
            qualified.as_str()
        };
        let Some((provider, prefix, bare)) =
            jcode_provider_core::selection::explicit_model_provider_prefix(model)
        else {
            return;
        };
        let bare = bare.trim();
        if bare.is_empty() {
            return;
        }
        let provider_key = jcode_provider_core::selection::provider_key(provider);
        let Some(context_limit) =
            crate::provider::context_limit_for_model_with_provider(bare, Some(provider_key))
        else {
            return;
        };
        self.set_context_limit_and_sync_budget(context_limit);
        self.remote_provider_name = Some(provider_key.to_string());
        self.remote_provider_model = Some(bare.to_string());
        self.remote_resolved_credential =
            jcode_provider_core::AuthRoute::parse_explicit_credential_prefix(prefix)
                .map(|route| route.resolved_credential());
        // A brand-new session starts at the configured per-family effort, so
        // that is known too. A resumed session keeps whatever it last used,
        // which only the server knows.
        if self.resume_session_id.is_none() {
            self.remote_reasoning_effort = self.remote_reasoning_effort_hint();
        }
        self.remote_session_facts_provisional = true;
    }

    /// Apply remembered session facts, but only when they describe the model
    /// this launch is expected to run: the resumed session's model, or else the
    /// configured default. A hint for a different model would show the wrong
    /// window and effort until History corrected it, which is worse than the
    /// generic placeholders.
    fn apply_session_facts_hint(&mut self, facts: SessionFactsHint) {
        if self.remote_provider_model.is_some() {
            return;
        }
        // `effective_remote_provider_model` prefers a resumed session's model,
        // then `JCODE_MODEL` / the configured default, and ignores the inert
        // remote provider's `unknown` placeholder that new sessions start with.
        // With no expectation at all there is nothing to check the hint
        // against (the server may pick anything), so keep the honest
        // "connecting" placeholders instead of guessing.
        let Some(expected) = self.effective_remote_provider_model() else {
            return;
        };
        if bare_model(&expected) != bare_model(&facts.provider_model) {
            return;
        }
        if facts.context_limit > 0 {
            self.set_context_limit_and_sync_budget(facts.context_limit as usize);
        }
        self.remote_provider_name = facts.provider_name;
        self.remote_provider_model = Some(facts.provider_model);
        self.remote_reasoning_effort = facts.reasoning_effort;
        self.remote_resolved_credential = facts.resolved_credential;
        self.remote_session_facts_provisional = true;
    }

    /// Remember the header facts History just delivered, for the next launch.
    /// Writes only when something changed, so steady-state History and
    /// ModelChanged traffic does not touch the disk.
    pub(super) fn persist_remote_header_hint(&self) {
        if cfg!(test) || crate::tui::is_ssh_remote() {
            return;
        }
        let Some(path) = hint_path() else {
            return;
        };
        let session = self
            .remote_provider_model
            .clone()
            .filter(|m| !m.trim().is_empty())
            .map(|provider_model| SessionFactsHint {
                provider_name: self.remote_provider_name.clone(),
                provider_model,
                context_limit: self.context_limit,
                reasoning_effort: self.remote_reasoning_effort.clone(),
                resolved_credential: self.remote_resolved_credential,
            });
        let previous = read_hint();
        let hint = RemoteHeaderHint {
            origin: origin(),
            mcp_servers: if self.mcp_server_names.is_empty() {
                previous
                    .as_ref()
                    .map(|p| p.mcp_servers.clone())
                    .unwrap_or_default()
            } else {
                self.mcp_server_names.clone()
            },
            session: session.or_else(|| previous.as_ref().and_then(|p| p.session.clone())),
        };
        if previous.as_ref() == Some(&hint) {
            return;
        }
        if let Err(error) = crate::storage::write_json_fast(&path, &hint) {
            crate::logging::warn(&format!("Failed to persist remote header hint: {error}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_model_ignores_route_prefix_and_case() {
        assert_eq!(
            bare_model("claude-oauth:claude-opus-5-5"),
            "claude-opus-5-5"
        );
        assert_eq!(bare_model("Claude-Opus-5-5"), "claude-opus-5-5");
        assert_ne!(bare_model("gpt-5.5"), bare_model("claude-opus-5-5"));
    }

    #[test]
    fn legacy_hint_without_session_facts_still_parses() {
        let hint: RemoteHeaderHint =
            serde_json::from_str(r#"{"origin":"/s.sock","mcp_servers":[["gh",3]]}"#).unwrap();
        assert_eq!(hint.mcp_servers, vec![("gh".to_string(), 3)]);
        assert!(hint.session.is_none());
    }

    #[test]
    fn session_facts_round_trip() {
        let hint = RemoteHeaderHint {
            origin: "/s.sock".into(),
            mcp_servers: vec![],
            session: Some(SessionFactsHint {
                provider_name: Some("claude".into()),
                provider_model: "claude-opus-5-5".into(),
                context_limit: 1_000_000,
                reasoning_effort: Some("medium".into()),
                resolved_credential: Some(jcode_provider_core::ResolvedCredential::Oauth),
            }),
        };
        let text = serde_json::to_string(&hint).unwrap();
        let back: RemoteHeaderHint = serde_json::from_str(&text).unwrap();
        assert_eq!(back, hint);
    }
}
