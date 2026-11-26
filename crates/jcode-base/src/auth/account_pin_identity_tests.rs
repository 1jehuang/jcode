use super::*;

pub(crate) struct TestHome {
    _sandbox: crate::auth::test_sandbox::AuthTestSandbox,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}
impl TestHome {
    pub(crate) fn new() -> Self {
        let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
        let mut previous = Vec::new();
        for (key, value) in [
            ("HOME", sandbox.root().to_path_buf()),
            ("JCODE_GLOBAL_HOME", sandbox.root().join("global")),
            ("XDG_CONFIG_HOME", sandbox.root().join("xdg")),
            ("CLAUDE_CONFIG_DIR", sandbox.root().join("claude")),
        ] {
            previous.push((key, std::env::var_os(key)));
            crate::env::set_var(key, value);
        }
        previous.push(("JCODE_TEST_SESSION", std::env::var_os("JCODE_TEST_SESSION")));
        crate::env::set_var("JCODE_TEST_SESSION", "1");
        Self {
            _sandbox: sandbox,
            previous,
        }
    }
}
impl Drop for TestHome {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..) {
            match value {
                Some(value) => crate::env::set_var(key, value),
                None => crate::env::remove_var(key),
            }
        }
    }
}

fn claude_account(label: &str, refresh: &str) -> claude::AnthropicAccount {
    claude::AnthropicAccount {
        record_id: None,
        label: label.into(),
        access: "fixture-access".into(),
        refresh: refresh.into(),
        expires: i64::MAX,
        email: None,
        subscription_type: None,
        scopes: vec![],
    }
}

pub(crate) fn openai_account(label: &str, workspace: &str) -> String {
    codex::upsert_account(codex::OpenAiAccount {
        record_id: None,
        label: label.into(),
        access_token: format!("{workspace}-access"),
        refresh_token: format!("{workspace}-refresh"),
        id_token: None,
        account_id: Some(workspace.into()),
        expires_at: Some(i64::MAX),
        email: Some("same@example.test".into()),
    })
    .unwrap()
}

#[test]
fn unknown_account_pin_survives_refresh_and_identity_enrichment_but_rejects_relogin() {
    let _home = TestHome::new();
    let label = claude::upsert_account(claude_account("first", "original-refresh")).unwrap();
    let pin = claude_pin(&label).unwrap();
    assert!(pin.record_id.is_some());
    assert!(pin.identity.as_deref().unwrap().starts_with("rt:"));
    claude::update_account_tokens(&label, "rotated-access", "rotated-refresh", i64::MAX).unwrap();
    assert_eq!(load_claude(&pin).unwrap().0.access_token, "rotated-access");
    claude::update_account_profile(&label, Some("enriched@example.test".into())).unwrap();
    assert_eq!(resolve_claude(&pin).unwrap(), label);
    let enriched = normalize_claude_pin(&pin).unwrap();
    assert_eq!(enriched.record_id, pin.record_id);
    assert_eq!(enriched.identity.as_deref(), Some("enriched@example.test"));
    claude::upsert_account(claude_account(&label, "unrelated-login-refresh")).unwrap();
    assert!(resolve_claude(&pin).is_err());
    assert!(resolve_claude(&enriched).is_err());
}

#[test]
fn legacy_saved_records_get_one_persistent_record_id() {
    let _home = TestHome::new();
    crate::storage::write_json_secret(&claude::jcode_path().unwrap(), &serde_json::json!({
        "anthropic_accounts": [{"label":"claude-otter","access":"fixture","refresh":"fixture-refresh","expires":i64::MAX}],
        "active_anthropic_account":"claude-otter"
    })).unwrap();
    let first = claude::load_auth_file()
        .unwrap()
        .anthropic_accounts
        .remove(0);
    let second = claude::load_auth_file()
        .unwrap()
        .anthropic_accounts
        .remove(0);
    assert!(uuid::Uuid::parse_str(first.record_id.as_deref().unwrap()).is_ok());
    assert_eq!(first.record_id, second.record_id);
    let pin = AccountPin::new(&first.label, claude_identity(&first));
    let normalized = normalize_claude_pin(&pin).unwrap();
    assert_eq!(normalized.record_id, first.record_id);
}

