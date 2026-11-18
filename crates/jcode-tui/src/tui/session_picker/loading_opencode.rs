//! OpenCode session metadata loading.

use super::*;

pub(super) fn load_opencode_session_info(
    path: &Path,
) -> Result<Option<(SessionInfo, SourceBuckets)>> {
    let value: serde_json::Value = serde_json::from_reader(File::open(path)?)?;
    let session_id = value
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if session_id.is_empty() {
        return Ok(None);
    }

    let created_at = value
        .get("time")
        .and_then(|time| time.get("created"))
        .and_then(|v| v.as_i64())
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .unwrap_or_else(chrono::Utc::now);
    let last_message_time = value
        .get("time")
        .and_then(|time| time.get("updated"))
        .and_then(|v| v.as_i64())
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .unwrap_or(created_at);
    let working_dir = value
        .get("directory")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let title = value
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| truncate_title_text(s, 72))
        .unwrap_or_else(|| {
            format!(
                "OpenCode session {}",
                jcode_core::util::truncate_str(&session_id, 8)
            )
        });

    let messages_root = crate::storage::user_home_path(format!(
        ".local/share/opencode/storage/message/{}",
        session_id
    ))?;
    let parts_base = crate::storage::user_home_path(".local/share/opencode/storage/part")?;
    let mut preview = Vec::new();
    let mut search = SourceBuckets::default();
    let mut user_message_count = 0usize;
    let mut assistant_message_count = 0usize;
    let mut provider_key: Option<String> = Some("opencode".to_string());
    let mut model: Option<String> = None;

    if messages_root.exists() {
        for msg_path in collect_files_recursive(&messages_root, "json") {
            let Ok(msg_value) =
                serde_json::from_reader::<_, serde_json::Value>(File::open(&msg_path)?)
            else {
                continue;
            };
            let role = msg_value
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            // Modern OpenCode (Go storage) stores body text in part files keyed
            // by message id; fall back to the legacy inline summary.
            if let Some(id) = msg_value.get("id").and_then(|v| v.as_str()) {
                classify_opencode_parts(&mut search, &parts_base, id, message_role_source(role));
            }
            let text = msg_value
                .get("id")
                .and_then(|v| v.as_str())
                .map(|id| extract_opencode_part_preview(&parts_base, id))
                .filter(|text| !text.trim().is_empty())
                .or_else(|| msg_value.get("summary").map(extract_text_from_value))
                .unwrap_or_default();
            match role {
                "user" => user_message_count += 1,
                "assistant" => assistant_message_count += 1,
                _ => {}
            }
            if model.is_none() {
                model = msg_value
                    .get("modelID")
                    .or_else(|| msg_value.get("model").and_then(|m| m.get("modelID")))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }
            if provider_key.is_none() {
                provider_key = msg_value
                    .get("providerID")
                    .or_else(|| msg_value.get("model").and_then(|m| m.get("providerID")))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }
            push_preview_message(&mut preview, role, text);
        }
    }

    let message_count = user_message_count + assistant_message_count;
    if message_count == 0 {
        return Ok(None);
    }

    let short_name = format!(
        "opencode {}",
        jcode_core::util::truncate_str(&session_id, 8)
    );
    if search.is_empty() {
        for msg in &preview {
            search.push(
                message_role_source(&msg.role),
                &msg.content,
                SEARCH_CONTENT_BUDGET_BYTES,
            );
        }
    }
    let search_index = build_classified_search_index(
        &format!("opencode:{session_id}"),
        &short_name,
        &title,
        working_dir.as_deref(),
        None,
        &search,
    );
    let first_user_prompt = preview
        .iter()
        .find(|msg| msg.role == "user" && !msg.content.trim().is_empty())
        .map(|msg| msg.content.clone());

    let session = SessionInfo {
        id: format!("opencode:{session_id}"),
        parent_id: None,
        short_name,
        icon: "◌".to_string(),
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
        source: SessionSource::OpenCode,
        resume_target: ResumeTarget::OpenCodeSession {
            session_id,
            session_path: path.to_string_lossy().to_string(),
        },
        external_path: Some(path.to_string_lossy().to_string()),
        todo_statuses: Vec::new(),
    };
    Ok(Some((session, search)))
}
