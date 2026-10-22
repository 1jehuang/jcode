//! Rewind acceptance path through the daemon socket and real file editing tools.
use super::*;
use crate::protocol::RewindMode;
use crate::session::Session;
use serde_json::json;

#[tokio::test]
async fn checkpoint_tools_restore_edits_replacements_moves_and_partial_patches() {
    use crate::message::Role;
    use crate::tool::{ToolContext, ToolExecutionMode};
    let _env_lock = crate::storage::lock_test_env();
    let _environment = IsolatedReloadRecoveryEnv::new();
    let work = tempfile::tempdir().unwrap();
    let directory = std::fs::canonicalize(work.path()).unwrap();
    let initial = [
        ("source.txt", b"original draft\n".as_slice()),
        ("destination.txt", b"existing destination\n".as_slice()),
        ("delete.txt", b"retain this\n".as_slice()),
        ("binary.dat", &[0, 255, 3, 128]),
    ];
    for (name, bytes) in initial {
        std::fs::write(directory.join(name), bytes).unwrap();
    }
    let mut session = Session::create(None, None);
    session.working_dir = Some(directory.to_string_lossy().into_owned());
    let checkpoint = session
        .create_rewind_checkpoint("change the files")
        .unwrap();
    let message_id = session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "change the files".into(),
            cache_control: None,
        }],
    );
    session
        .bind_rewind_checkpoint(&checkpoint.id, &message_id)
        .unwrap();
    session.save().unwrap();
    let provider: Arc<dyn Provider> = Arc::new(RewindEditingProvider::default());
    let registry = Registry::new(provider).await;
    let context = ToolContext {
        session_id: session.id.clone(),
        message_id,
        tool_call_id: "rewind-coverage".into(),
        working_dir: Some(directory.clone()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    };
    for (tool, input) in [
        (
            "edit",
            json!({"file_path":"source.txt","old_string":"original","new_string":"edited"}),
        ),
        (
            "replace",
            json!({"path":"source.txt","pattern":"edited","replacement":"replaced","expected_count":1}),
        ),
        (
            "write",
            json!({"file_path":"binary.dat","content":"text now\n"}),
        ),
        (
            "apply_patch",
            json!({"patch_text":"*** Begin Patch\n*** Update File: source.txt\n*** Move to: destination.txt\n@@\n-replaced draft\n+moved draft\n*** Delete File: delete.txt\n*** Add File: created.txt\n+created\n*** End Patch"}),
        ),
        (
            "apply_patch",
            json!({"patch_text":"--- a/destination.txt\n+++ b/destination.txt\n@@ -1 +1 @@\n-moved draft\n+unified patch\n"}),
        ),
    ] {
        registry
            .execute(tool, input, context.clone())
            .await
            .unwrap();
    }
    // A later failure must not hide an earlier mutation in the same patch.
    let _partial = registry.execute("apply_patch", json!({
        "patch_text":"*** Begin Patch\n*** Add File: partial.txt\n+first hunk succeeded\n*** Update File: missing.txt\n@@\n-absent\n+cannot match\n*** End Patch"
    }), context).await;
    assert!(directory.join("partial.txt").exists());
    assert!(!directory.join("source.txt").exists());
    assert!(!directory.join("delete.txt").exists());
    assert_eq!(
        std::fs::read(directory.join("destination.txt")).unwrap(),
        b"unified patch\n"
    );
    let restored = session
        .rewind_checkpoint(&checkpoint.id, RewindMode::FilesOnly)
        .await
        .unwrap();
    assert_eq!(restored.restored_files, 6);
    for (name, bytes) in initial {
        assert_eq!(
            std::fs::read(directory.join(name)).unwrap(),
            bytes,
            "{name}"
        );
    }
    assert!(!directory.join("created.txt").exists());
    assert!(!directory.join("partial.txt").exists());
    assert_eq!(session.messages.len(), 1);
}

#[derive(Clone, Default)]
struct RewindEditingProvider(Arc<std::sync::atomic::AtomicUsize>);

