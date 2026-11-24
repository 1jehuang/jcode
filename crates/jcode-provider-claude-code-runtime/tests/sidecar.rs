//! Sidecar integration uses its own process because CLI routing reads environment overrides.
#![cfg(unix)]
use jcode_provider_claude_code_runtime::{
    ClaudeCodeProvider,
    settings::{ClaudeCodeInstance, ClaudeCodeSettings},
};
use jcode_provider_core::{AccountPin, AccountProviderKind, Provider};
use serde_json::json;
use std::path::Path;

#[tokio::test]
async fn multiprovider_sidecar_uses_pinned_one_shot_account() {
    use jcode_base::provider::{MultiProvider, external};
    use std::sync::Arc;

    let _lock = jcode_base::storage::lock_test_env();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let wrapper = root.join("claude");
    let log = root.join("argv.log");
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake_claude.py");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nFAKE_CLAUDE_STATE='{}' FAKE_CLAUDE_LOG='{}' exec python3 '{}' \"$@\"\n",
            root.display(),
            log.display(),
            script.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    struct Env(Vec<(&'static str, Option<std::ffi::OsString>)>);
    impl Drop for Env {
        fn drop(&mut self) {
            for (key, value) in self.0.drain(..) {
                match value {
                    Some(value) => jcode_base::env::set_var(key, value),
                    None => jcode_base::env::remove_var(key),
                }
            }
        }
    }
    let values = [
        ("JCODE_HOME", root.join("home").into_os_string()),
        (
            "JCODE_CLAUDE_CODE_BIN",
            root.join("claude").into_os_string(),
        ),
        ("JCODE_ACTIVE_PROVIDER", "claude-code".into()),
    ];
    let _env = Env(values
        .iter()
        .map(|(key, _)| (*key, std::env::var_os(key)))
        .collect());
    for (key, value) in values {
        jcode_base::env::set_var(key, value);
    }
    let other_home = root.join("personal");
    let settings = ClaudeCodeSettings {
        instances: vec![
            ClaudeCodeInstance {
                id: "default".into(),
                ..Default::default()
            },
            ClaudeCodeInstance {
                id: "personal".into(),
                home: Some(other_home.display().to_string()),
                ..Default::default()
            },
        ],
        binary: wrapper.display().to_string(),
        ..Default::default()
    };
    external::register_external_provider(external::CLAUDE_CODE_RUNTIME, move || {
        Arc::new(ClaudeCodeProvider::with_settings(settings.clone()))
    });
    let provider = Arc::new(MultiProvider::new_fast());
    provider
        .set_account_pin(
            AccountProviderKind::ClaudeCode,
            Some(AccountPin::new("personal", None)),
        )
        .unwrap();
    provider.set_model("claude-code:claude-haiku-4-5").unwrap();
    jcode_base::provider::set_active_provider(provider);
    let sidecar = jcode_base::sidecar::Sidecar::new();
    assert_eq!(
        sidecar.complete("be brief", "text").await.unwrap(),
        "oneshot:text"
    );
    let launches: Vec<serde_json::Value> = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(launches.len(), 1);
    assert_eq!(launches[0]["config_dir"], other_home.display().to_string());
    let argv = launches[0]["argv"].as_array().unwrap();
    assert!(argv.contains(&json!("-p")));
    assert!(
        argv.windows(2)
            .any(|w| w == [json!("--model"), json!("claude-haiku-4-5")])
    );
    assert!(
        sidecar
            .complete("be brief", "authfail")
            .await
            .unwrap_err()
            .to_string()
            .contains("Sidecar completion")
    );
}
