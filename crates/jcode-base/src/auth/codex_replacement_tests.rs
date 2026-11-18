use super::*;

struct TestHome(Option<std::ffi::OsString>);

impl Drop for TestHome {
    fn drop(&mut self) {
        match self.0.as_ref() {
            Some(previous) => crate::env::set_var("JCODE_HOME", previous),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }
}

#[test]
fn replacement_public_api_roundtrip_preserves_defaults_and_rejections_preserve_disk_bytes() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = TestHome(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", temp.path());
    let mut initial = fixture();
    initial.active_openai_account = Some("work".into());
    save_auth_file(&initial).unwrap();
    let other = serde_json::to_value(&initial.openai_accounts[1]).unwrap();
    let replacement = account("unused-label", Some("new-id"), Some("new@example.com"));
    replace_account_if_unchanged("work", Some("id:old-id"), replacement.clone()).unwrap();
    let saved = load_auth_file().unwrap();
    assert_eq!(saved.openai_accounts.len(), 2);
    assert_eq!(saved.active_openai_account.as_deref(), Some("work"));
    assert_eq!(saved.openai_accounts[0].label, "work");
    assert_eq!(
        saved.openai_accounts[0].account_id.as_deref(),
        Some("new-id")
    );
    assert_eq!(
        saved.openai_accounts[0].email.as_deref(),
        Some("new@example.com")
    );
    assert_eq!(
        saved.openai_accounts[0].access_token,
        replacement.access_token
    );
    assert_eq!(
        saved.openai_accounts[0].refresh_token,
        replacement.refresh_token
    );
    assert_eq!(
        serde_json::to_value(&saved.openai_accounts[1]).unwrap(),
        other
    );
    let path = jcode_auth_path().unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(
        replace_account_if_unchanged(
            "work",
            Some("id:new-id"),
            account("unused", Some("other-id"), None)
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(
        replace_account_if_unchanged(
            "work",
            Some("id:old-id"),
            account("unused", Some("third-id"), None)
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(
        replace_account_if_unchanged(
            "missing",
            Some("id:old-id"),
            account("unused", Some("third-id"), None)
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

fn account(label: &str, id: Option<&str>, email: Option<&str>) -> OpenAiAccount {
    OpenAiAccount {
        label: label.into(),
        access_token: format!("access-{label}"),
        refresh_token: format!("refresh-{label}"),
        id_token: None,
        account_id: id.map(str::to_string),
        expires_at: Some(100),
        email: email.map(str::to_string),
    }
}

fn fixture() -> JcodeOpenAiAuthFile {
    JcodeOpenAiAuthFile {
        openai_accounts: vec![
            account("work", Some("old-id"), Some("old@example.com")),
            account("personal", Some("other-id"), Some("other@example.com")),
        ],
        active_openai_account: Some("personal".into()),
    }
}

#[test]
fn replacement_preserves_slot_default_and_other_account_and_drops_old_identity_fields() {
    let mut auth = fixture();
    let other = serde_json::to_value(&auth.openai_accounts[1]).unwrap();
    replace_saved_login(
        &mut auth,
        "work",
        Some("id:old-id"),
        account("ignored", None, Some("new@example.com")),
    )
    .unwrap();
    assert_eq!(auth.openai_accounts.len(), 2);
    assert_eq!(auth.active_openai_account.as_deref(), Some("personal"));
    assert_eq!(auth.openai_accounts[0].label, "work");
    assert_eq!(auth.openai_accounts[0].account_id, None);
    assert_eq!(auth.openai_accounts[0].id_token, None);
    assert_eq!(
        auth.openai_accounts[0].email.as_deref(),
        Some("new@example.com")
    );
    assert_eq!(
        serde_json::to_value(&auth.openai_accounts[1]).unwrap(),
        other
    );
}

#[test]
fn replacement_rejects_missing_changed_duplicate_and_unproven_identity_without_mutation() {
    for (label, expected, id, email) in [
        ("removed", Some("id:old-id"), Some("new-id"), None),
        ("work", Some("id:changed-id"), Some("new-id"), None),
        (
            "work",
            Some("id:old-id"),
            Some("other-id"),
            Some("different@example.com"),
        ),
        ("work", Some("id:old-id"), None, None),
        ("work", Some("id:old-id"), Some(" "), Some(" ")),
    ] {
        let mut auth = fixture();
        let before = serde_json::to_value(&auth).unwrap();
        assert!(
            replace_saved_login(&mut auth, label, expected, account("new", id, email)).is_err()
        );
        assert_eq!(serde_json::to_value(&auth).unwrap(), before);
    }
}

#[test]
fn replacement_distinguishes_workspaces_for_same_email() {
    let mut auth = fixture();
    replace_saved_login(
        &mut auth,
        "work",
        Some("id:old-id"),
        account("new", Some("new-workspace"), Some("other@example.com")),
    )
    .unwrap();
    assert_eq!(
        auth.openai_accounts[0].account_id.as_deref(),
        Some("new-workspace")
    );
    assert_eq!(auth.openai_accounts.len(), 2);
}

#[test]
fn replacement_rejects_email_duplicate_when_neither_account_has_workspace_id() {
    let mut auth = fixture();
    auth.openai_accounts[1].account_id = None;
    let before = serde_json::to_value(&auth).unwrap();
    assert!(
        replace_saved_login(
            &mut auth,
            "work",
            Some("id:old-id"),
            account("new", None, Some("OTHER@EXAMPLE.COM"))
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&auth).unwrap(), before);
}
