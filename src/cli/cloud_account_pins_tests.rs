use super::*;
use crate::auth::{claude, codex, test_sandbox::AuthTestSandbox};

fn select_home(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    crate::env::set_var("JCODE_HOME", path);
}

fn source_accounts() -> (session::Session, Vec<CloudCredentialTransfer>) {
    let claude = claude::upsert_account(claude::AnthropicAccount {
        record_id: None,
        label: "cloud-claude".into(),
        access: "source-claude-access".into(),
        refresh: "source-claude-refresh".into(),
        expires: i64::MAX,
        email: None,
        subscription_type: None,
        scopes: vec![],
    })
    .unwrap();
    let openai = codex::upsert_account(codex::OpenAiAccount {
        record_id: None,
        label: "cloud-openai".into(),
        access_token: "source-openai-access".into(),
        refresh_token: "source-openai-refresh".into(),
        expires_at: Some(i64::MAX),
        id_token: None,
        account_id: None,
        email: None,
    })
    .unwrap();
    let mut parent = session::Session::create_with_id("cloud-pin-parent".into(), None, None);
    parent
        .account_pins
        .insert("claude".into(), account_pins::claude_pin(&claude).unwrap());
    parent
        .account_pins
        .insert("openai".into(), account_pins::openai_pin(&openai).unwrap());
    parent.route_api_method = Some(format!("claude-oauth@{claude}"));
    let payloads = [
        (TransferProvider::Claude, &claude),
        (TransferProvider::OpenAi, &openai),
    ]
    .into_iter()
    .map(|(provider, label)| {
        transfer::export_cloud_account_at(&storage::jcode_dir().unwrap(), provider, Some(label))
            .unwrap()
    })
    .collect();
    (parent, payloads)
}

fn import(payloads: &[CloudCredentialTransfer]) {
    let home = storage::jcode_dir().unwrap();
    for payload in payloads {
        transfer::import_cloud_handoff_at(
            &home,
            payload.provider(),
            payload.label(),
            payload.as_bytes(),
        )
        .unwrap();
    }
}

fn manifest(sess: &session::Session, proofs: Vec<CloudAuthBaseline>) -> MoveManifest {
    let credentials = session_credentials(sess).unwrap().unwrap();
    MoveManifest {
        format_version: FORMAT_VERSION,
        session_id: sess.id.clone(),
        epoch: sess.migration_epoch,
        source_host: "source-fixture".into(),
        target_host: "destination-fixture".into(),
        created_at: "2026-11-26T00:00:00Z".into(),
        repo: None,
        env: vec![],
        toolchain_hints: vec![],
        not_transferred: vec![],
        mcp_servers: vec![],
        auth_account: Some(CloudAccount {
            provider: credentials.provider(),
            label: credentials.label().into(),
        }),
        auth_accounts: proofs.iter().map(|proof| proof.account.clone()).collect(),
        auth_pin_proofs: proofs,
    }
}

fn stage(sess: &session::Session, manifest: &MoveManifest) -> PathBuf {
    let dir = inbox_dir()
        .unwrap()
        .join(format!("{}.{}", sess.id, sess.migration_epoch));
    std::fs::create_dir_all(&dir).unwrap();
    storage::write_json(&dir.join("session.json"), sess).unwrap();
    storage::write_json(&dir.join("manifest.json"), manifest).unwrap();
    dir
}

