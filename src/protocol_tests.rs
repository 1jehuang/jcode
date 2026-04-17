use super::*;
use anyhow::{Result, anyhow};

fn parse_request_json(json: &str) -> Result<Request> {
    serde_json::from_str(json).map_err(Into::into)
}

fn parse_event_json(json: &str) -> Result<ServerEvent> {
    serde_json::from_str(json).map_err(Into::into)
}

#[test]
fn test_request_roundtrip() -> Result<()> {
    let req = Request::Message {
        id: 1,
        content: "hello".to_string(),
        images: vec![],
        system_reminder: None,
    };
    let json = serde_json::to_string(&req)?;
    let decoded = parse_request_json(&json)?;
    assert_eq!(decoded.id(), 1);
    Ok(())
}

#[test]
fn test_event_roundtrip() -> Result<()> {
    let event = ServerEvent::TextDelta {
        text: "hello".to_string(),
    };
    let json = encode_event(&event);
    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::TextDelta { text } = decoded else {
        return Err(anyhow!("wrong event type"));
    };
    assert_eq!(text, "hello");
    Ok(())
}

#[test]
fn test_interrupted_event_decodes_from_json() -> Result<()> {
    let json = r#"{"type":"interrupted"}"#;
    let decoded = parse_event_json(json)?;
    let ServerEvent::Interrupted = decoded else {
        return Err(anyhow!("wrong event type"));
    };
    Ok(())
}

#[test]
fn test_connection_type_event_roundtrip() -> Result<()> {
    let event = ServerEvent::ConnectionType {
        connection: "websocket".to_string(),
    };
    let json = encode_event(&event);
    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::ConnectionType { connection } = decoded else {
        return Err(anyhow!("wrong event type"));
    };
    assert_eq!(connection, "websocket");
    Ok(())
}

#[test]
fn test_status_detail_event_roundtrip() -> Result<()> {
    let event = ServerEvent::StatusDetail {
        detail: "reusing websocket".to_string(),
    };
    let json = encode_event(&event);
    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::StatusDetail { detail } = decoded else {
        return Err(anyhow!("wrong event type"));
    };
    assert_eq!(detail, "reusing websocket");
    Ok(())
}

#[test]
fn test_interrupted_event_roundtrip() -> Result<()> {
    let event = ServerEvent::Interrupted;
    let json = encode_event(&event);
    assert!(json.contains("\"type\":\"interrupted\""));
    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::Interrupted = decoded else {
        return Err(anyhow!("wrong event type"));
    };
    Ok(())
}

#[test]
fn test_history_event_decodes_without_compaction_mode_for_older_servers() -> Result<()> {
    let json = r#"{
            "type":"history",
            "id":1,
            "session_id":"ses_test_123",
            "messages":[],
            "provider_name":"openai",
            "provider_model":"gpt-5.4",
            "available_models":["gpt-5.4"],
            "connection_type":"websocket"
        }"#;
    let decoded = parse_event_json(json)?;
    let ServerEvent::History {
        provider_name,
        provider_model,
        available_models,
        connection_type,
        compaction_mode,
        side_panel,
        ..
    } = decoded
    else {
        return Err(anyhow!("wrong event type"));
    };
    assert_eq!(provider_name.as_deref(), Some("openai"));
    assert_eq!(provider_model.as_deref(), Some("gpt-5.4"));
    assert_eq!(available_models, vec!["gpt-5.4"]);
    assert_eq!(connection_type.as_deref(), Some("websocket"));
    assert_eq!(compaction_mode, crate::config::CompactionMode::Reactive);
    assert!(!side_panel.has_pages());
    Ok(())
}

#[test]
fn test_error_event_retry_after_roundtrip() -> Result<()> {
    let event = ServerEvent::Error {
        id: 42,
        message: "rate limited".to_string(),
        retry_after_secs: Some(17),
    };
    let json = encode_event(&event);
    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::Error {
        id,
        message,
        retry_after_secs,
    } = decoded
    else {
        return Err(anyhow!("wrong event type"));
    };
    assert_eq!(id, 42);
    assert_eq!(message, "rate limited");
    assert_eq!(retry_after_secs, Some(17));
    Ok(())
}

