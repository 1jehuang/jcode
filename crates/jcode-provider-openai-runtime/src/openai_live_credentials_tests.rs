use super::*;
use jcode_base::auth::{codex, test_sandbox::AuthTestSandbox};

struct EnvGuard(&'static str, Option<std::ffi::OsString>);
impl EnvGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        jcode_base::env::set_var(key, value);
        Self(key, previous)
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.1 {
            Some(value) => jcode_base::env::set_var(self.0, value),
            None => jcode_base::env::remove_var(self.0),
        }
    }
}
pub(crate) struct TestEnv {
    _guards: Vec<EnvGuard>,
    sandbox: AuthTestSandbox,
    previous_override: Option<String>,
}
impl TestEnv {
    pub(crate) fn new() -> Self {
        let sandbox = AuthTestSandbox::new().unwrap();
        let previous_override = codex::get_active_account_override();
        codex::set_active_account_override(None);
        Self {
            _guards: vec![
                EnvGuard::set("HOME", sandbox.root()),
                EnvGuard::set("CODEX_HOME", sandbox.root().join("codex")),
                EnvGuard::set("JCODE_GLOBAL_HOME", sandbox.root().join("global")),
                EnvGuard::set("JCODE_TEST_SESSION", "1"),
                EnvGuard::set("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES", ""),
                EnvGuard::set("JCODE_ALLOW_CODEX_LEGACY_AUTH", "1"),
                EnvGuard::set("JCODE_RUNTIME_PROVIDER", "openai-oauth"),
            ],
            sandbox,
            previous_override,
        }
    }
    fn external(&self, token: &str) {
        let path = codex::legacy_auth_file_path().unwrap();
        assert!(path.starts_with(self.sandbox.root()));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        jcode_base::storage::write_json_secret(
            &path,
            &serde_json::json!({"tokens": {
                "access_token":token, "refresh_token":format!("{token}-refresh"),
                "account_id":"external-workspace", "expires_at":future_expiry()
            }}),
        )
        .unwrap();
    }
}
impl Drop for TestEnv {
    fn drop(&mut self) {
        codex::set_active_account_override(self.previous_override.take());
    }
}
fn future_expiry() -> i64 {
    chrono::Utc::now().timestamp_millis() + 3_600_000
}
fn save(label: &str, token: &str, workspace: &str) -> String {
    codex::upsert_account(codex::OpenAiAccount {
        record_id: None,
        label: label.into(),
        access_token: token.into(),
        refresh_token: format!("{token}-refresh"),
        account_id: Some(workspace.into()),
        expires_at: Some(future_expiry()),
        id_token: None,
        email: None,
    })
    .unwrap()
}
fn response(token: &str) -> oauth::OAuthTokens {
    oauth::OAuthTokens {
        access_token: token.into(),
        refresh_token: format!("{token}-refresh"),
        expires_at: future_expiry(),
        id_token: None,
        scopes: Vec::new(),
    }
}
fn provider() -> OpenAIProvider {
    // Keep constructor model/usage prefetch outside an async runtime.
    std::thread::spawn(|| OpenAIProvider::new(codex::load_oauth_credentials().unwrap()))
        .join()
        .unwrap()
}
async fn token(provider: &OpenAIProvider) -> String {
    openai_access_token(provider.oauth_account_pin(), &provider.credentials)
        .await
        .unwrap()
}

#[tokio::test]
async fn same_label_relogin_reaches_independent_and_copied_caches() {
    let _env = TestEnv::new();
    let label = save("work", "old", "work-id");
    let sessions = [provider(), provider()];
    let copied = Arc::new(RwLock::new(sessions[0].credentials.read().await.clone()));
    save(&label, "replacement", "work-id");
    for session in &sessions {
        assert_eq!(token(session).await, "replacement");
    }
    assert_eq!(
        openai_access_token(None, &copied).await.unwrap(),
        "replacement"
    );
    assert_eq!(
        sessions[0].credentials.read().await.account_id.as_deref(),
        Some("work-id")
    );
}

#[tokio::test]
async fn default_switch_follows_current_login_while_explicit_binding_stays_selected() {
    let _env = TestEnv::new();
    let first = save("first", "first-token", "first-id");
    let second = save("second", "second-token", "second-id");
    codex::set_active_account(&first).unwrap();
    let unbound = provider();
    let bound = provider();
    bound.set_oauth_account(Some(&first)).unwrap();
    codex::set_active_account(&second).unwrap();
    assert_eq!(token(&unbound).await, "second-token");
    assert_eq!(token(&bound).await, "first-token");
    assert_eq!(
        unbound.credentials.read().await.account_id.as_deref(),
        Some("second-id")
    );
}

