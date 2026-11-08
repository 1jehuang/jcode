use super::*;

#[cfg(unix)]
#[test]
fn inventory_projection_uses_bounded_transport_without_mixing_timing_into_json() {
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _guard = HomeGuard::install(home.path());
    let id = "session_inventory_transport";
    let mut session = Session::create_with_id(
        id.into(),
        None,
        Some("Private title must stay out of timing records".into()),
    );
    session.migration_epoch = 8;
    session.save().unwrap();
    let mut command = crate::tui::cloud_git::script_command(
        None,
        None,
        id,
        &super::super::super::cloud_preview::projection_script("inventory"),
    );
    command.env("JCODE_HOME", home.path());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (status, bytes) = runtime
        .block_on(crate::tui::cloud_git::bounded_output(
            command,
            Duration::from_secs(5),
            1024 * 1024,
        ))
        .unwrap();
    assert!(status.success());
    let inventory: VerifiedInventory = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(inventory.sessions.len(), 1);
    assert_eq!(inventory.sessions[0].session_id, id);
    assert_eq!(inventory.sessions[0].epoch, 8);
    assert_eq!(
        inventory.sessions[0].title.as_deref(),
        Some("Private title must stay out of timing records")
    );
}

struct HomeGuard(Option<std::ffi::OsString>);

impl HomeGuard {
    fn install(home: &Path) -> Self {
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", home);
        Self(previous)
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.0.take() {
            Some(previous) => crate::env::set_var("JCODE_HOME", previous),
            None => crate::env::remove_var("JCODE_HOME"),
        }
        super::super::invalidate_session_list_cache();
    }
}

fn metadata(id: &str) -> crate::mobile_sync::SyncedSession {
    crate::mobile_sync::SyncedSession {
        session_id: id.into(),
        source: "cloud".into(),
        title: Some("Future orbital compiler work".into()),
        host: Some("verified-worker".into()),
        cwd: Some("/metadata/repository".into()),
        updated_at: Some("2026-11-08T05:00:00Z".into()),
        created_at: None,
    }
}