#[test]
fn test_error_event_retry_after_back_compat_default() -> Result<()> {
    let json = r#"{"type":"error","id":7,"message":"oops"}"#;
    let decoded = parse_event_json(json)?;
    let ServerEvent::Error {
        id,
        message,
        retry_after_secs,
    } = decoded
    else {
        return Err(anyhow!("wrong event type"));
    };
    assert_eq!(id, 7);
    assert_eq!(message, "oops");
    assert_eq!(retry_after_secs, None);
    Ok(())
}

#[test]
fn test_comm_propose_plan_roundtrip() -> Result<()> {
    let req = Request::CommProposePlan {
        id: 42,
        session_id: "sess_a".to_string(),
        items: vec![PlanItem {
            content: "Refactor parser".to_string(),
            status: "pending".to_string(),
            priority: "high".to_string(),
            id: "p1".to_string(),
            blocked_by: vec!["p0".to_string()],
            assigned_to: Some("sess_b".to_string()),
        }],
    };
    let json = serde_json::to_string(&req)?;
    let decoded = parse_request_json(&json)?;
    assert_eq!(decoded.id(), 42);
    let Request::CommProposePlan { items, .. } = decoded else {
        return Err(anyhow!("wrong request type"));
    };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "p1");
    Ok(())
}

#[test]
fn test_stdin_response_roundtrip() -> Result<()> {
    let req = Request::StdinResponse {
        id: 99,
        request_id: "stdin-call_abc-1".to_string(),
        input: "my_password".to_string(),
    };
    let json = serde_json::to_string(&req)?;
    assert!(json.contains("\"type\":\"stdin_response\""));
    assert!(json.contains("\"request_id\":\"stdin-call_abc-1\""));
    assert!(json.contains("\"input\":\"my_password\""));

    let decoded = parse_request_json(&json)?;
    assert_eq!(decoded.id(), 99);
    let Request::StdinResponse {
        request_id, input, ..
    } = decoded
    else {
        return Err(anyhow!("expected StdinResponse"));
    };
    assert_eq!(request_id, "stdin-call_abc-1");
    assert_eq!(input, "my_password");
    Ok(())
}

#[test]
fn test_stdin_response_deserialize_from_json() -> Result<()> {
    let json = r#"{"type":"stdin_response","id":5,"request_id":"req-42","input":"hello world"}"#;
    let decoded = parse_request_json(json)?;
    assert_eq!(decoded.id(), 5);
    let Request::StdinResponse {
        request_id, input, ..
    } = decoded
    else {
        return Err(anyhow!("expected StdinResponse"));
    };
    assert_eq!(request_id, "req-42");
    assert_eq!(input, "hello world");
    Ok(())
}

#[test]
fn test_stdin_request_event_roundtrip() -> Result<()> {
    let event = ServerEvent::StdinRequest {
        request_id: "stdin-xyz-1".to_string(),
        prompt: "Password: ".to_string(),
        is_password: true,
        tool_call_id: "call_abc".to_string(),
    };
    let json = encode_event(&event);
    assert!(json.contains("\"type\":\"stdin_request\""));
    assert!(json.contains("\"is_password\":true"));

    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::StdinRequest {
        request_id,
        prompt,
        is_password,
        tool_call_id,
    } = decoded
    else {
        return Err(anyhow!("expected StdinRequest"));
    };
    assert_eq!(request_id, "stdin-xyz-1");
    assert_eq!(prompt, "Password: ");
    assert!(is_password);
    assert_eq!(tool_call_id, "call_abc");
    Ok(())
}

#[test]
fn test_stdin_request_event_defaults() -> Result<()> {
    // is_password defaults to false when not present
    let json = r#"{"type":"stdin_request","request_id":"r1","prompt":"","tool_call_id":"tc1"}"#;
    let decoded = parse_event_json(json)?;
    let ServerEvent::StdinRequest { is_password, .. } = decoded else {
        return Err(anyhow!("expected StdinRequest"));
    };
    assert!(!is_password, "is_password should default to false");
    Ok(())
}