#[test]
fn cloud_fork_activation_rebinds_both_claimless_accounts_after_private_transfer() {
    let sandbox = AuthTestSandbox::new().unwrap();
    for family in ["claude", "openai"] {
        let source = sandbox.root().join(format!("source-{family}"));
        let destination = sandbox.root().join(format!("destination-{family}"));
        select_home(&source);
        let (mut parent, payloads) = source_accounts();
        parent.route_api_method = Some(format!(
            "{family}-oauth@{}",
            parent.account_pins[family].label
        ));
        let mut child = session::Session::create_with_id(
            format!("cloud-fork-{family}"),
            Some(parent.id.clone()),
            None,
        );
        child.inherit_provider_route(&parent);
        child.migration_epoch = 1;
        child.save().unwrap();
        let manifest = manifest(&child, account_pin_proofs(&child).unwrap());
        select_home(&destination);
        import(&payloads);
        assert!(account_pins::load_claude(&child.account_pins["claude"]).is_err());
        assert!(account_pins::load_openai(&child.account_pins["openai"]).is_err());
        let claude_default = claude::active_account_label();
        let openai_default = codex::active_account_label();
        stage(&child, &manifest);
        run_activate(&child.id, 1).unwrap();
        let local = session::Session::load(&child.id).unwrap();
        for family in ["claude", "openai"] {
            assert_ne!(
                local.account_pins[family].record_id,
                child.account_pins[family].record_id
            );
        }
        assert_eq!(
            account_pins::load_claude(&local.account_pins["claude"])
                .unwrap()
                .0
                .access_token,
            "source-claude-access"
        );
        assert_eq!(
            account_pins::load_openai(&local.account_pins["openai"])
                .unwrap()
                .0
                .access_token,
            "source-openai-access"
        );
        assert_eq!(claude::active_account_label(), claude_default);
        assert_eq!(codex::active_account_label(), openai_default);
        assert_eq!(local.route_api_method, child.route_api_method);
        select_home(&source);
        assert_eq!(
            session::Session::load(&child.id).unwrap().account_pins,
            child.account_pins
        );
    }
}

#[test]
fn cloud_activation_rejects_same_label_replacement_without_installing_or_rebinding() {
    let sandbox = AuthTestSandbox::new().unwrap();
    for replaced in [TransferProvider::Claude, TransferProvider::OpenAi] {
        let source = tempfile::tempdir_in(sandbox.root()).unwrap();
        let destination = tempfile::tempdir_in(sandbox.root()).unwrap();
        select_home(source.path());
        let (mut sess, payloads) = source_accounts();
        sess.migration_epoch = 1;
        sess.route_api_method = Some(format!(
            "{}-oauth@{}",
            replaced.as_str(),
            sess.account_pins[replaced.as_str()].label
        ));
        let manifest = manifest(&sess, account_pin_proofs(&sess).unwrap());
        select_home(destination.path());
        import(&payloads);
        match replaced {
            TransferProvider::Claude => {
                let mut auth = claude::load_auth_file().unwrap();
                auth.anthropic_accounts[0].access = "replacement-access".into();
                auth.anthropic_accounts[0].refresh = "replacement-refresh".into();
                claude::save_auth_file(&auth).unwrap();
            }
            TransferProvider::OpenAi => {
                let mut auth = codex::load_auth_file().unwrap();
                auth.openai_accounts[0].access_token = "replacement-access".into();
                auth.openai_accounts[0].refresh_token = "replacement-refresh".into();
                codex::save_auth_file(&auth).unwrap();
            }
        }
        let dir = stage(&sess, &manifest);
        let before = std::fs::read(dir.join("session.json")).unwrap();
        let error = run_activate(&sess.id, 1).unwrap_err();
        assert!(
            error.to_string().contains("credential changed"),
            "{error:#}"
        );
        assert_eq!(std::fs::read(dir.join("session.json")).unwrap(), before);
        assert!(storage::read_session_lease(&sess.id).is_none());
        assert!(session::Session::load(&sess.id).is_err());
    }
}

#[test]
fn cloud_pin_proofs_reject_missing_duplicate_and_reused_source_labels_atomically() {
    let sandbox = AuthTestSandbox::new().unwrap();
    let source = sandbox.root().join("source");
    select_home(&source);
    let (sess, payloads) = source_accounts();
    let mut proofs = account_pin_proofs(&sess).unwrap();
    select_home(&sandbox.root().join("destination"));
    import(&payloads);
    let mut unchanged = sess.clone();
    assert!(rebind_account_pins(&mut unchanged, &[]).is_err());
    assert_eq!(unchanged.account_pins, sess.account_pins);
    proofs.push(proofs[0].clone());
    assert!(rebind_account_pins(&mut unchanged, &proofs).is_err());
    assert_eq!(unchanged.account_pins, sess.account_pins);
    select_home(&source);
    let label = &sess.account_pins["claude"].label;
    claude::remove_account(label).unwrap();
    let (replacement, _) = source_accounts();
    assert_ne!(
        replacement.account_pins["claude"].record_id,
        sess.account_pins["claude"].record_id
    );
    assert!(account_pin_proofs(&sess).is_err());
    assert!(session_credentials(&sess).is_err());
}

