use super::*;

#[derive(Clone)]
pub(super) struct CachedCredentials {
    current: CodexCredentials,
    // External Codex grants rotate without rewriting their source file.
    source: Option<CodexCredentials>,
}

impl From<CodexCredentials> for CachedCredentials {
    fn from(current: CodexCredentials) -> Self {
        Self {
            current,
            source: None,
        }
    }
}

impl std::ops::Deref for CachedCredentials {
    type Target = CodexCredentials;
    fn deref(&self) -> &Self::Target {
        &self.current
    }
}

impl std::ops::DerefMut for CachedCredentials {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.current
    }
}

fn same_login(a: &CodexCredentials, b: &CodexCredentials) -> bool {
    a.access_token == b.access_token
        && a.refresh_token == b.refresh_token
        && (a.account_id.is_none() || b.account_id.is_none() || a.account_id == b.account_id)
}

fn load_current(
    label: Option<&jcode_provider_core::AccountPin>,
    cached: &CodexCredentials,
) -> Result<CodexCredentials> {
    if OpenAIProvider::is_chatgpt_mode(cached) {
        match label {
            Some(pin) => {
                jcode_base::auth::account_pins::load_openai(pin).map(|(credentials, _)| credentials)
            }
            None => jcode_base::auth::codex::load_oauth_credentials(),
        }
    } else {
        jcode_base::auth::codex::load_api_key_credentials()
    }
}

fn stored_matches_source(cached: &CachedCredentials, stored: &CodexCredentials) -> bool {
    same_login(cached, stored)
        || cached
            .source
            .as_ref()
            .is_some_and(|source| same_login(source, stored))
}

pub(super) fn adopt_stored(cached: &mut CachedCredentials, stored: CodexCredentials) {
    if !same_login(cached, &stored) && stored_matches_source(cached, &stored) {
        cached.current.account_id = stored
            .account_id
            .clone()
            .or_else(|| cached.account_id.clone());
        cached.source = Some(stored);
    } else {
        *cached = stored.into();
    }
}

pub(super) fn sync_locked(
    label: Option<&jcode_provider_core::AccountPin>,
    cached: &mut CachedCredentials,
) -> Result<()> {
    match load_current(label, cached) {
        Ok(stored) => adopt_stored(cached, stored),
        Err(error) if label.is_some() && OpenAIProvider::is_chatgpt_mode(cached) => {
            return Err(error);
        }
        // A caller may supply credentials without a corresponding local file.
        Err(_) => {}
    }
    Ok(())
}

pub(super) async fn sync(
    label: Option<&jcode_provider_core::AccountPin>,
    credentials: &Arc<RwLock<CachedCredentials>>,
) -> Result<()> {
    // Read the file after obtaining the lock, so a queued reload cannot apply
    // a snapshot taken before a newer login or explicit account selection.
    sync_locked(label, &mut *credentials.write().await)
}

pub(super) async fn openai_access_token(
    account_label: Option<jcode_provider_core::AccountPin>,
    credentials: &Arc<RwLock<CachedCredentials>>,
) -> Result<String> {
    let (access, refresh, needs_refresh) = {
        let mut cached = credentials.write().await;
        sync_locked(account_label.as_ref(), &mut cached)?;
        anyhow::ensure!(
            !cached.access_token.is_empty(),
            "OpenAI access token is empty"
        );
        (
            cached.access_token.clone(),
            cached.refresh_token.clone(),
            cached
                .expires_at
                .is_some_and(|expires| expires < chrono::Utc::now().timestamp_millis() + 300_000)
                && !cached.refresh_token.is_empty(),
        )
    };
    if needs_refresh {
        force_refresh_openai_token(credentials, &refresh, account_label).await
    } else {
        Ok(access)
    }
}

pub(super) async fn force_refresh_openai_token(
    credentials: &Arc<RwLock<CachedCredentials>>,
    refresh_token: &str,
    account_label: Option<jcode_provider_core::AccountPin>,
) -> Result<String> {
    {
        let mut current = credentials.write().await;
        sync_locked(account_label.as_ref(), &mut current)?;
        if current.refresh_token != refresh_token && !current.access_token.is_empty() {
            return Ok(current.access_token.clone());
        }
    }
    let refresh_from = refresh_token.to_owned();
    let refresh_token = refresh_token.to_owned();
    let label = account_label.clone();
    refresh_and_commit_openai_tokens(credentials, &refresh_from, account_label, async move {
        match label {
            Some(pin) => {
                oauth::refresh_openai_tokens_for_account(
                    &refresh_token,
                    &jcode_base::auth::account_pins::resolve_openai(&pin)?,
                )
                .await
            }
            None => oauth::refresh_openai_tokens(&refresh_token).await,
        }
    })
    .await
}

pub(super) async fn refresh_and_commit_openai_tokens(
    credentials: &Arc<RwLock<CachedCredentials>>,
    expected_refresh: &str,
    account_label: Option<jcode_provider_core::AccountPin>,
    refresh: impl std::future::Future<Output = Result<oauth::OAuthTokens>> + Send + 'static,
) -> Result<String> {
    let expected = credentials.read().await.clone();
    if expected.refresh_token != expected_refresh {
        return Ok(expected.access_token.clone());
    }
    let credentials = Arc::clone(credentials);
    // Rotation, persistence and cache commit survive cancellation together.
    oauth::run_refresh_transaction(async move {
        let result = refresh.await;
        let mut cached = credentials.write().await;
        sync_locked(account_label.as_ref(), &mut cached)?;
        if !same_login(&cached, &expected) && !cached.access_token.is_empty() {
            return Ok(cached.access_token.clone());
        }
        let refreshed = result?;
        let source = expected.source.unwrap_or_else(|| expected.current.clone());
        let current = CodexCredentials {
            access_token: refreshed.access_token,
            refresh_token: refreshed.refresh_token,
            id_token: refreshed.id_token.or_else(|| cached.id_token.clone()),
            account_id: cached.account_id.clone(),
            expires_at: Some(refreshed.expires_at),
        };
        // A persisted rotation becomes its own source. Only an unchanged
        // external source needs provenance to avoid reverting its new bearer.
        let source = match load_current(account_label.as_ref(), &current) {
            Ok(stored) if same_login(&stored, &current) => None,
            _ => Some(source),
        };
        *cached = CachedCredentials { current, source };
        Ok(cached.access_token.clone())
    })
    .await
}

