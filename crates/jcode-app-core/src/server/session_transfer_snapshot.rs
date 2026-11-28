//! Snapshot preparation for session split and transfer actions.

use super::*;

pub(super) fn clone_split_session(
    parent_session_id: &str,
    live_parent: Option<&Session>,
) -> anyhow::Result<(String, String)> {
    // Keep the persisted snapshot authoritative, including while the parent is
    // busy. A brand-new Agent may not have saved anything yet, however. Only a
    // missing snapshot permits an in-memory fallback, never corrupt/unreadable
    // history or a session belonging to a different client.
    let parent = Session::load(parent_session_id).or_else(|error| {
        let missing = error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound);
        match live_parent.filter(|parent| missing && parent.id == parent_session_id) {
            Some(parent) => Ok(parent.clone()),
            None => Err(error),
        }
    })?;

    let mut child = Session::create(Some(parent_session_id.to_string()), None);
    child.replace_messages(parent.messages.clone());
    child.compaction = parent.compaction.clone();
    child.system_prompt = parent.system_prompt.clone();
    child.working_dir = parent.working_dir.clone();
    child.model = parent.model.clone();
    // Keep the same provider account. Replayed provider-native blocks (Anthropic
    // web search `encrypted_content`) are bound to the account that produced
    // them, and another account rejects the whole request with a 400.
    child.inherit_provider_route(&parent);
    child.status = crate::session::SessionStatus::Closed;
    // The parent agent keeps ownership of any in-flight request; tell the
    // forked agent so it treats the next prompt as fresh work instead of
    // continuing (and duplicating) the parent's current turn.
    child.append_fork_notice(parent_session_id, parent.display_name());
    child.save()?;

    crate::session_env::inherit_session_env(parent_session_id, &child.id);
    let name = child.display_name().to_string();
    Ok((child.id.clone(), name))
}

pub(super) fn transfer_active_messages(session: &Session) -> Vec<crate::message::Message> {
    let start = session
        .compaction
        .as_ref()
        .map(|state| state.compacted_count.min(session.messages.len()))
        .unwrap_or(0);
    session.messages[start..]
        .iter()
        .map(crate::session::StoredMessage::to_message)
        .collect()
}

pub(super) fn create_transfer_child_session(
    parent_session_id: &str,
    parent: &Session,
    compaction: Option<Arc<crate::session::StoredCompactionState>>,
) -> anyhow::Result<(String, String)> {
    let mut todos = crate::todo::load_todos(parent_session_id)?;
    // Background tasks belong to the source session. A fresh session must
    // reassess those results, not inherit foreign task IDs or their wakes.
    for todo in &mut todos {
        todo.waiting_on.clear();
        todo.waiting_reason = None;
    }
    let mut child = Session::create(Some(parent_session_id.to_string()), None);
    child.messages.clear();
    child.compaction = compaction;
    child.system_prompt = parent.system_prompt.clone();
    child.working_dir = parent.working_dir.clone();
    child.model = parent.model.clone();
    child.inherit_provider_route(parent);
    child.subagent_model = parent.subagent_model.clone();
    child.improve_mode = parent.improve_mode;
    child.autoreview_enabled = parent.autoreview_enabled;
    child.autojudge_enabled = parent.autojudge_enabled;
    child.is_canary = parent.is_canary;
    child.testing_build = parent.testing_build.clone();
    child.provider_session_id = None;
    child.status = crate::session::SessionStatus::Closed;
    child.save()?;
    crate::todo::save_todos(&child.id, &todos)?;
    crate::session_env::inherit_session_env(parent_session_id, &child.id);
    Ok((child.id.clone(), child.display_name().to_string()))
}

#[cfg(test)]
mod account_pin_tests {
    use super::*;
    use crate::provider::Provider;