#[test]
fn returned_cloud_pins_rebind_to_original_local_lineage_after_verified_refresh() {
    let sandbox = AuthTestSandbox::new().unwrap();
    let source = sandbox.root().join("source");
    select_home(&source);
    let (mut sess, payloads) = source_accounts();
    let source_pins = sess.account_pins.clone();
    let proofs = account_pin_proofs(&sess).unwrap();
    select_home(&sandbox.root().join("destination"));
    import(&payloads);
    rebind_account_pins(&mut sess, &proofs).unwrap();
    for (family, update) in [("claude", true), ("openai", false)] {
        let label = &sess.account_pins[family].label;
        if update {
            claude::update_account_tokens(label, "refreshed-access", "refreshed-refresh", i64::MAX)
                .unwrap();
        } else {
            codex::update_account_tokens(
                label,
                "refreshed-access",
                "refreshed-refresh",
                None,
                None,
                Some(i64::MAX),
            )
            .unwrap();
        }
    }
    let return_proofs = account_pin_proofs(&sess).unwrap();
    let returned = return_proofs
        .iter()
        .map(|proof| {
            transfer::export_cloud_account_at(
                &storage::jcode_dir().unwrap(),
                proof.account.provider,
                Some(&proof.account.label),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    select_home(&source);
    for (original, returned) in payloads.iter().zip(&returned) {
        transfer::import_cloud_return_at(
            &source,
            original.provider(),
            original.label(),
            returned.as_bytes(),
            original,
        )
        .unwrap();
    }
    rebind_account_pins(&mut sess, &return_proofs).unwrap();
    for family in ["claude", "openai"] {
        assert_eq!(
            sess.account_pins[family].record_id,
            source_pins[family].record_id
        );
    }
    assert_eq!(
        account_pins::load_claude(&sess.account_pins["claude"])
            .unwrap()
            .0
            .access_token,
        "refreshed-access"
    );
    assert_eq!(
        account_pins::load_openai(&sess.account_pins["openai"])
            .unwrap()
            .0
            .access_token,
        "refreshed-access"
    );
}

#[test]
fn inactive_missing_pin_does_not_block_api_return_export_or_rebinding() {
    let sandbox = AuthTestSandbox::new().unwrap();
    select_home(&sandbox.root().join("inactive-source"));
    let (mut sess, _) = source_accounts();
    sess.route_api_method = Some("claude-api".into());
    let removed = sess.account_pins["claude"].label.clone();
    claude::remove_account(&removed).unwrap();
    let retained = sess.account_pins.clone();
    assert!(account_pin_proofs(&sess).is_ok());
    rebind_account_pins(&mut sess, &[]).unwrap();
    assert_eq!(sess.account_pins, retained);
    sess.save().unwrap();
    storage::write_session_lease(&storage::SessionLease {
        session_id: sess.id.clone(),
        epoch: 1,
        away_host: None,
        repo_root: None,
        last_sync_snapshot: None,
        last_sync_head: None,
        updated_at: "fixture".into(),
    })
    .unwrap();
    assert!(!export_return_archive(&sess.id, 2, true).unwrap().is_empty());
}

#[cfg(unix)]
fn return_fixture(root: &Path, bad_proof: bool) -> (session::Session, Target, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let source = root.join("source");
    select_home(&source);
    let (mut local, departure) = source_accounts();
    local.save().unwrap();
    let initial = account_pin_proofs(&local).unwrap();
    storage::write_json_secret(&baseline_path(&local.id).unwrap(), &initial).unwrap();
    storage::write_session_lease(&storage::SessionLease {
        session_id: local.id.clone(),
        epoch: 1,
        away_host: Some("offline-cloud".into()),
        repo_root: None,
        last_sync_snapshot: None,
        last_sync_head: None,
        updated_at: "fixture".into(),
    })
    .unwrap();
    select_home(&root.join("cloud"));
    import(&departure);
    let mut cloud = local.clone();
    rebind_account_pins(&mut cloud, &initial).unwrap();
    cloud.migration_epoch = 2;
    cloud.title = Some("returned cloud transcript".into());
    for (family, provider) in [
        ("claude", TransferProvider::Claude),
        ("openai", TransferProvider::OpenAi),
    ] {
        let label = &cloud.account_pins[family].label;
        match provider {
            TransferProvider::Claude => {
                claude::update_account_tokens(label, "cloud-rotated", "cloud-refresh", i64::MAX)
                    .unwrap()
            }
            TransferProvider::OpenAi => codex::update_account_tokens(
                label,
                "cloud-rotated",
                "cloud-refresh",
                None,
                None,
                Some(i64::MAX),
            )
            .unwrap(),
        }
        let payload = transfer::export_cloud_account_at(
            &storage::jcode_dir().unwrap(),
            provider,
            Some(label),
        )
        .unwrap();
        std::fs::write(
            source.join(format!("{}.payload", provider.as_str())),
            payload.as_bytes(),
        )
        .unwrap();
    }
    let mut manifest = manifest(&cloud, account_pin_proofs(&cloud).unwrap());
    if bad_proof {
        manifest
            .auth_pin_proofs
            .retain(|p| p.account.provider != TransferProvider::Claude);
        let pin = cloud.account_pins.get_mut("claude").unwrap();
        pin.record_id = Some("missing-cloud-record".into());
        pin.identity = None;
    }
    let stage = root.join("return-stage");
    std::fs::create_dir_all(&stage).unwrap();
    storage::write_json(&stage.join("session.json"), &cloud).unwrap();
    storage::write_json(&stage.join("manifest.json"), &manifest).unwrap();
    std::fs::write(stage.join("return-completion-v1"), "").unwrap();
    tar_dir(
        &stage,
        &source.join("return-fixture.tar"),
        &["session.json", "manifest.json", "return-completion-v1"],
    )
    .unwrap();
    let script = source.join("return-transport.sh");
    std::fs::write(
        &script,
        r#"#!/bin/sh
set -eu
case "$1" in
  *"'complete-return' '--help'"*) exit 0 ;;
  *"'cancel-return'"*) printf 'cancel\n' >> "$JCODE_HOME/return-calls" ;;
  *"'complete-return'"*) printf 'complete\n' >> "$JCODE_HOME/return-calls" ;;
  *"'export'"*) cat "$JCODE_HOME/return-fixture.tar" ;;
  *"'credentials'"*)
    printf 'credentials\n' >> "$JCODE_HOME/return-calls"
    case "$1" in
      *"'--provider' 'claude'"*) cat "$JCODE_HOME/claude.payload" ;;
      *"'--provider' 'openai'"*) cat "$JCODE_HOME/openai.payload" ;;
      *) exit 91 ;;
    esac ;;
  *) exit 92 ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    select_home(&source);
    let target = Target {
        transport: Some(script.to_string_lossy().into_owned()),
        ..Target::default()
    };
    (local, target, source)
}

