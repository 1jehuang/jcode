fn delta_test_messages() -> Vec<crate::protocol::HistoryMessage> {
    ["one", "two", "three", "four"]
        .into_iter()
        .enumerate()
        .map(|(index, content)| crate::protocol::HistoryMessage {
            response_stats: None,
            role: if index % 2 == 0 { "user" } else { "assistant" }.to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_data: None,
        })
        .collect()
}

fn delta_test_base(
    messages: &[crate::protocol::HistoryMessage],
    count: usize,
) -> crate::protocol::HistoryBase {
    crate::protocol::HistoryBase {
        count,
        fingerprint: crate::protocol::history_prefix_fingerprint(
            messages[..count]
                .iter()
                .map(crate::protocol::HistoryMessageRef::from),
        ),
        images_fingerprint: None,
        recent_messages: None,
    }
}

#[test]
fn history_base_match_sends_only_the_tail() {
    let messages = delta_test_messages();
    let base = delta_test_base(&messages, 3);
    let (tail, base_count) = super::super::apply_history_base(messages, Some(base));
    assert_eq!(base_count, Some(3));
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].content, "four");
}

#[test]
fn history_base_full_match_sends_empty_tail() {
    let messages = delta_test_messages();
    let base = delta_test_base(&messages, 4);
    let (tail, base_count) = super::super::apply_history_base(messages, Some(base));
    assert_eq!(base_count, Some(4));
    assert!(tail.is_empty());
}

#[test]
fn history_base_mismatch_or_overlong_sends_full_transcript() {
    let messages = delta_test_messages();
    let mut edited = delta_test_messages();
    edited[1].content = "rewound".to_string();
    let stale = delta_test_base(&edited, 3);
    let (full, base_count) = super::super::apply_history_base(messages.clone(), Some(stale));
    assert_eq!((full.len(), base_count), (4, None));

    let mut longer = delta_test_messages();
    longer.push(longer[0].clone());
    let overlong = delta_test_base(&longer, 5);
    let (full, base_count) = super::super::apply_history_base(messages.clone(), Some(overlong));
    assert_eq!((full.len(), base_count), (4, None));

    let (full, base_count) = super::super::apply_history_base(messages, None);
    assert_eq!((full.len(), base_count), (4, None));
}

#[test]
fn history_images_base_omits_only_matching_image_sets() {
    let image = |data: &str| crate::session::RenderedImage {
        media_type: "image/png".to_string(),
        data: data.to_string(),
        label: None,
        source: crate::session::RenderedImageSource::UserInput,
        anchor: None,
        history_message_index: None,
    };
    let images = vec![image("a"), image("b")];
    let base = |fingerprint| crate::protocol::HistoryBase {
        count: 1,
        fingerprint: 0,
        images_fingerprint: Some(fingerprint),
        recent_messages: None,
    };
    let matching = crate::protocol::history_images_fingerprint(&images);
    let (sent, unchanged) =
        super::super::apply_history_images_base(images.clone(), Some(base(matching)));
    assert!(unchanged && sent.is_empty());
    let other = crate::protocol::history_images_fingerprint(&[image("a")]);
    let (sent, unchanged) =
        super::super::apply_history_images_base(images.clone(), Some(base(other)));
    assert!(!unchanged && sent.len() == 2);
    let (sent, unchanged) = super::super::apply_history_images_base(images, None);
    assert!(!unchanged && sent.len() == 2);
}

#[test]
fn recent_history_bounds_seven_thousand_message_bootstrap() {
    let messages: Vec<_> = (0..7000)
        .map(|index| crate::protocol::HistoryMessage {
            response_stats: None,
            role: if index % 2 == 0 { "user" } else { "assistant" }.to_owned(),
            content: format!("{index}: {}", "x".repeat(2000)),
            tool_calls: None,
            tool_data: None,
        })
        .collect();
    let full_bytes = serde_json::to_vec(&messages).unwrap().len();
    let (window, images) = super::super::apply_recent_history_base(
        messages,
        Vec::new(),
        Some(crate::protocol::HistoryBase::recent(128)),
        None,
    );
    let window_bytes = serde_json::to_vec(&window).unwrap().len();
    assert!(full_bytes > 14_000_000);
    assert!(window_bytes < 512 * 1024);
    assert_eq!(window.len(), 129);
    assert!(images.is_empty());
    assert_eq!(
        window[0].content,
        "Earlier remote history - 6872 older historical messages hidden. Showing 128 of 7000 messages. Scroll to the top to load more. Hidden user prompts: 3436."
    );
    assert!(window[1].content.starts_with("6872:"));
    eprintln!("recent history bootstrap: {full_bytes} -> {window_bytes} bytes");
}