    struct Env(&'static str, Option<std::ffi::OsString>);
    impl Env {
        fn set(key: &'static str, path: &std::path::Path) -> Self {
            let previous = std::env::var_os(key);
            crate::env::set_var(key, path);
            Self(key, previous)
        }
    }
    impl Drop for Env {
        fn drop(&mut self) {
            match &self.1 {
                Some(value) => crate::env::set_var(self.0, value),
                None => crate::env::remove_var(self.0),
            }
        }
    }
    #[derive(Default)]
    struct PinnedRuntime(std::sync::Mutex<Option<jcode_provider_core::AccountPin>>);
    #[async_trait::async_trait]
    impl Provider for PinnedRuntime {
        async fn complete(
            &self,
            _: &[crate::message::Message],
            _: &[crate::message::ToolDefinition],
            _: &str,
            _: Option<&str>,
        ) -> anyhow::Result<crate::provider::EventStream> {
            panic!("session-copy tests never infer")
        }
        fn name(&self) -> &str {
            "OpenAI"
        }
        fn model(&self) -> String {
            "gpt-child-identity".into()
        }
        fn set_model(&self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(Self(std::sync::Mutex::new(self.oauth_account_pin())))
        }
        fn oauth_account_pin(&self) -> Option<jcode_provider_core::AccountPin> {
            self.0.lock().unwrap().clone()
        }
        fn set_oauth_account_pin(
            &self,
            pin: &jcode_provider_core::AccountPin,
        ) -> anyhow::Result<()> {
            *self.0.lock().unwrap() = Some(crate::auth::account_pins::normalize_openai_pin(pin)?);
            Ok(())
        }
        fn oauth_account_label(&self) -> Option<String> {
            self.oauth_account_pin()
                .map(|pin| crate::auth::account_pins::resolve_openai(&pin).unwrap_or(pin.label))
        }
        fn active_resolved_credential(&self) -> Option<jcode_provider_core::ResolvedCredential> {
            Some(jcode_provider_core::ResolvedCredential::Oauth)
        }
    }

    #[test]
    fn split_and_transfer_children_keep_identity_through_actual_resume_rename_and_label_reuse() {
        let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
        let _home = Env::set("HOME", sandbox.root());
        let _global = Env::set("JCODE_GLOBAL_HOME", &sandbox.root().join("global"));
        let _xdg = Env::set("XDG_CONFIG_HOME", &sandbox.root().join("xdg"));
        let original = crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            record_id: None,
            label: "original".into(),
            access_token: "original-access".into(),
            refresh_token: "original-refresh".into(),
            id_token: None,
            account_id: Some("original-workspace".into()),
            expires_at: Some(i64::MAX),
            email: None,
        })
        .unwrap();
        let pin = crate::auth::account_pins::openai_pin(&original).unwrap();
        let mut parent = Session::create_with_id("parent_account_identity".into(), None, None);
        let route = format!("openai-oauth@{original}");
        parent.model = Some(format!("{route}:gpt-child-identity"));
        parent.route_api_method = Some(route.clone());
        parent.provider_key = Some(route);
        parent.account_pins.insert("openai".into(), pin.clone());
        parent.account_pins.insert(
            "claude".into(),
            jcode_provider_core::AccountPin {
                label: "inactive-family".into(),
                identity: None,
                record_id: Some("inactive-row".into()),
            },
        );
        parent.save_prepared().unwrap();
        let mut unsaved = parent.clone();
        unsaved.account_pins.clear();
        let split = clone_split_session(&parent.id, Some(&unsaved)).unwrap().0;
        let transfer = create_transfer_child_session(&parent.id, &parent, None)
            .unwrap()
            .0;
        let mut fresh = parent.clone();
        fresh.id = "never_saved_pin_parent".into();
        let fresh_split = clone_split_session(&fresh.id, Some(&fresh)).unwrap().0;
        let children = [split, transfer, fresh_split];
        for id in &children {
            assert_eq!(
                Session::load_for_resume(id).unwrap().account_pins,
                parent.account_pins
            );
        }
        let mut auth = crate::auth::codex::load_auth_file().unwrap();
        auth.openai_accounts[0].label = "Renamed Account".into();
        auth.active_openai_account = Some("Renamed Account".into());
        crate::auth::codex::save_auth_file(&auth).unwrap();
        for id in &children {
            let session = Session::load_for_resume(id).unwrap();
            let resumed = crate::agent::Agent::new_with_session(
                Arc::new(PinnedRuntime::default()),
                crate::tool::Registry::empty(),
                session,
                None,
            );
            resumed.ensure_session_lease().unwrap();
            assert_eq!(
                resumed.session_route_api_method().as_deref(),
                Some("openai-oauth@Renamed Account")
            );
            let saved = resumed.session_for_split();
            assert_eq!(saved.account_pins["openai"].record_id, pin.record_id);
            assert_eq!(saved.account_pins["claude"], parent.account_pins["claude"]);
            drop(resumed);
            // Keep each persisted child at the original route to exercise reuse before resume.
            let mut original_child = Session::load(id).unwrap();
            original_child.inherit_provider_route(&parent);
            original_child.model = parent.model.clone();
            original_child.save_prepared().unwrap();
        }
        auth.openai_accounts[0].label = original;
        auth.openai_accounts[0].record_id = Some(uuid::Uuid::new_v4().to_string());
        auth.openai_accounts[0].account_id = Some("replacement-workspace".into());
        auth.active_openai_account = Some(auth.openai_accounts[0].label.clone());
        crate::auth::codex::save_auth_file(&auth).unwrap();
        for id in &children {
            let resumed = crate::agent::Agent::new_with_session(
                Arc::new(PinnedRuntime::default()),
                crate::tool::Registry::empty(),
                Session::load_for_resume(id).unwrap(),
                None,
            );
            assert!(
                resumed.ensure_session_lease().is_err(),
                "a child must not adopt a different workspace that reuses its label"
            );
        }
    }
}