#[test]
fn test_comm_await_members_roundtrip() -> Result<()> {
    let req = Request::CommAwaitMembers {
        id: 55,
        session_id: "sess_waiter".to_string(),
        target_status: vec!["completed".to_string(), "stopped".to_string()],
        session_ids: vec!["sess_a".to_string(), "sess_b".to_string()],
        timeout_secs: Some(120),
    };
    let json = serde_json::to_string(&req)?;
    assert!(json.contains("\"type\":\"comm_await_members\""));
    let decoded = parse_request_json(&json)?;
    assert_eq!(decoded.id(), 55);
    let Request::CommAwaitMembers {
        session_id,
        target_status,
        session_ids,
        timeout_secs,
        ..
    } = decoded
    else {
        return Err(anyhow!("expected CommAwaitMembers"));
    };
    assert_eq!(session_id, "sess_waiter");
    assert_eq!(target_status, vec!["completed", "stopped"]);
    assert_eq!(session_ids, vec!["sess_a", "sess_b"]);
    assert_eq!(timeout_secs, Some(120));
    Ok(())
}

#[test]
fn test_comm_await_members_defaults() -> Result<()> {
    let json =
        r#"{"type":"comm_await_members","id":1,"session_id":"s1","target_status":["completed"]}"#;
    let decoded = parse_request_json(json)?;
    let Request::CommAwaitMembers {
        session_ids,
        timeout_secs,
        ..
    } = decoded
    else {
        return Err(anyhow!("expected CommAwaitMembers"));
    };
    assert!(
        session_ids.is_empty(),
        "session_ids should default to empty"
    );
    assert_eq!(timeout_secs, None, "timeout_secs should default to None");
    Ok(())
}

#[test]
fn test_comm_await_members_response_roundtrip() -> Result<()> {
    let event = ServerEvent::CommAwaitMembersResponse {
        id: 55,
        completed: true,
        members: vec![
            AwaitedMemberStatus {
                session_id: "sess_a".to_string(),
                friendly_name: Some("fox".to_string()),
                status: "completed".to_string(),
                done: true,
            },
            AwaitedMemberStatus {
                session_id: "sess_b".to_string(),
                friendly_name: Some("wolf".to_string()),
                status: "stopped".to_string(),
                done: true,
            },
        ],
        summary: "All 2 members are done: fox, wolf".to_string(),
    };
    let json = encode_event(&event);
    assert!(json.contains("\"type\":\"comm_await_members_response\""));
    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::CommAwaitMembersResponse {
        id,
        completed,
        members,
        summary,
    } = decoded
    else {
        return Err(anyhow!("expected CommAwaitMembersResponse"));
    };
    assert_eq!(id, 55);
    assert!(completed);
    assert_eq!(members.len(), 2);
    assert_eq!(members[0].friendly_name.as_deref(), Some("fox"));
    assert!(members[0].done);
    assert_eq!(members[1].status, "stopped");
    assert!(summary.contains("fox"));
    Ok(())
}

#[test]
fn test_comm_task_control_roundtrip() -> Result<()> {
    let req = Request::CommTaskControl {
        id: 58,
        session_id: "sess_coord".to_string(),
        action: "salvage".to_string(),
        task_id: "task_42".to_string(),
        target_session: Some("sess_replacement".to_string()),
        message: Some("Recover partial progress first.".to_string()),
    };
    let json = serde_json::to_string(&req)?;
    assert!(json.contains("\"type\":\"comm_task_control\""));
    let decoded = parse_request_json(&json)?;
    assert_eq!(decoded.id(), 58);
    let Request::CommTaskControl {
        session_id,
        action,
        task_id,
        target_session,
        message,
        ..
    } = decoded
    else {
        return Err(anyhow!("expected CommTaskControl"));
    };
    assert_eq!(session_id, "sess_coord");
    assert_eq!(action, "salvage");
    assert_eq!(task_id, "task_42");
    assert_eq!(target_session.as_deref(), Some("sess_replacement"));
    assert_eq!(message.as_deref(), Some("Recover partial progress first."));
    Ok(())
}