impl OpenAIProvider {
    pub(super) async fn sync_credentials(&self) -> Result<()> {
        sync(self.oauth_account_pin().as_ref(), &self.credentials).await?;
        self.reload_cached_reasoning_efforts();
        Ok(())
    }

    pub(crate) fn reload_credentials_now(&self) {
        match self.credentials.try_write() {
            Ok(mut cached) => {
                if let Ok(credentials) =
                    self.load_selected_credentials(self.credential_mode_snapshot())
                {
                    adopt_stored(&mut cached, credentials);
                    self.browser_only.store(false, AtomicOrdering::Release);
                }
                drop(cached);
                self.reload_cached_reasoning_efforts();
            }
            Err(_) => {
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    let slot = Arc::clone(&self.credentials);
                    let mode = Arc::clone(&self.credential_mode);
                    let label = Arc::clone(&self.oauth_account);
                    let browser_only = Arc::clone(&self.browser_only);
                    let model = Arc::clone(&self.model);
                    let efforts = Arc::clone(&self.model_reasoning_efforts);
                    let tiers = Arc::clone(&self.model_service_tiers);
                    let selected_effort = Arc::clone(&self.reasoning_effort);
                    handle.spawn(async move {
                        let mut cached = slot.write().await;
                        let mode = *mode.read().await;
                        let label = label.read().unwrap_or_else(|p| p.into_inner()).clone();
                        let loaded = match (mode, label.as_ref()) {
                            (
                                OpenAICredentialMode::Auto | OpenAICredentialMode::OAuth,
                                Some(label),
                            ) => jcode_base::auth::account_pins::load_openai(label)
                                .map(|(credentials, _)| credentials),
                            _ => load_credentials_for_mode(mode),
                        };
                        if let Ok(credentials) = loaded {
                            adopt_stored(&mut cached, credentials);
                            let scope = if Self::is_chatgpt_mode(&cached) {
                                label
                                    .map(|pin| {
                                        jcode_base::auth::account_pins::resolve_openai(&pin)
                                            .unwrap_or(pin.label)
                                    })
                                    .unwrap_or_else(|| Self::catalog_scope_for(&cached))
                            } else {
                                Self::catalog_scope_for(&cached)
                            };
                            reload_capabilities(
                                &scope,
                                &model.read().await,
                                &efforts,
                                &tiers,
                                &selected_effort,
                            );
                            browser_only.store(false, AtomicOrdering::Release);
                        }
                    });
                }
            }
        }
        self.clear_persistent_ws_try("credentials reloaded");
    }
}

#[cfg(test)]
#[path = "openai_live_credentials_tests.rs"]
pub(crate) mod tests;

pub(super) fn available_efforts(
    model: &str,
    model_reasoning_efforts: &StdRwLock<HashMap<String, Vec<String>>>,
) -> Vec<&'static str> {
    let model = jcode_provider_core::model_id::canonical(model);
    // Platform-API-only GPT Pro models never appear in the Codex catalog,
    // so their ladders are pinned from live Responses API behavior:
    // gpt-5-pro accepts only `high`; newer pro generations accept
    // medium/high/xhigh.
    if jcode_provider_core::is_openai_api_only_pro_model(&model) {
        return if model.starts_with("gpt-5-pro") {
            vec!["high", "swarm"]
        } else {
            vec!["medium", "high", "xhigh", "swarm"]
        };
    }
    let advertised = model_reasoning_efforts
        .read()
        .map(|efforts| efforts.get(&model).cloned())
        .unwrap_or_else(|poisoned| poisoned.into_inner().get(&model).cloned());
    if let Some(advertised) = advertised {
        let mut efforts: Vec<&'static str> = advertised
            .iter()
            .filter_map(|effort| jcode_provider_core::canonical_reasoning_effort(effort))
            .collect();
        efforts.push("swarm");
        return efforts;
    }
    jcode_provider_core::OPENAI_SELECTABLE_EFFORTS.to_vec()
}

pub(super) fn revalidate_effort(
    model: &str,
    reasoning_effort: &StdRwLock<Option<String>>,
    model_reasoning_efforts: &StdRwLock<HashMap<String, Vec<String>>>,
) {
    let current = reasoning_effort
        .read()
        .map(|effort| effort.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
    let Some(current) = current else {
        return;
    };
    if jcode_base::prompt::is_swarm_effort(&current)
        || available_efforts(model, model_reasoning_efforts).contains(&current.as_str())
    {
        return;
    }
    jcode_base::logging::info(&format!(
        "Clearing OpenAI reasoning effort '{}' because model '{}' does not advertise it",
        current, model
    ));
    match reasoning_effort.write() {
        Ok(mut effort) => *effort = None,
        Err(poisoned) => *poisoned.into_inner() = None,
    }
}