#[test]
fn recent_history_preserves_archive_prompt_offset_and_rebases_images() {
    let mut messages = delta_test_messages();
    messages.insert(0, crate::protocol::HistoryMessage {
        response_stats: None, role: "system".to_owned(),
        content: "Earlier conversation compacted - 10 historical messages hidden from the UI. Scroll to the top to load older history. Hidden user prompts: 7.".to_owned(),
        tool_calls: None, tool_data: None,
    });
    let image = |ordinal| crate::session::RenderedImage {
        media_type: "image/png".to_owned(),
        data: "data".to_owned(),
        label: None,
        source: crate::session::RenderedImageSource::UserInput,
        anchor: Some(crate::session::RenderedImageAnchor::UserPrompt { ordinal }),
        history_message_index: None,
    };
    let (window, images, info) =
        super::super::recent_history_window(messages.clone(), vec![image(0), image(1)], 2);
    assert_eq!(
        (
            info.total_messages,
            info.visible_messages,
            info.remaining_messages,
            info.hidden_user_prompts
        ),
        (5, 2, 3, 8)
    );
    assert_eq!(window.len(), 3);
    assert_eq!(images.len(), 1);
    assert_eq!(
        images[0].anchor,
        Some(crate::session::RenderedImageAnchor::UserPrompt { ordinal: 0 })
    );
    let (full, _, info) = super::super::recent_history_window(messages.clone(), Vec::new(), 100);
    assert_eq!(info.remaining_messages, 0);
    assert_eq!(
        serde_json::to_value(&full).unwrap(),
        serde_json::to_value(&messages).unwrap()
    );
    assert!(
        full[0]
            .content
            .starts_with("Earlier conversation compacted - ")
    );
}

#[test]
fn recent_history_never_mixes_with_validated_or_stale_prefix() {
    let messages = delta_test_messages();
    let mut base = delta_test_base(&messages, 2);
    base.recent_messages = Some(1);
    let (tail, count) = super::super::apply_history_base(messages.clone(), Some(base));
    let (tail, _) = super::super::apply_recent_history_base(tail, Vec::new(), Some(base), count);
    assert_eq!(tail.len(), 2);
    base.fingerprint = 0;
    let (full, count) = super::super::apply_history_base(messages.clone(), Some(base));
    let (full, _) = super::super::apply_recent_history_base(full, Vec::new(), Some(base), count);
    assert_eq!(
        serde_json::to_value(&full).unwrap(),
        serde_json::to_value(&messages).unwrap()
    );
    let (full, _) =
        super::super::apply_recent_history_base(messages.clone(), Vec::new(), None, None);
    assert_eq!(
        serde_json::to_value(&full).unwrap(),
        serde_json::to_value(&messages).unwrap()
    );
}

#[test]
fn recent_history_never_truncates_one_large_message() {
    let mut messages = delta_test_messages();
    messages.last_mut().unwrap().content = "x".repeat(1024 * 1024);
    let expected = messages.last().unwrap().content.clone();
    let (window, _, _) = super::super::recent_history_window(messages, Vec::new(), 1);
    assert_eq!(window[1].content, expected);
}

#[test]
fn recent_history_scopes_tool_images_and_rebases_append_boundaries() {
    let mut messages = delta_test_messages();
    messages[3].role = "tool".to_owned();
    messages[3].tool_data = Some(crate::message::ToolCall {
        id: "visible-tool".to_owned(),
        name: "read".to_owned(),
        ..Default::default()
    });
    let image = |id: &str, index| crate::session::RenderedImage {
        media_type: "image/png".to_owned(),
        data: id.to_owned(),
        label: None,
        source: crate::session::RenderedImageSource::ToolResult {
            tool_name: "read".to_owned(),
        },
        anchor: Some(crate::session::RenderedImageAnchor::ToolCall { id: id.to_owned() }),
        history_message_index: Some(index),
    };
    let (window, images, _) = super::super::recent_history_window(
        messages,
        vec![image("hidden-tool", 1), image("visible-tool", 4)],
        2,
    );
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].history_message_index, Some(window.len()));
    assert_eq!(images[0].data, "visible-tool");
}

