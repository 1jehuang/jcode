use super::*;

fn set_or_clear_env(key: &str, value: Option<std::ffi::OsString>) {
    if let Some(value) = value {
        crate::env::set_var(key, value);
    } else {
        crate::env::remove_var(key);
    }
}

#[test]
fn scriptable_resume_command_matches_input_kind() {
    assert_eq!(
        scriptable_resume_command("openai", "callback_url", None),
        "jcode login --provider openai --callback-url '<url-or-query>'"
    );
    assert_eq!(
        scriptable_resume_command("gemini", "auth_code", None),
        "jcode login --provider gemini --auth-code '<code>'"
    );
    assert_eq!(
        scriptable_resume_command("copilot", "complete", None),
        "jcode login --provider copilot --complete"
    );
}

#[test]
fn load_pending_login_removes_expired_record() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let path = pending_login_path("openai", None).expect("pending path");
    let record = PendingScriptableLoginRecord {
        expires_at_ms: current_time_ms() - 1,
        login: PendingScriptableLogin::Openai {
            account_label: "default".to_string(),
            verifier: "verifier".to_string(),
            state: "state".to_string(),
            redirect_uri: "http://localhost:1455/auth/callback".to_string(),
        },
    };
    crate::storage::write_json_secret(&path, &record).expect("write pending login");

    let err = load_pending_login(&path, "openai").expect_err("expected expired state");
    assert!(err.to_string().contains("expired"));
    assert!(!path.exists(), "expired pending login should be removed");

    set_or_clear_env("JCODE_HOME", prev_home);
}

#[test]
fn load_pending_login_accepts_legacy_format() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let path = pending_login_path("gemini", None).expect("pending path");
    let legacy = PendingScriptableLogin::Gemini {
        verifier: "verifier".to_string(),
        redirect_uri: auth::gemini::GEMINI_MANUAL_REDIRECT_URI.to_string(),
    };
    crate::storage::write_json_secret(&path, &legacy).expect("write legacy pending login");

    let loaded = load_pending_login(&path, "gemini").expect("load legacy pending login");
    match loaded {
        PendingScriptableLogin::Gemini {
            verifier,
            redirect_uri,
        } => {
            assert_eq!(verifier, "verifier");
            assert_eq!(redirect_uri, auth::gemini::GEMINI_MANUAL_REDIRECT_URI);
        }
        other => panic!("unexpected login variant: {:?}", other),
    }

    set_or_clear_env("JCODE_HOME", prev_home);
}

#[test]
fn uses_scriptable_flow_detects_dash_input_without_consuming_stdin() {
    let options = LoginOptions {
        callback_url: Some("-".to_string()),
        ..LoginOptions::default()
    };
    assert!(
        options
            .uses_scriptable_flow()
            .expect("uses scriptable flow")
    );
    assert!(options.has_provided_input());
}

#[test]
fn auto_scriptable_flow_reason_prefers_non_interactive_for_oauth_provider() {
    let provider =
        crate::provider_catalog::resolve_login_provider("openai").expect("resolve openai provider");
    let reason = auto_scriptable_flow_reason(provider, &LoginOptions::default(), false);
    assert_eq!(reason, Some("non_interactive_terminal"));
}

#[test]
fn auto_scriptable_flow_reason_uses_no_browser_reason_when_requested() {
    let provider =
        crate::provider_catalog::resolve_login_provider("claude").expect("resolve claude provider");
    let reason = auto_scriptable_flow_reason(
        provider,
        &LoginOptions {
            no_browser: true,
            ..LoginOptions::default()
        },
        true,
    );
    assert_eq!(reason, Some("no_browser_requested"));
}

#[test]
fn auto_scriptable_flow_reason_skips_api_key_only_provider() {
    let provider = crate::provider_catalog::resolve_login_provider("openrouter")
        .expect("resolve openrouter provider");
    let reason = auto_scriptable_flow_reason(provider, &LoginOptions::default(), false);
    assert_eq!(reason, None);
}

