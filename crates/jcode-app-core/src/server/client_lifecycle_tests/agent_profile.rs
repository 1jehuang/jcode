//! Independent wire-level acceptance tests: real socket pairs and handle_client,
//! with only inference replaced by the shared prompt-capture provider.
use super::system_prompt::SystemPromptSocketFixture;
use super::*;
use serde_json::{Value, json};
use std::path::Path;

#[path = "agent_profile_sources.rs"]
mod sources;

struct ProfileClient {
    reader: BufReader<crate::transport::ReadHalf>,
    writer: crate::transport::WriteHalf,
    task: tokio::task::JoinHandle<Result<()>>,
}

impl ProfileClient {
    fn connect(fixture: &SystemPromptSocketFixture) -> Self {
        let (task, stream) = fixture.connect();
        let (reader, writer) = stream.into_split();
        Self {
            reader: BufReader::new(reader),
            writer,
            task,
        }
    }

    async fn send(&mut self, request: Value) {
        self.writer
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
    }

    async fn until(&mut self, predicate: impl Fn(&ServerEvent) -> bool) -> ServerEvent {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let mut line = String::new();
                assert!(
                    self.reader.read_line(&mut line).await.unwrap() > 0,
                    "socket closed before response"
                );
                let event: ServerEvent = serde_json::from_str(&line).unwrap();
                if predicate(&event) {
                    return event;
                }
                assert!(
                    !matches!(event, ServerEvent::Error { .. }),
                    "unexpected server error: {event:?}"
                );
            }
        })
        .await
        .expect("profile socket response timeout")
    }

    async fn done(&mut self, id: u64) {
        self.until(|event| matches!(event, ServerEvent::Done { id: event_id } if *event_id == id))
            .await;
    }

    async fn error(&mut self, id: u64) -> String {
        let event = self
            .until(|event| {
                matches!(event,
                    ServerEvent::Error { id: event_id, .. }
                        | ServerEvent::Done { id: event_id }
                        | ServerEvent::RuntimeCommandResult { id: event_id, .. }
                        if *event_id == id
                )
            })
            .await;
        let ServerEvent::Error { message, .. } = event else {
            panic!("request {id} silently succeeded instead of rejecting invalid selection");
        };
        assert!(!message.trim().is_empty());
        message
    }

    async fn session_id(&mut self, id: u64) -> String {
        self.send(serde_json::to_value(Request::GetState { id }).unwrap())
            .await;
        let ServerEvent::State { session_id, .. } = self
            .until(
                |event| matches!(event, ServerEvent::State { id: event_id, .. } if *event_id == id),
            )
            .await
        else {
            unreachable!()
        };
        session_id
    }

    async fn message_prompt(&mut self, fixture: &SystemPromptSocketFixture, id: u64) -> String {
        let before = fixture.captured.lock().unwrap().len();
        self.send(json!({"type":"message", "id":id, "content":"profile acceptance turn"}))
            .await;
        self.done(id).await;
        let captured = fixture.captured.lock().unwrap();
        assert_eq!(
            captured.len(),
            before + 1,
            "one actual provider request per turn"
        );
        captured.last().unwrap().clone()
    }

    async fn agent_command(&mut self, id: u64, args: &[&str]) -> Value {
        self.send(json!({"type":"runtime_command", "id":id, "command":"agent", "args":args}))
            .await;
        let ServerEvent::RuntimeCommandResult { command, data, .. } = self.until(|event| {
            matches!(event, ServerEvent::RuntimeCommandResult { id: event_id, .. } if *event_id == id)
        }).await else { unreachable!() };
        assert_eq!(command, "agent");
        data
    }

    async fn close(self) {
        let Self {
            reader,
            writer,
            task,
        } = self;
        drop(writer);
        drop(reader);
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

fn write_profile(directory: &Path, name: &str, instructions: &str) -> std::path::PathBuf {
    std::fs::create_dir_all(directory).unwrap();
    let path = directory.join(format!("{name}.md"));
    std::fs::write(&path, instructions).unwrap();
    path
}

fn assert_saved_profile(session_id: &str, expected: &str) {
    // Read the on-disk session, not the live Agent or the profile loader.
    let saved = crate::session::Session::load(session_id).unwrap();
    let profile = saved
        .agent_profile
        .as_ref()
        .expect("persisted named profile");
    assert_eq!(profile.name, "socket-profile");
    assert_eq!(profile.instructions, expected);
    assert!(
        saved.system_prompt.is_none(),
        "a profile must not replace the normal prompt"
    );
}

#[tokio::test]
async fn agent_profile_socket_creation_snapshot_resume_fork_and_isolation() {
    let _lock = crate::storage::lock_test_env();
    let home = IsolatedReloadRecoveryEnv::new();

    // Run the public creation path for both global-only lookup and project
    // override. Equal names with different literal content catch wrong precedence.
    for project_override in [false, true] {
        let project = tempfile::TempDir::new().unwrap();
        let global_path = write_profile(
            &home._home.path().join("agents"),
            "socket-profile",
            "SOCKET_GLOBAL_PROFILE_V1_68319",
        );
        let selected_path = if project_override {
            write_profile(
                &project.path().join(".jcode/agents"),
                "socket-profile",
                "SOCKET_PROJECT_PROFILE_V1_92517",
            )
        } else {
            global_path
        };
        let expected = if project_override {
            "SOCKET_PROJECT_PROFILE_V1_92517"
        } else {
            "SOCKET_GLOBAL_PROFILE_V1_68319"
        };
        std::fs::create_dir_all(project.path().join(".jcode")).unwrap();
        std::fs::write(
            project.path().join(".jcode/prompt-overlay.md"),
            "SOCKET_NORMAL_OVERLAY_34189",
        )
        .unwrap();

        let fixture = SystemPromptSocketFixture::new();
        let mut owner = ProfileClient::connect(&fixture);
        owner.send(json!({"type":"subscribe", "id":1, "working_dir":project.path(), "agent_profile_name":"socket-profile"})).await;
        owner.done(1).await;
        let parent_id = owner.session_id(2).await;
        assert_saved_profile(&parent_id, expected);
        assert_eq!(
            crate::session::Session::load(&parent_id)
                .unwrap()
                .visible_conversation_message_count(),
            0
        );
        assert!(
            fixture.captured.lock().unwrap().is_empty(),
            "Subscribe must persist without inference"
        );

        // Merely having profiles on disk must never auto-select one, even in
        // the same project while another session has that profile selected.
        let mut independent = ProfileClient::connect(&fixture);
        independent
            .send(json!({"type":"subscribe", "id":3, "working_dir":project.path()}))
            .await;
        independent.done(3).await;
        let independent_id = independent.session_id(4).await;
        assert_ne!(independent_id, parent_id);
        let plain = independent.message_prompt(&fixture, 5).await;
        assert!(
            crate::session::Session::load(&independent_id)
                .unwrap()
                .agent_profile
                .is_none()
        );
        assert!(plain.contains("You are Jcode."));
        assert!(plain.contains("SOCKET_NORMAL_OVERLAY_34189"));
        assert!(!plain.contains("SOCKET_GLOBAL_PROFILE_V1_68319"));
        assert!(!plain.contains("SOCKET_PROJECT_PROFILE_V1_92517"));

        let first = owner.message_prompt(&fixture, 6).await;
        assert!(
            first.contains("You are Jcode."),
            "normal base prompt must survive profile selection"
        );
        assert!(!first.contains("SOCKET_NORMAL_OVERLAY_34189"));
        assert!(
            first.contains(expected),
            "selected file's literal instructions must reach the provider"
        );
        if project_override {
            assert!(!first.contains("SOCKET_GLOBAL_PROFILE_V1_68319"));
        }

        std::fs::write(&selected_path, "SOCKET_MUTATED_PROFILE_V2_47291").unwrap();
        let unchanged = owner.message_prompt(&fixture, 7).await;
        assert!(unchanged.contains(expected));
        assert!(!unchanged.contains("SOCKET_NORMAL_OVERLAY_34189"));
        assert!(!unchanged.contains("SOCKET_MUTATED_PROFILE_V2_47291"));
        assert_saved_profile(&parent_id, expected);

        // Drop the actual owner connection and evict the live cache, so the
        // reconnect must restore the persisted snapshot, not reread the file.
        owner.close().await;
        fixture.sessions.write().await.remove(&parent_id);
        let mut reconnected = ProfileClient::connect(&fixture);
        reconnected.send(json!({"type":"subscribe", "id":8, "working_dir":project.path(), "target_session_id":parent_id})).await;
        reconnected.done(8).await;
        assert_eq!(reconnected.session_id(9).await, parent_id);
        let restored = reconnected.message_prompt(&fixture, 10).await;
        assert!(restored.contains(expected));
        assert!(!restored.contains("SOCKET_NORMAL_OVERLAY_34189"));
        assert!(restored.contains("You are Jcode."));
        assert!(!restored.contains("SOCKET_MUTATED_PROFILE_V2_47291"));

        // Exercise the explicit public resume path separately from attachment.
        reconnected.send(json!({"type":"clear", "id":11})).await;
        reconnected.done(11).await;
        let cleared = reconnected.message_prompt(&fixture, 12).await;
        assert!(!cleared.contains(expected));
        assert!(cleared.contains("SOCKET_NORMAL_OVERLAY_34189"));
        assert!(!cleared.contains("SOCKET_MUTATED_PROFILE_V2_47291"));
        fixture.sessions.write().await.remove(&parent_id);
        reconnected
            .send(json!({"type":"resume_session", "id":13, "session_id":parent_id}))
            .await;
        reconnected.done(13).await;
        let resumed = reconnected.message_prompt(&fixture, 14).await;
        assert!(resumed.contains(expected));
        assert!(!resumed.contains("SOCKET_NORMAL_OVERLAY_34189"));
        assert!(!resumed.contains("SOCKET_MUTATED_PROFILE_V2_47291"));
        assert_saved_profile(&parent_id, expected);

        reconnected.send(json!({"type":"split", "id":15})).await;
        let ServerEvent::SplitResponse {
            new_session_id: child_id,
            ..
        } = reconnected
            .until(|event| matches!(event, ServerEvent::SplitResponse { id: 15, .. }))
            .await
        else {
            unreachable!()
        };
        assert_ne!(child_id, parent_id);
        assert_saved_profile(&child_id, expected);
        let mut fork = ProfileClient::connect(&fixture);
        fork.send(json!({"type":"subscribe", "id":16, "working_dir":project.path(), "target_session_id":child_id})).await;
        fork.done(16).await;
        let fork_prompt = fork.message_prompt(&fixture, 17).await;
        assert!(fork_prompt.contains(expected));
        assert!(!fork_prompt.contains("SOCKET_NORMAL_OVERLAY_34189"));
        assert!(fork_prompt.contains("You are Jcode."));
        assert!(!fork_prompt.contains("SOCKET_MUTATED_PROFILE_V2_47291"));
        let still_plain = independent.message_prompt(&fixture, 18).await;
        assert!(!still_plain.contains(expected));
        assert!(still_plain.contains("SOCKET_NORMAL_OVERLAY_34189"));
        assert!(!still_plain.contains("SOCKET_MUTATED_PROFILE_V2_47291"));

        fork.close().await;
        reconnected.close().await;
        independent.close().await;
    }
}

#[tokio::test]
async fn agent_profile_socket_runtime_select_reload_off() {
    let _lock = crate::storage::lock_test_env();
    let _home = IsolatedReloadRecoveryEnv::new();
    let project = tempfile::TempDir::new().unwrap();
    let profile_path = write_profile(
        &project.path().join(".jcode/agents"),
        "socket-profile",
        "SOCKET_RUNTIME_PROFILE_V1_53971",
    );
    let fixture = SystemPromptSocketFixture::new();
    let mut client = ProfileClient::connect(&fixture);
    client
        .send(json!({"type":"subscribe", "id":101, "working_dir":project.path()}))
        .await;
    client.done(101).await;
    let session_id = client.session_id(102).await;
    let listed = client.agent_command(103, &[]).await;
    assert!(listed["current"].is_null());
    assert!(
        listed["available"]
            .as_array()
            .unwrap()
            .contains(&json!("socket-profile"))
    );
    assert_eq!(listed["changed"], false);

    let selected = client.agent_command(104, &["socket-profile"]).await;
    assert_eq!(selected["current"], "socket-profile");
    assert_eq!(selected["changed"], true);
    assert_eq!(selected["effective"], "next_turn");
    assert_saved_profile(&session_id, "SOCKET_RUNTIME_PROFILE_V1_53971");
    assert!(
        fixture.captured.lock().unwrap().is_empty(),
        "selection must save before a message"
    );
    let selected_prompt = client.message_prompt(&fixture, 105).await;
    assert!(selected_prompt.contains("You are Jcode."));
    assert!(selected_prompt.contains("SOCKET_RUNTIME_PROFILE_V1_53971"));

    std::fs::write(&profile_path, "SOCKET_RUNTIME_PROFILE_V2_80463").unwrap();
    let before_reload = client.message_prompt(&fixture, 106).await;
    assert!(before_reload.contains("SOCKET_RUNTIME_PROFILE_V1_53971"));
    assert!(!before_reload.contains("SOCKET_RUNTIME_PROFILE_V2_80463"));
    let reloaded = client.agent_command(107, &["reload"]).await;
    assert_eq!(reloaded["current"], "socket-profile");
    assert_eq!(reloaded["changed"], true);
    assert_saved_profile(&session_id, "SOCKET_RUNTIME_PROFILE_V2_80463");
    let after_reload = client.message_prompt(&fixture, 108).await;
    assert!(after_reload.contains("You are Jcode."));
    assert!(after_reload.contains("SOCKET_RUNTIME_PROFILE_V2_80463"));
    assert!(!after_reload.contains("SOCKET_RUNTIME_PROFILE_V1_53971"));

    let off = client.agent_command(109, &["off"]).await;
    assert!(off["current"].is_null());
    assert_eq!(off["changed"], true);
    assert!(
        crate::session::Session::load(&session_id)
            .unwrap()
            .agent_profile
            .is_none()
    );
    let disabled = client.message_prompt(&fixture, 110).await;
    assert!(disabled.contains("You are Jcode."));
    assert!(!disabled.contains("SOCKET_RUNTIME_PROFILE_V1_53971"));
    assert!(!disabled.contains("SOCKET_RUNTIME_PROFILE_V2_80463"));
    client.close().await;
}

#[tokio::test]
async fn agent_profile_socket_yaml_frontmatter_body_snapshot_reload_and_errors() {
    let _lock = crate::storage::lock_test_env();
    let _home = IsolatedReloadRecoveryEnv::new();
    let project = tempfile::TempDir::new().unwrap();
    const BODY_V1: &str = "  SOCKET_YAML_BODY_V1_59271\n\nKeep this trailing space. \n";
    const BODY_V2: &str = "\nSOCKET_YAML_BODY_V2_83164\n  Preserve indentation.\n";
    let header_v1 = "---\ndescription: SOCKET_YAML_DESCRIPTION_V1_73418\nmode: subagent\nname: SOCKET_YAML_NOT_FILENAME_62893\nmodel: socket-yaml-model-v1-47382\npermission:\n  edit: deny\nx_socket_header_v1_95231: ignored\n---\n";
    let header_v2 = "---\ndescription: SOCKET_YAML_DESCRIPTION_V2_37285\nmode: primary\nname: SOCKET_YAML_NOT_FILENAME_V2_54718\nmodel: socket-yaml-model-v2-93841\npermission:\n  edit: allow\nx_socket_header_v2_82649: ignored\n---\n";
    let profile_path = write_profile(
        &project.path().join(".jcode/agents"),
        "socket-profile",
        &format!("{header_v1}{BODY_V1}"),
    );
    let fixture = SystemPromptSocketFixture::new();
    let mut client = ProfileClient::connect(&fixture);
    client
        .send(json!({"type":"subscribe", "id":601, "working_dir":project.path()}))
        .await;
    client.done(601).await;
    let session_id = client.session_id(602).await;
    let baseline = client.message_prompt(&fixture, 603).await;
    assert!(baseline.contains("You are Jcode."));
    assert!(!baseline.contains(BODY_V1));

    client
        .send(json!({"type":"runtime_command", "id":604, "command":"agent", "args":["--select", "socket-profile", "[]"]}))
        .await;
    let ServerEvent::RuntimeCommandResult {
        data: selected,
        message,
        ..
    } = client
        .until(|event| matches!(event, ServerEvent::RuntimeCommandResult { id: 604, .. }))
        .await
    else {
        unreachable!()
    };
    assert!(message.starts_with("Agent profile socket-profile applied."));
    assert!(message.contains("Markdown instructions take effect next turn."));
    assert!(
        message
            .contains("Ignored YAML frontmatter fields: mode, model, permission, x_socket_header_")
    );
    assert!(message.contains("The Markdown instruction body still applies."));
    assert!(!message.contains("settings are not applied"));
    assert_eq!(selected["current"], "socket-profile");
    assert_eq!(selected["changed"], true);
    assert_eq!(selected["effective"], "next_turn");
    assert_saved_profile(&session_id, BODY_V1);
    let selected_prompt = client.message_prompt(&fixture, 605).await;
    assert!(selected_prompt.contains("You are Jcode."));
    assert!(selected_prompt.contains(BODY_V1));
    for metadata in [
        "SOCKET_YAML_DESCRIPTION_V1_73418",
        "SOCKET_YAML_NOT_FILENAME_62893",
        "socket-yaml-model-v1-47382",
        "x_socket_header_v1_95231",
    ] {
        assert!(
            !selected_prompt.contains(metadata),
            "leaked metadata: {metadata}"
        );
    }

    std::fs::write(&profile_path, format!("{header_v2}{BODY_V2}")).unwrap();
    let frozen = client.message_prompt(&fixture, 606).await;
    assert!(frozen.contains(BODY_V1));
    assert!(!frozen.contains(BODY_V2));
    assert_saved_profile(&session_id, BODY_V1);

    client
        .send(json!({"type":"runtime_command", "id":607, "command":"agent", "args":["reload"]}))
        .await;
    let ServerEvent::RuntimeCommandResult {
        data: reloaded,
        message,
        ..
    } = client
        .until(|event| matches!(event, ServerEvent::RuntimeCommandResult { id: 607, .. }))
        .await
    else {
        unreachable!()
    };
    assert!(message.starts_with("Agent profile socket-profile applied."));
    assert!(message.contains("Markdown instructions take effect next turn."));
    assert!(
        message
            .contains("Ignored YAML frontmatter fields: mode, model, permission, x_socket_header_")
    );
    assert!(message.contains("The Markdown instruction body still applies."));
    assert!(!message.contains("settings are not applied"));
    assert_eq!(reloaded["current"], "socket-profile");
    assert_eq!(reloaded["changed"], true);
    assert_saved_profile(&session_id, BODY_V2);
    let reloaded_prompt = client.message_prompt(&fixture, 608).await;
    assert!(reloaded_prompt.contains("You are Jcode."));
    assert!(reloaded_prompt.contains(BODY_V2));
    assert!(!reloaded_prompt.contains(BODY_V1));
    for metadata in [
        "SOCKET_YAML_DESCRIPTION_V2_37285",
        "SOCKET_YAML_NOT_FILENAME_V2_54718",
        "socket-yaml-model-v2-93841",
        "x_socket_header_v2_82649",
    ] {
        assert!(
            !reloaded_prompt.contains(metadata),
            "leaked metadata: {metadata}"
        );
    }

    for (id, invalid) in [
        (
            609,
            "---\ndescription: [unterminated\n---\nSOCKET_YAML_INVALID_BODY_41973",
        ),
        (
            611,
            "---\ndescription: SOCKET_YAML_EMPTY_BODY_63824\n---\n \n\t",
        ),
    ] {
        std::fs::write(&profile_path, invalid).unwrap();
        let before = fixture.captured.lock().unwrap().len();
        client
            .send(json!({"type":"runtime_command", "id":id, "command":"agent", "args":["reload"]}))
            .await;
        client.error(id).await;
        assert_eq!(fixture.captured.lock().unwrap().len(), before);
        assert_saved_profile(&session_id, BODY_V2);
        let unchanged = client.message_prompt(&fixture, id + 1).await;
        assert!(unchanged.contains("You are Jcode."));
        assert!(unchanged.contains(BODY_V2));
        assert!(!unchanged.contains("SOCKET_YAML_INVALID_BODY_41973"));
        assert!(!unchanged.contains("SOCKET_YAML_EMPTY_BODY_63824"));
    }
    client.close().await;
}

#[tokio::test]
async fn agent_profile_socket_rejects_invalid_selection_and_existing_session_replacement() {
    let _lock = crate::storage::lock_test_env();
    let home = IsolatedReloadRecoveryEnv::new();
    let project = tempfile::TempDir::new().unwrap();
    let project_agents = project.path().join(".jcode/agents");
    let global_agents = home._home.path().join("agents");
    write_profile(
        &project_agents,
        "socket-profile",
        "SOCKET_ORIGINAL_SELECTION_72943",
    );
    write_profile(
        &project_agents,
        "replacement",
        "SOCKET_FORBIDDEN_REPLACEMENT_81623",
    );
    for name in ["empty-profile", "yaml-profile"] {
        write_profile(
            &global_agents,
            name,
            "SOCKET_FORBIDDEN_GLOBAL_FALLBACK_28519",
        );
    }
    write_profile(&project_agents, "empty-profile", " \n\t");
    write_profile(
        &project_agents,
        "yaml-profile",
        "---\nname: [unterminated\n---\nSOCKET_FORBIDDEN_YAML_47831",
    );
    let fixture = SystemPromptSocketFixture::new();

    for name in [
        "missing-profile-93871",
        "../replacement",
        "off",
        "empty-profile",
        "yaml-profile",
    ] {
        let mut invalid = ProfileClient::connect(&fixture);
        let before = fixture.captured.lock().unwrap().len();
        invalid.send(json!({"type":"subscribe", "id":201, "working_dir":project.path(), "agent_profile_name":name})).await;
        invalid.error(201).await;
        assert_eq!(
            fixture.captured.lock().unwrap().len(),
            before,
            "invalid selection must not invoke inference"
        );
        // Recover on the same socket without a selection. A malformed project
        // file must not silently fall back to its valid global namesake.
        invalid
            .send(json!({"type":"subscribe", "id":202, "working_dir":project.path()}))
            .await;
        invalid.done(202).await;
        let session_id = invalid.session_id(203).await;
        let prompt = invalid.message_prompt(&fixture, 204).await;
        assert!(prompt.contains("You are Jcode."));
        assert!(!prompt.contains("SOCKET_FORBIDDEN_GLOBAL_FALLBACK_28519"));
        assert!(!prompt.contains("SOCKET_FORBIDDEN_REPLACEMENT_81623"));
        assert!(!prompt.contains("SOCKET_FORBIDDEN_YAML_47831"));
        assert!(
            crate::session::Session::load(&session_id)
                .unwrap()
                .agent_profile
                .is_none()
        );
        invalid.close().await;
    }

    let mut owner = ProfileClient::connect(&fixture);
    owner.send(json!({"type":"subscribe", "id":301, "working_dir":project.path(), "agent_profile_name":"socket-profile"})).await;
    owner.done(301).await;
    let parent_id = owner.session_id(302).await;
    owner.send(json!({"type":"subscribe", "id":303, "working_dir":project.path(), "agent_profile_name":"replacement"})).await;
    assert!(owner.error(303).await.contains("creating a new session"));
    assert_saved_profile(&parent_id, "SOCKET_ORIGINAL_SELECTION_72943");
    let after_repeat = owner.message_prompt(&fixture, 304).await;
    assert!(after_repeat.contains("SOCKET_ORIGINAL_SELECTION_72943"));
    assert!(!after_repeat.contains("SOCKET_FORBIDDEN_REPLACEMENT_81623"));

    let mut attaching = ProfileClient::connect(&fixture);
    attaching.send(json!({"type":"subscribe", "id":305, "working_dir":project.path(), "target_session_id":parent_id, "agent_profile_name":"replacement"})).await;
    assert!(
        attaching
            .error(305)
            .await
            .contains("creating a new session")
    );
    assert_saved_profile(&parent_id, "SOCKET_ORIGINAL_SELECTION_72943");
    owner.close().await;
    attaching.send(json!({"type":"subscribe", "id":306, "working_dir":project.path(), "target_session_id":parent_id})).await;
    attaching.done(306).await;
    assert_eq!(attaching.session_id(307).await, parent_id);
    let after_attach = attaching.message_prompt(&fixture, 308).await;
    assert!(after_attach.contains("SOCKET_ORIGINAL_SELECTION_72943"));
    assert!(!after_attach.contains("SOCKET_FORBIDDEN_REPLACEMENT_81623"));
    attaching.close().await;
}

#[tokio::test]
async fn agent_profile_socket_registered_active_turn_rejects_mutations_with_unlocked_agent() {
    let _lock = crate::storage::lock_test_env();
    let _home = IsolatedReloadRecoveryEnv::new();
    let project = tempfile::TempDir::new().unwrap();
    let agents = project.path().join(".jcode/agents");
    let selected_path = write_profile(&agents, "socket-profile", "SOCKET_BUSY_ORIGINAL_57329");
    write_profile(
        &agents,
        "replacement",
        "SOCKET_BUSY_REPLACEMENT_FORBIDDEN_81439",
    );
    let fixture = SystemPromptSocketFixture::new();
    let mut client = ProfileClient::connect(&fixture);
    client.send(json!({"type":"subscribe", "id":401, "working_dir":project.path(), "agent_profile_name":"socket-profile"})).await;
    client.done(401).await;
    let session_id = client.session_id(402).await;
    assert_saved_profile(&session_id, "SOCKET_BUSY_ORIGINAL_57329");
    std::fs::write(selected_path, "SOCKET_BUSY_RELOAD_FORBIDDEN_29683").unwrap();

    // Model an autonomous/other-owner turn during a gap where it has released
    // the Agent mutex. The public socket must honor the registered turn lease,
    // not mistake an available mutex for an idle session.
    let agent = fixture
        .sessions
        .read()
        .await
        .get(&session_id)
        .unwrap()
        .clone();
    let active_turn =
        crate::turn_cancel_registry::register_active_turn(&session_id, InterruptSignal::new());
    for (id, args) in [
        (403, vec!["replacement"]),
        (404, vec!["off"]),
        (405, vec!["reload"]),
        (407, vec!["--select", "replacement", "[]"]),
        (408, vec!["--sources", "[]"]),
    ] {
        assert!(
            agent.try_lock().is_ok(),
            "regression requires an unlocked Agent"
        );
        client
            .send(json!({"type":"runtime_command", "id":id, "command":"agent", "args":args}))
            .await;
        let error = client.error(id).await;
        assert!(
            error.to_ascii_lowercase().contains("busy"),
            "expected busy rejection, got: {error}"
        );
        assert_saved_profile(&session_id, "SOCKET_BUSY_ORIGINAL_57329");
        assert!(
            fixture.captured.lock().unwrap().is_empty(),
            "rejected mutation must not invoke the provider"
        );
        assert!(agent.try_lock().is_ok());
    }
    drop(active_turn);
    let prompt = client.message_prompt(&fixture, 406).await;
    assert!(prompt.contains("You are Jcode."));
    assert!(prompt.contains("SOCKET_BUSY_ORIGINAL_57329"));
    assert!(!prompt.contains("SOCKET_BUSY_REPLACEMENT_FORBIDDEN_81439"));
    assert!(!prompt.contains("SOCKET_BUSY_RELOAD_FORBIDDEN_29683"));
    client.close().await;
}