#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "serialize storage and exercise busy-agent fallback"
)]
async fn recent_history_live_and_busy_bootstrap_and_expansion() {
    use super::*;
    let _env = crate::storage::lock_test_env();
    let home = tempfile::TempDir::new().unwrap();
    let previous = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());
    let session_id = "session_recent_window_paths";
    let mut session = crate::session::Session::create_with_id(session_id.to_owned(), None, None);
    for index in 0..8 {
        session.append_stored_message(crate::session::StoredMessage {
            id: format!("message-{index}"),
            role: if index % 2 == 0 {
                crate::message::Role::User
            } else {
                crate::message::Role::Assistant
            },
            content: vec![crate::message::ContentBlock::Text {
                text: format!("message {index}"),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        });
    }
    let provider: Arc<dyn Provider> = Arc::new(MockProvider(None));
    let agent = Arc::new(Mutex::new(Agent::new_with_session(
        provider.clone(),
        Registry::empty(),
        session,
        None,
    )));
    let sessions = Arc::new(crate::server::session_manager::SessionManager::from_agents(
        HashMap::from([(session_id.to_owned(), agent.clone())]),
    ));
    let connections = Arc::new(RwLock::new(HashMap::new()));
    let count = Arc::new(RwLock::new(1));
    for busy in [false, true] {
        let guard = if busy { Some(agent.lock().await) } else { None };
        for visible in [Some(2), None] {
            let (stream, peer) = crate::transport::stream_pair().unwrap();
            let (_, write) = stream.into_split();
            let writer = Arc::new(Mutex::new(write));
            tokio::time::timeout(
                Duration::from_secs(2),
                super::super::handle_get_history(
                    1,
                    session_id,
                    busy,
                    &agent,
                    &provider,
                    &sessions,
                    &connections,
                    &count,
                    &writer,
                    "test",
                    "",
                    None,
                    false,
                    false,
                    visible.map(crate::protocol::HistoryBase::recent),
                ),
            )
            .await
            .expect("history must not wait for agent")
            .unwrap();
            let mut reader = tokio::io::BufReader::new(peer);
            let mut line = String::new();
            tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut line)
                .await
                .unwrap();
            let event: crate::protocol::ServerEvent = serde_json::from_str(&line).unwrap();
            let crate::protocol::ServerEvent::History {
                messages,
                messages_base_count,
                ..
            } = event
            else {
                panic!("expected History")
            };
            assert_eq!(messages_base_count, None);
            assert_eq!(messages.len(), if visible.is_some() { 3 } else { 8 });
            assert_eq!(messages.last().unwrap().content, "message 7");
        }
        for visible in [4, 100] {
            let (stream, peer) = crate::transport::stream_pair().unwrap();
            let (_, write) = stream.into_split();
            let writer = Arc::new(Mutex::new(write));
            tokio::time::timeout(
                Duration::from_secs(2),
                super::super::handle_get_compacted_history(
                    2, session_id, &agent, &writer, visible, true,
                ),
            )
            .await
            .expect("paging must not wait for agent")
            .unwrap();
            let mut reader = tokio::io::BufReader::new(peer);
            let mut line = String::new();
            tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut line)
                .await
                .unwrap();
            let event: crate::protocol::ServerEvent = serde_json::from_str(&line).unwrap();
            let crate::protocol::ServerEvent::CompactedHistory {
                messages,
                compacted_total,
                compacted_visible,
                compacted_remaining,
                compacted_hidden_prompts,
                ..
            } = event
            else {
                panic!("expected CompactedHistory")
            };
            assert_eq!(compacted_total, 8);
            assert_eq!(compacted_visible, visible.min(8));
            assert_eq!(compacted_remaining, 8 - visible.min(8));
            assert_eq!(compacted_hidden_prompts, (8 - visible.min(8)) / 2);
            assert_eq!(messages.len(), if visible == 4 { 5 } else { 8 });
        }
        drop(guard);
    }
    match previous {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}