#[cfg(unix)]
#[test]
fn keep_local_auth_returns_cloud_transcript_without_adopting_cloud_login() {
    let sandbox = AuthTestSandbox::new().unwrap();
    for api in [false, true] {
        let (mut local, target, home) = return_fixture(
            &sandbox.root().join(if api { "api" } else { "oauth" }),
            false,
        );
        if api {
            local.route_api_method = Some("claude-api".into());
            // Model the saved departure choice; ordinary away-session saves are fenced.
            storage::write_json(&session::session_path(&local.id).unwrap(), &local).unwrap();
        }
        let before = std::fs::read(home.join("auth.json")).unwrap();
        let report = run_return(Some(&local.id), &target, false, true).unwrap();
        assert_eq!(report.session_id, local.id);
        let returned = session::Session::load(&local.id).unwrap();
        assert_eq!(returned.title.as_deref(), Some("returned cloud transcript"));
        assert_eq!(returned.route_api_method, local.route_api_method);
        assert_eq!(returned.account_pins, local.account_pins);
        assert_eq!(std::fs::read(home.join("auth.json")).unwrap(), before);
        assert!(storage::read_session_lease(&local.id).unwrap().is_here());
        assert_eq!(
            std::fs::read_to_string(home.join("return-calls")).unwrap(),
            "complete\n"
        );
    }
}