#[test]
fn signed_in_fresh_client_can_discover_without_existing_ssh_config() {
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _home = HomeGuard::install(home.path());
    struct Environment(Vec<(&'static str, Option<std::ffi::OsString>)>);
    impl Drop for Environment {
        fn drop(&mut self) {
            for (name, value) in self.0.drain(..) {
                match value {
                    Some(value) => crate::env::set_var(name, value),
                    None => crate::env::remove_var(name),
                }
            }
        }
    }
    let names = [
        "JCODE_API_KEY",
        "JCODE_CLOUD_ENV_SESSION",
        "JCODE_CLOUD_MACHINE",
        "JCODE_SERVER_NAME",
    ];
    let _environment = Environment(
        names
            .iter()
            .map(|&name| (name, std::env::var_os(name)))
            .collect(),
    );
    for name in names {
        crate::env::remove_var(name);
    }
    crate::env::set_var("JCODE_API_KEY", "test-account-key");
    assert!(!home.path().join("cloud/ssh_config").exists());
    assert!(
        available(),
        "sign-in, not preexisting SSH files, enables discovery"
    );
    crate::env::set_var("JCODE_API_KEY", "  ");
    assert!(
        !available(),
        "blank credentials must not trigger network IO"
    );
    crate::env::set_var("JCODE_API_KEY", "test-account-key");
    crate::env::set_var("JCODE_CLOUD_MACHINE", "1");
    assert!(
        !available(),
        "cloud hosts must not recursively discover themselves"
    );
}

fn inventory(ids: &[(&str, u64)]) -> VerifiedInventory {
    VerifiedInventory {
        host: "verified-worker".into(),
        sessions: ids
            .iter()
            .map(|(id, epoch)| VerifiedSession {
                session_id: (*id).into(),
                epoch: *epoch,
                cwd: Some("/verified/repository".into()),
                title: Some("Future orbital compiler work".into()),
                updated_at: Some("2026-11-08T05:00:00Z".into()),
            })
            .collect(),
    }
}

#[test]
fn discovered_future_session_has_verified_lease_searchable_title_and_no_transcript() {
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _guard = HomeGuard::install(home.path());
    let id = "session_future_orbital_1999999999999";
    assert!(register(home.path(), vec![metadata(id)], inventory(&[(id, 42)])).unwrap());
    let lease = storage::read_session_lease(id).expect("discovered cloud ownership");
    assert_eq!(lease.epoch, 42);
    assert_eq!(lease.away_host.as_deref(), Some("jcode-cloud"));
    assert_eq!(lease.repo_root.as_deref(), Some("/verified/repository"));
    assert_eq!(lease.updated_at, "2026-11-08T05:00:00Z");
    assert!(lease.last_sync_snapshot.is_none());
    assert!(lease.last_sync_head.is_none());
    assert!(
        !home
            .path()
            .join("sessions")
            .join(format!("{id}.json"))
            .exists()
    );
    let stub = super::super::cloud_session_stub(&lease);
    assert_eq!(stub.title, "Future orbital compiler work");
    assert!(stub.search_index.contains("orbital"));
    assert!(stub.search_index.contains("compiler"));
    super::super::invalidate_session_list_cache();
    let (groups, orphans) = super::super::load_sessions_grouped().unwrap();
    let group = groups
        .iter()
        .find(|group| group.name == "cloud: jcode-cloud")
        .expect("cloud session picker group");
    assert_eq!(group.icon, "☁");
    assert_eq!(group.sessions.len(), 1);
    assert_eq!(group.sessions[0].id, id);
    assert!(group.sessions[0].search_index.contains("orbital"));
    assert!(!orphans.iter().any(|session| session.id == id));
    assert!(
        !home
            .path()
            .join("sessions")
            .join(format!("{id}.json"))
            .exists()
    );
}

#[test]
fn registration_is_idempotent_and_preserves_returned_leases_and_local_snapshots() {
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _guard = HomeGuard::install(home.path());
    let id = "session_inventory_idempotent";
    assert!(register(home.path(), vec![metadata(id)], inventory(&[(id, 7)])).unwrap());
    let lease_path = home
        .path()
        .join("session_leases")
        .join(format!("{id}.json"));
    let original = std::fs::read(&lease_path).unwrap();
    let mut changed_metadata = metadata(id);
    changed_metadata.title = Some("Must not replace original title".into());
    assert!(!register(home.path(), vec![changed_metadata], inventory(&[(id, 99)])).unwrap());
    assert_eq!(std::fs::read(&lease_path).unwrap(), original);
    assert_eq!(title(id).as_deref(), Some("Future orbital compiler work"));

    let returned_id = "session_inventory_returned";
    let returned = storage::SessionLease {
        session_id: returned_id.into(),
        epoch: 100,
        away_host: None,
        repo_root: Some("/local/returned".into()),
        last_sync_snapshot: Some("snapshot".into()),
        last_sync_head: Some("head".into()),
        updated_at: "2026-11-08T05:10:00Z".into(),
    };
    storage::write_session_lease(&returned).unwrap();
    let returned_path = home
        .path()
        .join("session_leases")
        .join(format!("{returned_id}.json"));
    let returned_bytes = std::fs::read(&returned_path).unwrap();
    let local_id = "session_inventory_local";
    let local_path = home
        .path()
        .join("sessions")
        .join(format!("{local_id}.json"));
    std::fs::create_dir_all(local_path.parent().unwrap()).unwrap();
    let snapshot = b"{\"id\":\"session_inventory_local\",\"messages\":[]}";
    std::fs::write(&local_path, snapshot).unwrap();
    assert!(
        !register(
            home.path(),
            vec![metadata(returned_id), metadata(local_id)],
            inventory(&[(returned_id, 101), (local_id, 12)])
        )
        .unwrap()
    );
    assert_eq!(std::fs::read(&returned_path).unwrap(), returned_bytes);
    assert_eq!(std::fs::read(&local_path).unwrap(), snapshot);
    assert!(storage::read_session_lease(local_id).is_none());
    assert!(title(returned_id).is_none());
    assert!(title(local_id).is_none());
}

#[test]
fn registration_skips_mismatched_hosts_and_unverified_missing_sessions() {
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _guard = HomeGuard::install(home.path());
    let mismatch_id = "session_inventory_wrong_host";
    let missing_id = "session_inventory_missing";
    let unknown_id = "session_inventory_unknown";
    let mut mismatch = metadata(mismatch_id);
    mismatch.host = Some("different-worker".into());
    assert!(
        !register(
            home.path(),
            vec![mismatch, metadata(missing_id)],
            inventory(&[(mismatch_id, 2), (unknown_id, 3)])
        )
        .unwrap()
    );
    for id in [mismatch_id, missing_id, unknown_id] {
        assert!(storage::read_session_lease(id).is_none());
        assert!(title(id).is_none());
        assert!(
            !home
                .path()
                .join("sessions")
                .join(format!("{id}.json"))
                .exists()
        );
    }
}

#[test]
fn known_cloud_metadata_refreshes_without_changing_migration_or_snapshot() {
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _guard = HomeGuard::install(home.path());
    let id = "session_metadata_refresh";
    assert!(register(home.path(), vec![metadata(id)], inventory(&[(id, 7)])).unwrap());
    let mut lease = storage::read_session_lease(id).unwrap();
    lease.last_sync_snapshot = Some("preserved-snapshot".into());
    lease.last_sync_head = Some("preserved-head".into());
    storage::write_session_lease(&lease).unwrap();
    let mut session = Session::create_with_id(id.into(), None, Some("Old local title".into()));
    session.working_dir = Some("/old/local/project".into());
    session.add_message(
        Role::User,
        vec![crate::message::ContentBlock::Text {
            text: "Historical local conversation".into(),
            cache_control: None,
        }],
    );
    let path = home.path().join("sessions").join(format!("{id}.json"));
    // Model the historical local copy left by migration, without attempting to
    // save an active local session against its deliberately-away lease.
    storage::write_json(&path, &session).unwrap();
    let original = std::fs::read(&path).unwrap();
    assert!(
        should_verify(home.path(), id),
        "known cloud snapshots must refresh too"
    );
    let mut refreshed = inventory(&[(id, 7)]);
    refreshed.sessions[0].title = Some("Updated lunar compiler".into());
    refreshed.sessions[0].cwd = Some("/cloud/new-project".into());
    refreshed.sessions[0].updated_at = Some("2026-11-09T01:00:00Z".into());
    assert!(register(home.path(), vec![metadata(id)], refreshed).unwrap());
    let updated = storage::read_session_lease(id).unwrap();
    assert_eq!(updated.epoch, 7);
    assert_eq!(updated.last_sync_snapshot, lease.last_sync_snapshot);
    assert_eq!(updated.last_sync_head, lease.last_sync_head);
    assert_eq!(updated.repo_root.as_deref(), Some("/cloud/new-project"));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let mut sessions = Vec::new();
    super::super::include_cloud_sessions(
        &home.path().join("sessions"),
        &crate::catchup::CatchupSeenSnapshot::load(),
        &mut sessions,
    );
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, "Updated lunar compiler");
    assert_eq!(
        sessions[0].working_dir.as_deref(),
        Some("/cloud/new-project")
    );
    assert!(sessions[0].search_index.contains("lunar"));
    assert!(sessions[0].search_index.contains("/cloud/new-project"));
    assert_eq!(
        sessions[0].last_message_time.to_rfc3339(),
        "2026-11-09T01:00:00+00:00"
    );
    let mut identical = inventory(&[(id, 7)]);
    identical.sessions[0].title = Some("Updated lunar compiler".into());
    identical.sessions[0].cwd = Some("/cloud/new-project".into());
    identical.sessions[0].updated_at = Some("2026-11-09T01:00:00Z".into());
    assert!(!register(home.path(), vec![metadata(id)], identical).unwrap());
}

#[test]
fn cleared_remote_title_and_directory_do_not_resurrect_stale_api_metadata() {
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _guard = HomeGuard::install(home.path());
    let id = "session_cleared_metadata";
    register(home.path(), vec![metadata(id)], inventory(&[(id, 3)])).unwrap();
    let mut cleared = inventory(&[(id, 3)]);
    cleared.sessions[0].title = None;
    cleared.sessions[0].cwd = None;
    assert!(register(home.path(), vec![metadata(id)], cleared).unwrap());
    assert!(storage::read_session_lease(id).unwrap().repo_root.is_none());
    assert_ne!(title(id).as_deref(), Some("Future orbital compiler work"));
    assert!(title(id).unwrap().contains("cloud session"));
}

#[test]
fn returned_or_other_host_sessions_and_unsafe_ids_are_not_refresh_candidates() {
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _guard = HomeGuard::install(home.path());
    let id = "session_candidate_ownership";
    register(home.path(), vec![metadata(id)], inventory(&[(id, 9)])).unwrap();
    let mut lease = storage::read_session_lease(id).unwrap();
    for host in [None, Some("other-machine".to_string())] {
        lease.away_host = host;
        storage::write_session_lease(&lease).unwrap();
        assert!(!should_verify(home.path(), id));
        assert!(!register(home.path(), vec![metadata(id)], inventory(&[(id, 9)])).unwrap());
        assert_eq!(storage::read_session_lease(id), Some(lease.clone()));
    }
    for id in ["../escape", ".", "session:route", "", "session\nsecond"] {
        assert!(!should_verify(home.path(), id));
        assert!(!register(home.path(), vec![metadata(id)], inventory(&[(id, 0)])).unwrap());
    }
    let id = "session_broken_ownership";
    let path = home
        .path()
        .join("session_leases")
        .join(format!("{id}.json"));
    std::fs::write(&path, b"{broken-lease").unwrap();
    assert!(!should_verify(home.path(), id));
    assert!(!register(home.path(), vec![metadata(id)], inventory(&[(id, 0)])).unwrap());
    assert_eq!(std::fs::read(path).unwrap(), b"{broken-lease");
}

#[cfg(unix)]
#[test]
fn verification_script_reads_authoritative_metadata_and_skips_moved_sessions() {
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _guard = HomeGuard::install(home.path());
    let id = "session_verification_script";
    let mut session =
        Session::create_with_id(id.into(), None, Some("Verified remote title".into()));
    session.working_dir = Some("/remote/current".into());
    session.migration_epoch = 6;
    session.save().unwrap();
    let run = || {
        let output = std::process::Command::new("sh")
            .args([
                "-c",
                &super::super::super::cloud_preview::projection_script("inventory"),
                "verify",
                &format!("{id}\nsession_missing\n../escape"),
            ])
            .env("JCODE_HOME", home.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<VerifiedInventory>(&output.stdout).unwrap()
    };
    let verified = run();
    assert_eq!(verified.sessions.len(), 1);
    assert_eq!(verified.sessions[0].epoch, 6);
    assert_eq!(
        verified.sessions[0].title.as_deref(),
        Some("Verified remote title")
    );
    assert_eq!(verified.sessions[0].cwd.as_deref(), Some("/remote/current"));
    assert!(verified.sessions[0].updated_at.is_some());
    session.custom_title = Some("Renamed by user".into());
    session.save().unwrap();
    assert_eq!(run().sessions[0].title.as_deref(), Some("Renamed by user"));
    storage::write_session_lease(&storage::SessionLease {
        session_id: id.into(),
        epoch: 7,
        away_host: Some("returned-machine".into()),
        repo_root: None,
        last_sync_snapshot: None,
        last_sync_head: None,
        updated_at: "2026-11-08T05:00:00Z".into(),
    })
    .unwrap();
    assert!(run().sessions.is_empty());
}
