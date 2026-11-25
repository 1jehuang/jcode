use super::*;

struct EnvGuard(&'static str, Option<std::ffi::OsString>);
impl EnvGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        jcode_base::env::set_var(key, value);
        Self(key, previous)
    }
    fn remove(key: &'static str) -> Self {
        let guard = Self::set(key, "");
        jcode_base::env::remove_var(key);
        guard
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
struct TestEnv {
    _guards: Vec<EnvGuard>,
    sandbox: auth::test_sandbox::AuthTestSandbox,
    previous_override: Option<String>,
}
impl TestEnv {
    fn new() -> Self {
        let sandbox = auth::test_sandbox::AuthTestSandbox::new().unwrap();
        let previous_override = auth::claude::get_active_account_override();
        auth::claude::set_active_account_override(None);
        let config = sandbox.root().join("claude-config");
        Self {
            _guards: vec![
                EnvGuard::remove("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES"),
                EnvGuard::set("CLAUDE_CODE_OAUTH_TOKEN", ""),
                EnvGuard::set("CLAUDE_CONFIG_DIR", config),
                EnvGuard::set("JCODE_TEST_SESSION", "1"),
            ],
            sandbox,
            previous_override,
        }
    }
}
impl Drop for TestEnv {
    fn drop(&mut self) {
        auth::claude::set_active_account_override(self.previous_override.take());
    }
}
// Construct outside Tokio so the constructor cannot spawn its usage prewarm.
fn offline_provider() -> AnthropicProvider {
    std::thread::spawn(AnthropicProvider::new).join().unwrap()
}

fn account(label: &str, token: &str) -> auth::claude::AnthropicAccount {
    auth::claude::AnthropicAccount {
        label: label.into(),
        access: token.into(),
        refresh: format!("{token}-refresh"),
        expires: chrono::Utc::now().timestamp_millis() + 3_600_000,
        email: Some(format!("{token}@example.invalid")),
        subscription_type: Some("max".into()),
        scopes: vec!["user:inference".into()],
    }
}
fn save(label: &str, token: &str) -> String {
    auth::claude::upsert_account(account(label, token)).unwrap()
}
fn response(token: &str) -> oauth::OAuthTokens {
    oauth::OAuthTokens {
        access_token: token.into(),
        refresh_token: format!("{token}-refresh"),
        expires_at: chrono::Utc::now().timestamp_millis() + 3_600_000,
        id_token: None,
        scopes: vec!["user:inference".into()],
    }
}

#[tokio::test]
async fn live_credentials_same_label_relogin_reaches_independent_session_caches() {
    let _env = TestEnv::new();
    let label = save("work", "old-login");
    let sessions = [offline_provider(), offline_provider()];
    for provider in &sessions {
        assert_eq!(
            provider.get_oauth_access_token().await.unwrap().0,
            "old-login"
        );
    }
    save(&label, "new-login");
    for provider in &sessions {
        assert_eq!(
            provider.get_oauth_access_token().await.unwrap().0,
            "new-login"
        );
    }
}

#[tokio::test]
async fn live_credentials_default_changes_preserve_explicit_bindings_and_removed_binding_fails() {
    let _env = TestEnv::new();
    let first = save("work", "first");
    let second = save("personal", "second");
    auth::claude::set_active_account(&first).unwrap();
    let default = offline_provider();
    let bound = offline_provider();
    bound.set_oauth_account(Some(&first)).unwrap();
    assert_eq!(default.get_oauth_access_token().await.unwrap().0, "first");
    assert_eq!(bound.get_oauth_access_token().await.unwrap().0, "first");
    auth::claude::set_active_account(&second).unwrap();
    assert_eq!(default.get_oauth_access_token().await.unwrap().0, "second");
    assert_eq!(bound.get_oauth_access_token().await.unwrap().0, "first");
    auth::claude::remove_account(&first).unwrap();
    assert!(bound.get_oauth_access_token().await.is_err());
}

#[tokio::test]
async fn live_credentials_external_rewrite_and_revocation_replace_a_fresh_cache() {
    let env = TestEnv::new();
    let path = env.sandbox.root().join("claude-config/.credentials.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let write = |token: &str| {
        std::fs::write(
            &path,
            serde_json::json!({"claudeAiOauth":{
                "accessToken": token, "refreshToken": format!("{token}-refresh"),
                "expiresAt": chrono::Utc::now().timestamp_millis()+3_600_000,
                "scopes":["user:inference"]
            }})
            .to_string(),
        )
        .unwrap();
    };
    write("external-first");
    jcode_base::config::Config::allow_external_auth_source_for_path(
        auth::claude::CLAUDE_CODE_AUTH_SOURCE_ID,
        &path,
    )
    .unwrap();
    let provider = offline_provider();
    assert_eq!(
        provider.get_oauth_access_token().await.unwrap().0,
        "external-first"
    );
    write("external-second");
    assert_eq!(
        provider.get_oauth_access_token().await.unwrap().0,
        "external-second"
    );
    jcode_base::config::Config::revoke_external_auth_source_for_path(
        auth::claude::CLAUDE_CODE_AUTH_SOURCE_ID,
        &path,
    )
    .unwrap();
    assert!(provider.get_oauth_access_token().await.is_err());
}

#[tokio::test]
async fn live_credentials_forced_refresh_joins_new_login_without_network_or_old_refresh() {
    let _env = TestEnv::new();
    let label = save("work", "first");
    let provider = offline_provider();
    provider.get_oauth_access_token().await.unwrap();
    save(&label, "replacement");
    for token in ["first-refresh", "replacement-refresh"] {
        auth::refresh_state::record_permanent_rejection("claude", token, "synthetic rejection")
            .unwrap();
    }
    assert_eq!(
        force_refresh_oauth_token(provider.credentials.clone(), &None)
            .await
            .unwrap(),
        "replacement"
    );
    let mut auth = auth::claude::load_auth_file().unwrap();
    auth.anthropic_accounts[0].scopes = vec!["user:profile".into()];
    auth::claude::save_auth_file(&auth).unwrap();
    assert!(provider.get_oauth_access_token().await.is_err());
    assert!(
        force_refresh_oauth_token(provider.credentials.clone(), &None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn live_credentials_finished_refresh_does_not_restore_old_choice_or_removed_login() {
    let _env = TestEnv::new();
    let first = save("work", "first");
    let second = save("personal", "second");
    auth::claude::set_active_account(&first).unwrap();
    let source = ("first".into(), "first-refresh".into());
    let credentials = Arc::new(RwLock::new(None));
    auth::claude::set_active_account(&second).unwrap();
    assert_eq!(
        commit_claude_refresh(
            &credentials,
            &None,
            source.clone(),
            response("rotated-first")
        )
        .await
        .unwrap(),
        "second"
    );
    assert!(credentials.read().await.is_none());
    auth::claude::remove_account(&first).unwrap();
    assert!(
        commit_claude_refresh(
            &credentials,
            &Some(first),
            source,
            response("rotated-first")
        )
        .await
        .is_err()
    );
    assert!(credentials.read().await.is_none());
}

#[tokio::test]
async fn live_credentials_finished_refresh_keeps_external_rotation_and_persisted_rotation() {
    let _env = TestEnv::new();
    let label = save("work", "first");
    let source = ("first".into(), "first-refresh".into());
    let credentials = Arc::new(RwLock::new(None));
    assert_eq!(
        commit_claude_refresh(
            &credentials,
            &None,
            source.clone(),
            response("rotated-first")
        )
        .await
        .unwrap(),
        "rotated-first"
    );
    assert_eq!(
        resolve_oauth_access_token(&credentials, &None)
            .await
            .unwrap()
            .0,
        "rotated-first"
    );
    save(&label, "persisted-rotation");
    assert_eq!(
        commit_claude_refresh(&credentials, &None, source, response("persisted-rotation"))
            .await
            .unwrap(),
        "persisted-rotation"
    );
    assert_eq!(
        resolve_oauth_access_token(&credentials, &None)
            .await
            .unwrap()
            .0,
        "persisted-rotation"
    );
}

async fn request_bearer(stream: &mut tokio::net::TcpStream) -> String {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    let mut reader = BufReader::new(stream);
    let mut bearer = String::new();
    let mut length = 0;
    loop {
        let mut line = String::new();
        assert_ne!(reader.read_line(&mut line).await.unwrap(), 0);
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("authorization") {
                bearer = value.trim().into();
            }
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse::<usize>().unwrap();
            }
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await.unwrap();
    bearer
}
async fn send_response(stream: &mut tokio::net::TcpStream, status: u16, body: &str) {
    use tokio::io::AsyncWriteExt;
    let response = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await.unwrap();
}
const SUCCESS: &str = "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

#[tokio::test]
async fn live_credentials_real_fork_and_retry_use_the_latest_default_bearer() {
    let _env = TestEnv::new();
    let first = save("work", "first");
    let second = save("personal", "second");
    auth::claude::set_active_account(&first).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    let mut provider = offline_provider();
    provider.direct_transport.oauth_api_url = Some(url);
    provider.oauth_preflight_done.store(true, Ordering::Relaxed);
    provider.set_model("claude-sonnet-4-6").unwrap();
    let fork = provider.fork();
    let server = tokio::spawn(async move {
        let mut seen = Vec::new();
        for index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            seen.push(request_bearer(&mut stream).await);
            if index == 1 {
                send_response(
                    &mut stream,
                    503,
                    r#"{"error":{"type":"overloaded_error","message":"overloaded"}}"#,
                )
                .await;
            } else {
                send_response(&mut stream, 200, SUCCESS).await;
            }
        }
        seen
    });
    let run = async {
        let mut stream = fork.complete(&[], &[], "fixture", None).await.unwrap();
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
        let mut stream = fork.complete(&[], &[], "fixture", None).await.unwrap();
        let mut changed = false;
        while let Some(event) = stream.next().await {
            if matches!(
                event.unwrap(),
                StreamEvent::ConnectionPhase {
                    phase: jcode_message_types::ConnectionPhase::Retrying { .. }
                }
            ) {
                auth::claude::set_active_account(&second).unwrap();
                changed = true;
            }
        }
        assert!(changed);
        assert_eq!(
            server.await.unwrap(),
            ["Bearer first", "Bearer first", "Bearer second"]
        );
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), run)
        .await
        .unwrap();
}

#[tokio::test]
async fn live_credentials_waiting_for_the_cache_lock_observe_the_current_login() {
    let _env = TestEnv::new();
    let label = save("work", "first");
    let provider = Arc::new(offline_provider());
    for forced in [false, true] {
        save(&label, "first");
        provider.get_oauth_access_token().await.unwrap();
        let held = provider.credentials.write().await;
        let task_provider = provider.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            started_tx.send(()).unwrap();
            if forced {
                force_refresh_oauth_token(task_provider.credentials.clone(), &None).await
            } else {
                task_provider
                    .get_oauth_access_token()
                    .await
                    .map(|result| result.0)
            }
        });
        started_rx.await.unwrap();
        tokio::task::yield_now().await;
        save(&label, "replacement");
        // A regression must fail before contacting an OAuth endpoint.
        for token in ["first-refresh", "replacement-refresh"] {
            auth::refresh_state::record_permanent_rejection("claude", token, "synthetic rejection")
                .unwrap();
        }
        drop(held);
        assert_eq!(task.await.unwrap().unwrap(), "replacement");
    }
}

#[test]
fn live_credentials_rejected_stale_refresh_can_use_a_new_login_but_not_stale_or_invalid_credentials()
 {
    let _env = TestEnv::new();
    let label = save("work", "first");
    let source = ("first".into(), "first-refresh".into());
    assert_eq!(
        replacement_after_refresh_failure(&None, &source).unwrap(),
        None
    );
    save(&label, "replacement");
    assert_eq!(
        replacement_after_refresh_failure(&None, &source)
            .unwrap()
            .as_deref(),
        Some("replacement")
    );
    let mut saved = auth::claude::load_auth_file().unwrap();
    saved.anthropic_accounts[0].expires = 1;
    auth::claude::save_auth_file(&saved).unwrap();
    assert!(replacement_after_refresh_failure(&None, &source).is_err());
    saved.anthropic_accounts[0].expires = chrono::Utc::now().timestamp_millis() + 3_600_000;
    saved.anthropic_accounts[0].scopes = vec!["user:profile".into()];
    auth::claude::save_auth_file(&saved).unwrap();
    assert!(replacement_after_refresh_failure(&None, &source).is_err());
    auth::claude::remove_account(&label).unwrap();
    assert!(replacement_after_refresh_failure(&Some(label), &source).is_err());
}
