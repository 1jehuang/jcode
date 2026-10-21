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
    initial.active_anthropic_account = Some("work".into());
    save_auth_file(&initial).unwrap();
    let other = serde_json::to_value(&initial.anthropic_accounts[1]).unwrap();
    let replacement = account("unused-label", Some("new@example.com"));
    replace_account_if_unchanged("work", Some("old@example.com"), replacement.clone()).unwrap();
    let saved = load_auth_file().unwrap();
    assert_eq!(saved.anthropic_accounts.len(), 2);
    assert_eq!(saved.active_anthropic_account.as_deref(), Some("work"));
    assert_eq!(saved.anthropic_accounts[0].label, "work");
    assert_eq!(
        saved.anthropic_accounts[0].email.as_deref(),
        Some("new@example.com")
    );
    assert_eq!(saved.anthropic_accounts[0].access, replacement.access);
    assert_eq!(saved.anthropic_accounts[0].refresh, replacement.refresh);
    assert_eq!(
        serde_json::to_value(&saved.anthropic_accounts[1]).unwrap(),
        other
    );
    let path = jcode_path().unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(
        replace_account_if_unchanged(
            "work",
            Some("new@example.com"),
            account("unused", Some("OTHER@EXAMPLE.COM"))
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(
        replace_account_if_unchanged(
            "work",
            Some("old@example.com"),
            account("unused", Some("third@example.com"))
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(
        replace_account_if_unchanged(
            "missing",
            Some("old@example.com"),
            account("unused", Some("third@example.com"))
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

fn account(label: &str, email: Option<&str>) -> AnthropicAccount {
    AnthropicAccount {
        label: label.into(),
        access: format!("access-{label}"),
        refresh: format!("refresh-{label}"),
        expires: 100,
        email: email.map(str::to_string),
        subscription_type: None,
        scopes: vec![],
    }
}

fn fixture() -> JcodeAuthFile {
    JcodeAuthFile {
        anthropic_accounts: vec![
            account("work", Some("old@example.com")),
            account("personal", Some("other@example.com")),
        ],
        active_anthropic_account: Some("personal".into()),
        anthropic: None,
    }
}

#[test]
fn replacement_preserves_slot_default_and_other_account_and_clears_old_metadata() {
    let mut auth = fixture();
    auth.anthropic_accounts[0].subscription_type = Some("old-plan".into());
    auth.anthropic_accounts[0].scopes = vec!["old-scope".into()];
    let other = serde_json::to_value(&auth.anthropic_accounts[1]).unwrap();
    let mut replacement = account("ignored-new-label", Some("new@example.com"));
    replacement.scopes = vec!["new-scope".into()];
    replace_saved_login(&mut auth, "work", Some("old@example.com"), replacement).unwrap();
    assert_eq!(auth.anthropic_accounts.len(), 2);
    assert_eq!(auth.active_anthropic_account.as_deref(), Some("personal"));
    assert_eq!(auth.anthropic_accounts[0].label, "work");
    assert_eq!(
        auth.anthropic_accounts[0].email.as_deref(),
        Some("new@example.com")
    );
    assert_eq!(auth.anthropic_accounts[0].subscription_type, None);
    assert_eq!(auth.anthropic_accounts[0].scopes, vec!["new-scope"]);
    assert_eq!(
        serde_json::to_value(&auth.anthropic_accounts[1]).unwrap(),
        other
    );
}

#[test]
fn replacement_rejects_missing_changed_duplicate_and_unproven_identity_without_mutation() {
    for (label, expected, email) in [
        ("removed", Some("old@example.com"), Some("new@example.com")),
        ("work", Some("changed@example.com"), Some("new@example.com")),
        ("work", Some("old@example.com"), Some(" OTHER@EXAMPLE.COM ")),
        ("work", Some("old@example.com"), None),
        ("work", Some("old@example.com"), Some("  ")),
    ] {
        let mut auth = fixture();
        let before = serde_json::to_value(&auth).unwrap();
        assert!(replace_saved_login(&mut auth, label, expected, account("new", email)).is_err());
        assert_eq!(serde_json::to_value(&auth).unwrap(), before);
    }
}

#[test]
fn replacement_accepts_own_identity_without_duplicate_label() {
    let mut auth = fixture();
    replace_saved_login(
        &mut auth,
        "work",
        Some("old@example.com"),
        account("new", Some("OLD@EXAMPLE.COM")),
    )
    .unwrap();
    assert_eq!(auth.anthropic_accounts.len(), 2);
    assert_eq!(auth.anthropic_accounts[0].label, "work");
}