#[cfg(unix)]
#[test]
fn failed_return_pin_binding_cancels_remote_export_and_restores_cloud_ownership() {
    let sandbox = AuthTestSandbox::new().unwrap();
    let (local, target, home) = return_fixture(sandbox.root(), true);
    assert!(run_return(Some(&local.id), &target, false, false).is_err());
    let calls = std::fs::read_to_string(home.join("return-calls")).unwrap();
    assert!(calls.lines().any(|s| s == "cancel"), "{calls}");
    let lease = storage::read_session_lease(&local.id).unwrap();
    assert_eq!(lease.epoch, 2);
    assert_eq!(lease.away_host.as_deref(), Some("offline-cloud"));
    let unchanged = session::Session::load(&local.id).unwrap();
    assert_eq!(unchanged.title, local.title);
    assert_eq!(unchanged.account_pins, local.account_pins);
}

#[cfg(unix)]
#[test]
fn captured_single_and_group_proofs_match_sent_bytes_across_source_refreshes() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = AuthTestSandbox::new().unwrap();
    let source = sandbox.root().join("capture-source");
    select_home(&source);
    let (first, _) = source_accounts();
    let mut second = first.clone();
    second.id = "second-group-member".into();
    let old = account_pin_proofs(&first).unwrap();
    claude::update_account_tokens(
        &first.account_pins["claude"].label,
        "captured-access",
        "captured-refresh",
        i64::MAX,
    )
    .unwrap();
    let (payloads, proofs) =
        capture_credentials(&[first.clone(), second.clone()], &cloud_accounts().unwrap()).unwrap();
    assert_eq!(payloads.len(), 2);
    let captured = proofs[&first.id]
        .iter()
        .find(|p| p.account.provider == TransferProvider::Claude)
        .unwrap();
    assert_ne!(
        captured.fingerprint,
        old.iter()
            .find(|p| p.account.provider == TransferProvider::Claude)
            .unwrap()
            .fingerprint
    );
    claude::update_account_tokens(
        &first.account_pins["claude"].label,
        "later-access",
        "later-refresh",
        i64::MAX,
    )
    .unwrap();
    let script = source.join("capture-transport.sh");
    std::fs::write(
        &script,
        r#"#!/bin/sh
set -eu
case "$1" in
  *"'--provider' 'claude'"*) cat > "$JCODE_HOME/wire-claude" ;;
  *"'--provider' 'openai'"*) cat > "$JCODE_HOME/wire-openai" ;;
  *) exit 93 ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let target = Target {
        transport: Some(script.to_string_lossy().into_owned()),
        ..Target::default()
    };
    for payload in &payloads {
        let baseline = send_credentials(&target, "offline", payload).unwrap();
        let wire =
            std::fs::read(source.join(format!("wire-{}", payload.provider().as_str()))).unwrap();
        assert_eq!(wire, payload.as_bytes());
        for id in [&first.id, &second.id] {
            let proof = proofs[id]
                .iter()
                .find(|p| p.account == baseline.account)
                .unwrap();
            assert_eq!(proof.fingerprint, baseline.fingerprint);
        }
    }
    select_home(&sandbox.root().join("capture-destination"));
    for payload in &payloads {
        let wire =
            std::fs::read(source.join(format!("wire-{}", payload.provider().as_str()))).unwrap();
        transfer::import_cloud_handoff_at(
            &storage::jcode_dir().unwrap(),
            payload.provider(),
            payload.label(),
            &wire,
        )
        .unwrap();
    }
    for mut sess in [first, second] {
        let expected = proofs[&sess.id].clone();
        rebind_account_pins(&mut sess, &expected).unwrap();
        assert_eq!(
            account_pins::load_claude(&sess.account_pins["claude"])
                .unwrap()
                .0
                .access_token,
            "captured-access"
        );
    }
}

