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
