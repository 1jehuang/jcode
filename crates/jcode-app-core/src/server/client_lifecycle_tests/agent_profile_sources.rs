use super::*;

#[tokio::test]
async fn agent_profile_socket_source_options_are_atomic_snapshotted_and_reload_preserved() {
    let _lock = crate::storage::lock_test_env();
    let _home = IsolatedReloadRecoveryEnv::new();
    let project = tempfile::tempdir().unwrap();
    let profile_path = write_profile(
        &project.path().join(".jcode/agents"),
        "socket-profile",
        "PROFILE_INITIAL_68241",
    );
    write_profile(
        &project.path().join(".jcode/agents"),
        "other",
        "OTHER_PROFILE_68241",
    );
    let agents_path = project.path().join("AGENTS.md");
    std::fs::write(&agents_path, "PROJECT_AGENTS_68241").unwrap();
    let overlay = crate::storage::jcode_dir()
        .unwrap()
        .join("prompt-overlay.md");
    std::fs::create_dir_all(overlay.parent().unwrap()).unwrap();
    std::fs::write(&overlay, "GLOBAL_OVERLAY_SNAPSHOT_68241").unwrap();
    let fixture = SystemPromptSocketFixture::new();
    let mut client = ProfileClient::connect(&fixture);
    client
        .send(json!({"type":"subscribe", "id":501, "working_dir":project.path()}))
        .await;
    client.done(501).await;
    let session_id = client.session_id(502).await;
    let listing = client.agent_command(503, &[]).await;
    assert!(listing["current"].is_null());
    assert_eq!(listing["included_sources"], json!([]));
    assert_eq!(listing["instruction_scope"], "jcode");
    let choices = listing["instruction_sources"].as_array().unwrap();
    let overlay_canonical = std::fs::canonicalize(&overlay).unwrap();
    let source = choices
        .iter()
        .find(|source| source["path"] == json!(overlay_canonical))
        .unwrap();
    assert!(source["available"].as_bool().unwrap());
    assert!(
        source.get("content").is_none(),
        "source bodies are not client metadata"
    );
    let selected_id = source["id"].as_str().unwrap().to_owned();
    let encoded = serde_json::to_string(&[&selected_id]).unwrap();
    let normal = client.message_prompt(&fixture, 504).await;
    assert!(normal.contains("PROJECT_AGENTS_68241"));
    assert!(normal.contains("GLOBAL_OVERLAY_SNAPSHOT_68241"));
    let selected = client
        .agent_command(505, &["--select", "socket-profile", &encoded])
        .await;
    assert_eq!(selected["included_sources"], json!([selected_id]));
    let prompt = client.message_prompt(&fixture, 506).await;
    assert_eq!(prompt.matches("PROFILE_INITIAL_68241").count(), 1);
    assert!(prompt.contains("GLOBAL_OVERLAY_SNAPSHOT_68241"));
    assert!(!prompt.contains("PROJECT_AGENTS_68241"));
    std::fs::write(&profile_path, "PROFILE_RELOADED_68241").unwrap();
    std::fs::write(&overlay, "GLOBAL_OVERLAY_REFRESHED_68241").unwrap();
    client.agent_command(507, &["reload"]).await;
    let prompt = client.message_prompt(&fixture, 508).await;
    assert!(prompt.contains("PROFILE_RELOADED_68241"));
    assert!(prompt.contains("GLOBAL_OVERLAY_SNAPSHOT_68241"));
    assert!(!prompt.contains("GLOBAL_OVERLAY_REFRESHED_68241"));
    client.agent_command(509, &["--sources", &encoded]).await;
    let prompt = client.message_prompt(&fixture, 510).await;
    assert!(prompt.contains("GLOBAL_OVERLAY_REFRESHED_68241"));
    let saved = crate::session::Session::load(&session_id)
        .unwrap()
        .agent_profile;
    for (id, args) in [
        (511, vec!["--sources", "{}"]),
        (512, vec!["--sources", "[\"file:/arbitrary-file\"]"]),
        (513, vec!["--select", "other", "[\"unknown\"]"]),
        (514, vec!["--select", "missing", "[]"]),
    ] {
        client
            .send(json!({"type":"runtime_command", "id":id, "command":"agent", "args":args}))
            .await;
        client.error(id).await;
        assert_eq!(
            crate::session::Session::load(&session_id)
                .unwrap()
                .agent_profile,
            saved
        );
    }
    std::fs::remove_file(&overlay).unwrap();
    let retained = client.agent_command(515, &["--sources", &encoded]).await;
    assert!(
        retained["instruction_sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|source| {
                source["id"] == selected_id
                    && source["included"] == true
                    && source["available"] == false
            })
    );
    let prompt = client.message_prompt(&fixture, 516).await;
    assert!(prompt.contains("GLOBAL_OVERLAY_REFRESHED_68241"));
    let switched = client.agent_command(517, &["other"]).await;
    assert_eq!(switched["included_sources"], json!([]));
    let prompt = client.message_prompt(&fixture, 518).await;
    assert!(prompt.contains("OTHER_PROFILE_68241"));
    assert!(!prompt.contains("GLOBAL_OVERLAY_"));
    assert!(!prompt.contains("PROJECT_AGENTS_68241"));
    client.agent_command(519, &["off"]).await;
    let normal = client.message_prompt(&fixture, 520).await;
    assert!(normal.contains("PROJECT_AGENTS_68241"));
    assert!(!normal.contains("OTHER_PROFILE_68241"));
    client.close().await;
}

