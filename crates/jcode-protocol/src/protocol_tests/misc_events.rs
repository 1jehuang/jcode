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
        result: jcode_message_types::InputShellResult {
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
fn test_set_route_deserializes_as_set_model_compat_alias() -> Result<()> {
    // Legacy/desktop compatibility shape: a bare model string under the
    // `set_route` tag. `decode_request` (not raw serde) normalizes it.
    let decoded = decode_request(r#"{"type":"set_route","id":42,"model":"claude-opus-4-5"}"#)?;
    let Request::SetModel { id, model } = decoded else {
        return Err(anyhow!(
            "expected set_route compatibility alias to decode as SetModel"
        ));
    };
    assert_eq!(id, 42);
    assert_eq!(model, "claude-opus-4-5");
    Ok(())
}

#[test]
fn test_structured_set_route_decodes_as_set_route_not_set_model() -> Result<()> {
    // Regression for the "Invalid request: missing field `model`" bug seen when
    // switching models via the picker: a structured `set_route` request (with a
    // `selection` object, no `model` field) must decode as `Request::SetRoute`,
    // not be shadowed by the legacy `set_model` compatibility path.
    let request = Request::SetRoute {
        id: 7,
        selection: jcode_provider_core::RouteSelection {
            model: "gpt-5.5".to_string(),
            runtime_key: jcode_provider_core::RuntimeKey::OpenAIApiKey,
            api_method: "openai-api".to_string(),
            provider_label: "OpenAI".to_string(),
            detail: String::new(),
        },
    };
    let line = serde_json::to_string(&request)?;
    assert!(line.contains("\"type\":\"set_route\""));

    let decoded = decode_request(&line)?;
    let Request::SetRoute { id, selection } = decoded else {
        return Err(anyhow!(
            "expected structured set_route to decode as SetRoute, got {decoded:?}"
        ));
    };
    assert_eq!(id, 7);
    assert_eq!(selection.model, "gpt-5.5");
    Ok(())
}

#[test]
fn test_jcode_subscription_set_route_is_wire_safe() -> Result<()> {
    let request = Request::SetRoute {
        id: 8,
        selection: jcode_provider_core::RouteSelection {
            model: "gpt-5.5".to_string(),
            runtime_key: jcode_provider_core::RuntimeKey::JcodeSubscription,
            api_method: "jcode-subscription".to_string(),
            provider_label: "Jcode Subscription".to_string(),
            detail: "jcode subscription routing · managed server-side".to_string(),
        },
    };

    let line = serde_json::to_string(&request)?;
    assert!(line.contains("\"type\":\"set_route\""));
    assert!(line.contains("\"kind\":\"jcode-subscription\""));

    let decoded = decode_request(&line)?;
    let Request::SetRoute { id, selection } = decoded else {
        return Err(anyhow!("expected Jcode SetRoute, got {decoded:?}"));
    };
    assert_eq!(id, 8);
    assert_eq!(selection.model, "gpt-5.5");
    assert_eq!(
        selection.runtime_key,
        jcode_provider_core::RuntimeKey::JcodeSubscription
    );
    assert_eq!(selection.routed_model_spec(), "gpt-5.5");
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
        terminal_env: vec![("ZELLIJ_SESSION_NAME".to_string(), "sessionB".to_string())],
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
        terminal_env,
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
    assert_eq!(
        terminal_env,
        vec![("ZELLIJ_SESSION_NAME".to_string(), "sessionB".to_string())]
    );
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
        terminal_env,
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
    assert!(terminal_env.is_empty());
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
fn test_resume_all_sessions_request_roundtrip() -> Result<()> {
    let req = Request::ResumeAllSessions { id: 451 };
    let json = serde_json::to_string(&req)?;
    assert!(json.contains("\"type\":\"resume_all_sessions\""));
    let decoded = parse_request_json(&json)?;
    let Request::ResumeAllSessions { id } = decoded else {
        return Err(anyhow!("expected ResumeAllSessions"));
    };
    assert_eq!(id, 451);
    Ok(())
}

#[test]
fn test_resume_all_result_event_roundtrip() -> Result<()> {
    let event = ServerEvent::ResumeAllResult {
        id: 451,
        resumed: 2,
        skipped: 1,
        resumed_sessions: vec!["fox".to_string(), "owl".to_string()],
        message: "Resuming 2 interrupted sessions: fox, owl.".to_string(),
    };
    let json = serde_json::to_string(&event)?;
    assert!(json.contains("\"type\":\"resume_all_result\""));
    let decoded = parse_event_json(&json)?;
    let ServerEvent::ResumeAllResult {
        id,
        resumed,
        skipped,
        resumed_sessions,
        message,
    } = decoded
    else {
        return Err(anyhow!("expected ResumeAllResult"));
    };
    assert_eq!(id, 451);
    assert_eq!(resumed, 2);
    assert_eq!(skipped, 1);
    assert_eq!(resumed_sessions, vec!["fox".to_string(), "owl".to_string()]);
    assert_eq!(message, "Resuming 2 interrupted sessions: fox, owl.");
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

#[test]
fn test_provider_guardrail_event_roundtrip() -> Result<()> {
    let event = ServerEvent::ProviderGuardrail {
        stop_reason: Some("refusal".to_string()),
        message: "Provider guardrail stopped the response".to_string(),
    };
    let json = encode_event(&event);
    assert!(json.contains("\"type\":\"provider_guardrail\""));
    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::ProviderGuardrail {
        stop_reason,
        message,
    } = decoded
    else {
        return Err(anyhow!("expected ProviderGuardrail event"));
    };
    assert_eq!(stop_reason.as_deref(), Some("refusal"));
    assert_eq!(message, "Provider guardrail stopped the response");

    // stop_reason is optional on the wire.
    let decoded = parse_event_json(
        r#"{"type":"provider_guardrail","message":"blocked"}"#,
    )?;
    let ServerEvent::ProviderGuardrail { stop_reason, .. } = decoded else {
        return Err(anyhow!("expected ProviderGuardrail event"));
    };
    assert!(stop_reason.is_none());
    Ok(())
}
