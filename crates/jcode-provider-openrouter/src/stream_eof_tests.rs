use super::*;

#[test]
fn unfold_eof_remains_terminal_after_final_events() {
    for chunks in [
        Vec::new(),
        vec![Bytes::from_static(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
        )],
    ] {
        let inner = futures::stream::unfold(chunks.into_iter(), |mut chunks| async move {
            chunks.next().map(|chunk| (Ok(chunk), chunks))
        });
        let mut stream =
            OpenRouterStream::new(inner, "test-model".into(), Arc::new(Mutex::new(None)));
        let events = futures::executor::block_on(async {
            let mut events = Vec::new();
            while let Some(event) = stream.next().await {
                events.push(event.expect("valid SSE"));
            }
            events
        });
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, StreamEvent::MessageEnd { .. }))
                .count(),
            1
        );
        for _ in 0..3 {
            assert!(futures::executor::block_on(stream.next()).is_none());
        }
    }
}
