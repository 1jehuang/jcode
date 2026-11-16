use super::*;

fn fixture() -> (tempfile::TempDir, Group) {
    let stage = tempfile::tempdir().unwrap();
    let repo = RepoManifest {
        root: "/isolated-group-repo".into(),
        working_dir: "/isolated-group-repo".into(),
        branch: Some("main".into()),
        head: "a".repeat(40),
        snapshot: "b".repeat(40),
        dirty: true,
        origin_url: None,
        git_user_name: None,
        git_user_email: None,
        ignored_files: vec![],
    };
    let ids = ["coordinator", "worker", "opted-in"];
    let mut members = Vec::new();
    for id in ids {
        let member_dir = stage.path().join("members").join(id);
        std::fs::create_dir_all(&member_dir).unwrap();
        let mut session = session::Session::create_with_id(id.into(), None, None);
        session.migration_epoch = 4;
        session.working_dir = Some(repo.root.clone());
        storage::write_json(&member_dir.join("session.json"), &session).unwrap();
        std::fs::write(
            member_dir.join("todos.json"),
            br#"{"items":[{"id":"lane-task","content":"preserve me"}]}"#,
        )
        .unwrap();
        members.push(Member {
            manifest: MoveManifest {
                format_version: FORMAT_VERSION,
                session_id: id.into(),
                epoch: 4,
                source_host: "local-test".into(),
                target_host: "cloud-test".into(),
                created_at: "2026-10-26T00:00:00Z".into(),
                repo: Some(repo.clone()),
                env: vec![],
                toolchain_hints: vec![],
                not_transferred: vec![],
                mcp_servers: vec![],
                auth_account: None,
                auth_accounts: vec![],
            },
            session_sha256: digest(&member_dir.join("session.json")).unwrap(),
            todos_sha256: Some(digest(&member_dir.join("todos.json")).unwrap()),
        });
    }
    let swarm = serde_json::json!({
        "version":1, "transfer_id":"group-test", "destination_host":"cloud-test", "swarms":[],
        "sessions":ids.iter().map(|id| serde_json::json!({"session_id":id,"epoch":4,"previous_lease":null})).collect::<Vec<_>>()
    });
    (
        stage,
        Group {
            version: GROUP_VERSION,
            preview: false,
            excluded_session_ids: vec![],
            coordinator: "coordinator".into(),
            members,
            swarm,
        },
    )
}

#[test]
fn complete_member_transcripts_and_todos_are_verified() {
    let (stage, group) = fixture();
    validate(&group, stage.path()).unwrap();
    std::fs::write(stage.path().join("members/worker/todos.json"), "{}").unwrap();
    assert!(
        validate(&group, stage.path())
            .unwrap_err()
            .to_string()
            .contains("todo integrity")
    );
}

#[test]
fn receiver_rejects_partial_duplicate_and_forged_membership() {
    let (stage, group) = fixture();
    let mut partial = group.clone();
    partial.swarm["sessions"].as_array_mut().unwrap().pop();
    assert!(validate(&partial, stage.path()).is_err());
    let mut duplicate = group.clone();
    duplicate.members[1] = duplicate.members[0].clone();
    assert!(validate(&duplicate, stage.path()).is_err());
    let mut traversal = group.clone();
    traversal.members[0].manifest.session_id = "../escape".into();
    assert!(validate(&traversal, stage.path()).is_err());
    let mut epoch = group.clone();
    epoch.members[1].manifest.epoch += 1;
    assert!(validate(&epoch, stage.path()).is_err());
}

#[test]
fn all_members_must_share_exact_snapshot_and_host() {
    let (stage, group) = fixture();
    let mut changed = group.clone();
    changed.members[1].manifest.repo.as_mut().unwrap().snapshot = "c".repeat(40);
    assert!(validate(&changed, stage.path()).is_err());
    let mut changed = group.clone();
    changed.members[1].manifest.target_host = "different-host".into();
    assert!(validate(&changed, stage.path()).is_err());
}

#[test]
fn receipt_authenticates_topology_and_every_member() {
    let (_, group) = fixture();
    let expected = receipt(&group).unwrap();
    let mut changed = group.clone();
    changed.swarm["swarms"] = serde_json::json!([{"swarm_id":"changed"}]);
    assert_ne!(receipt(&changed).unwrap(), expected);
    let mut changed = group.clone();
    changed.members[1].todos_sha256 = None;
    assert_ne!(receipt(&changed).unwrap(), expected);
    assert_eq!(expected.members.len(), 3);
}