#[test]
fn auto_scriptable_flow_reason_skips_when_scriptable_input_already_explicit() {
    let provider =
        crate::provider_catalog::resolve_login_provider("openai").expect("resolve openai provider");
    let reason = auto_scriptable_flow_reason(
        provider,
        &LoginOptions {
            print_auth_url: true,
            ..LoginOptions::default()
        },
        false,
    );
    assert_eq!(reason, None);
}

#[test]
fn scoped_pending_paths_validate_ids_and_preserve_legacy_layout() {
    let dir = Path::new("pending-login");
    assert_eq!(
        pending_login_path_in(dir, "openai", None).unwrap(),
        dir.join("openai.json")
    );
    assert_eq!(
        pending_login_path_in(dir, "openai", Some("flow_A-1")).unwrap(),
        dir.join("flows/flow_A-1/openai.json")
    );
    for id in [
        "",
        "..",
        "../victim",
        "a/b",
        "a\\b",
        "a.json",
        "a b",
        "é",
        "x\0y",
    ] {
        assert!(pending_login_path_in(dir, "openai", Some(id)).is_err());
    }
    assert!(pending_login_path_in(dir, "../credentials", Some("safe")).is_err());
    assert!(pending_login_path_in(dir, "openai", Some(&"a".repeat(65))).is_err());
    assert!(pending_login_path_in(dir, "openai", Some(&"a".repeat(64))).is_ok());
    for (provider, kind) in [
        ("openai", "callback_url"),
        ("gemini", "auth_code"),
        ("copilot", "complete"),
        ("claude", "auth_code_or_callback_url"),
    ] {
        let command = scriptable_resume_command(provider, kind, Some("flow_A-1"));
        assert!(command.contains(&format!("--provider {provider} --flow-id flow_A-1 ")));
    }
}

struct ScopedLoginTestHome(Option<std::ffi::OsString>);

impl ScopedLoginTestHome {
    fn new(path: &Path) -> Self {
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", path);
        Self(previous)
    }
}

impl Drop for ScopedLoginTestHome {
    fn drop(&mut self) {
        set_or_clear_env("JCODE_HOME", self.0.take());
    }
}

#[tokio::test]
async fn scoped_concurrent_begin_completion_and_cancel_are_isolated() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = ScopedLoginTestHome::new(temp.path());
    let provider = crate::provider_catalog::resolve_login_provider("openai").unwrap();
    let options_a = LoginOptions {
        flow_id: Some("flow-a".into()),
        print_auth_url: true,
        json: true,
        no_browser: true,
        ..Default::default()
    };
    let options_b = LoginOptions {
        flow_id: Some("flow-b".into()),
        ..options_a.clone()
    };
    let (a, b) = tokio::join!(
        start_scriptable_login(provider, None, &options_a),
        start_scriptable_login(provider, None, &options_b),
    );
    assert_eq!(a.unwrap(), LoginFlowOutcome::Deferred);
    assert_eq!(b.unwrap(), LoginFlowOutcome::Deferred);
    let path_a = pending_login_path("openai", Some("flow-a")).unwrap();
    let path_b = pending_login_path("openai", Some("flow-b")).unwrap();
    assert_ne!(path_a, path_b);
    let record_a: PendingScriptableLoginRecord =
        serde_json::from_str(&std::fs::read_to_string(&path_a).unwrap()).unwrap();
    let record_b: PendingScriptableLoginRecord =
        serde_json::from_str(&std::fs::read_to_string(&path_b).unwrap()).unwrap();
    match (&record_a.login, &record_b.login) {
        (
            PendingScriptableLogin::Openai {
                verifier: a,
                state: sa,
                ..
            },
            PendingScriptableLogin::Openai {
                verifier: b,
                state: sb,
                ..
            },
        ) => {
            assert_ne!(a, b);
            assert_ne!(sa, sb);
        }
        _ => panic!("expected OpenAI records"),
    }
    assert!(!pending_login_path("openai", None).unwrap().exists());
    // Auth-code rejection happens only after loading the selected flow, before any HTTP call.
    let error = complete_scriptable_openai_login(
        "openai",
        &options_a,
        ProvidedAuthInput::AuthCode("unused".into()),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("requires --callback-url"));
    let missing = LoginOptions {
        flow_id: Some("missing".into()),
        ..options_a.clone()
    };
    let error = complete_scriptable_openai_login(
        "openai",
        &missing,
        ProvidedAuthInput::AuthCode("unused".into()),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("No pending"));
    let legacy = pending_login_path("openai", None).unwrap();
    let other_provider = pending_login_path("claude", Some("flow-a")).unwrap();
    let credentials = temp.path().join("openai-auth.json");
    for path in [&legacy, &other_provider, &credentials] {
        std::fs::write(path, "preserve me").unwrap();
    }
    let before_b = std::fs::read(&path_b).unwrap();
    let cancel = LoginOptions {
        flow_id: Some("flow-a".into()),
        cancel: true,
        json: true,
        ..Default::default()
    };
    run_login_provider(provider, None, cancel.clone())
        .await
        .unwrap();
    run_login_provider(provider, None, cancel).await.unwrap();
    assert!(!path_a.exists());
    assert_eq!(std::fs::read(&path_b).unwrap(), before_b);
    for path in [&legacy, &other_provider, &credentials] {
        assert_eq!(std::fs::read_to_string(path).unwrap(), "preserve me");
    }
}

