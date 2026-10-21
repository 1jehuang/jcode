use super::*;
use crate::auth::codex::{self, OpenAiAccount};

struct TestHome {
    previous: Option<std::ffi::OsString>,
    _temp: tempfile::TempDir,
}

impl TestHome {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());
        codex::set_active_account_override(None);
        Self {
            previous,
            _temp: temp,
        }
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        codex::set_active_account_override(None);
        if let Some(previous) = &self.previous {
            crate::env::set_var("JCODE_HOME", previous);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}

fn login(label: &str, identity: &str, refresh: &str) -> OpenAiAccount {
    OpenAiAccount {
        label: label.into(),
        access_token: format!("access-{identity}"),
        refresh_token: refresh.into(),
        id_token: None,
        account_id: Some(identity.into()),
        expires_at: Some(1),
        email: Some(format!("{identity}@example.invalid")),
    }
}

fn rotated() -> OAuthTokens {
    OAuthTokens {
        access_token: "rotated-access".into(),
        refresh_token: "rotated-refresh".into(),
        expires_at: 9000,
        id_token: None,
        scopes: Vec::new(),
    }
}

// The request future is polled only AFTER the production snapshot capture.
// Channels order mutation before response completion, with no HTTP or sleeps.
async fn delayed_refresh(
    label: String,
) -> (
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<Result<OAuthTokens>>,
) {
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        refresh_openai_tokens_with("original-refresh", Some(&label), async move {
            started_tx.send(()).unwrap();
            release_rx.await.unwrap();
            Ok(rotated())
        })
        .await
    });
    started_rx.await.unwrap();
    (release_tx, task)
}

#[tokio::test]
async fn delayed_refresh_rejects_same_label_replacement_without_writing() {
    let _lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let label = codex::upsert_account(login("work", "original", "original-refresh")).unwrap();
    let (release, task) = delayed_refresh(label.clone()).await;
    let identity = codex::pin_for_label(&label).unwrap().identity;
    codex::replace_account_if_unchanged(
        &label,
        identity.as_deref(),
        login(&label, "replacement", "replacement-refresh"),
    )
    .unwrap();
    let path = crate::storage::jcode_dir()
        .unwrap()
        .join("openai-auth.json");
    let before = std::fs::read(&path).unwrap();
    release.send(()).unwrap();
    let error = task.await.unwrap().unwrap_err().to_string();
    assert!(error.contains("login changed"));
    assert!(!error.contains("original-refresh"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let saved = codex::load_credentials_for_account(&label).unwrap();
    assert_eq!(saved.refresh_token, "replacement-refresh");
    assert_eq!(saved.account_id.as_deref(), Some("replacement"));
}

#[tokio::test]
async fn delayed_refresh_follows_external_rename_not_reused_label() {
    let _lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let label = codex::upsert_account(login("work", "original", "original-refresh")).unwrap();
    let (release, task) = delayed_refresh(label.clone()).await;
    // Persisted external rename: do not migrate the in-process override.
    let mut auth = codex::load_auth_file().unwrap();
    auth.openai_accounts[0].label = "renamed-work".into();
    auth.active_openai_account = Some("renamed-work".into());
    auth.openai_accounts
        .push(login(&label, "replacement", "replacement-refresh"));
    codex::save_auth_file(&auth).unwrap();
    release.send(()).unwrap();
    task.await.unwrap().unwrap();
    let auth = codex::load_auth_file().unwrap();
    assert_eq!(auth.active_openai_account.as_deref(), Some("renamed-work"));
    assert_eq!(auth.openai_accounts.len(), 2);
    assert_eq!(auth.openai_accounts[0].label, "renamed-work");
    assert_eq!(auth.openai_accounts[0].refresh_token, "rotated-refresh");
    assert_eq!(
        auth.openai_accounts[0].account_id.as_deref(),
        Some("original")
    );
    assert_eq!(
        auth.openai_accounts[0].email.as_deref(),
        Some("original@example.invalid")
    );
    assert_eq!(auth.openai_accounts[1].refresh_token, "replacement-refresh");
}

#[tokio::test]
async fn delayed_refresh_normal_success_preserves_identity_and_default() {
    let _lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let label = codex::upsert_account(login("work", "original", "original-refresh")).unwrap();
    let (release, task) = delayed_refresh(label.clone()).await;
    release.send(()).unwrap();
    assert_eq!(task.await.unwrap().unwrap().access_token, "rotated-access");
    let auth = codex::load_auth_file().unwrap();
    assert_eq!(auth.active_openai_account.as_deref(), Some(label.as_str()));
    assert_eq!(auth.openai_accounts.len(), 1);
    let account = &auth.openai_accounts[0];
    assert_eq!(account.refresh_token, "rotated-refresh");
    assert_eq!(account.expires_at, Some(9000));
    assert_eq!(account.account_id.as_deref(), Some("original"));
    assert_eq!(account.email.as_deref(), Some("original@example.invalid"));
}

#[tokio::test]
async fn cancelled_caller_refresh_does_not_overwrite_replacement() {
    let _lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let label = codex::upsert_account(login("work", "original", "original-refresh")).unwrap();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    let caller_label = label.clone();
    let caller = tokio::spawn(async move {
        run_refresh_transaction(async move {
            let result =
                refresh_openai_tokens_with("original-refresh", Some(&caller_label), async {
                    started_tx.send(()).unwrap();
                    release_rx.await.unwrap();
                    Ok(rotated())
                })
                .await;
            finished_tx.send(result.is_err()).unwrap();
            result
        })
        .await
    });
    started_rx.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    codex::upsert_account(login(&label, "replacement", "replacement-refresh")).unwrap();
    release_tx.send(()).unwrap();
    assert!(finished_rx.await.unwrap());
    assert_eq!(
        codex::load_credentials_for_account(&label)
            .unwrap()
            .refresh_token,
        "replacement-refresh"
    );
}

#[tokio::test]
async fn changed_login_is_rejected_before_request_future_is_polled() {
    let _lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let label = codex::upsert_account(login("work", "replacement", "replacement-refresh")).unwrap();
    let polled = std::sync::atomic::AtomicBool::new(false);
    let result = refresh_openai_tokens_with("original-refresh", Some(&label), async {
        polled.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(rotated())
    })
    .await;
    assert!(!polled.load(std::sync::atomic::Ordering::SeqCst));
    assert!(result.unwrap_err().to_string().contains("login changed"));
    assert_eq!(
        codex::load_credentials_for_account(&label)
            .unwrap()
            .refresh_token,
        "replacement-refresh"
    );
}