#[test]
fn ambiguous_or_completed_activation_cannot_restore_source_leases() {
    let (_, group) = fixture();
    for phase in [Phase::Activating, Phase::Complete] {
        let journal = Journal {
            group: group.clone(),
            previous: vec![None; 3],
            phase,
        };
        assert!(
            restore_preparing(&journal)
                .unwrap_err()
                .to_string()
                .contains("cannot restore")
        );
    }
}

#[test]
fn durable_journal_survives_replace_and_preserves_member_epochs() {
    let (stage, group) = fixture();
    let mut journal = Journal {
        group,
        previous: vec![None; 3],
        phase: Phase::Preparing,
    };
    save_journal(stage.path(), &journal).unwrap();
    journal.phase = Phase::Activating;
    save_journal(stage.path(), &journal).unwrap();
    let restored: Journal = read_json(&stage.path().join("journal.json")).unwrap();
    assert_eq!(restored.phase, Phase::Activating);
    assert_eq!(
        receipt(&restored.group).unwrap(),
        receipt(&journal.group).unwrap()
    );
    assert_eq!(
        std::fs::read_dir(stage.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp"))
            .count(),
        0
    );
}

#[test]
fn group_guidance_defers_worker_resume_until_shared_setup() {
    let (_, group) = fixture();
    let ids = group
        .members
        .iter()
        .map(|m| m.manifest.session_id.clone())
        .collect::<Vec<_>>();
    let coordinator = group_arrival_notice(&group.members[0].manifest, &group.coordinator, &ids);
    assert!(coordinator.contains("group machine handoff"));
    assert!(coordinator.contains("Then explicitly resume"));
    assert!(coordinator.contains("Group return is not implemented"));
    assert!(!coordinator.contains("jcode cloud return"));
    let worker = group_arrival_notice(&group.members[1].manifest, &group.coordinator, &ids);
    assert!(worker.contains("do not independently repeat workspace setup"));
    assert!(worker.contains("Remain paused"));
    assert!(!worker.contains("after a machine handoff."));
}

#[test]
fn preview_receive_drops_staging_and_does_not_modify_repository() {
    let (repository, bundle_stage, repo) = super::super::repo_tests::repository();
    let (stage, mut group) = fixture();
    group.preview = true;
    for member in &mut group.members {
        member.manifest.repo = Some(repo.clone());
    }
    std::fs::copy(
        bundle_stage.path().join("repo.bundle"),
        stage.path().join("repo.bundle"),
    )
    .unwrap();
    storage::write_json(&stage.path().join("group.json"), &group).unwrap();
    let stage_path = stage.path().to_path_buf();
    let before = git(repository.path(), &["status", "--porcelain"]).unwrap();
    receive(stage).unwrap();
    assert!(
        !stage_path.exists(),
        "preview must not keep its inbox staging"
    );
    assert_eq!(
        git(repository.path(), &["status", "--porcelain"]).unwrap(),
        before
    );
    // A real follow-up is a different transaction at the same epoch, not an
    // identity collision with a persisted preview.
    group.preview = false;
    assert_ne!(receipt(&group).unwrap().manifest_sha256, {
        group.preview = true;
        receipt(&group).unwrap().manifest_sha256
    });
}

#[test]
fn inventory_reports_only_omitted_sessions_in_same_repository() {
    let (repository, _, _) = super::super::repo_tests::repository();
    let inventory = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let included = format!("included-{}", uuid::Uuid::new_v4());
    let excluded = format!("excluded-{}", uuid::Uuid::new_v4());
    for (id, directory) in [
        (included.as_str(), repository.path()),
        (excluded.as_str(), repository.path()),
        ("other-repo", other.path()),
    ] {
        storage::write_json(
            &inventory.path().join(format!("{id}.json")),
            &serde_json::json!({"id":id,"working_dir":directory}),
        )
        .unwrap();
    }
    assert_eq!(
        excluded_local_sessions_at(
            inventory.path(),
            &repository.path().canonicalize().unwrap(),
            &[included]
        )
        .unwrap(),
        vec![excluded]
    );
}