#[async_trait]
impl Provider for RewindEditingProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        let call = self.0.fetch_add(1, Ordering::SeqCst);
        let events = if call.is_multiple_of(2) {
            vec![
                StreamEvent::ToolUseStart {
                    id: format!("rewind-write-{call}"),
                    name: "write".into(),
                },
                StreamEvent::ToolInputDelta(
                    json!({
                        "file_path": "tracked.txt",
                        "content": if call == 0 { "version one\n" } else { "version two\n" },
                        "intent": "Edit the rewind test fixture",
                    })
                    .to_string(),
                ),
                StreamEvent::ToolUseEnd,
                StreamEvent::MessageEnd {
                    stop_reason: Some("tool_use".into()),
                },
            ]
        } else {
            vec![
                StreamEvent::TextDelta("File updated.".into()),
                StreamEvent::MessageEnd {
                    stop_reason: Some("end_turn".into()),
                },
            ]
        };
        Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
    }

    fn name(&self) -> &str {
        "rewind-acceptance"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

async fn send_checkpoint_request(
    writer: &mut crate::transport::WriteHalf,
    request: serde_json::Value,
) {
    writer
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
}

async fn checkpoint_event(
    reader: &mut BufReader<crate::transport::ReadHalf>,
    predicate: impl Fn(&ServerEvent) -> bool,
) -> ServerEvent {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let mut line = String::new();
            assert!(
                reader.read_line(&mut line).await.unwrap() > 0,
                "socket closed before expected rewind event"
            );
            let event: ServerEvent = serde_json::from_str(&line).unwrap();
            if predicate(&event) {
                return event;
            }
        }
    })
    .await
    .expect("rewind socket event timeout")
}

