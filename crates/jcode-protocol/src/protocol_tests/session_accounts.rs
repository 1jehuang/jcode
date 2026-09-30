#[test]
fn test_session_account_requests_roundtrip() -> Result<()> {
    let requests = [
        Request::SetSessionAccount {
            id: 5,
            provider: "claude".to_string(),
            label: Some("claude-fox".to_string()),
        },
        Request::SetSessionAccount {
            id: 6,
            provider: "openai".to_string(),
            label: None,
        },
        Request::SetDefaultAccount {
            id: 7,
            provider: "openai".to_string(),
            label: "openai-otter".to_string(),
        },
        Request::SetAccountFailover {
            id: 8,
            enabled: Some(false),
        },
        Request::SetAccountFailover {
            id: 9,
            enabled: None,
        },
    ];
    for req in requests {
        let json = serde_json::to_string(&req)?;
        let decoded = parse_request_json(&json)?;
        assert_eq!(decoded.id(), req.id());
        assert_eq!(serde_json::to_string(&decoded)?, json);
    }

    let json = r#"{"type":"set_session_account","id":11,"provider":"claude"}"#;
    let Request::SetSessionAccount { id, provider, label } = parse_request_json(json)? else {
        return Err(anyhow!("expected SetSessionAccount"));
    };
    assert_eq!((id, provider.as_str(), label), (11, "claude", None));
    Ok(())
}

#[test]
fn test_subscribe_account_fields_roundtrip_and_default() -> Result<()> {
    let json = r#"{"type":"subscribe","id":3,"account_pins":[["claude","claude-fox"]],"supports_session_accounts":true}"#;
    let Request::Subscribe {
        account_pins,
        supports_session_accounts,
        ..
    } = parse_request_json(json)?
    else {
        return Err(anyhow!("expected Subscribe"));
    };
    assert_eq!(
        account_pins,
        vec![("claude".to_string(), "claude-fox".to_string())]
    );
    assert!(supports_session_accounts);

    // Old clients omit both fields.
    let Request::Subscribe {
        account_pins,
        supports_session_accounts,
        ..
    } = parse_request_json(r#"{"type":"subscribe","id":4}"#)?
    else {
        return Err(anyhow!("expected Subscribe"));
    };
    assert!(account_pins.is_empty());
    assert!(!supports_session_accounts);
    Ok(())
}

#[test]
fn test_session_account_changed_event_roundtrip() -> Result<()> {
    let event = ServerEvent::SessionAccountChanged {
        provider: "claude".to_string(),
        label: Some("claude-fox".to_string()),
        pinned: true,
        is_default: false,
        reason: Some("claude-otter is out of usage".to_string()),
    };
    let json = serde_json::to_string(&event)?;
    assert!(json.contains("\"type\":\"session_account_changed\""));
    let ServerEvent::SessionAccountChanged {
        provider,
        label,
        pinned,
        is_default,
        reason,
    } = parse_event_json(&json)?
    else {
        return Err(anyhow!("expected SessionAccountChanged"));
    };
    assert_eq!(provider, "claude");
    assert_eq!(label.as_deref(), Some("claude-fox"));
    assert!(pinned);
    assert!(!is_default);
    assert_eq!(reason.as_deref(), Some("claude-otter is out of usage"));
    Ok(())
}

#[test]
fn test_credentials_changed_account_label_roundtrip_and_old_json() -> Result<()> {
    let event = ServerEvent::CredentialsChanged {
        provider: Some("openai".to_string()),
        account_label: Some("openai-fox".to_string()),
    };
    let json = serde_json::to_string(&event)?;
    let ServerEvent::CredentialsChanged {
        provider,
        account_label,
    } = parse_event_json(&json)?
    else {
        return Err(anyhow!("expected CredentialsChanged"));
    };
    assert_eq!(provider.as_deref(), Some("openai"));
    assert_eq!(account_label.as_deref(), Some("openai-fox"));

    let ServerEvent::CredentialsChanged { account_label, .. } =
        parse_event_json(r#"{"type":"credentials_changed","provider":"claude"}"#)?
    else {
        return Err(anyhow!("expected CredentialsChanged"));
    };
    assert_eq!(account_label, None);
    Ok(())
}

#[test]
fn test_history_account_labels_roundtrip_and_old_json() -> Result<()> {
    let info = SessionAccountInfo {
        provider: "claude".to_string(),
        label: Some("claude-fox".to_string()),
        pinned: true,
        is_default: false,
    };
    let json = serde_json::to_string(&info)?;
    assert_eq!(serde_json::from_str::<SessionAccountInfo>(&json)?, info);

    let history = r#"{"type":"history","id":1,"session_id":"s","messages":[],"account_labels":[{"provider":"openai","label":"openai-otter","is_default":true}]}"#;
    let ServerEvent::History { account_labels, .. } = parse_event_json(history)? else {
        return Err(anyhow!("expected History"));
    };
    assert_eq!(
        account_labels,
        vec![SessionAccountInfo {
            provider: "openai".to_string(),
            label: Some("openai-otter".to_string()),
            pinned: false,
            is_default: true,
        }]
    );

    let old = r#"{"type":"history","id":1,"session_id":"s","messages":[]}"#;
    let ServerEvent::History { account_labels, .. } = parse_event_json(old)? else {
        return Err(anyhow!("expected History"));
    };
    assert!(account_labels.is_empty());
    Ok(())
}
