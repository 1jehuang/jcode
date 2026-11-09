use super::*;

pub(in crate::server) async fn handle_get_compacted_history(
    id: u64,
    session_id: &str,
    agent: &Arc<Mutex<Agent>>,
    writer: &Arc<Mutex<WriteHalf>>,
    visible_messages: usize,
    recent_history: bool,
) -> Result<()> {
    let mut timing = Timing::new(
        if recent_history {
            "recent_history_page"
        } else {
            "compacted_history_page"
        },
        Some(id),
        session_id,
    );
    let started = Instant::now();
    let (messages, images, compacted_info, source) = match agent.try_lock() {
        Ok(agent_guard) => {
            let (messages, images, info) = if recent_history {
                let (messages, images) = agent_guard.get_history_and_rendered_images();
                let (messages, images, info) =
                    recent_history_window(messages, images, visible_messages);
                (messages, images, Some(info))
            } else {
                agent_guard.get_history_and_rendered_images_with_compacted_history(visible_messages)
            };
            (messages, images, info, "live")
        }
        Err(_) => {
            timing.stage("persisted_load");
            let loaded = crate::session::Session::load_for_remote_startup(session_id)
                .or_else(|_| crate::session::Session::load_startup_stub(session_id));
            if loaded.is_err() {
                timing.result(&loaded);
            }
            let session = loaded?;
            timing.stage("render");
            let (rendered_messages, images, info) = if recent_history {
                let (messages, images) = crate::session::render_messages_and_images(&session);
                (messages, images, None)
            } else {
                crate::session::render_messages_and_images_with_compacted_history(
                    &session,
                    visible_messages,
                )
            };
            let messages = rendered_messages
                .into_iter()
                .map(rendered_to_history_message)
                .collect();
            let (messages, images, info) = if recent_history {
                let (messages, images, info) =
                    recent_history_window(messages, images, visible_messages);
                (messages, images, Some(info))
            } else {
                (messages, images, info)
            };
            (messages, images, info, "persisted")
        }
    };

    let compacted_info = compacted_info.unwrap_or(crate::session::RenderedCompactedHistoryInfo {
        total_messages: 0,
        visible_messages: 0,
        remaining_messages: 0,
        hidden_user_prompts: 0,
    });
    crate::logging::info(&format!(
        "[TIMING] get_compacted_history: session={}, source={}, requested={}, visible={}, remaining={}, messages={}, total={}ms",
        session_id,
        source,
        visible_messages,
        compacted_info.visible_messages,
        compacted_info.remaining_messages,
        messages.len(),
        started.elapsed().as_millis(),
    ));

    timing.stage("encode");
    let event = ServerEvent::CompactedHistory {
        id,
        session_id: session_id.to_string(),
        messages,
        images,
        compacted_total: compacted_info.total_messages,
        compacted_visible: compacted_info.visible_messages,
        compacted_remaining: compacted_info.remaining_messages,
        compacted_hidden_prompts: compacted_info.hidden_user_prompts,
    };
    let json = encode_event(&event);
    timing.stage("writer_lock");
    let mut writer = writer.lock().await;
    timing.stage("write");
    let result = writer.write_all(json.as_bytes()).await;
    timing.result(&result);
    result.map_err(Into::into)
}
