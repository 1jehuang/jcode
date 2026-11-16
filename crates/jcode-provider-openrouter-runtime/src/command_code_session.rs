use super::*;

impl OpenRouterProvider {
    fn is_command_code(&self) -> bool {
        self.profile_id
            .as_deref()
            .is_some_and(|id| id.eq_ignore_ascii_case("command-code"))
    }

    pub(super) fn initial_session_account(profile_id: Option<&str>) -> Arc<Mutex<Option<String>>> {
        let label = profile_id
            .is_some_and(|id| id.eq_ignore_ascii_case("command-code"))
            .then(jcode_base::provider_catalog::command_code_accounts::active_label)
            .flatten()
            .filter(|label| {
                jcode_base::provider_catalog::command_code_accounts::api_key_for_label(label)
                    .is_some()
            });
        Arc::new(Mutex::new(label))
    }

    pub(super) fn session_account_label(&self) -> Option<String> {
        if !self.is_command_code() {
            return None;
        }
        self.session_account
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn bind_session_account_inner(
        &self,
        account: Option<&str>,
    ) -> Result<Option<String>> {
        if !self.is_command_code() {
            return Ok(None);
        }
        let label = account
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .map(str::to_string)
            .or_else(|| self.session_account_label())
            .or_else(|| {
                jcode_base::provider_catalog::command_code_accounts::active_label().filter(
                    |label| {
                        jcode_base::provider_catalog::command_code_accounts::api_key_for_label(
                            label,
                        )
                        .is_some()
                    },
                )
            });
        if account.is_some() || label.is_some() {
            *self
                .session_account
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = label.clone();
        }
        Ok(label)
    }

    /// Resolve request auth from the pinned account, never from the current
    /// global active account once a session is bound.
    pub(super) fn resolve_auth(&self) -> Result<ProviderAuth> {
        if let Some(label) = self.session_account_label() {
            let token =
                jcode_base::provider_catalog::command_code_accounts::api_key_for_label(&label)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Command Code account `{label}` is no longer stored. Run `jcode login --provider command-code --account {label}` to restore it."
                        )
                    })?;
            return Ok(ProviderAuth::AuthorizationBearer {
                token,
                label: format!("COMMAND_CODE_API_KEY ({label})"),
            });
        }
        (self.auth)()
    }
}
