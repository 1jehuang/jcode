use super::*;

impl App {
    /// Default auto-retry attempt budget when no per-provider or global
    /// override is configured. The effective budget is resolved at call time
    /// by `effective_auto_retry_max_attempts`; this is the fallback baseline
    /// (and the cap used by the automatic-failover target limiter).
    pub(crate) const AUTO_RETRY_MAX_ATTEMPTS: u8 = 3;

    /// Resolve the config key of the provider profile whose auto-retry
    /// overrides apply to the active turn.
    ///
    /// Local mode reads `session.provider_key`, set at session start and kept
    /// current by the local model-switch paths. Remote mode cannot: a
    /// server-side `/model` switch updates `remote_provider_name` but never
    /// `session.provider_key`, so that key keeps naming the launch-time
    /// provider (greptile 4307529097). When the server has reported a provider
    /// name, derive the profile key from it instead: a catalog profile display
    /// name (e.g. "NVIDIA NIM" -> `nvidia-nim`) or a named `[providers.<id>]`
    /// key resolves directly, and a canonical provider name (e.g. "Claude" ->
    /// `claude`) through the session-key vocabulary.
    ///
    /// The canonical map is inlined rather than reusing
    /// `derive_session_provider_key`, because that function consults the
    /// client process's provider env vars first (`JCODE_RUNTIME_PROVIDER`,
    /// `JCODE_OPENROUTER_CACHE_NAMESPACE`, ...). In remote mode those describe
    /// the client's local provider stack, not the server-side session, so they
    /// would poison the mapping (a client that last used DeepSeek locally would
    /// resolve a remote "Claude" to `deepseek`).
    pub(crate) fn auto_retry_provider_profile_key(&self) -> Option<String> {
        let remote_name = self
            .is_remote
            .then_some(self.remote_provider_name.as_deref())
            .flatten()
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let Some(remote_name) = remote_name else {
            return self.session.provider_key.clone();
        };
        // Matches by display name ("NVIDIA NIM") and by profile id
        // (`nvidia-nim`, the shape `remote_header_hint` stores).
        if let Some(profile_id) =
            crate::provider_catalog::openai_compatible_profile_id_for_display_name(remote_name)
        {
            return Some(profile_id.to_string());
        }
        // A named `[providers.<id>]` section id reported verbatim. Checked
        // before the canonical map so a deliberately configured id wins over
        // inference; the map's verbatim arm below still catches lowercase ids.
        if crate::config::config().providers.contains_key(remote_name) {
            return Some(remote_name.to_string());
        }
        let normalized = remote_name.to_ascii_lowercase();
        let key = match normalized.as_str() {
            "jcode" | "jcode subscription" => "jcode",
            "anthropic" | "claude" | "claude cli" => "claude",
            "openai" => "openai",
            // The Azure slot reports its display name, while the local session
            // key is the profile id.
            "azure openai" => "azure-openai",
            "github copilot" | "copilot" => "copilot",
            "openrouter" => "openrouter",
            "cursor" => "cursor",
            "gemini" | "google" => "gemini",
            "antigravity" => "antigravity",
            "bedrock" | "aws bedrock" => "bedrock",
            // Unknown label ("OpenAI-compatible", ...): keep the lowercased
            // label as the key so a matching `[providers.<id>]` section (ids
            // are lowercase) still applies; the callers' profile lookup
            // decides the rest.
            other => other,
        };
        Some(key.to_string())
    }

    /// Resolve the effective auto-retry base delay for the active provider.
    /// Uses the named provider profile override, then the env-resolved global
    /// `[provider]` value. Reading the live config (instead of a value snapshotted
    /// at construction) means a config.toml edit or env change applies to the
    /// next retry without an app restart, matching how the per-profile overrides
    /// already behave.
    pub(crate) fn effective_auto_retry_base_delay_secs(&self) -> u64 {
        if let Some(key) = self.auto_retry_provider_profile_key()
            && let Some(profile) = crate::config::config().providers.get(&key)
            && let Some(override_val) = profile.auto_retry_base_delay_secs
        {
            return jcode_config_types::clamp_auto_retry_base_delay_secs(override_val);
        }
        jcode_config_types::clamp_auto_retry_base_delay_secs(
            crate::config::config().provider.auto_retry_base_delay_secs,
        )
    }

    /// Resolve whether auto-retry is enabled for the active provider.
    /// Uses the named provider profile override, then the env-resolved global
    /// `[provider]` value. Live config read: config.toml edits and env changes
    /// apply to the running session without a restart.
    pub(crate) fn effective_auto_retry_enabled(&self) -> bool {
        if let Some(key) = self.auto_retry_provider_profile_key()
            && let Some(profile) = crate::config::config().providers.get(&key)
            && let Some(override_val) = profile.auto_retry_enabled
        {
            return override_val;
        }
        crate::config::config().provider.auto_retry_enabled
    }

    /// Resolve the effective auto-retry max attempts for the active provider.
    /// Uses the named provider profile override, then the env-resolved global
    /// `[provider]` value. Live config read: config.toml edits and env changes
    /// apply to the running session without a restart.
    pub(crate) fn effective_auto_retry_max_attempts(&self) -> u8 {
        if let Some(key) = self.auto_retry_provider_profile_key()
            && let Some(profile) = crate::config::config().providers.get(&key)
            && let Some(override_val) = profile.auto_retry_max_attempts
        {
            return override_val;
        }
        crate::config::config().provider.auto_retry_max_attempts
    }

