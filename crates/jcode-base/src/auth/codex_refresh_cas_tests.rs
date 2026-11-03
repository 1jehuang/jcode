use super::*;
use crate::auth::oauth::OAuthTokens;

struct HomeGuard(Option<std::ffi::OsString>);

impl HomeGuard {
    fn set(path: &std::path::Path) -> Self {
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", path);
        Self(previous)
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.0 {
            crate::env::set_var("JCODE_HOME", previous);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}

fn account() -> OpenAiAccount {
    OpenAiAccount {
        label: "work".into(),
        access_token: "old-access".into(),
        refresh_token: "old-refresh".into(),
        id_token: Some("old-id-token".into()),
        account_id: Some("workspace-original".into()),
        expires_at: Some(1),
        email: Some("original@example.invalid".into()),
    }
}

fn response() -> OAuthTokens {
    OAuthTokens {
        access_token: "new-access".into(),
        refresh_token: "new-refresh".into(),
        id_token: None,
        expires_at: 42,
        scopes: Vec::new(),
    }
}

fn snapshot(account: &OpenAiAccount) -> RefreshAccount {
    RefreshAccount {
        refresh_token: account.refresh_token.clone(),
        identity: account_identity(account),
    }
}

#[test]
fn refresh_cas_rejects_removed_rotated_reidentified_and_ambiguous_login() {
    let original = account();
    let expected = snapshot(&original);
    let mut rotated = original.clone();
    rotated.refresh_token = "replacement-refresh".into();
    let mut reidentified = original.clone();
    reidentified.account_id = Some("replacement-workspace".into());
    let mut duplicate = original.clone();
    duplicate.label = "duplicate".into();
    for accounts in [
        vec![],
        vec![rotated],
        vec![reidentified],
        vec![original.clone(), duplicate],
    ] {
        let mut auth = JcodeOpenAiAuthFile {
            openai_accounts: accounts,
            active_openai_account: Some("work".into()),
        };
        let before = serde_json::to_vec(&auth).unwrap();
        assert!(apply_refresh(&mut auth, &expected, &response()).is_err());
        assert_eq!(serde_json::to_vec(&auth).unwrap(), before);
    }
}

#[test]
fn refresh_cas_rename_preserves_metadata_id_token_and_default() {
    let mut original = account();
    let expected = snapshot(&original);
    original.label = "renamed".into();
    let mut auth = JcodeOpenAiAuthFile {
        openai_accounts: vec![original],
        active_openai_account: Some("renamed".into()),
    };
    apply_refresh(&mut auth, &expected, &response()).unwrap();
    let saved = &auth.openai_accounts[0];
    assert_eq!(saved.label, "renamed");
    assert_eq!(saved.id_token.as_deref(), Some("old-id-token"));
    assert_eq!(saved.account_id.as_deref(), Some("workspace-original"));
    assert_eq!(saved.email.as_deref(), Some("original@example.invalid"));
    assert_eq!(saved.refresh_token, "new-refresh");
    assert_eq!(auth.active_openai_account.as_deref(), Some("renamed"));
}

#[test]
fn refresh_cas_without_claims_uses_original_refresh_fingerprint() {
    let mut original = account();
    original.account_id = None;
    original.email = None;
    original.id_token = None;
    let expected = snapshot(&original);
    original.label = "renamed".into();
    let mut auth = JcodeOpenAiAuthFile {
        openai_accounts: vec![original],
        active_openai_account: None,
    };
    apply_refresh(&mut auth, &expected, &response()).unwrap();
    assert_eq!(auth.openai_accounts[0].label, "renamed");
    assert_eq!(auth.openai_accounts[0].refresh_token, "new-refresh");
}

#[test]
fn refresh_cas_rejects_response_for_different_workspace_without_mutation() {
    let original = account();
    let expected = snapshot(&original);
    let mut auth = JcodeOpenAiAuthFile {
        openai_accounts: vec![original],
        active_openai_account: Some("work".into()),
    };
    let before = serde_json::to_vec(&auth).unwrap();
    let mut tokens = response();
    let claims = serde_json::json!({
        "https://api.openai.com/auth": { "chatgpt_account_id": "different-workspace" }
    });
    tokens.id_token = Some(format!(
        "header.{}.signature",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
    ));
    assert!(apply_refresh(&mut auth, &expected, &tokens).is_err());
    assert_eq!(serde_json::to_vec(&auth).unwrap(), before);
}

// Runs only as a child of the next test, with a synthetic JCODE_HOME.
#[test]
fn refresh_cas_cross_process_writer_child() {
    if std::env::var_os("JCODE_TEST_REFRESH_CAS_CHILD").is_none() {
        return;
    }
    let path = jcode_auth_path().unwrap().with_extension("lock");
    let probe = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    assert!(matches!(
        probe.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    println!("CAS_LOCK_HELD");
    use std::io::Write;
    std::io::stdout().flush().unwrap();
    // This must read/compare/write under the same sidecar lock as refresh.
    replace_account_if_unchanged(
        "work",
        Some("id:workspace-original"),
        OpenAiAccount {
            account_id: Some("workspace-replacement".into()),
            refresh_token: "replacement-refresh".into(),
            ..account()
        },
    )
    .unwrap();
}

#[test]
fn refresh_cas_sidecar_lock_coordinates_cross_process_replacement() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = HomeGuard::set(temp.path());
    save_auth_file(&JcodeOpenAiAuthFile {
        openai_accounts: vec![account()],
        active_openai_account: Some("work".into()),
    })
    .unwrap();
    let lock = lock_auth_file().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "auth::codex::refresh_cas::tests::refresh_cas_cross_process_writer_child",
            "--nocapture",
        ])
        .env("JCODE_TEST_REFRESH_CAS_CHILD", "1")
        .env("JCODE_HOME", temp.path())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::BufRead;
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert_ne!(
            stdout.read_line(&mut line).unwrap(),
            0,
            "child must observe held lock"
        );
        if line.contains("CAS_LOCK_HELD") {
            break;
        }
    }
    // The child has verified exclusion while a refresh transaction owns the lock.
    let mut auth = load_auth_file_unlocked().unwrap();
    apply_refresh(&mut auth, &snapshot(&account()), &response()).unwrap();
    save_auth_file_unlocked(&auth).unwrap();
    drop(lock);
    assert!(child.wait().unwrap().success());
    let auth = load_auth_file().unwrap();
    assert_eq!(auth.openai_accounts[0].refresh_token, "replacement-refresh");
    assert_eq!(
        auth.openai_accounts[0].account_id.as_deref(),
        Some("workspace-replacement")
    );
}
