use super::*;

#[cfg(unix)]
#[test]
fn account_lease_cross_process_helper() {
    let Ok(status_path) = std::env::var("JCODE_ACCOUNT_LEASE_STATUS") else {
        return;
    };
    let acquired =
        try_acquire_account_lease("cursor", "cross-process-test", Duration::from_secs(30))
            .is_some();
    std::fs::write(status_path, if acquired { "acquired" } else { "blocked" })
        .expect("write child lease status");
    if acquired {
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(unix)]
#[test]
fn account_lease_is_exclusive_across_processes() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
    let previous_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());

    let lease = try_acquire_account_lease("cursor", "cross-process-test", Duration::from_secs(30))
        .expect("parent should acquire account lease");
    let run_child = |expected: &str| {
        let status_path = home.path().join(format!("status-{expected}"));
        let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "auth::provider_pool::tests::account_lease_cross_process_helper",
                "--nocapture",
            ])
            .env("JCODE_HOME", home.path())
            .env("JCODE_ACCOUNT_LEASE_STATUS", &status_path)
            .status()
            .expect("run child lease test");
        assert!(status.success(), "child lease test failed: {status}");
        assert_eq!(
            std::fs::read_to_string(status_path).expect("read child lease status"),
            expected
        );
    };

    run_child("blocked");
    drop(lease);
    run_child("acquired");

    match previous_home {
        Some(previous) => crate::env::set_var("JCODE_HOME", previous),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

#[test]
fn labels_are_stable_and_safe() {
    assert_eq!(
        label("cursor", 0, Some("Work Email"), "id"),
        "cursor-Work-Email"
    );
    assert_eq!(label("cursor", 0, None, "id/unsafe"), "cursor-id-unsafe");
}

#[test]
fn persisted_managed_switch_updates_runtime_override() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
    let previous_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());
    set_runtime_active_override("cursor", Some("cursor-one".to_string()));

    let account = |label: &str| ManagedProviderAccount {
        id: label.to_string(),
        label: label.to_string(),
        access_token: "access".to_string(),
        refresh_token: "refresh".to_string(),
        expires_at: 0,
        email: None,
        project_id: None,
    };
    write(
        "cursor",
        &AccountFile {
            active_account: Some("cursor-one".to_string()),
            accounts: vec![account("cursor-one"), account("cursor-two")],
        },
    )
    .expect("write managed accounts");

    set_active_account("cursor", "cursor-two").expect("switch managed account");
    assert_eq!(
        crate::auth::account_store::runtime_active_override("cursor").as_deref(),
        Some("cursor-two")
    );
    assert_eq!(
        active_account("cursor")
            .expect("read active managed account")
            .expect("active account")
            .label,
        "cursor-two"
    );

    set_runtime_active_override("cursor", None);
    match previous_home {
        Some(previous) => crate::env::set_var("JCODE_HOME", previous),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

#[test]
fn managed_account_file_contains_metadata_only() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
    let previous_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());

    write(
        "cursor",
        &AccountFile {
            active_account: Some("cursor-secret-free".to_string()),
            accounts: vec![ManagedProviderAccount {
                id: "cursor-secret-free".to_string(),
                label: "cursor-secret-free".to_string(),
                access_token: "access-token-must-not-be-on-disk".to_string(),
                refresh_token: "refresh-token-must-not-be-on-disk".to_string(),
                expires_at: 0,
                email: None,
                project_id: None,
            }],
        },
    )
    .expect("write managed account");

    let contents = std::fs::read_to_string(accounts_path("cursor").expect("account path"))
        .expect("read account metadata");
    assert!(!contents.contains("access-token-must-not-be-on-disk"));
    assert!(!contents.contains("refresh-token-must-not-be-on-disk"));
    assert!(contents.contains("credential_key"));

    match previous_home {
        Some(previous) => crate::env::set_var("JCODE_HOME", previous),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

#[test]
fn cooldown_state_survives_process_local_cache_reset_without_secrets() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
    let previous_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());
    let provider = "cursor";
    let account = "cursor-persistence-test";

    clear_account_cooldown(provider, account);
    mark_account_cooldown(provider, account, Duration::from_secs(60));
    assert!(account_on_cooldown(provider, account));

    let state_path = health_path().expect("resolve health path");
    let state = std::fs::read_to_string(&state_path).expect("read persisted health state");
    assert!(state.contains(account));
    assert!(!state.contains("access_token"));
    assert!(!state.contains("refresh_token"));

    ACCOUNT_COOLDOWNS
        .lock()
        .expect("lock cooldown cache")
        .clear();
    assert!(account_on_cooldown(provider, account));

    clear_account_cooldown(provider, account);
    assert!(!account_on_cooldown(provider, account));

    match previous_home {
        Some(previous) => crate::env::set_var("JCODE_HOME", previous),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

#[test]
fn quota_score_prefers_recent_high_remaining_account() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
    let previous_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());

    record_account_quotas(
        "antigravity",
        "antigravity-low",
        &[("gemini-3-flash".to_string(), Some(100), None)],
    );
    record_account_quotas(
        "antigravity",
        "antigravity-high",
        &[(
            "gemini-3-flash".to_string(),
            Some(900),
            Some("reset".to_string()),
        )],
    );

    assert_eq!(
        account_quota_score("antigravity", "antigravity-low"),
        Some(100)
    );
    assert_eq!(
        account_quota_score("antigravity", "antigravity-high"),
        Some(900)
    );
    let state = std::fs::read_to_string(health_path().expect("resolve health path"))
        .expect("read quota state");
    assert!(state.contains("gemini-3-flash"));
    assert!(state.contains("reset"));
    assert!(!state.contains("access_token"));
    assert!(!state.contains("refresh_token"));

    match previous_home {
        Some(previous) => crate::env::set_var("JCODE_HOME", previous),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

#[test]
fn model_quota_score_does_not_use_another_model_as_a_proxy() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
    let previous_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());

    record_account_quotas(
        "antigravity",
        "antigravity-model-scoped",
        &[
            ("gemini-3-pro".to_string(), Some(900), None),
            ("gemini-3-flash".to_string(), Some(100), None),
        ],
    );

    assert_eq!(
        account_quota_score_for_model("antigravity", "antigravity-model-scoped", "gemini-3-pro",),
        Some(900)
    );
    assert_eq!(
        account_quota_score_for_model("antigravity", "antigravity-model-scoped", "gemini-3-flash",),
        Some(100)
    );
    assert_eq!(
        account_quota_score_for_model("antigravity", "antigravity-model-scoped", "unknown-model",),
        None
    );

    match previous_home {
        Some(previous) => crate::env::set_var("JCODE_HOME", previous),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

#[test]
fn quota_history_is_bounded_and_legacy_snapshots_still_deserialize() {
    let legacy: AccountQuotaSnapshot = serde_json::from_str(
        r#"{"remaining_fraction_milli":321,"reset_time":"reset","observed_at_unix_secs":1}"#,
    )
    .expect("legacy quota snapshot should deserialize");
    assert!(legacy.history.is_empty());

    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
    let previous_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());
    for remaining in 0..(MAX_QUOTA_HISTORY as u16 + 3) {
        record_account_quota(
            "antigravity",
            "history-test",
            "gemini-3-flash",
            Some(remaining),
            None,
        );
    }
    let health = read_health();
    let snapshot = &health.quotas["antigravity"]["history-test"]["gemini-3-flash"];
    assert_eq!(snapshot.history.len(), MAX_QUOTA_HISTORY - 1);
    assert_eq!(
        account_quota_score_for_model("antigravity", "history-test", "gemini-3-flash"),
        Some(MAX_QUOTA_HISTORY as u16 + 2)
    );

    match previous_home {
        Some(previous) => crate::env::set_var("JCODE_HOME", previous),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

#[test]
fn concurrent_health_updates_preserve_each_account_snapshot() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
    let previous_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());

    let workers = (0..12)
        .map(|index| {
            std::thread::spawn(move || {
                let label = format!("antigravity-concurrent-{index}");
                record_account_quota(
                    "antigravity",
                    &label,
                    "gemini-3-flash",
                    Some((index as u16 + 1) * 50),
                    None,
                );
                label
            })
        })
        .collect::<Vec<_>>();

    for worker in workers {
        let label = worker.join().expect("health update worker should finish");
        assert!(
            account_quota_score_for_model("antigravity", &label, "gemini-3-flash").is_some(),
            "concurrent update for {label} was lost"
        );
    }

    match previous_home {
        Some(previous) => crate::env::set_var("JCODE_HOME", previous),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn account_request_scopes_serialize_same_provider_and_isolate_other_providers() {
    let first = acquire_account_request_lease("openai")
        .await
        .expect("OpenAI should use an account request scope");
    assert!(
        try_acquire_account_request_lease("openai").is_none(),
        "a synchronous account switch must not mutate an active request"
    );

    let second_started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let second_started_task = std::sync::Arc::clone(&second_started);
    let second = tokio::spawn(async move {
        let lease = acquire_account_request_lease("openai")
            .await
            .expect("OpenAI should use an account request scope");
        second_started_task.store(true, std::sync::atomic::Ordering::Release);
        lease
    });

    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    assert!(
        !second_started.load(std::sync::atomic::Ordering::Acquire),
        "same-provider requests must wait for the prior request scope"
    );

    let other_provider = acquire_account_request_lease("claude").await;
    assert!(
        other_provider.is_some(),
        "different account providers must retain independent request scopes"
    );
    drop(other_provider);
    drop(first);

    let second = second.await.expect("waiting request should finish");
    assert!(second_started.load(std::sync::atomic::Ordering::Acquire));
    drop(second);
    assert!(
        try_acquire_account_request_lease("openai").is_some(),
        "the request scope must be reusable after the stream owner drops it"
    );
}

#[test]
fn concurrent_account_upserts_preserve_each_account() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
    let previous_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());

    let workers = (0..12)
        .map(|index| {
            std::thread::spawn(move || {
                upsert_account(
                    "cursor",
                    ManagedProviderAccount {
                        id: format!("cursor-id-{index}"),
                        label: format!("cursor-imported-{index}"),
                        access_token: format!("access-{index}"),
                        refresh_token: format!("refresh-{index}"),
                        expires_at: 1,
                        email: None,
                        project_id: None,
                    },
                )
                .expect("account upsert should succeed");
            })
        })
        .collect::<Vec<_>>();

    for worker in workers {
        worker.join().expect("account upsert worker should finish");
    }

    let accounts = list_accounts("cursor").expect("list accounts");
    assert_eq!(accounts.len(), 12);
    for index in 0..12 {
        assert!(
            accounts
                .iter()
                .any(|account| account.id == format!("cursor-id-{index}")),
            "concurrent account {index} was lost"
        );
    }

    match previous_home {
        Some(previous) => crate::env::set_var("JCODE_HOME", previous),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

#[test]
fn account_lease_excludes_concurrent_acquisition_and_releases_on_drop() {
    let label = format!("lease-test-{}", std::process::id());
    let first = try_acquire_account_lease("cursor", &label, Duration::from_secs(60))
        .expect("first lease should be available");
    assert!(try_acquire_account_lease("cursor", &label, Duration::from_secs(60)).is_none());

    drop(first);
    assert!(try_acquire_account_lease("cursor", &label, Duration::from_secs(60)).is_some());
}

#[test]
fn account_and_health_locks_block_a_second_handle() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
    let previous_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());

    let account_lock = AccountFileLock::acquire("cursor", false).expect("account lock");
    let account_path = accounts_path("cursor")
        .expect("account path")
        .with_file_name("cursor_accounts.json.lock");
    let second = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(account_path)
        .expect("open account lock");
    assert!(matches!(
        second.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    drop(account_lock);
    second.try_lock().expect("account lock released");

    let health_lock = HealthFileLock::acquire(false).expect("health lock");
    let health_second = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(health_lock_path().expect("health lock path"))
        .expect("open health lock");
    assert!(matches!(
        health_second.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    drop(health_lock);

    match previous_home {
        Some(previous) => crate::env::set_var("JCODE_HOME", previous),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

#[test]
fn remove_account_deletes_credentials_and_promotes_the_next_active_label() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
    let previous_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());
    set_runtime_active_override("cursor", None);

    let account = |label: &str| ManagedProviderAccount {
        id: label.to_string(),
        label: label.to_string(),
        access_token: format!("{label}-access"),
        refresh_token: format!("{label}-refresh"),
        expires_at: 0,
        email: None,
        project_id: None,
    };
    write(
        "cursor",
        &AccountFile {
            active_account: Some("cursor-one".to_string()),
            accounts: vec![account("cursor-one"), account("cursor-two")],
        },
    )
    .expect("write managed accounts");

    remove_account("cursor", "cursor-one").expect("remove account");
    let accounts = list_accounts("cursor").expect("list accounts");
    assert_eq!(
        accounts
            .iter()
            .map(|account| account.label.as_str())
            .collect::<Vec<_>>(),
        vec!["cursor-two"]
    );
    assert_eq!(
        active_account("cursor")
            .expect("read active")
            .expect("active account")
            .label,
        "cursor-two"
    );
    assert!(
        crate::storage::get_native_credential(&native_account_key(
            "cursor",
            "cursor-one",
            "access"
        ))
        .is_err()
    );

    match previous_home {
        Some(previous) => crate::env::set_var("JCODE_HOME", previous),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}