#[test]
fn unavailable_inactive_pin_is_retained_while_active_pin_stays_strict() {
    let sandbox = AuthTestSandbox::new().unwrap();
    select_home(&sandbox.root().join("source"));
    let (mut sess, _) = source_accounts();
    let inactive = sess.account_pins["openai"].clone();
    codex::remove_account(&inactive.label).unwrap();
    let (payloads, proofs) =
        capture_credentials(std::slice::from_ref(&sess), &cloud_accounts().unwrap()).unwrap();
    assert_eq!(payloads.len(), 1);
    select_home(&sandbox.root().join("destination"));
    import(&payloads);
    let expected = proofs[&sess.id].clone();
    rebind_account_pins(&mut sess, &expected).unwrap();
    assert_eq!(sess.account_pins["openai"], inactive);
    assert!(account_pins::load_openai(&inactive).is_err());
    assert!(account_pins::load_claude(&sess.account_pins["claude"]).is_ok());
    claude::remove_account(&sess.account_pins["claude"].label).unwrap();
    assert!(capture_credentials(std::slice::from_ref(&sess), &[]).is_err());
}

#[cfg(unix)]
#[test]
fn return_uses_actual_credential_export_after_archive_snapshot_refresh() {
    let sandbox = AuthTestSandbox::new().unwrap();
    let (local, target, source) = return_fixture(sandbox.root(), false);
    select_home(&sandbox.root().join("cloud"));
    for (family, provider) in [
        ("claude", TransferProvider::Claude),
        ("openai", TransferProvider::OpenAi),
    ] {
        let label = &local.account_pins[family].label;
        match provider {
            TransferProvider::Claude => {
                claude::update_account_tokens(label, "second-rotation", "second-refresh", i64::MAX)
                    .unwrap()
            }
            TransferProvider::OpenAi => codex::update_account_tokens(
                label,
                "second-rotation",
                "second-refresh",
                None,
                None,
                Some(i64::MAX),
            )
            .unwrap(),
        }
        let payload = transfer::export_cloud_account_at(
            &storage::jcode_dir().unwrap(),
            provider,
            Some(label),
        )
        .unwrap();
        std::fs::write(
            source.join(format!("{}.payload", provider.as_str())),
            payload.as_bytes(),
        )
        .unwrap();
    }
    select_home(&source);
    run_return(Some(&local.id), &target, false, false).unwrap();
    let returned = session::Session::load(&local.id).unwrap();
    for family in ["claude", "openai"] {
        assert_eq!(
            returned.account_pins[family].record_id,
            local.account_pins[family].record_id
        );
    }
    assert_eq!(
        account_pins::load_claude(&returned.account_pins["claude"])
            .unwrap()
            .0
            .access_token,
        "second-rotation"
    );
    assert_eq!(
        account_pins::load_openai(&returned.account_pins["openai"])
            .unwrap()
            .0
            .access_token,
        "second-rotation"
    );
    assert!(storage::read_session_lease(&local.id).unwrap().is_here());
    assert_eq!(
        std::fs::read_to_string(source.join("return-calls")).unwrap(),
        "credentials\ncredentials\ncomplete\n"
    );
}

#[cfg(unix)]
#[test]
fn keep_local_auth_restores_the_local_model_and_effort_with_its_route() {
    let sandbox = AuthTestSandbox::new().unwrap();
    let (mut local, target, home) = return_fixture(sandbox.root(), false);
    local.model = Some("claude-sonnet-4-5".into());
    local.reasoning_effort = Some("high".into());
    storage::write_json(&session::session_path(&local.id).unwrap(), &local).unwrap();
    let stage = sandbox.root().join("return-stage");
    let mut cloud: session::Session = read_json(&stage.join("session.json")).unwrap();
    cloud.model = Some("gpt-5".into());
    cloud.reasoning_effort = Some("minimal".into());
    cloud.route_api_method = Some("openai-oauth@cloud-openai".into());
    storage::write_json(&stage.join("session.json"), &cloud).unwrap();
    tar_dir(
        &stage,
        &home.join("return-fixture.tar"),
        &["session.json", "manifest.json", "return-completion-v1"],
    )
    .unwrap();
    run_return(Some(&local.id), &target, false, true).unwrap();
    let returned = session::Session::load(&local.id).unwrap();
    assert_eq!(returned.model, local.model);
    assert_eq!(returned.reasoning_effort, local.reasoning_effort);
    assert_eq!(
        crate::provider::MultiProvider::model_switch_request_for_session_route(
            returned.model.as_deref().unwrap(),
            returned.provider_key.as_deref(),
            returned.route_api_method.as_deref()
        ),
        format!(
            "{}:claude-sonnet-4-5",
            local.route_api_method.as_deref().unwrap()
        )
    );
}

