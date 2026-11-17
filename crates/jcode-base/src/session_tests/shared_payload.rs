//! Regression tests for canonical transcript and copy-on-write request payloads.
use super::*;

#[test]
#[ignore = "allocator probe, run serially and separately from other tests"]
fn request_snapshot_allocator_probe() {
    fn live() -> u64 {
        crate::process_memory::allocator_info()
            .stats
            .and_then(|stats| stats.allocated_bytes)
            .expect("allocator statistics required for this opt-in probe")
    }
    let mut session = fixture();
    let source_bytes = session.messages[0]
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text, .. } => text.len(),
            _ => 0,
        })
        .sum::<usize>();
    let baseline = live();
    let legacy: Vec<Vec<Vec<ContentBlock>>> = (0..32)
        .map(|_| {
            session
                .messages
                .iter()
                .map(|message| message.content.to_vec())
                .collect()
        })
        .collect();
    std::hint::black_box(&legacy);
    let legacy_growth = live().saturating_sub(baseline);
    drop(legacy);
    let baseline = live();
    let shared: Vec<Vec<Message>> = (0..32).map(|_| session.messages_for_provider()).collect();
    std::hint::black_box(&shared);
    let shared_growth = live().saturating_sub(baseline);
    assert!(shared.iter().all(|messages| {
        messages[0]
            .content
            .shares_storage_with(&session.messages[0].content)
    }));
    println!(
        "request snapshots=32 source_bytes={source_bytes} legacy_allocator_growth={legacy_growth} shared_allocator_growth={shared_growth}; isolated ownership probe, not production RSS"
    );
}

fn fixture() -> Session {
    let mut session = Session::create_with_id("session_shared_payload".into(), None, None);
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "large transcript body ".repeat(32 * 1024),
            cache_control: None,
        }],
    );
    session.add_message(
        Role::Assistant,
        vec![ContentBlock::Text {
            text: "response".into(),
            cache_control: None,
        }],
    );
    session
}

fn assert_canonical_sharing(session: &mut Session) {
    let cached = session.messages_for_provider();
    let uncached = session.messages_for_provider_uncached();
    assert_eq!(cached.len(), session.messages.len());
    for ((stored, cached), uncached) in session.messages.iter().zip(&cached).zip(&uncached) {
        assert!(stored.content.shares_storage_with(&cached.content));
        assert!(stored.content.shares_storage_with(&uncached.content));
    }
    assert_eq!(
        serde_json::to_vec(&cached).unwrap(),
        serde_json::to_vec(&uncached).unwrap()
    );
}

#[test]
fn provider_cache_and_multiple_requests_share_canonical_bodies() {
    let mut session = fixture();
    assert_canonical_sharing(&mut session);
    let first = session.messages_for_provider();
    let second = session.messages_for_provider();
    let clone = session.clone();
    for index in 0..first.len() {
        assert!(
            first[index]
                .content
                .shares_storage_with(&second[index].content)
        );
        assert!(
            first[index]
                .content
                .shares_storage_with(&clone.messages[index].content)
        );
    }
    session.release_provider_messages_cache();
    assert_canonical_sharing(&mut session);
    assert!(
        first[0]
            .content
            .shares_storage_with(&session.messages[0].content)
    );
}

#[test]
fn request_mutation_detaches_only_changed_message_and_preserves_transcript() {
    let mut session = fixture();
    let original = serde_json::to_vec(&session.messages).unwrap();
    let mut request = session.messages_for_provider();
    let sibling = request.clone();
    if let ContentBlock::Text { text, .. } = &mut request[0].content[0] {
        text.push_str(" request-only edit");
    } else {
        panic!("expected text");
    }
    assert!(
        !request[0]
            .content
            .shares_storage_with(&session.messages[0].content)
    );
    assert!(
        request[1]
            .content
            .shares_storage_with(&session.messages[1].content)
    );
    assert!(
        sibling[0]
            .content
            .shares_storage_with(&session.messages[0].content)
    );
    assert_eq!(serde_json::to_vec(&session.messages).unwrap(), original);
    assert_eq!(
        serde_json::to_vec(&session.messages_for_provider()).unwrap(),
        serde_json::to_vec(&sibling).unwrap()
    );
}

#[test]
fn append_replace_and_truncate_refresh_cache_without_changing_inflight_snapshot() {
    let mut session = fixture();
    let inflight = session.messages_for_provider();
    let original = serde_json::to_vec(&inflight).unwrap();
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "appended".into(),
            cache_control: None,
        }],
    );
    assert_canonical_sharing(&mut session);
    assert_eq!(session.provider_messages().len(), 3);
    assert!(
        inflight[0]
            .content
            .shares_storage_with(&session.messages[0].content)
    );
    let mut replacement = session.messages.clone();
    replacement[0].content = vec![ContentBlock::Text {
        text: "replacement".into(),
        cache_control: None,
    }]
    .into();
    session.replace_messages(replacement);
    assert_canonical_sharing(&mut session);
    assert!(
        !inflight[0]
            .content
            .shares_storage_with(&session.messages[0].content)
    );
    session.truncate_messages(1);
    assert_canonical_sharing(&mut session);
    assert_eq!(session.provider_messages().len(), 1);
    assert_eq!(serde_json::to_vec(&inflight).unwrap(), original);
}

#[test]
fn shared_transcript_content_preserves_legacy_json_arrays() -> anyhow::Result<()> {
    let mut session = fixture();
    let request = session.messages_for_provider();
    for (stored, message) in session.messages.iter().zip(&request) {
        let owned: Vec<ContentBlock> = stored.content.iter().cloned().collect();
        assert_eq!(
            serde_json::to_vec(&owned)?,
            serde_json::to_vec(&stored.content)?
        );
        assert_eq!(
            serde_json::to_vec(&owned)?,
            serde_json::to_vec(&message.content)?
        );
        let mut legacy_message = serde_json::to_value(message)?;
        legacy_message["content"] = serde_json::to_value(&owned)?;
        assert_eq!(
            serde_json::to_vec(message)?,
            serde_json::to_vec(&serde_json::from_value::<Message>(legacy_message)?)?
        );
        let restored: StoredMessage = serde_json::from_slice(&serde_json::to_vec(stored)?)?;
        assert_eq!(serde_json::to_vec(&restored)?, serde_json::to_vec(stored)?);
    }
    Ok(())
}

#[test]
fn session_image_recovery_detaches_changed_body_and_invalidates_provider_cache() {
    let mut session = fixture();
    session.add_message(
        Role::User,
        vec![ContentBlock::Image {
            media_type: "image/png".into(),
            data: "a".repeat(4096),
        }],
    );
    let inflight = session.messages_for_provider();
    assert_eq!(session.strip_oversized_images(4096), 0);
    assert!(
        inflight[2]
            .content
            .shares_storage_with(&session.messages[2].content)
    );
    assert_eq!(session.strip_oversized_images(0), 1);
    assert!(matches!(inflight[2].content[0], ContentBlock::Image { .. }));
    assert!(
        !inflight[2]
            .content
            .shares_storage_with(&session.messages[2].content)
    );
    assert_canonical_sharing(&mut session);
    assert!(matches!(
        session.provider_messages()[2].content[0],
        ContentBlock::Text { .. }
    ));
}