#[test]
fn workspace_claim_beats_shared_email_and_legacy_email_is_ambiguous() {
    let _home = TestHome::new();
    let personal = openai_account("personal", "personal-workspace");
    let legacy = AccountPin::new(&personal, Some("SAME@example.test".into()));
    assert_eq!(resolve_openai(&legacy).unwrap(), personal);
    let personal_pin = openai_pin(&personal).unwrap();
    let work = openai_account("work", "work-workspace");
    codex::set_active_account(&work).unwrap();
    assert!(resolve_openai(&legacy).is_err());
    assert_eq!(
        personal_pin.identity.as_deref(),
        Some("id:personal-workspace")
    );
    let (credentials, label) = load_openai(&personal_pin).unwrap();
    assert_eq!(label, personal);
    assert_eq!(
        credentials.account_id.as_deref(),
        Some("personal-workspace")
    );
    let mut store = codex::load_auth_file().unwrap();
    store
        .openai_accounts
        .iter_mut()
        .find(|account| account.label == personal)
        .unwrap()
        .account_id = Some("other-workspace".into());
    codex::save_auth_file(&store).unwrap();
    assert!(resolve_openai(&personal_pin).is_err());
}

#[test]
fn identities_follow_relabeling_and_never_guess_an_ambiguous_record() {
    let _home = TestHome::new();
    let first = openai_account("first", "first-workspace");
    let second = openai_account("second", "second-workspace");
    let pin = openai_pin(&first).unwrap();
    let mut store = codex::load_auth_file().unwrap();
    store
        .openai_accounts
        .iter_mut()
        .find(|account| account.label == first)
        .unwrap()
        .label = "openai-panda".into();
    codex::save_auth_file(&store).unwrap();
    assert_eq!(resolve_openai(&pin).unwrap(), "openai-panda");
    let mut store = codex::load_auth_file().unwrap();
    store
        .openai_accounts
        .iter_mut()
        .find(|account| account.label == second)
        .unwrap()
        .record_id = pin.record_id.clone();
    codex::save_auth_file(&store).unwrap();
    assert!(resolve_openai(&pin).is_err());
}

#[test]
fn mismatched_weak_or_missing_identity_at_a_reused_label_is_rejected() {
    let accounts = [("same-label", Some("rt:new")), ("unknown", None)];
    for pin in [
        AccountPin::new("same-label", Some("rt:old".into())),
        AccountPin::new("unknown", Some("gone@example.test".into())),
    ] {
        assert!(
            super::super::account_store::resolve_pin(
                &pin,
                &accounts,
                |account| account.0,
                |account| account.1.map(str::to_owned),
                |_| None
            )
            .is_none()
        );
    }
}

#[test]
fn openai_email_without_workspace_does_not_replace_a_removed_record() {
    let _home = TestHome::new();
    let account = codex::OpenAiAccount {
        record_id: None,
        label: "unknown-workspace".into(),
        access_token: "original".into(),
        refresh_token: "original-refresh".into(),
        id_token: None,
        account_id: None,
        expires_at: Some(i64::MAX),
        email: Some("same@example.test".into()),
    };
    let label = codex::upsert_account(account.clone()).unwrap();
    let pin = openai_pin(&label).unwrap();
    let mut replacement = account;
    replacement.label = label.clone();
    replacement.access_token = "replacement".into();
    replacement.refresh_token = "replacement-refresh".into();
    codex::upsert_account(replacement).unwrap();
    assert!(resolve_openai(&pin).is_err());
    // A legacy session without a record ID retains its unique-email migration.
    assert_eq!(
        resolve_openai(&AccountPin::new(label.clone(), pin.identity)).unwrap(),
        label
    );
}

