//! Per-session account pins: scoped credential loading (design S1).

use super::test_sandbox::AuthTestSandbox;
use super::{AccountPin, AccountScope, claude, codex, oauth};

const FAR_FUTURE_MS: i64 = 4_102_444_800_000;

fn claude_account(
    label: &str,
    access: &str,
    refresh: &str,
    email: &str,
) -> claude::AnthropicAccount {
    claude::AnthropicAccount {
        label: label.to_string(),
        access: access.to_string(),
        refresh: refresh.to_string(),
        expires: FAR_FUTURE_MS,
        email: Some(email.to_string()),
        subscription_type: Some("max".to_string()),
        scopes: Vec::new(),
    }
}

fn seed_two_claude_accounts() {
    let mut auth = claude::JcodeAuthFile::default();
    auth.anthropic_accounts = vec![
        claude_account(
            "claude-otter",
            "otter-access",
            "otter-refresh",
            "otter@example.com",
        ),
        claude_account("claude-fox", "fox-access", "fox-refresh", "fox@example.com"),
    ];
    auth.active_anthropic_account = Some("claude-otter".to_string());
    claude::save_auth_file(&auth).expect("seed claude accounts");
}

fn seed_two_openai_accounts(expires_at: i64) {
    let account = |label: &str, access: &str, refresh: &str, email: &str| codex::OpenAiAccount {
        label: label.to_string(),
        access_token: access.to_string(),
        refresh_token: refresh.to_string(),
        id_token: None,
        account_id: Some(format!("acct-{label}")),
        expires_at: Some(expires_at),
        email: Some(email.to_string()),
    };
    codex::save_auth_file(&codex::JcodeOpenAiAuthFile {
        openai_accounts: vec![
            account("openai-otter", "o-access", "o-refresh", "otter@example.com"),
            account("openai-fox", "f-access", "f-refresh", "fox@example.com"),
        ],
        active_openai_account: Some("openai-otter".to_string()),
    })
    .expect("seed openai accounts");
}

#[test]
fn scoped_load_returns_pinned_account_even_when_override_points_elsewhere() {
    let _sandbox = AuthTestSandbox::new().expect("sandbox");
    seed_two_claude_accounts();
    claude::set_active_account_override(Some("claude-otter".to_string()));
    codex::set_active_account_override(None);
    seed_two_openai_accounts(FAR_FUTURE_MS);
    codex::set_active_account_override(Some("openai-otter".to_string()));

    let fox = claude::pin_for_label("claude-fox").expect("pin fox");
    let (creds, label) =
        claude::load_credentials_scoped(AccountScope::Pinned(&fox)).expect("scoped claude");
    claude::set_active_account_override(None);
    assert_eq!(creds.access_token, "fox-access");
    assert_eq!(label.as_deref(), Some("claude-fox"));

    let ofox = codex::pin_for_label("openai-fox").expect("pin openai fox");
    let (creds, label) =
        codex::load_oauth_credentials_scoped(AccountScope::Pinned(&ofox)).expect("scoped openai");
    codex::set_active_account_override(None);
    assert_eq!(creds.access_token, "f-access");
    assert_eq!(label.as_deref(), Some("openai-fox"));
}

#[test]
fn default_scope_ignores_runtime_override_for_default_label() {
    let _sandbox = AuthTestSandbox::new().expect("sandbox");
    seed_two_claude_accounts();
    seed_two_openai_accounts(FAR_FUTURE_MS);
    claude::set_active_account_override(Some("claude-fox".to_string()));
    codex::set_active_account_override(Some("openai-fox".to_string()));

    let claude_default = claude::default_account_label();
    let openai_default = codex::default_account_label();
    // Legacy global view still honours the one-shot override.
    let claude_active = claude::active_account_label();
    claude::set_active_account_override(None);
    codex::set_active_account_override(None);

    assert_eq!(claude_default.as_deref(), Some("claude-otter"));
    assert_eq!(openai_default.as_deref(), Some("openai-otter"));
    assert_eq!(claude_active.as_deref(), Some("claude-fox"));
}

#[test]
fn pin_survives_relabel_after_first_account_removed() {
    let _sandbox = AuthTestSandbox::new().expect("sandbox");
    seed_two_claude_accounts();
    seed_two_openai_accounts(FAR_FUTURE_MS);

    let fox = claude::pin_for_label("claude-fox").expect("pin fox");
    assert_eq!(fox.identity.as_deref(), Some("fox@example.com"));
    let ofox = codex::pin_for_label("openai-fox").expect("pin openai fox");

    // Removing otter relabels fox to claude-otter on the next load.
    claude::remove_account("claude-otter").expect("remove otter");
    codex::remove_account("openai-otter").expect("remove openai otter");
    let labels: Vec<String> = claude::list_accounts()
        .expect("list")
        .into_iter()
        .map(|a| a.label)
        .collect();
    assert_eq!(labels, vec!["claude-otter".to_string()]);

    assert_eq!(claude::resolve_pin(&fox).as_deref(), Some("claude-otter"));
    let (creds, label) =
        claude::load_credentials_scoped(AccountScope::Pinned(&fox)).expect("scoped fox");
    assert_eq!(creds.access_token, "fox-access");
    assert_eq!(label.as_deref(), Some("claude-otter"));

    assert_eq!(codex::resolve_pin(&ofox).as_deref(), Some("openai-otter"));
    let (creds, _) =
        codex::load_oauth_credentials_scoped(AccountScope::Pinned(&ofox)).expect("scoped ofox");
    assert_eq!(creds.access_token, "f-access");

    // A pin whose identity is gone does not silently fall back to its label.
    let ghost = AccountPin::new("claude-otter", Some("gone@example.com".to_string()));
    assert_eq!(claude::resolve_pin(&ghost), None);
    assert!(claude::load_credentials_scoped(AccountScope::Pinned(&ghost)).is_err());
}