#[cfg(unix)]
#[test]
fn return_rejects_a_replacement_login_after_the_archive_snapshot() {
    let sandbox = AuthTestSandbox::new().unwrap();
    let (local, target, source) = return_fixture(sandbox.root(), false);
    let stage = sandbox.root().join("return-stage");
    let cloud: session::Session = read_json(&stage.join("session.json")).unwrap();
    let captured = cloud.account_pins["claude"].record_id.as_deref().unwrap();
    select_home(&sandbox.root().join("cloud"));
    claude::upsert_account(claude::AnthropicAccount {
        record_id: None,
        label: cloud.account_pins["claude"].label.clone(),
        access: "replacement-access".into(),
        refresh: "replacement-refresh".into(),
        expires: i64::MAX,
        email: None,
        subscription_type: None,
        scopes: vec![],
    })
    .unwrap();
    assert!(
        account_pins::export_cloud_pin(TransferProvider::Claude, &cloud.account_pins["claude"])
            .is_err()
    );
    let replacement = transfer::export_cloud_account_at(
        &storage::jcode_dir().unwrap(),
        TransferProvider::Claude,
        Some(&cloud.account_pins["claude"].label),
    )
    .unwrap();
    std::fs::write(source.join("claude.payload"), replacement.as_bytes()).unwrap();
    // The private host rejects an export guarded by the captured, now-removed record.
    let script = source.join("return-transport.sh");
    let original = std::fs::read_to_string(&script).unwrap();
    std::fs::write(&script, original.replace(
        "printf 'credentials\\n' >> \"$JCODE_HOME/return-calls\"",
        &format!("case \"$1\" in *\"'--expected-pin'\"*{captured}*) exit 23 ;; esac\nprintf 'credentials\\n' >> \"$JCODE_HOME/return-calls\"")
    )).unwrap();
    select_home(&source);
    let before = std::fs::read(source.join("auth.json")).unwrap();
    assert!(run_return(Some(&local.id), &target, false, false).is_err());
    assert_eq!(std::fs::read(source.join("auth.json")).unwrap(), before);
    let unchanged = session::Session::load(&local.id).unwrap();
    assert_eq!(unchanged.account_pins, local.account_pins);
    assert!(
        std::fs::read_to_string(source.join("return-calls"))
            .unwrap()
            .lines()
            .any(|line| line == "cancel")
    );
}

#[test]
fn credential_rpc_inherits_the_global_provider_in_both_argument_positions() {
    // The full application's debug parser exceeds Rust's default test-thread stack.
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            use crate::cli::args::{Args, CloudCommand, Command};
            use clap::Parser;
            for provider in ["claude", "openai"] {
                for before in [false, true] {
                    let mut args = vec!["jcode"];
                    if before {
                        args.extend(["--provider", provider]);
                    }
                    args.extend(["cloud", "credentials", "--account", "fixture", "--export"]);
                    if !before {
                        args.extend(["--provider", provider]);
                    }
                    args.extend(["--expected-pin", "{}"]);
                    let parsed = Args::try_parse_from(args).unwrap();
                    let Some(Command::Cloud(CloudCommand::Credentials {
                        provider: actual,
                        expected_pin,
                        export,
                        ..
                    })) = parsed.command
                    else {
                        panic!("expected credential command")
                    };
                    assert_eq!(actual.as_arg_value(), provider);
                    assert_eq!(expected_pin.as_deref(), Some("{}"));
                    assert!(export);
                }
            }
            assert!(
                Args::try_parse_from([
                    "jcode",
                    "cloud",
                    "credentials",
                    "--provider",
                    "claude",
                    "--account",
                    "fixture",
                    "--expected-pin",
                    "{}"
                ])
                .is_err()
            );
        })
        .unwrap()
        .join()
        .unwrap();
}
