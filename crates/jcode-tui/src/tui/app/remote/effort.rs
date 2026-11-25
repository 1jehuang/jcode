use super::*;
use crate::tui::app as app_mod;

#[derive(Default)]
pub(in crate::tui::app) struct RemoteEffortState {
    confirmed_id: u64,
    pub(in crate::tui::app) requested: Option<(u64, String)>,
}

pub(super) async fn request_remote_effort(
    app: &mut App,
    remote: &mut RemoteConnection,
    effort: &str,
) -> Result<()> {
    let id = match remote.set_reasoning_effort(effort).await {
        Ok(id) => id,
        Err(error) => {
            app.set_status_notice("Effort switch failed");
            return Err(error);
        }
    };
    app.remote_effort_state.requested = Some((id, effort.to_string()));
    app.suppress_next_reasoning_effort_message = false;
    app.invalidate_model_picker_cache();
    app.set_status_notice(format!(
        "Effort: {} requested (waiting for server)",
        app_mod::effort_display_label(effort)
    ));
    Ok(())
}

impl App {
    pub(in crate::tui::app) fn observe_remote_effort(
        &mut self,
        effort: Option<String>,
        id: Option<u64>,
    ) {
        let state = &mut self.remote_effort_state;
        if let Some(id) = id {
            state.confirmed_id = state.confirmed_id.max(id);
        }
        if state
            .requested
            .as_ref()
            .is_some_and(|(pending, _)| id.is_some_and(|id| *pending <= id))
        {
            state.requested = None;
            self.suppress_next_reasoning_effort_message = false;
        }
        self.remote_reasoning_effort = effort;
        self.invalidate_model_picker_cache();
    }

    pub(in crate::tui::app) fn reset_remote_effort_for_session(&mut self) {
        if let Some((id, _)) = self.remote_effort_state.requested.take() {
            self.remote_effort_state.confirmed_id = self.remote_effort_state.confirmed_id.max(id);
        }
        self.suppress_next_reasoning_effort_message = false;
        self.invalidate_model_picker_cache();
    }

    /// Best-known current reasoning effort for the remote session. Falls back
    /// to the configured provider-family default when the server has not
    /// reported one yet, so pre-settle effort cycling starts from the value the
    /// session will actually use instead of assuming the maximum.
    pub(in crate::tui::app) fn remote_reasoning_effort_hint(&self) -> Option<String> {
        if let Some((_, effort)) = &self.remote_effort_state.requested {
            return Some(effort.clone());
        }
        if crate::tui::is_ssh_remote() {
            return self.remote_reasoning_effort.clone().or_else(|| {
                let (provider, model) = self.remote_effort_identity();
                if provider
                    .as_deref()
                    .is_some_and(|p| p.to_ascii_lowercase().contains("antigravity"))
                {
                    model.as_deref().and_then(|m| {
                        jcode_provider_core::split_antigravity_model_effort(m)
                            .map(|(_, effort)| effort.to_string())
                    })
                } else {
                    None
                }
            });
        }
        self.remote_reasoning_effort.clone().or_else(|| {
            let (provider, model) = self.remote_effort_identity();
            let provider = provider.as_deref().unwrap_or("").to_ascii_lowercase();
            let model = model.as_deref().unwrap_or("").to_ascii_lowercase();
            let cfg = &crate::config::config().provider;
            if provider.contains("antigravity") {
                jcode_provider_core::split_antigravity_model_effort(&model)
                    .map(|(_, effort)| effort.to_string())
            } else if provider.contains("anthropic")
                || provider.contains("claude")
                || model.starts_with("claude-")
            {
                cfg.anthropic_reasoning_effort.clone()
            } else if provider.contains("openai")
                || provider.contains("codex")
                || model.starts_with("gpt-")
            {
                cfg.openai_reasoning_effort.clone()
            } else {
                None
            }
        })
    }
}

pub(super) fn handle_effort_changed(
    app: &mut App,
    id: u64,
    effort: Option<String>,
    model: Option<String>,
    error: Option<String>,
) -> bool {
    let state = &mut app.remote_effort_state;
    // ID zero is a server-initiated change, not a request acknowledgement.
    if id != 0 {
        if id <= state.confirmed_id {
            return false;
        }
        let latest = state.requested.as_ref().map(|(id, _)| *id);
        if error.is_some() && latest.is_some_and(|latest| id != latest) {
            return false;
        }
        if latest.is_some_and(|latest| id >= latest) {
            state.requested = None;
            if error.is_some() {
                app.suppress_next_reasoning_effort_message = false;
            }
        }
        if error.is_none() {
            state.confirmed_id = id;
        }
    }
    app.invalidate_model_picker_cache();
    if let Some(err) = error {
        app.push_display_message(DisplayMessage::error(format!(
            "Failed to set effort: {}",
            err
        )));
        app.set_status_notice("Effort switch failed");
        true
    } else {
        app.remote_reasoning_effort = effort.clone();
        let model_changed = if let Some(new_model) = model {
            let changed = app.remote_provider_model.as_deref() != Some(new_model.as_str());
            app.remote_provider_model = Some(new_model.clone());
            app.session.model = Some(new_model);
            changed
        } else {
            false
        };
        app.persist_remote_header_hint();
        if model_changed {
            crate::tui::ui::prepare::invalidate_header_prep_cache();
        }
        if app.remote_effort_state.requested.is_some() {
            return true;
        }
        let label = effort
            .as_deref()
            .map(app_mod::effort_display_label)
            .unwrap_or("default");
        if !std::mem::take(&mut app.suppress_next_reasoning_effort_message) {
            app.push_display_message(DisplayMessage::system(format!(
                "✓ Reasoning effort → {}",
                label
            )));
        }
        app.set_status_notice(format!("Effort: {}", label));
        true
    }
}

/// Forward the reasoning-effort variant staged by a model-picker selection
/// (e.g. "gpt-5.5 (high)") to the server right after the model-switch request.
/// In remote mode the picker cannot apply effort to `app.provider` (a local
/// stand-in), so skipping this leaves the server on its configured default
/// effort - typically low - silently downgrading the request (issue #427).
pub(super) async fn forward_pending_reasoning_effort(app: &mut App, remote: &mut RemoteConnection) {
    let Some(effort) = app.pending_reasoning_effort.take() else {
        return;
    };
    match request_remote_effort(app, remote, &effort).await {
        Ok(()) => {
            app.suppress_next_reasoning_effort_message = true;
        }
        Err(error) => {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to request reasoning effort '{}': {}",
                effort, error
            )));
            app.set_status_notice("Effort switch failed");
        }
    }
}

#[cfg(test)]
mod tests;