#[test]
fn test_comm_assign_task_roundtrip_without_explicit_task_id() -> Result<()> {
    let req = Request::CommAssignTask {
        id: 57,
        session_id: "sess_coord".to_string(),
        target_session: "sess_worker".to_string(),
        task_id: None,
        message: Some("Take the next highest-priority runnable task.".to_string()),
    };
    let json = serde_json::to_string(&req)?;
    assert!(json.contains("\"type\":\"comm_assign_task\""));
    assert!(!json.contains("\"task_id\""));
    let decoded = parse_request_json(&json)?;
    assert_eq!(decoded.id(), 57);
    let Request::CommAssignTask {
        session_id,
        target_session,
        task_id,
        message,
        ..
    } = decoded
    else {
        return Err(anyhow!("expected CommAssignTask"));
    };
    assert_eq!(session_id, "sess_coord");
    assert_eq!(target_session, "sess_worker");
    assert_eq!(task_id, None);
    assert_eq!(
        message.as_deref(),
        Some("Take the next highest-priority runnable task.")
    );
    Ok(())
}

#[test]
fn test_comm_status_roundtrip() -> Result<()> {
    let req = Request::CommStatus {
        id: 56,
        session_id: "sess_watcher".to_string(),
        target_session: "sess_peer".to_string(),
    };
    let json = serde_json::to_string(&req)?;
    assert!(json.contains("\"type\":\"comm_status\""));
    let decoded = parse_request_json(&json)?;
    assert_eq!(decoded.id(), 56);
    let Request::CommStatus {
        session_id,
        target_session,
        ..
    } = decoded
    else {
        return Err(anyhow!("expected CommStatus"));
    };
    assert_eq!(session_id, "sess_watcher");
    assert_eq!(target_session, "sess_peer");
    Ok(())
}

#[test]
fn test_comm_members_roundtrip_includes_status() -> Result<()> {
    let event = ServerEvent::CommMembers {
        id: 9,
        members: vec![AgentInfo {
            session_id: "sess-peer".to_string(),
            friendly_name: Some("bear".to_string()),
            files_touched: vec!["src/main.rs".to_string()],
            status: Some("running".to_string()),
            detail: Some("working on tests".to_string()),
            role: Some("agent".to_string()),
            is_headless: Some(true),
            live_attachments: Some(0),
            status_age_secs: Some(12),
        }],
    };

    let json = encode_event(&event);
    assert!(json.contains("\"type\":\"comm_members\""));
    assert!(json.contains("\"status\":\"running\""));

    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::CommMembers { id, members } = decoded else {
        return Err(anyhow!("expected CommMembers"));
    };
    assert_eq!(id, 9);
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].friendly_name.as_deref(), Some("bear"));
    assert_eq!(members[0].status.as_deref(), Some("running"));
    assert_eq!(members[0].detail.as_deref(), Some("working on tests"));
    assert_eq!(members[0].is_headless, Some(true));
    assert_eq!(members[0].live_attachments, Some(0));
    assert_eq!(members[0].status_age_secs, Some(12));
    Ok(())
}

#[test]
fn test_comm_status_response_roundtrip() -> Result<()> {
    let event = ServerEvent::CommStatusResponse {
        id: 57,
        snapshot: AgentStatusSnapshot {
            session_id: "sess-peer".to_string(),
            friendly_name: Some("bear".to_string()),
            swarm_id: Some("swarm-test".to_string()),
            status: Some("running".to_string()),
            detail: Some("working on tests".to_string()),
            role: Some("agent".to_string()),
            is_headless: Some(true),
            live_attachments: Some(0),
            status_age_secs: Some(5),
            joined_age_secs: Some(30),
            files_touched: vec!["src/main.rs".to_string()],
            activity: Some(SessionActivitySnapshot {
                is_processing: true,
                current_tool_name: Some("bash".to_string()),
            }),
            provider_name: None,
            provider_model: None,
        },
    };

    let json = encode_event(&event);
    assert!(json.contains("\"type\":\"comm_status_response\""));
    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::CommStatusResponse { id, snapshot } = decoded else {
        return Err(anyhow!("expected CommStatusResponse"));
    };
    assert_eq!(id, 57);
    assert_eq!(snapshot.session_id, "sess-peer");
    assert_eq!(snapshot.friendly_name.as_deref(), Some("bear"));
    assert_eq!(
        snapshot
            .activity
            .and_then(|activity| activity.current_tool_name),
        Some("bash".to_string())
    );
    Ok(())
}