#[tokio::test]
async fn scoped_cancel_requires_explicit_provider_and_flow() {
    let options = LoginOptions {
        cancel: true,
        flow_id: Some("safe".into()),
        ..Default::default()
    };
    let error = run_login(&ProviderChoice::Auto, None, options)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("explicit provider"));
    let options = LoginOptions {
        cancel: true,
        ..Default::default()
    };
    let error = run_login(&ProviderChoice::Openai, None, options)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("requires --flow-id"));
}

fn scoped_test_record() -> PendingScriptableLoginRecord {
    PendingScriptableLoginRecord {
        expires_at_ms: current_time_ms() + 60_000,
        login: PendingScriptableLogin::Gemini {
            verifier: "test-verifier".into(),
            redirect_uri: "https://example.invalid/callback".into(),
        },
    }
}

#[test]
fn scoped_cancel_before_begin_fences_late_writes_and_expires() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = pending_login_path_in(temp.path(), "gemini", Some("cancelled-first")).unwrap();
    cancel_scoped_pending_login(&path).unwrap();
    cancel_scoped_pending_login(&path).unwrap();
    let error = persist_pending_login(&path, &scoped_test_record(), true).unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert!(!path.exists());
    let marker = path.with_extension("cancelled");
    let expires: i64 = serde_json::from_str(&std::fs::read_to_string(&marker).unwrap()).unwrap();
    assert!(expires > current_time_ms());
    assert!(expires <= current_time_ms() + 30 * 60 * 1000);
    crate::storage::write_json_secret(&marker, &(current_time_ms() - 1)).unwrap();
    persist_pending_login(&path, &scoped_test_record(), true).unwrap();
    assert!(path.exists());
    assert!(!marker.exists());
}

#[test]
fn scoped_concurrent_begin_and_cancel_never_resurrect_pending_state() {
    let temp = tempfile::TempDir::new().unwrap();
    for attempt in 0..32 {
        let path =
            pending_login_path_in(temp.path(), "gemini", Some(&format!("race-{attempt}"))).unwrap();
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let begin = scope.spawn(|| {
                barrier.wait();
                persist_pending_login(&path, &scoped_test_record(), true)
            });
            let cancel = scope.spawn(|| {
                barrier.wait();
                cancel_scoped_pending_login(&path)
            });
            if let Err(error) = begin.join().unwrap() {
                assert!(error.to_string().contains("cancelled"));
            }
            cancel.join().unwrap().unwrap();
        });
        assert!(!path.exists(), "cancelled pending state resurrected");
        assert!(persist_pending_login(&path, &scoped_test_record(), true).is_err());
    }
}
