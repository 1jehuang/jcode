use super::*;

pub(in crate::tui::session_picker) fn load_pi_preview_from_path(
    path: &Path,
) -> Option<Vec<PreviewMessage>> {
    load_pi_session_info(path)
        .ok()
        .flatten()
        .map(|session| session.messages_preview)
}

pub(super) fn load_external_pi_sessions(scan_limit: usize) -> Vec<SessionInfo> {
    let Ok(root) = crate::storage::user_home_path(".pi/agent/sessions") else {
        return Vec::new();
    };
    if !root.exists() {
        return Vec::new();
    }

    let paths = collect_recent_files_recursive(&root, "jsonl", scan_limit);
    parallel_map(paths, |path| load_pi_session_stub(&path).ok().flatten())
        .into_iter()
        .flatten()
        .collect()
}

pub(super) fn load_pi_session_stub(path: &Path) -> Result<Option<SessionInfo>> {
    let file = File::open(path)?;
    let mut lines = BufReader::new(file).lines();
    let Some(first_line) = lines.next() else {
        return Ok(None);
    };
    let header: serde_json::Value = serde_json::from_str(&first_line?)?;
    if header.get("type").and_then(|v| v.as_str()) != Some("session") {
        return Ok(None);
    }

    let session_id = header
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if session_id.is_empty() {
        return Ok(None);
    }

    let created_at = header
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);
    let last_message_time = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .map(chrono::DateTime::<chrono::Utc>::from)
        .unwrap_or(created_at);
    let working_dir = header
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let short_name = format!("pi {}", jcode_core::util::truncate_str(&session_id, 8));
    let title = format!(
        "Pi session {}",
        jcode_core::util::truncate_str(&session_id, 8)
    );
    let search_index = build_search_index(
        &format!("pi:{session_id}"),
        &short_name,
        &title,
        working_dir.as_deref(),
        None,
        &[],
    );

    Ok(Some(SessionInfo {
        id: format!("pi:{session_id}"),
        parent_id: None,
        short_name,
        icon: "π".to_string(),
        title,
        message_count: 0,
        user_message_count: 0,
        assistant_message_count: 0,
        created_at,
        last_message_time,
        last_active_at: Some(last_message_time),
        working_dir,
        model: None,
        provider_key: Some("pi".to_string()),
        is_canary: false,
        is_debug: false,
        saved: false,
        save_label: None,
        status: SessionStatus::Closed,
        needs_catchup: false,
        estimated_tokens: 0,
        first_user_prompt: None,
        messages_preview: Vec::new(),
        search_index,
        server_name: None,
        server_icon: None,
        source: SessionSource::Pi,
        resume_target: ResumeTarget::PiSession {
            session_path: path.to_string_lossy().to_string(),
        },
        external_path: Some(path.to_string_lossy().to_string()),
        todo_statuses: Vec::new(),
    }))
}

pub(super) fn load_pi_session_info(path: &Path) -> Result<Option<SessionInfo>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();
    let Some(first_line) = lines.next() else {
        return Ok(None);
    };
    let header: serde_json::Value = serde_json::from_str(&first_line?)?;
    if header.get("type").and_then(|v| v.as_str()) != Some("session") {
        return Ok(None);
    }

    let session_id = header
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if session_id.is_empty() {
        return Ok(None);
    }

    let created_at = header
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);
    let working_dir = header
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let mut title: Option<String> = None;
    let mut model: Option<String> = None;
    let mut provider_key: Option<String> = Some("pi".to_string());
    let mut last_message_time = created_at;
    let mut user_message_count = 0usize;
    let mut assistant_message_count = 0usize;
    let mut message_count = 0usize;
    let mut preview = Vec::new();

    for line in lines {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };

        if let Some(ts) = value
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc))
        {
            last_message_time = ts;
        }

        match value.get("type").and_then(|v| v.as_str()) {
            Some("model_change") => {
                provider_key = value
                    .get("provider")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or(provider_key);
                model = value
                    .get("modelId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or(model);
            }
            Some("message") => {
                let Some(message) = value.get("message") else {
                    continue;
                };
                let role = message
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let text = extract_text_from_value(
                    message.get("content").unwrap_or(&serde_json::Value::Null),
                );
                if title.is_none() && role == "user" && !text.trim().is_empty() {
                    title = Some(truncate_title_text(&text, 72));
                }
                if model.is_none() {
                    model = message
                        .get("model")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
                message_count += 1;
                match role {
                    "user" => user_message_count += 1,
                    "assistant" => assistant_message_count += 1,
                    _ => {}
                }
                push_preview_message(&mut preview, role, text);
            }
            _ => {}
        }
    }

    if message_count == 0 {
        return Ok(None);
    }

    let short_name = format!("pi {}", jcode_core::util::truncate_str(&session_id, 8));
    let title = title.unwrap_or_else(|| {
        format!(
            "Pi session {}",
            jcode_core::util::truncate_str(&session_id, 8)
        )
    });
    let search_index = build_search_index(
        &format!("pi:{session_id}"),
        &short_name,
        &title,
        working_dir.as_deref(),
        None,
        &preview,
    );
    let first_user_prompt = preview
        .iter()
        .find(|msg| msg.role == "user" && !msg.content.trim().is_empty())
        .map(|msg| msg.content.clone());

    Ok(Some(SessionInfo {
        id: format!("pi:{session_id}"),
        parent_id: None,
        short_name,
        icon: "π".to_string(),
        title,
        message_count,
        user_message_count,
        assistant_message_count,
        created_at,
        last_message_time,
        last_active_at: Some(last_message_time),
        working_dir,
        model,
        provider_key,
        is_canary: false,
        is_debug: false,
        saved: false,
        save_label: None,
        status: SessionStatus::Closed,
        needs_catchup: false,
        estimated_tokens: 0,
        first_user_prompt,
        messages_preview: preview,
        search_index,
        server_name: None,
        server_icon: None,
        source: SessionSource::Pi,
        resume_target: ResumeTarget::PiSession {
            session_path: path.to_string_lossy().to_string(),
        },
        external_path: Some(path.to_string_lossy().to_string()),
        todo_statuses: Vec::new(),
    }))
}