#[tokio::test]
async fn checkpoint_socket_restores_files_and_conversation_and_rejects_conflicts() {
    let _lock = crate::storage::lock_test_env();
    let home = IsolatedReloadRecoveryEnv::new();
    let work = std::fs::canonicalize(home._home.path()).unwrap();
    let file = work.join("tracked.txt");
    std::fs::write(&file, "original uncommitted work\n").unwrap();
    let unrelated = work.join("unrelated.txt");
    std::fs::write(&unrelated, "leave this alone\n").unwrap();
    let provider: Arc<dyn Provider> = Arc::new(RewindEditingProvider::default());
    let sessions: SessionAgents = Arc::new(RwLock::new(HashMap::new()));
    let connections = Arc::new(RwLock::new(HashMap::new()));
    let (_, client_stream, server_task) = {
        let (server_stream, client_stream) = crate::transport::Stream::pair().unwrap();
        let (global_events, _) = broadcast::channel(8);
        let (debug_responses, _) = broadcast::channel(8);
        let (swarm_events, _) = broadcast::channel(8);
        let task = tokio::spawn(handle_client(
            server_stream,
            sessions.clone(),
            global_events,
            provider,
            Arc::new(RwLock::new(false)),
            Arc::new(RwLock::new(String::new())),
            Arc::new(RwLock::new(0usize)),
            connections.clone(),
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(RwLock::new(HashMap::new())),
            FileTouchService::new(),
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(RwLock::new(ClientDebugState::default())),
            debug_responses,
            Arc::new(RwLock::new(std::collections::VecDeque::new())),
            Arc::new(std::sync::atomic::AtomicU64::new(0)),
            swarm_events,
            "rewind-test".into(),
            "test".into(),
            Arc::new(crate::mcp::SharedMcpPool::from_default_config()),
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(RwLock::new(HashMap::new())),
            AwaitMembersRuntime::default(),
            SwarmMutationRuntime::default(),
        ));
        ((), client_stream, task)
    };
    let (reader, mut writer) = client_stream.into_split();
    let mut reader = BufReader::new(reader);
    send_checkpoint_request(
        &mut writer,
        json!({"type":"subscribe","id":1,"working_dir":work}),
    )
    .await;
    checkpoint_event(&mut reader, |event| {
        matches!(event, ServerEvent::Done { id: 1 })
    })
    .await;
    let session_id = sessions.read().await.keys().next().unwrap().clone();

    for (id, prompt, expected) in [
        (2, "first checkpoint", "version one\n"),
        (3, "second checkpoint", "version two\n"),
    ] {
        send_checkpoint_request(
            &mut writer,
            json!({"type":"message","id":id,"content":prompt}),
        )
        .await;
        checkpoint_event(
            &mut reader,
            |event| matches!(event, ServerEvent::Done { id: done } if *done == id),
        )
        .await;
        assert_eq!(std::fs::read_to_string(&file).unwrap(), expected);
    }

    send_checkpoint_request(
        &mut writer,
        json!({"type":"list_rewind_checkpoints","id":4}),
    )
    .await;
    let ServerEvent::RewindCheckpoints {
        checkpoints,
        session_id: listed_session,
        ..
    } = checkpoint_event(&mut reader, |event| {
        matches!(event, ServerEvent::RewindCheckpoints { id: 4, .. })
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(listed_session, session_id);
    assert_eq!(checkpoints.len(), 2);
    let first = checkpoints
        .iter()
        .find(|point| point.prompt == "first checkpoint")
        .unwrap();
    let second = checkpoints
        .iter()
        .find(|point| point.prompt == "second checkpoint")
        .unwrap();
    assert!(second.can_restore_files);

    // Reject a competing attachment before touching either the files or history.
    for connection in connections.write().await.values_mut() {
        connection.is_processing = true;
    }
    send_checkpoint_request(
        &mut writer,
        json!({"type":"rewind_checkpoint","id":5,"checkpoint_id":second.id,"mode":"files_only"}),
    )
    .await;
    let error = checkpoint_event(&mut reader, |event| {
        matches!(event, ServerEvent::Error { id: 5, .. })
    })
    .await;
    assert!(matches!(error, ServerEvent::Error { message, .. } if message.contains("busy")));
    for connection in connections.write().await.values_mut() {
        connection.is_processing = false;
    }
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "version two\n");

    let _held_resume = crate::server::usage_limit_resume::register_pending_resume(&session_id);
    send_checkpoint_request(
        &mut writer,
        json!({"type":"rewind_checkpoint","id":6,"checkpoint_id":second.id,"mode":"files_only"}),
    )
    .await;
    let ServerEvent::RewindComplete { result, .. } = checkpoint_event(&mut reader, |event| {
        matches!(event, ServerEvent::RewindComplete { id: 6, .. })
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(result.mode, RewindMode::FilesOnly);
    assert_eq!(result.removed_messages, 0);
    assert_eq!(result.restored_files, 1);
    assert!(
        crate::server::usage_limit_resume::has_pending_resume(&session_id),
        "files-only keeps the held conversation"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "version one\n");

    // External edits must not be overwritten by either half of a combined restore.
    std::fs::write(&file, "concurrent human edit\n").unwrap();
    send_checkpoint_request(&mut writer, json!({"type":"rewind_checkpoint","id":7,"checkpoint_id":first.id,"mode":"conversation_and_files"})).await;
    checkpoint_event(&mut reader, |event| {
        matches!(event, ServerEvent::Error { id: 7, .. })
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "concurrent human edit\n"
    );
    send_checkpoint_request(
        &mut writer,
        json!({"type":"list_rewind_checkpoints","id":8}),
    )
    .await;
    let ServerEvent::RewindCheckpoints {
        checkpoints: after_conflict,
        ..
    } = checkpoint_event(&mut reader, |event| {
        matches!(event, ServerEvent::RewindCheckpoints { id: 8, .. })
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(
        after_conflict.len(),
        2,
        "failed file restore must keep the conversation"
    );
    std::fs::write(&file, "version one\n").unwrap();

    send_checkpoint_request(&mut writer, json!({"type":"rewind_checkpoint","id":9,"checkpoint_id":first.id,"mode":"conversation_and_files"})).await;
    // History must precede completion so the client can refill its input safely.
    let event = checkpoint_event(&mut reader, |event| {
        matches!(
            event,
            ServerEvent::History { .. }
                | ServerEvent::RewindComplete { id: 9, .. }
                | ServerEvent::Error { id: 9, .. }
        )
    })
    .await;
    assert!(
        matches!(event, ServerEvent::History { .. }),
        "replacement history must precede completion: {event:?}"
    );
    let ServerEvent::RewindComplete { result, .. } = checkpoint_event(&mut reader, |event| {
        matches!(event, ServerEvent::RewindComplete { id: 9, .. })
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(result.prompt, "first checkpoint");
    assert!(
        !crate::server::usage_limit_resume::has_pending_resume(&session_id),
        "removed turns must never auto-resume"
    );
    assert!(result.removed_messages >= 2);
    assert_eq!(result.restored_files, 1);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "original uncommitted work\n"
    );
    assert_eq!(
        std::fs::read_to_string(&unrelated).unwrap(),
        "leave this alone\n"
    );

    // Already-consumed IDs cannot accidentally target a new turn at the same index.
    for (id, mode) in [(10, "conversation_only"), (11, "files_only")] {
        send_checkpoint_request(
            &mut writer,
            json!({"type":"rewind_checkpoint","id":id,"checkpoint_id":second.id,"mode":mode}),
        )
        .await;
        checkpoint_event(&mut reader, |event| {
            matches!(event, ServerEvent::Error { id: response_id, .. } if *response_id == id)
        }).await;
    }
    send_checkpoint_request(
        &mut writer,
        json!({"type":"list_rewind_checkpoints","id":12}),
    )
    .await;
    let ServerEvent::RewindCheckpoints { checkpoints, .. } =
        checkpoint_event(&mut reader, |event| {
            matches!(event, ServerEvent::RewindCheckpoints { id: 12, .. })
        })
        .await
    else {
        unreachable!()
    };
    assert!(
        checkpoints.is_empty(),
        "consumed checkpoints must leave the picker"
    );
    sessions.write().await.clear();
    drop(writer);
    drop(reader);
    tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
