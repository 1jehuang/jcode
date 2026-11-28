use super::*;

struct ImportFixture {
    _sandbox: crate::auth::test_sandbox::AuthTestSandbox,
    previous_override: Option<String>,
    env: Vec<(&'static str, Option<std::ffi::OsString>)>,
}
impl ImportFixture {
    fn new() -> Self {
        let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
        let previous_override = get_active_account_override();
        set_active_account_override(None);
        let values = [
            ("HOME", sandbox.root().as_os_str().to_os_string()),
            (
                "JCODE_GLOBAL_HOME",
                sandbox.root().join("global").into_os_string(),
            ),
            (
                "CLAUDE_CONFIG_DIR",
                sandbox.root().join("claude").into_os_string(),
            ),
            ("JCODE_TEST_SESSION", "1".into()),
            (
                CLAUDE_CODE_OAUTH_TOKEN_ENV,
                "fixture-environment-only-token".into(),
            ),
        ];
        let env = values
            .into_iter()
            .map(|(key, value)| {
                let old = std::env::var_os(key);
                crate::env::set_var(key, value);
                (key, old)
            })
            .collect();
        Self {
            _sandbox: sandbox,
            previous_override,
            env,
        }
    }
}
impl Drop for ImportFixture {
    fn drop(&mut self) {
        set_active_account_override(self.previous_override.take());
        for (key, value) in self.env.drain(..) {
            match value {
                Some(value) => crate::env::set_var(key, value),
                None => crate::env::remove_var(key),
            }
        }
    }
}
fn snapshot() -> ClaudeCredentials {
    ClaudeCredentials {
        access_token: "fixture-native-access".into(),
        refresh_token: "fixture-native-refresh".into(),
        expires_at: 4102444800000,
        scopes: vec!["user:inference".into()],
        subscription_type: Some("max".into()),
    }
}
fn saved(label: &str) -> AnthropicAccount {
    AnthropicAccount {
        label: label.into(),
        access: format!("{label}-access"),
        refresh: format!("{label}-refresh"),
        expires: 4102444800000,
        email: Some(format!("{label}@example.invalid")),
        subscription_type: Some("pro".into()),
        scopes: vec!["user:inference".into()],
    }
}

#[test]
fn native_snapshot_import_preserves_existing_accounts_and_active_selection() {
    let _env = ImportFixture::new();
    let first = upsert_account(saved("personal")).unwrap();
    let second = upsert_account(saved("team")).unwrap();
    set_active_account(&second).unwrap();
    let before = load_auth_file().unwrap();
    let label = import_native_credentials_into_account_from(snapshot()).unwrap();
    let after = load_auth_file().unwrap();
    assert_eq!(after.anthropic_accounts.len(), 3);
    assert_ne!(label, first);
    assert_ne!(label, second);
    assert_eq!(
        after.active_anthropic_account,
        before.active_anthropic_account
    );
    for previous in &before.anthropic_accounts {
        let retained = after
            .anthropic_accounts
            .iter()
            .find(|a| a.label == previous.label)
            .unwrap();
        assert_eq!(
            serde_json::to_value(retained).unwrap(),
            serde_json::to_value(previous).unwrap()
        );
    }
    let imported = after
        .anthropic_accounts
        .iter()
        .find(|a| a.label == label)
        .unwrap();
    assert_eq!(imported.access, "fixture-native-access");
    assert_eq!(imported.scopes, vec!["user:inference"]);
}

#[test]
fn repeated_native_snapshot_reuses_existing_record_without_overwriting_metadata() {
    let _env = ImportFixture::new();
    let label = import_native_credentials_into_account_from(snapshot()).unwrap();
    let mut auth = load_auth_file().unwrap();
    auth.anthropic_accounts[0].email = Some("retained@example.invalid".into());
    auth.anthropic_accounts[0].subscription_type = Some("hydrated-plan".into());
    save_auth_file(&auth).unwrap();
    let before = std::fs::read(jcode_path().unwrap()).unwrap();
    assert_eq!(
        import_native_credentials_into_account_from(snapshot()).unwrap(),
        label
    );
    assert_eq!(std::fs::read(jcode_path().unwrap()).unwrap(), before);
    assert_eq!(list_accounts().unwrap().len(), 1);
}

#[test]
fn native_snapshot_label_allocation_skips_live_labels_and_preserves_runtime_choice() {
    let _env = ImportFixture::new();
    let one = crate::auth::account_store::canonical_account_label(ACCOUNT_LABEL_PREFIX, 1);
    let three = crate::auth::account_store::canonical_account_label(ACCOUNT_LABEL_PREFIX, 3);
    let chosen = "my-custom-label".to_string();
    save_auth_file(&JcodeAuthFile {
        anthropic_accounts: vec![saved(&one), saved(&three), saved(&chosen)],
        active_anthropic_account: Some(chosen.clone()),
        anthropic: None,
    })
    .unwrap();
    set_active_account_override(Some(chosen.clone()));
    let label = import_native_credentials_into_account_from(snapshot()).unwrap();
    assert_eq!(
        label,
        crate::auth::account_store::canonical_account_label(ACCOUNT_LABEL_PREFIX, 2)
    );
    assert_eq!(active_account_label(), Some(chosen.clone()));
    assert_eq!(get_active_account_override(), Some(chosen));
    assert_eq!(list_accounts().unwrap().len(), 4);
}

#[test]
fn first_native_snapshot_becomes_default_and_public_entry_reads_one_staged_env_snapshot() {
    let _env = ImportFixture::new();
    let creds = snapshot();
    crate::env::set_var(CLAUDE_CODE_OAUTH_TOKEN_ENV,serde_json::json!({"claudeAiOauth":{
        "accessToken":creds.access_token,"refreshToken":creds.refresh_token,"expiresAt":creds.expires_at,
        "scopes":creds.scopes,"subscriptionType":creds.subscription_type
    }}).to_string());
    let label = import_native_credentials_into_account().unwrap();
    assert_eq!(active_account_label(), Some(label.clone()));
    assert_eq!(
        load_credentials_for_account(&label).unwrap().access_token,
        "fixture-native-access"
    );
    assert_eq!(import_native_credentials_into_account().unwrap(), label);
    assert_eq!(list_accounts().unwrap().len(), 1);
}

#[test]
fn environment_only_native_token_creates_no_saved_account() {
    let _env = ImportFixture::new();
    let label = upsert_account(saved("preserved")).unwrap();
    let before = std::fs::read(jcode_path().unwrap()).unwrap();
    assert!(import_native_credentials_into_account().is_err());
    assert_eq!(std::fs::read(jcode_path().unwrap()).unwrap(), before);
    assert_eq!(active_account_label(), Some(label));
}

#[test]
fn concurrent_native_snapshots_get_distinct_labels_without_losing_the_active_login() {
    let _env = ImportFixture::new();
    let active = upsert_account(saved("keep-selected")).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
    let workers = (0..5)
        .map(|i| {
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut creds = snapshot();
                creds.access_token = format!("parallel-native-{i}");
                creds.refresh_token = format!("parallel-refresh-{i}");
                barrier.wait();
                import_native_credentials_into_account_from(creds).unwrap()
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let labels = workers
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(labels.len(), 5);
    assert!(!labels.contains(&active));
    assert_eq!(list_accounts().unwrap().len(), 6);
    assert_eq!(active_account_label(), Some(active));
}