#[tokio::test]
async fn agent_profile_socket_stale_same_session_source_draft_cannot_edit_other_clients_profile() {
    let _lock = crate::storage::lock_test_env();
    let _home = IsolatedReloadRecoveryEnv::new();
    let project = tempfile::tempdir().unwrap();
    let agents = project.path().join(".jcode/agents");
    write_profile(&agents, "first", "FIRST_PROFILE_STALE_DRAFT_31897");
    write_profile(&agents, "second", "SECOND_PROFILE_31897");
    let overlay = project.path().join(".jcode/prompt-overlay.md");
    std::fs::write(&overlay, "STALE_DRAFT_EXTRA_FORBIDDEN_31897").unwrap();
    let fixture = SystemPromptSocketFixture::new();
    let mut first = ProfileClient::connect(&fixture);
    first.send(json!({"type":"subscribe", "id":601, "working_dir":project.path(), "agent_profile_name":"first"})).await;
    first.done(601).await;
    let session_id = first.session_id(602).await;
    let listing = first.agent_command(603, &[]).await;
    let source = listing["instruction_sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["path"] == json!(std::fs::canonicalize(&overlay).unwrap()))
        .unwrap();
    let draft = serde_json::to_string(&[source["id"].as_str().unwrap()]).unwrap();
    let mut second = ProfileClient::connect(&fixture);
    second.send(json!({"type":"subscribe", "id":604, "working_dir":project.path(), "target_session_id":session_id})).await;
    second.done(604).await;
    second.agent_command(605, &["second"]).await;
    let saved = crate::session::Session::load(&session_id)
        .unwrap()
        .agent_profile;
    first.send(json!({"type":"runtime_command", "id":606, "command":"agent", "args":["--sources","first",draft]})).await;
    let error = first.error(606).await;
    assert!(error.contains("changed"), "{error}");
    assert_eq!(
        crate::session::Session::load(&session_id)
            .unwrap()
            .agent_profile,
        saved
    );
    let prompt = second.message_prompt(&fixture, 607).await;
    assert!(prompt.contains("SECOND_PROFILE_31897"));
    assert!(!prompt.contains("STALE_DRAFT_EXTRA_FORBIDDEN_31897"));
    second.close().await;
    first.close().await;
}
