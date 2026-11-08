use super::*;

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

fn inventory(ids: &[(&str, u64)]) -> VerifiedInventory {
    VerifiedInventory {
        host: "verified-worker".into(),
        sessions: ids
            .iter()
            .map(|(id, epoch)| VerifiedSession {
                session_id: (*id).into(),
                epoch: *epoch,
                cwd: Some("/verified/repository".into()),
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
