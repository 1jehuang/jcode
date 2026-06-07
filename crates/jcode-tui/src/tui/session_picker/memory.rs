use super::{
    PickerItem, PreviewMessage, ResumeTarget, ServerGroup, SessionInfo, SessionPicker, SessionRef,
};

pub(super) fn debug_memory_profile(picker: &SessionPicker) -> serde_json::Value {
    let items_estimate_bytes: usize = picker.items.iter().map(estimate_picker_item_bytes).sum();
    let visible_sessions_estimate_bytes =
        picker.visible_sessions.capacity() * std::mem::size_of::<SessionRef>();
    let all_sessions_estimate_bytes: usize = picker
        .all_sessions
        .iter()
        .map(estimate_session_info_bytes)
        .sum();
    let all_server_groups_estimate_bytes: usize = picker
        .all_server_groups
        .iter()
        .map(estimate_server_group_bytes)
        .sum();
    let all_orphan_sessions_estimate_bytes: usize = picker
        .all_orphan_sessions
        .iter()
        .map(estimate_session_info_bytes)
        .sum();
    let item_to_session_estimate_bytes =
        picker.item_to_session.capacity() * std::mem::size_of::<Option<usize>>();
    let crashed_session_ids_estimate_bytes: usize = picker
        .crashed_session_ids
        .iter()
        .map(|value| value.capacity())
        .sum();
    let selected_session_ids_estimate_bytes: usize = picker
        .selected_session_ids
        .iter()
        .map(|value| value.capacity())
        .sum();
    let search_query_bytes = picker.search_query.capacity();
    let loading_message_bytes = picker
        .loading_message
        .as_ref()
        .map(|message| message.capacity())
        .unwrap_or(0);
    let total_estimate_bytes = items_estimate_bytes
        + visible_sessions_estimate_bytes
        + all_sessions_estimate_bytes
        + all_server_groups_estimate_bytes
        + all_orphan_sessions_estimate_bytes
        + item_to_session_estimate_bytes
        + crashed_session_ids_estimate_bytes
        + selected_session_ids_estimate_bytes
        + search_query_bytes
        + loading_message_bytes;

    serde_json::json!({
        "items_count": picker.items.len(),
        "visible_sessions_count": picker.visible_sessions.len(),
        "all_sessions_count": picker.all_sessions.len(),
        "all_server_groups_count": picker.all_server_groups.len(),
        "all_orphan_sessions_count": picker.all_orphan_sessions.len(),
        "crashed_session_ids_count": picker.crashed_session_ids.len(),
        "selected_session_ids_count": picker.selected_session_ids.len(),
        "search_query_bytes": search_query_bytes,
        "loading_message_bytes": loading_message_bytes,
        "items_estimate_bytes": items_estimate_bytes,
        "visible_sessions_estimate_bytes": visible_sessions_estimate_bytes,
        "all_sessions_estimate_bytes": all_sessions_estimate_bytes,
        "all_server_groups_estimate_bytes": all_server_groups_estimate_bytes,
        "all_orphan_sessions_estimate_bytes": all_orphan_sessions_estimate_bytes,
        "item_to_session_estimate_bytes": item_to_session_estimate_bytes,
        "crashed_session_ids_estimate_bytes": crashed_session_ids_estimate_bytes,
        "selected_session_ids_estimate_bytes": selected_session_ids_estimate_bytes,
        "total_estimate_bytes": total_estimate_bytes,
    })
}

fn estimate_optional_string_bytes(value: &Option<String>) -> usize {
    value.as_ref().map(|value| value.capacity()).unwrap_or(0)
}

fn estimate_preview_message_bytes(message: &PreviewMessage) -> usize {
    message.role.capacity() + message.content.capacity()
}

fn estimate_resume_target_bytes(value: &ResumeTarget) -> usize {
    match value {
        ResumeTarget::JcodeSession { session_id } => session_id.capacity(),
        ResumeTarget::ClaudeCodeSession {
            session_id,
            session_path,
        }
        | ResumeTarget::CodexSession {
            session_id,
            session_path,
        }
        | ResumeTarget::OpenCodeSession {
            session_id,
            session_path,
        } => session_id.capacity() + session_path.capacity(),
        ResumeTarget::PiSession { session_path } => session_path.capacity(),
    }
}

fn estimate_session_info_bytes(info: &SessionInfo) -> usize {
    info.id.capacity()
        + estimate_optional_string_bytes(&info.parent_id)
        + info.short_name.capacity()
        + info.icon.capacity()
        + info.title.capacity()
        + estimate_optional_string_bytes(&info.working_dir)
        + estimate_optional_string_bytes(&info.model)
        + estimate_optional_string_bytes(&info.provider_key)
        + estimate_optional_string_bytes(&info.save_label)
        + info
            .messages_preview
            .iter()
            .map(estimate_preview_message_bytes)
            .sum::<usize>()
        + info.search_index.capacity()
        + estimate_optional_string_bytes(&info.server_name)
        + estimate_optional_string_bytes(&info.server_icon)
        + estimate_resume_target_bytes(&info.resume_target)
        + estimate_optional_string_bytes(&info.external_path)
}

fn estimate_server_group_bytes(group: &ServerGroup) -> usize {
    group.name.capacity()
        + group.icon.capacity()
        + group.version.capacity()
        + group.git_hash.capacity()
        + group
            .sessions
            .iter()
            .map(estimate_session_info_bytes)
            .sum::<usize>()
}

fn estimate_picker_item_bytes(item: &PickerItem) -> usize {
    match item {
        PickerItem::ServerHeader {
            name,
            icon,
            version,
            ..
        } => name.capacity() + icon.capacity() + version.capacity(),
        PickerItem::Session | PickerItem::OrphanHeader { .. } | PickerItem::SavedHeader { .. } => 0,
    }
}