#[test]
fn test_transcript_request_roundtrip() -> Result<()> {
    let req = Request::Transcript {
        id: 77,
        text: "hello from whisper".to_string(),
        mode: TranscriptMode::Send,
        session_id: Some("sess_abc".to_string()),
    };
    let json = serde_json::to_string(&req)?;
    assert!(json.contains("\"type\":\"transcript\""));
    let decoded = parse_request_json(&json)?;
    assert_eq!(decoded.id(), 77);
    let Request::Transcript {
        text,
        mode,
        session_id,
        ..
    } = decoded
    else {
        return Err(anyhow!("expected Transcript request"));
    };
    assert_eq!(text, "hello from whisper");
    assert_eq!(mode, TranscriptMode::Send);
    assert_eq!(session_id.as_deref(), Some("sess_abc"));
    Ok(())
}

#[test]
fn test_transcript_event_roundtrip() -> Result<()> {
    let event = ServerEvent::Transcript {
        text: "dictated text".to_string(),
        mode: TranscriptMode::Replace,
    };
    let json = encode_event(&event);
    assert!(json.contains("\"type\":\"transcript\""));
    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::Transcript { text, mode } = decoded else {
        return Err(anyhow!("expected Transcript event"));
    };
    assert_eq!(text, "dictated text");
    assert_eq!(mode, TranscriptMode::Replace);
    Ok(())
}

#[test]
fn test_memory_activity_event_roundtrip() -> Result<()> {
    let event = ServerEvent::MemoryActivity {
        activity: MemoryActivitySnapshot {
            state: MemoryStateSnapshot::SidecarChecking { count: 3 },
            state_age_ms: 275,
            pipeline: Some(MemoryPipelineSnapshot {
                search: MemoryStepStatusSnapshot::Done,
                search_result: Some(MemoryStepResultSnapshot {
                    summary: "5 hits".to_string(),
                    latency_ms: 14,
                }),
                verify: MemoryStepStatusSnapshot::Running,
                verify_result: None,
                verify_progress: Some((1, 3)),
                inject: MemoryStepStatusSnapshot::Pending,
                inject_result: None,
                maintain: MemoryStepStatusSnapshot::Pending,
                maintain_result: None,
            }),
        },
    };

    let json = encode_event(&event);
    assert!(json.contains("\"type\":\"memory_activity\""));
    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::MemoryActivity { activity } = decoded else {
        return Err(anyhow!("expected MemoryActivity event"));
    };
    assert_eq!(
        activity.state,
        MemoryStateSnapshot::SidecarChecking { count: 3 }
    );
    assert_eq!(activity.state_age_ms, 275);
    let pipeline = activity
        .pipeline
        .ok_or_else(|| anyhow!("pipeline snapshot"))?;
    assert_eq!(pipeline.search, MemoryStepStatusSnapshot::Done);
    assert_eq!(pipeline.verify, MemoryStepStatusSnapshot::Running);
    assert_eq!(pipeline.verify_progress, Some((1, 3)));
    Ok(())
}

#[test]
fn test_input_shell_request_roundtrip() -> Result<()> {
    let req = Request::InputShell {
        id: 88,
        command: "ls -la".to_string(),
    };
    let json = serde_json::to_string(&req)?;
    assert!(json.contains("\"type\":\"input_shell\""));
    let decoded = parse_request_json(&json)?;
    assert_eq!(decoded.id(), 88);
    let Request::InputShell { id, command } = decoded else {
        return Err(anyhow!("expected InputShell request"));
    };
    assert_eq!(id, 88);
    assert_eq!(command, "ls -la");
    Ok(())
}

#[test]
fn test_input_shell_result_event_roundtrip() -> Result<()> {
    let event = ServerEvent::InputShellResult {
        result: crate::message::InputShellResult {
            command: "pwd".to_string(),
            cwd: Some("/tmp/project".to_string()),
            output: "/tmp/project\n".to_string(),
            exit_code: Some(0),
            duration_ms: 7,
            truncated: false,
            failed_to_start: false,
        },
    };
    let json = encode_event(&event);
    assert!(json.contains("\"type\":\"input_shell_result\""));
    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::InputShellResult { result } = decoded else {
        return Err(anyhow!("expected InputShellResult event"));
    };
    assert_eq!(result.command, "pwd");
    assert_eq!(result.cwd.as_deref(), Some("/tmp/project"));
    assert_eq!(result.exit_code, Some(0));
    Ok(())
}