#[tokio::test]
async fn source_is_reread_after_waiting_for_the_cache_lock() {
    let _env = TestEnv::new();
    let label = save("work", "old", "work-id");
    let session = provider();
    let guard = session.credentials.read().await;
    let cache = Arc::clone(&session.credentials);
    let task = tokio::spawn(async move { openai_access_token(None, &cache).await });
    tokio::task::yield_now().await;
    save(&label, "latest", "work-id");
    drop(guard);
    assert_eq!(task.await.unwrap().unwrap(), "latest");
}

#[tokio::test]
async fn deferred_reload_reads_latest_file_and_current_binding() {
    let _env = TestEnv::new();
    let first = save("first", "first-token", "first-id");
    let second = save("second", "second-token", "second-id");
    codex::set_active_account(&first).unwrap();
    let session = provider();
    let guard = session.credentials.write().await;
    session.reload_credentials_now();
    tokio::task::yield_now().await;
    save(&second, "latest", "second-id");
    *session.oauth_account.write().unwrap() =
        Some(jcode_base::auth::account_pins::openai_pin(&second).unwrap());
    drop(guard);
    tokio::time::timeout(Duration::from_secs(2), async {
        while session.credentials.read().await.access_token != "latest" {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        session.credentials.read().await.account_id.as_deref(),
        Some("second-id")
    );
}

#[tokio::test]
async fn forced_refresh_adopts_replacement_without_rotating_old_grant() {
    let _env = TestEnv::new();
    let label = save("work", "old", "work-id");
    let session = provider();
    save(&label, "new", "new-id");
    let bearer = force_refresh_openai_token(&session.credentials, "old-refresh", None)
        .await
        .unwrap();
    assert_eq!(bearer, "new");
    assert_eq!(
        session.credentials.read().await.account_id.as_deref(),
        Some("new-id")
    );
}

#[tokio::test]
async fn completed_refresh_cannot_overwrite_login_replaced_on_disk_without_reload() {
    let _env = TestEnv::new();
    let label = save("work", "old", "old-id");
    let session = provider();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let cache = Arc::clone(&session.credentials);
    let task = tokio::spawn(async move {
        refresh_and_commit_openai_tokens(&cache, "old-refresh", None, async move {
            started_tx.send(()).unwrap();
            release_rx.await.unwrap();
            Ok(response("old-rotated"))
        })
        .await
    });
    started_rx.await.unwrap();
    save(&label, "replacement", "new-id");
    release_tx.send(()).unwrap();
    assert_eq!(task.await.unwrap().unwrap(), "replacement");
    let cached = session.credentials.read().await;
    assert_eq!(cached.refresh_token, "replacement-refresh");
    assert_eq!(cached.account_id.as_deref(), Some("new-id"));
}

#[tokio::test]
async fn failed_stale_refresh_can_use_new_login_but_unchanged_failure_stays_an_error() {
    let _env = TestEnv::new();
    let label = save("work", "old", "old-id");
    let session = provider();
    let replacement = label.clone();
    let bearer =
        refresh_and_commit_openai_tokens(&session.credentials, "old-refresh", None, async move {
            save(&replacement, "new", "new-id");
            anyhow::bail!("old grant rejected")
        })
        .await
        .unwrap();
    assert_eq!(bearer, "new");
    let error =
        refresh_and_commit_openai_tokens(&session.credentials, "new-refresh", None, async {
            anyhow::bail!("current grant rejected")
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("current grant rejected"));
}

#[tokio::test]
async fn external_rotations_survive_unchanged_source_and_adopt_external_relogin() {
    let env = TestEnv::new();
    env.external("external-old");
    let session = provider();
    for (old, new) in [
        ("external-old", "rotated-one"),
        ("rotated-one", "rotated-two"),
    ] {
        let bearer = refresh_and_commit_openai_tokens(
            &session.credentials,
            &format!("{old}-refresh"),
            None,
            async move { Ok(response(new)) },
        )
        .await
        .unwrap();
        assert_eq!(bearer, new);
        assert_eq!(token(&session).await, new);
        session.reload_credentials_now();
        assert_eq!(token(&session).await, new);
        session.invalidate_credentials().await;
        assert_eq!(token(&session).await, new);
        session
            .set_credential_mode(OpenAICredentialMode::OAuth)
            .unwrap();
        assert_eq!(token(&session).await, new);
    }
    env.external("external-relogin");
    assert_eq!(token(&session).await, "external-relogin");
}

#[tokio::test]
async fn persisted_rotation_does_not_skip_a_later_rejection_of_its_own_bearer() {
    let _env = TestEnv::new();
    let label = save("work", "old", "work-id");
    let session = provider();
    refresh_and_commit_openai_tokens(&session.credentials, "old-refresh", None, async move {
        save(&label, "rotated", "work-id");
        Ok(response("rotated"))
    })
    .await
    .unwrap();
    assert_eq!(token(&session).await, "rotated");
    let failure: Result<oauth::OAuthTokens> = Err(anyhow::anyhow!(
        "invalid_grant fixture terminal refresh rejection"
    ));
    jcode_base::auth::refresh_state::record_refresh_outcome("openai", "rotated-refresh", &failure);
    let err = force_refresh_openai_token(&session.credentials, "rotated-refresh", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("previously rejected"), "{err:#}");
}

#[tokio::test]
async fn deleted_explicit_oauth_binding_fails_closed_but_api_key_rotation_ignores_it() {
    let _env = TestEnv::new();
    let label = save("work", "old", "work-id");
    let session = provider();
    session.set_oauth_account(Some(&label)).unwrap();
    codex::remove_account(&label).unwrap();
    assert!(
        openai_access_token(session.oauth_account_pin(), &session.credentials)
            .await
            .is_err()
    );
    let _key = EnvGuard::set("OPENAI_API_KEY", "fixture-platform-one");
    session
        .set_credential_mode(OpenAICredentialMode::ApiKey)
        .unwrap();
    let _rotated = EnvGuard::set("OPENAI_API_KEY", "fixture-platform-two");
    assert_eq!(token(&session).await, "fixture-platform-two");
    let cached = session.credentials.read().await;
    assert!(!OpenAIProvider::is_chatgpt_mode(&cached));
    assert!(cached.account_id.is_none());
    drop(cached);
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let _base = EnvGuard::set("JCODE_OPENAI_API_BASE", format!("http://{address}/v1"));
    let server = tokio::spawn(async move {
        let (mut tcp, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut block = [0; 4096];
            let count = tcp.read(&mut block).await.unwrap();
            assert_ne!(count, 0);
            request.extend_from_slice(&block[..count]);
            if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                let length: usize = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: ")?.parse().ok())
                    .unwrap();
                if request.len() >= end + 4 + length {
                    break;
                }
            }
        }
        let body = "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n";
        tcp.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        String::from_utf8(request).unwrap().to_ascii_lowercase()
    });
    let (tx, mut rx) = mpsc::channel(128);
    super::super::openai_stream_runtime::stream_response(
        Client::new(),
        Arc::clone(&session.credentials),
        Some(jcode_provider_core::AccountPin::new(label, None)),
        &serde_json::json!({"model":"fixture-api-model","input":[]}),
        "fixture".into(),
        tx,
    )
    .await
    .unwrap();
    while let Some(event) = rx.recv().await {
        event.unwrap();
    }
    let request = server.await.unwrap();
    assert!(request.starts_with("post /v1/responses "));
    assert!(request.contains("authorization: bearer fixture-platform-two\r\n"));
    assert!(!request.contains("chatgpt-account-id:"));
}

#[tokio::test]
async fn external_identity_hydration_keeps_its_rotated_refresh_generation() {
    let env = TestEnv::new();
    env.external("external-old");
    let path = codex::legacy_auth_file_path().unwrap();
    let mut file: Value = jcode_base::storage::read_json(&path).unwrap();
    file["tokens"]["account_id"] = Value::Null;
    jcode_base::storage::write_json_secret(&path, &file).unwrap();
    let session = provider();
    refresh_and_commit_openai_tokens(&session.credentials, "external-old-refresh", None, async {
        Ok(response("rotated"))
    })
    .await
    .unwrap();
    file["tokens"]["account_id"] = Value::String("hydrated-workspace".into());
    jcode_base::storage::write_json_secret(&path, &file).unwrap();
    assert_eq!(token(&session).await, "rotated");
    assert_eq!(
        session.credentials.read().await.account_id.as_deref(),
        Some("hydrated-workspace")
    );
    assert_eq!(
        session.credentials.read().await.refresh_token,
        "rotated-refresh"
    );
}

const CAPABILITY_MODEL: &str = "fixture-live-account-model";
fn capability_accounts() -> (OpenAIProvider, String, String) {
    let first = save("cap-first", "first-token", "first-workspace");
    let second = save("cap-second", "second-token", "second-workspace");
    for (scope, tier, effort) in [
        (&first, "ultrafast", "xhigh"),
        (&second, "priority", "medium"),
    ] {
        let mut catalog = jcode_base::provider::OpenAIModelCatalog::default();
        catalog.available_models.push(CAPABILITY_MODEL.into());
        catalog
            .service_tiers
            .insert(CAPABILITY_MODEL.into(), vec![tier.into()]);
        catalog
            .reasoning_efforts
            .insert(CAPABILITY_MODEL.into(), vec![effort.into()]);
        jcode_base::provider::persist_openai_model_catalog_for_scope(scope, &catalog);
    }
    codex::set_active_account(&first).unwrap();
    let session = provider();
    *session.model.try_write().unwrap() = CAPABILITY_MODEL.into();
    session.reload_cached_reasoning_efforts();
    session.set_service_tier("ultrafast").unwrap();
    session.set_reasoning_effort("xhigh").unwrap();
    (session, first, second)
}
fn assert_second_capabilities(session: &OpenAIProvider) {
    assert_eq!(session.service_tier(), None);
    assert_eq!(session.available_efforts(), vec!["medium", "swarm"]);
    assert_eq!(session.reasoning_effort(), None);
    assert!(!session.model_capabilities_need_refresh());
    let request = session.response_request_for_model(CAPABILITY_MODEL, &[], &[], "", true);
    assert!(request.get("service_tier").is_none());
    assert!(
        request
            .get("reasoning")
            .is_none_or(|r| r.get("effort").is_none())
    );
}
#[tokio::test]
async fn direct_account_sync_replaces_tier_and_effort_capabilities_before_request_setup() {
    let _env = TestEnv::new();
    let (session, _, second) = capability_accounts();
    codex::set_active_account(&second).unwrap();
    session.sync_credentials().await.unwrap();
    assert_second_capabilities(&session);
    let uncached = save("uncached", "uncached-token", "uncached-workspace");
    codex::set_active_account(&uncached).unwrap();
    session.sync_credentials().await.unwrap();
    assert!(session.model_capabilities_need_refresh());
    assert!(
        !session
            .model_service_tiers
            .read()
            .unwrap()
            .contains_key(CAPABILITY_MODEL)
    );
}
#[tokio::test]
async fn deferred_account_reload_replaces_tier_and_effort_capabilities_before_publishing_login() {
    let _env = TestEnv::new();
    let (session, _, second) = capability_accounts();
    let guard = session.credentials.read().await;
    session.reload_credentials_now();
    tokio::task::yield_now().await;
    codex::set_active_account(&second).unwrap();
    drop(guard);
    tokio::time::timeout(Duration::from_secs(2), async {
        while session.credentials.read().await.access_token != "second-token" {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_second_capabilities(&session);
}
#[tokio::test]
async fn capability_sync_keeps_explicit_account_and_platform_catalog_scopes() {
    let _env = TestEnv::new();
    let (session, first, second) = capability_accounts();
    session.set_oauth_account(Some(&first)).unwrap();
    codex::set_active_account(&second).unwrap();
    session.sync_credentials().await.unwrap();
    assert_eq!(session.service_tier().as_deref(), Some("ultrafast"));
    assert_eq!(session.available_efforts(), vec!["xhigh", "swarm"]);
    let _key = EnvGuard::set("OPENAI_API_KEY", "fixture-platform-capabilities");
    let api = codex::load_api_key_credentials().unwrap();
    let scope = OpenAIProvider::catalog_scope_for(&api);
    let mut catalog = jcode_base::provider::OpenAIModelCatalog::default();
    catalog.available_models.push(CAPABILITY_MODEL.into());
    catalog
        .service_tiers
        .insert(CAPABILITY_MODEL.into(), vec!["priority".into()]);
    catalog
        .reasoning_efforts
        .insert(CAPABILITY_MODEL.into(), vec!["medium".into()]);
    jcode_base::provider::persist_openai_model_catalog_for_scope(&scope, &catalog);
    session
        .set_credential_mode(OpenAICredentialMode::ApiKey)
        .unwrap();
    session.sync_credentials().await.unwrap();
    assert_eq!(session.catalog_scope(), scope);
    assert_second_capabilities(&session);
}

#[path = "openai_identity_tests.rs"]
mod identity;

#[tokio::test(flavor = "current_thread")]
async fn complete_reloads_account_capabilities_before_preparing_the_next_request() {
    let _env = TestEnv::new();
    let (mut session, _, second) = capability_accounts();
    session.set_transport("https").unwrap();
    session.client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all("http://127.0.0.1:9").unwrap())
        .build()
        .unwrap();
    codex::set_active_account(&second).unwrap();
    let stream = session.complete(&[], &[], "", None).await.unwrap();
    // On this current-thread executor, close the consumer before its transport task can run.
    drop(stream);
    assert_second_capabilities(&session);
    let credentials = session.credentials.try_read().unwrap();
    assert_eq!(credentials.access_token, "second-token");
    assert_eq!(credentials.account_id.as_deref(), Some("second-workspace"));
    drop(credentials);
    tokio::task::yield_now().await;
}