#[test]
fn pinned_scope_skips_trusted_claude_code_file() {
    let _sandbox = AuthTestSandbox::new().expect("sandbox");
    seed_two_claude_accounts();

    // A trusted, valid Claude Code login wins for the Default scope.
    let path = claude::ExternalClaudeAuthSource::ClaudeCode
        .path()
        .expect("claude code path");
    std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
    std::fs::write(
        &path,
        format!(
            r#"{{"claudeAiOauth":{{"accessToken":"cc-access","refreshToken":"cc-refresh","expiresAt":{FAR_FUTURE_MS}}}}}"#
        ),
    )
    .expect("write claude code creds");
    claude::trust_external_auth_source(claude::ExternalClaudeAuthSource::ClaudeCode)
        .expect("trust claude code");

    let (default_creds, _) =
        claude::load_credentials_scoped(AccountScope::Default).expect("default scope");
    assert_eq!(default_creds.access_token, "cc-access");
    assert_eq!(
        claude::load_credentials()
            .expect("legacy load")
            .access_token,
        "cc-access",
        "Default scope must match load_credentials()"
    );

    let otter = claude::pin_for_label("claude-otter").expect("pin otter");
    let (pinned, label) =
        claude::load_credentials_scoped(AccountScope::Pinned(&otter)).expect("pinned scope");
    assert_eq!(pinned.access_token, "otter-access");
    assert_eq!(label.as_deref(), Some("claude-otter"));
}

async fn one_shot_token_server(body: String) -> (String, tokio::task::JoinHandle<String>) {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/token", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }
            if let Some((k, v)) = trimmed.split_once(':')
                && k.trim().eq_ignore_ascii_case("content-length")
            {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
        let mut request_body = vec![0u8; content_length];
        reader.read_exact(&mut request_body).await.unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        writer.write_all(response.as_bytes()).await.unwrap();
        String::from_utf8(request_body).unwrap_or_default()
    });
    (url, handle)
}

#[tokio::test(flavor = "current_thread")]
async fn refresh_for_pinned_label_persists_to_that_label_only() {
    let _sandbox = AuthTestSandbox::new().expect("sandbox");
    seed_two_claude_accounts();
    seed_two_openai_accounts(1);

    // Claude: refreshing under a fox pin must write fox, not the default otter.
    let (url, server) = one_shot_token_server(
        r#"{"access_token":"fox-new","refresh_token":"fox-refresh-2","expires_in":3600}"#
            .to_string(),
    )
    .await;
    oauth::set_token_url_override_for_tests("claude", Some(url));
    let fox = claude::pin_for_label("claude-fox").expect("pin fox");
    let result =
        oauth::refresh_claude_tokens_scoped("fox-refresh", AccountScope::Pinned(&fox)).await;
    oauth::set_token_url_override_for_tests("claude", None);
    result.expect("claude refresh");
    let request = tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .expect("refresh must hit the token endpoint")
        .unwrap();
    assert!(request.contains("fox-refresh"), "request body: {request}");
    let accounts = claude::list_accounts().unwrap();
    let otter = accounts.iter().find(|a| a.label == "claude-otter").unwrap();
    let fox_acc = accounts.iter().find(|a| a.label == "claude-fox").unwrap();
    assert_eq!(otter.access, "otter-access");
    assert_eq!(fox_acc.access, "fox-new");
    assert_eq!(fox_acc.refresh, "fox-refresh-2");

    // OpenAI: same for the codex store.
    let (url, server) = one_shot_token_server(
        r#"{"access_token":"of-new","refresh_token":"f-refresh-2","expires_in":3600}"#.to_string(),
    )
    .await;
    oauth::set_token_url_override_for_tests("openai", Some(url));
    let ofox = codex::pin_for_label("openai-fox").expect("pin openai fox");
    let result =
        oauth::refresh_openai_tokens_scoped("f-refresh", AccountScope::Pinned(&ofox)).await;
    oauth::set_token_url_override_for_tests("openai", None);
    result.expect("openai refresh");
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .expect("refresh must hit the token endpoint")
        .unwrap();
    let accounts = codex::list_accounts().unwrap();
    let otter = accounts.iter().find(|a| a.label == "openai-otter").unwrap();
    let fox_acc = accounts.iter().find(|a| a.label == "openai-fox").unwrap();
    assert_eq!(otter.access_token, "o-access");
    assert_eq!(fox_acc.access_token, "of-new");
}