#[test]
fn cloud_export_keeps_the_captured_record_through_refresh_but_not_relogin() {
    use crate::auth::transfer::TransferProvider;
    let _home = TestHome::new();
    let mut first = claude_account("cloud-export", "first-refresh");
    first.email = Some("export@example.test".into());
    let label = claude::upsert_account(first).unwrap();
    let pin = claude_pin(&label).unwrap();
    claude::update_account_tokens(&label, "rotated", "rotated-refresh", i64::MAX).unwrap();
    assert!(export_cloud_pin(TransferProvider::Claude, &pin).is_ok());
    let mut replacement = claude_account(&label, "replacement-refresh");
    replacement.email = Some("export@example.test".into());
    claude::upsert_account(replacement).unwrap();
    assert_ne!(claude_pin(&label).unwrap().record_id, pin.record_id);
    assert!(export_cloud_pin(TransferProvider::Claude, &pin).is_err());
    assert_eq!(
        claude::load_credentials_for_account(&label)
            .unwrap()
            .refresh_token,
        "replacement-refresh"
    );
}

#[test]
fn claude_removed_record_cannot_follow_inherited_email_from_replacement_login() {
    let _home = TestHome::new();
    let mut account = claude_account("original", "original-refresh");
    account.email = Some("same@example.test".into());
    let label = claude::upsert_account(account).unwrap();
    let pin = claude_pin(&label).unwrap();
    assert!(pin.record_id.is_some());
    assert_eq!(pin.identity.as_deref(), Some("same@example.test"));

    crate::auth::oauth::save_claude_tokens_for_account(
        &crate::auth::oauth::OAuthTokens {
            access_token: "replacement-access".into(),
            refresh_token: "replacement-refresh".into(),
            expires_at: i64::MAX,
            id_token: None,
            scopes: vec![],
        },
        &label,
    )
    .unwrap();
    let replacement = claude_pin(&label).unwrap();
    assert_eq!(replacement.label, pin.label);
    assert_eq!(
        replacement.identity, pin.identity,
        "the real saver retains email"
    );
    assert_ne!(replacement.record_id, pin.record_id);
    assert_eq!(
        load_claude(&replacement).unwrap().0.access_token,
        "replacement-access"
    );
    assert_eq!(
        (
            resolve_claude(&pin).is_err(),
            load_claude(&pin).is_err(),
            normalize_claude_pin(&pin).is_err(),
        ),
        (true, true, true),
        "a removed local record must not resolve, load, or normalize via inherited email"
    );
}

#[test]
fn claude_record_refresh_rename_and_recordless_email_pins_keep_their_lineage() {
    let _home = TestHome::new();
    let mut account = claude_account("original", "original-refresh");
    account.email = Some("legacy@example.test".into());
    let label = claude::upsert_account(account).unwrap();
    let pin = claude_pin(&label).unwrap();
    let legacy = AccountPin::new(&label, pin.identity.clone());
    assert!(legacy.record_id.is_none());
    claude::update_account_tokens(&label, "rotated-access", "rotated-refresh", i64::MAX).unwrap();
    let mut store = claude::load_auth_file().unwrap();
    store.anthropic_accounts[0].label = "claude-panda".into();
    claude::save_auth_file(&store).unwrap();
    for retained in [&pin, &legacy] {
        assert_eq!(resolve_claude(retained).unwrap(), "claude-panda");
        let (credentials, label) = load_claude(retained).unwrap();
        assert_eq!(label, "claude-panda");
        assert_eq!(credentials.access_token, "rotated-access");
        assert_eq!(
            normalize_claude_pin(retained).unwrap().record_id,
            pin.record_id
        );
    }
    let mut duplicate = claude_account("other", "other-refresh");
    duplicate.email = Some("legacy@example.test".into());
    claude::upsert_account(duplicate).unwrap();
    assert!(
        resolve_claude(&legacy).is_err(),
        "legacy email must remain unique"
    );
    assert_eq!(resolve_claude(&pin).unwrap(), "claude-panda");
}
