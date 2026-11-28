use super::*;

/// Debounce window for the pending config.toml write.
pub(super) const MODEL_CONFIG_PERSIST_DEBOUNCE: Duration = Duration::from_millis(500);

/// A user-initiated model switch waiting for its debounced config write.
#[derive(Debug, Clone)]
pub(super) struct PendingModelConfigPersist {
    pub(super) model_spec: String,
    pub(super) provider_key: Option<String>,
    pub(super) due_at: Instant,
}

impl App {
    /// Persist a user-initiated model switch to `config.toml` so it survives
    /// across jcode relaunches and new sessions.
    ///
    /// `finalize_model_switch` only saves the model to the *session* file, so a
    /// resumed session restores it but a **new** session or relaunch reads
    /// `config.toml`'s `[provider].default_model` and reverts to the old
    /// default. This writes the active model + provider key back to config so
    /// the choice is sticky globally. It must only be called for
    /// user-initiated switches (cycle_model, `/model <name>`, model picker),
    /// never for failover- or auth-driven switches which should not override
    /// the user's configured default.
    ///
    /// `model_spec` is the user-facing route spec the user chose (e.g.
    /// "llmg-coding:my-model", "copilot:gpt-5", or a bare model id). It already
    /// encodes the routing the user picked. `provider_key` is the resolved
    /// session provider key; when `None`, the caller wants to use the session's
    /// current `provider_key` (the common case for cycle/slash which already
    /// finalized the switch and updated `session.provider_key`). When `Some`,
    /// the caller passes the key explicitly (the model picker computes it from
    /// the route before the switch is applied).
    pub(super) fn persist_model_switch_to_config(
        &mut self,
        model_spec: &str,
        provider_key: Option<&str>,
    ) {
        let provider_key = match provider_key {
            Some(key) => Some(key.to_string()),
            None => self.session.provider_key.clone(),
        };

        let cfg = crate::config::config();
        if !cfg.provider.persist_model_switch {
            self.pending_model_config_persist = None;
            crate::logging::info(&format!(
                "Skipped persisting model switch because provider.persist_model_switch=false: {} via {}",
                model_spec,
                provider_key.as_deref().unwrap_or("auto")
            ));
            return;
        }

        // No pre-compare against the loaded config here: `config()` applies
        // env overrides (JCODE_MODEL), so comparing against it makes a switch
        // back to the env-overridden model look unchanged and skip a real
        // write. `set_default_model_if_changed` compares file-only defaults at
        // flush time, which is the comparison that actually matters.

        if self
            .pending_model_config_persist
            .as_ref()
            .is_some_and(|pending| {
                pending.model_spec == model_spec && pending.provider_key == provider_key
            })
        {
            crate::logging::info(&format!(
                "Model switch config persist already pending: {} via {}",
                model_spec,
                provider_key.as_deref().unwrap_or("auto")
            ));
            return;
        }

        self.pending_model_config_persist = Some(PendingModelConfigPersist {
            model_spec: model_spec.to_string(),
            provider_key,
            due_at: Instant::now() + MODEL_CONFIG_PERSIST_DEBOUNCE,
        });
        crate::logging::info(&format!(
            "Scheduled debounced model switch persist to config.toml: {}",
            model_spec
        ));
    }

    pub(super) fn maybe_flush_pending_model_config_persist(&mut self) -> bool {
        let Some(pending) = self.pending_model_config_persist.as_ref() else {
            return false;
        };
        if Instant::now() < pending.due_at {
            return false;
        }
        let Some(pending) = self.pending_model_config_persist.take() else {
            return false;
        };
        self.write_pending_model_config_default(pending)
    }

    /// Flush any pending model-switch persist immediately, ignoring the
    /// debounce deadline. Called on orderly shutdown so a switch made right
    /// before exiting is not silently lost.
    pub(super) fn force_flush_pending_model_config_persist(&mut self) {
        let Some(pending) = self.pending_model_config_persist.take() else {
            return;
        };
        self.write_pending_model_config_default(pending);
    }

    fn write_pending_model_config_default(
        &mut self,
        pending: super::PendingModelConfigPersist,
    ) -> bool {
        match crate::config::Config::set_default_model_if_changed(
            Some(&pending.model_spec),
            pending.provider_key.as_deref(),
        ) {
            Ok(true) => {
                self.set_status_notice("Default model saved");
                crate::logging::info(&format!(
                    "Persisted model switch to config.toml: {} via {} (survives relaunch)",
                    pending.model_spec,
                    pending.provider_key.as_deref().unwrap_or("auto")
                ));
                true
            }
            Ok(false) => {
                crate::logging::info(&format!(
                    "Skipped unchanged model switch persist at flush: {} via {}",
                    pending.model_spec,
                    pending.provider_key.as_deref().unwrap_or("auto")
                ));
                false
            }
            Err(e) => {
                crate::logging::warn(&format!(
                    "Failed to persist model switch to config.toml (session still saved): {}",
                    e
                ));
                false
            }
        }
    }
}
