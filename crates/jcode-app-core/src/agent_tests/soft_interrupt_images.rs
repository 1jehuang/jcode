use super::*;

#[tokio::test]
async fn queued_soft_interrupt_images_are_injected_as_image_blocks() {
    let provider: Arc<dyn Provider> = Arc::new(NativeAutoCompactionProvider);
    let registry = Registry::new(provider.clone()).await;
    let _env = jcode_base::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let mut agent = Agent::new(provider, registry);

    for content in ["look at this", ""] {
        agent.queue_soft_interrupt_with_id(
            content.into(),
            vec![("image/png".into(), "ZmFrZQ==".into())],
            false,
            SoftInterruptSource::User,
            Some("image-client".into()),
        );
        let events = Agent::build_soft_interrupt_events(agent.inject_soft_interrupts(), "D", None);
        assert!(matches!(
            &events[..],
            [ServerEvent::SoftInterruptInjected { client_ids, .. }]
                if client_ids == &["image-client"]
        ));
        let message = agent.session.messages.last().unwrap();
        assert!(matches!(
            &message.content[0],
            ContentBlock::Image { media_type, data }
                if media_type == "image/png" && data == "ZmFrZQ=="
        ));
        if content.is_empty() {
            assert_eq!(message.content.len(), 1);
        } else {
            assert!(matches!(
                &message.content[1],
                ContentBlock::Text { text, .. } if text == content
            ));
        }
    }
}

#[tokio::test]
async fn soft_interrupt_restore_preserves_disk_input_when_queue_is_unavailable() {
    let provider: Arc<dyn Provider> = Arc::new(NativeAutoCompactionProvider);
    let registry = Registry::new(provider.clone()).await;
    let _env = jcode_base::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let agent = Agent::new(provider, registry);
    let pending = jcode_agent_runtime::SoftInterruptMessage {
        content: String::new(),
        images: vec![("image/png".into(), "ZmFrZQ==".into())],
        urgent: false,
        source: SoftInterruptSource::User,
        client_id: Some("image-client".into()),
    };
    crate::soft_interrupt_store::append(agent.session_id(), pending).unwrap();
    let queue = agent.soft_interrupt_queue();
    assert!(
        std::panic::catch_unwind(|| {
            let _guard = queue.lock().unwrap();
            panic!("poison the test queue");
        })
        .is_err()
    );
    assert_eq!(agent.restore_persisted_soft_interrupts(), 0);
    let pending = crate::soft_interrupt_store::load(agent.session_id()).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].client_id.as_deref(), Some("image-client"));
    assert_eq!(pending[0].images, [("image/png".into(), "ZmFrZQ==".into())]);
}