    pub(super) fn schedule_pending_remote_retry(&mut self, reason: &str) -> bool {
        if !self.effective_auto_retry_enabled() {
            return false;
        }
        self.schedule_pending_remote_retry_with_limit(
            reason,
            self.effective_auto_retry_max_attempts(),
        )
    }

    pub(super) fn schedule_pending_remote_network_wait(&mut self, reason: &str) -> bool {
        self.schedule_pending_remote_network_wait_with_force(reason, false)
    }

    /// Hold the in-flight remote turn until the network recovers, then resume it.
    ///
    /// Connectivity failures (DNS, connection reset, no route, transient TLS,
    /// timeouts) usually recover: the request never reached the provider, so
    /// resending is safe. When `force` is set (a real provider attempt just
    /// failed) we wait regardless of the pending message's `auto_retry` flag
    /// and promote it to auto-retry so the tick-based resume re-sends it.
    ///
    /// A provider-specific failure (for example DNS for one host) can persist
    /// while the generic connectivity probe succeeds, so each forced wait
    /// consumes one attempt from the per-turn retry budget and returns `false`
    /// once it is spent. Offline probe re-waits (`force == false`) do not
    /// consume attempts because no request was sent.
    pub(super) fn schedule_pending_remote_network_wait_with_force(
        &mut self,
        reason: &str,
        force: bool,
    ) -> bool {
        if !self.effective_auto_retry_enabled() {
            return false;
        }
        let max_attempts = self.effective_auto_retry_max_attempts();
        // A forced wait follows a real failed provider attempt, so it honors
        // the configured base delay like any other retry. Offline probe
        // re-waits (force == false) keep their short fixed cadence.
        let base_delay_secs = if force {
            self.effective_auto_retry_base_delay_secs()
        } else {
            5
        };
        let Some(pending) = self.rate_limit_pending_message.as_mut() else {
            self.log_recovery_event(
                crate::logging::LogLevel::Warn,
                "RECOVERY_NETWORK_WAIT_SKIPPED",
                "no_pending_turn",
            );
            return false;
        };
        if force {
            if pending.retry_attempts >= max_attempts {
                self.log_recovery_event(
                    crate::logging::LogLevel::Warn,
                    "RECOVERY_NETWORK_WAIT_STOPPED",
                    "retry_budget_exhausted",
                );
                return false;
            }
            pending.retry_attempts = pending.retry_attempts.saturating_add(1);
        }
        if !pending.auto_retry {
            if force {
                pending.auto_retry = true;
            } else {
                self.log_recovery_event(
                    crate::logging::LogLevel::Info,
                    "RECOVERY_NETWORK_WAIT_SKIPPED",
                    "auto_retry_disabled",
                );
                return false;
            }
        }

        let plan = crate::network_retry::wait_plan();
        let retry_at = Instant::now()
            + Duration::from_secs(
                base_delay_secs
                    .saturating_mul(u64::from(pending.retry_attempts.max(1)))
                    .min(jcode_config_types::MAX_AUTO_RETRY_BACKOFF_SECS),
            );
        pending.retry_at = Some(retry_at);
        self.rate_limit_reset = Some(retry_at);
        self.status = ProcessingStatus::WaitingForNetwork {
            listener: plan.listener_summary.clone(),
        };
        self.status_detail = Some("offline; waiting for network before retry".to_string());
        self.log_recovery_event(
            crate::logging::LogLevel::Info,
            "RECOVERY_NETWORK_WAIT_ARMED",
            if force {
                "provider_network_failure"
            } else {
                "offline_probe"
            },
        );

        let content = format!(
            "📡 Network appears offline - waiting to retry automatically. {} - {}",
            plan.listener_summary,
            reason.trim().trim_end_matches('.')
        );
        if let Some(idx) = self.display_messages.iter().rposition(|message| {
            message.role == "system"
                && (message.title.as_deref() == Some("Connection")
                    || message.content.starts_with("📡 Network appears offline"))
        }) {
            self.replace_display_message_title_and_content(
                idx,
                Some("Connection".to_string()),
                content,
            );
        } else {
            self.push_display_message(DisplayMessage {
                role: "system".to_string(),
                content,
                tool_calls: Vec::new(),
                duration_secs: None,
                title: Some("Connection".to_string()),
                tool_data: None,
            });
        }
        true
    }
}

/// Auto-retry was disabled after a turn was already held for resend.
/// The queued turn will never resend; put the prompt back in the input box
/// instead of dropping it. `restore_failed_input_to_box` keeps any draft the
/// user started meanwhile.
pub(super) fn drop_pending_remote_retry(app: &mut App) -> bool {
    app.rate_limit_pending_message = None;
    if matches!(app.status, ProcessingStatus::WaitingForNetwork { .. }) {
        app.status = ProcessingStatus::Idle;
        app.status_detail = None;
    }
    app.restore_failed_input_to_box();
    app.push_display_message(DisplayMessage::system(
        "Auto-retry disabled; not resending pending remote message.".to_string(),
    ));
    true
}
