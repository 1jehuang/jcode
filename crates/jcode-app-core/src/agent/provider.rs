use super::*;

impl Agent {
    pub fn set_premium_mode(&self, mode: crate::provider::copilot::PremiumMode) {
        self.provider.set_premium_mode(mode);
    }

    pub fn premium_mode(&self) -> crate::provider::copilot::PremiumMode {
        self.provider.premium_mode()
    }

    pub fn provider_fork(&self) -> Arc<dyn Provider> {
        self.provider.fork()
    }

    pub fn provider_handle(&self) -> Arc<dyn Provider> {
        Arc::clone(&self.provider)
    }

    pub fn available_models(&self) -> Vec<&'static str> {
        self.provider.available_models()
    }

    pub fn available_models_for_switching(&self) -> Vec<String> {
        self.provider.available_models_for_switching()
    }

    pub fn available_models_display(&self) -> Vec<String> {
        self.provider.available_models_display()
    }

    pub fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        let mut routes = self.provider.model_routes();
        crate::model_usage::enrich_routes(&mut routes);
        routes
    }

    pub(super) fn begin_model_usage_turn(&mut self, message_id: &str) {
        self.session.model_usage_turn_id = Some(format!("{}:{}", self.session.id, message_id));
    }

    pub(super) fn model_usage_turn_id(&mut self) -> String {
        if let Some(id) = &self.session.model_usage_turn_id {
            return id.clone();
        }
        // Old sessions and direct loop callers have no durable anchor yet.
        // Internal reminders and tool-result rows do not start a logical turn.
        let message_id = self
            .session
            .visible_conversation_messages()
            .into_iter()
            .rev()
            .find(|message| {
                message.role == Role::User
                    && message.content.iter().any(|block| {
                        matches!(block, ContentBlock::Text { text, .. }
                    if !text.trim().is_empty() && !text.starts_with("[System reminder:"))
                            || matches!(block, ContentBlock::Image { .. })
                    })
            })
            .map(|message| message.id.clone())
            .unwrap_or_else(|| "initial".to_string());
        self.begin_model_usage_turn(&message_id);
        self.session.model_usage_turn_id.clone().unwrap()
    }

    pub(super) fn record_model_turn_usage(&self, turn_id: &str) {
        if self.session.is_debug {
            return;
        }
        let Some(mut route) = crate::model_usage::serving_route(
            self.provider.as_ref(),
            self.session.route_api_method.as_deref(),
        ) else {
            return;
        };
        match crate::model_usage::record_turn(turn_id, &route) {
            Ok(usage) => {
                route.usage = Some(usage);
                Bus::global().publish(BusEvent::ModelUsageUpdated(route));
            }
            Err(error) => logging::warn(&format!("Could not record model turn usage: {error}")),
        }
    }

    pub fn model_catalog_snapshot(&self) -> jcode_provider_core::ModelCatalogSnapshot {
        jcode_provider_core::ModelCatalogSnapshot::new(
            Some(self.provider_name()),
            Some(self.provider_model()),
            self.available_models_display(),
            self.model_routes(),
        )
    }

    pub fn registry(&self) -> Registry {
        self.registry.clone()
    }

    pub async fn compaction_mode(&self) -> crate::config::CompactionMode {
        self.registry.compaction().read().await.mode()
    }

    pub async fn set_compaction_mode(&self, mode: crate::config::CompactionMode) -> Result<()> {
        let compaction = self.registry.compaction();
        let mut manager = compaction.write().await;
        manager.set_mode(mode);
        Ok(())
    }

    fn refresh_compaction_budget(&self) {
        let compaction = self.registry.compaction();
        match compaction.try_write() {
            Ok(mut manager) => manager.set_budget(self.provider.context_window()),
            Err(_) => crate::logging::warn(
                "Could not refresh compaction token budget after provider change: compaction manager is busy",
            ),
        }
    }

    #[cfg(test)]
    pub(crate) async fn compaction_token_budget(&self) -> usize {
        self.registry.compaction().read().await.token_budget()
    }

    pub fn provider_messages(&mut self) -> Vec<Message> {
        self.session.messages_for_provider()
    }

    pub fn set_model(&mut self, model: &str) -> Result<()> {
        self.set_model_from_provider_state_event(
            model,
            crate::provider::ProviderModelSelectionSource::User,
        )
    }

    pub fn set_route_selection(
        &mut self,
        selection: &crate::provider::RouteSelection,
    ) -> Result<()> {
        self.set_route_selection_from_provider_state_event(
            selection,
            crate::provider::ProviderModelSelectionSource::User,
        )
    }

    pub(crate) fn set_route_selection_from_auth(
        &mut self,
        selection: &crate::provider::RouteSelection,
    ) -> Result<()> {
        self.set_route_selection_from_provider_state_event(
            selection,
            crate::provider::ProviderModelSelectionSource::Auth,
        )
    }

    fn set_route_selection_from_provider_state_event(
        &mut self,
        selection: &crate::provider::RouteSelection,
        source: crate::provider::ProviderModelSelectionSource,
    ) -> Result<()> {
        self.provider.set_route_selection(selection)?;
        let resolved_model = self.provider.model();
        self.session.provider_key = Some(selection.runtime_key.stable_id());
        self.session.route_api_method = Some(selection.api_method.clone());
        self.session.model = Some(self.provider_model());
        let event = crate::provider::ProviderStateEvent::selected_model(source, resolved_model);
        self.provider_runtime_state.apply(event);
        self.refresh_compaction_budget();
        self.persist_session_best_effort("route selection");
        self.log_env_snapshot("set_route_selection");
        Ok(())
    }

    pub(crate) fn set_model_from_auth(&mut self, model: &str) -> Result<()> {
        self.set_model_from_provider_state_event(
            model,
            crate::provider::ProviderModelSelectionSource::Auth,
        )
    }

    fn set_model_from_provider_state_event(
        &mut self,
        model: &str,
        source: crate::provider::ProviderModelSelectionSource,
    ) -> Result<()> {
        crate::provider::set_model_with_auth_refresh(self.provider.as_ref(), model)?;
        let resolved_model = self.provider.model();
        self.session.provider_key =
            crate::provider::MultiProvider::session_provider_key_after_model_switch(
                model,
                self.provider.name(),
                self.session.provider_key.as_deref(),
            );
        self.session.model = Some(self.provider_model());
        let event = crate::provider::ProviderStateEvent::selected_model(source, resolved_model);
        self.provider_runtime_state.apply(event);
        self.refresh_compaction_budget();
        self.persist_session_best_effort("model selection");
        self.log_env_snapshot("set_model");
        Ok(())
    }

    pub(crate) fn provider_model_selection_generation(&self) -> u64 {
        self.provider_runtime_state.selection_generation()
    }

    pub(crate) fn user_selected_provider_model_after(&self, generation: u64) -> bool {
        self.provider_runtime_state.user_selected_after(generation)
    }

    pub fn restore_reasoning_effort_from_session(&mut self) {
        if let Some(effort) = self.session.reasoning_effort.clone() {
            if let Err(e) = self.provider.set_reasoning_effort(&effort) {
                crate::logging::error(&format!(
                    "Failed to restore session reasoning effort '{}': {}",
                    effort, e
                ));
            }
        } else {
            self.session.reasoning_effort = self.provider.reasoning_effort();
        }
        // Mirror the effort into the deadlock-free side-table so server handlers
        // (e.g. the swarm seed handler) can learn this session's effort without
        // taking the agent lock.
        crate::session_effort::record_session_effort(
            &self.session.id,
            self.session.reasoning_effort.as_deref(),
        );
    }

    /// Push the session's per-provider account pins, failover toggle, and
    /// failover home onto the provider instance. Pins whose account no longer
    /// exists are dropped (the session falls back to the default account).
    /// Kinds without a pin are unpinned, since a restored session may reuse a
    /// provider instance that carried another session's pin.
    pub fn restore_account_pins_from_session(&mut self) {
        use crate::provider::AccountProviderKind;
        let mut dropped = Vec::new();
        for kind in AccountProviderKind::ALL {
            let key = kind.key();
            match self.session.account_pins.get(key).cloned() {
                Some(pin) => match crate::session_accounts::resolve_pin(kind, &pin) {
                    Some(label) => {
                        let pin = crate::provider::AccountPin::new(label, pin.identity.clone());
                        if let Err(e) = self.provider.set_account_pin(kind, Some(pin.clone())) {
                            logging::warn(&format!(
                                "Failed to restore {key} account pin '{}': {e}",
                                pin.label
                            ));
                        } else {
                            self.session.account_pins.insert(key.to_string(), pin);
                        }
                    }
                    None => {
                        let reason = crate::session_accounts::dropped_pin_reason(kind, &pin);
                        logging::warn(&format!(
                            "Session {} was pinned to {key} account '{}': {reason}",
                            self.session.id, pin.label
                        ));
                        // Never keep a pin whose label now names another
                        // login: unpin the provider too, so it cannot carry
                        // a stale pin from an earlier session.
                        if self.provider.account_pin(kind).is_some() {
                            let _ = self.provider.set_account_pin(kind, None);
                        }
                        self.pending_account_notices.push((kind, reason));
                        dropped.push(key.to_string());
                    }
                },
                None => {
                    if self.provider.account_pin(kind).is_some() {
                        let _ = self.provider.set_account_pin(kind, None);
                    }
                }
            }
            self.provider.set_account_failover_home(
                kind,
                self.session.account_failover_home.get(key).cloned(),
            );
        }
        for key in dropped {
            self.session.account_pins.remove(&key);
        }
        self.provider
            .set_account_failover(self.session.account_failover);
        self.record_observed_account_pins();
    }

    /// `SessionAccountChanged` events for pins that restore had to drop (the
    /// account was removed, or its label now names another login). Drained
    /// once, by whoever can reach the client first.
    pub fn take_account_notices(&mut self) -> Vec<ServerEvent> {
        std::mem::take(&mut self.pending_account_notices)
            .into_iter()
            .map(|(kind, what)| {
                let uses =
                    crate::session_accounts::current_use_suffix(self.provider.as_ref(), kind);
                crate::session_accounts::account_changed_event(
                    self.provider.as_ref(),
                    kind,
                    Some(format!("{what}{uses}")),
                )
            })
            .collect()
    }

    /// Remember what the provider reports now, so the next post-stream sync
    /// only reacts to moves the provider made on its own.
    fn record_observed_account_pins(&mut self) {
        for kind in crate::provider::AccountProviderKind::ALL {
            self.observed_account_pins
                .insert(kind, self.provider.account_pin(kind));
        }
    }

    /// Pin (or unpin with `None`) this session to one stored account. Only
    /// this session's provider instance changes; the stored default and other
    /// sessions are untouched. A user choice clears the failover home.
    pub fn set_account_pin(
        &mut self,
        kind: crate::provider::AccountProviderKind,
        pin: Option<crate::provider::AccountPin>,
    ) -> Result<()> {
        crate::session_accounts::apply_manual_pin(
            self.provider.as_ref(),
            &mut self.session,
            kind,
            pin,
        )?;
        self.record_observed_account_pins();
        self.log_env_snapshot("set_account_pin");
        self.session.save()?;
        Ok(())
    }

    /// Per-session same-provider account failover toggle (`None` = config).
    pub fn set_account_failover(&mut self, enabled: Option<bool>) -> Result<()> {
        self.provider.set_account_failover(enabled);
        self.session.account_failover = enabled;
        self.log_env_snapshot("set_account_failover");
        self.session.save()?;
        Ok(())
    }

    pub fn account_pins(&self) -> &std::collections::BTreeMap<String, crate::provider::AccountPin> {
        &self.session.account_pins
    }

    pub fn account_failover(&self) -> Option<bool> {
        self.session.account_failover
    }

    /// Account state a child session inherits from this one.
    pub fn account_inheritance(&self) -> crate::session_accounts::AccountInheritance {
        crate::session_accounts::AccountInheritance::from_session(&self.session)
    }

    /// Adopt a parent's account pins and failover toggle (swarm workers).
    pub fn apply_account_inheritance(
        &mut self,
        inheritance: &crate::session_accounts::AccountInheritance,
    ) {
        if inheritance.is_empty() {
            return;
        }
        inheritance.apply_to_session(&mut self.session);
        self.restore_account_pins_from_session();
        self.persist_session_best_effort("inherited account pins");
    }

    /// Persist account moves the provider made on its own (same-provider
    /// failover) and tell clients. Runs after every stream, on success and on
    /// every error exit, because a failed turn can still leave the provider on
    /// another account.
    pub(super) fn sync_account_pins_after_stream(
        &mut self,
        event_tx: Option<&mpsc::UnboundedSender<ServerEvent>>,
    ) {
        let (changed, events) = crate::session_accounts::sync_pins_after_stream(
            self.provider.as_ref(),
            &mut self.session,
            &mut self.observed_account_pins,
        );
        if let Some(event_tx) = event_tx {
            for event in events {
                let _ = event_tx.send(event);
            }
        }
        if changed {
            self.persist_session_best_effort("account failover");
        }
    }

    /// At turn start, return to the preferred account once its usage limit
    /// has reset (the provider decides, locally). Never runs mid-turn.
    pub(super) fn return_account_home_at_turn_start(
        &mut self,
        event_tx: Option<&mpsc::UnboundedSender<ServerEvent>>,
    ) {
        let events = crate::session_accounts::return_home_if_reset(
            self.provider.as_ref(),
            &mut self.session,
            &mut self.observed_account_pins,
        );
        if events.is_empty() {
            return;
        }
        if let Some(event_tx) = event_tx {
            for event in events {
                let _ = event_tx.send(event);
            }
        }
        self.persist_session_best_effort("account return home");
    }

    pub fn set_reasoning_effort(&mut self, effort: &str) -> Result<Option<String>> {
        self.provider.set_reasoning_effort(effort)?;
        let current = self.provider.reasoning_effort();
        self.session.reasoning_effort = current.clone();
        // Keep the side-table in sync (see `restore_reasoning_effort_from_session`).
        crate::session_effort::record_session_effort(&self.session.id, current.as_deref());
        self.log_env_snapshot("set_reasoning_effort");
        self.session.save()?;
        Ok(current)
    }

    pub fn subagent_model(&self) -> Option<String> {
        self.session.subagent_model.clone()
    }

    pub fn set_subagent_model(&mut self, model: Option<String>) -> Result<()> {
        self.session.subagent_model = model;
        self.log_env_snapshot("set_subagent_model");
        self.session.save()?;
        Ok(())
    }

    pub fn session_provider_key(&self) -> Option<String> {
        self.session.provider_key.clone()
    }

    /// API method/runtime route used to select the active model (e.g.
    /// "openai-api", "claude-oauth", "openai-compatible:nvidia-nim"). Spawned
    /// swarm agents inherit this so they reconstruct the coordinator's exact
    /// auth route instead of falling back to the config default.
    pub fn session_route_api_method(&self) -> Option<String> {
        self.session.route_api_method.clone()
    }

    /// The credential the active provider will use for the next request, when
    /// the provider distinguishes OAuth (subscription) from API key (cost).
    /// Resolved authoritatively here so remote clients can render billing/usage
    /// without re-deriving it from the provider name.
    pub fn active_resolved_credential(&self) -> Option<jcode_provider_core::ResolvedCredential> {
        self.provider.active_resolved_credential()
    }

    pub fn set_session_provider_key(&mut self, provider_key: Option<String>) {
        self.session.provider_key = provider_key;
    }

    /// Bookmark or unbookmark the session, returning the effective label.
    pub fn set_session_saved(
        &mut self,
        saved: bool,
        label: Option<String>,
    ) -> Result<Option<String>> {
        if saved {
            self.session.mark_saved(label);
        } else {
            self.session.unmark_saved();
        }
        self.session.save()?;
        Ok(self.session.save_label.clone())
    }

    pub fn rename_session_title(&mut self, title: Option<String>) -> Result<String> {
        self.session.rename_title(title);
        self.log_env_snapshot("rename_session");
        self.session.save()?;
        Ok(self.session.display_title_or_name().to_string())
    }

    pub fn session_display_title_or_name(&self) -> String {
        self.session.display_title_or_name().to_string()
    }

    pub fn autoreview_enabled(&self) -> Option<bool> {
        self.session.autoreview_enabled
    }

    pub fn set_autoreview_enabled(&mut self, enabled: bool) -> Result<()> {
        self.session.autoreview_enabled = Some(enabled);
        self.log_env_snapshot("set_autoreview_enabled");
        self.session.save()?;
        Ok(())
    }

    pub fn autojudge_enabled(&self) -> Option<bool> {
        self.session.autojudge_enabled
    }

    pub fn set_autojudge_enabled(&mut self, enabled: bool) -> Result<()> {
        self.session.autojudge_enabled = Some(enabled);
        self.log_env_snapshot("set_autojudge_enabled");
        self.session.save()?;
        Ok(())
    }

    /// Set the working directory for this session
    pub fn set_working_dir(&mut self, dir: &str) {
        if self.session.working_dir.as_deref() == Some(dir) {
            return;
        }
        self.session.working_dir = Some(dir.to_string());
        self.refresh_agents_md_snapshot();
        self.session.refresh_initial_session_context_message();
        self.log_env_snapshot("working_dir");
    }

    /// Get the working directory for this session
    pub fn working_dir(&self) -> Option<&str> {
        self.session.working_dir.as_deref()
    }

    /// Get the stored messages (for transcript export)
    pub fn messages(&self) -> &[StoredMessage] {
        &self.session.messages
    }
}
