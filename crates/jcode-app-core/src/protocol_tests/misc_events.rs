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

#[test]
fn test_protocol_enum_roundtrips_cover_wire_names() -> Result<()> {
    let transcript_modes = [
        (TranscriptMode::Insert, "insert"),
        (TranscriptMode::Append, "append"),
        (TranscriptMode::Replace, "replace"),
        (TranscriptMode::Send, "send"),
    ];
    for (mode, wire) in transcript_modes {
        let json = serde_json::to_string(&mode)?;
        assert_eq!(json, format!("\"{}\"", wire));
        let decoded: TranscriptMode = serde_json::from_str(&json)?;
        assert_eq!(decoded, mode);
    }

    let delivery_modes = [
        (CommDeliveryMode::Notify, "notify"),
        (CommDeliveryMode::Interrupt, "interrupt"),
        (CommDeliveryMode::Wake, "wake"),
    ];
    for (mode, wire) in delivery_modes {
        let json = serde_json::to_string(&mode)?;
        assert_eq!(json, format!("\"{}\"", wire));
        let decoded: CommDeliveryMode = serde_json::from_str(&json)?;
        assert_eq!(decoded, mode);
    }

    let feature_toggles = [
        (FeatureToggle::Memory, "memory"),
        (FeatureToggle::Swarm, "swarm"),
        (FeatureToggle::Autoreview, "autoreview"),
        (FeatureToggle::Autojudge, "autojudge"),
    ];
    for (feature, wire) in feature_toggles {
        let json = serde_json::to_string(&feature)?;
        assert_eq!(json, format!("\"{}\"", wire));
        let decoded: FeatureToggle = serde_json::from_str(&json)?;
        assert_eq!(decoded, feature);
    }

    Ok(())
}

#[test]
fn test_set_feature_roundtrip() -> Result<()> {
    let req = Request::SetFeature {
        id: 77,
        feature: FeatureToggle::Swarm,
        enabled: true,
    };
    let json = serde_json::to_string(&req)?;
    assert!(json.contains("\"type\":\"set_feature\""));
    let decoded = parse_request_json(&json)?;
    let Request::SetFeature {
        id,
        feature,
        enabled,
    } = decoded
    else {
        return Err(anyhow!("expected SetFeature"));
    };
    assert_eq!(id, 77);
    assert_eq!(feature, FeatureToggle::Swarm);
    assert!(enabled);
    Ok(())
}

#[test]
fn test_subscribe_request_roundtrip_preserves_session_takeover_flags() -> Result<()> {
    let req = Request::Subscribe {
        id: 89,
        working_dir: Some("/tmp/project".to_string()),
        selfdev: Some(true),
        target_session_id: Some("sess_target".to_string()),
        client_instance_id: Some("client-123".to_string()),
        client_has_local_history: true,
        allow_session_takeover: true,
    };
    let json = serde_json::to_string(&req)?;
    assert!(json.contains("\"type\":\"subscribe\""));
    let decoded = parse_request_json(&json)?;
    let Request::Subscribe {
        id,
        working_dir,
        selfdev,
        target_session_id,
        client_instance_id,
        client_has_local_history,
        allow_session_takeover,
    } = decoded
    else {
        return Err(anyhow!("expected Subscribe"));
    };
    assert_eq!(id, 89);
    assert_eq!(working_dir.as_deref(), Some("/tmp/project"));
    assert_eq!(selfdev, Some(true));
    assert_eq!(target_session_id.as_deref(), Some("sess_target"));
    assert_eq!(client_instance_id.as_deref(), Some("client-123"));
    assert!(client_has_local_history);
    assert!(allow_session_takeover);
    Ok(())
}

#[test]
fn test_subscribe_request_defaults_optional_flags() -> Result<()> {
    let json = r#"{"type":"subscribe","id":91}"#;
    let decoded = parse_request_json(json)?;
    let Request::Subscribe {
        id,
        working_dir,
        selfdev,
        target_session_id,
        client_instance_id,
        client_has_local_history,
        allow_session_takeover,
    } = decoded
    else {
        return Err(anyhow!("expected Subscribe"));
    };
    assert_eq!(id, 91);
    assert_eq!(working_dir, None);
    assert_eq!(selfdev, None);
    assert_eq!(target_session_id, None);
    assert_eq!(client_instance_id, None);
    assert!(!client_has_local_history);
    assert!(!allow_session_takeover);
    Ok(())
}

#[test]
fn test_resume_session_defaults_sync_flags() -> Result<()> {
    let json = r#"{"type":"resume_session","id":92,"session_id":"sess_resume"}"#;
    let decoded = parse_request_json(json)?;
    let Request::ResumeSession {
        id,
        session_id,
        client_instance_id,
        client_has_local_history,
        allow_session_takeover,
    } = decoded
    else {
        return Err(anyhow!("expected ResumeSession"));
    };
    assert_eq!(id, 92);
    assert_eq!(session_id, "sess_resume");
    assert_eq!(client_instance_id, None);
    assert!(!client_has_local_history);
    assert!(!allow_session_takeover);
    Ok(())
}

#[test]
fn test_message_request_roundtrip_preserves_images_and_system_reminder() -> Result<()> {
    let req = Request::Message {
        id: 88,
        content: "inspect this".to_string(),
        images: vec![
            ("image/png".to_string(), "AAA".to_string()),
            ("image/jpeg".to_string(), "BBB".to_string()),
        ],
        system_reminder: Some("be concise".to_string()),
    };
    let json = serde_json::to_string(&req)?;
    let decoded = parse_request_json(&json)?;
    let Request::Message {
        id,
        content,
        images,
        system_reminder,
    } = decoded
    else {
        return Err(anyhow!("expected Message"));
    };
    assert_eq!(id, 88);
    assert_eq!(content, "inspect this");
    assert_eq!(images.len(), 2);
    assert_eq!(images[0].0, "image/png");
    assert_eq!(images[1].0, "image/jpeg");
    assert_eq!(system_reminder.as_deref(), Some("be concise"));
    Ok(())
}
